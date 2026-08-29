use std::{
    collections::VecDeque,
    ffi::{CStr, CString, c_char, c_void},
    net::{Ipv4Addr, SocketAddrV4},
    panic::{AssertUnwindSafe, catch_unwind},
    path::PathBuf,
    ptr,
    sync::{Arc, Condvar, LazyLock, Mutex, MutexGuard, mpsc},
    thread::{self, JoinHandle},
    time::Duration,
};

use skyengine_core::{
    AUDIO_CHANNELS, AUDIO_SAMPLE_RATE, AudioPlayer, DeviceDate, DisplayEvent, DnsMapping,
    Framebuffer, PlatformDisplay, Result as CoreResult, Runtime, RuntimeConfig, RuntimeState,
};

const DEFAULT_WIDTH: u16 = 240;
const DEFAULT_HEIGHT: u16 = 320;
const DEFAULT_MEMORY_MB: i32 = 1;
const WORKER_INTERVAL: Duration = Duration::from_millis(10);
const MAX_SCREEN_DIMENSION: u16 = 4096;

static ENGINE: LazyLock<Mutex<Engine>> = LazyLock::new(|| Mutex::new(Engine::default()));

#[derive(Clone)]
struct EngineConfig {
    initialized: bool,
    width: u16,
    height: u16,
    memory_limit: u32,
    device_date: DeviceDate,
    work_dir: PathBuf,
    sound_font_path: Option<PathBuf>,
    dns_mappings: Vec<DnsMapping>,
    image_processing_mode: i32,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            initialized: false,
            width: DEFAULT_WIDTH,
            height: DEFAULT_HEIGHT,
            memory_limit: DEFAULT_MEMORY_MB as u32 * 1024 * 1024,
            device_date: DeviceDate::host_now(),
            work_dir: PathBuf::from("."),
            sound_font_path: None,
            dns_mappings: Vec::new(),
            image_processing_mode: 0,
        }
    }
}

struct Engine {
    config: EngineConfig,
    worker: Option<Worker>,
    exposed_rgb565: Box<[u16]>,
    exposed_rgba: Box<[u8]>,
    exposed_edit_text: CString,
    last_error: CString,
}

impl Default for Engine {
    fn default() -> Self {
        Self {
            config: EngineConfig::default(),
            worker: None,
            exposed_rgb565: Box::new([]),
            exposed_rgba: Box::new([]),
            exposed_edit_text: empty_c_string(),
            last_error: empty_c_string(),
        }
    }
}

struct Worker {
    shared: Arc<Shared>,
    thread: JoinHandle<()>,
}

struct Shared {
    state: Mutex<SharedState>,
    wake: Condvar,
    audio: AudioPlayer,
}

impl Shared {
    #[cfg(test)]
    fn new(width: u16, height: u16) -> Self {
        Self::with_audio(width, height, AudioPlayer::default())
    }

    fn with_audio(width: u16, height: u16, audio: AudioPlayer) -> Self {
        Self {
            state: Mutex::new(SharedState {
                events: VecDeque::new(),
                frame: vec![0; usize::from(width) * usize::from(height)],
                width,
                height,
                rotation: 0,
                dirty: false,
                running: false,
                paused: false,
                stop_requested: false,
                edit_text: None,
                last_error: None,
            }),
            wake: Condvar::new(),
            audio,
        }
    }

    fn push_event(&self, event: DisplayEvent) -> Result<(), String> {
        let mut state = lock(&self.state);
        if !state.running || state.stop_requested {
            return Err("SkyEngine runtime is not running".into());
        }
        state.events.push_back(event);
        self.wake.notify_one();
        Ok(())
    }

    fn wait_timeout(&self, timeout: Duration) {
        let state = lock(&self.state);
        drop(
            self.wake
                .wait_timeout(state, timeout)
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
        );
    }
}

struct SharedState {
    events: VecDeque<DisplayEvent>,
    frame: Vec<u16>,
    width: u16,
    height: u16,
    rotation: i32,
    dirty: bool,
    running: bool,
    paused: bool,
    stop_requested: bool,
    edit_text: Option<String>,
    last_error: Option<String>,
}

struct FlutterDisplay {
    shared: Arc<Shared>,
    panel_width: u16,
    panel_height: u16,
}

