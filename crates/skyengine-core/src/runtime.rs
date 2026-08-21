use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use crate::{
    DisplayEvent, Error, Framebuffer, Package, PlatformDisplay, ResourceLimits, Result,
    mr::{MrVm, value::Value},
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

#[derive(Clone, Debug)]
pub struct RuntimeConfig {
    pub app_path: PathBuf,
    pub entry: Vec<u8>,
    pub work_dir: PathBuf,
    pub font_path: PathBuf,
    pub memory_limit: u32,
    pub screen_width: u16,
    pub screen_height: u16,
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
            config.work_dir,
            font.into(),
            config.memory_limit,
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

    pub fn start(&mut self) -> Result<()> {
        if self.state != RuntimeState::Loaded {
            return Err(Error::MrFault(format!(
                "cannot start runtime in {:?}",
                self.state
            )));
        }
        self.vm.run_entry(&self.entry)?;
        self.state = RuntimeState::Running;
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
        while let Some(event) = self.vm.display_mut().poll_event()? {
            self.dispatch(event)?;
            if self.state == RuntimeState::Stopping {
                return Ok(());
            }
        }
        if self.state == RuntimeState::Running {
            self.vm.dispatch_native_timer()?;
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
                self.vm.call_global(
                    b"dealevent",
                    vec![
                        Value::Number(if pressed { 0.0 } else { 1.0 }),
                        Value::Number(f64::from(code)),
                        Value::Number(0.0),
                    ],
                )?;
            }
            DisplayEvent::Pointer { x, y, pressed } if self.state == RuntimeState::Running => {
                self.vm.call_global(
                    b"dealevent",
                    vec![
                        Value::Number(if pressed { 2.0 } else { 3.0 }),
                        Value::Number(f64::from(x)),
                        Value::Number(f64::from(y)),
                    ],
                )?;
            }
            _ => {}
        }
        Ok(())
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
}
