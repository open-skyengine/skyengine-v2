use std::{
    env,
    ffi::OsStr,
    fs::File,
    io::{BufWriter, Write},
    net::{Ipv4Addr, SocketAddrV4},
    path::{Path, PathBuf},
    process::ExitCode,
    time::Duration,
};

use serde_json::json;
use skyengine_core::{
    DeviceDate, DnsMapping, Framebuffer, Package, PlatformDisplay, ResourceLimits, Result, Runtime,
    RuntimeConfig,
};
use skyengine_sdl::SdlDisplay;

#[cfg(unix)]
mod e2e;

fn main() -> ExitCode {
    match real_main() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("skyengine: {error}");
            ExitCode::FAILURE
        }
    }
}

fn real_main() -> Result<()> {
    let mut args = env::args_os().skip(1).collect::<Vec<_>>();
    if args.is_empty() || args[0] == "--help" || args[0] == "-h" {
        print_help();
        return Ok(());
    }
    let command = args.remove(0);
    match command.to_str() {
        Some("inspect") => inspect(args),
        Some("run") => run(args),
        _ => {
            args.insert(0, command);
            run(args)
        }
    }
}

fn inspect(mut args: Vec<std::ffi::OsString>) -> Result<()> {
    let json_output = take_flag(&mut args, "--json");
    if args.len() != 1 {
        return Err(skyengine_core::Error::Config(
            "usage: skyengine inspect [--json] <app.mrp>".into(),
        ));
    }
    let package = Package::open(PathBuf::from(&args[0]), ResourceLimits::default())?;
    if json_output {
        let output = json!({
            "path": package.path(),
            "header": package.header(),
            "entries": package.entries(),
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&output)
                .map_err(|error| skyengine_core::Error::Config(error.to_string()))?
        );
    } else {
        println!("MRP: {}", package.path().display());
        println!("name: {}", package.header().display_name());
        println!(
            "app-id: {}  version: {}  screen: {}x{}",
            package.header().app_id,
            package.header().version,
            package.header().screen_width,
            package.header().screen_height
        );
        for entry in package.entries() {
            println!(
                "{:>8} bytes  {:<5}  {}",
                entry.stored_len,
                if entry.compressed { "gzip" } else { "raw" },
                entry.display_name()
            );
        }
    }
    Ok(())
}

fn run(mut args: Vec<std::ffi::OsString>) -> Result<()> {
    let entry = take_option(&mut args, "--entry")?.unwrap_or_else(|| "start.mr".into());
    let work_dir = take_option(&mut args, "--work-dir")?.unwrap_or_else(|| ".".into());
    let font = take_option(&mut args, "--font")?;
    let screen = take_option(&mut args, "--screen")?.unwrap_or_else(|| "240x320".into());
    let memory = take_option(&mut args, "--memory")?.unwrap_or_else(|| "1M".into());
    let dns_map = take_option(&mut args, "--dns-map")?;
    let device_date =
        take_option(&mut args, "--device-date")?.or_else(|| env::var("SKYENGINE_DEVICE_DATE").ok());
    let frame_output = take_option(&mut args, "--frame-output")?;
    let headless = take_flag(&mut args, "--headless") || frame_output.is_some();
    if args.len() != 1 {
        return Err(skyengine_core::Error::Config(
            "usage: skyengine run [options] <app.mrp>".into(),
        ));
    }
    let (width, height) = parse_screen(&screen)?;
    let memory_limit = parse_memory(&memory)?;
    let mut config = RuntimeConfig::for_app(PathBuf::from(&args[0]));
    config.entry = entry.as_bytes().to_vec();
    config.work_dir = PathBuf::from(work_dir);
    if let Some(font) = font {
        config.font_path = PathBuf::from(font);
    }
    config.screen_width = width;
    config.screen_height = height;
    config.memory_limit = memory_limit;
    apply_dns_map_option(&mut config, dns_map.as_deref())?;
    if let Some(device_date) = device_date {
        config.device_date = parse_device_date(&device_date)?;
    }

    if headless {
        let mut runtime = Runtime::load(config, Box::new(HeadlessDisplay))?;
        runtime.start()?;
        let output = frame_output.unwrap_or_else(|| "skyengine-frame.ppm".into());
        write_ppm(Path::new(&output), runtime.framebuffer())?;
        println!(
            "rendered {}x{} frame to {}",
            width,
            height,
            Path::new(&output).display()
        );
        return Ok(());
    }

    if let Some(socket_path) = env::var_os("SKYENGINE_E2E_SOCKET") {
        #[cfg(unix)]
        {
            let display: Box<dyn PlatformDisplay> =
                if e2e_sdl_preview_enabled(env::var_os("SDL_VIDEODRIVER").as_deref()) {
                    // Initialize SDL before binding the E2E socket so a failed preview
                    // does not leave a detached control-server thread behind.
                    let preview = SdlDisplay::new(width, height, 2)?;
                    let audio = preview.audio_player();
                    let control = e2e::E2eDisplay::new(PathBuf::from(socket_path), width, height)?;
                    let display = Box::new(E2eSdlDisplay { control, preview });
                    return match audio {
                        Some(audio) => Runtime::load_with_audio(config, display, Box::new(audio))?,
                        None => Runtime::load(config, display)?,
                    }
                    .run();
                } else {
                    Box::new(e2e::E2eDisplay::new(
                        PathBuf::from(socket_path),
                        width,
                        height,
                    )?)
                };
            return Runtime::load(config, display)?.run();
        }
        #[cfg(not(unix))]
        {
            let _ = socket_path;
            return Err(skyengine_core::Error::Config(
                "SKYENGINE_E2E_SOCKET is not supported on this host".into(),
            ));
        }
    }

    let display = SdlDisplay::new(width, height, 2)?;
    let audio = display.audio_player();
    match audio {
        Some(audio) => Runtime::load_with_audio(config, Box::new(display), Box::new(audio))?,
        None => Runtime::load(config, Box::new(display))?,
    }
    .run()
}

