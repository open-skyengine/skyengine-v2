use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    io::{Read, Write},
    net::{Ipv4Addr, SocketAddr, SocketAddrV4, TcpStream},
    path::PathBuf,
    sync::{Arc, mpsc},
    thread,
    time::{Duration, Instant},
};

use flate2::read::GzDecoder;

use crate::{
    DeviceDate, DnsMapping, Error, Framebuffer, Package, ResourceLimits, Result, SoundType,
    VIRTUAL_IMEI, VIRTUAL_IMSI,
};

use super::{ArmCpu, GuestAddr, GuestMemory, Permissions};
mod dispatch;
mod graphics;
mod heap;
mod libc;
mod md5;
mod network;
mod platform;
mod ram_package;

const PLATFORM_TABLE: GuestAddr = GuestAddr(0x0100_0000);
const PLATFORM_DATA: GuestAddr = GuestAddr(0x0100_1000);
const PLATFORM_RESOURCE_BACKING_LEN: usize = 0x1000;
const BITMAP_ARRAY_DATA: GuestAddr = GuestAddr(0x0100_2000);
const TILE_ARRAY_DATA: GuestAddr = GuestAddr(0x0100_4000);
const MAP_ARRAY_DATA: GuestAddr = GuestAddr(0x0100_6000);
const SOUND_ARRAY_DATA: GuestAddr = GuestAddr(0x0100_8000);
const SPRITE_ARRAY_DATA: GuestAddr = GuestAddr(0x0100_a000);
const SMS_CONFIG_DATA: GuestAddr = GuestAddr(0x0100_c000);
const START_FILE_PARAMETER_DATA: GuestAddr = GuestAddr(0x0100_d000);
pub(crate) const START_FILE_PARAMETER_LEN: usize = 128;
const PACKAGE_NAME_DATA: GuestAddr = GuestAddr(0x0100_1400);
const START_NAME_DATA: GuestAddr = GuestAddr(0x0100_1500);
const PREVIOUS_PACKAGE_NAME_DATA: GuestAddr = GuestAddr(0x0100_1600);
const PREVIOUS_START_NAME_DATA: GuestAddr = GuestAddr(0x0100_1700);
const CURRENT_ENTRY_DATA: GuestAddr = GuestAddr(0x0100_1800);
const INTERNAL_TABLE_DATA: GuestAddr = GuestAddr(0x0100_1900);
const APPLICATION_STATE_DATA: GuestAddr = GuestAddr(0x0100_1980);
const LIFECYCLE_CALLBACK_DATA: GuestAddr = GuestAddr(0x0100_1984);
const TIMER_ACTIVE_DATA: GuestAddr = GuestAddr(0x0100_1988);
const EXT_CHUNK_MAGIC: u32 = 0x7fd8_54eb;
const DYNAMIC_IMAGE_PARAMETER_OFFSET: usize = 4;
const DYNAMIC_IMAGE_ENTRY_OFFSET: u32 = 8;
const MODULE_PARAMETER_LEN: usize = 0x14;
const MODULE_PARAMETER_RW_LEN_OFFSET: u32 = 0x04;
const MODULE_PARAMETER_EXT_CHUNK_OFFSET: u32 = 0x0c;
const EXT_CHUNK_ENTRY_OFFSET: u32 = 0x04;
const EXT_CHUNK_IMAGE_ADDRESS_OFFSET: u32 = 0x0c;
const EXT_CHUNK_IMAGE_LEN_OFFSET: u32 = 0x10;
const EXT_CHUNK_PARAMETER_OFFSET: u32 = 0x1c;
const EXT_CHUNK_PARAMETER_LEN_OFFSET: u32 = 0x20;
const EXT_CHUNK_SUSPEND_DEPTH_OFFSET: u32 = 0x34;
const EXT_CHUNK_TIMER_STATE_LEN: usize = 0x38;
const COMPACT_TIMER_MAGIC: u32 = 0x79ab_bccf;
const COMPACT_TIMER_PERIOD_OFFSET: u32 = 0x04;
const COMPACT_TIMER_HANDLER_OFFSET: u32 = 0x0c;
const COMPACT_TIMER_DATA_OFFSET: u32 = 0x10;
const COMPACT_TIMER_REPEAT_OFFSET: u32 = 0x14;
const COMPACT_TIMER_TAIL_OFFSET: u32 = 0x18;
const COMPACT_TIMER_NODE_LEN: usize = 0x1c;
const MAX_COMPACT_TIMER_POINTER_SCAN_LEN: usize = 1024 * 1024;
const MAX_TRACKED_COMPACT_TIMERS: usize = 1024;
const APPLICATION_STATE_NORMAL: u32 = 1;
const APPLICATION_STATE_RESTART_PENDING: u32 = 3;
const PLATFORM_SIM_INFO_DATA: GuestAddr = GuestAddr(0x0100_1a00);
const PLATFORM_SIM_INFO_LEN: usize = 12;
const PLATFORM_STORAGE_INFO_DATA: GuestAddr = GuestAddr(0x0100_1a10);
const PLATFORM_STORAGE_INFO_LEN: usize = 16;
const PLATFORM_STORAGE_DRIVE_DATA: GuestAddr = GuestAddr(0x0100_1a20);
const PLATFORM_STORAGE_DRIVE: &[u8] = b"C:/mythroad/";
const PLATFORM_STORAGE_DRIVE_LEN: usize = PLATFORM_STORAGE_DRIVE.len();
const PLATFORM_JPEG_INFO_DATA: GuestAddr = GuestAddr(0x0100_1a30);
const PLATFORM_JPEG_INFO_LEN: usize = 8;
const FIRMWARE_SLOT_DATA: GuestAddr = GuestAddr(0x0100_1b00);
const FIRMWARE_SLOT_COUNT: u32 = 26;
const PLATFORM_RUNTIME_PROFILE_DATA: GuestAddr = GuestAddr(0x0100_1b80);
const PLATFORM_RUNTIME_PROFILE_LEN: usize = 12;
const PLATFORM_USER_INFO_LEN: usize = 64;
// Common MTK EXT fixtures identify the 1.0.4 runtime through this encoded version.
const PLATFORM_USER_INFO_VERSION: u32 = 101_040_000;
const MTK_NATIVE_EXTENSION_BASE: GuestAddr = GuestAddr(0x4001_8800);
const MTK_NATIVE_EXTENSION_LEN: usize = MODULE_STRIDE as usize;
const PLATFORM_STORAGE_BLOCK_SIZE: u32 = 4 * 1024;
const PLATFORM_STORAGE_AVAILABLE_BLOCKS: u32 = 32 * 1024;
const PLATFORM_STORAGE_TOTAL_BLOCKS: u32 = PLATFORM_STORAGE_AVAILABLE_BLOCKS * 2;
const INTERNAL_APPLICATION_STATE_OFFSETS: [u32; 2] = [8, 44];
const MODULE_BASE: u32 = 0x1000_0000;
const MODULE_STRIDE: u32 = 0x0010_0000;
const HEAP_BASE: GuestAddr = GuestAddr(0x2000_0000);
const MIN_GUEST_RAM_LEN: usize = 8 * 1024 * 1024;
const MAX_GUEST_HEAP_LEN: usize = 16 * 1024 * 1024;
const GUEST_MEMORY_GUARD_LEN: u32 = 4 * 1024;
const SCREEN_STAGING_CAPACITY: usize = 1024 * 1024;
#[cfg(test)]
const DEFAULT_HEAP_LEN: usize = 4 * 1024 * 1024;
const STACK_BASE: GuestAddr = GuestAddr(0x3000_0000);
const STACK_LEN: usize = 256 * 1024;
const PLATFORM_MEMORY_BASE: GuestAddr = GuestAddr(0x4000_0000);
const MAX_PLATFORM_MEMORY_EXTENSION_LEN: usize = 16 * 1024 * 1024;
const DETACHED_GUEST_ALLOCATION_BASE: GuestAddr = GuestAddr(0x5000_0000);
const LEGACY_KEYPAD_REGISTERS: GuestAddr = GuestAddr(0x8011_0000);
const LEGACY_KEYPAD_REGISTERS_LEN: usize = 16;
const SCREEN_BASE: GuestAddr = GuestAddr(HEAP_BASE.0 + MIN_GUEST_RAM_LEN as u32);
const EXPANDED_SCREEN_BASE: GuestAddr =
    GuestAddr(HEAP_BASE.0 + MAX_GUEST_HEAP_LEN as u32 + GUEST_MEMORY_GUARD_LEN);
const FREE_BLOCK_HEADER_LEN: u32 = 8;
const HEAP_ALIGNMENT: u32 = 8;
const BITMAP_ENTRY_SIZE: u32 = 16;
const SCREEN_BITMAP_ID: u32 = 30;
const TRAP_BASE: u32 = 0xff00_0000;
const RETURN_SENTINEL: u32 = 0xffff_ff00;
const PLATFORM_SLOT_COUNT: u32 = 150;
const INSTRUCTION_BUDGET: u64 = 200_000_000;
const MD5_BUFFER_OFFSET: u32 = 24;
const MAX_NATIVE_SOCKETS: usize = 64;
const MAX_PLATFORM_UI_HANDLES: usize = 64;
const MAX_PLATFORM_MENU_ITEMS: usize = 1024;
const MAX_PLATFORM_EDITOR_CODE_UNITS: usize = 4096;
const MAX_PENDING_PLATFORM_MENU_RETURNS: usize = 1024;
const MAX_PENDING_SMS_RESULTS: usize = 32;
const MAX_GUEST_ALLOCATION_VIEWS: usize = 256;
const NETWORK_CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const NETWORK_FIRST_RECEIVE_TIMEOUT: Duration = Duration::from_secs(3);
const MAX_PENDING_EXTERNAL_ACTIONS: usize = 32;
const LEGACY_EXTERNAL_ACTION_KINDS: [u32; 1] = [2];

