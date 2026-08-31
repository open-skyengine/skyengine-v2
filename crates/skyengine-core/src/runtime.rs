use std::{
    fs,
    net::{Ipv4Addr, SocketAddrV4},
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use crate::{
    DisplayEvent, Error, Framebuffer, Package, PlatformAudio, PlatformDisplay, ResourceLimits,
    Result, SilentAudio,
    mr::{
        MrHostConfig, MrVm,
        value::Value,
        vm::{LifecycleError, LifecycleOutcome},
    },
    wap_proxy::{WAP_PROXY_ADDRESS, WapProxyService},
};

pub const DEFAULT_MEMORY_LIMIT: u32 = 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuntimeState {
    Created,
    Loaded,
    Running,
    Paused,
    Stopping,
    Stopped,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DnsMapping {
    pub source: String,
    pub address: Ipv4Addr,
    pub port: Option<u16>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DeviceDate {
    pub year: u16,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
}

impl DeviceDate {
    pub const BASELINE: Self = Self {
        year: 2012,
        month: 6,
        day: 20,
        hour: 0,
        minute: 0,
        second: 0,
    };

    pub fn new(year: u16, month: u8, day: u8) -> Option<Self> {
        let max_day = match month {
            1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
            4 | 6 | 9 | 11 => 30,
            2 if is_leap_year(year) => 29,
            2 => 28,
            _ => return None,
        };
        (year != 0 && day != 0 && day <= max_day).then_some(Self {
            year,
            month,
            day,
            hour: 0,
            minute: 0,
            second: 0,
        })
    }

    pub fn with_time_of_day(
        year: u16,
        month: u8,
        day: u8,
        hour: u8,
        minute: u8,
        second: u8,
    ) -> Option<Self> {
        if hour > 23 || minute > 59 || second > 59 {
            return None;
        }
        let mut value = Self::new(year, month, day)?;
        value.hour = hour;
        value.minute = minute;
        value.second = second;
        Some(value)
    }

    pub fn host_today() -> Self {
        let mut value = Self::host_now();
        value.hour = 0;
        value.minute = 0;
        value.second = 0;
        value
    }

    pub fn host_now() -> Self {
        if let Some(value) = host_local_datetime() {
            return value;
        }
        let timestamp = match SystemTime::now().duration_since(UNIX_EPOCH) {
            Ok(duration) => duration.as_secs() as i64,
            Err(error) => -((error.duration().as_secs()) as i64),
        };
        Self::from_unix_timestamp(timestamp).unwrap_or(Self::BASELINE)
    }

    pub fn from_unix_timestamp(timestamp: i64) -> Option<Self> {
        let days = timestamp.div_euclid(86_400);
        let seconds_of_day = timestamp.rem_euclid(86_400);
        let mut value = civil_date_from_unix_days(days);
        value.hour = (seconds_of_day / 3_600) as u8;
        value.minute = ((seconds_of_day % 3_600) / 60) as u8;
        value.second = (seconds_of_day % 60) as u8;
        Some(value)
    }

    pub(crate) fn advance(self, elapsed: Duration) -> Self {
        let timestamp = self
            .unix_timestamp()
            .saturating_add(i64::try_from(elapsed.as_secs()).unwrap_or(i64::MAX));
        Self::from_unix_timestamp(timestamp).unwrap_or(Self::BASELINE)
    }

    pub(crate) fn weekday(self) -> u8 {
        (self.unix_days() + 4).rem_euclid(7) as u8
    }

    fn unix_timestamp(self) -> i64 {
        self.unix_days() * 86_400
            + i64::from(self.hour) * 3_600
            + i64::from(self.minute) * 60
            + i64::from(self.second)
    }

    fn unix_days(self) -> i64 {
        let mut year = i64::from(self.year);
        let month = i64::from(self.month);
        let day = i64::from(self.day);
        year -= i64::from(month <= 2);
        let era = year.div_euclid(400);
        let year_of_era = year - era * 400;
        let shifted_month = month + if month > 2 { -3 } else { 9 };
        let day_of_year = (153 * shifted_month + 2) / 5 + day - 1;
        let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
        era * 146_097 + day_of_era - 719_468
    }
}

#[cfg(unix)]
fn host_local_datetime() -> Option<DeviceDate> {
    use std::{
        ffi::{c_char, c_int, c_long},
        mem::MaybeUninit,
        ptr,
    };

    #[repr(C)]
    struct LocalTime {
        second: c_int,
        minute: c_int,
        hour: c_int,
        day: c_int,
        month_from_zero: c_int,
        year_from_1900: c_int,
        _weekday: c_int,
        _year_day: c_int,
        _daylight_saving: c_int,
        _utc_offset: c_long,
        _zone: *const c_char,
    }

    unsafe extern "C" {
        fn time(timer: *mut c_long) -> c_long;
        fn localtime_r(timer: *const c_long, result: *mut LocalTime) -> *mut LocalTime;
    }

    let timestamp = unsafe { time(ptr::null_mut()) };
    let mut local = MaybeUninit::<LocalTime>::uninit();
    if unsafe { localtime_r(&timestamp, local.as_mut_ptr()) }.is_null() {
        return None;
    }
    let local = unsafe { local.assume_init() };
    DeviceDate::with_time_of_day(
        u16::try_from(local.year_from_1900.checked_add(1900)?).ok()?,
        u8::try_from(local.month_from_zero.checked_add(1)?).ok()?,
        u8::try_from(local.day).ok()?,
        u8::try_from(local.hour).ok()?,
        u8::try_from(local.minute).ok()?,
        u8::try_from(local.second).ok()?,
    )
}

#[cfg(windows)]
fn host_local_datetime() -> Option<DeviceDate> {
    use std::mem::MaybeUninit;

    #[repr(C)]
    struct SystemTime {
        year: u16,
        month: u16,
        _weekday: u16,
        day: u16,
        hour: u16,
        minute: u16,
        second: u16,
        _milliseconds: u16,
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        #[link_name = "GetLocalTime"]
        fn get_local_time(system_time: *mut SystemTime);
    }

    let mut local = MaybeUninit::<SystemTime>::uninit();
    unsafe { get_local_time(local.as_mut_ptr()) };
    let local = unsafe { local.assume_init() };
    DeviceDate::with_time_of_day(
        local.year,
        u8::try_from(local.month).ok()?,
        u8::try_from(local.day).ok()?,
        u8::try_from(local.hour).ok()?,
        u8::try_from(local.minute).ok()?,
        u8::try_from(local.second).ok()?,
    )
}

#[cfg(not(any(unix, windows)))]
fn host_local_datetime() -> Option<DeviceDate> {
    None
}

impl Default for DeviceDate {
    fn default() -> Self {
        Self::BASELINE
    }
}

fn is_leap_year(year: u16) -> bool {
    year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400))
}

fn civil_date_from_unix_days(days: i64) -> DeviceDate {
    let shifted_days = days + 719_468;
    let era = shifted_days.div_euclid(146_097);
    let day_of_era = shifted_days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    DeviceDate {
        year: year as u16,
        month: month as u8,
        day: day as u8,
        hour: 0,
        minute: 0,
        second: 0,
    }
}

#[derive(Clone, Debug)]
pub struct RuntimeConfig {
    pub app_path: PathBuf,
    pub entry: Vec<u8>,
    pub work_dir: PathBuf,
    pub font_path: PathBuf,
    pub memory_limit: u32,
    pub screen_width: u16,
    pub screen_height: u16,
    pub dns_mappings: Vec<DnsMapping>,
    pub device_date: DeviceDate,
    pub limits: ResourceLimits,
}

impl RuntimeConfig {
    pub fn for_app(app_path: impl Into<PathBuf>) -> Self {
        Self {
            app_path: app_path.into(),
            entry: b"start.mr".to_vec(),
            work_dir: PathBuf::from("."),
            font_path: PathBuf::from("mythroad/system/gb16.uc2"),
            memory_limit: DEFAULT_MEMORY_LIMIT,
            screen_width: 240,
            screen_height: 320,
            dns_mappings: default_dns_mappings(),
            device_date: DeviceDate::default(),
            limits: ResourceLimits::default(),
        }
    }
}

fn default_dns_mappings() -> Vec<DnsMapping> {
    ["rop.skymobiapp.com", "spd.skymobiapp.com", "wap.skmeg.com"]
        .into_iter()
        .map(|source| DnsMapping {
            source: source.into(),
            address: Ipv4Addr::new(159, 75, 119, 124),
            port: None,
        })
        .collect()
}

pub struct Runtime {
    state: RuntimeState,
    entry: Vec<u8>,
    vm: MrVm,
    _wap_proxy: Option<WapProxyService>,
}

impl Runtime {
    pub fn load(config: RuntimeConfig, display: Box<dyn PlatformDisplay>) -> Result<Self> {
        Self::load_with_audio(config, display, Box::new(SilentAudio))
    }

    pub fn load_with_audio(
        config: RuntimeConfig,
        display: Box<dyn PlatformDisplay>,
        audio: Box<dyn PlatformAudio>,
    ) -> Result<Self> {
        let package = Arc::new(Package::open(&config.app_path, config.limits.clone())?);
        package.resolve(&config.entry)?;

        let font_path = resolve_font_path(&config.work_dir, &config.font_path);
        let font = fs::read(&font_path).map_err(|source| Error::Io {
            path: font_path.clone(),
            source,
        })?;
        const GB16_BYTES: usize = 65_536 * 32;
        if font.len() < GB16_BYTES {
            return Err(Error::Config(format!(
                "font {} is {} bytes; gb16.uc2 needs at least {GB16_BYTES}",
                font_path.display(),
                font.len()
            )));
        }
        let framebuffer = Framebuffer::new(config.screen_width, config.screen_height)?;
        let (wap_proxy, wap_proxy_endpoint) = start_wap_proxy(&config.dns_mappings)?;
        let vm = MrVm::new(
            package,
            framebuffer,
            display,
            audio,
            MrHostConfig {
                work_dir: config.work_dir,
                font: font.into(),
                memory_limit: config.memory_limit,
                dns_mappings: config.dns_mappings.into(),
                device_date: config.device_date,
                wap_proxy_endpoint,
            },
            config.limits,
        );
        Ok(Self {
            state: RuntimeState::Loaded,
            entry: config.entry,
            vm,
            _wap_proxy: wap_proxy,
        })
    }

    pub fn state(&self) -> RuntimeState {
        self.state
    }

    pub fn framebuffer(&self) -> &Framebuffer {
        self.vm.framebuffer()
    }

    /// Returns the text of the native editor currently owned by the guest.
    ///
    /// Platform frontends use this to present their own text input UI. `None`
    /// means that the runtime is not waiting for editor input.
    pub fn active_editor_text(&self) -> Option<String> {
        self.vm.active_editor_text()
    }

    pub fn motion_active(&self) -> bool {
        self.state == RuntimeState::Running && self.vm.motion_active()
    }

    pub fn start(&mut self) -> Result<()> {
        if self.state != RuntimeState::Loaded {
            return Err(Error::MrFault(format!(
                "cannot start runtime in {:?}",
                self.state
            )));
        }
        self.vm.run_entry(&self.entry)?;
        self.state = RuntimeState::Running;
        self.apply_lifecycle_request()?;
        Ok(())
    }

    pub fn run(&mut self) -> Result<()> {
        self.start()?;
        while matches!(self.state, RuntimeState::Running | RuntimeState::Paused) {
            self.tick()?;
            let wait = self
                .vm
                .native_timer_due_in()
                .map(|delay| delay.as_millis().clamp(1, 10) as u32)
                .unwrap_or(10);
            self.vm.display_mut().wait_timeout(wait);
        }
        self.state = RuntimeState::Stopped;
        Ok(())
    }

    pub fn tick(&mut self) -> Result<()> {
        let dispatched_completion = self.state == RuntimeState::Running
            && (self.vm.dispatch_pending_platform_event()?
                || self.vm.dispatch_external_action_completion()?);
        if dispatched_completion {
            self.apply_lifecycle_request()?;
            if self.state == RuntimeState::Stopping {
                return Ok(());
            }
        }
        while let Some(event) = self.vm.display_mut().poll_event()? {
            self.dispatch(event)?;
            if self.state == RuntimeState::Stopping {
                return Ok(());
            }
        }
        if self.state == RuntimeState::Running {
            self.vm.dispatch_native_timer()?;
            self.apply_lifecycle_request()?;
        }
        Ok(())
    }

    pub fn stop(&mut self) {
        if !matches!(self.state, RuntimeState::Stopping | RuntimeState::Stopped) {
            self.state = RuntimeState::Stopping;
        }
        self.vm.stop_audio();
    }

    fn dispatch(&mut self, event: DisplayEvent) -> Result<()> {
        match event {
            DisplayEvent::Quit => self.stop(),
            DisplayEvent::Key { code, pressed } if self.state == RuntimeState::Running => {
                if let Some((event, parameter0, parameter1)) =
                    self.vm.route_key_event(code, pressed)?
                {
                    let result = self.vm.call_global(
                        b"dealevent",
                        vec![
                            Value::Number(f64::from(event)),
                            Value::Number(f64::from(parameter0)),
                            Value::Number(f64::from(parameter1)),
                        ],
                    );
                    let finish_result = self.vm.finish_platform_event();
                    result?;
                    finish_result?;
                }
                self.apply_lifecycle_request()?;
            }
            DisplayEvent::Pointer { x, y, pressed } if self.state == RuntimeState::Running => {
                if let Some((event, parameter0, parameter1)) =
                    self.vm.route_pointer_event(x, y, pressed)?
                {
                    let result = self.vm.call_global(
                        b"dealevent",
                        vec![
                            Value::Number(f64::from(event)),
                            Value::Number(f64::from(parameter0)),
                            Value::Number(f64::from(parameter1)),
                        ],
                    );
                    let finish_result = self.vm.finish_platform_event();
                    result?;
                    finish_result?;
                }
                self.apply_lifecycle_request()?;
            }
            DisplayEvent::PointerMove { x, y } if self.state == RuntimeState::Running => {
                if let Some((event, parameter0, parameter1)) = self.vm.route_pointer_move(x, y)? {
                    self.vm.call_global(
                        b"dealevent",
                        vec![
                            Value::Number(f64::from(event)),
                            Value::Number(f64::from(parameter0)),
                            Value::Number(f64::from(parameter1)),
                        ],
                    )?;
                }
                self.apply_lifecycle_request()?;
            }
            DisplayEvent::Motion { x, y, z } if self.state == RuntimeState::Running => {
                if let Some((event, parameter0, parameter1)) = self.vm.route_motion(x, y, z)? {
                    self.vm.call_global(
                        b"dealevent",
                        vec![
                            Value::Number(f64::from(event)),
                            Value::Number(f64::from(parameter0)),
                            Value::Number(f64::from(parameter1)),
                        ],
                    )?;
                }
                self.apply_lifecycle_request()?;
            }
            DisplayEvent::TextInput { text } if self.state == RuntimeState::Running => {
                if let Some((event, parameter0, parameter1)) = self.vm.route_text_input(&text)? {
                    self.vm.call_global(
                        b"dealevent",
                        vec![
                            Value::Number(f64::from(event)),
                            Value::Number(f64::from(parameter0)),
                            Value::Number(f64::from(parameter1)),
                        ],
                    )?;
                }
                self.apply_lifecycle_request()?;
            }
            _ => {}
        }
        Ok(())
    }

    fn apply_lifecycle_request(&mut self) -> Result<()> {
        let result = apply_lifecycle_result(&mut self.state, self.vm.process_lifecycle_request());
        if matches!(self.state, RuntimeState::Stopping | RuntimeState::Stopped) {
            self.vm.stop_audio();
        }
        result
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        self.vm.stop_audio();
    }
}

fn start_wap_proxy(
    dns_mappings: &[DnsMapping],
) -> Result<(Option<WapProxyService>, Option<SocketAddrV4>)> {
    if dns_mappings
        .iter()
        .any(|mapping| mapping.source.parse::<Ipv4Addr>() == Ok(WAP_PROXY_ADDRESS))
    {
        return Ok((None, None));
    }
    let proxy = WapProxyService::start(dns_mappings.into()).map_err(|error| {
        Error::Platform(format!("failed to start the WAP proxy service: {error}"))
    })?;
    let endpoint = proxy.endpoint();
    Ok((Some(proxy), Some(endpoint)))
}

fn apply_lifecycle_result(
    state: &mut RuntimeState,
    result: std::result::Result<LifecycleOutcome, LifecycleError>,
) -> Result<()> {
    match result {
        Ok(LifecycleOutcome::Continue) => Ok(()),
        Ok(LifecycleOutcome::ExitRequested) => {
            if !matches!(*state, RuntimeState::Stopping | RuntimeState::Stopped) {
                *state = RuntimeState::Stopping;
            }
            Ok(())
        }
        Err(LifecycleError::BeforeCommit(error)) => Err(error),
        Err(LifecycleError::AfterCommit(error)) => {
            *state = RuntimeState::Stopping;
            Err(error)
        }
    }
}

fn resolve_font_path(work_dir: &Path, font_path: &Path) -> PathBuf {
    if font_path.is_absolute() {
        font_path.to_path_buf()
    } else {
        work_dir.join(font_path)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    use super::*;

    struct TestDisplay;

    impl PlatformDisplay for TestDisplay {
        fn present(&mut self, _framebuffer: &Framebuffer) -> Result<()> {
            Ok(())
        }

        fn poll_event(&mut self) -> Result<Option<DisplayEvent>> {
            Ok(None)
        }

        fn wait_timeout(&mut self, _milliseconds: u32) {}
    }

    struct TrackingAudio {
        active: Arc<AtomicBool>,
    }

    impl PlatformAudio for TrackingAudio {
        fn play_sound(
            &mut self,
            _sound_type: crate::SoundType,
            _data: &[u8],
            _looped: bool,
        ) -> Result<()> {
            self.active.store(true, Ordering::Relaxed);
            Ok(())
        }

        fn stop_sound(&mut self) -> Result<()> {
            self.active.store(false, Ordering::Relaxed);
            Ok(())
        }

        fn is_active(&self) -> bool {
            self.active.load(Ordering::Relaxed)
        }

        fn set_volume(&mut self, _volume: u8) -> Result<()> {
            Ok(())
        }
    }

    fn runtime_with_active_audio(active: Arc<AtomicBool>) -> Runtime {
        let limits = ResourceLimits::default();
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test/fixtures/talkcat.mrp");
        let package = Arc::new(Package::open(fixture, limits.clone()).unwrap());
        let vm = MrVm::new(
            package,
            Framebuffer::new(240, 320).unwrap(),
            Box::new(TestDisplay),
            Box::new(TrackingAudio { active }),
            MrHostConfig {
                work_dir: PathBuf::from("."),
                font: Arc::from(&b""[..]),
                memory_limit: 1024 * 1024,
                dns_mappings: Arc::from([]),
                device_date: DeviceDate::default(),
                wap_proxy_endpoint: None,
            },
            limits,
        );
        Runtime {
            state: RuntimeState::Running,
            entry: b"start.mr".to_vec(),
            vm,
            _wap_proxy: None,
        }
    }

    #[test]
    fn resolves_relative_font_paths_from_the_work_directory() {
        assert_eq!(
            resolve_font_path(
                Path::new("runtime-work"),
                Path::new("mythroad/system/gb16.uc2")
            ),
            PathBuf::from("runtime-work/mythroad/system/gb16.uc2")
        );
    }

    #[test]
    fn uses_the_mythroad_gb16_font_by_default() {
        assert_eq!(
            RuntimeConfig::for_app("app.mrp").font_path,
            PathBuf::from("mythroad/system/gb16.uc2")
        );
    }

    #[test]
    fn uses_the_compatible_memory_profile_by_default() {
        assert_eq!(
            RuntimeConfig::for_app("app.mrp").memory_limit,
            DEFAULT_MEMORY_LIMIT
        );
    }

    #[test]
    fn uses_skymobi_dns_mappings_by_default() {
        assert_eq!(
            RuntimeConfig::for_app("app.mrp").dns_mappings,
            [
                DnsMapping {
                    source: "rop.skymobiapp.com".into(),
                    address: Ipv4Addr::new(159, 75, 119, 124),
                    port: None,
                },
                DnsMapping {
                    source: "spd.skymobiapp.com".into(),
                    address: Ipv4Addr::new(159, 75, 119, 124),
                    port: None,
                },
                DnsMapping {
                    source: "wap.skmeg.com".into(),
                    address: Ipv4Addr::new(159, 75, 119, 124),
                    port: None,
                },
            ]
        );
    }

    #[test]
    fn starts_an_internal_wap_proxy_unless_the_gateway_is_overridden() {
        let mappings = Vec::new();
        let (service, endpoint) = start_wap_proxy(&mappings).unwrap();
        let endpoint = endpoint.unwrap();
        assert!(service.is_some());
        assert_eq!(*endpoint.ip(), Ipv4Addr::LOCALHOST);
        assert!(endpoint.port() > 0);
        assert!(mappings.is_empty());

        let override_mapping = DnsMapping {
            source: WAP_PROXY_ADDRESS.to_string(),
            address: Ipv4Addr::LOCALHOST,
            port: Some(8080),
        };
        let mappings = vec![override_mapping.clone()];
        let (service, endpoint) = start_wap_proxy(&mappings).unwrap();
        assert!(service.is_none());
        assert_eq!(endpoint, None);
        assert_eq!(mappings, [override_mapping]);
    }

    #[test]
    fn device_dates_validate_leap_years_and_compute_weekdays() {
        assert_eq!(DeviceDate::new(2000, 2, 29).unwrap().weekday(), 2);
        assert_eq!(DeviceDate::new(2012, 6, 20).unwrap().weekday(), 3);
        assert_eq!(DeviceDate::new(1970, 1, 1).unwrap().weekday(), 4);
        assert_eq!(DeviceDate::new(1900, 2, 29), None);
        assert_eq!(DeviceDate::new(2011, 4, 31), None);
    }

    #[test]
    fn converts_unix_epoch_days_to_gregorian_dates() {
        assert_eq!(
            civil_date_from_unix_days(0),
            DeviceDate::new(1970, 1, 1).unwrap()
        );
        assert_eq!(
            civil_date_from_unix_days(15_492),
            DeviceDate::new(2012, 6, 1).unwrap()
        );
    }

    #[test]
    fn device_dates_convert_and_advance_with_time_of_day() {
        let datetime = DeviceDate::with_time_of_day(2012, 6, 20, 23, 59, 59).unwrap();
        assert_eq!(
            datetime.advance(Duration::from_secs(2)),
            DeviceDate::with_time_of_day(2012, 6, 21, 0, 0, 1).unwrap()
        );
        assert_eq!(
            DeviceDate::from_unix_timestamp(0),
            DeviceDate::with_time_of_day(1970, 1, 1, 0, 0, 0)
        );
        assert_eq!(
            DeviceDate::from_unix_timestamp(86_399),
            DeviceDate::with_time_of_day(1970, 1, 1, 23, 59, 59)
        );
    }

    #[test]
    fn stopping_or_dropping_the_runtime_stops_host_audio() {
        let stopped = Arc::new(AtomicBool::new(true));
        let mut runtime = runtime_with_active_audio(stopped.clone());
        runtime.stop();
        assert!(!stopped.load(Ordering::Relaxed));

        let dropped = Arc::new(AtomicBool::new(true));
        drop(runtime_with_active_audio(dropped.clone()));
        assert!(!dropped.load(Ordering::Relaxed));
    }

    #[test]
    fn lifecycle_fault_after_replacement_commit_stops_dispatch() {
        let mut state = RuntimeState::Running;

        assert!(
            apply_lifecycle_result(
                &mut state,
                Err(LifecycleError::AfterCommit(Error::ArmFault(
                    "replacement init failed".into(),
                ))),
            )
            .is_err()
        );
        assert_eq!(state, RuntimeState::Stopping);
    }

    #[test]
    fn lifecycle_fault_before_replacement_commit_keeps_dispatch_running() {
        let mut state = RuntimeState::Running;

        assert!(
            apply_lifecycle_result(
                &mut state,
                Err(LifecycleError::BeforeCommit(Error::Package(
                    "replacement staging failed".into(),
                ))),
            )
            .is_err()
        );
        assert_eq!(state, RuntimeState::Running);
    }
}
