use std::{
    collections::BTreeMap,
    ffi::{OsStr, OsString},
    fs::{self, File, OpenOptions},
    io::{Cursor, Read, Seek, SeekFrom, Write},
    net::SocketAddrV4,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use encoding_rs::GBK;

use crate::{
    DnsMapping, Framebuffer, Package, PlatformAudio, PlatformDisplay, ResourceLimits, Result,
    VIRTUAL_IMEI, VIRTUAL_IMSI,
    arm::{
        ExtLifecycleRequest, ExtRuntime, GuestAddr, NativeExtensionProfile, NativeServices,
        START_FILE_PARAMETER_LEN,
    },
};

use super::{
    chunk::{MrChunk, Prototype},
    value::{Table, TableRef, Value},
};

mod network;
mod services;

use services::PackageServices;

// Baseline headless SDK compatibility profile. Local native fixtures prove the
// >= 2000 path; this is an advertised capability floor, not a device identity.
const BASELINE_VM_VERSION: u32 = 2_000;

#[derive(Clone, Debug)]
struct Bitmap {
    width: usize,
    height: usize,
    pixels: Vec<u16>,
    frame_height: Option<usize>,
    transparent_color: Option<u16>,
}

#[derive(Clone, Copy, Debug)]
struct BlitRegion {
    source_x: usize,
    source_y: usize,
    width: usize,
    height: usize,
    destination_x: i32,
    destination_y: i32,
    transparent_color: Option<u16>,
}

struct DirectorySearch {
    entries: Vec<Arc<[u8]>>,
    next: usize,
}

enum NativeFile {
    Host(File),
    Package(Cursor<Vec<u8>>),
}

enum ExtHelperInput<'a> {
    Buffer(&'a [u8]),
    Arguments([u32; 2]),
}

#[derive(Debug, Eq, PartialEq)]
enum ApplicationStackTransition {
    Stay,
    Push((Vec<u8>, Vec<u8>, PathBuf)),
    Pop,
}

pub(crate) enum PreparedEntry {
    Mr(Arc<Prototype>),
    Native(Vec<u8>),
}

pub(crate) struct PreparedApplication {
    package: Arc<Package>,
    entry: Vec<u8>,
    prepared_entry: PreparedEntry,
    stack_transition: ApplicationStackTransition,
    previous_application: Option<(Vec<u8>, Vec<u8>)>,
    start_file_parameter: [u8; START_FILE_PARAMETER_LEN],
    ext_runtime: Option<ExtRuntime>,
}

pub(crate) struct MrHost {
    pub package: Arc<Package>,
    pub framebuffer: Framebuffer,
    pub display: Box<dyn PlatformDisplay>,
    audio: Box<dyn PlatformAudio>,
    pub work_dir: PathBuf,
    font: Arc<[u8]>,
    memory_limit: u32,
    dns_mappings: Arc<[DnsMapping]>,
    wap_proxy_endpoint: Option<SocketAddrV4>,
    device_date: crate::DeviceDate,
    bitmaps: BTreeMap<i32, Bitmap>,
    directory_searches: BTreeMap<i32, DirectorySearch>,
    next_directory_handle: i32,
    native_files: BTreeMap<i32, NativeFile>,
    next_native_file_handle: i32,
    sdk_key: Option<i32>,
    current_entry: Vec<u8>,
    application_stack: Vec<(Vec<u8>, Vec<u8>, PathBuf)>,
    previous_application: Option<(Vec<u8>, Vec<u8>)>,
    start_file_parameter: [u8; START_FILE_PARAMETER_LEN],
    ext_runtime: Option<ExtRuntime>,
    socket_library: TableRef,
    mr_sockets: BTreeMap<i32, network::MrSocket>,
    next_mr_socket_handle: i32,
    mr_timer_interval: Option<Duration>,
    mr_timer_deadline: Option<Instant>,
    mr_timer_callback: Option<Arc<[u8]>>,
    mr_timer_pending: bool,
    mr_lifecycle_request: Option<ExtLifecycleRequest>,
    loaded_pack: Option<Arc<Package>>,
}

pub(crate) struct MrHostConfig {
    pub work_dir: PathBuf,
    pub font: Arc<[u8]>,
    pub memory_limit: u32,
    pub dns_mappings: Arc<[DnsMapping]>,
    pub device_date: crate::DeviceDate,
    pub wap_proxy_endpoint: Option<SocketAddrV4>,
}

impl MrHost {
    pub fn new(
        package: Arc<Package>,
        framebuffer: Framebuffer,
        display: Box<dyn PlatformDisplay>,
        audio: Box<dyn PlatformAudio>,
        config: MrHostConfig,
    ) -> Self {
        Self {
            package,
            framebuffer,
            display,
            audio,
            work_dir: config.work_dir,
            font: config.font,
            memory_limit: config.memory_limit,
            dns_mappings: config.dns_mappings,
            wap_proxy_endpoint: config.wap_proxy_endpoint,
            device_date: config.device_date,
            bitmaps: BTreeMap::new(),
            directory_searches: BTreeMap::new(),
            next_directory_handle: 1,
            native_files: BTreeMap::new(),
            next_native_file_handle: 1,
            sdk_key: None,
            current_entry: b"start.mr".to_vec(),
            application_stack: Vec::new(),
            previous_application: None,
            start_file_parameter: [0; START_FILE_PARAMETER_LEN],
            ext_runtime: None,
            socket_library: network::socket_library(),
            mr_sockets: BTreeMap::new(),
            next_mr_socket_handle: 1,
            mr_timer_interval: None,
            mr_timer_deadline: None,
            mr_timer_callback: None,
            mr_timer_pending: false,
            mr_lifecycle_request: None,
            loaded_pack: None,
        }
    }

    pub fn call(&mut self, name: &str, args: &[Value]) -> Result<Vec<Value>> {
        match name {
            "sys_get_info" => {
                let info = Table::new();
                {
                    let mut values = info.borrow_mut();
                    values.set(
                        bytes(b"scrw"),
                        Value::Number(f64::from(self.framebuffer.width())),
                    );
                    values.set(
                        bytes(b"scrh"),
                        Value::Number(f64::from(self.framebuffer.height())),
                    );
                    values.set(bytes(b"IMEI"), bytes(VIRTUAL_IMEI));
                    values.set(bytes(b"IMSI"), bytes(VIRTUAL_IMSI));
                }
                Ok(vec![Value::Table(info)])
            }
            "sys_find_start" => self.find_start(args),
            "sys_find_next" => self.find_next(args),
            "sys_find_stop" => self.find_stop(args),
            "GetSysInfo" => {
                let table = Table::new();
                table.borrow_mut().set(
                    bytes(b"ScreenW"),
                    Value::Number(f64::from(self.framebuffer.width())),
                );
                table.borrow_mut().set(
                    bytes(b"ScreenH"),
                    Value::Number(f64::from(self.framebuffer.height())),
                );
                table.borrow_mut().set(
                    bytes(b"vmver"),
                    Value::Number(f64::from(BASELINE_VM_VERSION)),
                );
                Ok(vec![Value::Table(table)])
            }
            "_platEx" => {
                let command = integer(args.first())?;
                match command {
                    1201 => Ok(vec![bytes(&[16, 16, 8, 16]), Value::Number(0.0)]),
                    _ => Err(crate::Error::Platform(format!(
                        "unsupported _platEx command {command}"
                    ))),
                }
            }
            "BitmapLoad" => {
                self.bitmap_load(args)?;
                Ok(Vec::new())
            }
            "BitmapShow" => {
                self.bitmap_show(args)?;
                Ok(Vec::new())
            }
            "SpriteSet" => {
                let id = integer(args.first())?;
                let frame_height = positive_usize(args.get(1), "sprite frame height")?;
                let bitmap = self.bitmaps.get_mut(&id).ok_or_else(|| {
                    crate::Error::Platform(format!("SpriteSet references missing bitmap {id}"))
                })?;
                bitmap.frame_height = Some(frame_height);
                Ok(Vec::new())
            }
            "SpriteDraw" => {
                self.sprite_draw(args)?;
                Ok(Vec::new())
            }
            "_drawRect" | "DrawRect" | "_effSetCon" => {
                let x = integer(args.first())?;
                let y = integer(args.get(1))?;
                let width = integer(args.get(2))?;
                let height = integer(args.get(3))?;
                let color = color(args, 4)?;
                self.framebuffer.rect(x, y, width, height, color);
                Ok(Vec::new())
            }
            "_drawLine" | "DrawLine" => {
                let x0 = integer(args.first())?;
                let y0 = integer(args.get(1))?;
                let x1 = integer(args.get(2))?;
                let y1 = integer(args.get(3))?;
                let color = color(args, 4)?;
                self.framebuffer.line(x0, y0, x1, y1, color);
                Ok(Vec::new())
            }
            "DrawText" => {
                let text = value_bytes(args.first())?;
                let x = integer(args.get(1))?;
                let y = integer(args.get(2))?;
                let color = color(args, 3)?;
                self.draw_text(&text, x, y, color);
                Ok(Vec::new())
            }
            "_textWidth" => self.text_width(args),
            "DispUpEx" => {
                self.framebuffer.mark_presented();
                self.display.present(&self.framebuffer)?;
                Ok(Vec::new())
            }
            "TestCom" => Ok(vec![Value::Number(0.0)]),
            "TestCom1" => self.test_com1(args),
            "_com" => self.com(args),
            // tcpip.mr owns the protocol state machine; these functions expose
            // the mutable buffers and host sockets that it drives.
            "_closeNet" => self.close_network(),
            "socket_tcp" => self.socket_tcp(),
            "socket_connect" => self.socket_connect(args),
            "socket_getstate" => self.socket_get_state(args),
            "socket_getinfo" => self.socket_get_info(args),
            "socket_send" => self.socket_send(args),
            "socket_receive" => self.socket_receive(args),
            "socket_close" => self.socket_close(args),
            "_strCom" => self.string_command(args),
            "LoadTable" => Ok(vec![Value::Nil]),
            "SaveTable" => Ok(vec![Value::Number(0.0)]),
            "LoadPack" => Ok(vec![Value::Nil]),
            "RunFile" => {
                let package = value_bytes(args.first())?;
                let entry = value_bytes(args.get(1))?;
                if package.is_empty() || entry.is_empty() {
                    return Err(crate::Error::MrFault(
                        "RunFile package and entry must not be empty".into(),
                    ));
                }
                self.mr_lifecycle_request = Some(ExtLifecycleRequest::Restart {
                    package: package.to_vec(),
                    entry: entry.to_vec(),
                });
                Ok(vec![Value::Number(0.0)])
            }
            "UAReset" => Ok(Vec::new()),
            "TimerStart" => self.timer_start(args),
            "TimerStop" => self.timer_stop(),
            "mr_c_load" => self.mr_c_load(args),
            "_gc" => Ok(Vec::new()),
            "Exit" => {
                self.mr_lifecycle_request = Some(ExtLifecycleRequest::Exit);
                Ok(Vec::new())
            }
            _ => Err(crate::Error::Platform(format!(
                "unsupported MR platform function {name}"
            ))),
        }
    }

    pub fn native_timer_due_in(&self) -> Option<Duration> {
        let mr_due = self
            .mr_timer_deadline
            .map(|deadline| deadline.saturating_duration_since(Instant::now()));
        match (
            mr_due,
            self.ext_runtime.as_ref().and_then(ExtRuntime::timer_due_in),
        ) {
            (Some(left), Some(right)) => Some(left.min(right)),
            (Some(due), None) | (None, Some(due)) => Some(due),
            (None, None) => None,
        }
    }

    pub fn take_due_native_timer(&mut self) -> Result<bool> {
        let now = Instant::now();
        let mr_due = self
            .mr_timer_deadline
            .is_some_and(|deadline| deadline <= now);
        if mr_due {
            self.mr_timer_deadline = self.mr_timer_interval.map(|interval| now + interval);
            self.mr_timer_pending = true;
            return Ok(true);
        }
        let native_due = match self.ext_runtime.as_mut() {
            Some(runtime) => runtime.take_due_timer()?,
            None => false,
        };
        Ok(mr_due || native_due)
    }

    pub fn take_due_mr_timer_callback(&mut self) -> Option<Arc<[u8]>> {
        if !std::mem::take(&mut self.mr_timer_pending) {
            return None;
        }
        self.mr_timer_callback.clone()
    }

    pub fn dispatch_native_timer(&mut self) -> Result<()> {
        self.call_ext_helper(2, ExtHelperInput::Buffer(&[]))?;
        Ok(())
    }

    pub fn dispatch_external_action_completion(&mut self) -> Result<bool> {
        let Some(mut runtime) = self.ext_runtime.take() else {
            return Ok(false);
        };
        let result = {
            let mut services = PackageServices {
                package: self.package.clone(),
                work_dir: self.work_dir.clone(),
                directory_searches: &mut self.directory_searches,
                next_directory_handle: &mut self.next_directory_handle,
                files: &mut self.native_files,
                next_file_handle: &mut self.next_native_file_handle,
                font: &self.font,
                framebuffer: &mut self.framebuffer,
                display: self.display.as_mut(),
                audio: self.audio.as_mut(),
            };
            runtime.dispatch_pending_external_action(&mut services)
        };
        self.ext_runtime = Some(runtime);
        result
    }

    pub fn dispatch_pending_platform_event(&mut self) -> Result<bool> {
        let Some(mut runtime) = self.ext_runtime.take() else {
            return Ok(false);
        };
        let result = {
            let mut services = PackageServices {
                package: self.package.clone(),
                work_dir: self.work_dir.clone(),
                directory_searches: &mut self.directory_searches,
                next_directory_handle: &mut self.next_directory_handle,
                files: &mut self.native_files,
                next_file_handle: &mut self.next_native_file_handle,
                font: &self.font,
                framebuffer: &mut self.framebuffer,
                display: self.display.as_mut(),
                audio: self.audio.as_mut(),
            };
            runtime.dispatch_pending_platform_event(&mut services)
        };
        self.ext_runtime = Some(runtime);
        result
    }

    pub(super) fn mr_file_remove(&mut self, name: &[u8]) -> Result<i32> {
        self.package_services().remove_file(name)
    }

    pub(super) fn mr_file_open(&mut self, name: &[u8], mode: u32) -> Result<i32> {
        self.package_services().open_file(name, mode)
    }

    pub(super) fn mr_file_write(&mut self, handle: i32, bytes: &[u8]) -> Result<Option<usize>> {
        self.package_services().write_file(handle, bytes)
    }

    pub(super) fn mr_file_read(&mut self, handle: i32, len: usize) -> Result<Option<Vec<u8>>> {
        self.package_services().read_file(handle, len)
    }

    pub(super) fn mr_file_seek(
        &mut self,
        handle: i32,
        offset: i32,
        origin: u32,
    ) -> Result<Option<u64>> {
        self.package_services().seek_file(handle, offset, origin)
    }

    pub(super) fn mr_file_close(&mut self, handle: i32) -> Result<i32> {
        self.package_services().close_file(handle)
    }

    pub(super) fn load_pack(&mut self, name: Option<&[u8]>) -> Result<bool> {
        let Some(name) = name else {
            self.loaded_pack = None;
            return Ok(true);
        };
        let Some(path) = native_file_path(
            &self.work_dir,
            self.package.path(),
            &self.package.header().internal_name,
            name,
        ) else {
            return Ok(false);
        };
        self.loaded_pack = match Package::open(path, self.package.limits().clone()) {
            Ok(package) => Some(Arc::new(package)),
            Err(crate::Error::Io { source, .. })
                if source.kind() == std::io::ErrorKind::NotFound =>
            {
                None
            }
            Err(error) => return Err(error),
        };
        Ok(self.loaded_pack.is_some())
    }

    pub(super) fn read_loaded_pack(&self, name: &[u8]) -> Result<Vec<u8>> {
        self.loaded_pack
            .as_ref()
            .ok_or_else(|| crate::Error::MrFault("no package is loaded".into()))?
            .read_named(name)
    }

    fn read_active_pack_resource(&self, name: &[u8]) -> Result<Vec<u8>> {
        match &self.loaded_pack {
            Some(package) => package.read_named(name),
            None => self.package.read_named(name),
        }
    }

    fn package_services(&mut self) -> PackageServices<'_> {
        PackageServices {
            package: self.package.clone(),
            work_dir: self.work_dir.clone(),
            directory_searches: &mut self.directory_searches,
            next_directory_handle: &mut self.next_directory_handle,
            files: &mut self.native_files,
            next_file_handle: &mut self.next_native_file_handle,
            font: &self.font,
            framebuffer: &mut self.framebuffer,
            display: self.display.as_mut(),
            audio: self.audio.as_mut(),
        }
    }

    pub fn lifecycle_request(&self) -> Result<Option<ExtLifecycleRequest>> {
        if let Some(request) = &self.mr_lifecycle_request {
            return Ok(Some(request.clone()));
        }
        match self.ext_runtime.as_ref() {
            Some(runtime) => runtime.lifecycle_request(),
            None => Ok(None),
        }
    }

    pub fn route_key_event(&mut self, code: i32, pressed: bool) -> Result<Option<(i32, i32, i32)>> {
        let Some(mut runtime) = self.ext_runtime.take() else {
            return Ok(Some((if pressed { 0 } else { 1 }, code, 0)));
        };
        let result = {
            let mut services = PackageServices {
                package: self.package.clone(),
                work_dir: self.work_dir.clone(),
                directory_searches: &mut self.directory_searches,
                next_directory_handle: &mut self.next_directory_handle,
                files: &mut self.native_files,
                next_file_handle: &mut self.next_native_file_handle,
                font: &self.font,
                framebuffer: &mut self.framebuffer,
                display: self.display.as_mut(),
                audio: self.audio.as_mut(),
            };
            runtime.route_key_event(code, pressed, &mut services)
        };
        self.ext_runtime = Some(runtime);
        result
    }

    pub fn route_pointer_event(
        &mut self,
        x: i32,
        y: i32,
        pressed: bool,
    ) -> Result<Option<(i32, i32, i32)>> {
        let Some(mut runtime) = self.ext_runtime.take() else {
            return Ok(Some((if pressed { 2 } else { 3 }, x, y)));
        };
        let result = {
            let mut services = PackageServices {
                package: self.package.clone(),
                work_dir: self.work_dir.clone(),
                directory_searches: &mut self.directory_searches,
                next_directory_handle: &mut self.next_directory_handle,
                files: &mut self.native_files,
                next_file_handle: &mut self.next_native_file_handle,
                font: &self.font,
                framebuffer: &mut self.framebuffer,
                display: self.display.as_mut(),
                audio: self.audio.as_mut(),
            };
            runtime.route_pointer_event(x, y, pressed, &mut services)
        };
        self.ext_runtime = Some(runtime);
        result
    }

    pub fn route_pointer_move(&self, x: i32, y: i32) -> Option<(i32, i32, i32)> {
        match self.ext_runtime.as_ref() {
            Some(runtime) => runtime.route_pointer_move(x, y),
            None => Some((12, x, y)),
        }
    }

    pub fn route_text_input(&mut self, text: &str) -> Result<Option<(i32, i32, i32)>> {
        let Some(mut runtime) = self.ext_runtime.take() else {
            return Ok(None);
        };
        let result = runtime.route_text_input(text);
        self.ext_runtime = Some(runtime);
        result
    }

    pub fn active_editor_text(&self) -> Option<String> {
        self.ext_runtime
            .as_ref()
            .and_then(ExtRuntime::active_editor_text)
    }

    pub fn dispatch_native_event(
        &mut self,
        event: i32,
        parameter0: i32,
        parameter1: i32,
    ) -> Result<()> {
        let mut input = [0_u8; 12];
        input[0..4].copy_from_slice(&event.to_le_bytes());
        input[4..8].copy_from_slice(&parameter0.to_le_bytes());
        input[8..12].copy_from_slice(&parameter1.to_le_bytes());
        self.call_ext_helper(1, ExtHelperInput::Buffer(&input))?;
        Ok(())
    }

    pub fn prepare_restart(
        &self,
        package_name: &[u8],
        entry: &[u8],
        limits: &ResourceLimits,
    ) -> Result<PreparedApplication> {
        // A bare package identity cannot distinguish duplicate installations. In that
        // ABI form, prefer the recorded parent at the top of the stack. A request that
        // carries a path is always resolved as that path, and the resolved target path
        // below decides whether this is a push, pop, or self-restart.
        let parent_identity_path =
            self.application_stack
                .last()
                .and_then(|(package, parent_entry, parent_path)| {
                    (is_identity_only_application_reference(package_name)
                        && package == package_name
                        && parent_entry == entry)
                        .then(|| parent_path.clone())
                });
        let unresolved_path = if let Some(parent_path) = parent_identity_path {
            parent_path
        } else {
            native_file_path(
                &self.work_dir,
                self.package.path(),
                &self.package.header().internal_name,
                package_name,
            )
            .ok_or_else(|| {
                crate::Error::Platform(format!(
                    "restart package path {:?} is outside the work directory",
                    String::from_utf8_lossy(package_name)
                ))
            })?
        };
        let path = resolve_native_work_path(&self.work_dir, &unresolved_path).ok_or_else(|| {
            crate::Error::Platform(format!(
                "restart package path {:?} is outside the work directory",
                String::from_utf8_lossy(package_name)
            ))
        })?;
        let package = Arc::new(Package::open(path, self.package.limits().clone())?);
        let entry_bytes = package.read_named(entry)?;
        let start_file_parameter = match self.ext_runtime.as_ref() {
            Some(runtime) => runtime.start_file_parameter()?,
            None => self.start_file_parameter,
        };
        let stack_transition = application_stack_transition(
            &self.application_stack,
            &self.package.header().internal_name,
            &self.current_entry,
            self.package.path(),
            package.path(),
            entry,
        );
        let previous_application = match &stack_transition {
            ApplicationStackTransition::Stay => self.previous_application.clone(),
            ApplicationStackTransition::Push(_) | ApplicationStackTransition::Pop => Some((
                self.package.header().internal_name.clone(),
                self.current_entry.clone(),
            )),
        };
        let (prepared_entry, ext_runtime) = if entry_bytes.starts_with(b"MRPGCMAP") {
            ExtRuntime::validate_module_image(&entry_bytes)?;
            let runtime = self.create_ext_runtime(
                &package,
                entry,
                previous_application.as_ref(),
                &start_file_parameter,
            )?;
            (PreparedEntry::Native(entry_bytes), Some(runtime))
        } else {
            if !entry_bytes.starts_with(b"\x1bMRP") {
                return Err(crate::Error::UnsupportedMr(format!(
                    "text MR frontend is not implemented for {}",
                    String::from_utf8_lossy(entry)
                )));
            }
            let chunk = MrChunk::load(&entry_bytes, limits)?;
            (PreparedEntry::Mr(chunk.root), None)
        };
        Ok(PreparedApplication {
            package,
            entry: entry.to_vec(),
            prepared_entry,
            stack_transition,
            previous_application,
            start_file_parameter,
            ext_runtime,
        })
    }

    pub fn acknowledge_lifecycle_request(&mut self) -> Result<()> {
        if self.mr_lifecycle_request.take().is_some() {
            return Ok(());
        }
        match self.ext_runtime.as_mut() {
            Some(runtime) => runtime.clear_lifecycle_request(),
            None => Ok(()),
        }
    }

    pub(crate) fn stop_audio(&mut self) {
        let _ = self.audio.stop_sound();
    }

    pub fn commit_application(&mut self, prepared: PreparedApplication) -> PreparedEntry {
        match prepared.stack_transition {
            ApplicationStackTransition::Stay => {}
            ApplicationStackTransition::Push(application) => {
                self.application_stack.push(application)
            }
            ApplicationStackTransition::Pop => {
                self.application_stack.pop();
            }
        }

        self.start_file_parameter = prepared.start_file_parameter;
        self.previous_application = prepared.previous_application;
        self.package = prepared.package;
        self.bitmaps.clear();
        self.stop_audio();
        self.directory_searches.clear();
        self.next_directory_handle = 1;
        self.native_files.clear();
        self.next_native_file_handle = 1;
        self.sdk_key = None;
        self.current_entry = prepared.entry;
        self.ext_runtime = prepared.ext_runtime;
        self.reset_mr_platform_state();
        prepared.prepared_entry
    }

    pub(crate) fn discard_failed_application_runtime(&mut self) {
        self.ext_runtime = None;
        self.bitmaps.clear();
        self.stop_audio();
        self.directory_searches.clear();
        self.next_directory_handle = 1;
        self.native_files.clear();
        self.next_native_file_handle = 1;
        self.sdk_key = None;
        self.reset_mr_platform_state();
    }

    fn reset_mr_platform_state(&mut self) {
        self.socket_library = network::socket_library();
        self.mr_sockets.clear();
        self.next_mr_socket_handle = 1;
        self.mr_timer_interval = None;
        self.mr_timer_deadline = None;
        self.mr_timer_callback = None;
        self.mr_timer_pending = false;
        self.loaded_pack = None;
    }

    pub fn set_current_entry(&mut self, entry: &[u8]) {
        self.current_entry = entry.to_vec();
    }

    fn bitmap_load(&mut self, args: &[Value]) -> Result<()> {
        let id = integer(args.first())?;
        let name = value_bytes(args.get(1))?;
        let width = positive_usize(args.get(4), "bitmap width")?;
        let height = positive_usize(args.get(5), "bitmap height")?;
        let raw = self.read_active_pack_resource(&name)?;
        let pixel_count = width
            .checked_mul(height)
            .ok_or_else(|| crate::Error::Platform(format!("bitmap {id} dimensions overflow")))?;
        let byte_count = pixel_count
            .checked_mul(2)
            .ok_or_else(|| crate::Error::Platform(format!("bitmap {id} byte count overflow")))?;
        let (pixels, transparent_color) = if raw.starts_with(b"BM") {
            (decode_bmp(&raw, width, height)?, None)
        } else {
            if raw.len() < byte_count {
                return Err(crate::Error::Platform(format!(
                    "bitmap {} contains {} bytes, needs {byte_count}",
                    String::from_utf8_lossy(&name),
                    raw.len()
                )));
            }
            let pixels = raw[..byte_count]
                .as_chunks::<2>()
                .0
                .iter()
                .map(|pixel| u16::from_le_bytes([pixel[0], pixel[1]]))
                .collect::<Vec<_>>();
            let transparent_color = pixels.first().copied();
            (pixels, transparent_color)
        };
        self.bitmaps.insert(
            id,
            Bitmap {
                width,
                height,
                pixels,
                frame_height: None,
                transparent_color,
            },
        );
        Ok(())
    }

    fn bitmap_show(&mut self, args: &[Value]) -> Result<()> {
        let id = integer(args.first())?;
        let x = integer(args.get(1))?;
        let y = integer(args.get(2))?;
        let bitmap = self.bitmaps.get(&id).ok_or_else(|| {
            crate::Error::Platform(format!("BitmapShow references missing bitmap {id}"))
        })?;
        blit(
            &mut self.framebuffer,
            bitmap,
            BlitRegion {
                source_x: 0,
                source_y: 0,
                width: bitmap.width,
                height: bitmap.height,
                destination_x: x,
                destination_y: y,
                transparent_color: None,
            },
        );
        Ok(())
    }

    fn sprite_draw(&mut self, args: &[Value]) -> Result<()> {
        let id = integer(args.first())?;
        let frame = integer(args.get(1))?.max(0) as usize;
        let x = integer(args.get(2))?;
        let y = integer(args.get(3))?;
        let bitmap = self.bitmaps.get(&id).ok_or_else(|| {
            crate::Error::Platform(format!("SpriteDraw references missing bitmap {id}"))
        })?;
        let frame_height = bitmap.frame_height.unwrap_or(bitmap.height);
        let source_y = frame.saturating_mul(frame_height);
        let source_y = if source_y == bitmap.height && frame > 0 {
            (frame - 1).saturating_mul(frame_height)
        } else {
            source_y
        };
        if source_y >= bitmap.height {
            return Ok(());
        }
        blit(
            &mut self.framebuffer,
            bitmap,
            BlitRegion {
                source_x: 0,
                source_y,
                width: bitmap.width,
                height: frame_height.min(bitmap.height - source_y),
                destination_x: x,
                destination_y: y,
                transparent_color: bitmap.transparent_color,
            },
        );
        Ok(())
    }

    fn draw_text(&mut self, encoded: &[u8], mut x: i32, y: i32, color: u16) {
        let (decoded, _, _) = GBK.decode(encoded);
        for character in decoded.chars() {
            let codepoint = character as usize;
            let width = if character.is_ascii() { 8 } else { 16 };
            let Some(start) = codepoint.checked_mul(32) else {
                continue;
            };
            let Some(glyph) = self.font.get(start..start + 32) else {
                x += width;
                continue;
            };
            for row in 0..16_i32 {
                let bits =
                    u16::from_be_bytes([glyph[row as usize * 2], glyph[row as usize * 2 + 1]]);
                for column in 0..width {
                    if bits & (0x8000_u16 >> column) != 0 {
                        self.framebuffer.point(x + column, y + row, color);
                    }
                }
            }
            x += width;
        }
    }

    fn text_width(&self, args: &[Value]) -> Result<Vec<Value>> {
        let width = match args.first() {
            Some(Value::Bytes(encoded)) => {
                let unicode = args.get(1).is_some_and(Value::truthy);
                if unicode {
                    encoded
                        .as_chunks::<2>()
                        .0
                        .iter()
                        .take_while(|bytes| **bytes != [0, 0])
                        .map(|bytes| {
                            if u16::from_be_bytes([bytes[0], bytes[1]]) < 128 {
                                8
                            } else {
                                16
                            }
                        })
                        .sum::<i32>()
                } else {
                    let (decoded, _, _) = GBK.decode(encoded);
                    decoded
                        .chars()
                        .map(|character| if character.is_ascii() { 8 } else { 16 })
                        .sum::<i32>()
                }
            }
            Some(Value::Number(codepoint)) => {
                if *codepoint >= 0.0 && *codepoint < 128.0 {
                    8
                } else {
                    16
                }
            }
            other => {
                return Err(crate::Error::MrFault(format!(
                    "_textWidth expects string or character code, got {other:?}"
                )));
            }
        };
        Ok(vec![Value::Number(f64::from(width)), Value::Number(16.0)])
    }

    fn com(&mut self, args: &[Value]) -> Result<Vec<Value>> {
        let command = integer(args.first())?;
        match command {
            // UI reset and screen mode notifications used by the baseline SDK.
            0 | 1 | 403 => Ok(vec![Value::Number(0.0)]),
            // Select the SDK network access point. Host networking is already
            // routed through the configured DNS/socket layer, so only validate
            // the legacy profile name and acknowledge the selection.
            402 => {
                let access_point = value_bytes(args.get(1))?;
                if !matches!(access_point.as_ref(), b"cmwap" | b"cmnet") {
                    return Err(crate::Error::Platform(format!(
                        "unsupported network access point {access_point:?}"
                    )));
                }
                self.initialize_network();
                Ok(vec![Value::Number(0.0)])
            }
            // Register the SDK compatibility key selected by start.mr.
            3629 => {
                self.sdk_key = Some(integer(args.get(1))?);
                Ok(vec![Value::Number(0.0)])
            }
            other => Err(crate::Error::Platform(format!(
                "unsupported _com command {other} with arguments {args:?}"
            ))),
        }
    }

    fn timer_start(&mut self, args: &[Value]) -> Result<Vec<Value>> {
        let milliseconds = integer(args.get(1))?;
        if milliseconds <= 0 {
            return Err(crate::Error::MrFault(format!(
                "TimerStart interval must be positive, got {milliseconds}"
            )));
        }
        let interval = Duration::from_millis(milliseconds as u64);
        let callback = value_bytes(args.get(2))?;
        self.mr_timer_interval = Some(interval);
        self.mr_timer_deadline = Some(Instant::now() + interval);
        self.mr_timer_callback = Some(callback);
        self.mr_timer_pending = false;
        Ok(vec![Value::Number(0.0)])
    }

    fn timer_stop(&mut self) -> Result<Vec<Value>> {
        self.mr_timer_interval = None;
        self.mr_timer_deadline = None;
        self.mr_timer_callback = None;
        self.mr_timer_pending = false;
        Ok(vec![Value::Number(0.0)])
    }

    fn string_command(&mut self, args: &[Value]) -> Result<Vec<Value>> {
        let command = integer(args.first())?;
        match command {
            // Optional carrier/SMS metadata suffix. The deterministic headless
            // profile has no provider, so it returns the ABI's neutral string.
            501 => {
                let input = value_bytes(args.get(1))?;
                if input.len() > 1024 {
                    return Err(crate::Error::Platform(format!(
                        "_strCom 501 input is {} bytes; limit is 1024",
                        input.len()
                    )));
                }
                Ok(vec![bytes(b"")])
            }
            // Read a checked byte range from the currently loaded MRP file.
            600 => {
                let requested = value_bytes(args.get(1))?;
                if matches!(requested.as_ref(), [b'*', b'A'..=b'Z']) {
                    // M0 firmware slots are empty until a host registers one.
                    return Ok(Vec::new());
                }
                let requested = requested.strip_prefix(b"%").unwrap_or(&requested);
                let current = self
                    .package
                    .path()
                    .file_name()
                    .and_then(|name| name.to_str())
                    .ok_or_else(|| {
                        crate::Error::Platform("current package name is not valid Unicode".into())
                    })?;
                if requested != current.as_bytes() {
                    return Err(crate::Error::Platform(format!(
                        "_strCom 600 cannot read package {} while {} is loaded",
                        String::from_utf8_lossy(requested),
                        self.package.path().display()
                    )));
                }
                let offset = nonnegative_usize(args.get(2), "package offset")?;
                let len = nonnegative_usize(args.get(3), "package range length")?;
                Ok(vec![bytes(&self.package.read_raw_range(offset, len)?)])
            }
            // 601 reads a package resource into a VM byte string.
            601 => {
                let name = value_bytes(args.get(1))?;
                Ok(vec![bytes(&self.package.read_named(&name)?)])
            }
            // Turn an MRPGCMAP image into the callable first-stage EXT loader.
            800 => {
                let code = integer(args.get(2))?;
                enum ImageSource {
                    Bytes(Arc<[u8]>),
                    GuestRange(GuestAddr, usize),
                }
                let source = match args.get(1) {
                    Some(Value::Bytes(image)) => ImageSource::Bytes(image.clone()),
                    Some(Value::Table(range)) => {
                        let range = range.borrow();
                        let address = guest_u32(&range.get(&Value::Number(1.0)), "EXT address")?;
                        let len =
                            guest_u32(&range.get(&Value::Number(2.0)), "EXT length")? as usize;
                        ImageSource::GuestRange(GuestAddr(address), len)
                    }
                    other => {
                        return Err(crate::Error::MrFault(format!(
                            "_strCom 800 expects EXT bytes or {{address, length}}, got {other:?}"
                        )));
                    }
                };
                let mut runtime = self.take_or_create_ext_runtime()?;
                let package = self.package.clone();
                let mut services = PackageServices {
                    package,
                    work_dir: self.work_dir.clone(),
                    directory_searches: &mut self.directory_searches,
                    next_directory_handle: &mut self.next_directory_handle,
                    files: &mut self.native_files,
                    next_file_handle: &mut self.next_native_file_handle,
                    font: &self.font,
                    framebuffer: &mut self.framebuffer,
                    display: self.display.as_mut(),
                    audio: self.audio.as_mut(),
                };
                let result = match source {
                    ImageSource::Bytes(image) => {
                        runtime.load_and_call_entry(&image, code, &mut services)
                    }
                    ImageSource::GuestRange(address, len) => {
                        runtime.load_guest_image_and_call_entry(address, len, code, &mut services)
                    }
                };
                self.ext_runtime = Some(runtime);
                Ok(vec![Value::Number(f64::from(result?))])
            }
            // Invoke the helper registered by the most recently loaded EXT.
            801 => {
                let input = ext_input(args.get(1))?;
                let code = integer(args.get(2))?;
                let (result, output) = self.call_ext_helper(code, input)?;
                Ok(vec![bytes(&output), Value::Number(f64::from(result))])
            }
            3 => Ok(vec![Value::Number(0.0)]),
            other => Err(crate::Error::Platform(format!(
                "unsupported _strCom command {other} with arguments {args:?}"
            ))),
        }
    }

    pub fn run_native_entry(&mut self, image: &[u8]) -> Result<()> {
        let mut runtime = self.take_or_create_ext_runtime()?;
        let package = self.package.clone();
        let result = {
            let mut services = PackageServices {
                package,
                work_dir: self.work_dir.clone(),
                directory_searches: &mut self.directory_searches,
                next_directory_handle: &mut self.next_directory_handle,
                files: &mut self.native_files,
                next_file_handle: &mut self.next_native_file_handle,
                font: &self.font,
                framebuffer: &mut self.framebuffer,
                display: self.display.as_mut(),
                audio: self.audio.as_mut(),
            };
            runtime.load_and_call_entry(image, 0, &mut services)
        };
        self.ext_runtime = Some(runtime);
        result?;

        self.call_ext_helper(6, ExtHelperInput::Arguments([1, BASELINE_VM_VERSION]))?;
        self.call_ext_helper(0, ExtHelperInput::Buffer(&[]))?;
        Ok(())
    }

    fn take_or_create_ext_runtime(&mut self) -> Result<ExtRuntime> {
        if let Some(runtime) = self.ext_runtime.take() {
            return Ok(runtime);
        }
        self.create_ext_runtime(
            &self.package,
            &self.current_entry,
            self.previous_application.as_ref(),
            &self.start_file_parameter,
        )
    }

    fn create_ext_runtime(
        &self,
        package: &Package,
        entry: &[u8],
        previous_application: Option<&(Vec<u8>, Vec<u8>)>,
        start_file_parameter: &[u8; START_FILE_PARAMETER_LEN],
    ) -> Result<ExtRuntime> {
        let mut runtime = ExtRuntime::new(
            self.framebuffer.width(),
            self.framebuffer.height(),
            &package.header().internal_name,
            entry,
            self.memory_limit,
        )?;
        runtime.set_native_extension_profile(native_extension_profile(
            package.header().platform,
            package.header().version,
        ))?;
        runtime.set_dns_mappings(self.dns_mappings.clone());
        runtime.set_wap_proxy_endpoint(self.wap_proxy_endpoint);
        runtime.set_device_date(self.device_date);
        let (previous_package, previous_entry) = previous_application
            .map(|(package, entry)| (package.as_slice(), entry.as_slice()))
            .unwrap_or((&[], &[]));
        runtime.set_previous_application(previous_package, previous_entry)?;
        runtime.set_start_file_parameter(start_file_parameter)?;
        Ok(runtime)
    }

    fn mr_c_load(&mut self, args: &[Value]) -> Result<Vec<Value>> {
        let code = integer(args.first())?;
        let input = args
            .get(1)
            .and_then(Value::bytes)
            .unwrap_or_else(|| Arc::from(&b""[..]));
        let (result, output) =
            self.call_ext_helper(code, ExtHelperInput::Buffer(input.as_ref()))?;
        Ok(vec![Value::Number(f64::from(result)), bytes(&output)])
    }

    fn call_ext_helper(&mut self, code: i32, input: ExtHelperInput<'_>) -> Result<(i32, Vec<u8>)> {
        let package = self.package.clone();
        let mut runtime = self
            .ext_runtime
            .take()
            .ok_or_else(|| crate::Error::Abi("no EXT runtime has been initialized".into()))?;
        let result = {
            let mut services = PackageServices {
                package,
                work_dir: self.work_dir.clone(),
                directory_searches: &mut self.directory_searches,
                next_directory_handle: &mut self.next_directory_handle,
                files: &mut self.native_files,
                next_file_handle: &mut self.next_native_file_handle,
                font: &self.font,
                framebuffer: &mut self.framebuffer,
                display: self.display.as_mut(),
                audio: self.audio.as_mut(),
            };
            match input {
                ExtHelperInput::Buffer(input) => {
                    runtime.call_active_helper(code, input, &mut services)
                }
                ExtHelperInput::Arguments(arguments) => {
                    runtime.call_active_helper_raw(code, arguments, &mut services)
                }
            }
        };
        self.ext_runtime = Some(runtime);
        result
    }

    fn find_start(&mut self, args: &[Value]) -> Result<Vec<Value>> {
        let directory = value_bytes(args.first())?;
        let Some(path) = safe_work_path(&self.work_dir, &directory) else {
            return Ok(vec![Value::Number(-1.0), bytes(b"")]);
        };
        let Ok(entries) = fs::read_dir(path) else {
            return Ok(vec![Value::Number(-1.0), bytes(b"")]);
        };
        let mut names = entries
            .filter_map(|entry| entry.ok())
            .map(|entry| {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                let (encoded, _, _) = GBK.encode(&name);
                Arc::<[u8]>::from(encoded.as_ref())
            })
            .collect::<Vec<_>>();
        names.sort();

        let handle = self.allocate_directory_handle()?;
        let first = names
            .first()
            .cloned()
            .unwrap_or_else(|| Arc::from(&b""[..]));
        self.directory_searches.insert(
            handle,
            DirectorySearch {
                entries: names,
                next: 1,
            },
        );
        Ok(vec![Value::Number(f64::from(handle)), Value::Bytes(first)])
    }

    fn find_next(&mut self, args: &[Value]) -> Result<Vec<Value>> {
        let handle = integer(args.first())?;
        let Some(search) = self.directory_searches.get_mut(&handle) else {
            return Ok(vec![Value::Nil]);
        };
        let Some(name) = search.entries.get(search.next).cloned() else {
            return Ok(vec![Value::Nil]);
        };
        search.next += 1;
        Ok(vec![Value::Bytes(name)])
    }

    fn find_stop(&mut self, args: &[Value]) -> Result<Vec<Value>> {
        let handle = integer(args.first())?;
        Ok(vec![Value::Number(
            if self.directory_searches.remove(&handle).is_some() {
                0.0
            } else {
                -1.0
            },
        )])
    }

    fn allocate_directory_handle(&mut self) -> Result<i32> {
        let start = self.next_directory_handle;
        loop {
            let handle = self.next_directory_handle;
            self.next_directory_handle = self.next_directory_handle.checked_add(1).unwrap_or(1);
            if !self.directory_searches.contains_key(&handle) {
                return Ok(handle);
            }
            if self.next_directory_handle == start {
                return Err(crate::Error::ResourceLimit(
                    "no directory search handles available".into(),
                ));
            }
        }
    }
}

