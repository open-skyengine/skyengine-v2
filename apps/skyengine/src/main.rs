use std::{
    env,
    fs::File,
    io::{BufWriter, Write},
    path::{Path, PathBuf},
    process::ExitCode,
    time::Duration,
};

use serde_json::json;
use skyengine_core::{
    Framebuffer, Package, PlatformDisplay, ResourceLimits, Result, Runtime, RuntimeConfig,
};
use skyengine_sdl::SdlDisplay;

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
        Some(other) => Err(skyengine_core::Error::Config(format!(
            "unknown command {other:?}; expected inspect or run"
        ))),
        None => Err(skyengine_core::Error::Config(
            "command is not valid Unicode".into(),
        )),
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
    let font =
        take_option(&mut args, "--font")?.unwrap_or_else(|| "test/fixtures/fonts/gb16.uc2".into());
    let screen = take_option(&mut args, "--screen")?.unwrap_or_else(|| "240x320".into());
    let frame_output = take_option(&mut args, "--frame-output")?;
    let headless = take_flag(&mut args, "--headless") || frame_output.is_some();
    if args.len() != 1 {
        return Err(skyengine_core::Error::Config(
            "usage: skyengine run [options] <app.mrp>".into(),
        ));
    }
    let (width, height) = parse_screen(&screen)?;
    let mut config = RuntimeConfig::for_app(PathBuf::from(&args[0]));
    config.entry = entry.as_bytes().to_vec();
    config.work_dir = PathBuf::from(work_dir);
    config.font_path = PathBuf::from(font);
    config.screen_width = width;
    config.screen_height = height;

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

    let display = SdlDisplay::new(width, height, 2)?;
    Runtime::load(config, Box::new(display))?.run()
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
                       [--screen WIDTHxHEIGHT] [--headless]\n  \
                       [--frame-output FILE.ppm] <app.mrp>\n\n\
         The default font is test/fixtures/fonts/gb16.uc2."
    );
}