fn e2e_sdl_preview_enabled(video_driver: Option<&OsStr>) -> bool {
    video_driver != Some(OsStr::new("dummy"))
}

#[cfg(unix)]
struct E2eSdlDisplay {
    control: e2e::E2eDisplay,
    preview: SdlDisplay,
}

#[cfg(unix)]
impl PlatformDisplay for E2eSdlDisplay {
    fn resize(&mut self, width: u16, height: u16) -> Result<()> {
        self.preview.resize(width, height)?;
        self.control.resize(width, height)
    }

    fn present(&mut self, framebuffer: &Framebuffer) -> Result<()> {
        // Publish the frame to the test only after SDL has presented it, keeping
        // automated steps and the visible preview on the same frame.
        self.preview.present(framebuffer)?;
        self.control.present(framebuffer)
    }

    fn poll_event(&mut self) -> Result<Option<skyengine_core::DisplayEvent>> {
        if let Some(event) = self.control.poll_event()? {
            return Ok(Some(event));
        }
        self.preview.poll_event()
    }

    fn wait_timeout(&mut self, milliseconds: u32) {
        // E2E wait_timeout also wakes early for scheduled key/button releases.
        self.control.wait_timeout(milliseconds);
    }
}

fn take_flag(args: &mut Vec<std::ffi::OsString>, name: &str) -> bool {
    if let Some(index) = args.iter().position(|argument| argument == name) {
        args.remove(index);
        true
    } else {
        false
    }
}

fn take_option(args: &mut Vec<std::ffi::OsString>, name: &str) -> Result<Option<String>> {
    let Some(index) = args.iter().position(|argument| argument == name) else {
        return Ok(None);
    };
    if index + 1 >= args.len() {
        return Err(skyengine_core::Error::Config(format!(
            "{name} requires a value"
        )));
    }
    args.remove(index);
    let value = args
        .remove(index)
        .into_string()
        .map_err(|_| skyengine_core::Error::Config(format!("{name} value is not valid Unicode")))?;
    Ok(Some(value))
}

fn parse_screen(value: &str) -> Result<(u16, u16)> {
    let (width, height) = value.split_once('x').ok_or_else(|| {
        skyengine_core::Error::Config(format!("invalid screen {value:?}; expected WIDTHxHEIGHT"))
    })?;
    let width = width
        .parse::<u16>()
        .map_err(|_| skyengine_core::Error::Config(format!("invalid screen width {width:?}")))?;
    let height = height
        .parse::<u16>()
        .map_err(|_| skyengine_core::Error::Config(format!("invalid screen height {height:?}")))?;
    if width == 0 || height == 0 {
        return Err(skyengine_core::Error::Config(
            "screen dimensions must be non-zero".into(),
        ));
    }
    Ok((width, height))
}

fn parse_memory(value: &str) -> Result<u32> {
    let mebibytes = match value {
        "1M" => 1,
        "2M" => 2,
        "4M" => 4,
        "6M" => 6,
        "8M" => 8,
        "16M" => 16,
        _ => {
            return Err(skyengine_core::Error::Config(format!(
                "invalid memory size {value:?}; expected 1M, 2M, 4M, 6M, 8M, or 16M"
            )));
        }
    };
    Ok(mebibytes * 1024 * 1024)
}