fn decode_bmp(raw: &[u8], target_width: usize, target_height: usize) -> Result<Vec<u16>> {
    if raw.len() < 54 {
        return Err(crate::Error::Package("BMP header is truncated".into()));
    }
    let u16_at = |offset: usize| {
        raw.get(offset..offset + 2)
            .and_then(|bytes| bytes.try_into().ok())
            .map(u16::from_le_bytes)
    };
    let u32_at = |offset: usize| {
        raw.get(offset..offset + 4)
            .and_then(|bytes| bytes.try_into().ok())
            .map(u32::from_le_bytes)
    };
    let i32_at = |offset: usize| {
        raw.get(offset..offset + 4)
            .and_then(|bytes| bytes.try_into().ok())
            .map(i32::from_le_bytes)
    };
    let pixel_offset = usize::try_from(u32_at(10).unwrap()).unwrap();
    let source_width = i32_at(18).unwrap();
    let signed_height = i32_at(22).unwrap();
    let planes = u16_at(26).unwrap();
    let bits_per_pixel = usize::from(u16_at(28).unwrap());
    let compression = u32_at(30).unwrap();
    if source_width <= 0
        || signed_height == 0
        || signed_height == i32::MIN
        || planes != 1
        || !matches!(bits_per_pixel, 24 | 32)
        || compression != 0
    {
        return Err(crate::Error::Package(
            "unsupported BMP dimensions or pixel format".into(),
        ));
    }
    let source_width = usize::try_from(source_width).unwrap();
    let source_height = usize::try_from(signed_height.abs()).unwrap();
    let row_bits = source_width
        .checked_mul(bits_per_pixel)
        .ok_or_else(|| crate::Error::ResourceLimit("BMP row size overflows".into()))?;
    let row_stride = row_bits
        .checked_add(31)
        .map(|bits| bits / 32 * 4)
        .ok_or_else(|| crate::Error::ResourceLimit("BMP row stride overflows".into()))?;
    let pixel_bytes = row_stride
        .checked_mul(source_height)
        .ok_or_else(|| crate::Error::ResourceLimit("BMP pixel range overflows".into()))?;
    let pixel_end = pixel_offset
        .checked_add(pixel_bytes)
        .ok_or_else(|| crate::Error::ResourceLimit("BMP pixel offset overflows".into()))?;
    if pixel_end > raw.len() {
        return Err(crate::Error::Package("BMP pixel data is truncated".into()));
    }
    let output_len = target_width
        .checked_mul(target_height)
        .ok_or_else(|| crate::Error::ResourceLimit("BMP output dimensions overflow".into()))?;
    let bytes_per_pixel = bits_per_pixel / 8;
    let bottom_up = signed_height > 0;
    let mut pixels = Vec::with_capacity(output_len);
    for target_y in 0..target_height {
        let source_y = target_y * source_height / target_height;
        let stored_y = if bottom_up {
            source_height - 1 - source_y
        } else {
            source_y
        };
        let row = pixel_offset + stored_y * row_stride;
        for target_x in 0..target_width {
            let source_x = target_x * source_width / target_width;
            let pixel = row + source_x * bytes_per_pixel;
            let blue = u16::from(raw[pixel]);
            let green = u16::from(raw[pixel + 1]);
            let red = u16::from(raw[pixel + 2]);
            pixels.push(((red >> 3) << 11) | ((green >> 2) << 5) | (blue >> 3));
        }
    }
    Ok(pixels)
}

