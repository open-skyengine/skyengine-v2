use std::{
    collections::{BTreeMap, BTreeSet},
    io::{Read, Write},
    net::{Ipv4Addr, SocketAddr, SocketAddrV4, TcpStream},
    path::PathBuf,
    sync::{Arc, mpsc},
    thread,
    time::{Duration, Instant},
};

use flate2::read::GzDecoder;

use crate::{Error, Framebuffer, Package, ResourceLimits, Result};

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
const PACKAGE_NAME_DATA: GuestAddr = GuestAddr(0x0100_1400);
const START_NAME_DATA: GuestAddr = GuestAddr(0x0100_1500);
const PREVIOUS_PACKAGE_NAME_DATA: GuestAddr = GuestAddr(0x0100_1600);
const PREVIOUS_START_NAME_DATA: GuestAddr = GuestAddr(0x0100_1700);
const CURRENT_ENTRY_DATA: GuestAddr = GuestAddr(0x0100_1800);
const INTERNAL_TABLE_DATA: GuestAddr = GuestAddr(0x0100_1900);
const APPLICATION_STATE_DATA: GuestAddr = GuestAddr(0x0100_1980);
const LIFECYCLE_CALLBACK_DATA: GuestAddr = GuestAddr(0x0100_1984);
const TIMER_ACTIVE_DATA: GuestAddr = GuestAddr(0x0100_1988);
const PLATFORM_SIM_INFO_DATA: GuestAddr = GuestAddr(0x0100_1a00);
const PLATFORM_SIM_INFO_LEN: usize = 12;
const PLATFORM_STORAGE_INFO_DATA: GuestAddr = GuestAddr(0x0100_1a10);
const PLATFORM_STORAGE_INFO_LEN: usize = 16;
const PLATFORM_STORAGE_DRIVE_DATA: GuestAddr = GuestAddr(0x0100_1a20);
const PLATFORM_STORAGE_DRIVE_LEN: usize = 2;
const PLATFORM_USER_INFO_LEN: usize = 64;
// Common MTK EXT fixtures identify the 1.0.4 runtime through this encoded version.
const PLATFORM_USER_INFO_VERSION: u32 = 101_040_000;
const MTK_NATIVE_EXTENSION_BASE: GuestAddr = GuestAddr(0x4001_8800);
const MTK_NATIVE_EXTENSION_LEN: usize = MODULE_STRIDE as usize;
const PLATFORM_STORAGE_BLOCK_SIZE: u32 = 4 * 1024;
const PLATFORM_STORAGE_AVAILABLE_BLOCKS: u32 = 4 * 1024;
const INTERNAL_APPLICATION_STATE_OFFSETS: [u32; 2] = [8, 44];
const MODULE_BASE: u32 = 0x1000_0000;
const MODULE_STRIDE: u32 = 0x0010_0000;
const HEAP_BASE: GuestAddr = GuestAddr(0x2000_0000);
const MIN_GUEST_RAM_LEN: usize = 8 * 1024 * 1024;
#[cfg(test)]
const DEFAULT_HEAP_LEN: usize = 4 * 1024 * 1024;
const STACK_BASE: GuestAddr = GuestAddr(0x3000_0000);
const STACK_LEN: usize = 256 * 1024;
const PLATFORM_MEMORY_BASE: GuestAddr = GuestAddr(0x4000_0000);
const SCREEN_BASE: GuestAddr = GuestAddr(HEAP_BASE.0 + MIN_GUEST_RAM_LEN as u32);
const FREE_BLOCK_HEADER_LEN: u32 = 8;
const ALLOCATED_BLOCK_HEADER_LEN: u32 = FREE_BLOCK_HEADER_LEN;
const HEAP_ALIGNMENT: u32 = 8;
const BITMAP_ENTRY_SIZE: u32 = 16;
const SCREEN_BITMAP_ID: u32 = 30;
const TRAP_BASE: u32 = 0xff00_0000;
const RETURN_SENTINEL: u32 = 0xffff_ff00;
const PLATFORM_SLOT_COUNT: u32 = 150;
const INSTRUCTION_BUDGET: u64 = 200_000_000;
const MD5_BUFFER_OFFSET: u32 = 24;
const MAX_NATIVE_SOCKETS: usize = 64;
const NETWORK_CONNECT_TIMEOUT: Duration = Duration::from_secs(3);