impl PlatformDisplay for FlutterDisplay {
    fn resize(&mut self, width: u16, height: u16) -> CoreResult<()> {
        let mut state = lock(&self.shared.state);
        state.width = width;
        state.height = height;
        state.rotation = if width == self.panel_height && height == self.panel_width {
            1
        } else {
            0
        };
        state
            .frame
            .resize(usize::from(width) * usize::from(height), 0);
        state.dirty = true;
        Ok(())
    }

    fn present(&mut self, framebuffer: &Framebuffer) -> CoreResult<()> {
        let mut state = lock(&self.shared.state);
        state.width = framebuffer.width();
        state.height = framebuffer.height();
        state.rotation = if state.width == self.panel_height && state.height == self.panel_width {
            1
        } else {
            0
        };
        state.frame.clear();
        state.frame.extend_from_slice(framebuffer.pixels());
        state.dirty = true;
        Ok(())
    }

    fn poll_event(&mut self) -> CoreResult<Option<DisplayEvent>> {
        Ok(lock(&self.shared.state).events.pop_front())
    }

    fn wait_timeout(&mut self, milliseconds: u32) {
        self.shared
            .wait_timeout(Duration::from_millis(u64::from(milliseconds)));
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn empty_c_string() -> CString {
    CString::new(Vec::<u8>::new()).expect("an empty string has no NUL bytes")
}

fn safe_c_string(value: &str) -> CString {
    CString::new(value.replace('\0', "\u{fffd}"))
        .expect("replacement removes all interior NUL bytes")
}

fn panic_message(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_owned()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "unknown panic".into()
    }
}

fn set_last_error(message: impl AsRef<str>) {
    lock(&ENGINE).last_error = safe_c_string(message.as_ref());
}

fn clear_last_error(engine: &mut Engine) {
    engine.last_error = empty_c_string();
}

fn ffi_result(operation: impl FnOnce() -> Result<i32, String>) -> i32 {
    match catch_unwind(AssertUnwindSafe(operation)) {
        Ok(Ok(value)) => value,
        Ok(Err(error)) => {
            set_last_error(error);
            -1
        }
        Err(payload) => {
            set_last_error(format!(
                "panic in SkyEngine FFI call: {}",
                panic_message(payload)
            ));
            -1
        }
    }
}

unsafe fn required_string(pointer: *const c_char, name: &str) -> Result<String, String> {
    if pointer.is_null() {
        return Err(format!("{name} must not be null"));
    }
    // SAFETY: The C ABI requires a non-null, NUL-terminated string for this argument.
    let value = unsafe { CStr::from_ptr(pointer) };
    value
        .to_str()
        .map(str::to_owned)
        .map_err(|_| format!("{name} is not valid UTF-8"))
}

unsafe fn optional_string(pointer: *const c_char, name: &str) -> Result<Option<String>, String> {
    if pointer.is_null() {
        return Ok(None);
    }
    // SAFETY: A non-null optional C ABI string must be NUL-terminated.
    let value = unsafe { CStr::from_ptr(pointer) };
    value
        .to_str()
        .map(|value| Some(value.to_owned()))
        .map_err(|_| format!("{name} is not valid UTF-8"))
}

fn parse_device_date(value: &str) -> Result<DeviceDate, String> {
    if value == "host" {
        return Ok(DeviceDate::host_now());
    }
    let invalid = || format!("invalid device date {value:?}; expected YYYY-M-D or host");
    let mut parts = value.split('-');
    let year = parts.next().ok_or_else(invalid)?;
    let month = parts.next().ok_or_else(invalid)?;
    let day = parts.next().ok_or_else(invalid)?;
    if parts.next().is_some() || year.is_empty() || month.is_empty() || day.is_empty() {
        return Err(invalid());
    }
    DeviceDate::new(
        year.parse().map_err(|_| invalid())?,
        month.parse().map_err(|_| invalid())?,
        day.parse().map_err(|_| invalid())?,
    )
    .ok_or_else(invalid)
}

fn parse_dns_mappings(value: &str) -> Result<Vec<DnsMapping>, String> {
    let mut mappings = Vec::new();
    for item in value
        .split(';')
        .map(str::trim)
        .filter(|item| !item.is_empty())
    {
        let (source, target) = item
            .split_once("->")
            .ok_or_else(|| format!("invalid DNS mapping {item:?}; expected SOURCE->IPv4[:PORT]"))?;
        let source = source.trim().trim_end_matches('.').to_ascii_lowercase();
        let target = target.trim();
        if source.is_empty() || !source.is_ascii() {
            return Err(format!("invalid DNS mapping source {source:?}"));
        }
        if mappings
            .iter()
            .any(|mapping: &DnsMapping| mapping.source == source)
        {
            return Err(format!("duplicate DNS mapping source {source:?}"));
        }
        let (address, port) = match target.parse::<Ipv4Addr>() {
            Ok(address) => (address, None),
            Err(_) => {
                let endpoint = target.parse::<SocketAddrV4>().map_err(|_| {
                    format!("invalid DNS mapping target {target:?}; expected IPv4[:PORT]")
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

fn worker_main(
    config: EngineConfig,
    app_path: PathBuf,
    entry: Vec<u8>,
    shared: Arc<Shared>,
    startup: &mpsc::SyncSender<Result<(), String>>,
) {
    let display = FlutterDisplay {
        shared: shared.clone(),
        panel_width: config.width,
        panel_height: config.height,
    };
    let mut runtime_config = RuntimeConfig::for_app(app_path);
    runtime_config.entry = entry;
    runtime_config.work_dir = config.work_dir;
    runtime_config.memory_limit = config.memory_limit;
    runtime_config.screen_width = config.width;
    runtime_config.screen_height = config.height;
    runtime_config.dns_mappings = config.dns_mappings;
    runtime_config.device_date = config.device_date;

    let mut runtime = match Runtime::load_with_audio(
        runtime_config,
        Box::new(display),
        Box::new(shared.audio.clone()),
    ) {
        Ok(runtime) => runtime,
        Err(error) => {
            let message = error.to_string();
            lock(&shared.state).last_error = Some(message.clone());
            let _ = startup.send(Err(message));
            return;
        }
    };
    if let Err(error) = runtime.start() {
        let message = error.to_string();
        lock(&shared.state).last_error = Some(message.clone());
        let _ = startup.send(Err(message));
        return;
    }

    {
        let mut state = lock(&shared.state);
        state.running = matches!(
            runtime.state(),
            RuntimeState::Running | RuntimeState::Paused
        );
        state.edit_text = runtime.active_editor_text();
    }
    let _ = startup.send(Ok(()));

    loop {
        {
            let state = lock(&shared.state);
            if state.stop_requested {
                runtime.stop();
                break;
            }
            if state.paused {
                drop(
                    shared
                        .wake
                        .wait_timeout(state, WORKER_INTERVAL)
                        .unwrap_or_else(|poisoned| poisoned.into_inner()),
                );
                continue;
            }
        }

        if let Err(error) = runtime.tick() {
            lock(&shared.state).last_error = Some(error.to_string());
            runtime.stop();
            break;
        }
        {
            let mut state = lock(&shared.state);
            state.edit_text = runtime.active_editor_text();
        }
        if !matches!(
            runtime.state(),
            RuntimeState::Running | RuntimeState::Paused
        ) {
            break;
        }
        shared.wait_timeout(WORKER_INTERVAL);
    }

    shared.audio.stop();
    let mut state = lock(&shared.state);
    state.running = false;
    state.paused = false;
    state.edit_text = None;
    shared.wake.notify_all();
}

fn spawn_worker(config: EngineConfig, app_path: PathBuf, entry: Vec<u8>) -> Result<Worker, String> {
    let audio = match &config.sound_font_path {
        Some(path) => {
            let path = if path.is_absolute() {
                path.clone()
            } else {
                config.work_dir.join(path)
            };
            AudioPlayer::with_sound_font_file(&path)
                .map_err(|error| format!("failed to load SoundFont {}: {error}", path.display()))?
        }
        None => AudioPlayer::default(),
    };
    let shared = Arc::new(Shared::with_audio(config.width, config.height, audio));
    let thread_shared = shared.clone();
    let (startup_sender, startup_receiver) = mpsc::sync_channel(1);
    let thread = thread::Builder::new()
        .name("skyengine-runtime".into())
        .spawn(move || {
            let result = catch_unwind(AssertUnwindSafe(|| {
                worker_main(
                    config,
                    app_path,
                    entry,
                    thread_shared.clone(),
                    &startup_sender,
                );
            }));
            if let Err(payload) = result {
                let message = format!("SkyEngine runtime panicked: {}", panic_message(payload));
                let mut state = lock(&thread_shared.state);
                state.last_error = Some(message.clone());
                state.running = false;
                state.edit_text = None;
                drop(state);
                let _ = startup_sender.send(Err(message));
                thread_shared.wake.notify_all();
            }
        })
        .map_err(|error| format!("failed to create SkyEngine runtime thread: {error}"))?;

    match startup_receiver.recv() {
        Ok(Ok(())) => Ok(Worker { shared, thread }),
        Ok(Err(error)) => {
            let _ = thread.join();
            Err(error)
        }
        Err(_) => {
            let _ = thread.join();
            Err("SkyEngine runtime thread exited before startup completed".into())
        }
    }
}

fn stop_worker() {
    let worker = lock(&ENGINE).worker.take();
    if let Some(worker) = worker {
        {
            let mut state = lock(&worker.shared.state);
            state.stop_requested = true;
            state.events.clear();
        }
        worker.shared.audio.stop();
        worker.shared.wake.notify_all();
        if worker.thread.join().is_err() {
            set_last_error("SkyEngine runtime thread panicked during shutdown");
        }
    }
}

fn current_shared() -> Result<Arc<Shared>, String> {
    lock(&ENGINE)
        .worker
        .as_ref()
        .map(|worker| worker.shared.clone())
        .ok_or_else(|| "SkyEngine runtime has not been started".into())
}

fn enqueue(event: DisplayEvent) -> Result<i32, String> {
    current_shared()?.push_event(event)?;
    Ok(0)
}

#[unsafe(no_mangle)]
pub extern "C" fn skyengine_api_init(width: i32, height: i32) -> i32 {
    ffi_result(|| {
        stop_worker();
        let width = u16::try_from(width).map_err(|_| "screen width is out of range")?;
        let height = u16::try_from(height).map_err(|_| "screen height is out of range")?;
        if width == 0 || height == 0 {
            return Err("screen dimensions must be non-zero".into());
        }
        if width > MAX_SCREEN_DIMENSION || height > MAX_SCREEN_DIMENSION {
            return Err(format!(
                "screen dimensions exceed {MAX_SCREEN_DIMENSION}x{MAX_SCREEN_DIMENSION}"
            ));
        }
        let pixels = usize::from(width) * usize::from(height);
        let mut engine = lock(&ENGINE);
        engine.config = EngineConfig {
            initialized: true,
            width,
            height,
            ..EngineConfig::default()
        };
        engine.exposed_rgb565 = vec![0; pixels].into_boxed_slice();
        engine.exposed_rgba = vec![0; pixels * 4].into_boxed_slice();
        engine.exposed_edit_text = empty_c_string();
        clear_last_error(&mut engine);
        Ok(0)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn skyengine_api_set_memory(memory_mb: i32) -> i32 {
    ffi_result(|| {
        if !matches!(memory_mb, 1 | 2 | 4 | 6 | 8 | 16) {
            return Err("memory size must be 1, 2, 4, 6, 8, or 16 MiB".into());
        }
        let mut engine = lock(&ENGINE);
        if !engine.config.initialized {
            return Err("SkyEngine must be initialized before configuring memory".into());
        }
        if engine.worker.is_some() {
            return Err("memory cannot be changed while SkyEngine is running".into());
        }
        engine.config.memory_limit = memory_mb as u32 * 1024 * 1024;
        clear_last_error(&mut engine);
        Ok(0)
    })
}

#[unsafe(no_mangle)]
/// # Safety
///
/// `date` must point to a readable, NUL-terminated UTF-8 string for the
/// duration of this call.
pub unsafe extern "C" fn skyengine_api_set_device_date(date: *const c_char) -> i32 {
    ffi_result(|| {
        // SAFETY: This function's C contract requires a valid date string.
        let date = unsafe { required_string(date, "date") }?;
        let value = parse_device_date(date.trim())?;
        let mut engine = lock(&ENGINE);
        if !engine.config.initialized || engine.worker.is_some() {
            return Err("device date can only be set after init and before start".into());
        }
        engine.config.device_date = value;
        clear_last_error(&mut engine);
        Ok(0)
    })
}

#[unsafe(no_mangle)]
/// # Safety
///
/// `path` must point to a readable, NUL-terminated UTF-8 string for the
/// duration of this call.
pub unsafe extern "C" fn skyengine_api_set_work_dir(path: *const c_char) -> i32 {
    ffi_result(|| {
        // SAFETY: This function's C contract requires a valid path string.
        let path = unsafe { required_string(path, "work directory") }?;
        if path.is_empty() {
            return Err("work directory must not be empty".into());
        }
        let mut engine = lock(&ENGINE);
        if !engine.config.initialized || engine.worker.is_some() {
            return Err("work directory can only be set after init and before start".into());
        }
        engine.config.work_dir = PathBuf::from(path);
        clear_last_error(&mut engine);
        Ok(0)
    })
}

#[unsafe(no_mangle)]
/// # Safety
///
/// `path` must point to a readable, NUL-terminated UTF-8 string for the
/// duration of this call. Relative paths are resolved from the work directory.
pub unsafe extern "C" fn skyengine_api_set_sound_font(path: *const c_char) -> i32 {
    ffi_result(|| {
        // SAFETY: This function's C contract requires a valid path string.
        let path = unsafe { required_string(path, "SoundFont path") }?;
        if path.is_empty() {
            return Err("SoundFont path must not be empty".into());
        }
        let mut engine = lock(&ENGINE);
        if !engine.config.initialized || engine.worker.is_some() {
            return Err("SoundFont can only be set after init and before start".into());
        }
        engine.config.sound_font_path = Some(PathBuf::from(path));
        clear_last_error(&mut engine);
        Ok(0)
    })
}

#[unsafe(no_mangle)]
/// # Safety
///
/// `mappings` must point to a readable, NUL-terminated UTF-8 string for the
/// duration of this call.
pub unsafe extern "C" fn skyengine_api_set_dns_map(mappings: *const c_char) -> i32 {
    ffi_result(|| {
        // SAFETY: This function's C contract requires a valid mapping string.
        let mappings = unsafe { required_string(mappings, "DNS mappings") }?;
        let mappings = parse_dns_mappings(&mappings)?;
        let mut engine = lock(&ENGINE);
        if !engine.config.initialized || engine.worker.is_some() {
            return Err("DNS mappings can only be set after init and before start".into());
        }
        engine.config.dns_mappings = mappings;
        clear_last_error(&mut engine);
        Ok(0)
    })
}

#[unsafe(no_mangle)]
/// # Safety
///
/// `mrp_path` must point to a readable, NUL-terminated UTF-8 string. `entry`
/// and `entry_override` may be null; otherwise they must meet the same
/// requirement. All strings only need to remain valid for this call.
pub unsafe extern "C" fn skyengine_api_start(
    mrp_path: *const c_char,
    entry: *const c_char,
    entry_override: *const c_char,
) -> i32 {
    ffi_result(|| {
        // SAFETY: The C contract requires valid NUL-terminated strings when non-null.
        let mrp_path = unsafe { required_string(mrp_path, "MRP path") }?;
        // SAFETY: See the argument contract above.
        let entry = unsafe { optional_string(entry, "entry") }?;
        // SAFETY: See the argument contract above.
        let entry_override = unsafe { optional_string(entry_override, "entry override") }?;
        let (config, app_path) = {
            let engine = lock(&ENGINE);
            if !engine.config.initialized {
                return Err("SkyEngine must be initialized before start".into());
            }
            if engine.worker.is_some() {
                return Err("SkyEngine is already running".into());
            }
            let path = PathBuf::from(mrp_path);
            let path = if path.is_absolute() {
                path
            } else {
                engine.config.work_dir.join(path)
            };
            (engine.config.clone(), path)
        };
        let entry = entry_override
            .filter(|entry| !entry.is_empty())
            .or_else(|| entry.filter(|entry| !entry.is_empty()))
            .unwrap_or_else(|| "start.mr".into())
            .into_bytes();
        let worker = spawn_worker(config, app_path, entry)?;
        let mut engine = lock(&ENGINE);
        engine.worker = Some(worker);
        clear_last_error(&mut engine);
        Ok(0)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn skyengine_api_destroy() {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        stop_worker();
        let mut engine = lock(&ENGINE);
        engine.config.initialized = false;
        engine.exposed_edit_text = empty_c_string();
    }));
}

#[unsafe(no_mangle)]
pub extern "C" fn skyengine_api_is_running() -> i32 {
    catch_unwind(AssertUnwindSafe(|| {
        let engine = lock(&ENGINE);
        engine
            .worker
            .as_ref()
            .map_or(0, |worker| i32::from(lock(&worker.shared.state).running))
    }))
    .unwrap_or(0)
}

#[unsafe(no_mangle)]
pub extern "C" fn skyengine_api_pause() -> i32 {
    ffi_result(|| {
        let shared = current_shared()?;
        let mut state = lock(&shared.state);
        if !state.running {
            return Err("SkyEngine runtime is not running".into());
        }
        state.paused = true;
        Ok(0)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn skyengine_api_resume() -> i32 {
    ffi_result(|| {
        let shared = current_shared()?;
        let mut state = lock(&shared.state);
        if !state.running {
            return Err("SkyEngine runtime is not running".into());
        }
        state.paused = false;
        drop(state);
        shared.wake.notify_all();
        Ok(0)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn skyengine_api_event(event: i32, parameter0: i32, parameter1: i32) -> i32 {
    ffi_result(|| match event {
        0 => enqueue(DisplayEvent::Key {
            code: parameter0,
            pressed: true,
        }),
        1 => enqueue(DisplayEvent::Key {
            code: parameter0,
            pressed: false,
        }),
        2 => enqueue(DisplayEvent::Pointer {
            x: parameter0,
            y: parameter1,
            pressed: true,
        }),
        3 => enqueue(DisplayEvent::Pointer {
            x: parameter0,
            y: parameter1,
            pressed: false,
        }),
        12 => enqueue(DisplayEvent::PointerMove {
            x: parameter0,
            y: parameter1,
        }),
        _ => Err(format!("unsupported SkyEngine event code {event}")),
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn skyengine_api_timer() -> i32 {
    // Runtime timers are driven by the native worker thread.
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn skyengine_api_get_timer_interval() -> i32 {
    WORKER_INTERVAL.as_millis() as i32
}

#[unsafe(no_mangle)]
pub extern "C" fn skyengine_api_set_image_processing_mode(mode: i32) -> i32 {
    ffi_result(|| {
        if !matches!(mode, 0 | 1) {
            return Err(format!("unsupported image processing mode {mode}"));
        }
        let mut engine = lock(&ENGINE);
        if !engine.config.initialized {
            return Err("SkyEngine must be initialized before setting image mode".into());
        }
        engine.config.image_processing_mode = mode;
        Ok(0)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn skyengine_api_get_image_processing_mode() -> i32 {
    lock(&ENGINE).config.image_processing_mode
}

#[unsafe(no_mangle)]
pub extern "C" fn skyengine_api_get_screen_buffer() -> *const u16 {
    match catch_unwind(AssertUnwindSafe(|| {
        let mut engine = lock(&ENGINE);
        let shared = engine.worker.as_ref()?.shared.clone();
        let state = lock(&shared.state);
        if engine.exposed_rgb565.len() != state.frame.len() {
            return None;
        }
        engine.exposed_rgb565.copy_from_slice(&state.frame);
        Some(engine.exposed_rgb565.as_ptr())
    })) {
        Ok(Some(pointer)) => pointer,
        _ => ptr::null(),
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn skyengine_api_get_screen_rgba_buffer() -> *const u8 {
    match catch_unwind(AssertUnwindSafe(|| {
        let mut engine = lock(&ENGINE);
        let shared = engine.worker.as_ref()?.shared.clone();
        let state = lock(&shared.state);
        if engine.exposed_rgba.len() != state.frame.len() * 4 {
            return None;
        }
        let (rgba_pixels, remainder) = engine.exposed_rgba.as_chunks_mut::<4>();
        debug_assert!(remainder.is_empty());
        for (pixel, rgba) in state.frame.iter().copied().zip(rgba_pixels) {
            let red = ((pixel >> 11) & 0x1f) as u8;
            let green = ((pixel >> 5) & 0x3f) as u8;
            let blue = (pixel & 0x1f) as u8;
            rgba[0] = (red << 3) | (red >> 2);
            rgba[1] = (green << 2) | (green >> 4);
            rgba[2] = (blue << 3) | (blue >> 2);
            rgba[3] = 0xff;
        }
        Some(engine.exposed_rgba.as_ptr())
    })) {
        Ok(Some(pointer)) => pointer,
        _ => ptr::null(),
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn skyengine_api_get_screen_dirty() -> i32 {
    catch_unwind(AssertUnwindSafe(|| {
        let engine = lock(&ENGINE);
        engine.worker.as_ref().map_or(0, |worker| {
            let mut state = lock(&worker.shared.state);
            i32::from(std::mem::take(&mut state.dirty))
        })
    }))
    .unwrap_or(0)
}

#[unsafe(no_mangle)]
pub extern "C" fn skyengine_api_get_screen_width() -> i32 {
    let engine = lock(&ENGINE);
    engine.worker.as_ref().map_or_else(
        || i32::from(engine.config.width),
        |worker| i32::from(lock(&worker.shared.state).width),
    )
}

#[unsafe(no_mangle)]
pub extern "C" fn skyengine_api_get_screen_height() -> i32 {
    let engine = lock(&ENGINE);
    engine.worker.as_ref().map_or_else(
        || i32::from(engine.config.height),
        |worker| i32::from(lock(&worker.shared.state).height),
    )
}

#[unsafe(no_mangle)]
pub extern "C" fn skyengine_api_get_screen_rotation() -> i32 {
    let engine = lock(&ENGINE);
    engine
        .worker
        .as_ref()
        .map_or(0, |worker| lock(&worker.shared.state).rotation)
}

#[unsafe(no_mangle)]
pub extern "C" fn skyengine_api_audio_sample_rate() -> i32 {
    AUDIO_SAMPLE_RATE as i32
}

#[unsafe(no_mangle)]
pub extern "C" fn skyengine_api_audio_channels() -> i32 {
    AUDIO_CHANNELS as i32
}

#[unsafe(no_mangle)]
pub extern "C" fn skyengine_api_audio_is_active() -> i32 {
    catch_unwind(AssertUnwindSafe(|| {
        lock(&ENGINE)
            .worker
            .as_ref()
            .is_some_and(|worker| worker.shared.audio.is_active()) as i32
    }))
    .unwrap_or(0)
}

#[unsafe(no_mangle)]
/// # Safety
///
/// `output` must point to writable storage for `frames * 2` `int16_t` samples.
pub unsafe extern "C" fn skyengine_api_audio_render_s16le(output: *mut c_void, frames: i32) -> i32 {
    ffi_result(|| {
        let frames = usize::try_from(frames).map_err(|_| "audio frame count is negative")?;
        if frames == 0 {
            return Ok(0);
        }
        if output.is_null() {
            return Err("audio output must not be null".into());
        }
        let sample_count = frames
            .checked_mul(AUDIO_CHANNELS)
            .ok_or("audio output length overflows")?;
        if sample_count > isize::MAX as usize / size_of::<i16>() {
            return Err("audio output length exceeds the addressable range".into());
        }
        let shared = lock(&ENGINE)
            .worker
            .as_ref()
            .map(|worker| worker.shared.clone());
        // SAFETY: The C contract requires writable storage for sample_count i16 values.
        let output = unsafe { std::slice::from_raw_parts_mut(output.cast::<i16>(), sample_count) };
        Ok(match shared {
            Some(shared) if !lock(&shared.state).paused => shared.audio.render(output) as i32,
            None => {
                output.fill(0);
                0
            }
            Some(_) => {
                output.fill(0);
                0
            }
        })
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn skyengine_api_audio_stop() {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        if let Some(worker) = lock(&ENGINE).worker.as_ref() {
            worker.shared.audio.stop();
        }
    }));
}

#[unsafe(no_mangle)]
pub extern "C" fn skyengine_api_is_edit_active() -> i32 {
    let engine = lock(&ENGINE);
    engine.worker.as_ref().map_or(0, |worker| {
        i32::from(lock(&worker.shared.state).edit_text.is_some())
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn skyengine_api_get_edit_text() -> *const c_char {
    match catch_unwind(AssertUnwindSafe(|| {
        let mut engine = lock(&ENGINE);
        let value = engine
            .worker
            .as_ref()
            .and_then(|worker| lock(&worker.shared.state).edit_text.clone())
            .unwrap_or_default();
        engine.exposed_edit_text = safe_c_string(&value);
        engine.exposed_edit_text.as_ptr()
    })) {
        Ok(pointer) => pointer,
        Err(_) => ptr::null(),
    }
}

#[unsafe(no_mangle)]
/// # Safety
///
/// `text` must point to a readable, NUL-terminated UTF-8 string for the
/// duration of this call.
pub unsafe extern "C" fn skyengine_api_set_edit_text(text: *const c_char) -> i32 {
    ffi_result(|| {
        // SAFETY: This function's C contract requires a valid text string.
        let text = unsafe { required_string(text, "editor text") }?;
        enqueue(DisplayEvent::TextInput { text })
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn skyengine_api_cancel_edit() -> i32 {
    ffi_result(|| {
        let shared = current_shared()?;
        shared.push_event(DisplayEvent::Key {
            code: 18,
            pressed: true,
        })?;
        shared.push_event(DisplayEvent::Key {
            code: 18,
            pressed: false,
        })?;
        Ok(0)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn skyengine_api_motion(_x: i32, _y: i32, _z: i32) -> i32 {
    -1
}

#[unsafe(no_mangle)]
pub extern "C" fn skyengine_api_motion_active() -> i32 {
    -1
}

#[unsafe(no_mangle)]
pub extern "C" fn skyengine_api_take_shake() -> i32 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn skyengine_api_last_error() -> *const c_char {
    match catch_unwind(AssertUnwindSafe(|| {
        let mut engine = lock(&ENGINE);
        if let Some(error) = engine
            .worker
            .as_ref()
            .and_then(|worker| lock(&worker.shared.state).last_error.clone())
        {
            engine.last_error = safe_c_string(&error);
        }
        engine.last_error.as_ptr()
    })) {
        Ok(pointer) => pointer,
        Err(_) => ptr::null(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct EngineGuard;

    impl Drop for EngineGuard {
        fn drop(&mut self) {
            skyengine_api_destroy();
        }
    }

    #[test]
    fn parses_dates_used_by_the_flutter_runtime_config() {
        assert_eq!(
            parse_device_date("2012-06-20").unwrap(),
            DeviceDate::new(2012, 6, 20).unwrap()
        );
        assert!(parse_device_date("2011-02-29").is_err());
        assert!(parse_device_date("host").unwrap().year >= 1970);
    }

    #[test]
    fn parses_dns_routes_with_optional_port_overrides() {
        assert_eq!(
            parse_dns_mappings("example.com->127.0.0.1;api.test->10.0.2.2:8080").unwrap(),
            [
                DnsMapping {
                    source: "example.com".into(),
                    address: Ipv4Addr::LOCALHOST,
                    port: None,
                },
                DnsMapping {
                    source: "api.test".into(),
                    address: Ipv4Addr::new(10, 0, 2, 2),
                    port: Some(8080),
                },
            ]
        );
    }

    #[test]
    fn flutter_display_publishes_rgb565_frames_and_geometry() {
        let shared = Arc::new(Shared::new(2, 3));
        let mut display = FlutterDisplay {
            shared: shared.clone(),
            panel_width: 2,
            panel_height: 3,
        };
        let mut frame = Framebuffer::new(2, 3).unwrap();
        frame.clear(0xf81f);
        display.present(&frame).unwrap();

        let state = lock(&shared.state);
        assert_eq!(state.frame, vec![0xf81f; 6]);
        assert_eq!((state.width, state.height, state.rotation), (2, 3, 0));
        assert!(state.dirty);
    }

    #[test]
    fn c_api_runs_a_real_package_and_publishes_its_first_frame() {
        let _guard = EngineGuard;
        let fixture_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../test/fixtures")
            .canonicalize()
            .unwrap();
        let work_dir = safe_c_string(&fixture_root.to_string_lossy());
        let app_path = safe_c_string("mythroad/dsm_gm.mrp");
        let entry = safe_c_string("start.mr");
        let empty_sound_font = safe_c_string("");

        assert_eq!(skyengine_api_init(240, 320), 0);
        // SAFETY: The test-owned C string lives for the duration of this call.
        assert_eq!(
            unsafe { skyengine_api_set_sound_font(empty_sound_font.as_ptr()) },
            -1
        );
        // SAFETY: The test-owned C strings live for the duration of each call.
        assert_eq!(unsafe { skyengine_api_set_work_dir(work_dir.as_ptr()) }, 0);
        // SAFETY: The test-owned C strings live for the duration of this call.
        assert_eq!(
            unsafe { skyengine_api_start(app_path.as_ptr(), entry.as_ptr(), ptr::null()) },
            0,
            "{}",
            unsafe { CStr::from_ptr(skyengine_api_last_error()) }.to_string_lossy()
        );
        assert_eq!(skyengine_api_is_running(), 1);
        assert_eq!(skyengine_api_pause(), 0);
        assert_eq!(skyengine_api_resume(), 0);
        assert_eq!(skyengine_api_event(12, 120, 160), 0);

        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while skyengine_api_get_screen_dirty() == 0 && std::time::Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(skyengine_api_get_screen_width(), 240);
        assert_eq!(skyengine_api_get_screen_height(), 320);
        assert!(!skyengine_api_get_screen_buffer().is_null());
        assert!(!skyengine_api_get_screen_rgba_buffer().is_null());
    }
}