fn package_entry_path(internal_name: &[u8], guest_name: &[u8]) -> Option<Vec<u8>> {
    let components = guest_name
        .split(|byte| matches!(byte, b'/' | b'\\'))
        .filter(|component| !component.is_empty() && *component != b".")
        .collect::<Vec<_>>();
    if components.is_empty() || components.iter().any(|component| *component == b"..") {
        return None;
    }
    let package_stem = internal_name.strip_suffix(b".mrp").unwrap_or(internal_name);
    let components = if components.len() > 1 && components[0] == package_stem {
        &components[1..]
    } else {
        &components[..]
    };
    let mut path = Vec::new();
    for component in components {
        if !path.is_empty() {
            path.push(b'/');
        }
        path.extend_from_slice(component);
    }
    Some(path)
}

fn is_mrp_file_name(path: &[u8]) -> bool {
    let name = path
        .rsplit(|byte| matches!(byte, b'/' | b'\\'))
        .next()
        .unwrap_or_default();
    name.len() >= 4 && name[name.len() - 4..].eq_ignore_ascii_case(b".mrp")
}

fn native_extension_profile(platform: u8, version: u32) -> NativeExtensionProfile {
    // Versions from 1000 onward on this platform use the fixed MTK native-code window.
    if platform == 1 && version >= 1_000 {
        NativeExtensionProfile::Mtk
    } else {
        NativeExtensionProfile::Baseline
    }
}