fn parse_dns_mappings(value: &str) -> Result<Vec<DnsMapping>> {
    let mut mappings = Vec::new();
    for item in value
        .split(';')
        .map(str::trim)
        .filter(|item| !item.is_empty())
    {
        let (source, target) = item.split_once("->").ok_or_else(|| {
            skyengine_core::Error::Config(format!(
                "invalid DNS mapping {item:?}; expected SOURCE->IPv4[:PORT]"
            ))
        })?;
        let source = source.trim().trim_end_matches('.').to_ascii_lowercase();
        let target = target.trim();
        if source.is_empty() || !source.is_ascii() {
            return Err(skyengine_core::Error::Config(format!(
                "invalid DNS mapping source {source:?}"
            )));
        }
        if mappings
            .iter()
            .any(|mapping: &DnsMapping| mapping.source == source)
        {
            return Err(skyengine_core::Error::Config(format!(
                "duplicate DNS mapping source {source:?}"
            )));
        }
        let (address, port) = match target.parse::<Ipv4Addr>() {
            Ok(address) => (address, None),
            Err(_) => {
                let endpoint = target.parse::<SocketAddrV4>().map_err(|_| {
                    skyengine_core::Error::Config(format!(
                        "invalid DNS mapping target {target:?}; expected IPv4[:PORT]"
                    ))
                })?;
                (*endpoint.ip(), Some(endpoint.port()))
            }
        };
        mappings.push(DnsMapping {
            source,
            address,
            port,
        });
    }
    Ok(mappings)
}

fn apply_dns_map_option(config: &mut RuntimeConfig, value: Option<&str>) -> Result<()> {
    if let Some(value) = value {
        config.dns_mappings = parse_dns_mappings(value)?;
    }
    Ok(())
}

fn parse_device_date(value: &str) -> Result<DeviceDate> {
    if value == "host" {
        return Ok(DeviceDate::host_today());
    }
    let invalid = || {
        skyengine_core::Error::Config(format!(
            "invalid device date '{value}'; expected YYYY-M-D or host"
        ))
    };
    let mut parts = value.split('-');
    let year = parts.next().ok_or_else(invalid)?;
    let month = parts.next().ok_or_else(invalid)?;
    let day = parts.next().ok_or_else(invalid)?;
    if parts.next().is_some() || year.is_empty() || month.is_empty() || day.is_empty() {
        return Err(invalid());
    }
    let year = year.parse::<u16>().map_err(|_| invalid())?;
    let month = month.parse::<u8>().map_err(|_| invalid())?;
    let day = day.parse::<u8>().map_err(|_| invalid())?;
    DeviceDate::new(year, month, day).ok_or_else(invalid)
}

