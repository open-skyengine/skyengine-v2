use std::{
    fs,
    net::Ipv4Addr,
    path::{Path, PathBuf},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use crate::{
    DisplayEvent, Error, Framebuffer, Package, PlatformDisplay, ResourceLimits, Result,
    mr::{
        MrHostConfig, MrVm,
        value::Value,
        vm::{LifecycleError, LifecycleOutcome},
    },
};

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
}

impl DeviceDate {
    pub const BASELINE: Self = Self {
        year: 2012,
        month: 6,
        day: 20,
    };

    pub fn new(year: u16, month: u8, day: u8) -> Option<Self> {
        let max_day = match month {
            1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
            4 | 6 | 9 | 11 => 30,
            2 if is_leap_year(year) => 29,
            2 => 28,
            _ => return None,
        };
        (year != 0 && day != 0 && day <= max_day).then_some(Self { year, month, day })
    }

    pub fn host_today() -> Self {
        let days = match SystemTime::now().duration_since(UNIX_EPOCH) {
            Ok(duration) => (duration.as_secs() / 86_400) as i64,
            Err(error) => -((error.duration().as_secs() / 86_400) as i64),
        };
        civil_date_from_unix_days(days)
    }

    pub(crate) fn weekday(self) -> u8 {
        let mut year = i64::from(self.year);
        let month = i64::from(self.month);
        let day = i64::from(self.day);
        year -= i64::from(month <= 2);
        let era = year.div_euclid(400);
        let year_of_era = year - era * 400;
        let shifted_month = month + if month > 2 { -3 } else { 9 };
        let day_of_year = (153 * shifted_month + 2) / 5 + day - 1;
        let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
        let unix_days = era * 146_097 + day_of_era - 719_468;
        (unix_days + 4).rem_euclid(7) as u8
    }
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
            memory_limit: 1024 * 1024,
            screen_width: 240,
            screen_height: 320,
            dns_mappings: Vec::new(),
            device_date: DeviceDate::default(),
            limits: ResourceLimits::default(),
        }
    }
}

pub struct Runtime {
    state: RuntimeState,
    entry: Vec<u8>,
    vm: MrVm,
}

impl Runtime {
    pub fn load(config: RuntimeConfig, display: Box<dyn PlatformDisplay>) -> Result<Self> {
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
        let vm = MrVm::new(
            package,
            framebuffer,
            display,
            MrHostConfig {
                work_dir: config.work_dir,
                font: font.into(),
                memory_limit: config.memory_limit,
                dns_mappings: config.dns_mappings.into(),
                device_date: config.device_date,
            },
            config.limits,
        );
        Ok(Self {
            state: RuntimeState::Loaded,
            entry: config.entry,
            vm,
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
    }

    fn dispatch(&mut self, event: DisplayEvent) -> Result<()> {
        match event {
            DisplayEvent::Quit => self.stop(),
            DisplayEvent::Key { code, pressed } if self.state == RuntimeState::Running => {
                if let Some((event, parameter0, parameter1)) =
                    self.vm.route_key_event(code, pressed)?
                {
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
            DisplayEvent::Pointer { x, y, pressed } if self.state == RuntimeState::Running => {
                if let Some((event, parameter0, parameter1)) =
                    self.vm.route_pointer_event(x, y, pressed)?
                {
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
        apply_lifecycle_result(&mut self.state, self.vm.process_lifecycle_request())
    }
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
    use super::*;

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