fn safe_work_path(work_dir: &Path, bytes: &[u8]) -> Option<PathBuf> {
    let mut path = std::str::from_utf8(bytes).ok()?;
    if path.starts_with('/') || path.starts_with('\\') {
        return None;
    }
    let mut resolved = work_dir.to_path_buf();
    if path.len() < 2 || path.as_bytes()[1] != b':' {
        let first_component = path
            .split(['/', '\\'])
            .find(|component| !matches!(*component, "" | "."));
        if !first_component.is_some_and(|component| component.eq_ignore_ascii_case("mythroad")) {
            resolved.push("mythroad");
        }
    } else {
        match path.as_bytes()[0].to_ascii_uppercase() {
            b'C' => {}
            drive @ (b'X' | b'Y' | b'Z') => {
                resolved.push("disk");
                resolved.push(char::from(drive.to_ascii_lowercase()).to_string());
            }
            _ => return None,
        }
        path = &path[2..];
        if !path.is_empty() && !path.starts_with('/') && !path.starts_with('\\') {
            return None;
        }
    }
    for component in path
        .split(['/', '\\'])
        .filter(|component| !matches!(*component, "" | "."))
    {
        match component {
            ".." => return None,
            component if component.contains('\0') || component.contains(':') => return None,
            component => resolved.push(component),
        }
    }
    Some(resolved)
}