pub(crate) trait NativeServices {
    fn resize_screen(&mut self, width: u16, height: u16) -> Result<()>;
    fn start_shake(&mut self, _milliseconds: u32) -> Result<()> {
        Ok(())
    }
    fn stop_shake(&mut self) -> Result<()> {
        Ok(())
    }
    fn capture_framebuffer(&mut self) -> Result<Option<Vec<u8>>> {
        Ok(None)
    }
    fn read_package_file(&mut self, package_name: &[u8], name: &[u8]) -> Result<Option<Vec<u8>>>;
    fn file_info(&mut self, name: &[u8]) -> Result<i32>;
    fn remove_file(&mut self, name: &[u8]) -> Result<i32>;
    fn rename_file(&mut self, source: &[u8], destination: &[u8]) -> Result<i32>;
    fn create_dir(&mut self, name: &[u8]) -> Result<i32>;
    fn remove_dir(&mut self, name: &[u8]) -> Result<i32>;
    fn open_file(&mut self, name: &[u8], mode: u32) -> Result<i32>;
    fn close_file(&mut self, handle: i32) -> Result<i32>;
    fn write_file(&mut self, handle: i32, bytes: &[u8]) -> Result<Option<usize>>;
    fn read_file(&mut self, handle: i32, len: usize) -> Result<Option<Vec<u8>>>;
    fn seek_file(&mut self, handle: i32, offset: i32, origin: u32) -> Result<Option<u64>>;
    fn file_len(&mut self, name: &[u8]) -> Result<Option<u64>>;
    fn find_start(&mut self, directory: &[u8]) -> Result<Option<(i32, Vec<u8>)>>;
    fn find_next(&mut self, handle: i32) -> Result<Option<Vec<u8>>>;
    fn find_stop(&mut self, handle: i32) -> Result<bool>;
    /// Returns an MSB-first glyph with two bytes per scanline. The platform
    /// table adapter repacks narrow glyphs to the guest ABI's byte stride.
    fn char_bitmap(&mut self, codepoint: u32, font: u32) -> Result<Option<(Vec<u8>, u32, u32)>>;
    fn draw_bitmap(
        &mut self,
        pixels: &[u8],
        x: i32,
        y: i32,
        width: usize,
        height: usize,
    ) -> Result<()>;
    fn read_sound_file(&mut self, _name: &[u8]) -> Result<Option<Vec<u8>>> {
        Ok(None)
    }
    fn play_sound(&mut self, _sound_type: SoundType, _data: &[u8], _looped: bool) -> Result<()> {
        Ok(())
    }
    fn stop_sound(&mut self) -> Result<()> {
        Ok(())
    }
    fn sound_is_active(&self) -> bool {
        false
    }
    fn set_sound_volume(&mut self, _volume: u8) -> Result<()> {
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ExtLifecycleRequest {
    Restart { package: Vec<u8>, entry: Vec<u8> },
    Exit,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum NativeExtensionProfile {
    Baseline,
    Mtk,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ExtLifecycleState {
    pub application: u32,
    pub callback: Vec<u8>,
    pub package: Vec<u8>,
    pub entry: Vec<u8>,
}

#[derive(Clone, Copy, Debug)]
struct GuestFunction {
    module: usize,
    address: u32,
    expected_image: Option<ExecutableImage>,
    captured_r9: Option<u32>,
}

#[derive(Debug)]
struct GuestExecution {
    function: GuestFunction,
    cpu: ArmCpu,
    entered_guest_call: Option<bool>,
    instruction_count: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ExecutableImage {
    Static,
    Dynamic(u64),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ExecutableRange {
    base: GuestAddr,
    len: usize,
}

impl ExecutableRange {
    fn end(self) -> Option<u32> {
        u32::try_from(self.len)
            .ok()
            .and_then(|len| self.base.0.checked_add(len))
    }

    fn contains(self, address: u32, len: usize) -> bool {
        let Some(range_end) = self.end() else {
            return false;
        };
        let Some(request_len) = u32::try_from(len).ok() else {
            return false;
        };
        let Some(request_end) = address.checked_add(request_len) else {
            return false;
        };
        address >= self.base.0 && request_end <= range_end
    }

    fn contains_range(self, other: Self) -> bool {
        let (Some(end), Some(other_end)) = (self.end(), other.end()) else {
            return false;
        };
        other.base.0 >= self.base.0 && other_end <= end
    }

    fn overlaps(self, other: Self) -> bool {
        let (Some(end), Some(other_end)) = (self.end(), other.end()) else {
            return true;
        };
        self.base.0 < other_end && other.base.0 < end
    }

    fn intersection(self, other: Self) -> Option<Self> {
        let (Some(end), Some(other_end)) = (self.end(), other.end()) else {
            return None;
        };
        let base = self.base.0.max(other.base.0);
        let end = end.min(other_end);
        (base < end).then(|| Self {
            base: GuestAddr(base),
            len: (end - base) as usize,
        })
    }

    fn subtract(self, removed: Self) -> Vec<Self> {
        let Some(overlap) = self.intersection(removed) else {
            return vec![self];
        };
        let end = self
            .end()
            .expect("tracked executable ranges have validated bounds");
        let overlap_end = overlap
            .end()
            .expect("executable intersections have validated bounds");
        let mut retained = Vec::with_capacity(2);
        if self.base.0 < overlap.base.0 {
            retained.push(Self {
                base: self.base,
                len: (overlap.base.0 - self.base.0) as usize,
            });
        }
        if overlap_end < end {
            retained.push(Self {
                base: GuestAddr(overlap_end),
                len: (end - overlap_end) as usize,
            });
        }
        retained
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct DynamicExecutableImage {
    id: u64,
    intervals: Vec<ExecutableRange>,
    module_parameter: Option<GuestAddr>,
    compact_repeating_timers: Vec<GuestAddr>,
}

// Keep existing single-range state assertions readable while dynamic images can
// retain multiple disjoint executable intervals after a partial overwrite.
impl PartialEq<ExecutableRange> for DynamicExecutableImage {
    fn eq(&self, other: &ExecutableRange) -> bool {
        self.intervals.as_slice() == [*other]
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct DynamicExecutableImageSlot(Option<DynamicExecutableImage>);

impl DynamicExecutableImageSlot {
    fn as_ref(&self) -> Option<&DynamicExecutableImage> {
        self.0.as_ref()
    }

    fn as_mut(&mut self) -> Option<&mut DynamicExecutableImage> {
        self.0.as_mut()
    }

    fn is_none(&self) -> bool {
        self.0.is_none()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct RepeatingTimerSnapshot {
    node: GuestAddr,
    period: u32,
    handler: u32,
    data: u32,
    repeat: u32,
    tail: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ModalRepeatingTimers {
    owner_generation: u64,
    image_id: u64,
    parameter: GuestAddr,
    rw_range: ExecutableRange,
    timers: Vec<RepeatingTimerSnapshot>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ModalTimerObservation {
    depth: u32,
    timers: ModalRepeatingTimers,
}

impl PartialEq<Option<ExecutableRange>> for DynamicExecutableImageSlot {
    fn eq(&self, other: &Option<ExecutableRange>) -> bool {
        match (self.as_ref(), other) {
            (Some(image), Some(range)) => image == range,
            (None, None) => true,
            _ => false,
        }
    }
}

#[derive(Debug)]
struct ModuleContext {
    generation: u64,
    base: GuestAddr,
    len: usize,
    loader_context: GuestAddr,
    helper: Option<GuestFunction>,
    helper_parameter: GuestAddr,
    static_base_r9: u32,
    dynamic_executable_ranges: Vec<DynamicExecutableImageSlot>,
    next_dynamic_executable_image_id: u64,
}

impl ModuleContext {
    fn image_range(&self) -> ExecutableRange {
        ExecutableRange {
            base: self.base,
            len: self.len,
        }
    }

    fn executable_image(&self, function: u32) -> Option<(ExecutableImage, u32)> {
        if function & 1 == 0 && function & 3 != 0 {
            return None;
        }
        let address = function & !1;
        let instruction_len = if function & 1 == 0 { 4 } else { 2 };
        if self.image_range().contains(address, instruction_len) {
            return Some((ExecutableImage::Static, address - self.base.0));
        }
        self.dynamic_executable_ranges
            .iter()
            .filter_map(DynamicExecutableImageSlot::as_ref)
            .find_map(|image| {
                image.intervals.iter().find_map(|range| {
                    range
                        .contains(address, instruction_len)
                        .then(|| (ExecutableImage::Dynamic(image.id), address - range.base.0))
                })
            })
    }
}

fn merge_executable_intervals(mut intervals: Vec<ExecutableRange>) -> Vec<ExecutableRange> {
    intervals.sort_unstable_by_key(|range| range.base.0);
    let mut merged: Vec<ExecutableRange> = Vec::with_capacity(intervals.len());
    for interval in intervals {
        if let Some(previous) = merged.last_mut() {
            let previous_end = previous
                .end()
                .expect("tracked executable ranges have validated bounds");
            let interval_end = interval
                .end()
                .expect("tracked executable ranges have validated bounds");
            if interval.base.0 <= previous_end {
                previous.len = previous_end.max(interval_end) as usize - previous.base.0 as usize;
                continue;
            }
        }
        merged.push(interval);
    }
    merged
}

#[derive(Clone, Copy, Debug)]
struct PlatformMemoryExtension {
    len: usize,
    previous_cursor: u32,
    owner_generation: u64,
}

#[derive(Clone, Copy, Debug)]
struct PendingExternalActionCompletion {
    owner_generation: u64,
    callback: GuestFunction,
    callback_data: u32,
}

#[derive(Clone, Copy, Debug)]
struct PendingSmsResult {
    owner_generation: u64,
    helper: GuestFunction,
    result: i32,
}

#[derive(Clone, Copy, Debug)]
struct ModuleLoadSnapshot {
    active_helper: Option<GuestFunction>,
    detached_guest_allocation_cursor: u32,
    platform_memory_cursor: u32,
    mtk_native_extension_owner: Option<u64>,
}

#[derive(Clone, Copy, Debug)]
struct GuestGlyph {
    address: GuestAddr,
    width: u32,
    height: u32,
}

#[derive(Debug)]
struct PlatformDialog {
    previous_screen: Vec<u8>,
    dialog_screen: Vec<u8>,
}

#[derive(Debug)]
struct PlatformTextViewer {
    previous_screen: Vec<u8>,
    style: u32,
    title: Vec<u16>,
    lines: Vec<Vec<u16>>,
    first_visible_line: usize,
    viewer_screen: Vec<u8>,
}

#[derive(Debug)]
struct PlatformEditor {
    owner_generation: u64,
    _title: Vec<u16>,
    _editor_type: u32,
    max_code_units: usize,
    text: Vec<u16>,
    buffer: GuestAddr,
    buffer_len: usize,
}

#[derive(Debug)]
struct PlatformMenu {
    title: Vec<u16>,
    items: Vec<Option<Vec<u16>>>,
    focused_item: usize,
    first_visible_item: usize,
    previous_screen: Option<Vec<u8>>,
    menu_screen: Option<Vec<u8>>,
    modal_detached: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ActivePlatformUi {
    Menu(u32),
    Dialog(u32),
    TextViewer(u32),
    Editor(u32),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PlatformPointerAction {
    None,
    MenuSelect(usize),
    MenuReturn,
    DialogAccept,
    DialogCancel,
    TextViewerAccept,
    TextViewerReturn,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PlatformPointerCapture {
    ui: ActivePlatformUi,
    action: PlatformPointerAction,
}

#[derive(Clone, Copy, Debug)]
struct BitmapDescriptor {
    pixels: GuestAddr,
    width: usize,
    height: usize,
    x: i32,
    y: i32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BitmapDrawMode {
    Or,
    Copy,
    Transparent(u16),
    Gray(u16),
}

#[derive(Clone, Copy, Debug)]
struct BitmapTransform {
    a: i16,
    b: i16,
    c: i16,
    d: i16,
    mode: i16,
}

#[derive(Clone, Copy, Debug)]
struct GuestHeapState {
    base: u32,
    span: u32,
    head: u32,
    head_variable: GuestAddr,
    free_left: u32,
    free_left_variable: GuestAddr,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct FreeBlock {
    offset: u32,
    len: u32,
}

#[derive(Clone, Debug)]
struct GuestHeapSnapshot {
    base: u32,
    span: u32,
    head: u32,
    free_left: u32,
    blocks: Vec<FreeBlock>,
    terminator: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct GuestAllocationView {
    len: u32,
    backing_base: u32,
    owner_generation: u64,
    // A same-address expansion may later be trimmed only from this exact old boundary.
    reclaimable_prefix_len: Option<u32>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct NestedGuestSuballocation {
    block_len: u32,
    restored_view: Option<(u32, u32)>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct NestedGuestHeap {
    owner_generation: u64,
    heap_base: u32,
    heap_span: u32,
}

#[derive(Debug)]
enum NativeSocketState {
    Created,
    Connecting(mpsc::Receiver<std::io::Result<TcpStream>>),
    Connected(TcpStream),
    Failed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum NativeSocketReceiveMode {
    BeforeSend,
    WaitForFirstResponse,
    Polling,
}

#[derive(Debug)]
struct NativeSocket {
    state: NativeSocketState,
    endpoint: Option<SocketAddrV4>,
    pending_http_request: Option<Vec<u8>>,
    receive_mode: NativeSocketReceiveMode,
}

#[derive(Debug)]
pub(crate) struct ExtRuntime {
    memory: GuestMemory,
    modules: Vec<ModuleContext>,
    next_module_generation: u64,
    active_helper: Option<GuestFunction>,
    heap_len: usize,
    screen_base: GuestAddr,
    screen_memory_len: usize,
    guest_allocations: BTreeMap<u32, u32>,
    guest_allocation_owners: BTreeMap<u32, u64>,
    guest_allocation_views: BTreeMap<u32, GuestAllocationView>,
    nested_guest_heaps: BTreeMap<u32, NestedGuestHeap>,
    guest_heap_snapshot: Option<GuestHeapSnapshot>,
    detached_guest_allocations: BTreeMap<u32, (usize, u32)>,
    detached_guest_allocation_owners: BTreeMap<u32, u64>,
    detached_guest_allocation_cursor: u32,
    dns_mappings: Arc<[DnsMapping]>,
    wap_proxy_endpoint: Option<SocketAddrV4>,
    pending_external_action_completions: VecDeque<PendingExternalActionCompletion>,
    device_date: DeviceDate,
    device_clock_origin: Instant,
    platform_memory_extensions: BTreeMap<u32, PlatformMemoryExtension>,
    platform_memory_cursor: u32,
    mtk_native_extension_owner: Option<u64>,
    random_state: u32,
    glyphs: BTreeMap<(u32, u32), GuestGlyph>,
    dialogs: BTreeMap<u32, PlatformDialog>,
    text_viewers: BTreeMap<u32, PlatformTextViewer>,
    editors: BTreeMap<u32, PlatformEditor>,
    menus: BTreeMap<u32, PlatformMenu>,
    native_windows: BTreeMap<u32, u64>,
    active_platform_ui: Vec<ActivePlatformUi>,
    pending_platform_menu_selection: Option<u32>,
    pending_platform_menu_returns: usize,
    pending_sms_results: VecDeque<PendingSmsResult>,
    next_ui_handle: u32,
    suppressed_ui_key_releases: BTreeSet<i32>,
    platform_pointer_capture: Option<PlatformPointerCapture>,
    native_sockets: BTreeMap<i32, NativeSocket>,
    next_native_socket_handle: i32,
    exit_requested: bool,
    native_extension_profile: NativeExtensionProfile,
    clock_origin: Instant,
    timer_deadline: Option<Instant>,
    compact_timer_scan_cursor: usize,
    modal_repeating_timers: Vec<ModalRepeatingTimers>,
    motion_active: bool,
}

impl ExtRuntime {
    pub(crate) fn validate_module_image(image: &[u8]) -> Result<()> {
        if !image.starts_with(b"MRPGCMAP") || image.len() <= 8 {
            return Err(Error::Abi(
                "EXT image is missing the complete MRPGCMAP marker".into(),
            ));
        }
        if image.len() > MODULE_STRIDE as usize {
            return Err(Error::ArmFault(format!(
                "EXT image is {} bytes (module stride is {})",
                image.len(),
                MODULE_STRIDE
            )));
        }
        Ok(())
    }

    pub fn new(
        screen_width: u16,
        screen_height: u16,
        package_name: &[u8],
        entry_name: &[u8],
        heap_len: u32,
    ) -> Result<Self> {
        let heap_len = usize::try_from(heap_len)
            .map_err(|_| Error::ArmFault("guest heap length does not fit the host".into()))?;
        if heap_len == 0 {
            return Err(Error::ArmFault("guest heap length must be non-zero".into()));
        }
        if heap_len > MAX_GUEST_HEAP_LEN {
            return Err(Error::ArmFault(format!(
                "guest heap length {heap_len} exceeds supported maximum {MAX_GUEST_HEAP_LEN}"
            )));
        }
        let heap_end = HEAP_BASE
            .0
            .checked_add(heap_len as u32)
            .ok_or_else(|| Error::ArmFault("guest heap end overflow".into()))?;
        // Legacy helpers use the full fixed RAM window even when the configured
        // allocator heap is smaller. Larger heaps use a separately guarded screen.
        let (guest_ram_len, screen_base) = if heap_len <= MIN_GUEST_RAM_LEN {
            (MIN_GUEST_RAM_LEN, SCREEN_BASE)
        } else {
            (heap_len, EXPANDED_SCREEN_BASE)
        };
        let mut memory = GuestMemory::new();
        memory.map(
            PLATFORM_TABLE,
            0x1000,
            Permissions::READ_WRITE,
            "platform table",
        )?;
        memory.map(
            PLATFORM_DATA,
            0x1000,
            Permissions::READ_WRITE,
            "platform data",
        )?;
        for (address, name) in [
            (BITMAP_ARRAY_DATA, "platform bitmap array"),
            (TILE_ARRAY_DATA, "platform tile array"),
            (MAP_ARRAY_DATA, "platform map array"),
            (SOUND_ARRAY_DATA, "platform sound array"),
            (SPRITE_ARRAY_DATA, "platform sprite array"),
            (SMS_CONFIG_DATA, "platform SMS configuration"),
        ] {
            memory.map(
                address,
                PLATFORM_RESOURCE_BACKING_LEN,
                Permissions::READ_WRITE,
                name,
            )?;
        }
        memory.map(
            START_FILE_PARAMETER_DATA,
            START_FILE_PARAMETER_LEN,
            Permissions::READ_WRITE,
            "platform start-file parameter",
        )?;
        memory.map(
            HEAP_BASE,
            guest_ram_len,
            Permissions::READ_WRITE,
            "guest RAM",
        )?;
        memory.map(STACK_BASE, STACK_LEN, Permissions::READ_WRITE, "EXT stack")?;
        // Legacy MTK key scanners read three active-low 16-bit registers from
        // this fixed MMIO window. Host key events below keep the bits current.
        memory.map_bytes(
            LEGACY_KEYPAD_REGISTERS,
            vec![u8::MAX; LEGACY_KEYPAD_REGISTERS_LEN],
            Permissions::READ_WRITE,
            "legacy keypad registers",
        )?;
        let screen_len = usize::from(screen_width)
            .checked_mul(usize::from(screen_height))
            .and_then(|pixels| pixels.checked_mul(2))
            .ok_or_else(|| Error::ArmFault("guest screen buffer size overflow".into()))?;
        let screen_memory_len = screen_len.max(SCREEN_STAGING_CAPACITY);
        memory.map(
            screen_base,
            screen_memory_len,
            Permissions::READ_WRITE,
            "screen memory",
        )?;

        for slot in 0..PLATFORM_SLOT_COUNT {
            let value = if is_function_slot(slot) {
                TRAP_BASE + slot * 4
            } else if is_data_slot(slot) {
                platform_data_slot_backing_address(slot).0
            } else {
                0
            };
            memory.write_u32(GuestAddr(PLATFORM_TABLE.0 + slot * 4), value)?;
        }
        write_platform_string(&mut memory, PACKAGE_NAME_DATA, package_name)?;
        write_platform_string(&mut memory, START_NAME_DATA, entry_name)?;
        write_platform_string(&mut memory, PREVIOUS_PACKAGE_NAME_DATA, b"")?;
        write_platform_string(&mut memory, PREVIOUS_START_NAME_DATA, b"")?;
        write_platform_string(&mut memory, CURRENT_ENTRY_DATA, entry_name)?;
        memory.write_u32(table_slot_address(23), INTERNAL_TABLE_DATA.0)?;
        memory.write_u32(INTERNAL_TABLE_DATA, FIRMWARE_SLOT_DATA.0)?;
        for index in 0..FIRMWARE_SLOT_COUNT {
            memory.write_u32(FIRMWARE_SLOT_DATA.checked_add(index * 4)?, 0)?;
        }
        memory.write(
            PLATFORM_RUNTIME_PROFILE_DATA,
            &[0; PLATFORM_RUNTIME_PROFILE_LEN],
        )?;
        for offset in INTERNAL_APPLICATION_STATE_OFFSETS {
            memory.write_u32(
                INTERNAL_TABLE_DATA.checked_add(offset)?,
                APPLICATION_STATE_DATA.0,
            )?;
        }
        memory.write_u32(
            INTERNAL_TABLE_DATA.checked_add(16)?,
            LIFECYCLE_CALLBACK_DATA.0,
        )?;
        memory.write_u32(INTERNAL_TABLE_DATA.checked_add(20)?, TIMER_ACTIVE_DATA.0)?;
        memory.write_u32(APPLICATION_STATE_DATA, APPLICATION_STATE_NORMAL)?;
        memory.write_u32(data_slot_address(91), screen_base.0)?;
        memory.write_u32(data_slot_address(92), u32::from(screen_width))?;
        memory.write_u32(data_slot_address(93), u32::from(screen_height))?;
        memory.write_u32(data_slot_address(94), 16)?;
        let bitmap_table = GuestAddr(memory.read_u32(table_slot_address(95))?);
        let screen_bitmap = bitmap_table.checked_add(SCREEN_BITMAP_ID * BITMAP_ENTRY_SIZE)?;
        memory.write_u16(screen_bitmap, screen_width)?;
        memory.write_u16(screen_bitmap.checked_add(2)?, screen_height)?;
        memory.write_u32(
            screen_bitmap.checked_add(4)?,
            u32::try_from(screen_len)
                .map_err(|_| Error::ArmFault("guest screen buffer size exceeds u32".into()))?,
        )?;
        memory.write_u32(screen_bitmap.checked_add(8)?, 0)?;
        memory.write_u32(screen_bitmap.checked_add(12)?, screen_base.0)?;
        memory.write_u32(data_slot_address(106), 1)?;
        memory.write_u32(data_slot_address(107), 1)?;
        memory.write_u32(data_slot_address(108), HEAP_BASE.0)?;
        memory.write_u32(data_slot_address(109), heap_len as u32)?;
        memory.write_u32(data_slot_address(110), heap_end)?;
        memory.write_u32(data_slot_address(111), heap_len as u32)?;
        memory.write_u32(data_slot_address(146), 0)?;
        memory.write_u32(HEAP_BASE, heap_len as u32)?;
        memory.write_u32(HEAP_BASE.checked_add(4)?, heap_len as u32)?;
        memory.write(PLATFORM_SIM_INFO_DATA, &[0; PLATFORM_SIM_INFO_LEN])?;
        for (index, value) in [
            PLATFORM_STORAGE_TOTAL_BLOCKS,
            PLATFORM_STORAGE_BLOCK_SIZE,
            PLATFORM_STORAGE_BLOCK_SIZE,
            PLATFORM_STORAGE_AVAILABLE_BLOCKS,
        ]
        .into_iter()
        .enumerate()
        {
            memory.write_u32(
                PLATFORM_STORAGE_INFO_DATA.checked_add((index * 4) as u32)?,
                value,
            )?;
        }
        memory.write(PLATFORM_STORAGE_DRIVE_DATA, PLATFORM_STORAGE_DRIVE)?;

        Ok(Self {
            memory,
            modules: Vec::new(),
            next_module_generation: 1,
            active_helper: None,
            heap_len,
            screen_base,
            screen_memory_len,
            guest_allocations: BTreeMap::new(),
            guest_allocation_owners: BTreeMap::new(),
            guest_allocation_views: BTreeMap::new(),
            nested_guest_heaps: BTreeMap::new(),
            guest_heap_snapshot: None,
            detached_guest_allocations: BTreeMap::new(),
            detached_guest_allocation_owners: BTreeMap::new(),
            detached_guest_allocation_cursor: DETACHED_GUEST_ALLOCATION_BASE.0,
            dns_mappings: Arc::from([]),
            wap_proxy_endpoint: None,
            pending_external_action_completions: VecDeque::new(),
            device_date: DeviceDate::default(),
            device_clock_origin: Instant::now(),
            platform_memory_extensions: BTreeMap::new(),
            platform_memory_cursor: PLATFORM_MEMORY_BASE.0,
            mtk_native_extension_owner: None,
            random_state: 1,
            glyphs: BTreeMap::new(),
            dialogs: BTreeMap::new(),
            text_viewers: BTreeMap::new(),
            editors: BTreeMap::new(),
            menus: BTreeMap::new(),
            native_windows: BTreeMap::new(),
            active_platform_ui: Vec::new(),
            pending_platform_menu_selection: None,
            pending_platform_menu_returns: 0,
            pending_sms_results: VecDeque::new(),
            next_ui_handle: 1,
            suppressed_ui_key_releases: BTreeSet::new(),
            platform_pointer_capture: None,
            native_sockets: BTreeMap::new(),
            next_native_socket_handle: 1,
            exit_requested: false,
            native_extension_profile: NativeExtensionProfile::Baseline,
            clock_origin: Instant::now(),
            timer_deadline: None,
            compact_timer_scan_cursor: 0,
            modal_repeating_timers: Vec::new(),
            motion_active: false,
        })
    }

    pub fn load_and_call_entry(
        &mut self,
        image: &[u8],
        code: i32,
        services: &mut dyn NativeServices,
    ) -> Result<i32> {
        Self::validate_module_image(image)?;
        let module_index = self.modules.len();
        let module_offset = u32::try_from(module_index)
            .ok()
            .and_then(|index| index.checked_mul(MODULE_STRIDE))
            .ok_or_else(|| Error::ArmFault("module address allocation overflow".into()))?;
        let base = GuestAddr(
            MODULE_BASE
                .checked_add(module_offset)
                .ok_or_else(|| Error::ArmFault("module base overflow".into()))?,
        );
        let generation = self.next_module_generation;
        self.next_module_generation = self
            .next_module_generation
            .checked_add(1)
            .ok_or_else(|| Error::Abi("EXT module generation overflow".into()))?;
        let snapshot = ModuleLoadSnapshot {
            active_helper: self.active_helper,
            detached_guest_allocation_cursor: self.detached_guest_allocation_cursor,
            platform_memory_cursor: self.platform_memory_cursor,
            mtk_native_extension_owner: self.mtk_native_extension_owner,
        };
        self.memory.map_bytes(
            base,
            image.to_vec(),
            Permissions::READ_WRITE_EXECUTE,
            format!("EXT module {module_index}"),
        )?;
        self.modules.push(ModuleContext {
            generation,
            base,
            len: image.len(),
            loader_context: GuestAddr(0),
            helper: None,
            helper_parameter: GuestAddr(0),
            static_base_r9: 0,
            dynamic_executable_ranges: Vec::new(),
            next_dynamic_executable_image_id: 0,
        });

        let result = (|| {
            let loader_context = self
                .allocate_guest_block_for_module(64, module_index)?
                .ok_or_else(|| {
                    Error::ArmFault("guest heap exhausted while allocating loader context".into())
                })?;
            self.modules[module_index].loader_context = loader_context;
            self.memory.write(loader_context, &[0; 64])?;
            self.memory.write_u32(base, PLATFORM_TABLE.0)?;
            self.memory
                .write_u32(base.checked_add(4)?, loader_context.0)?;
            self.call_guest(
                GuestFunction {
                    module: module_index,
                    address: base.0 + 8,
                    expected_image: Some(ExecutableImage::Static),
                    captured_r9: None,
                },
                [code as u32, 0, 0, 0],
                &[],
                services,
            )
            .and_then(|value| {
                let loader_context = self.modules[module_index].loader_context;
                let static_base = self.memory.read_u32(loader_context)?;
                let helper_parameter = self.modules[module_index].helper_parameter;
                self.modules[module_index].static_base_r9 = static_base;
                if helper_parameter.0 != 0 {
                    self.memory.write_u32(helper_parameter, static_base)?;
                }
                Ok(value)
            })
        })();
        if let Err(error) = result {
            return match self.rollback_module_initialization(module_index, generation, snapshot) {
                Ok(()) => Err(error),
                Err(rollback_error) => Err(Error::Abi(format!(
                    "EXT module initialization failed: {error}; rollback failed: {rollback_error}"
                ))),
            };
        }
        result.map(|value| value as i32)
    }

    pub fn load_guest_image_and_call_entry(
        &mut self,
        address: GuestAddr,
        len: usize,
        code: i32,
        services: &mut dyn NativeServices,
    ) -> Result<i32> {
        let image = self.memory.read(address, len)?;
        self.load_and_call_entry(&image, code, services)
    }

    pub fn call_active_helper(
        &mut self,
        code: i32,
        input: &[u8],
        services: &mut dyn NativeServices,
    ) -> Result<(i32, Vec<u8>)> {
        let helper = self
            .active_helper
            .ok_or_else(|| Error::Abi("no EXT helper is registered".into()))?;
        self.call_helper(helper, code, input, services)
    }

    fn call_helper(
        &mut self,
        helper: GuestFunction,
        code: i32,
        input: &[u8],
        services: &mut dyn NativeServices,
    ) -> Result<(i32, Vec<u8>)> {
        let input_address = if input.is_empty() {
            GuestAddr(0)
        } else {
            let address = self.allocate(input.len(), 4)?;
            self.memory.write(address, input)?;
            address
        };
        self.call_active_helper_arguments(
            helper,
            code,
            input_address.0,
            input.len() as u32,
            services,
        )
    }

    pub fn call_active_helper_raw(
        &mut self,
        code: i32,
        arguments: [u32; 2],
        services: &mut dyn NativeServices,
    ) -> Result<(i32, Vec<u8>)> {
        let helper = self
            .active_helper
            .ok_or_else(|| Error::Abi("no EXT helper is registered".into()))?;
        self.call_active_helper_arguments(helper, code, arguments[0], arguments[1], services)
    }

    pub fn call_active_motion_event(
        &mut self,
        x: i32,
        y: i32,
        z: i32,
        services: &mut dyn NativeServices,
    ) -> Result<(i32, Vec<u8>)> {
        let helper = self
            .active_helper
            .ok_or_else(|| Error::Abi("no EXT helper is registered".into()))?;
        let input_address = self.allocate(24, HEAP_ALIGNMENT)?;
        let sample_address = input_address.checked_add(12)?;
        let mut input = [0_u8; 24];
        input[0..4].copy_from_slice(&18_i32.to_le_bytes());
        input[8..12].copy_from_slice(&sample_address.0.to_le_bytes());
        input[12..16].copy_from_slice(&x.to_le_bytes());
        input[16..20].copy_from_slice(&y.to_le_bytes());
        input[20..24].copy_from_slice(&z.to_le_bytes());
        self.memory.write(input_address, &input)?;
        let result = self.call_active_helper_arguments(helper, 1, input_address.0, 12, services);
        let free_result = self.free_guest_block(input_address, input.len());
        match (result, free_result) {
            (Ok(output), Ok(())) => Ok(output),
            (Err(error), _) => Err(error),
            (Ok(_), Err(error)) => Err(error),
        }
    }

    pub fn motion_active(&self) -> bool {
        self.motion_active
    }

    fn call_active_helper_arguments(
        &mut self,
        helper: GuestFunction,
        code: i32,
        argument_2: u32,
        argument_3: u32,
        services: &mut dyn NativeServices,
    ) -> Result<(i32, Vec<u8>)> {
        let output_fields = self.allocate(8, 4)?;
        self.memory.write_u32(output_fields, 0)?;
        self.memory.write_u32(output_fields.checked_add(4)?, 0)?;
        let module_parameter = self.modules[helper.module].helper_parameter;
        let return_value = self.call_guest(
            helper,
            [module_parameter.0, code as u32, argument_2, argument_3],
            &[output_fields.0, output_fields.0 + 4],
            services,
        )? as i32;
        self.active_helper_output(return_value, output_fields)
    }

    fn range_is_owned_by(&self, address: GuestAddr, len: usize, owner_generation: u64) -> bool {
        self.allocation_owner_for_range(ExecutableRange { base: address, len })
            == Some(owner_generation)
    }

    fn registered_dynamic_image_parameter(
        &self,
        image: &[u8],
        image_address: GuestAddr,
        image_len: u32,
        owner_generation: u64,
    ) -> Option<GuestAddr> {
        // The relocated loader header carries its parameter pointer in word 1.
        // Validate the extChunk's bidirectional ABI links so arbitrary code bytes
        // cannot opt into timer-state handling.
        let end = DYNAMIC_IMAGE_PARAMETER_OFFSET.checked_add(4)?;
        let parameter = GuestAddr(u32::from_le_bytes(
            image
                .get(DYNAMIC_IMAGE_PARAMETER_OFFSET..end)?
                .try_into()
                .ok()?,
        ));
        self.compact_timer_suspend_depth(parameter, owner_generation)?;
        let ext_chunk = GuestAddr(
            self.memory
                .read_u32(
                    parameter
                        .checked_add(MODULE_PARAMETER_EXT_CHUNK_OFFSET)
                        .ok()?,
                )
                .ok()?,
        );
        if self
            .memory
            .read_u32(ext_chunk.checked_add(EXT_CHUNK_ENTRY_OFFSET).ok()?)
            .ok()?
            != image_address
                .checked_add(DYNAMIC_IMAGE_ENTRY_OFFSET)
                .ok()?
                .0
            || self
                .memory
                .read_u32(ext_chunk.checked_add(EXT_CHUNK_IMAGE_ADDRESS_OFFSET).ok()?)
                .ok()?
                != image_address.0
            || self
                .memory
                .read_u32(ext_chunk.checked_add(EXT_CHUNK_IMAGE_LEN_OFFSET).ok()?)
                .ok()?
                != image_len
            || self
                .memory
                .read_u32(ext_chunk.checked_add(EXT_CHUNK_PARAMETER_OFFSET).ok()?)
                .ok()?
                != parameter.0
            || self
                .memory
                .read_u32(ext_chunk.checked_add(EXT_CHUNK_PARAMETER_LEN_OFFSET).ok()?)
                .ok()?
                != MODULE_PARAMETER_LEN as u32
        {
            return None;
        }
        Some(parameter)
    }

    fn compact_timer_rw_range(
        &self,
        parameter: GuestAddr,
        owner_generation: u64,
    ) -> Option<(GuestAddr, usize)> {
        self.compact_timer_suspend_depth(parameter, owner_generation)?;
        let static_base = GuestAddr(self.memory.read_u32(parameter).ok()?);
        let static_len = usize::try_from(
            self.memory
                .read_u32(parameter.checked_add(MODULE_PARAMETER_RW_LEN_OFFSET).ok()?)
                .ok()?,
        )
        .ok()?;
        if !static_base.0.is_multiple_of(4)
            || !static_len.is_multiple_of(4)
            || static_len == 0
            || static_len > MAX_COMPACT_TIMER_POINTER_SCAN_LEN
            || self
                .memory
                .check_range(static_base, static_len, Permissions::READ_WRITE)
                .is_err()
            || !self.range_is_owned_by(static_base, static_len, owner_generation)
        {
            return None;
        }
        Some((static_base, static_len))
    }

    fn reachable_repeating_timers(
        &self,
        module_index: usize,
        image_id: u64,
        parameter: GuestAddr,
        owner_generation: u64,
    ) -> Option<Vec<RepeatingTimerSnapshot>> {
        // Inspect only the module-declared RW range. Multiple scheduler layouts
        // can reference the same node, so node identity is the stable unit.
        let (static_base, static_len) = self.compact_timer_rw_range(parameter, owner_generation)?;
        let mut checked_pointers = BTreeSet::new();
        let mut timers = BTreeMap::new();
        for offset in (0..static_len).step_by(4) {
            let pointer = GuestAddr(
                self.memory
                    .read_u32(static_base.checked_add(u32::try_from(offset).ok()?).ok()?)
                    .ok()?,
            );
            if pointer.0 == 0
                || !pointer.0.is_multiple_of(4)
                || self.memory.read_u32(pointer).ok() != Some(COMPACT_TIMER_MAGIC)
            {
                continue;
            }
            if !checked_pointers.insert(pointer.0) {
                continue;
            }
            if checked_pointers.len() > MAX_TRACKED_COMPACT_TIMERS {
                return None;
            }
            if let Some(timer) =
                self.repeating_timer_at(pointer, module_index, image_id, owner_generation)
            {
                if !timers.contains_key(&pointer.0) && timers.len() >= MAX_TRACKED_COMPACT_TIMERS {
                    return None;
                }
                timers.insert(pointer.0, timer);
            }
        }
        Some(timers.into_values().collect())
    }

    fn discover_compact_repeating_timers(&mut self) {
        let active = self
            .modal_repeating_timers
            .iter()
            .map(|timers| (timers.owner_generation, timers.image_id, timers.parameter))
            .collect::<BTreeSet<_>>();
        let mut pending = self
            .modules
            .iter()
            .enumerate()
            .flat_map(|(module_index, module)| {
                let active = &active;
                module
                    .dynamic_executable_ranges
                    .iter()
                    .enumerate()
                    .filter_map(move |(image_index, slot)| {
                        let image = slot.as_ref()?;
                        let parameter = image.module_parameter?;
                        (!active.contains(&(module.generation, image.id, parameter))).then_some((
                            module_index,
                            image_index,
                            module.generation,
                            image.id,
                            parameter,
                        ))
                    })
            })
            .collect::<Vec<_>>();

        if !pending.is_empty() {
            let start = self.compact_timer_scan_cursor % pending.len();
            pending.rotate_left(start);
            self.compact_timer_scan_cursor = self.compact_timer_scan_cursor.wrapping_add(1);
        }
        // This is a per-dispatch budget shared by every dynamic image.
        let mut scan_budget = MAX_COMPACT_TIMER_POINTER_SCAN_LEN;
        for (module_index, image_index, owner_generation, image_id, parameter) in pending {
            let Some((_, static_len)) = self.compact_timer_rw_range(parameter, owner_generation)
            else {
                continue;
            };
            if static_len > scan_budget {
                continue;
            }
            scan_budget -= static_len;
            let Some(discovered) = self.reachable_repeating_timers(
                module_index,
                image_id,
                parameter,
                owner_generation,
            ) else {
                continue;
            };
            if discovered.is_empty() {
                continue;
            }
            let Some(image) = self
                .modules
                .get_mut(module_index)
                .and_then(|module| module.dynamic_executable_ranges.get_mut(image_index))
                .and_then(DynamicExecutableImageSlot::as_mut)
            else {
                continue;
            };
            if image.id != image_id || image.module_parameter != Some(parameter) {
                continue;
            }
            for node in discovered.into_iter().map(|timer| timer.node) {
                if image.compact_repeating_timers.len() >= MAX_TRACKED_COMPACT_TIMERS {
                    break;
                }
                if !image.compact_repeating_timers.contains(&node) {
                    image.compact_repeating_timers.push(node);
                }
            }
        }
    }

    fn modal_timer_state_is_live(&self, timers: &ModalRepeatingTimers) -> bool {
        self.modules
            .iter()
            .find(|module| module.generation == timers.owner_generation)
            .and_then(|module| {
                module
                    .dynamic_executable_ranges
                    .iter()
                    .filter_map(DynamicExecutableImageSlot::as_ref)
                    .find(|image| {
                        image.id == timers.image_id
                            && image.module_parameter == Some(timers.parameter)
                    })
            })
            .is_some()
    }

    fn restore_compact_repeating_timers(&mut self, timers: &ModalRepeatingTimers) -> Result<()> {
        let Some(module_index) = self
            .modules
            .iter()
            .position(|module| module.generation == timers.owner_generation)
        else {
            return Ok(());
        };
        if self
            .compact_timer_rw_range(timers.parameter, timers.owner_generation)
            .map(|(base, len)| ExecutableRange { base, len })
            != Some(timers.rw_range)
        {
            return Ok(());
        }
        let reachable = self
            .reachable_repeating_timers(
                module_index,
                timers.image_id,
                timers.parameter,
                timers.owner_generation,
            )
            .unwrap_or_default()
            .into_iter()
            .map(|timer| (timer.node.0, timer))
            .collect::<BTreeMap<_, _>>();
        let mut retained_nodes = Vec::new();
        for saved in &timers.timers {
            let Some(current) = reachable.get(&saved.node.0) else {
                continue;
            };
            // Guest timers have no exposed allocation generation. Treat the
            // immutable callback record plus module/image ownership as identity;
            // any observable reuse makes this compatibility repair fail closed.
            if current.handler != saved.handler
                || current.data != saved.data
                || current.repeat != saved.repeat
                || current.tail != saved.tail
            {
                continue;
            }
            // The current deadline at +8 was recomputed while suspended. Preserve
            // it and restore only the repeating interval corrupted by modal return.
            self.memory.write_u32(
                saved.node.checked_add(COMPACT_TIMER_PERIOD_OFFSET)?,
                saved.period,
            )?;
            retained_nodes.push(saved.node);
        }

        if let Some(image) = self.modules[module_index]
            .dynamic_executable_ranges
            .iter_mut()
            .filter_map(DynamicExecutableImageSlot::as_mut)
            .find(|image| image.id == timers.image_id)
        {
            image.module_parameter = Some(timers.parameter);
            image.compact_repeating_timers = retained_nodes;
        }
        Ok(())
    }

    fn compact_timer_suspend_depth(
        &self,
        parameter: GuestAddr,
        owner_generation: u64,
    ) -> Option<u32> {
        if parameter.0 == 0
            || !parameter.0.is_multiple_of(4)
            || self
                .memory
                .check_range(parameter, MODULE_PARAMETER_LEN, Permissions::READ_WRITE)
                .is_err()
            || !self.range_is_owned_by(parameter, MODULE_PARAMETER_LEN, owner_generation)
        {
            return None;
        }
        let ext_chunk = GuestAddr(
            self.memory
                .read_u32(
                    parameter
                        .checked_add(MODULE_PARAMETER_EXT_CHUNK_OFFSET)
                        .ok()?,
                )
                .ok()?,
        );
        if ext_chunk.0 == 0
            || !ext_chunk.0.is_multiple_of(4)
            || self
                .memory
                .check_range(
                    ext_chunk,
                    EXT_CHUNK_TIMER_STATE_LEN,
                    Permissions::READ_WRITE,
                )
                .is_err()
            || !self.range_is_owned_by(ext_chunk, EXT_CHUNK_TIMER_STATE_LEN, owner_generation)
            || self.memory.read_u32(ext_chunk).ok()? != EXT_CHUNK_MAGIC
        {
            return None;
        }
        self.memory
            .read_u32(ext_chunk.checked_add(EXT_CHUNK_SUSPEND_DEPTH_OFFSET).ok()?)
            .ok()
    }

    fn repeating_timer_at(
        &self,
        node: GuestAddr,
        module_index: usize,
        image_id: u64,
        owner_generation: u64,
    ) -> Option<RepeatingTimerSnapshot> {
        if node.0 == 0
            || !node.0.is_multiple_of(4)
            || self.memory.read_u32(node).ok()? != COMPACT_TIMER_MAGIC
        {
            return None;
        }
        if self
            .memory
            .check_range(node, COMPACT_TIMER_NODE_LEN, Permissions::READ_WRITE)
            .is_err()
        {
            return None;
        }
        let period = self
            .memory
            .read_u32(node.checked_add(COMPACT_TIMER_PERIOD_OFFSET).ok()?)
            .ok()
            .filter(|period| *period != 0)?;
        let handler = self
            .memory
            .read_u32(node.checked_add(COMPACT_TIMER_HANDLER_OFFSET).ok()?)
            .ok()?;
        let module = self.modules.get(module_index)?;
        if module.generation != owner_generation
            || module.executable_image(handler).map(|(image, _)| image)
                != Some(ExecutableImage::Dynamic(image_id))
        {
            return None;
        }
        let timer = RepeatingTimerSnapshot {
            node,
            period,
            handler,
            data: self
                .memory
                .read_u32(node.checked_add(COMPACT_TIMER_DATA_OFFSET).ok()?)
                .ok()?,
            repeat: self
                .memory
                .read_u32(node.checked_add(COMPACT_TIMER_REPEAT_OFFSET).ok()?)
                .ok()?,
            tail: self
                .memory
                .read_u32(node.checked_add(COMPACT_TIMER_TAIL_OFFSET).ok()?)
                .ok()?,
        };
        if timer.repeat == 0
            || !self.range_is_owned_by(node, COMPACT_TIMER_NODE_LEN, owner_generation)
        {
            return None;
        }
        Some(timer)
    }

    fn current_repeating_timer_states(&self) -> Vec<ModalRepeatingTimers> {
        let mut states = Vec::new();
        for (module_index, module) in self.modules.iter().enumerate() {
            for image in module
                .dynamic_executable_ranges
                .iter()
                .filter_map(DynamicExecutableImageSlot::as_ref)
            {
                let Some(parameter) = image.module_parameter else {
                    continue;
                };
                let timers = image
                    .compact_repeating_timers
                    .iter()
                    .filter_map(|node| {
                        self.repeating_timer_at(*node, module_index, image.id, module.generation)
                    })
                    .collect::<Vec<_>>();
                let Some((rw_base, rw_len)) =
                    self.compact_timer_rw_range(parameter, module.generation)
                else {
                    continue;
                };
                if timers.is_empty() {
                    continue;
                }
                states.push(ModalRepeatingTimers {
                    owner_generation: module.generation,
                    image_id: image.id,
                    parameter,
                    rw_range: ExecutableRange {
                        base: rw_base,
                        len: rw_len,
                    },
                    timers,
                });
            }
        }
        states
    }

    fn modal_timer_key(timers: &ModalRepeatingTimers) -> (u64, u64, GuestAddr) {
        (timers.owner_generation, timers.image_id, timers.parameter)
    }

    fn modal_timer_state_fits_budget(&self, timers: &ModalRepeatingTimers) -> bool {
        self.modal_repeating_timers
            .iter()
            .try_fold(timers.rw_range.len, |total, active| {
                total.checked_add(active.rw_range.len)
            })
            .is_some_and(|total| total <= MAX_COMPACT_TIMER_POINTER_SCAN_LEN)
    }

    fn modal_timer_observations(&mut self) -> Result<Vec<ModalTimerObservation>> {
        let mut observations = Vec::new();
        for timers in std::mem::take(&mut self.modal_repeating_timers) {
            if !self.modal_timer_state_is_live(&timers) {
                continue;
            }
            match self.compact_timer_suspend_depth(timers.parameter, timers.owner_generation) {
                Some(0) => self.restore_compact_repeating_timers(&timers)?,
                Some(depth) => {
                    observations.push(ModalTimerObservation {
                        depth,
                        timers: timers.clone(),
                    });
                    self.modal_repeating_timers.push(timers);
                }
                None => self.modal_repeating_timers.push(timers),
            }
        }
        let active_keys = self
            .modal_repeating_timers
            .iter()
            .map(Self::modal_timer_key)
            .collect::<BTreeSet<_>>();
        for timers in self
            .current_repeating_timer_states()
            .into_iter()
            .filter(|timers| !active_keys.contains(&Self::modal_timer_key(timers)))
        {
            let Some(depth) =
                self.compact_timer_suspend_depth(timers.parameter, timers.owner_generation)
            else {
                continue;
            };
            observations.push(ModalTimerObservation {
                depth,
                timers: timers.clone(),
            });
            if depth > 0 && self.modal_timer_state_fits_budget(&timers) {
                self.modal_repeating_timers.push(timers);
            }
        }
        Ok(observations)
    }

    fn remove_modal_timer_state(
        &mut self,
        key: (u64, u64, GuestAddr),
    ) -> Option<ModalRepeatingTimers> {
        self.modal_repeating_timers
            .iter()
            .position(|timers| Self::modal_timer_key(timers) == key)
            .map(|index| self.modal_repeating_timers.remove(index))
    }

    fn finish_modal_timer_observations(
        &mut self,
        before: Vec<ModalTimerObservation>,
    ) -> Result<()> {
        for before in before {
            let key = Self::modal_timer_key(&before.timers);
            if !self.modal_timer_state_is_live(&before.timers) {
                self.remove_modal_timer_state(key);
                continue;
            }
            let Some(depth_after) = self.compact_timer_suspend_depth(
                before.timers.parameter,
                before.timers.owner_generation,
            ) else {
                if before.depth == 0 {
                    self.remove_modal_timer_state(key);
                }
                continue;
            };
            if before.depth == 0 && depth_after > 0 {
                if !self
                    .modal_repeating_timers
                    .iter()
                    .any(|timers| Self::modal_timer_key(timers) == key)
                    && self.modal_timer_state_fits_budget(&before.timers)
                {
                    self.modal_repeating_timers.push(before.timers);
                }
            } else if before.depth > 0
                && depth_after == 0
                && let Some(timers) = self.remove_modal_timer_state(key)
                && self.modal_timer_state_is_live(&timers)
            {
                self.restore_compact_repeating_timers(&timers)?;
            }
        }
        Ok(())
    }

    fn active_helper_output(
        &self,
        return_value: i32,
        output_fields: GuestAddr,
    ) -> Result<(i32, Vec<u8>)> {
        let output_address = self.memory.read_u32(output_fields)?;
        let output_len = self.memory.read_u32(output_fields.checked_add(4)?)? as usize;
        let output = if output_address == 0 || output_len == 0 {
            Vec::new()
        } else {
            if output_len > self.heap_len {
                return Err(Error::Abi(format!(
                    "EXT helper returned {output_len} output bytes"
                )));
            }
            self.memory.read(GuestAddr(output_address), output_len)?
        };
        Ok((return_value, output))
    }

    pub fn timer_due_in(&self) -> Option<Duration> {
        self.timer_deadline
            .map(|deadline| deadline.saturating_duration_since(Instant::now()))
    }

    pub fn take_due_timer(&mut self) -> Result<bool> {
        let Some(deadline) = self.timer_deadline else {
            return Ok(false);
        };
        if deadline > Instant::now() {
            return Ok(false);
        }
        self.timer_deadline = None;
        Ok(true)
    }

    pub fn lifecycle_request(&self) -> Result<Option<ExtLifecycleRequest>> {
        if self.exit_requested {
            return Ok(Some(ExtLifecycleRequest::Exit));
        }
        if self.memory.read_u32(APPLICATION_STATE_DATA)? != APPLICATION_STATE_RESTART_PENDING {
            return Ok(None);
        }
        let state = self.lifecycle_state()?;
        if state.callback != b"restart" {
            return Ok(None);
        }
        if state.package.is_empty() || state.entry.is_empty() {
            return Err(Error::Abi(format!(
                "restart request has empty package or entry (application state {})",
                state.application
            )));
        }
        Ok(Some(ExtLifecycleRequest::Restart {
            package: state.package,
            entry: state.entry,
        }))
    }

    pub fn clear_lifecycle_request(&mut self) -> Result<()> {
        self.memory.write_u32(LIFECYCLE_CALLBACK_DATA, 0)?;
        self.memory
            .write_u32(APPLICATION_STATE_DATA, APPLICATION_STATE_NORMAL)
    }

    pub fn set_previous_application(&mut self, package: &[u8], entry: &[u8]) -> Result<()> {
        write_platform_string(&mut self.memory, PREVIOUS_PACKAGE_NAME_DATA, package)?;
        write_platform_string(&mut self.memory, PREVIOUS_START_NAME_DATA, entry)
    }

    pub(crate) fn start_file_parameter(&self) -> Result<[u8; START_FILE_PARAMETER_LEN]> {
        self.memory
            .read(START_FILE_PARAMETER_DATA, START_FILE_PARAMETER_LEN)?
            .try_into()
            .map_err(|_| Error::Abi("invalid start-file parameter length".into()))
    }

    pub(crate) fn set_start_file_parameter(
        &mut self,
        parameter: &[u8; START_FILE_PARAMETER_LEN],
    ) -> Result<()> {
        self.memory.write(START_FILE_PARAMETER_DATA, parameter)
    }

    pub fn set_native_extension_profile(&mut self, profile: NativeExtensionProfile) -> Result<()> {
        if profile == self.native_extension_profile {
            return Ok(());
        }
        if self.native_extension_profile != NativeExtensionProfile::Baseline {
            return Err(Error::Abi(
                "native-extension profile cannot change after configuration".into(),
            ));
        }
        if profile == NativeExtensionProfile::Mtk {
            self.memory.map(
                MTK_NATIVE_EXTENSION_BASE,
                MTK_NATIVE_EXTENSION_LEN,
                Permissions::READ_WRITE,
                "MTK native extension window",
            )?;
            let window_end = MTK_NATIVE_EXTENSION_BASE
                .0
                .checked_add(MTK_NATIVE_EXTENSION_LEN as u32)
                .ok_or_else(|| Error::ArmFault("MTK extension window end overflow".into()))?;
            self.platform_memory_cursor = window_end
                .checked_add(0xfff)
                .map(|address| address & !0xfff)
                .ok_or_else(|| Error::ArmFault("platform memory cursor overflow".into()))?;
        }
        self.native_extension_profile = profile;
        Ok(())
    }

    pub fn set_dns_mappings(&mut self, mappings: Arc<[DnsMapping]>) {
        self.dns_mappings = mappings;
    }

    pub(crate) fn set_wap_proxy_endpoint(&mut self, endpoint: Option<SocketAddrV4>) {
        self.wap_proxy_endpoint = endpoint;
    }

    pub fn set_device_date(&mut self, device_date: DeviceDate) {
        self.device_date = device_date;
        self.device_clock_origin = Instant::now();
    }

    fn current_device_datetime(&self) -> DeviceDate {
        self.device_date.advance(self.device_clock_origin.elapsed())
    }

    pub fn route_key_event(
        &mut self,
        code: i32,
        pressed: bool,
        services: &mut dyn NativeServices,
    ) -> Result<Option<(i32, i32, i32)>> {
        self.set_legacy_keypad_state(code, pressed)?;
        if !pressed && self.suppressed_ui_key_releases.remove(&code) {
            return Ok(None);
        }
        let Some(active_ui) = self.active_platform_ui.last().copied() else {
            return Ok(Some((if pressed { 0 } else { 1 }, code, 0)));
        };
        if !pressed {
            return Ok(None);
        }
        self.suppressed_ui_key_releases.insert(code);
        match active_ui {
            ActivePlatformUi::Menu(handle) => match code {
                12 => {
                    self.move_platform_menu_focus(handle, -1, services)?;
                    Ok(None)
                }
                13 => {
                    self.move_platform_menu_focus(handle, 1, services)?;
                    Ok(None)
                }
                17 | 20 => {
                    let selected = self.selected_platform_menu_item(handle);
                    if selected.is_some() {
                        self.pending_platform_menu_selection = Some(handle);
                    }
                    Ok(selected.map(|index| (4, index as i32, 0)))
                }
                16 | 18 => Ok(Some((5, 0, 0))),
                _ => Ok(None),
            },
            ActivePlatformUi::Dialog(_) => match code {
                // Left soft key and select accept; right soft key and power cancel.
                17 | 20 => Ok(Some((6, 1, 0))),
                16 | 18 => Ok(Some((6, 0, 0))),
                _ => Ok(None),
            },
            ActivePlatformUi::TextViewer(handle) => match code {
                12 => {
                    self.move_platform_text_viewer(handle, -1, services)?;
                    Ok(None)
                }
                13 => {
                    self.move_platform_text_viewer(handle, 1, services)?;
                    Ok(None)
                }
                // Style 1 exposes both callbacks; style 2 is a read-only viewer.
                17 | 20
                    if self
                        .text_viewers
                        .get(&handle)
                        .is_some_and(|viewer| viewer.style == 1) =>
                {
                    Ok(Some((6, 0, 0)))
                }
                16 | 18 => Ok(Some((6, 1, 0))),
                _ => Ok(None),
            },
            ActivePlatformUi::Editor(_) => match code {
                17 | 20 => Ok(Some((6, 0, 0))),
                16 | 18 => Ok(Some((6, 1, 0))),
                _ => Ok(None),
            },
        }
    }

    fn set_legacy_keypad_state(&mut self, code: i32, pressed: bool) -> Result<()> {
        let Ok(code) = u32::try_from(code) else {
            return Ok(());
        };
        if code >= 42 {
            return Ok(());
        }
        let register = LEGACY_KEYPAD_REGISTERS.checked_add(4 + code / 16 * 4)?;
        let mask = 1_u16 << (code % 16);
        let value = self.memory.read_u16(register)?;
        self.memory
            .write_u16(register, if pressed { value & !mask } else { value | mask })
    }

    pub fn route_text_input(&mut self, text: &str) -> Result<Option<(i32, i32, i32)>> {
        let Some(ActivePlatformUi::Editor(handle)) = self.active_platform_ui.last().copied() else {
            return Ok(None);
        };
        self.set_platform_editor_text(handle, text)?;
        Ok(Some((6, 0, 0)))
    }

    pub(crate) fn active_editor_text(&self) -> Option<String> {
        let ActivePlatformUi::Editor(handle) = self.active_platform_ui.last().copied()? else {
            return None;
        };
        self.editors
            .get(&handle)
            .map(|editor| String::from_utf16_lossy(&editor.text))
    }

    pub fn route_pointer_event(
        &mut self,
        x: i32,
        y: i32,
        pressed: bool,
        services: &mut dyn NativeServices,
    ) -> Result<Option<(i32, i32, i32)>> {
        if pressed {
            let Some(ui) = self.active_platform_ui.last().copied() else {
                return Ok(Some((2, x, y)));
            };
            let action = match ui {
                ActivePlatformUi::Menu(handle) => {
                    let action = self.platform_menu_pointer_action(handle, x, y)?;
                    if let PlatformPointerAction::MenuSelect(index) = action {
                        self.set_platform_menu_focus(handle, index, services)?;
                    } else {
                        self.render_platform_menu(handle, services)?;
                    }
                    action
                }
                ActivePlatformUi::Dialog(_) => self.platform_dialog_pointer_action(x, y)?,
                ActivePlatformUi::TextViewer(handle) => {
                    self.platform_text_viewer_pointer_action(handle, x, y)?
                }
                ActivePlatformUi::Editor(_) => PlatformPointerAction::None,
            };
            self.platform_pointer_capture = Some(PlatformPointerCapture { ui, action });
            return Ok(None);
        }

        let Some(capture) = self.platform_pointer_capture.take() else {
            if self.active_platform_ui.is_empty() {
                return Ok(Some((3, x, y)));
            }
            return Ok(None);
        };
        if self.active_platform_ui.last().copied() != Some(capture.ui) {
            return Ok(None);
        }
        let released_action = match capture.ui {
            ActivePlatformUi::Menu(handle) => self.platform_menu_pointer_action(handle, x, y)?,
            ActivePlatformUi::Dialog(_) => self.platform_dialog_pointer_action(x, y)?,
            ActivePlatformUi::TextViewer(handle) => {
                self.platform_text_viewer_pointer_action(handle, x, y)?
            }
            ActivePlatformUi::Editor(_) => PlatformPointerAction::None,
        };
        if released_action != capture.action {
            return Ok(None);
        }
        if matches!(capture.action, PlatformPointerAction::MenuSelect(_))
            && let ActivePlatformUi::Menu(handle) = capture.ui
        {
            self.pending_platform_menu_selection = Some(handle);
        }
        Ok(match capture.action {
            PlatformPointerAction::MenuSelect(index) => Some((4, index as i32, 0)),
            PlatformPointerAction::MenuReturn => Some((5, 0, 0)),
            PlatformPointerAction::DialogAccept => Some((6, 1, 0)),
            PlatformPointerAction::DialogCancel => Some((6, 0, 0)),
            PlatformPointerAction::TextViewerAccept => Some((6, 0, 0)),
            PlatformPointerAction::TextViewerReturn => Some((6, 1, 0)),
            PlatformPointerAction::None => None,
        })
    }

    pub fn route_pointer_move(&self, x: i32, y: i32) -> Option<(i32, i32, i32)> {
        self.active_platform_ui.is_empty().then_some((12, x, y))
    }

    pub fn finish_platform_event(&mut self, services: &mut dyn NativeServices) -> Result<()> {
        let Some(handle) = self.pending_platform_menu_selection.take() else {
            return Ok(());
        };
        let ui = ActivePlatformUi::Menu(handle);
        if self.active_platform_ui.last().copied() != Some(ui) {
            return Ok(());
        }
        self.active_platform_ui.pop();
        if self
            .platform_pointer_capture
            .is_some_and(|capture| capture.ui == ui)
        {
            self.platform_pointer_capture = None;
        }
        let screens = self.menus.get(&handle).and_then(|menu| {
            Some((
                menu.menu_screen.as_ref()?.clone(),
                menu.previous_screen.as_ref()?.clone(),
            ))
        });
        let restore_screen = if let Some((menu_screen, previous_screen)) = screens {
            (self.memory.read(self.screen_base, menu_screen.len())? == menu_screen)
                .then_some(previous_screen)
        } else {
            None
        };
        if let Some(menu) = self.menus.get_mut(&handle) {
            menu.modal_detached = menu.previous_screen.is_some();
        }
        if let Some(previous_screen) = restore_screen {
            self.memory.write(self.screen_base, &previous_screen)?;
            self.present_screen(services)?;
        }
        Ok(())
    }

    fn lifecycle_state(&self) -> Result<ExtLifecycleState> {
        let read_slot_string = |slot| -> Result<Vec<u8>> {
            let address = self.memory.read_u32(table_slot_address(slot))?;
            if address == 0 {
                Ok(Vec::new())
            } else {
                self.read_c_string(GuestAddr(address), 1024)
            }
        };
        let callback = self.memory.read_u32(LIFECYCLE_CALLBACK_DATA)?;
        Ok(ExtLifecycleState {
            application: self.memory.read_u32(APPLICATION_STATE_DATA)?,
            callback: if callback == 0 {
                Vec::new()
            } else {
                self.read_c_string(GuestAddr(callback), 256)?
            },
            package: read_slot_string(100)?,
            entry: read_slot_string(101)?,
        })
    }

    pub fn dispatch_pending_platform_event(
        &mut self,
        services: &mut dyn NativeServices,
    ) -> Result<bool> {
        if let Some(completion) = self.pending_sms_results.pop_front() {
            if self
                .modules
                .get(completion.helper.module)
                .is_none_or(|module| module.generation != completion.owner_generation)
                || !self.guest_function_is_executable(completion.helper)
            {
                return Ok(true);
            }
            let mut event = [0_u8; 12];
            event[..4].copy_from_slice(&9_i32.to_le_bytes());
            event[4..8].copy_from_slice(&completion.result.to_le_bytes());
            self.call_helper(completion.helper, 1, &event, services)?;
            return Ok(true);
        }
        if self.pending_platform_menu_returns == 0 {
            return Ok(false);
        }
        let mut event = [0_u8; 12];
        event[..4].copy_from_slice(&5_i32.to_le_bytes());
        self.call_active_helper(1, &event, services)?;
        self.pending_platform_menu_returns -= 1;
        Ok(true)
    }

    pub fn dispatch_pending_external_action(
        &mut self,
        services: &mut dyn NativeServices,
    ) -> Result<bool> {
        let Some(completion) = self.pending_external_action_completions.pop_front() else {
            return Ok(false);
        };
        if self
            .modules
            .get(completion.callback.module)
            .is_none_or(|module| module.generation != completion.owner_generation)
        {
            return Ok(true);
        }
        self.call_guest(
            completion.callback,
            [0, completion.callback_data, 0, 0],
            &[],
            services,
        )?;
        Ok(true)
    }

    fn allocation_owner_for_range(&self, requested: ExecutableRange) -> Option<u64> {
        let guest_owner = self.guest_allocations.iter().find_map(|(base, len)| {
            let allocation = ExecutableRange {
                base: GuestAddr(*base),
                len: *len as usize,
            };
            allocation
                .contains_range(requested)
                .then(|| self.guest_allocation_owners.get(base).copied())
                .flatten()
        });
        guest_owner
            .or_else(|| {
                self.detached_guest_allocations
                    .iter()
                    .find_map(|(base, (len, _))| {
                        ExecutableRange {
                            base: GuestAddr(*base),
                            len: *len,
                        }
                        .contains_range(requested)
                        .then(|| self.detached_guest_allocation_owners.get(base).copied())
                        .flatten()
                    })
            })
            .or_else(|| {
                self.platform_memory_extensions
                    .iter()
                    .find_map(|(base, extension)| {
                        ExecutableRange {
                            base: GuestAddr(*base),
                            len: extension.len,
                        }
                        .contains_range(requested)
                        .then_some(extension.owner_generation)
                    })
            })
    }

    fn guest_function_is_executable(&self, function: GuestFunction) -> bool {
        let actual_image = self
            .modules
            .get(function.module)
            .and_then(|module| module.executable_image(function.address))
            .map(|(image, _)| image);
        actual_image.is_some()
            && function
                .expected_image
                .is_none_or(|expected| actual_image == Some(expected))
    }

    fn revoke_executable_ranges_in(&mut self, released: ExecutableRange) -> Result<()> {
        let intersections = self
            .modules
            .iter()
            .flat_map(|module| {
                module
                    .dynamic_executable_ranges
                    .iter()
                    .filter_map(DynamicExecutableImageSlot::as_ref)
            })
            .flat_map(|image| image.intervals.iter())
            .filter_map(|range| range.intersection(released))
            .collect::<Vec<_>>();
        if intersections.is_empty() {
            return Ok(());
        }

        // Remove execute permission before changing image metadata. A permission
        // failure can then only leave the runtime fail-closed.
        for intersection in &intersections {
            self.memory.remove_permissions(
                intersection.base,
                intersection.len,
                Permissions::EXECUTE,
            )?;
        }

        for module in &mut self.modules {
            for image in &mut module.dynamic_executable_ranges {
                let Some(dynamic_image) = image.as_mut() else {
                    continue;
                };
                dynamic_image.intervals = dynamic_image
                    .intervals
                    .iter()
                    .flat_map(|range| range.subtract(released))
                    .collect();
                if dynamic_image.intervals.is_empty() {
                    image.0 = None;
                }
            }
        }

        let invalid_helpers = self
            .modules
            .iter()
            .enumerate()
            .filter_map(|(module_index, module)| {
                module
                    .helper
                    .is_some_and(|helper| !self.guest_function_is_executable(helper))
                    .then_some(module_index)
            })
            .collect::<Vec<_>>();
        for module_index in invalid_helpers {
            self.modules[module_index].helper = None;
        }
        if self
            .active_helper
            .is_some_and(|helper| !self.guest_function_is_executable(helper))
        {
            self.active_helper = None;
        }
        let mut pending = std::mem::take(&mut self.pending_external_action_completions);
        pending.retain(|completion| self.guest_function_is_executable(completion.callback));
        self.pending_external_action_completions = pending;
        let mut pending_sms_results = std::mem::take(&mut self.pending_sms_results);
        pending_sms_results
            .retain(|completion| self.guest_function_is_executable(completion.helper));
        self.pending_sms_results = pending_sms_results;
        Ok(())
    }

    fn rollback_module_initialization(
        &mut self,
        module_index: usize,
        generation: u64,
        snapshot: ModuleLoadSnapshot,
    ) -> Result<()> {
        let (module_base, module_len) = self
            .modules
            .get(module_index)
            .filter(|module| module.generation == generation)
            .map(|module| (module.base, module.len))
            .ok_or_else(|| {
                Error::Abi(format!(
                    "cannot roll back missing EXT module generation {generation}"
                ))
            })?;
        if module_index + 1 != self.modules.len() {
            return Err(Error::Abi(format!(
                "cannot roll back EXT module {module_index} while later modules are active"
            )));
        }

        self.modules[module_index].helper = None;
        self.pending_external_action_completions
            .retain(|completion| completion.owner_generation != generation);
        self.pending_sms_results
            .retain(|completion| completion.owner_generation != generation);
        self.native_windows
            .retain(|_, owner_generation| *owner_generation != generation);
        let discarded_editors = self
            .editors
            .iter()
            .filter_map(|(handle, editor)| {
                (editor.owner_generation == generation).then_some(*handle)
            })
            .collect::<BTreeSet<_>>();
        self.editors
            .retain(|_, editor| editor.owner_generation != generation);
        self.active_platform_ui.retain(|ui| {
            !matches!(ui, ActivePlatformUi::Editor(handle) if discarded_editors.contains(handle))
        });
        if self.platform_pointer_capture.is_some_and(|capture| {
            matches!(capture.ui, ActivePlatformUi::Editor(handle) if discarded_editors.contains(&handle))
        }) {
            self.platform_pointer_capture = None;
        }

        let dynamic_ranges = self.modules[module_index]
            .dynamic_executable_ranges
            .iter()
            .filter_map(DynamicExecutableImageSlot::as_ref)
            .flat_map(|image| image.intervals.iter())
            .copied()
            .collect::<Vec<_>>();
        let mut rollback_error = None;
        let mut revocation_failed = false;
        for range in dynamic_ranges {
            if let Err(error) = self.revoke_executable_ranges_in(range) {
                revocation_failed = true;
                if rollback_error.is_none() {
                    rollback_error = Some(error);
                }
            }
        }

        let mut owned_allocations = self
            .guest_allocation_owners
            .iter()
            .filter_map(|(base, owner)| {
                (*owner == generation).then(|| {
                    self.guest_allocations
                        .get(base)
                        .map(|len| (GuestAddr(*base), *len as usize))
                })?
            })
            .chain(
                self.detached_guest_allocation_owners
                    .iter()
                    .filter_map(|(base, owner)| {
                        (*owner == generation).then(|| {
                            self.detached_guest_allocations
                                .get(base)
                                .map(|(len, _)| (GuestAddr(*base), *len))
                        })?
                    }),
            )
            .collect::<Vec<_>>();
        owned_allocations.sort_unstable_by_key(|(address, _)| std::cmp::Reverse(address.0));

        for (address, len) in &owned_allocations {
            if let Err(error) = self.free_guest_block(*address, *len)
                && rollback_error.is_none()
            {
                rollback_error = Some(error);
            }
        }

        let mut owned_platform_arenas = self
            .platform_memory_extensions
            .iter()
            .filter_map(|(base, extension)| {
                (extension.owner_generation == generation).then_some((*base, *extension))
            })
            .collect::<Vec<_>>();
        owned_platform_arenas.sort_unstable_by_key(|(base, _)| std::cmp::Reverse(*base));
        for (base, extension) in owned_platform_arenas {
            match self.memory.unmap(GuestAddr(base), extension.len) {
                Ok(()) => {
                    self.platform_memory_extensions.remove(&base);
                    self.guest_allocation_views
                        .retain(|_, view| view.backing_base != base);
                }
                Err(error) if rollback_error.is_none() => rollback_error = Some(error),
                Err(_) => {}
            }
        }

        if !self
            .detached_guest_allocation_owners
            .values()
            .any(|owner| *owner == generation)
        {
            self.detached_guest_allocation_cursor = snapshot.detached_guest_allocation_cursor;
        }
        if !self
            .platform_memory_extensions
            .values()
            .any(|extension| extension.owner_generation == generation)
        {
            self.platform_memory_cursor = snapshot.platform_memory_cursor;
        }
        if !revocation_failed && self.mtk_native_extension_owner == Some(generation) {
            self.mtk_native_extension_owner = snapshot.mtk_native_extension_owner;
        }
        self.active_helper = snapshot.active_helper;

        match self.memory.unmap(module_base, module_len) {
            Ok(()) => {
                self.modules.pop();
            }
            Err(error) if rollback_error.is_none() => rollback_error = Some(error),
            Err(_) => {}
        }

        rollback_error.map_or(Ok(()), Err)
    }

    fn try_dispatch_legacy_external_action(
        &mut self,
        module: usize,
        return_to_thumb: bool,
        cpu: &mut ArmCpu,
    ) -> Result<bool> {
        let Some(context) = self.modules.get(module) else {
            return Ok(false);
        };
        let Some((entry_image @ ExecutableImage::Dynamic(_), _)) =
            context.executable_image(cpu.pc().0 | u32::from(cpu.is_thumb()))
        else {
            return Ok(false);
        };
        const REQUEST_WORDS: usize = 11;
        const REQUEST_LEN: usize = REQUEST_WORDS * 4;
        let request = GuestAddr(cpu.register(0));
        if request.0 == 0
            || request.0 & 3 != 0
            || self
                .memory
                .check_range(request, REQUEST_LEN, Permissions::READ)
                .is_err()
        {
            return Ok(false);
        }
        let mut words = [0_u32; REQUEST_WORDS];
        for (index, word) in words.iter_mut().enumerate() {
            *word = self
                .memory
                .read_u32(request.checked_add((index * 4) as u32)?)?;
        }
        // This is an ABI allowlist, not a function-signature guess: only calls into
        // module-owned dynamic code with the complete bounded record are handled.
        if words[0] != 0
            || words[1] != 0
            || words[2] != 20
            || !LEGACY_EXTERNAL_ACTION_KINDS.contains(&words[3])
            || words[4] == 0
            || words[5] == 0
            || words[6..=8] != [0, 0, 0]
            || words[10] == 0
        {
            return Ok(false);
        }
        let Ok(identifier) = self.read_c_string(GuestAddr(words[4]), 65) else {
            return Ok(false);
        };
        if identifier.is_empty() {
            return Ok(false);
        }
        if self.read_c_string(GuestAddr(words[5]), 257).is_err() {
            return Ok(false);
        }

        let callback = words[10];
        let return_address = cpu.register(14);
        let encoded_return_address = return_address | u32::from(return_to_thumb);
        let Some((callback_image, _)) = context.executable_image(callback) else {
            return Ok(false);
        };
        let Some((_return_image, _)) = context.executable_image(encoded_return_address) else {
            return Ok(false);
        };
        if callback_image != entry_image {
            return Ok(false);
        }

        if self.pending_external_action_completions.len() >= MAX_PENDING_EXTERNAL_ACTIONS {
            cpu.set_register(0, 0);
            cpu.set_pc(encoded_return_address);
            return Ok(true);
        }

        let owner_generation = context.generation;
        self.pending_external_action_completions
            .push_back(PendingExternalActionCompletion {
                owner_generation,
                callback: GuestFunction {
                    module,
                    address: callback,
                    expected_image: Some(callback_image),
                    captured_r9: Some(cpu.register(9)),
                },
                callback_data: words[9],
            });
        cpu.set_register(0, 1);
        cpu.set_pc(encoded_return_address);
        Ok(true)
    }

    fn call_guest(
        &mut self,
        function: GuestFunction,
        registers: [u32; 4],
        stack_arguments: &[u32],
        services: &mut dyn NativeServices,
    ) -> Result<u32> {
        let modal_timer_before = self.modal_timer_observations()?;
        let execution = self.prepare_guest_execution(function, registers, stack_arguments)?;
        let return_value = self.run_guest_execution(execution, services)?;
        self.finish_modal_timer_observations(modal_timer_before)?;
        Ok(return_value)
    }

    fn prepare_guest_execution(
        &mut self,
        function: GuestFunction,
        registers: [u32; 4],
        stack_arguments: &[u32],
    ) -> Result<GuestExecution> {
        let module = self.modules.get(function.module).ok_or_else(|| {
            Error::Abi(format!(
                "guest function references module {}",
                function.module
            ))
        })?;
        let actual_image = module
            .executable_image(function.address)
            .map(|(image, _)| image);
        if actual_image.is_none()
            || function
                .expected_image
                .is_some_and(|expected| actual_image != Some(expected))
        {
            return Err(Error::Abi(format!(
                "guest function {:#010x} is outside module {}",
                function.address, function.module
            )));
        }
        let static_base_r9 = module.static_base_r9;
        let mut cpu = ArmCpu::new();
        cpu.allow_legacy_null_data_accesses();
        for (index, value) in registers.into_iter().enumerate() {
            cpu.set_register(index, value);
        }
        cpu.set_register(9, function.captured_r9.unwrap_or(static_base_r9));
        let stack_top = STACK_BASE.0 + STACK_LEN as u32;
        let stack_bytes = u32::try_from(stack_arguments.len())
            .ok()
            .and_then(|count| count.checked_mul(4))
            .ok_or_else(|| Error::ArmFault("stack argument size overflow".into()))?;
        let stack_pointer = stack_top
            .checked_sub(stack_bytes)
            .ok_or_else(|| Error::ArmFault("EXT stack underflow".into()))?
            & !7;
        for (index, argument) in stack_arguments.iter().copied().enumerate() {
            self.memory.write_u32(
                GuestAddr(stack_pointer + u32::try_from(index).unwrap() * 4),
                argument,
            )?;
        }
        cpu.set_register(13, stack_pointer);
        cpu.set_register(14, RETURN_SENTINEL);
        cpu.set_pc(function.address);

        Ok(GuestExecution {
            function,
            cpu,
            entered_guest_call: None,
            instruction_count: 0,
        })
    }

    fn prepare_guest_gzip_screen_buffer_capacity(&mut self, cpu: &ArmCpu) -> Result<()> {
        let output_pointer = GuestAddr(cpu.register(2));
        let output_len_pointer = GuestAddr(cpu.register(3));
        let stack_end = STACK_BASE.0 + STACK_LEN as u32;
        let stack_pointer = cpu.register(13);
        if !(STACK_BASE.0..=stack_end).contains(&stack_pointer) {
            return Ok(());
        }
        let is_stack_word = |address: GuestAddr| {
            address.0 & 3 == 0
                && address.0 >= stack_pointer
                && address.0.checked_add(4).is_some_and(|end| end <= stack_end)
        };
        if output_pointer == output_len_pointer
            || !is_stack_word(output_pointer)
            || !is_stack_word(output_len_pointer)
            || self
                .memory
                .check_range(output_pointer, 4, Permissions::READ_WRITE)
                .is_err()
            || self
                .memory
                .check_range(output_len_pointer, 4, Permissions::READ_WRITE)
                .is_err()
        {
            return Ok(());
        }

        let Some(output) = self.memory.read_u32(output_pointer).ok().map(GuestAddr) else {
            return Ok(());
        };
        let Some(screen) = self
            .memory
            .read_u32(data_slot_address(91))
            .ok()
            .map(GuestAddr)
        else {
            return Ok(());
        };
        if output != self.screen_base || output != screen {
            return Ok(());
        }
        let Some(output_len) = self.memory.read_u32(output_len_pointer).ok() else {
            return Ok(());
        };
        let Ok(screen_memory_len) = u32::try_from(self.screen_memory_len) else {
            return Ok(());
        };

        let source = GuestAddr(cpu.register(0));
        let source_len = cpu.register(1);
        if source_len < 18
            || self
                .memory
                .check_range(source, source_len as usize, Permissions::READ)
                .is_err()
            || self.memory.read(source, 3).ok().as_deref() != Some(&[0x1f, 0x8b, 0x08])
        {
            return Ok(());
        }
        let Some(trailer) = source.0.checked_add(source_len - 4).map(GuestAddr) else {
            return Ok(());
        };
        let Some(source_end) = source.0.checked_add(source_len) else {
            return Ok(());
        };
        let Some(screen_end) = self.screen_base.0.checked_add(screen_memory_len) else {
            return Ok(());
        };
        if source.0 < screen_end && self.screen_base.0 < source_end {
            return Ok(());
        }
        let Some(required_len) = self.memory.read_u32(trailer).ok() else {
            return Ok(());
        };
        let capacity_is_unsafe = output_len > screen_memory_len;
        let missing_usable_capacity =
            output_len < required_len && required_len <= screen_memory_len;
        if !capacity_is_unsafe && !missing_usable_capacity {
            return Ok(());
        }

        // Physical runtimes expose this fixed platform-owned allocation even
        // when legacy guest code leaves its local capacity scalar indeterminate.
        // Recover the capacity from ABI data at a guest call boundary; never
        // identify a decompressor by its PC or compiled instruction sequence.
        if self
            .memory
            .check_range(output, screen_memory_len as usize, Permissions::READ_WRITE)
            .is_ok()
        {
            self.memory
                .write_u32(output_len_pointer, screen_memory_len)?;
        }
        Ok(())
    }

    fn run_guest_execution(
        &mut self,
        execution: GuestExecution,
        services: &mut dyn NativeServices,
    ) -> Result<u32> {
        let GuestExecution {
            function,
            mut cpu,
            mut entered_guest_call,
            mut instruction_count,
        } = execution;
        let trace_arm = std::env::var_os("SKYENGINE_TRACE_ARM").is_some();
        while instruction_count < INSTRUCTION_BUDGET {
            let pc = cpu.pc().0;
            if let Some(return_to_thumb) = entered_guest_call.take()
                && self.try_dispatch_legacy_external_action(
                    function.module,
                    return_to_thumb,
                    &mut cpu,
                )?
            {
                instruction_count += 1;
                continue;
            }
            if pc == RETURN_SENTINEL {
                return Ok(cpu.register(0));
            }
            if let Some(slot) = trap_slot(pc) {
                if let Err(error) = self.dispatch(slot, function.module, &mut cpu, services) {
                    let context = format!(
                        " while dispatching module {} slot {slot} at LR {:#010x} (r0={:#010x}, r1={:#010x}, r2={:#010x}, r3={:#010x}, sp={:#010x})",
                        function.module,
                        cpu.register(14),
                        cpu.register(0),
                        cpu.register(1),
                        cpu.register(2),
                        cpu.register(3),
                        cpu.register(13),
                    );
                    return Err(match error {
                        Error::ArmFault(message) => Error::ArmFault(message + &context),
                        Error::Abi(message) => Error::Abi(message + &context),
                        other => other,
                    });
                }
                let return_address = cpu.register(14);
                cpu.set_pc(return_address);
                instruction_count += 1;
                continue;
            }
            if trace_arm {
                eprintln!(
                    "[arm-step] module={} n={} pc={pc:#010x} cpsr={:#010x} r0={:#010x} r1={:#010x} r2={:#010x} r3={:#010x} r9={:#010x} sp={:#010x} lr={:#010x}",
                    function.module,
                    instruction_count,
                    cpu.cpsr(),
                    cpu.register(0),
                    cpu.register(1),
                    cpu.register(2),
                    cpu.register(3),
                    cpu.register(9),
                    cpu.register(13),
                    cpu.register(14),
                );
            }
            let previous_lr = cpu.register(14);
            let previous_thumb = cpu.is_thumb();
            let sequential_pc = pc.wrapping_add(if previous_thumb { 2 } else { 4 });
            if let Err(error) = cpu.step(&mut self.memory) {
                return Err(match error {
                    Error::ArmFault(message) => {
                        let instruction = self
                            .memory
                            .read(GuestAddr(pc), 4)
                            .map(|bytes| {
                                format!(
                                    "{:02x}{:02x}{:02x}{:02x}",
                                    bytes[0], bytes[1], bytes[2], bytes[3]
                                )
                            })
                            .unwrap_or_else(|_| "unavailable".into());
                        Error::ArmFault(format!(
                            "{message} while executing module {} at PC {pc:#010x} (insn={instruction}, cpsr={:#010x}, r0={:#010x}, r1={:#010x}, r2={:#010x}, r3={:#010x}, r4={:#010x}, r5={:#010x}, r6={:#010x}, r7={:#010x}, r8={:#010x}, r9={:#010x}, r10={:#010x}, r11={:#010x}, r12={:#010x}, sp={:#010x}, lr={:#010x})",
                            function.module,
                            cpu.cpsr(),
                            cpu.register(0),
                            cpu.register(1),
                            cpu.register(2),
                            cpu.register(3),
                            cpu.register(4),
                            cpu.register(5),
                            cpu.register(6),
                            cpu.register(7),
                            cpu.register(8),
                            cpu.register(9),
                            cpu.register(10),
                            cpu.register(11),
                            cpu.register(12),
                            cpu.register(13),
                            cpu.register(14),
                        ))
                    }
                    other => other,
                });
            }
            if cpu.take_semihosting_exit_reason().is_some() {
                self.exit_requested = true;
                return Ok(0);
            }
            if cpu.register(14) != previous_lr && cpu.pc().0 != sequential_pc {
                let instruction_len = if cpu.is_thumb() { 2 } else { 4 };
                if self
                    .memory
                    .check_range(cpu.pc(), instruction_len, Permissions::EXECUTE)
                    .is_ok()
                {
                    self.prepare_guest_gzip_screen_buffer_capacity(&cpu)?;
                }
                entered_guest_call = Some(previous_thumb);
            }
            instruction_count += 1;
        }
        let pc = cpu.pc().0;
        let instruction = self
            .memory
            .read(GuestAddr(pc), if cpu.is_thumb() { 2 } else { 4 })
            .map(|bytes| {
                bytes
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect::<String>()
            })
            .unwrap_or_else(|_| "unavailable".into());
        Err(Error::ArmFault(format!(
            "instruction budget {INSTRUCTION_BUDGET} exhausted in module {} at PC {pc:#010x} (insn={instruction}, cpsr={:#010x}, r0={:#010x}, r1={:#010x}, r2={:#010x}, r3={:#010x}, r4={:#010x}, r5={:#010x}, r6={:#010x}, r7={:#010x}, r8={:#010x}, r9={:#010x}, r10={:#010x}, r11={:#010x}, r12={:#010x}, sp={:#010x}, lr={:#010x})",
            function.module,
            cpu.cpsr(),
            cpu.register(0),
            cpu.register(1),
            cpu.register(2),
            cpu.register(3),
            cpu.register(4),
            cpu.register(5),
            cpu.register(6),
            cpu.register(7),
            cpu.register(8),
            cpu.register(9),
            cpu.register(10),
            cpu.register(11),
            cpu.register(12),
            cpu.register(13),
            cpu.register(14),
        )))
    }

    fn platform_data_slot_address(&self, slot: u32) -> Result<GuestAddr> {
        let address = GuestAddr(self.memory.read_u32(table_slot_address(slot))?);
        if address.0 == 0 {
            return Err(Error::Abi(format!(
                "platform data slot {slot} contains a null variable address"
            )));
        }
        Ok(address)
    }

    fn read_platform_data_slot(&self, slot: u32) -> Result<u32> {
        let variable = self.platform_data_slot_address(slot)?;
        self.memory.read_u32(variable)
    }

    fn read_c_string(&self, address: GuestAddr, limit: usize) -> Result<Vec<u8>> {
        let mut bytes = Vec::new();
        for offset in 0..limit {
            let byte = self.memory.read_u8(address.checked_add(offset as u32)?)?;
            if byte == 0 {
                return Ok(bytes);
            }
            bytes.push(byte);
        }
        Err(Error::Abi(format!(
            "guest C string at {:#010x} exceeds {limit} bytes",
            address.0
        )))
    }

    fn write_directory_entry(
        &mut self,
        output: GuestAddr,
        output_len: usize,
        entry: &[u8],
    ) -> Result<bool> {
        let Some(required) = entry.len().checked_add(1) else {
            return Ok(false);
        };
        if output.0 == 0 || required > output_len {
            return Ok(false);
        }
        self.memory.write(output, entry)?;
        self.memory
            .write_u8(output.checked_add(entry.len() as u32)?, 0)?;
        Ok(true)
    }

    fn read_c_string_bounded(&self, address: GuestAddr, limit: usize) -> Result<Vec<u8>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let mut bytes = self.memory.read(address, limit)?;
        if let Some(nul) = bytes.iter().position(|byte| *byte == 0) {
            bytes.truncate(nul);
        }
        Ok(bytes)
    }

    fn read_wide_string_be(&self, address: GuestAddr, limit: usize) -> Result<Vec<u16>> {
        let mut codepoints = Vec::new();
        for offset in 0..limit {
            let address = address.checked_add(
                u32::try_from(offset)
                    .ok()
                    .and_then(|offset| offset.checked_mul(2))
                    .ok_or_else(|| Error::Abi("guest wide-string offset overflow".into()))?,
            )?;
            let bytes = self.memory.read(address, 2)?;
            let codepoint = u16::from_be_bytes([bytes[0], bytes[1]]);
            if codepoint == 0 {
                return Ok(codepoints);
            }
            codepoints.push(codepoint);
        }
        Err(Error::Abi(format!(
            "guest wide string at {:#010x} exceeds {limit} code units",
            address.0
        )))
    }
}

fn platform_user_info() -> [u8; PLATFORM_USER_INFO_LEN] {
    let mut info = [0_u8; PLATFORM_USER_INFO_LEN];
    info[..VIRTUAL_IMEI.len()].copy_from_slice(VIRTUAL_IMEI);
    info[16..16 + VIRTUAL_IMSI.len()].copy_from_slice(VIRTUAL_IMSI);
    info[32..40].copy_from_slice(b"SkyEng\0\0");
    info[40..48].copy_from_slice(b"SE-V2\0\0\0");
    info[48..52].copy_from_slice(&PLATFORM_USER_INFO_VERSION.to_le_bytes());
    info
}

fn trap_slot(address: u32) -> Option<u32> {
    let offset = address.checked_sub(TRAP_BASE)?;
    (offset % 4 == 0 && offset / 4 < PLATFORM_SLOT_COUNT).then_some(offset / 4)
}

fn data_slot_address(slot: u32) -> GuestAddr {
    GuestAddr(PLATFORM_DATA.0 + slot * 4)
}

fn platform_data_slot_backing_address(slot: u32) -> GuestAddr {
    match slot {
        95 => BITMAP_ARRAY_DATA,
        96 => TILE_ARRAY_DATA,
        97 => MAP_ARRAY_DATA,
        98 => SOUND_ARRAY_DATA,
        99 => SPRITE_ARRAY_DATA,
        100 => PACKAGE_NAME_DATA,
        101 => START_NAME_DATA,
        102 => PREVIOUS_PACKAGE_NAME_DATA,
        103 => PREVIOUS_START_NAME_DATA,
        112 => SMS_CONFIG_DATA,
        138 => START_FILE_PARAMETER_DATA,
        144 => CURRENT_ENTRY_DATA,
        _ => data_slot_address(slot),
    }
}

fn table_slot_address(slot: u32) -> GuestAddr {
    GuestAddr(PLATFORM_TABLE.0 + slot * 4)
}

fn write_platform_string(memory: &mut GuestMemory, address: GuestAddr, value: &[u8]) -> Result<()> {
    if value.len() >= 256 {
        return Err(Error::Abi(format!(
            "platform string contains {} bytes (limit 255)",
            value.len()
        )));
    }
    memory.write(address, value)?;
    memory.write_u8(address.checked_add(value.len() as u32)?, 0)
}

fn native_file_open_result(result: i32) -> u32 {
    if result >= 0 {
        return result as u32;
    }
    // Unlike the other MRC file APIs, mrc_open uses a NULL-style failure handle.
    0
}

fn is_function_slot(slot: u32) -> bool {
    matches!(
        slot,
        0..=20
            | 22
            | 25..=65
            | 67..=90
            | 113..=134
            | 137
            | 141
            | 145
            | 147..=148
    )
}

fn is_data_slot(slot: u32) -> bool {
    matches!(slot, 91..=112 | 135..=136 | 138..=140 | 142..=144 | 146)
}

#[cfg(test)]
mod tests;