fn write_ppm(path: &Path, framebuffer: &Framebuffer) -> Result<()> {
    let file = File::create(path).map_err(|source| skyengine_core::Error::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let mut output = BufWriter::new(file);
    writeln!(
        output,
        "P6\n{} {}\n255",
        framebuffer.width(),
        framebuffer.height()
    )
    .map_err(|source| skyengine_core::Error::Io {
        path: path.to_path_buf(),
        source,
    })?;
    for pixel in framebuffer.pixels() {
        let red = ((pixel >> 11) & 0x1f) as u8;
        let green = ((pixel >> 5) & 0x3f) as u8;
        let blue = (pixel & 0x1f) as u8;
        let rgb = [red << 3, green << 2, blue << 3];
        output
            .write_all(&rgb)
            .map_err(|source| skyengine_core::Error::Io {
                path: path.to_path_buf(),
                source,
            })?;
    }
    output.flush().map_err(|source| skyengine_core::Error::Io {
        path: path.to_path_buf(),
        source,
    })
}

struct HeadlessDisplay;

impl PlatformDisplay for HeadlessDisplay {
    fn present(&mut self, _framebuffer: &Framebuffer) -> Result<()> {
        Ok(())
    }

    fn poll_event(&mut self) -> Result<Option<skyengine_core::DisplayEvent>> {
        Ok(None)
    }

    fn wait_timeout(&mut self, milliseconds: u32) {
        std::thread::sleep(Duration::from_millis(u64::from(milliseconds)));
    }
}

fn print_help() {
    println!(
        "SkyEngine v0.1\n\n\
         Usage:\n  \
         skyengine inspect [--json] <app.mrp>\n  \
         skyengine run [--entry NAME] [--work-dir DIR] [--font FILE]\n  \
                       [--screen WIDTHxHEIGHT] [--memory SIZE] [--dns-map MAP]\n  \
                       [--device-date YYYY-M-D|host]\n  \
                       [--headless]\n  \
                       [--frame-output FILE.ppm] <app.mrp>\n  \
         skyengine [run options] <app.mrp>\n\n\
         --work-dir is the device root; installed MRP files live in mythroad/.\n  \
         Relative font paths are resolved from --work-dir.\n  \
         DNS MAP is a semicolon-separated SOURCE->IPv4[:PORT] list.\n  \
         DNS MAP maps the default Skymobi hosts to 159.75.119.124.\n  \
         Connections to 10.0.0.172 use the built-in WAP proxy unless explicitly mapped.\n  \
         Device date defaults to 2012-6-20 or SKYENGINE_DEVICE_DATE when set.\n  \
         Memory SIZE is one of 1M, 2M, 4M, 6M, 8M, or 16M.\n  \
         The default font is mythroad/system/gb16.uc2."
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn e2e_sdl_preview_is_disabled_only_for_the_dummy_driver() {
        assert!(!e2e_sdl_preview_enabled(Some(OsStr::new("dummy"))));
        assert!(e2e_sdl_preview_enabled(Some(OsStr::new("x11"))));
        assert!(e2e_sdl_preview_enabled(Some(OsStr::new("wayland"))));
        assert!(e2e_sdl_preview_enabled(None));
    }

    #[test]
    fn parses_supported_memory_profiles() {
        for (profile, expected_mebibytes) in [
            ("1M", 1),
            ("2M", 2),
            ("4M", 4),
            ("6M", 6),
            ("8M", 8),
            ("16M", 16),
        ] {
            assert_eq!(
                parse_memory(profile).unwrap(),
                expected_mebibytes * 1024 * 1024
            );
        }
    }

    #[test]
    fn rejects_ambiguous_memory_sizes() {
        let error = parse_memory("2048K").unwrap_err();
        assert!(
            error
                .to_string()
                .contains("expected 1M, 2M, 4M, 6M, 8M, or 16M")
        );
    }

    #[test]
    fn parses_dns_mappings_with_optional_port_routes() {
        assert_eq!(
            parse_dns_mappings(
                "SPD.SkyMobiApp.com.->159.75.119.124;211.155.236.18->127.0.0.1:8088"
            )
            .unwrap(),
            [
                DnsMapping {
                    source: "spd.skymobiapp.com".into(),
                    address: Ipv4Addr::new(159, 75, 119, 124),
                    port: None,
                },
                DnsMapping {
                    source: "211.155.236.18".into(),
                    address: Ipv4Addr::LOCALHOST,
                    port: Some(8088),
                },
            ]
        );
    }

    #[test]
    fn rejects_malformed_dns_mappings() {
        assert!(parse_dns_mappings("example.com=127.0.0.1").is_err());
        assert!(parse_dns_mappings("example.com->localhost").is_err());
        assert!(parse_dns_mappings("a.example->127.0.0.1;a.example->127.0.0.2").is_err());
    }

    #[test]
    fn dns_map_option_overrides_defaults_only_when_present() {
        let mut config = RuntimeConfig::for_app("app.mrp");
        let defaults = config.dns_mappings.clone();

        apply_dns_map_option(&mut config, None).unwrap();
        assert_eq!(config.dns_mappings, defaults);

        apply_dns_map_option(&mut config, Some("example.com->127.0.0.1:8080")).unwrap();
        assert_eq!(
            config.dns_mappings,
            [DnsMapping {
                source: "example.com".into(),
                address: Ipv4Addr::LOCALHOST,
                port: Some(8080),
            }]
        );
    }

    #[test]
    fn parses_valid_device_dates_and_host_mode() {
        assert_eq!(
            parse_device_date("2000-2-29").unwrap(),
            DeviceDate::new(2000, 2, 29).unwrap()
        );
        assert_eq!(
            parse_device_date("2012-06-20").unwrap(),
            DeviceDate::new(2012, 6, 20).unwrap()
        );
        assert!(parse_device_date("host").unwrap().year >= 1970);
    }

    #[test]
    fn rejects_invalid_device_dates() {
        for value in [
            "2011-2-29",
            "1900-2-29",
            "2011-13-1",
            "2011-0-1",
            "2011-1-0",
            "2011-1-1x",
        ] {
            let error = parse_device_date(value).unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains(&format!("invalid device date '{value}'"))
            );
        }
    }
}