fn native_file_path(
    work_dir: &Path,
    package_path: &Path,
    package_internal_name: &[u8],
    bytes: &[u8],
) -> Option<PathBuf> {
    if !package_internal_name.is_empty() && bytes == package_internal_name {
        return Some(package_path.to_path_buf());
    }
    let path = std::str::from_utf8(bytes).ok().map(Path::new)?;
    if path.components().count() == 1 && package_path.file_name() == Some(path.as_os_str()) {
        return Some(package_path.to_path_buf());
    }
    safe_work_path(work_dir, bytes)
}

fn is_identity_only_application_reference(package: &[u8]) -> bool {
    !package.is_empty() && !package.iter().any(|byte| matches!(byte, b'/' | b'\\'))
}

#[derive(Debug, PartialEq, Eq)]
enum NativePathComponent {
    Missing,
    Match(OsString),
    Ambiguous,
}

fn select_ascii_case_component(
    requested: &OsStr,
    candidates: impl IntoIterator<Item = OsString>,
) -> NativePathComponent {
    let Some(requested_text) = requested.to_str() else {
        return NativePathComponent::Missing;
    };
    let mut folded_match = None;
    let mut ambiguous = false;
    for candidate in candidates {
        if candidate == requested {
            return NativePathComponent::Match(candidate);
        }
        if candidate
            .to_str()
            .is_some_and(|candidate| candidate.eq_ignore_ascii_case(requested_text))
        {
            if folded_match.is_some() {
                ambiguous = true;
            } else {
                folded_match = Some(candidate);
            }
        }
    }
    if ambiguous {
        NativePathComponent::Ambiguous
    } else {
        folded_match
            .map(NativePathComponent::Match)
            .unwrap_or(NativePathComponent::Missing)
    }
}