pub(crate) trait NativeServices {
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
    fn seek_file(&mut self, handle: i32, offset: i32, origin: u32) -> Result<bool>;
    fn file_len(&mut self, name: &[u8]) -> Result<Option<u64>>;
    fn find_start(&mut self, directory: &[u8]) -> Result<Option<(i32, Vec<u8>)>>;
    fn find_next(&mut self, handle: i32) -> Result<Option<Vec<u8>>>;
    fn find_stop(&mut self, handle: i32) -> Result<bool>;
    fn char_bitmap(&mut self, codepoint: u32, font: u32) -> Result<Option<(Vec<u8>, u32, u32)>>;
    fn draw_bitmap(
        &mut self,
        pixels: &[u8],
        x: i32,
        y: i32,
        width: usize,
        height: usize,
    ) -> Result<()>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ExtLifecycleRequest {
    Restart { package: Vec<u8>, entry: Vec<u8> },
    Exit,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DeviceInfoProfile {
    Unavailable,
    DeterministicMtk,
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
}

#[derive(Debug)]
struct ModuleContext {
    base: GuestAddr,
    len: usize,
    loader_context: GuestAddr,
    helper: Option<GuestFunction>,
    helper_parameter: GuestAddr,
    static_base_r9: u32,
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

#[derive(Clone, Copy, Debug)]
struct BitmapDescriptor {
    pixels: GuestAddr,
    width: usize,
    height: usize,
    x: i32,
    y: i32,
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

#[derive(Debug)]
enum NativeSocketState {
    Created,
    Connecting(mpsc::Receiver<std::io::Result<TcpStream>>),
    Connected(TcpStream),
    Failed,
}

#[derive(Debug)]
struct NativeSocket {
    state: NativeSocketState,
}

#[derive(Debug)]
pub(crate) struct ExtRuntime {
    memory: GuestMemory,
    modules: Vec<ModuleContext>,
    active_helper: Option<GuestFunction>,
    heap_len: usize,
    platform_memory_extensions: BTreeMap<u32, (usize, u32)>,
    platform_memory_cursor: u32,
    random_state: u32,
    glyphs: BTreeMap<(u32, u32), GuestGlyph>,
    dialogs: BTreeMap<u32, PlatformDialog>,
    next_ui_handle: u32,
    suppressed_ui_key_releases: BTreeSet<i32>,
    native_sockets: BTreeMap<i32, NativeSocket>,
    next_native_socket_handle: i32,
    exit_requested: bool,
    device_info_profile: DeviceInfoProfile,
    clock_origin: Instant,
    timer_deadline: Option<Instant>,
}

impl ExtRuntime {
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
        let heap_end = HEAP_BASE
            .0
            .checked_add(heap_len as u32)
            .ok_or_else(|| Error::ArmFault("guest heap end overflow".into()))?;
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
        memory.map(
            HEAP_BASE,
            heap_len.max(MIN_GUEST_RAM_LEN),
            Permissions::READ_WRITE,
            "guest RAM",
        )?;
        memory.map(STACK_BASE, STACK_LEN, Permissions::READ_WRITE, "EXT stack")?;
        let screen_len = usize::from(screen_width)
            .checked_mul(usize::from(screen_height))
            .and_then(|pixels| pixels.checked_mul(2))
            .ok_or_else(|| Error::ArmFault("guest screen buffer size overflow".into()))?;
        memory.map(
            SCREEN_BASE,
            screen_len,
            Permissions::READ_WRITE,
            "screen buffer",
        )?;

        for slot in 0..PLATFORM_SLOT_COUNT {
            let value = if is_function_slot(slot) {
                TRAP_BASE + slot * 4
            } else if is_data_slot(slot) {
                PLATFORM_DATA.0 + slot * 4
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
        memory.write_u32(table_slot_address(100), PACKAGE_NAME_DATA.0)?;
        memory.write_u32(table_slot_address(101), START_NAME_DATA.0)?;
        memory.write_u32(table_slot_address(102), PREVIOUS_PACKAGE_NAME_DATA.0)?;
        memory.write_u32(table_slot_address(103), PREVIOUS_START_NAME_DATA.0)?;
        memory.write_u32(table_slot_address(144), CURRENT_ENTRY_DATA.0)?;
        memory.write_u32(table_slot_address(23), INTERNAL_TABLE_DATA.0)?;
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
        memory.write_u32(APPLICATION_STATE_DATA, 1)?;
        memory.write_u32(data_slot_address(91), SCREEN_BASE.0)?;
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
        memory.write_u32(screen_bitmap.checked_add(12)?, SCREEN_BASE.0)?;
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
            PLATFORM_STORAGE_AVAILABLE_BLOCKS * 2,
            PLATFORM_STORAGE_AVAILABLE_BLOCKS,
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
        memory.write(PLATFORM_STORAGE_DRIVE_DATA, b"C\0")?;

        Ok(Self {
            memory,
            modules: Vec::new(),
            active_helper: None,
            heap_len,
            platform_memory_extensions: BTreeMap::new(),
            platform_memory_cursor: PLATFORM_MEMORY_BASE.0,
            random_state: 1,
            glyphs: BTreeMap::new(),
            dialogs: BTreeMap::new(),
            next_ui_handle: 1,
            suppressed_ui_key_releases: BTreeSet::new(),
            native_sockets: BTreeMap::new(),
            next_native_socket_handle: 1,
            exit_requested: false,
            device_info_profile: DeviceInfoProfile::Unavailable,
            clock_origin: Instant::now(),
            timer_deadline: None,
        })
    }

    pub fn load_and_call_entry(
        &mut self,
        image: &[u8],
        code: i32,
        services: &mut dyn NativeServices,
    ) -> Result<i32> {
        if !image.starts_with(b"MRPGCMAP") || image.len() <= 8 {
            return Err(Error::Abi(
                "EXT image is missing the complete MRPGCMAP marker".into(),
            ));
        }
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
        if image.len() > MODULE_STRIDE as usize {
            return Err(Error::ArmFault(format!(
                "EXT image is {} bytes (module stride is {})",
                image.len(),
                MODULE_STRIDE
            )));
        }
        self.memory.map_bytes(
            base,
            image.to_vec(),
            Permissions::READ_WRITE_EXECUTE,
            format!("EXT module {module_index}"),
        )?;
        let loader_context = self.allocate(64, 8)?;
        self.memory.write(loader_context, &[0; 64])?;
        self.memory.write_u32(base, PLATFORM_TABLE.0)?;
        self.memory
            .write_u32(base.checked_add(4)?, loader_context.0)?;
        self.modules.push(ModuleContext {
            base,
            len: image.len(),
            loader_context,
            helper: None,
            helper_parameter: GuestAddr(0),
            static_base_r9: 0,
        });

        let result = self
            .call_guest(
                GuestFunction {
                    module: module_index,
                    address: base.0 + 8,
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
            });
        if result.is_err() {
            self.modules.pop();
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
        let input_address = if input.is_empty() {
            GuestAddr(0)
        } else {
            let address = self.allocate(input.len(), 4)?;
            self.memory.write(address, input)?;
            address
        };
        let output_fields = self.allocate(8, 4)?;
        self.memory.write_u32(output_fields, 0)?;
        self.memory.write_u32(output_fields.checked_add(4)?, 0)?;
        let module_parameter = self.modules[helper.module].helper_parameter;
        let return_value = self.call_guest(
            helper,
            [
                module_parameter.0,
                code as u32,
                input_address.0,
                input.len() as u32,
            ],
            &[output_fields.0, output_fields.0 + 4],
            services,
        )? as i32;
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

    pub fn set_previous_application(&mut self, package: &[u8], entry: &[u8]) -> Result<()> {
        write_platform_string(&mut self.memory, PREVIOUS_PACKAGE_NAME_DATA, package)?;
        write_platform_string(&mut self.memory, PREVIOUS_START_NAME_DATA, entry)
    }

    pub fn set_device_info_profile(&mut self, profile: DeviceInfoProfile) -> Result<()> {
        if profile == self.device_info_profile {
            return Ok(());
        }
        if self.device_info_profile != DeviceInfoProfile::Unavailable {
            return Err(Error::Abi(
                "device-information profile cannot change after configuration".into(),
            ));
        }
        if profile == DeviceInfoProfile::DeterministicMtk {
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
        self.device_info_profile = profile;
        Ok(())
    }

    pub fn route_key_event(&mut self, code: i32, pressed: bool) -> Option<(i32, i32, i32)> {
        if !pressed && self.suppressed_ui_key_releases.remove(&code) {
            return None;
        }
        if self.dialogs.is_empty() {
            return Some((if pressed { 0 } else { 1 }, code, 0));
        }
        if !pressed {
            return None;
        }
        self.suppressed_ui_key_releases.insert(code);
        match code {
            // Left soft key and select accept; right soft key and power cancel.
            17 | 20 => Some((6, 1, 0)),
            16 | 18 => Some((6, 0, 0)),
            _ => None,
        }
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

    fn call_guest(
        &mut self,
        function: GuestFunction,
        registers: [u32; 4],
        stack_arguments: &[u32],
        services: &mut dyn NativeServices,
    ) -> Result<u32> {
        let module = self.modules.get(function.module).ok_or_else(|| {
            Error::Abi(format!(
                "guest function references module {}",
                function.module
            ))
        })?;
        let module_end = module.base.0 + module.len as u32;
        let executable_address = function.address & !1;
        if executable_address < module.base.0 || executable_address >= module_end {
            return Err(Error::Abi(format!(
                "guest function {:#010x} is outside module {}",
                function.address, function.module
            )));
        }
        let mut cpu = ArmCpu::new();
        for (index, value) in registers.into_iter().enumerate() {
            cpu.set_register(index, value);
        }
        cpu.set_register(9, module.static_base_r9);
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

        for instruction_count in 0..INSTRUCTION_BUDGET {
            let pc = cpu.pc().0;
            if pc == RETURN_SENTINEL {
                return Ok(cpu.register(0));
            }
            if let Some(slot) = trap_slot(pc) {
                self.dispatch(slot, function.module, &mut cpu, services)?;
                let return_address = cpu.register(14);
                cpu.set_pc(return_address);
                continue;
            }
            if std::env::var_os("SKYENGINE_TRACE_ARM").is_some() {
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
    info[..16].copy_from_slice(b"000000000000000\0");
    info[16..32].copy_from_slice(b"460001234567890\0");
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