fn resolve_native_work_path(work_dir: &Path, target: &Path) -> Option<PathBuf> {
    let Ok(relative) = target.strip_prefix(work_dir) else {
        return Some(target.to_path_buf());
    };
    let mut resolved = work_dir.to_path_buf();
    for component in relative.components() {
        let std::path::Component::Normal(requested) = component else {
            return None;
        };
        let exact = resolved.join(requested);
        match fs::symlink_metadata(&exact) {
            Ok(metadata) if metadata.file_type().is_symlink() => return None,
            Ok(_) => {
                resolved = exact;
                continue;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return None,
        }
        let candidates = match fs::read_dir(&resolved) {
            Ok(entries) => entries
                .filter_map(|entry| entry.ok().map(|entry| entry.file_name()))
                .collect::<Vec<_>>(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                resolved = exact;
                continue;
            }
            Err(_) => return None,
        };
        match select_ascii_case_component(requested, candidates) {
            NativePathComponent::Missing => resolved = exact,
            NativePathComponent::Match(actual) => {
                let matched = resolved.join(actual);
                let metadata = fs::symlink_metadata(&matched).ok()?;
                if metadata.file_type().is_symlink() {
                    return None;
                }
                resolved = matched;
            }
            NativePathComponent::Ambiguous => return None,
        }
    }
    Some(resolved)
}

fn application_stack_transition(
    stack: &[(Vec<u8>, Vec<u8>, PathBuf)],
    current_package: &[u8],
    current_entry: &[u8],
    current_path: &Path,
    target_path: &Path,
    target_entry: &[u8],
) -> ApplicationStackTransition {
    if current_path == target_path && current_entry == target_entry {
        return ApplicationStackTransition::Stay;
    }
    if stack
        .last()
        .is_some_and(|(_, entry, path)| path == target_path && entry == target_entry)
    {
        ApplicationStackTransition::Pop
    } else {
        ApplicationStackTransition::Push((
            current_package.to_vec(),
            current_entry.to_vec(),
            current_path.to_path_buf(),
        ))
    }
}

fn blit(framebuffer: &mut Framebuffer, bitmap: &Bitmap, region: BlitRegion) {
    for row in 0..region.height {
        for column in 0..region.width {
            let pixel =
                bitmap.pixels[(region.source_y + row) * bitmap.width + region.source_x + column];
            if Some(pixel) != region.transparent_color {
                framebuffer.point(
                    region.destination_x + column as i32,
                    region.destination_y + row as i32,
                    pixel,
                );
            }
        }
    }
}

fn color(args: &[Value], offset: usize) -> Result<u16> {
    Ok(Framebuffer::rgb565(
        integer(args.get(offset))?,
        integer(args.get(offset + 1))?,
        integer(args.get(offset + 2))?,
    ))
}

fn integer(value: Option<&Value>) -> Result<i32> {
    let value = value.ok_or_else(|| crate::Error::MrFault("missing numeric argument".into()))?;
    let number = value
        .number()
        .ok_or_else(|| crate::Error::MrFault(format!("expected number, got {value:?}")))?;
    if !number.is_finite() || number < i32::MIN as f64 || number > i32::MAX as f64 {
        return Err(crate::Error::MrFault(format!(
            "number {number} does not fit i32"
        )));
    }
    Ok(number as i32)
}

fn guest_u32(value: &Value, label: &str) -> Result<u32> {
    let number = value
        .number()
        .ok_or_else(|| crate::Error::MrFault(format!("{label} is not numeric: {value:?}")))?;
    if !number.is_finite() || number < 0.0 || number > f64::from(u32::MAX) {
        return Err(crate::Error::MrFault(format!("invalid {label}: {number}")));
    }
    Ok(number as u32)
}

fn ext_input<'a>(value: Option<&'a Value>) -> Result<ExtHelperInput<'a>> {
    match value {
        Some(Value::Bytes(bytes)) => Ok(ExtHelperInput::Buffer(bytes.as_ref())),
        Some(Value::Table(table)) => {
            let table = table.borrow();
            let len = table.sequence_len();
            if len > 2 {
                return Err(crate::Error::MrFault(format!(
                    "EXT helper argument table has {len} items; the ABI accepts at most 2"
                )));
            }
            let mut arguments = [0; 2];
            for index in 1..=len {
                let value = table.get(&Value::Number(index as f64));
                let number = value.number().ok_or_else(|| {
                    crate::Error::MrFault(format!(
                        "EXT input table item {index} is not numeric: {value:?}"
                    ))
                })?;
                if !number.is_finite() || number < i32::MIN as f64 || number > u32::MAX as f64 {
                    return Err(crate::Error::MrFault(format!(
                        "EXT input table item {index} does not fit 32 bits: {number}"
                    )));
                }
                arguments[index - 1] = number as i64 as u32;
            }
            Ok(ExtHelperInput::Arguments(arguments))
        }
        other => Err(crate::Error::MrFault(format!(
            "_strCom 801 expects bytes or a numeric sequence, got {other:?}"
        ))),
    }
}

fn positive_usize(value: Option<&Value>, label: &str) -> Result<usize> {
    let value = integer(value)?;
    usize::try_from(value)
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| crate::Error::MrFault(format!("invalid {label}: {value}")))
}

fn nonnegative_usize(value: Option<&Value>, label: &str) -> Result<usize> {
    let value = integer(value)?;
    usize::try_from(value).map_err(|_| crate::Error::MrFault(format!("invalid {label}: {value}")))
}

fn value_bytes(value: Option<&Value>) -> Result<Arc<[u8]>> {
    let value = value.ok_or_else(|| crate::Error::MrFault("missing string argument".into()))?;
    value
        .bytes()
        .ok_or_else(|| crate::Error::MrFault(format!("expected string, got {value:?}")))
}

fn bytes(value: &[u8]) -> Value {
    Value::Bytes(Arc::from(value))
}

#[cfg(test)]
mod tests;
