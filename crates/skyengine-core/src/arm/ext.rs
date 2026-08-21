use std::{
    collections::{BTreeMap, BTreeSet},
    io::Read,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use flate2::read::GzDecoder;

use crate::{Error, Framebuffer, Package, ResourceLimits, Result};

use super::{ArmCpu, GuestAddr, GuestMemory, Permissions};

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
const PLATFORM_USER_INFO_VERSION: u32 = 1_001;
const PLATFORM_STORAGE_BLOCK_SIZE: u32 = 4 * 1024;
const PLATFORM_STORAGE_AVAILABLE_BLOCKS: u32 = 4 * 1024;
const INTERNAL_APPLICATION_STATE_OFFSETS: [u32; 2] = [8, 44];
const MODULE_BASE: u32 = 0x1000_0000;
const MODULE_STRIDE: u32 = 0x0010_0000;
const HEAP_BASE: GuestAddr = GuestAddr(0x2000_0000);
#[cfg(test)]
const DEFAULT_HEAP_LEN: usize = 4 * 1024 * 1024;
const STACK_BASE: GuestAddr = GuestAddr(0x3000_0000);
const STACK_LEN: usize = 256 * 1024;
const SCREEN_BASE: GuestAddr = GuestAddr(0x4000_0000);
const BITMAP_ENTRY_SIZE: u32 = 16;
const SCREEN_BITMAP_ID: u32 = 30;
const TRAP_BASE: u32 = 0xff00_0000;
const RETURN_SENTINEL: u32 = 0xffff_ff00;
const PLATFORM_SLOT_COUNT: u32 = 150;
const INSTRUCTION_BUDGET: u64 = 20_000_000;
const MD5_BUFFER_OFFSET: u32 = 24;

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

#[derive(Debug)]
pub(crate) struct ExtRuntime {
    memory: GuestMemory,
    modules: Vec<ModuleContext>,
    active_helper: Option<GuestFunction>,
    heap_cursor: u32,
    heap_len: usize,
    platform_memory_extensions: BTreeMap<u32, (usize, u32)>,
    random_state: u32,
    glyphs: BTreeMap<(u32, u32), GuestGlyph>,
    dialogs: BTreeMap<u32, PlatformDialog>,
    next_ui_handle: u32,
    suppressed_ui_key_releases: BTreeSet<i32>,
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
        memory.map(HEAP_BASE, heap_len, Permissions::READ_WRITE, "guest heap")?;
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
            heap_cursor: HEAP_BASE.0,
            heap_len,
            platform_memory_extensions: BTreeMap::new(),
            random_state: 1,
            glyphs: BTreeMap::new(),
            dialogs: BTreeMap::new(),
            next_ui_handle: 1,
            suppressed_ui_key_releases: BTreeSet::new(),
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

    pub fn set_device_info_profile(&mut self, profile: DeviceInfoProfile) {
        self.device_info_profile = profile;
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
                    Error::ArmFault(message) => Error::ArmFault(format!(
                        "{message} while executing module {} at PC {pc:#010x} (r0={:#010x}, r1={:#010x}, r2={:#010x}, r3={:#010x}, r9={:#010x}, sp={:#010x}, lr={:#010x})",
                        function.module,
                        cpu.register(0),
                        cpu.register(1),
                        cpu.register(2),
                        cpu.register(3),
                        cpu.register(9),
                        cpu.register(13),
                        cpu.register(14),
                    )),
                    other => other,
                });
            }
        }
        Err(Error::ArmFault(format!(
            "instruction budget {INSTRUCTION_BUDGET} exhausted in module {} at PC {:#010x}",
            function.module,
            cpu.pc().0
        )))
    }

    fn dispatch(
        &mut self,
        slot: u32,
        module: usize,
        cpu: &mut ArmCpu,
        services: &mut dyn NativeServices,
    ) -> Result<()> {
        if std::env::var_os("SKYENGINE_TRACE_ARM").is_some() {
            eprintln!(
                "[arm-trap] module={module} slot={slot} r0={:#010x} r1={:#010x} r2={:#010x} r3={:#010x} r9={:#010x}",
                cpu.register(0),
                cpu.register(1),
                cpu.register(2),
                cpu.register(3),
                cpu.register(9),
            );
        }
        match slot {
            0..=20 => self.dispatch_libc(slot, cpu)?,
            25 => {
                let helper = cpu.register(0);
                let parameter_len = cpu.register(1).max(20) as usize;
                let parameter = self.allocate(parameter_len, 8)?;
                self.memory.write(parameter, &vec![0; parameter_len])?;
                let function = GuestFunction {
                    module,
                    address: helper,
                };
                let context = self.modules.get_mut(module).ok_or_else(|| {
                    Error::Abi(format!("helper registration for missing module {module}"))
                })?;
                context.helper = Some(function);
                context.helper_parameter = parameter;
                self.active_helper = Some(function);
                cpu.set_register(0, parameter.0);
            }
            26 => {
                let format = self.read_c_string(GuestAddr(cpu.register(0)), 64 * 1024)?;
                if std::env::var_os("SKYENGINE_TRACE_ARM").is_some() {
                    eprintln!(
                        "[guest-printf] format={:?} r1={:#010x} r2={:#010x} r3={:#010x}",
                        String::from_utf8_lossy(&format),
                        cpu.register(1),
                        cpu.register(2),
                        cpu.register(3)
                    );
                }
                cpu.set_register(0, format.len() as u32);
            }
            29 => {
                let source = GuestAddr(cpu.register(0));
                let x = cpu.register(1) as i32;
                let y = cpu.register(2) as i32;
                let width = cpu.register(3) as usize;
                let height = self.memory.read_u32(GuestAddr(cpu.register(13)))? as usize;
                let pixels = self.read_platform_draw_pixels(source, x, y, width, height)?;
                services.draw_bitmap(&pixels, x, y, width, height)?;
                cpu.set_register(0, 0);
            }
            30 => {
                let codepoint = cpu.register(0);
                let font = cpu.register(1);
                let width_out = GuestAddr(cpu.register(2));
                let height_out = GuestAddr(cpu.register(3));
                let key = (codepoint, font);
                let glyph = match self.glyphs.get(&key).copied() {
                    Some(glyph) => Some(glyph),
                    None => match services.char_bitmap(codepoint, font)? {
                        Some((bitmap, width, height)) => {
                            let bitmap =
                                bitmap.into_iter().map(u8::reverse_bits).collect::<Vec<_>>();
                            let address = self.allocate(bitmap.len(), 4)?;
                            self.memory.write(address, &bitmap)?;
                            let glyph = GuestGlyph {
                                address,
                                width,
                                height,
                            };
                            self.glyphs.insert(key, glyph);
                            Some(glyph)
                        }
                        None => None,
                    },
                };
                let (address, width, height) = glyph
                    .map(|glyph| (glyph.address.0, glyph.width, glyph.height))
                    .unwrap_or((0, 0, 0));
                if width_out.0 != 0 {
                    self.memory.write_u32(width_out, width)?;
                }
                if height_out.0 != 0 {
                    self.memory.write_u32(height_out, height)?;
                }
                cpu.set_register(0, address);
            }
            31 => {
                let delay = Duration::from_millis(u64::from(cpu.register(0)));
                self.timer_deadline = Instant::now().checked_add(delay);
                self.memory.write_u32(TIMER_ACTIVE_DATA, 1)?;
                cpu.set_register(0, 0);
            }
            32 => {
                self.timer_deadline = None;
                self.memory.write_u32(TIMER_ACTIVE_DATA, 0)?;
                cpu.set_register(0, 0);
            }
            33 => {
                cpu.set_register(0, self.clock_origin.elapsed().as_millis() as u32);
            }
            34 => {
                let output = GuestAddr(cpu.register(0));
                self.memory.write_u16(output, 2012)?;
                self.memory.write(
                    output.checked_add(2)?,
                    // month, day, hour, minute, second, weekday (Sunday = 0)
                    &[6, 20, 0, 0, 0, 3],
                )?;
                cpu.set_register(0, 0);
            }
            35 => {
                let output = GuestAddr(cpu.register(0));
                match self.device_info_profile {
                    DeviceInfoProfile::Unavailable => {
                        // The baseline profile has no device-information provider.
                        // Leave the caller-owned output buffer untouched.
                        cpu.set_register(0, u32::MAX);
                    }
                    DeviceInfoProfile::DeterministicMtk if output.0 == 0 => {
                        cpu.set_register(0, u32::MAX);
                    }
                    DeviceInfoProfile::DeterministicMtk => {
                        self.memory.write(output, &platform_user_info())?;
                        cpu.set_register(0, 0);
                    }
                }
            }
            36 => {
                // The outer runtime owns scheduling; acknowledge guest sleeps
                // without blocking the event and control loops.
                cpu.set_register(0, 0);
            }
            37 => match (cpu.register(0), cpu.register(1)) {
                // Baseline SDK initialization notification; the return value is ignored.
                (1_106, 0) => cpu.set_register(0, 0),
                // Report the normal storage profile. 1002 denotes USB mass-storage
                // mode, in which applications must not access their regular volume.
                (1_218, 0) => cpu.set_register(0, 1_001),
                // Network request compatibility version used by message.ext.
                (1_205, 0) => cpu.set_register(0, 1_001),
                // Optional dual-SIM selection probe. A false result keeps the
                // guest on its default network selection path.
                (1_327, 0) => cpu.set_register(0, u32::MAX),
                // No explicit SIM/network selection is configured.
                (1_328, 0) => cpu.set_register(0, u32::MAX),
                (command, argument) => {
                    return Err(Error::Abi(format!(
                        "unsupported platform slot 37 command ({command}, {argument}) called by module {module}"
                    )));
                }
            },
            38 => match cpu.register(0) {
                // Requests an additional guest-memory arena. The requested byte
                // count is carried in input_len even though input is null; the
                // returned arena follows the normal mr_platEx output convention.
                1_014 if cpu.register(1) == 0 => self.allocate_platform_memory_extension(cpu)?,
                // Releases an arena returned by command 1014. The ABI carries
                // the 32-bit guest address as a four-byte input buffer.
                1_015 => self.release_platform_memory_extension(cpu)?,
                // Resolve the logical application storage volume to a drive.
                1_204 => self.return_platform_storage_drive(cpu)?,
                // Optional platform metadata query. No metadata provider is configured.
                1_222 => self.return_unavailable_platform_extension(cpu)?,
                // Optional device metadata blob used to enrich network requests.
                1_116 if cpu.register(1) == 0 && cpu.register(2) == 0 => {
                    self.return_unavailable_platform_extension(cpu)?
                }
                // Returns the available SIM slots. The headless baseline has no
                // carrier provider, so expose a valid empty result structure.
                1_307 if cpu.register(1) == 0 && cpu.register(2) == 0 => {
                    self.return_platform_sim_info(cpu)?
                }
                // Disk geometry used by the guest's startup space check.
                1_305 if cpu.register(2) == 1 => self.return_platform_storage_info(cpu)?,
                // Optional platform control/query without input or output buffers.
                1_223 if cpu.register(1) == 0 && cpu.register(2) == 0 && cpu.register(3) == 0 => {
                    cpu.set_register(0, u32::MAX)
                }
                // Optional vendor capability probe. The baseline headless profile
                // does not provide it, so report the ABI failure value.
                0x0009_0003
                    if cpu.register(1) == 0 && cpu.register(2) == 0 && cpu.register(3) == 0 =>
                {
                    cpu.set_register(0, u32::MAX)
                }
                // Observed optional vendor extension with an opaque input record
                // and no output buffer. This profile does not provide it.
                0x0009_0004
                    if cpu.register(1) != 0 && cpu.register(2) != 0 && cpu.register(3) == 0 =>
                {
                    cpu.set_register(0, u32::MAX)
                }
                // Optional vendor capability structure.
                0x0007_0001 if cpu.register(1) == 0 && cpu.register(2) == 0 => {
                    self.return_unavailable_platform_extension(cpu)?
                }
                command => {
                    return Err(Error::Abi(format!(
                        "unsupported platform slot 38 command {command} called by module {module}"
                    )));
                }
            },
            40 => {
                let name = self.read_c_string(GuestAddr(cpu.register(0)), 1024)?;
                cpu.set_register(0, services.open_file(&name, cpu.register(1))? as u32);
            }
            41 => {
                cpu.set_register(0, services.close_file(cpu.register(0) as i32)? as u32);
            }
            42 => {
                let name = self.read_c_string(GuestAddr(cpu.register(0)), 1024)?;
                cpu.set_register(0, services.file_info(&name)? as u32);
            }
            43 => {
                let handle = cpu.register(0) as i32;
                let bytes = self
                    .memory
                    .read(GuestAddr(cpu.register(1)), cpu.register(2) as usize)?;
                cpu.set_register(
                    0,
                    services
                        .write_file(handle, &bytes)?
                        .and_then(|written| u32::try_from(written).ok())
                        .unwrap_or(u32::MAX),
                );
            }
            44 => {
                let handle = cpu.register(0) as i32;
                let destination = GuestAddr(cpu.register(1));
                let len = cpu.register(2) as usize;
                match services.read_file(handle, len)? {
                    Some(bytes) => {
                        self.memory.write(destination, &bytes)?;
                        cpu.set_register(0, bytes.len() as u32);
                    }
                    None => cpu.set_register(0, u32::MAX),
                }
            }
            45 => {
                let succeeded = services.seek_file(
                    cpu.register(0) as i32,
                    cpu.register(1) as i32,
                    cpu.register(2),
                )?;
                cpu.set_register(0, if succeeded { 0 } else { u32::MAX });
            }
            46 => {
                let name = self.read_c_string(GuestAddr(cpu.register(0)), 1024)?;
                let result = services.file_len(&name)?;
                cpu.set_register(
                    0,
                    result
                        .and_then(|len| u32::try_from(len).ok())
                        .unwrap_or(u32::MAX),
                );
            }
            47 => {
                let name = self.read_c_string(GuestAddr(cpu.register(0)), 1024)?;
                cpu.set_register(0, services.remove_file(&name)? as u32);
            }
            48 => {
                let source = self.read_c_string(GuestAddr(cpu.register(0)), 1024)?;
                let destination = self.read_c_string(GuestAddr(cpu.register(1)), 1024)?;
                cpu.set_register(0, services.rename_file(&source, &destination)? as u32);
            }
            49 | 50 => {
                let name = self.read_c_string(GuestAddr(cpu.register(0)), 1024)?;
                let result = if slot == 49 {
                    services.create_dir(&name)?
                } else {
                    services.remove_dir(&name)?
                };
                cpu.set_register(0, result as u32);
            }
            51 => {
                let directory = self.read_c_string(GuestAddr(cpu.register(0)), 1024)?;
                let output = GuestAddr(cpu.register(1));
                let output_len = cpu.register(2) as usize;
                match services.find_start(&directory)? {
                    Some((handle, entry))
                        if self.write_directory_entry(output, output_len, &entry)? =>
                    {
                        cpu.set_register(0, handle as u32);
                    }
                    Some((handle, _)) => {
                        services.find_stop(handle)?;
                        cpu.set_register(0, u32::MAX);
                    }
                    None => cpu.set_register(0, u32::MAX),
                }
            }
            52 => {
                let handle = cpu.register(0) as i32;
                let output = GuestAddr(cpu.register(1));
                let output_len = cpu.register(2) as usize;
                let succeeded = match services.find_next(handle)? {
                    Some(entry) => self.write_directory_entry(output, output_len, &entry)?,
                    None => false,
                };
                cpu.set_register(0, if succeeded { 0 } else { u32::MAX });
            }
            53 => {
                let succeeded = services.find_stop(cpu.register(0) as i32)?;
                cpu.set_register(0, if succeeded { 0 } else { u32::MAX });
            }
            54 => {
                self.exit_requested = true;
                cpu.set_register(0, 0);
            }
            58 => {
                // The headless profile uses an explicit no-output audio sink.
                // Stopping an absent or completed sound remains idempotent.
                cpu.set_register(0, 0);
            }
            61 => {
                // The offline profile still exposes a deterministic default
                // network identity; connectivity is reported by socket calls.
                cpu.set_register(0, 0);
            }
            69 => {
                let title = self.read_wide_string_be(GuestAddr(cpu.register(0)), 1024)?;
                let message = self.read_wide_string_be(GuestAddr(cpu.register(1)), 16 * 1024)?;
                let style = cpu.register(2);
                let handle = self.create_platform_dialog(&title, &message, style, services)?;
                cpu.set_register(0, handle);
            }
            70 => {
                let handle = cpu.register(0);
                let Some(dialog) = self.dialogs.remove(&handle) else {
                    cpu.set_register(0, u32::MAX);
                    return Ok(());
                };
                self.memory.write(SCREEN_BASE, &dialog.previous_screen)?;
                self.present_screen(services)?;
                cpu.set_register(0, 0);
            }
            71 => {
                let handle = cpu.register(0);
                let Some(dialog) = self.dialogs.get(&handle) else {
                    cpu.set_register(0, u32::MAX);
                    return Ok(());
                };
                let screen = dialog.dialog_screen.clone();
                self.memory.write(SCREEN_BASE, &screen)?;
                self.present_screen(services)?;
                cpu.set_register(0, 0);
            }
            80 => {
                let info = GuestAddr(cpu.register(0));
                let width = self.memory.read_u32(data_slot_address(92))?;
                let height = self.memory.read_u32(data_slot_address(93))?;
                self.memory.write_u32(info, width)?;
                self.memory.write_u32(info.checked_add(4)?, height)?;
                cpu.set_register(0, 0);
            }
            81 => {
                // Initializing the network service does not imply that a link is
                // available. Later DNS/socket operations report connectivity.
                cpu.set_register(0, 0);
            }
            82 => {
                // Closing an unavailable or already closed network service is
                // intentionally idempotent.
                cpu.set_register(0, 0);
            }
            84 => {
                // No socket provider is configured in the deterministic offline profile.
                cpu.set_register(0, u32::MAX);
            }
            85 => {
                cpu.set_register(0, u32::MAX);
            }
            86 => {
                cpu.set_register(0, u32::MAX);
            }
            87 => {
                cpu.set_register(0, u32::MAX);
            }
            89 => {
                let len = cpu.register(2) as usize;
                self.memory.read(GuestAddr(cpu.register(1)), len)?;
                cpu.set_register(0, u32::MAX);
            }
            113 => {
                self.md5_init(GuestAddr(cpu.register(0)))?;
                cpu.set_register(0, 0);
            }
            114 => {
                let context = GuestAddr(cpu.register(0));
                let input = self
                    .memory
                    .read(GuestAddr(cpu.register(1)), cpu.register(2) as usize)?;
                self.md5_append(context, &input)?;
                cpu.set_register(0, 0);
            }
            115 => {
                self.md5_finish(GuestAddr(cpu.register(0)), GuestAddr(cpu.register(1)))?;
                cpu.set_register(0, 0);
            }
            119 => {
                let (width, height) = self.screen_dimensions()?;
                self.write_screen_pixel(
                    cpu.register(0) as i32,
                    cpu.register(1) as i32,
                    cpu.register(2) as u16,
                    width,
                    height,
                )?;
                cpu.set_register(0, 0);
            }
            120 => {
                let source = GuestAddr(cpu.register(0));
                let x = cpu.register(1) as i32;
                let y = cpu.register(2) as i32;
                let width = cpu.register(3) as usize;
                let stack = GuestAddr(cpu.register(13));
                let height = self.memory.read_u32(stack)? as usize;
                let mode = self.memory.read_u32(stack.checked_add(4)?)?;
                let transparent_color = self.memory.read_u32(stack.checked_add(8)?)? as u16;
                let source_x = self.memory.read_u32(stack.checked_add(12)?)? as usize;
                let source_y = self.memory.read_u32(stack.checked_add(16)?)? as usize;
                let source_stride = self.memory.read_u32(stack.checked_add(20)?)? as usize;
                let transparent_color = match mode {
                    2 => None,
                    6 => Some(transparent_color),
                    _ => {
                        return Err(Error::Abi(format!(
                            "unsupported bitmap drawing mode {mode} called by module {module}"
                        )));
                    }
                };
                let source_end_x = source_x
                    .checked_add(width)
                    .ok_or_else(|| Error::Abi("bitmap source width overflow".into()))?;
                if source_end_x > source_stride {
                    return Err(Error::Abi(format!(
                        "bitmap source region ends at {source_end_x}, beyond stride {source_stride}"
                    )));
                }
                let source_end_y = source_y
                    .checked_add(height)
                    .ok_or_else(|| Error::Abi("bitmap source height overflow".into()))?;
                let byte_len = width
                    .checked_mul(height)
                    .and_then(|pixels| pixels.checked_mul(2))
                    .ok_or_else(|| Error::Abi("bitmap source byte count overflow".into()))?;
                if byte_len > self.heap_len {
                    return Err(Error::Abi(format!(
                        "bitmap source region requires {byte_len} bytes"
                    )));
                }
                let pixels = if source_x == 0 && width == source_stride {
                    let byte_offset = source_y
                        .checked_mul(source_stride)
                        .and_then(|offset| offset.checked_mul(2))
                        .and_then(|offset| u32::try_from(offset).ok())
                        .ok_or_else(|| Error::Abi("bitmap source offset overflow".into()))?;
                    self.memory
                        .read(source.checked_add(byte_offset)?, byte_len)?
                } else {
                    let row_len = width
                        .checked_mul(2)
                        .ok_or_else(|| Error::Abi("bitmap source row overflow".into()))?;
                    let mut pixels = Vec::with_capacity(byte_len);
                    for row in source_y..source_end_y {
                        let byte_offset = row
                            .checked_mul(source_stride)
                            .and_then(|offset| offset.checked_add(source_x))
                            .and_then(|offset| offset.checked_mul(2))
                            .and_then(|offset| u32::try_from(offset).ok())
                            .ok_or_else(|| Error::Abi("bitmap source offset overflow".into()))?;
                        pixels.extend_from_slice(
                            &self
                                .memory
                                .read(source.checked_add(byte_offset)?, row_len)?,
                        );
                    }
                    pixels
                };
                self.draw_bitmap_region_to_screen(&pixels, x, y, width, height, transparent_color)?;
                cpu.set_register(0, 0);
            }
            121 => {
                let source = self.read_bitmap_descriptor(GuestAddr(cpu.register(0)))?;
                let destination = self.read_bitmap_descriptor(GuestAddr(cpu.register(1)))?;
                let stack = GuestAddr(cpu.register(13));
                let transform_address = GuestAddr(self.memory.read_u32(stack)?);
                let transform = self.read_bitmap_transform(transform_address)?;
                let transparent_color = self.memory.read_u32(stack.checked_add(4)?)? as u16;
                self.copy_transformed_bitmap(
                    destination,
                    source,
                    cpu.register(2) as usize,
                    cpu.register(3) as usize,
                    transform,
                    transparent_color,
                    module,
                )?;
                cpu.set_register(0, 0);
            }
            122 => {
                let stack = GuestAddr(cpu.register(13));
                let color = Framebuffer::rgb565(
                    self.memory.read_u32(stack)? as i32,
                    self.memory.read_u32(stack.checked_add(4)?)? as i32,
                    self.memory.read_u32(stack.checked_add(8)?)? as i32,
                );
                let x = cpu.register(0) as i32;
                let y = cpu.register(1) as i32;
                let width = cpu.register(2) as i32;
                let height = cpu.register(3) as i32;
                self.draw_rectangle_to_screen(x, y, width, height, color)?;
                cpu.set_register(0, 0);
            }
            123 => {
                let stack = GuestAddr(cpu.register(13));
                let flags = self.memory.read_u32(stack.checked_add(12)?)?;
                if flags > 2 {
                    return Err(Error::Abi(format!(
                        "unsupported text drawing flags {flags} called by module {module}"
                    )));
                }
                let text = self.read_wide_string_be(GuestAddr(cpu.register(0)), 64 * 1024)?;
                let color = Framebuffer::rgb565(
                    cpu.register(3) as i32,
                    self.memory.read_u32(stack)? as i32,
                    self.memory.read_u32(stack.checked_add(4)?)? as i32,
                );
                self.draw_text_to_screen(
                    &text,
                    cpu.register(1) as i32,
                    cpu.register(2) as i32,
                    color,
                    self.memory.read_u32(stack.checked_add(8)?)?,
                    services,
                )?;
                cpu.set_register(0, 0);
            }
            125 => {
                let name = self.read_c_string(GuestAddr(cpu.register(0)), 1024)?;
                let ram_address = self.memory.read_u32(data_slot_address(104))?;
                let ram_len = self.memory.read_u32(data_slot_address(105))? as usize;
                let bytes = if ram_address == 0 && ram_len == 0 {
                    let package_name = self.read_c_string(PACKAGE_NAME_DATA, 256)?;
                    services.read_package_file(&package_name, &name)?
                } else {
                    if ram_address == 0 || ram_len == 0 {
                        return Err(Error::Abi(format!(
                            "RAM-backed MRP has inconsistent address {ram_address:#010x} and length {ram_len}"
                        )));
                    }
                    self.read_ram_package_file(GuestAddr(ram_address), ram_len, &name)?
                };
                if std::env::var_os("SKYENGINE_TRACE_ARM").is_some() {
                    eprintln!(
                        "[arm-package] name={:?} ram={ram_address:#010x}+{ram_len:#x} result_len={:?}",
                        String::from_utf8_lossy(&name),
                        bytes.as_ref().map(Vec::len),
                    );
                }
                let Some(bytes) = bytes else {
                    let len_pointer = GuestAddr(cpu.register(1));
                    if len_pointer.0 != 0 {
                        self.memory.write_u32(len_pointer, 0)?;
                    }
                    cpu.set_register(0, 0);
                    return Ok(());
                };
                let prepared_output = if ram_address == 0 {
                    None
                } else {
                    self.compact_ram_output_target(GuestAddr(ram_address), ram_len, bytes.len())?
                };
                let output = match prepared_output {
                    Some(output) => output,
                    None => self.allocate(bytes.len(), 8)?,
                };
                self.memory.write(output, &bytes)?;
                let len_pointer = GuestAddr(cpu.register(1));
                if len_pointer.0 != 0 {
                    self.memory.write_u32(len_pointer, bytes.len() as u32)?;
                }
                cpu.set_register(0, output.0);
            }
            130 => match (cpu.register(0), cpu.register(1), cpu.register(2)) {
                // Baseline SDK compatibility probe, equivalent to the MR TestCom stub.
                (0, 7, 9_999) => cpu.set_register(0, 0),
                (command, argument, fallback) => {
                    return Err(Error::Abi(format!(
                        "unsupported platform slot 130 command ({command}, {argument}, {fallback}) called by module {module}"
                    )));
                }
            },
            131 => match (
                cpu.register(0),
                cpu.register(1),
                cpu.register(2),
                cpu.register(3),
            ) {
                // Marks a dynamically loaded native module as executable.
                (0, 9, address, len) if len != 0 => {
                    let address = GuestAddr(address);
                    let image = self.memory.read(address, len as usize)?;
                    if std::env::var_os("SKYENGINE_TRACE_ARM").is_some() {
                        eprintln!(
                            "[arm-executable] address={:#010x} len={len:#x} head={:02x?}",
                            address.0,
                            &image[..image.len().min(64)]
                        );
                    }
                    self.memory
                        .add_permissions(address, len as usize, Permissions::EXECUTE)?;
                    cpu.set_register(0, 0);
                }
                (command, argument, address, len) => {
                    return Err(Error::Abi(format!(
                        "unsupported platform slot 131 command ({command}, {argument}, {address:#010x}, {len}) called by module {module}"
                    )));
                }
            },
            other => {
                let return_address = cpu.register(14) & !1;
                let caller_start = return_address.saturating_sub(24);
                let caller_bytes = self
                    .memory
                    .read(GuestAddr(caller_start), 48)
                    .map(|bytes| format!("{bytes:02x?}"))
                    .unwrap_or_else(|error| format!("unavailable: {error}"));
                let stack_words = (0..6)
                    .map(|index| {
                        self.memory
                            .read_u32(GuestAddr(cpu.register(13).wrapping_add(index * 4)))
                    })
                    .collect::<Result<Vec<_>>>()
                    .map(|words| format!("{words:08x?}"))
                    .unwrap_or_else(|error| format!("unavailable: {error}"));
                let argument_bytes = self
                    .memory
                    .read(GuestAddr(cpu.register(0)), 32)
                    .map(|bytes| format!("{bytes:02x?}"))
                    .unwrap_or_else(|error| format!("unavailable: {error}"));
                let second_argument_bytes = self
                    .memory
                    .read(GuestAddr(cpu.register(1)), 32)
                    .map(|bytes| format!("{bytes:02x?}"))
                    .unwrap_or_else(|error| format!("unavailable: {error}"));
                let stack_record_bytes = self
                    .memory
                    .read_u32(GuestAddr(cpu.register(13)))
                    .and_then(|address| self.memory.read(GuestAddr(address), 32))
                    .map(|bytes| format!("{bytes:02x?}"))
                    .unwrap_or_else(|error| format!("unavailable: {error}"));
                return Err(Error::Abi(format!(
                    "unsupported platform slot {other} called by module {module} at LR {:#010x} (r0={:#010x}, r1={:#010x}, r2={:#010x}, r3={:#010x}, sp={:#010x}, stack={stack_words}, r0-bytes={argument_bytes}, r1-bytes={second_argument_bytes}, stack-record={stack_record_bytes}); guest bytes at {caller_start:#010x}: {caller_bytes}",
                    cpu.register(14),
                    cpu.register(0),
                    cpu.register(1),
                    cpu.register(2),
                    cpu.register(3),
                    cpu.register(13),
                )));
            }
        }
        Ok(())
    }

    fn md5_init(&mut self, context: GuestAddr) -> Result<()> {
        self.memory.write_u32(context, 0)?;
        self.memory.write_u32(context.checked_add(4)?, 0)?;
        for (index, value) in [0x6745_2301, 0xefcd_ab89, 0x98ba_dcfe, 0x1032_5476]
            .into_iter()
            .enumerate()
        {
            self.memory
                .write_u32(context.checked_add(8 + index as u32 * 4)?, value)?;
        }
        self.memory
            .write(context.checked_add(MD5_BUFFER_OFFSET)?, &[0; 64])
    }

    fn md5_append(&mut self, context: GuestAddr, input: &[u8]) -> Result<()> {
        let total = u64::from(self.memory.read_u32(context)?)
            | (u64::from(self.memory.read_u32(context.checked_add(4)?)?) << 32);
        let next_total = total
            .checked_add(input.len() as u64)
            .ok_or_else(|| Error::Abi("MD5 byte count overflow".into()))?;
        let mut state = [0_u32; 4];
        for (index, value) in state.iter_mut().enumerate() {
            *value = self
                .memory
                .read_u32(context.checked_add(8 + index as u32 * 4)?)?;
        }
        let mut buffer: [u8; 64] = self
            .memory
            .read(context.checked_add(MD5_BUFFER_OFFSET)?, 64)?
            .try_into()
            .expect("checked MD5 buffer length");
        md5_consume(&mut state, &mut buffer, (total % 64) as usize, input);

        self.memory.write_u32(context, next_total as u32)?;
        self.memory
            .write_u32(context.checked_add(4)?, (next_total >> 32) as u32)?;
        for (index, value) in state.into_iter().enumerate() {
            self.memory
                .write_u32(context.checked_add(8 + index as u32 * 4)?, value)?;
        }
        self.memory
            .write(context.checked_add(MD5_BUFFER_OFFSET)?, &buffer)
    }

    fn md5_finish(&mut self, context: GuestAddr, output: GuestAddr) -> Result<()> {
        let total = u64::from(self.memory.read_u32(context)?)
            | (u64::from(self.memory.read_u32(context.checked_add(4)?)?) << 32);
        let mut state = [0_u32; 4];
        for (index, value) in state.iter_mut().enumerate() {
            *value = self
                .memory
                .read_u32(context.checked_add(8 + index as u32 * 4)?)?;
        }
        let mut buffer: [u8; 64] = self
            .memory
            .read(context.checked_add(MD5_BUFFER_OFFSET)?, 64)?
            .try_into()
            .expect("checked MD5 buffer length");
        let buffered = (total % 64) as usize;
        let padding_len = if buffered < 56 {
            56 - buffered
        } else {
            120 - buffered
        };
        let mut padding = vec![0; padding_len + 8];
        padding[0] = 0x80;
        padding[padding_len..].copy_from_slice(&total.wrapping_mul(8).to_le_bytes());
        let remaining = md5_consume(&mut state, &mut buffer, buffered, &padding);
        debug_assert_eq!(remaining, 0);

        let mut digest = [0_u8; 16];
        for (chunk, value) in digest.chunks_exact_mut(4).zip(state) {
            chunk.copy_from_slice(&value.to_le_bytes());
        }
        self.memory.write(output, &digest)
    }

    fn return_unavailable_platform_extension(&mut self, cpu: &mut ArmCpu) -> Result<()> {
        let output = GuestAddr(cpu.register(3));
        if output.0 != 0 {
            self.memory.write_u32(output, 0)?;
        }
        let output_len = GuestAddr(self.memory.read_u32(GuestAddr(cpu.register(13)))?);
        if output_len.0 != 0 {
            self.memory.write_u32(output_len, 0)?;
        }
        cpu.set_register(0, u32::MAX);
        Ok(())
    }

    fn return_platform_sim_info(&mut self, cpu: &mut ArmCpu) -> Result<()> {
        let output = GuestAddr(cpu.register(3));
        if output.0 == 0 {
            return Err(Error::Abi(
                "platform SIM query has a null output pointer".into(),
            ));
        }
        let output_len = GuestAddr(self.memory.read_u32(GuestAddr(cpu.register(13)))?);
        if output_len.0 == 0 {
            return Err(Error::Abi(
                "platform SIM query has a null output-length pointer".into(),
            ));
        }
        self.memory.write_u32(output, PLATFORM_SIM_INFO_DATA.0)?;
        self.memory
            .write_u32(output_len, PLATFORM_SIM_INFO_LEN as u32)?;
        cpu.set_register(0, 0);
        Ok(())
    }

    fn return_platform_storage_info(&mut self, cpu: &mut ArmCpu) -> Result<()> {
        self.memory.read(GuestAddr(cpu.register(1)), 1)?;
        let output = GuestAddr(cpu.register(3));
        if output.0 == 0 {
            return Err(Error::Abi(
                "platform storage query has a null output pointer".into(),
            ));
        }
        let output_len = GuestAddr(self.memory.read_u32(GuestAddr(cpu.register(13)))?);
        if output_len.0 == 0 {
            return Err(Error::Abi(
                "platform storage query has a null output-length pointer".into(),
            ));
        }
        self.memory
            .write_u32(output, PLATFORM_STORAGE_INFO_DATA.0)?;
        self.memory
            .write_u32(output_len, PLATFORM_STORAGE_INFO_LEN as u32)?;
        cpu.set_register(0, 0);
        Ok(())
    }

    fn return_platform_storage_drive(&mut self, cpu: &mut ArmCpu) -> Result<()> {
        let input_len = cpu.register(2) as usize;
        let input = self.memory.read(GuestAddr(cpu.register(1)), input_len)?;
        if input != b"Y" {
            cpu.set_register(0, u32::MAX);
            return Ok(());
        }
        let output = GuestAddr(cpu.register(3));
        if output.0 == 0 {
            return Err(Error::Abi(
                "platform storage drive query has a null output pointer".into(),
            ));
        }
        let output_len = GuestAddr(self.memory.read_u32(GuestAddr(cpu.register(13)))?);
        if output_len.0 == 0 {
            return Err(Error::Abi(
                "platform storage drive query has a null output-length pointer".into(),
            ));
        }
        self.memory
            .write_u32(output, PLATFORM_STORAGE_DRIVE_DATA.0)?;
        self.memory
            .write_u32(output_len, PLATFORM_STORAGE_DRIVE_LEN as u32)?;
        cpu.set_register(0, 0);
        Ok(())
    }

    fn allocate_platform_memory_extension(&mut self, cpu: &mut ArmCpu) -> Result<()> {
        let requested_len = cpu.register(2) as usize;
        if requested_len == 0 {
            return Err(Error::Abi(
                "platform memory extension requested zero bytes".into(),
            ));
        }
        let output = GuestAddr(cpu.register(3));
        if output.0 == 0 {
            return Err(Error::Abi(
                "platform memory extension has a null output pointer".into(),
            ));
        }
        let output_len = GuestAddr(self.memory.read_u32(GuestAddr(cpu.register(13)))?);
        if output_len.0 == 0 {
            return Err(Error::Abi(
                "platform memory extension has a null output-length pointer".into(),
            ));
        }

        let previous_heap_cursor = self.heap_cursor;
        let arena = self.allocate(requested_len, 8)?;
        self.memory.write(arena, &vec![0; requested_len])?;
        self.platform_memory_extensions
            .insert(arena.0, (requested_len, previous_heap_cursor));
        self.memory.write_u32(output, arena.0)?;
        self.memory.write_u32(output_len, cpu.register(2))?;
        cpu.set_register(0, 0);
        Ok(())
    }

    fn release_platform_memory_extension(&mut self, cpu: &mut ArmCpu) -> Result<()> {
        let arena = GuestAddr(cpu.register(1));
        if cpu.register(2) != 4 {
            return Err(Error::Abi(format!(
                "platform memory extension release input is {} bytes, expected 4",
                cpu.register(2)
            )));
        }
        let (len, previous_heap_cursor) = self
            .platform_memory_extensions
            .remove(&arena.0)
            .ok_or_else(|| {
                Error::Abi(format!(
                    "platform memory extension release references unknown arena {:#010x}",
                    arena.0
                ))
            })?;
        self.memory.write(arena, &vec![0; len])?;

        let end = arena
            .0
            .checked_add(u32::try_from(len).map_err(|_| {
                Error::Abi(format!(
                    "platform memory extension length {len} exceeds u32"
                ))
            })?)
            .ok_or_else(|| Error::Abi("platform memory extension end overflow".into()))?;
        if end == self.heap_cursor {
            self.heap_cursor = previous_heap_cursor;
            let heap_end = HEAP_BASE.0 + self.heap_len as u32;
            self.memory
                .write_u32(data_slot_address(111), heap_end - self.heap_cursor)?;
        }
        cpu.set_register(0, 0);
        Ok(())
    }

    fn create_platform_dialog(
        &mut self,
        title: &[u16],
        message: &[u16],
        style: u32,
        services: &mut dyn NativeServices,
    ) -> Result<u32> {
        if style != 0 {
            return Err(Error::Abi(format!(
                "unsupported platform dialog style {style}"
            )));
        }
        let (width, height) = self.screen_dimensions()?;
        let screen_len = usize::try_from(width)
            .ok()
            .and_then(|width| {
                usize::try_from(height)
                    .ok()
                    .and_then(|height| width.checked_mul(height))
            })
            .and_then(|pixels| pixels.checked_mul(2))
            .ok_or_else(|| Error::Abi("platform dialog screen size overflow".into()))?;
        let previous_screen = self.memory.read(SCREEN_BASE, screen_len)?;

        let background = Framebuffer::rgb565(248, 252, 248);
        let accent = Framebuffer::rgb565(32, 160, 224);
        let accent_dark = Framebuffer::rgb565(0, 96, 176);
        let black = Framebuffer::rgb565(0, 0, 0);
        let white = Framebuffer::rgb565(255, 255, 255);
        self.draw_rectangle_to_screen(0, 0, width, height, background)?;
        self.draw_rectangle_to_screen(0, 0, width, 30, accent)?;
        self.draw_text_to_screen(title, 8, 7, white, 0, services)?;
        self.draw_wrapped_text_to_screen(message, 12, 48, width - 24, black, services)?;

        let button_width = 120.min(width.saturating_sub(24));
        let button_x = (width - button_width) / 2;
        let button_y = height.saturating_sub(68);
        self.draw_rectangle_to_screen(
            button_x - 1,
            button_y - 1,
            button_width + 2,
            32,
            accent_dark,
        )?;
        self.draw_rectangle_to_screen(button_x, button_y, button_width, 30, accent)?;
        self.draw_text_to_screen(
            &[0x786e, 0x5b9a],
            button_x + button_width / 2 - 16,
            button_y + 7,
            white,
            0,
            services,
        )?;

        let dialog_screen = self.memory.read(SCREEN_BASE, screen_len)?;
        let handle = self.allocate_ui_handle()?;
        self.dialogs.insert(
            handle,
            PlatformDialog {
                previous_screen,
                dialog_screen,
            },
        );
        self.present_screen(services)?;
        Ok(handle)
    }

    fn draw_wrapped_text_to_screen(
        &mut self,
        text: &[u16],
        x: i32,
        mut y: i32,
        max_width: i32,
        color: u16,
        services: &mut dyn NativeServices,
    ) -> Result<()> {
        let mut line = Vec::new();
        let mut line_width = 0;
        for &codepoint in text {
            let glyph_width = if codepoint < 128 { 8 } else { 16 };
            if codepoint == b'\n' as u16
                || (!line.is_empty() && line_width + glyph_width > max_width)
            {
                self.draw_text_to_screen(&line, x, y, color, 0, services)?;
                line.clear();
                line_width = 0;
                y += 22;
                if codepoint == b'\n' as u16 {
                    continue;
                }
            }
            line.push(codepoint);
            line_width += glyph_width;
        }
        if !line.is_empty() {
            self.draw_text_to_screen(&line, x, y, color, 0, services)?;
        }
        Ok(())
    }

    fn allocate_ui_handle(&mut self) -> Result<u32> {
        let start = self.next_ui_handle;
        loop {
            let handle = self.next_ui_handle;
            self.next_ui_handle = self.next_ui_handle.checked_add(1).unwrap_or(1);
            if handle != 0 && !self.dialogs.contains_key(&handle) {
                return Ok(handle);
            }
            if self.next_ui_handle == start {
                return Err(Error::ResourceLimit(
                    "no platform UI handles available".into(),
                ));
            }
        }
    }

    fn present_screen(&self, services: &mut dyn NativeServices) -> Result<()> {
        let (width, height) = self.screen_dimensions()?;
        let byte_len = usize::try_from(width)
            .ok()
            .and_then(|width| {
                usize::try_from(height)
                    .ok()
                    .and_then(|height| width.checked_mul(height))
            })
            .and_then(|pixels| pixels.checked_mul(2))
            .ok_or_else(|| Error::Abi("screen presentation size overflow".into()))?;
        let pixels = self.memory.read(SCREEN_BASE, byte_len)?;
        services.draw_bitmap(&pixels, 0, 0, width as usize, height as usize)
    }

    fn read_platform_draw_pixels(
        &self,
        source: GuestAddr,
        x: i32,
        y: i32,
        width: usize,
        height: usize,
    ) -> Result<Vec<u8>> {
        let byte_len = width
            .checked_mul(height)
            .and_then(|pixels| pixels.checked_mul(2))
            .ok_or_else(|| Error::Abi("mr_drawBitmap dimensions overflow".into()))?;
        if byte_len > self.heap_len {
            return Err(Error::Abi(format!(
                "mr_drawBitmap source is {byte_len} bytes"
            )));
        }
        if source != SCREEN_BASE {
            return self.memory.read(source, byte_len);
        }

        let (screen_width, screen_height) = self.screen_dimensions()?;
        let region_width = i64::try_from(width)
            .map_err(|_| Error::Abi("mr_drawBitmap width exceeds i64".into()))?;
        let region_height = i64::try_from(height)
            .map_err(|_| Error::Abi("mr_drawBitmap height exceeds i64".into()))?;
        let region_end_x = i64::from(x) + region_width;
        let region_end_y = i64::from(y) + region_height;
        if x < 0
            || y < 0
            || region_end_x > i64::from(screen_width)
            || region_end_y > i64::from(screen_height)
        {
            return Err(Error::Abi(format!(
                "mr_drawBitmap screen region ({x}, {y}) {width}x{height} exceeds {screen_width}x{screen_height}"
            )));
        }

        let row_byte_len = width
            .checked_mul(2)
            .ok_or_else(|| Error::Abi("mr_drawBitmap row size overflow".into()))?;
        let mut pixels = Vec::with_capacity(byte_len);
        for row in 0..height {
            let row = i32::try_from(row)
                .map_err(|_| Error::Abi("mr_drawBitmap row exceeds i32".into()))?;
            let row_address = self.screen_address(x, y + row, screen_width)?;
            pixels.extend(self.memory.read(row_address, row_byte_len)?);
        }
        Ok(pixels)
    }

    fn compact_ram_output_target(
        &self,
        package_address: GuestAddr,
        package_len: usize,
        output_len: usize,
    ) -> Result<Option<GuestAddr>> {
        if package_len < 24 {
            return Ok(None);
        }
        let header = self.memory.read(package_address, 24)?;
        if &header[..4] != b"MRPG"
            || read_le_u32(&header, 4)? != 4
            || read_le_u32(&header, 12)? != 4
        {
            return Ok(None);
        }

        let output_len = u32::try_from(output_len)
            .map_err(|_| Error::Abi("compact RAM MRP output length exceeds u32".into()))?;
        let aligned_len = output_len
            .checked_add(7)
            .map(|len| len & !7)
            .ok_or_else(|| Error::Abi("compact RAM MRP output alignment overflow".into()))?;
        let heap_end = HEAP_BASE.0 + self.heap_len as u32;
        let mut candidates = Vec::new();
        for descriptor_len_address in (HEAP_BASE.0 + 4..heap_end).step_by(4) {
            let recorded_len = self.memory.read_u32(GuestAddr(descriptor_len_address))?;
            if recorded_len != aligned_len {
                continue;
            }
            let candidate = self
                .memory
                .read_u32(GuestAddr(descriptor_len_address - 4))?;
            let Some(candidate_end) = candidate.checked_add(output_len) else {
                continue;
            };
            if candidate & 3 != 0 || candidate < HEAP_BASE.0 || candidate_end > heap_end {
                continue;
            }
            let candidate = GuestAddr(candidate);
            if self.memory.read_u32(candidate)? == 0
                && self.memory.read_u32(candidate.checked_add(4)?)? == aligned_len
            {
                candidates.push(candidate);
            }
        }
        candidates.sort_unstable();
        candidates.dedup();
        match candidates.as_slice() {
            [] => Ok(None),
            [candidate] => Ok(Some(*candidate)),
            _ => Err(Error::Abi(format!(
                "compact RAM MRP output has ambiguous prepared buffers: {candidates:?}"
            ))),
        }
    }

    fn draw_bitmap_region_to_screen(
        &mut self,
        pixels: &[u8],
        x: i32,
        y: i32,
        width: usize,
        height: usize,
        transparent_color: Option<u16>,
    ) -> Result<()> {
        let (screen_width, screen_height) = self.screen_dimensions()?;
        let destination_x0 = i64::from(x).max(0);
        let destination_y0 = i64::from(y).max(0);
        let destination_x1 = (i64::from(x) + width as i64).min(i64::from(screen_width));
        let destination_y1 = (i64::from(y) + height as i64).min(i64::from(screen_height));
        if destination_x0 >= destination_x1 || destination_y0 >= destination_y1 {
            return Ok(());
        }

        let visible_width = usize::try_from(destination_x1 - destination_x0)
            .map_err(|_| Error::Abi("visible bitmap width exceeds usize".into()))?;
        let source_x = usize::try_from(destination_x0 - i64::from(x))
            .map_err(|_| Error::Abi("visible bitmap source x exceeds usize".into()))?;
        let source_y = usize::try_from(destination_y0 - i64::from(y))
            .map_err(|_| Error::Abi("visible bitmap source y exceeds usize".into()))?;
        let row_byte_len = visible_width
            .checked_mul(2)
            .ok_or_else(|| Error::Abi("visible bitmap row byte count overflow".into()))?;

        for visible_row in 0..usize::try_from(destination_y1 - destination_y0)
            .map_err(|_| Error::Abi("visible bitmap height exceeds usize".into()))?
        {
            let source_offset = (source_y + visible_row)
                .checked_mul(width)
                .and_then(|offset| offset.checked_add(source_x))
                .and_then(|offset| offset.checked_mul(2))
                .ok_or_else(|| Error::Abi("visible bitmap source offset overflow".into()))?;
            let source_row = &pixels[source_offset..source_offset + row_byte_len];
            let destination_address = self.screen_address(
                destination_x0 as i32,
                destination_y0 as i32 + visible_row as i32,
                screen_width,
            )?;
            if let Some(transparent_color) = transparent_color {
                let mut destination_row = self.memory.read(destination_address, row_byte_len)?;
                for (source, destination) in source_row
                    .chunks_exact(2)
                    .zip(destination_row.chunks_exact_mut(2))
                {
                    let color = u16::from_le_bytes([source[0], source[1]]);
                    if color != transparent_color {
                        destination.copy_from_slice(source);
                    }
                }
                self.memory.write(destination_address, &destination_row)?;
            } else {
                self.memory.write(destination_address, source_row)?;
            }
        }
        Ok(())
    }

    fn read_bitmap_descriptor(&self, address: GuestAddr) -> Result<BitmapDescriptor> {
        Ok(BitmapDescriptor {
            pixels: GuestAddr(self.memory.read_u32(address)?),
            width: usize::from(self.memory.read_u16(address.checked_add(4)?)?),
            height: usize::from(self.memory.read_u16(address.checked_add(6)?)?),
            x: i32::from(self.memory.read_u16(address.checked_add(8)?)? as i16),
            y: i32::from(self.memory.read_u16(address.checked_add(10)?)? as i16),
        })
    }

    fn read_bitmap_transform(&self, address: GuestAddr) -> Result<BitmapTransform> {
        let read_field = |offset| {
            self.memory
                .read_u16(address.checked_add(offset)?)
                .map(|value| value as i16)
        };
        Ok(BitmapTransform {
            a: read_field(0)?,
            b: read_field(2)?,
            c: read_field(4)?,
            d: read_field(6)?,
            mode: read_field(8)?,
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn copy_transformed_bitmap(
        &mut self,
        destination: BitmapDescriptor,
        source: BitmapDescriptor,
        width: usize,
        height: usize,
        transform: BitmapTransform,
        transparent_color: u16,
        module: usize,
    ) -> Result<()> {
        let transparent_color = match transform.mode {
            2 => None,
            6 => Some(transparent_color),
            mode => {
                return Err(Error::Abi(format!(
                    "unsupported transformed bitmap mode {mode} called by module {module}"
                )));
            }
        };
        if width == 0 || height == 0 {
            return Ok(());
        }

        let source_x = usize::try_from(source.x).map_err(|_| {
            Error::Abi(format!("negative transformed bitmap source x {}", source.x))
        })?;
        let source_y = usize::try_from(source.y).map_err(|_| {
            Error::Abi(format!("negative transformed bitmap source y {}", source.y))
        })?;
        let source_end_x = source_x
            .checked_add(width)
            .ok_or_else(|| Error::Abi("transformed bitmap source width overflow".into()))?;
        let source_end_y = source_y
            .checked_add(height)
            .ok_or_else(|| Error::Abi("transformed bitmap source height overflow".into()))?;
        if source_end_x > source.width || source_end_y > source.height {
            return Err(Error::Abi(format!(
                "transformed bitmap source region ({source_x}, {source_y}) {width}x{height} exceeds {}x{} bitmap",
                source.width, source.height
            )));
        }
        let pixel_count = width
            .checked_mul(height)
            .ok_or_else(|| Error::Abi("transformed bitmap region dimensions overflow".into()))?;
        if pixel_count > self.heap_len / 2 {
            return Err(Error::Abi(format!(
                "transformed bitmap region requires {pixel_count} pixels"
            )));
        }

        // Source and destination can refer to the same bitmap. Capture the
        // complete source region before changing any destination pixel.
        let mut pixels = Vec::with_capacity(pixel_count);
        for row in 0..height {
            for column in 0..width {
                let address = bitmap_pixel_address(
                    source.pixels,
                    source.width,
                    source_x + column,
                    source_y + row,
                )?;
                pixels.push(self.memory.read_u16(address)?);
            }
        }

        let last_x = i64::try_from(width - 1)
            .map_err(|_| Error::Abi("transformed bitmap width exceeds i64".into()))?;
        let last_y = i64::try_from(height - 1)
            .map_err(|_| Error::Abi("transformed bitmap height exceeds i64".into()))?;
        let corners = [
            transform.apply(0, 0),
            transform.apply(last_x, 0),
            transform.apply(0, last_y),
            transform.apply(last_x, last_y),
        ];
        let minimum_x = corners
            .iter()
            .map(|(x, _)| *x)
            .min()
            .expect("four transform corners");
        let minimum_y = corners
            .iter()
            .map(|(_, y)| *y)
            .min()
            .expect("four transform corners");

        for row in 0..height {
            for column in 0..width {
                let color = pixels[row * width + column];
                if Some(color) == transparent_color {
                    continue;
                }
                let (transformed_x, transformed_y) = transform.apply(column as i64, row as i64);
                let destination_x = i64::from(destination.x) + transformed_x - minimum_x;
                let destination_y = i64::from(destination.y) + transformed_y - minimum_y;
                if destination_x < 0
                    || destination_y < 0
                    || destination_x >= destination.width as i64
                    || destination_y >= destination.height as i64
                {
                    continue;
                }
                let address = bitmap_pixel_address(
                    destination.pixels,
                    destination.width,
                    destination_x as usize,
                    destination_y as usize,
                )?;
                self.memory.write_u16(address, color)?;
            }
        }
        Ok(())
    }

    fn draw_rectangle_to_screen(
        &mut self,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        color: u16,
    ) -> Result<()> {
        if width <= 0 || height <= 0 {
            return Ok(());
        }
        let (screen_width, screen_height) = self.screen_dimensions()?;
        let x0 = x.clamp(0, screen_width);
        let y0 = y.clamp(0, screen_height);
        let x1 = x.saturating_add(width).clamp(0, screen_width);
        let y1 = y.saturating_add(height).clamp(0, screen_height);
        if x0 >= x1 || y0 >= y1 {
            return Ok(());
        }
        let color = color.to_le_bytes();
        let mut row = Vec::with_capacity((x1 - x0) as usize * 2);
        for _ in x0..x1 {
            row.extend_from_slice(&color);
        }
        for screen_y in y0..y1 {
            let address = self.screen_address(x0, screen_y, screen_width)?;
            self.memory.write(address, &row)?;
        }
        Ok(())
    }

    fn draw_text_to_screen(
        &mut self,
        text: &[u16],
        mut x: i32,
        y: i32,
        color: u16,
        font: u32,
        services: &mut dyn NativeServices,
    ) -> Result<()> {
        let (screen_width, screen_height) = self.screen_dimensions()?;
        for &codepoint in text {
            let Some((glyph, width, height)) = services.char_bitmap(u32::from(codepoint), font)?
            else {
                x += if codepoint < 128 { 8 } else { 16 };
                continue;
            };
            let width = width.min(16) as i32;
            let height = height.min(16) as usize;
            let required = height
                .checked_mul(2)
                .ok_or_else(|| Error::Abi("character bitmap size overflow".into()))?;
            if glyph.len() < required {
                return Err(Error::Abi(format!(
                    "character bitmap for {codepoint:#06x} has {} bytes, needs {required}",
                    glyph.len()
                )));
            }
            for row in 0..height as i32 {
                let offset = row as usize * 2;
                let bits = u16::from_be_bytes([glyph[offset], glyph[offset + 1]]);
                for column in 0..width {
                    if bits & (0x8000_u16 >> column) != 0 {
                        self.write_screen_pixel(
                            x + column,
                            y + row,
                            color,
                            screen_width,
                            screen_height,
                        )?;
                    }
                }
            }
            x += width;
        }
        Ok(())
    }

    fn write_screen_pixel(
        &mut self,
        x: i32,
        y: i32,
        color: u16,
        width: i32,
        height: i32,
    ) -> Result<()> {
        if x < 0 || y < 0 || x >= width || y >= height {
            return Ok(());
        }
        let address = self.screen_address(x, y, width)?;
        self.memory.write_u16(address, color)
    }

    fn screen_dimensions(&self) -> Result<(i32, i32)> {
        let width = self.memory.read_u32(data_slot_address(92))?;
        let height = self.memory.read_u32(data_slot_address(93))?;
        Ok((
            i32::try_from(width)
                .map_err(|_| Error::Abi(format!("screen width {width} exceeds i32")))?,
            i32::try_from(height)
                .map_err(|_| Error::Abi(format!("screen height {height} exceeds i32")))?,
        ))
    }

    fn screen_address(&self, x: i32, y: i32, width: i32) -> Result<GuestAddr> {
        let offset = y
            .checked_mul(width)
            .and_then(|offset| offset.checked_add(x))
            .and_then(|offset| offset.checked_mul(2))
            .and_then(|offset| u32::try_from(offset).ok())
            .ok_or_else(|| Error::Abi("screen pixel offset overflow".into()))?;
        SCREEN_BASE.checked_add(offset)
    }

    fn read_ram_package_file(
        &self,
        address: GuestAddr,
        len: usize,
        name: &[u8],
    ) -> Result<Option<Vec<u8>>> {
        let image = self.memory.read(address, len)?;
        if image.len() < 24 || &image[..4] != b"MRPG" {
            return Err(Error::Package(
                "RAM-backed MRP is missing its 24-byte MRPG header".into(),
            ));
        }

        // Native wrappers use this compact one-file MRP while the current
        // package name is "$". The four-byte name precedes the stored length,
        // and the single payload follows the 24-byte header immediately.
        if read_le_u32(&image, 4)? == 4 && read_le_u32(&image, 12)? == 4 {
            let compact_name = image[16..20]
                .split(|byte| *byte == 0)
                .next()
                .unwrap_or_default();
            if compact_name.is_empty() || name != compact_name {
                return Ok(None);
            }
            let declared_len = read_le_u32(&image, 8)? as usize;
            let stored_len = read_le_u32(&image, 20)? as usize;
            let payload_end = 24_usize
                .checked_add(stored_len)
                .ok_or_else(|| Error::Package("compact RAM MRP payload range overflow".into()))?;
            if declared_len > image.len() || payload_end > declared_len {
                return Err(Error::Package(format!(
                    "compact RAM MRP payload 0x18..{payload_end:#x} exceeds declared length {declared_len}"
                )));
            }
            return expand_ram_payload(&image[24..payload_end], self.heap_len).map(Some);
        }

        let limits = ResourceLimits {
            max_package_len: self.heap_len,
            max_stored_file_len: self.heap_len,
            max_expanded_file_len: self.heap_len,
            max_total_expanded_len: self.heap_len,
            ..ResourceLimits::default()
        };
        let package = Package::parse(
            PathBuf::from("<guest-memory>.mrp"),
            Arc::from(image),
            limits,
        )?;
        match package.read_named(name) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(Error::EntryNotFound(_)) => Ok(None),
            Err(error) => Err(error),
        }
    }

    fn dispatch_libc(&mut self, slot: u32, cpu: &mut ArmCpu) -> Result<()> {
        match slot {
            0 => {
                let address = self.allocate(cpu.register(0) as usize, 8)?;
                cpu.set_register(0, address.0);
            }
            1 => cpu.set_register(0, 0),
            2 => {
                let source = GuestAddr(cpu.register(0));
                let old_len = cpu.register(1) as usize;
                let new_len = cpu.register(2) as usize;
                if source.0 == 0 {
                    let output = self.allocate(new_len, 8)?;
                    cpu.set_register(0, output.0);
                } else if new_len == 0 {
                    cpu.set_register(0, 0);
                } else {
                    let output = self.allocate(new_len, 8)?;
                    let bytes = self.memory.read(source, old_len.min(new_len))?;
                    self.memory.write(output, &bytes)?;
                    cpu.set_register(0, output.0);
                }
            }
            3 | 4 => {
                let destination = GuestAddr(cpu.register(0));
                let bytes = self
                    .memory
                    .read(GuestAddr(cpu.register(1)), cpu.register(2) as usize)?;
                self.memory.write(destination, &bytes)?;
                cpu.set_register(0, destination.0);
            }
            5 => {
                let destination = GuestAddr(cpu.register(0));
                let bytes = self.read_c_string(GuestAddr(cpu.register(1)), 1024 * 1024)?;
                self.memory.write(destination, &bytes)?;
                self.memory
                    .write_u8(destination.checked_add(bytes.len() as u32)?, 0)?;
                cpu.set_register(0, destination.0);
            }
            6 => {
                let destination = GuestAddr(cpu.register(0));
                let len = cpu.register(2) as usize;
                let source = self.read_c_string_bounded(GuestAddr(cpu.register(1)), len)?;
                let mut bytes = vec![0; len];
                let copied = source.len().min(len);
                bytes[..copied].copy_from_slice(&source[..copied]);
                self.memory.write(destination, &bytes)?;
                cpu.set_register(0, destination.0);
            }
            7 | 8 => {
                let destination = GuestAddr(cpu.register(0));
                let destination_len = self.read_c_string(destination, 1024 * 1024)?.len();
                let source = if slot == 8 {
                    self.read_c_string_bounded(
                        GuestAddr(cpu.register(1)),
                        cpu.register(2) as usize,
                    )?
                } else {
                    self.read_c_string(GuestAddr(cpu.register(1)), 1024 * 1024)?
                };
                let append_at = destination.checked_add(destination_len as u32)?;
                self.memory.write(append_at, &source)?;
                self.memory
                    .write_u8(append_at.checked_add(source.len() as u32)?, 0)?;
                cpu.set_register(0, destination.0);
            }
            9 => {
                let len = cpu.register(2) as usize;
                let left = self.memory.read(GuestAddr(cpu.register(0)), len)?;
                let right = self.memory.read(GuestAddr(cpu.register(1)), len)?;
                cpu.set_register(0, compare_bytes(&left, &right) as u32);
            }
            10 | 12 => {
                let left = self.read_c_string(GuestAddr(cpu.register(0)), 1024 * 1024)?;
                let right = self.read_c_string(GuestAddr(cpu.register(1)), 1024 * 1024)?;
                cpu.set_register(0, compare_bytes(&left, &right) as u32);
            }
            11 => {
                let limit = cpu.register(2) as usize;
                let left = self.read_c_string_bounded(GuestAddr(cpu.register(0)), limit)?;
                let right = self.read_c_string_bounded(GuestAddr(cpu.register(1)), limit)?;
                cpu.set_register(0, compare_bytes(&left, &right) as u32);
            }
            13 => {
                let start = cpu.register(0);
                let needle = cpu.register(1) as u8;
                let bytes = self
                    .memory
                    .read(GuestAddr(start), cpu.register(2) as usize)?;
                cpu.set_register(
                    0,
                    bytes
                        .iter()
                        .position(|byte| *byte == needle)
                        .map(|offset| start + offset as u32)
                        .unwrap_or(0),
                );
            }
            14 => {
                let destination = GuestAddr(cpu.register(0));
                let value = cpu.register(1) as u8;
                let len = cpu.register(2) as usize;
                self.memory.write(destination, &vec![value; len])?;
                cpu.set_register(0, destination.0);
            }
            15 => {
                let len = self
                    .read_c_string(GuestAddr(cpu.register(0)), 1024 * 1024)?
                    .len();
                cpu.set_register(0, len as u32);
            }
            16 => {
                let start = cpu.register(0);
                let haystack = self.read_c_string(GuestAddr(start), 1024 * 1024)?;
                let needle = self.read_c_string(GuestAddr(cpu.register(1)), 1024 * 1024)?;
                let found = if needle.is_empty() {
                    Some(0)
                } else {
                    haystack
                        .windows(needle.len())
                        .position(|window| window == needle)
                };
                cpu.set_register(0, found.map(|offset| start + offset as u32).unwrap_or(0));
            }
            17 => self.sprintf(cpu)?,
            18 => {
                let text = self.read_c_string(GuestAddr(cpu.register(0)), 1024)?;
                cpu.set_register(0, parse_integer(&text, 10).0 as u32);
            }
            19 => {
                let source = cpu.register(0);
                let text = self.read_c_string(GuestAddr(source), 1024)?;
                let base = cpu.register(2);
                let (value, consumed) = parse_integer(&text, base);
                let end_pointer = GuestAddr(cpu.register(1));
                if end_pointer.0 != 0 {
                    self.memory
                        .write_u32(end_pointer, source.wrapping_add(consumed as u32))?;
                }
                cpu.set_register(0, value as u32);
            }
            20 => {
                self.random_state = self
                    .random_state
                    .wrapping_mul(1_103_515_245)
                    .wrapping_add(12_345);
                cpu.set_register(0, (self.random_state >> 16) & 0x7fff);
            }
            _ => unreachable!(),
        }
        Ok(())
    }

    fn sprintf(&mut self, cpu: &mut ArmCpu) -> Result<()> {
        let destination = GuestAddr(cpu.register(0));
        let format = self.read_c_string(GuestAddr(cpu.register(1)), 64 * 1024)?;
        let stack_pointer = cpu.register(13);
        let mut argument_index = 0_u32;
        let mut next_argument = |memory: &GuestMemory| -> Result<u32> {
            let value = match argument_index {
                0 => cpu.register(2),
                1 => cpu.register(3),
                index => memory.read_u32(GuestAddr(stack_pointer + (index - 2) * 4))?,
            };
            argument_index += 1;
            Ok(value)
        };
        let mut output = Vec::new();
        let mut index = 0;
        while index < format.len() {
            if format[index] != b'%' {
                output.push(format[index]);
                index += 1;
                continue;
            }
            index += 1;
            if format.get(index) == Some(&b'%') {
                output.push(b'%');
                index += 1;
                continue;
            }
            while format
                .get(index)
                .is_some_and(|byte| b"-+ #0.123456789hl".contains(byte))
            {
                index += 1;
            }
            let specifier = *format
                .get(index)
                .ok_or_else(|| Error::Abi("sprintf format ends after '%'".into()))?;
            index += 1;
            let argument = next_argument(&self.memory)?;
            match specifier {
                b's' => {
                    output.extend_from_slice(&self.read_c_string(GuestAddr(argument), 1024 * 1024)?)
                }
                b'c' => output.push(argument as u8),
                b'd' | b'i' => output.extend_from_slice((argument as i32).to_string().as_bytes()),
                b'u' => output.extend_from_slice(argument.to_string().as_bytes()),
                b'x' => output.extend_from_slice(format!("{argument:x}").as_bytes()),
                b'X' => output.extend_from_slice(format!("{argument:X}").as_bytes()),
                b'p' => output.extend_from_slice(format!("0x{argument:08x}").as_bytes()),
                other => {
                    return Err(Error::Abi(format!(
                        "unsupported sprintf specifier {:?}",
                        char::from(other)
                    )));
                }
            }
        }
        self.memory.write(destination, &output)?;
        self.memory
            .write_u8(destination.checked_add(output.len() as u32)?, 0)?;
        cpu.set_register(0, output.len() as u32);
        Ok(())
    }

    fn allocate(&mut self, len: usize, alignment: u32) -> Result<GuestAddr> {
        let len = len.max(1);
        let mask = alignment - 1;
        let start = self
            .heap_cursor
            .checked_add(mask)
            .map(|value| value & !mask)
            .ok_or_else(|| Error::ArmFault("guest heap alignment overflow".into()))?;
        let end = start
            .checked_add(u32::try_from(len).map_err(|_| {
                Error::ArmFault(format!("guest allocation length {len} does not fit u32"))
            })?)
            .ok_or_else(|| Error::ArmFault("guest heap allocation overflow".into()))?;
        let heap_end = HEAP_BASE.0 + self.heap_len as u32;
        if end > heap_end {
            return Err(Error::ArmFault(format!(
                "guest heap exhausted while allocating {len} bytes"
            )));
        }
        self.heap_cursor = end;
        self.memory
            .write_u32(data_slot_address(111), heap_end - end)?;
        Ok(GuestAddr(start))
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

fn md5_consume(
    state: &mut [u32; 4],
    buffer: &mut [u8; 64],
    mut buffered: usize,
    mut input: &[u8],
) -> usize {
    if buffered != 0 {
        let copied = (64 - buffered).min(input.len());
        buffer[buffered..buffered + copied].copy_from_slice(&input[..copied]);
        buffered += copied;
        input = &input[copied..];
        if buffered == 64 {
            md5_transform(state, buffer);
            buffered = 0;
        }
    }
    while input.len() >= 64 {
        let block: &[u8; 64] = input[..64].try_into().expect("checked MD5 block length");
        md5_transform(state, block);
        input = &input[64..];
    }
    if !input.is_empty() {
        buffer[..input.len()].copy_from_slice(input);
        buffered = input.len();
    }
    buffered
}

fn md5_transform(state: &mut [u32; 4], block: &[u8; 64]) {
    const SHIFTS: [u32; 64] = [
        7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5,
        9, 14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10,
        15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
    ];
    const CONSTANTS: [u32; 64] = [
        0xd76a_a478,
        0xe8c7_b756,
        0x2420_70db,
        0xc1bd_ceee,
        0xf57c_0faf,
        0x4787_c62a,
        0xa830_4613,
        0xfd46_9501,
        0x6980_98d8,
        0x8b44_f7af,
        0xffff_5bb1,
        0x895c_d7be,
        0x6b90_1122,
        0xfd98_7193,
        0xa679_438e,
        0x49b4_0821,
        0xf61e_2562,
        0xc040_b340,
        0x265e_5a51,
        0xe9b6_c7aa,
        0xd62f_105d,
        0x0244_1453,
        0xd8a1_e681,
        0xe7d3_fbc8,
        0x21e1_cde6,
        0xc337_07d6,
        0xf4d5_0d87,
        0x455a_14ed,
        0xa9e3_e905,
        0xfcef_a3f8,
        0x676f_02d9,
        0x8d2a_4c8a,
        0xfffa_3942,
        0x8771_f681,
        0x6d9d_6122,
        0xfde5_380c,
        0xa4be_ea44,
        0x4bde_cfa9,
        0xf6bb_4b60,
        0xbebf_bc70,
        0x289b_7ec6,
        0xeaa1_27fa,
        0xd4ef_3085,
        0x0488_1d05,
        0xd9d4_d039,
        0xe6db_99e5,
        0x1fa2_7cf8,
        0xc4ac_5665,
        0xf429_2244,
        0x432a_ff97,
        0xab94_23a7,
        0xfc93_a039,
        0x655b_59c3,
        0x8f0c_cc92,
        0xffef_f47d,
        0x8584_5dd1,
        0x6fa8_7e4f,
        0xfe2c_e6e0,
        0xa301_4314,
        0x4e08_11a1,
        0xf753_7e82,
        0xbd3a_f235,
        0x2ad7_d2bb,
        0xeb86_d391,
    ];

    let mut words = [0_u32; 16];
    for (word, bytes) in words.iter_mut().zip(block.chunks_exact(4)) {
        *word = u32::from_le_bytes(bytes.try_into().expect("four-byte MD5 word"));
    }
    let [mut a, mut b, mut c, mut d] = *state;
    for index in 0..64 {
        let (function, word) = match index {
            0..=15 => ((b & c) | (!b & d), index),
            16..=31 => ((d & b) | (!d & c), (5 * index + 1) % 16),
            32..=47 => (b ^ c ^ d, (3 * index + 5) % 16),
            _ => (c ^ (b | !d), (7 * index) % 16),
        };
        let next = b.wrapping_add(
            a.wrapping_add(function)
                .wrapping_add(CONSTANTS[index])
                .wrapping_add(words[word])
                .rotate_left(SHIFTS[index]),
        );
        a = d;
        d = c;
        c = b;
        b = next;
    }
    state[0] = state[0].wrapping_add(a);
    state[1] = state[1].wrapping_add(b);
    state[2] = state[2].wrapping_add(c);
    state[3] = state[3].wrapping_add(d);
}

impl BitmapTransform {
    fn apply(self, x: i64, y: i64) -> (i64, i64) {
        (
            (i64::from(self.a) * x + i64::from(self.b) * y) >> 8,
            (i64::from(self.c) * x + i64::from(self.d) * y) >> 8,
        )
    }
}

fn bitmap_pixel_address(pixels: GuestAddr, stride: usize, x: usize, y: usize) -> Result<GuestAddr> {
    let byte_offset = y
        .checked_mul(stride)
        .and_then(|offset| offset.checked_add(x))
        .and_then(|offset| offset.checked_mul(2))
        .and_then(|offset| u32::try_from(offset).ok())
        .ok_or_else(|| Error::Abi("bitmap pixel offset overflow".into()))?;
    pixels.checked_add(byte_offset)
}

fn compare_bytes(left: &[u8], right: &[u8]) -> i32 {
    for (left, right) in left.iter().copied().zip(right.iter().copied()) {
        if left != right {
            return i32::from(left) - i32::from(right);
        }
    }
    left.len().cmp(&right.len()) as i32
}

fn parse_integer(input: &[u8], requested_base: u32) -> (i64, usize) {
    let mut index = 0;
    while input.get(index).is_some_and(u8::is_ascii_whitespace) {
        index += 1;
    }
    let negative = input.get(index) == Some(&b'-');
    if negative || input.get(index) == Some(&b'+') {
        index += 1;
    }
    let mut base = requested_base;
    if base == 0 {
        base = if input
            .get(index..index + 2)
            .is_some_and(|prefix| prefix[0] == b'0' && matches!(prefix[1], b'x' | b'X'))
        {
            16
        } else if input.get(index) == Some(&b'0') {
            8
        } else {
            10
        };
    }
    if base == 16
        && input
            .get(index..index + 2)
            .is_some_and(|prefix| prefix[0] == b'0' && matches!(prefix[1], b'x' | b'X'))
    {
        index += 2;
    }
    if !(2..=36).contains(&base) {
        return (0, index);
    }
    let digit_start = index;
    let mut value = 0_i64;
    while let Some(digit) = input.get(index).and_then(|byte| match byte {
        b'0'..=b'9' => Some(u32::from(byte - b'0')),
        b'a'..=b'z' => Some(u32::from(byte - b'a') + 10),
        b'A'..=b'Z' => Some(u32::from(byte - b'A') + 10),
        _ => None,
    }) {
        if digit >= base {
            break;
        }
        value = value
            .saturating_mul(i64::from(base))
            .saturating_add(i64::from(digit));
        index += 1;
    }
    if index == digit_start {
        return (0, digit_start);
    }
    (if negative { -value } else { value }, index)
}

fn read_le_u32(bytes: &[u8], offset: usize) -> Result<u32> {
    let raw = bytes
        .get(offset..offset + 4)
        .ok_or_else(|| Error::Package(format!("truncated RAM MRP u32 at {offset:#x}")))?;
    Ok(u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]))
}

fn expand_ram_payload(stored: &[u8], limit: usize) -> Result<Vec<u8>> {
    if !stored.starts_with(&[0x1f, 0x8b, 0x08]) {
        return Ok(stored.to_vec());
    }
    let mut decoder = GzDecoder::new(stored).take((limit as u64).saturating_add(1));
    let mut output = Vec::new();
    decoder
        .read_to_end(&mut output)
        .map_err(|error| Error::Package(format!("invalid RAM MRP gzip payload: {error}")))?;
    if output.len() > limit {
        return Err(Error::ResourceLimit(format!(
            "expanded RAM MRP payload exceeds {limit} bytes"
        )));
    }
    Ok(output)
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
mod tests {
    use std::io::Write;

    use flate2::{Compression, write::GzEncoder};

    use super::*;

    struct StubServices;

    impl NativeServices for StubServices {
        fn read_package_file(
            &mut self,
            _package_name: &[u8],
            _name: &[u8],
        ) -> Result<Option<Vec<u8>>> {
            Ok(None)
        }

        fn file_info(&mut self, _name: &[u8]) -> Result<i32> {
            Ok(-1)
        }

        fn remove_file(&mut self, _name: &[u8]) -> Result<i32> {
            Ok(0)
        }

        fn rename_file(&mut self, _source: &[u8], _destination: &[u8]) -> Result<i32> {
            Ok(0)
        }

        fn create_dir(&mut self, _name: &[u8]) -> Result<i32> {
            Ok(0)
        }

        fn remove_dir(&mut self, _name: &[u8]) -> Result<i32> {
            Ok(0)
        }

        fn open_file(&mut self, _name: &[u8], _mode: u32) -> Result<i32> {
            Ok(-1)
        }

        fn close_file(&mut self, _handle: i32) -> Result<i32> {
            Ok(0)
        }

        fn write_file(&mut self, _handle: i32, _bytes: &[u8]) -> Result<Option<usize>> {
            Ok(None)
        }

        fn read_file(&mut self, _handle: i32, _len: usize) -> Result<Option<Vec<u8>>> {
            Ok(None)
        }

        fn seek_file(&mut self, _handle: i32, _offset: i32, _origin: u32) -> Result<bool> {
            Ok(false)
        }

        fn file_len(&mut self, _name: &[u8]) -> Result<Option<u64>> {
            Ok(None)
        }

        fn find_start(&mut self, _directory: &[u8]) -> Result<Option<(i32, Vec<u8>)>> {
            Ok(Some((7, b"entry.dat".to_vec())))
        }

        fn find_next(&mut self, _handle: i32) -> Result<Option<Vec<u8>>> {
            Ok(None)
        }

        fn find_stop(&mut self, handle: i32) -> Result<bool> {
            Ok(handle == 7)
        }

        fn char_bitmap(
            &mut self,
            codepoint: u32,
            font: u32,
        ) -> Result<Option<(Vec<u8>, u32, u32)>> {
            Ok((codepoint == 0x2603 && font == 7).then(|| (vec![0x01, 0x80, 0x96, 0x4b], 9, 2)))
        }

        fn draw_bitmap(
            &mut self,
            _pixels: &[u8],
            _x: i32,
            _y: i32,
            _width: usize,
            _height: usize,
        ) -> Result<()> {
            Ok(())
        }
    }

    fn read_bitmap_pixels(
        runtime: &ExtRuntime,
        address: GuestAddr,
        width: usize,
        height: usize,
    ) -> Vec<u16> {
        (0..width * height)
            .map(|index| {
                runtime
                    .memory
                    .read_u16(address.checked_add((index * 2) as u32).unwrap())
                    .unwrap()
            })
            .collect()
    }

    #[test]
    fn transformed_bitmap_copy_snapshots_overlapping_source_pixels() {
        let mut runtime =
            ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
        let bitmap = runtime.allocate(16, 2).unwrap();
        for (index, color) in [1_u16, 2, 3, 4, 5, 6, 7, 8].into_iter().enumerate() {
            runtime
                .memory
                .write_u16(bitmap.checked_add((index * 2) as u32).unwrap(), color)
                .unwrap();
        }

        runtime
            .copy_transformed_bitmap(
                BitmapDescriptor {
                    pixels: bitmap,
                    width: 4,
                    height: 2,
                    x: 1,
                    y: 0,
                },
                BitmapDescriptor {
                    pixels: bitmap,
                    width: 4,
                    height: 2,
                    x: 0,
                    y: 0,
                },
                3,
                2,
                BitmapTransform {
                    a: 256,
                    b: 0,
                    c: 0,
                    d: 256,
                    mode: 2,
                },
                0,
                0,
            )
            .unwrap();

        assert_eq!(
            read_bitmap_pixels(&runtime, bitmap, 4, 2),
            [1, 1, 2, 3, 5, 5, 6, 7]
        );
    }

    #[test]
    fn strncmp_compares_a_bounded_prefix_without_requiring_a_nul() {
        let mut runtime =
            ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
        let left = runtime.allocate(8, 1).unwrap();
        let right = runtime.allocate(8, 1).unwrap();
        runtime.memory.write(left, b"MRPleft!").unwrap();
        runtime.memory.write(right, b"MRQright").unwrap();

        let mut cpu = ArmCpu::new();
        cpu.set_register(0, left.0);
        cpu.set_register(1, right.0);
        cpu.set_register(2, 2);
        runtime.dispatch_libc(11, &mut cpu).unwrap();
        assert_eq!(cpu.register(0), 0);

        cpu.set_register(0, left.0);
        cpu.set_register(1, right.0);
        cpu.set_register(2, 3);
        runtime.dispatch_libc(11, &mut cpu).unwrap();
        assert_eq!(cpu.register(0) as i32, -1);
    }

    #[test]
    fn transformed_bitmap_copy_normalizes_a_quarter_turn() {
        let mut runtime =
            ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
        let source = runtime.allocate(12, 2).unwrap();
        let destination = runtime.allocate(12, 2).unwrap();
        for (index, color) in [1_u16, 2, 3, 4, 5, 6].into_iter().enumerate() {
            runtime
                .memory
                .write_u16(source.checked_add((index * 2) as u32).unwrap(), color)
                .unwrap();
        }

        runtime
            .copy_transformed_bitmap(
                BitmapDescriptor {
                    pixels: destination,
                    width: 2,
                    height: 3,
                    x: 0,
                    y: 0,
                },
                BitmapDescriptor {
                    pixels: source,
                    width: 3,
                    height: 2,
                    x: 0,
                    y: 0,
                },
                3,
                2,
                BitmapTransform {
                    a: 0,
                    b: -256,
                    c: 256,
                    d: 0,
                    mode: 2,
                },
                0,
                0,
            )
            .unwrap();

        assert_eq!(
            read_bitmap_pixels(&runtime, destination, 2, 3),
            [4, 1, 5, 2, 6, 3]
        );
    }

    #[test]
    fn transformed_bitmap_trap_treats_r0_as_the_source() {
        let mut runtime =
            ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
        let source = runtime.allocate(4, 2).unwrap();
        let destination = runtime.allocate(4, 2).unwrap();
        runtime.memory.write_u16(source, 0x1234).unwrap();
        runtime
            .memory
            .write_u16(source.checked_add(2).unwrap(), 0xabcd)
            .unwrap();

        let source_descriptor = runtime.allocate(12, 4).unwrap();
        runtime
            .memory
            .write_u32(source_descriptor, source.0)
            .unwrap();
        runtime
            .memory
            .write_u16(source_descriptor.checked_add(4).unwrap(), 2)
            .unwrap();
        runtime
            .memory
            .write_u16(source_descriptor.checked_add(6).unwrap(), 1)
            .unwrap();

        let destination_descriptor = runtime.allocate(12, 4).unwrap();
        runtime
            .memory
            .write_u32(destination_descriptor, destination.0)
            .unwrap();
        runtime
            .memory
            .write_u16(destination_descriptor.checked_add(4).unwrap(), 2)
            .unwrap();
        runtime
            .memory
            .write_u16(destination_descriptor.checked_add(6).unwrap(), 1)
            .unwrap();

        let transform = runtime.allocate(10, 2).unwrap();
        runtime.memory.write_u16(transform, 256).unwrap();
        runtime
            .memory
            .write_u16(transform.checked_add(6).unwrap(), 256)
            .unwrap();
        runtime
            .memory
            .write_u16(transform.checked_add(8).unwrap(), 2)
            .unwrap();
        let stack = runtime.allocate(8, 4).unwrap();
        runtime.memory.write_u32(stack, transform.0).unwrap();

        let mut cpu = ArmCpu::new();
        cpu.set_register(0, source_descriptor.0);
        cpu.set_register(1, destination_descriptor.0);
        cpu.set_register(2, 2);
        cpu.set_register(3, 1);
        cpu.set_register(13, stack.0);
        runtime
            .dispatch(121, 0, &mut cpu, &mut StubServices)
            .unwrap();

        assert_eq!(
            read_bitmap_pixels(&runtime, destination, 2, 1),
            [0x1234, 0xabcd]
        );
    }

    #[test]
    fn datetime_uses_the_deterministic_headless_baseline() {
        let mut runtime =
            ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
        let output = runtime.allocate(8, 2).unwrap();
        let mut cpu = ArmCpu::new();
        cpu.set_register(0, output.0);

        runtime
            .dispatch(34, 0, &mut cpu, &mut StubServices)
            .unwrap();

        assert_eq!(cpu.register(0), 0);
        assert_eq!(
            runtime.memory.read(output, 8).unwrap(),
            [0xdc, 0x07, 6, 20, 0, 0, 0, 3]
        );
    }

    #[test]
    fn guest_character_bitmap_uses_lsb_first_bytes() {
        let mut runtime =
            ExtRuntime::new(16, 16, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
        let width_out = runtime.allocate(4, 4).unwrap();
        let height_out = runtime.allocate(4, 4).unwrap();
        let mut cpu = ArmCpu::new();
        cpu.set_register(0, 0x2603);
        cpu.set_register(1, 7);
        cpu.set_register(2, width_out.0);
        cpu.set_register(3, height_out.0);

        runtime
            .dispatch(30, 0, &mut cpu, &mut StubServices)
            .unwrap();

        let bitmap = GuestAddr(cpu.register(0));
        assert_ne!(bitmap.0, 0);
        assert_eq!(
            runtime.memory.read(bitmap, 4).unwrap(),
            [0x80, 0x01, 0x69, 0xd2]
        );
        assert_eq!(runtime.memory.read_u32(width_out).unwrap(), 9);
        assert_eq!(runtime.memory.read_u32(height_out).unwrap(), 2);
    }

    #[test]
    fn host_text_drawing_keeps_msb_first_glyph_bytes() {
        let mut runtime =
            ExtRuntime::new(16, 16, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();

        runtime
            .draw_text_to_screen(&[0x2603], 0, 0, 0xffff, 7, &mut StubServices)
            .unwrap();

        assert_eq!(
            runtime
                .memory
                .read_u16(runtime.screen_address(7, 0, 16).unwrap())
                .unwrap(),
            0xffff
        );
        assert_eq!(
            runtime
                .memory
                .read_u16(runtime.screen_address(8, 0, 16).unwrap())
                .unwrap(),
            0xffff
        );
        assert_eq!(
            runtime
                .memory
                .read_u16(runtime.screen_address(0, 0, 16).unwrap())
                .unwrap(),
            0
        );
    }

    #[test]
    fn user_info_reports_unavailable_without_mutating_the_output() {
        let mut runtime =
            ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
        let output = runtime.allocate(64, 4).unwrap();
        runtime.memory.write(output, &[0xaa; 64]).unwrap();
        let mut cpu = ArmCpu::new();
        cpu.set_register(0, output.0);

        runtime
            .dispatch(35, 0, &mut cpu, &mut StubServices)
            .unwrap();

        assert_eq!(cpu.register(0) as i32, -1);
        assert_eq!(runtime.memory.read(output, 64).unwrap(), vec![0xaa; 64]);
    }

    #[test]
    fn mtk_user_info_returns_the_deterministic_virtual_device_profile() {
        let mut runtime =
            ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
        runtime.set_device_info_profile(DeviceInfoProfile::DeterministicMtk);
        let output = runtime.allocate(PLATFORM_USER_INFO_LEN, 4).unwrap();
        let mut cpu = ArmCpu::new();
        cpu.set_register(0, output.0);

        runtime
            .dispatch(35, 0, &mut cpu, &mut StubServices)
            .unwrap();

        assert_eq!(cpu.register(0), 0);
        assert_eq!(
            runtime.memory.read(output, PLATFORM_USER_INFO_LEN).unwrap(),
            platform_user_info()
        );
        assert_eq!(
            runtime
                .memory
                .read_u32(output.checked_add(48).unwrap())
                .unwrap(),
            PLATFORM_USER_INFO_VERSION
        );

        cpu.set_register(0, 0);
        runtime
            .dispatch(35, 0, &mut cpu, &mut StubServices)
            .unwrap();
        assert_eq!(cpu.register(0) as i32, -1);
    }

    #[test]
    fn headless_audio_stop_is_idempotent() {
        let mut runtime =
            ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
        let mut cpu = ArmCpu::new();
        cpu.set_register(0, 7);

        runtime
            .dispatch(58, 0, &mut cpu, &mut StubServices)
            .unwrap();

        assert_eq!(cpu.register(0), 0);
    }

    #[test]
    fn headless_network_lifecycle_succeeds_but_socket_operations_fail() {
        let mut runtime =
            ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
        let mut cpu = ArmCpu::new();
        cpu.set_register(0, 0x1000_0009);

        runtime
            .dispatch(81, 0, &mut cpu, &mut StubServices)
            .unwrap();

        assert_eq!(cpu.register(0), 0);

        runtime
            .dispatch(82, 0, &mut cpu, &mut StubServices)
            .unwrap();
        assert_eq!(cpu.register(0), 0);

        runtime
            .dispatch(84, 0, &mut cpu, &mut StubServices)
            .unwrap();
        assert_eq!(cpu.register(0) as i32, -1);

        for slot in [85, 86, 87] {
            runtime
                .dispatch(slot, 0, &mut cpu, &mut StubServices)
                .unwrap();
            assert_eq!(cpu.register(0) as i32, -1, "slot {slot}");
        }

        let payload = runtime.allocate(4, 1).unwrap();
        runtime.memory.write(payload, b"test").unwrap();
        cpu.set_register(1, payload.0);
        cpu.set_register(2, 4);
        runtime
            .dispatch(89, 0, &mut cpu, &mut StubServices)
            .unwrap();
        assert_eq!(cpu.register(0) as i32, -1);
    }

    #[test]
    fn platform_storage_query_reports_normal_mode() {
        let mut runtime =
            ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
        let mut cpu = ArmCpu::new();
        cpu.set_register(0, 1_218);

        runtime
            .dispatch(37, 0, &mut cpu, &mut StubServices)
            .unwrap();

        assert_eq!(cpu.register(0), 1_001);
    }

    #[test]
    fn platform_storage_info_reports_sufficient_available_space() {
        let mut runtime =
            ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
        let drive = runtime.allocate(1, 1).unwrap();
        runtime.memory.write(drive, b"C").unwrap();
        let output = runtime.allocate(4, 4).unwrap();
        let output_len = runtime.allocate(4, 4).unwrap();
        let stack = runtime.allocate(4, 4).unwrap();
        runtime.memory.write_u32(stack, output_len.0).unwrap();

        let mut cpu = ArmCpu::new();
        cpu.set_register(0, 1_305);
        cpu.set_register(1, drive.0);
        cpu.set_register(2, 1);
        cpu.set_register(3, output.0);
        cpu.set_register(13, stack.0);
        runtime
            .dispatch(38, 0, &mut cpu, &mut StubServices)
            .unwrap();

        let info = GuestAddr(runtime.memory.read_u32(output).unwrap());
        let block_size = runtime
            .memory
            .read_u32(info.checked_add(8).unwrap())
            .unwrap();
        let available_blocks = runtime
            .memory
            .read_u32(info.checked_add(12).unwrap())
            .unwrap();
        assert_eq!(cpu.register(0), 0);
        assert_eq!(runtime.memory.read_u32(output_len).unwrap(), 16);
        assert_eq!(block_size, PLATFORM_STORAGE_BLOCK_SIZE);
        assert_eq!(available_blocks, PLATFORM_STORAGE_AVAILABLE_BLOCKS);
        assert!(u64::from(block_size) * u64::from(available_blocks) / 1024 > 2048);
    }

    #[test]
    fn platform_storage_drive_query_resolves_the_application_volume() {
        let mut runtime =
            ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
        let volume = runtime.allocate(1, 1).unwrap();
        runtime.memory.write(volume, b"Y").unwrap();
        let output = runtime.allocate(4, 4).unwrap();
        let output_len = runtime.allocate(4, 4).unwrap();
        let stack = runtime.allocate(4, 4).unwrap();
        runtime.memory.write_u32(stack, output_len.0).unwrap();

        let mut cpu = ArmCpu::new();
        cpu.set_register(0, 1_204);
        cpu.set_register(1, volume.0);
        cpu.set_register(2, 1);
        cpu.set_register(3, output.0);
        cpu.set_register(13, stack.0);
        runtime
            .dispatch(38, 0, &mut cpu, &mut StubServices)
            .unwrap();

        let drive = GuestAddr(runtime.memory.read_u32(output).unwrap());
        assert_eq!(cpu.register(0), 0);
        assert_eq!(runtime.memory.read_u32(output_len).unwrap(), 2);
        assert_eq!(runtime.memory.read(drive, 2).unwrap(), b"C\0");
    }

    #[test]
    fn text_drawing_accepts_the_baseline_wide_text_flags() {
        let mut runtime =
            ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
        let text = runtime.allocate(4, 2).unwrap();
        runtime.memory.write(text, &[0, b'A', 0, 0]).unwrap();
        let stack = runtime.allocate(16, 4).unwrap();

        let mut cpu = ArmCpu::new();
        cpu.set_register(13, stack.0);
        for flags in 0..=2 {
            runtime
                .memory
                .write_u32(stack.checked_add(12).unwrap(), flags)
                .unwrap();
            cpu.set_register(0, text.0);
            runtime
                .dispatch(123, 0, &mut cpu, &mut StubServices)
                .unwrap();
            assert_eq!(cpu.register(0), 0);
        }

        runtime
            .memory
            .write_u32(stack.checked_add(12).unwrap(), 3)
            .unwrap();
        assert!(matches!(
            runtime.dispatch(123, 0, &mut cpu, &mut StubServices),
            Err(Error::Abi(message))
                if message == "unsupported text drawing flags 3 called by module 0"
        ));
    }

    #[test]
    fn md5_slots_support_incremental_and_cross_block_inputs() {
        let mut runtime =
            ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
        let context = runtime.allocate(88, 4).unwrap();
        let digest = runtime.allocate(16, 4).unwrap();
        let input = runtime.allocate(80, 4).unwrap();
        let bytes = (0_u8..80).collect::<Vec<_>>();
        runtime.memory.write(input, &bytes).unwrap();
        let mut cpu = ArmCpu::new();

        cpu.set_register(0, context.0);
        runtime
            .dispatch(113, 0, &mut cpu, &mut StubServices)
            .unwrap();
        cpu.set_register(0, context.0);
        cpu.set_register(1, input.0);
        cpu.set_register(2, 17);
        runtime
            .dispatch(114, 0, &mut cpu, &mut StubServices)
            .unwrap();
        cpu.set_register(0, context.0);
        cpu.set_register(1, input.checked_add(17).unwrap().0);
        cpu.set_register(2, 63);
        runtime
            .dispatch(114, 0, &mut cpu, &mut StubServices)
            .unwrap();
        cpu.set_register(0, context.0);
        cpu.set_register(1, digest.0);
        runtime
            .dispatch(115, 0, &mut cpu, &mut StubServices)
            .unwrap();
        let incremental = runtime.memory.read(digest, 16).unwrap();

        cpu.set_register(0, context.0);
        runtime
            .dispatch(113, 0, &mut cpu, &mut StubServices)
            .unwrap();
        cpu.set_register(0, context.0);
        cpu.set_register(1, input.0);
        cpu.set_register(2, 80);
        runtime
            .dispatch(114, 0, &mut cpu, &mut StubServices)
            .unwrap();
        cpu.set_register(0, context.0);
        cpu.set_register(1, digest.0);
        runtime
            .dispatch(115, 0, &mut cpu, &mut StubServices)
            .unwrap();

        assert_eq!(runtime.memory.read(digest, 16).unwrap(), incremental);

        cpu.set_register(0, context.0);
        runtime
            .dispatch(113, 0, &mut cpu, &mut StubServices)
            .unwrap();
        cpu.set_register(0, context.0);
        cpu.set_register(1, digest.0);
        runtime
            .dispatch(115, 0, &mut cpu, &mut StubServices)
            .unwrap();
        assert_eq!(
            runtime.memory.read(digest, 16).unwrap(),
            [
                0xd4, 0x1d, 0x8c, 0xd9, 0x8f, 0x00, 0xb2, 0x04, 0xe9, 0x80, 0x09, 0x98, 0xec, 0xf8,
                0x42, 0x7e,
            ]
        );
    }

    #[test]
    fn platform_memory_extension_returns_a_zeroed_guest_arena() {
        let mut runtime =
            ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
        let output = runtime.allocate(4, 4).unwrap();
        let output_len = runtime.allocate(4, 4).unwrap();
        let stack = runtime.allocate(4, 4).unwrap();
        runtime.memory.write_u32(stack, output_len.0).unwrap();
        let heap_cursor_before = runtime.heap_cursor;
        let free_before = runtime.memory.read_u32(data_slot_address(111)).unwrap();

        let mut cpu = ArmCpu::new();
        cpu.set_register(0, 1_014);
        cpu.set_register(2, 32);
        cpu.set_register(3, output.0);
        cpu.set_register(13, stack.0);
        runtime
            .dispatch(38, 0, &mut cpu, &mut StubServices)
            .unwrap();

        let arena = GuestAddr(runtime.memory.read_u32(output).unwrap());
        assert_eq!(cpu.register(0), 0);
        assert_eq!(arena.0 % 8, 0);
        assert_eq!(runtime.memory.read_u32(output_len).unwrap(), 32);
        assert_eq!(runtime.memory.read(arena, 32).unwrap(), vec![0; 32]);

        runtime.memory.write_u32(arena, 0xaaaa_aaaa).unwrap();
        cpu.set_register(0, 1_015);
        cpu.set_register(1, arena.0);
        cpu.set_register(2, 4);
        runtime
            .dispatch(38, 0, &mut cpu, &mut StubServices)
            .unwrap();

        assert_eq!(cpu.register(0), 0);
        assert_eq!(runtime.heap_cursor, heap_cursor_before);
        assert_eq!(
            runtime.memory.read_u32(data_slot_address(111)).unwrap(),
            free_before
        );
        assert_eq!(runtime.memory.read(arena, 32).unwrap(), vec![0; 32]);
        cpu.set_register(0, 1_015);
        assert!(matches!(
            runtime.dispatch(38, 0, &mut cpu, &mut StubServices),
            Err(Error::Abi(message)) if message.contains("unknown arena")
        ));
    }

    #[test]
    fn unavailable_platform_extension_clears_its_output_fields() {
        let mut runtime =
            ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
        let output = runtime.allocate(4, 4).unwrap();
        let output_len = runtime.allocate(4, 4).unwrap();
        let stack = runtime.allocate(4, 4).unwrap();
        runtime.memory.write_u32(output, 0xaaaa_aaaa).unwrap();
        runtime.memory.write_u32(output_len, 0xbbbb_bbbb).unwrap();
        runtime.memory.write_u32(stack, output_len.0).unwrap();

        let mut cpu = ArmCpu::new();
        cpu.set_register(0, 1_222);
        cpu.set_register(3, output.0);
        cpu.set_register(13, stack.0);
        runtime
            .dispatch(38, 0, &mut cpu, &mut StubServices)
            .unwrap();

        assert_eq!(cpu.register(0) as i32, -1);
        assert_eq!(runtime.memory.read_u32(output).unwrap(), 0);
        assert_eq!(runtime.memory.read_u32(output_len).unwrap(), 0);

        cpu.set_register(0, 1_223);
        cpu.set_register(1, 0);
        cpu.set_register(2, 0);
        cpu.set_register(3, 0);
        runtime
            .dispatch(38, 0, &mut cpu, &mut StubServices)
            .unwrap();
        assert_eq!(cpu.register(0) as i32, -1);

        cpu.set_register(0, 0x0009_0003);
        runtime
            .dispatch(38, 0, &mut cpu, &mut StubServices)
            .unwrap();
        assert_eq!(cpu.register(0) as i32, -1);

        let event = runtime.allocate(35, 1).unwrap();
        cpu.set_register(0, 0x0009_0004);
        cpu.set_register(1, event.0);
        cpu.set_register(2, 35);
        runtime
            .dispatch(38, 0, &mut cpu, &mut StubServices)
            .unwrap();
        assert_eq!(cpu.register(0) as i32, -1);
    }

    #[test]
    fn platform_sim_query_returns_a_valid_empty_slot_list() {
        let mut runtime =
            ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
        let output = runtime.allocate(4, 4).unwrap();
        let output_len = runtime.allocate(4, 4).unwrap();
        let stack = runtime.allocate(4, 4).unwrap();
        runtime.memory.write_u32(output, 0xaaaa_aaaa).unwrap();
        runtime.memory.write_u32(output_len, 0xbbbb_bbbb).unwrap();
        runtime.memory.write_u32(stack, output_len.0).unwrap();

        let mut cpu = ArmCpu::new();
        cpu.set_register(0, 1_307);
        cpu.set_register(3, output.0);
        cpu.set_register(13, stack.0);
        runtime
            .dispatch(38, 0, &mut cpu, &mut StubServices)
            .unwrap();

        assert_eq!(cpu.register(0), 0);
        assert_eq!(
            runtime.memory.read_u32(output).unwrap(),
            PLATFORM_SIM_INFO_DATA.0
        );
        assert_eq!(
            runtime.memory.read_u32(output_len).unwrap(),
            PLATFORM_SIM_INFO_LEN as u32
        );
        assert_eq!(
            runtime
                .memory
                .read(PLATFORM_SIM_INFO_DATA, PLATFORM_SIM_INFO_LEN)
                .unwrap(),
            vec![0; PLATFORM_SIM_INFO_LEN]
        );
    }

    #[test]
    fn platform_dialog_draws_and_restores_the_screen() {
        let mut runtime =
            ExtRuntime::new(240, 320, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
        let title = runtime.allocate(6, 2).unwrap();
        let message = runtime.allocate(6, 2).unwrap();
        runtime
            .memory
            .write(title, &[0x00, 0x41, 0, 0, 0, 0])
            .unwrap();
        runtime
            .memory
            .write(message, &[0x00, 0x42, 0, 0, 0, 0])
            .unwrap();

        let mut cpu = ArmCpu::new();
        cpu.set_register(0, title.0);
        cpu.set_register(1, message.0);
        runtime
            .dispatch(69, 0, &mut cpu, &mut StubServices)
            .unwrap();
        let handle = cpu.register(0);
        assert_ne!(handle, 0);
        assert_eq!(
            runtime
                .memory
                .read_u16(runtime.screen_address(89, 266, 240).unwrap())
                .unwrap(),
            Framebuffer::rgb565(32, 160, 224)
        );

        cpu.set_register(0, handle);
        runtime
            .dispatch(70, 0, &mut cpu, &mut StubServices)
            .unwrap();
        assert_eq!(cpu.register(0), 0);
        assert_eq!(
            runtime
                .memory
                .read_u16(runtime.screen_address(89, 266, 240).unwrap())
                .unwrap(),
            0
        );

        cpu.set_register(0, title.0);
        cpu.set_register(1, message.0);
        cpu.set_register(2, 1);
        assert!(matches!(
            runtime.dispatch(69, 0, &mut cpu, &mut StubServices),
            Err(Error::Abi(message)) if message == "unsupported platform dialog style 1"
        ));
    }

    #[test]
    fn platform_dialog_routes_and_fully_consumes_a_cancel_key() {
        let mut runtime =
            ExtRuntime::new(240, 320, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
        runtime.dialogs.insert(
            1,
            PlatformDialog {
                previous_screen: Vec::new(),
                dialog_screen: Vec::new(),
            },
        );

        assert_eq!(runtime.route_key_event(18, true), Some((6, 0, 0)));
        runtime.dialogs.clear();
        assert_eq!(runtime.route_key_event(18, false), None);
        assert_eq!(runtime.route_key_event(12, true), Some((0, 12, 0)));
    }

    #[test]
    fn exposes_an_exit_lifecycle_request() {
        let mut runtime =
            ExtRuntime::new(240, 320, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
        let stack = runtime.allocate(4, 4).unwrap();
        let mut cpu = ArmCpu::new();
        cpu.set_register(13, stack.0);

        runtime
            .dispatch(54, 0, &mut cpu, &mut StubServices)
            .unwrap();

        assert_eq!(cpu.register(0), 0);
        assert_eq!(
            runtime.lifecycle_request().unwrap(),
            Some(ExtLifecycleRequest::Exit)
        );
    }

    #[test]
    fn reads_the_compact_ram_package_payload() {
        let expected = b"MRPGCMAPguest module";
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(expected).unwrap();
        let stored = encoder.finish().unwrap();

        let mut image = vec![0_u8; 24 + stored.len()];
        let image_len = image.len() as u32;
        image[..4].copy_from_slice(b"MRPG");
        image[4..8].copy_from_slice(&4_u32.to_le_bytes());
        image[8..12].copy_from_slice(&image_len.to_le_bytes());
        image[12..16].copy_from_slice(&4_u32.to_le_bytes());
        image[16..20].copy_from_slice(b"abc\0");
        image[20..24].copy_from_slice(&(stored.len() as u32).to_le_bytes());
        image[24..].copy_from_slice(&stored);

        let mut runtime =
            ExtRuntime::new(240, 320, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
        let address = runtime.allocate(image.len(), 8).unwrap();
        runtime.memory.write(address, &image).unwrap();

        assert_eq!(
            runtime
                .read_ram_package_file(address, image.len(), b"abc")
                .unwrap(),
            Some(expected.to_vec())
        );
        assert_eq!(
            runtime
                .read_ram_package_file(address, image.len(), b"other")
                .unwrap(),
            None
        );
    }

    #[test]
    fn compact_ram_package_writes_into_four_and_eight_byte_aligned_wrappers() {
        let expected = b"MRPGCMAPguest module";
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(expected).unwrap();
        let stored = encoder.finish().unwrap();
        let mut image = vec![0_u8; 24 + stored.len()];
        let image_len = image.len() as u32;
        image[..4].copy_from_slice(b"MRPG");
        image[4..8].copy_from_slice(&4_u32.to_le_bytes());
        image[8..12].copy_from_slice(&image_len.to_le_bytes());
        image[12..16].copy_from_slice(&4_u32.to_le_bytes());
        image[16..20].copy_from_slice(b"abc\0");
        image[20..24].copy_from_slice(&(stored.len() as u32).to_le_bytes());
        image[24..].copy_from_slice(&stored);

        for alignment in [4, 8] {
            let mut runtime =
                ExtRuntime::new(240, 320, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32)
                    .unwrap();
            if alignment == 4 {
                runtime.allocate(4, 4).unwrap();
            }
            let aligned_len = (expected.len() + 7) & !7;
            let prepared = runtime.allocate(aligned_len, alignment).unwrap();
            assert_eq!(prepared.0 % 8, if alignment == 4 { 4 } else { 0 });
            runtime.memory.write_u32(prepared, 0).unwrap();
            runtime
                .memory
                .write_u32(prepared.checked_add(4).unwrap(), aligned_len as u32)
                .unwrap();

            let package = runtime.allocate(image.len(), 8).unwrap();
            runtime.memory.write(package, &image).unwrap();
            let descriptor = runtime.allocate(8, 4).unwrap();
            runtime.memory.write_u32(descriptor, prepared.0).unwrap();
            runtime
                .memory
                .write_u32(descriptor.checked_add(4).unwrap(), aligned_len as u32)
                .unwrap();
            runtime
                .memory
                .write_u32(data_slot_address(104), package.0)
                .unwrap();
            runtime
                .memory
                .write_u32(data_slot_address(105), image.len() as u32)
                .unwrap();

            let name = runtime.allocate(4, 1).unwrap();
            runtime.memory.write(name, b"abc\0").unwrap();
            let output_len = runtime.allocate(4, 4).unwrap();
            let mut cpu = ArmCpu::new();
            cpu.set_register(0, name.0);
            cpu.set_register(1, output_len.0);
            runtime
                .dispatch(125, 0, &mut cpu, &mut StubServices)
                .unwrap();

            assert_eq!(cpu.register(0), prepared.0);
            assert_eq!(
                runtime.memory.read_u32(output_len).unwrap(),
                expected.len() as u32
            );
            assert_eq!(
                runtime.memory.read(prepared, expected.len()).unwrap(),
                expected
            );
        }
    }

    #[test]
    fn initializes_the_internal_runtime_state_subtable() {
        let runtime =
            ExtRuntime::new(240, 320, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
        let internal_table = runtime.memory.read_u32(table_slot_address(23)).unwrap();

        assert_eq!(internal_table, INTERNAL_TABLE_DATA.0);
        assert_eq!(
            runtime
                .memory
                .read_u32(INTERNAL_TABLE_DATA.checked_add(8).unwrap())
                .unwrap(),
            APPLICATION_STATE_DATA.0
        );
        assert_eq!(runtime.memory.read_u32(APPLICATION_STATE_DATA).unwrap(), 1);
        assert_eq!(
            runtime
                .memory
                .read_u32(INTERNAL_TABLE_DATA.checked_add(44).unwrap())
                .unwrap(),
            APPLICATION_STATE_DATA.0
        );
        assert_eq!(
            runtime
                .memory
                .read_u32(INTERNAL_TABLE_DATA.checked_add(16).unwrap())
                .unwrap(),
            LIFECYCLE_CALLBACK_DATA.0
        );
        assert_eq!(
            runtime
                .memory
                .read_u32(INTERNAL_TABLE_DATA.checked_add(20).unwrap())
                .unwrap(),
            TIMER_ACTIVE_DATA.0
        );
    }

    #[test]
    fn due_timer_is_consumed_without_clearing_the_guest_active_flag() {
        let mut runtime =
            ExtRuntime::new(240, 320, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
        runtime.timer_deadline = Some(Instant::now());
        runtime.memory.write_u32(TIMER_ACTIVE_DATA, 1).unwrap();

        assert!(runtime.take_due_timer().unwrap());
        assert_eq!(runtime.timer_deadline, None);
        assert_eq!(runtime.memory.read_u32(TIMER_ACTIVE_DATA).unwrap(), 1);
    }

    #[test]
    fn exposes_a_checked_restart_lifecycle_request() {
        let mut runtime = ExtRuntime::new(
            240,
            320,
            b"parent.mrp",
            b"start.mr",
            DEFAULT_HEAP_LEN as u32,
        )
        .unwrap();
        let callback = runtime.allocate(8, 4).unwrap();
        runtime.memory.write(callback, b"restart\0").unwrap();
        runtime
            .memory
            .write_u32(LIFECYCLE_CALLBACK_DATA, callback.0)
            .unwrap();
        runtime.memory.write_u32(APPLICATION_STATE_DATA, 3).unwrap();
        write_platform_string(&mut runtime.memory, PACKAGE_NAME_DATA, b"child.mrp").unwrap();
        write_platform_string(&mut runtime.memory, START_NAME_DATA, b"main.mr").unwrap();

        assert_eq!(
            runtime.lifecycle_request().unwrap(),
            Some(ExtLifecycleRequest::Restart {
                package: b"child.mrp".to_vec(),
                entry: b"main.mr".to_vec(),
            })
        );
    }

    #[test]
    fn exposes_the_configured_heap_to_the_guest() {
        let heap_len = 2 * 1024 * 1024;
        let runtime = ExtRuntime::new(240, 320, b"test.mrp", b"start.mr", heap_len).unwrap();

        assert_eq!(
            runtime.memory.read_u32(data_slot_address(108)).unwrap(),
            HEAP_BASE.0
        );
        assert_eq!(
            runtime.memory.read_u32(data_slot_address(109)).unwrap(),
            heap_len
        );
        assert_eq!(
            runtime.memory.read_u32(data_slot_address(110)).unwrap(),
            HEAP_BASE.0 + heap_len
        );
        assert_eq!(
            runtime.memory.read_u32(data_slot_address(111)).unwrap(),
            heap_len
        );
    }

    #[test]
    fn initializes_the_screen_bitmap_resource() {
        let runtime =
            ExtRuntime::new(240, 320, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
        let bitmap_table = GuestAddr(runtime.memory.read_u32(table_slot_address(95)).unwrap());
        let screen_bitmap = bitmap_table
            .checked_add(SCREEN_BITMAP_ID * BITMAP_ENTRY_SIZE)
            .unwrap();

        assert_eq!(runtime.memory.read_u16(screen_bitmap).unwrap(), 240);
        assert_eq!(
            runtime
                .memory
                .read_u16(screen_bitmap.checked_add(2).unwrap())
                .unwrap(),
            320
        );
        assert_eq!(
            runtime
                .memory
                .read_u32(screen_bitmap.checked_add(4).unwrap())
                .unwrap(),
            240 * 320 * 2
        );
        assert_eq!(
            runtime
                .memory
                .read_u32(screen_bitmap.checked_add(8).unwrap())
                .unwrap(),
            0
        );
        assert_eq!(
            runtime
                .memory
                .read_u32(screen_bitmap.checked_add(12).unwrap())
                .unwrap(),
            SCREEN_BASE.0
        );
    }

    #[test]
    fn platform_draw_reads_screen_updates_with_the_screen_stride() {
        let mut runtime =
            ExtRuntime::new(4, 3, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
        for (index, color) in (1_u16..=12).enumerate() {
            runtime
                .memory
                .write_u16(SCREEN_BASE.checked_add((index * 2) as u32).unwrap(), color)
                .unwrap();
        }

        let pixels = runtime
            .read_platform_draw_pixels(SCREEN_BASE, 1, 1, 2, 2)
            .unwrap();
        let colors = pixels
            .chunks_exact(2)
            .map(|pixel| u16::from_le_bytes([pixel[0], pixel[1]]))
            .collect::<Vec<_>>();

        assert_eq!(colors, vec![6, 7, 10, 11]);
    }

    #[test]
    fn rejects_a_compact_ram_package_with_an_out_of_range_payload() {
        let mut image = vec![0_u8; 24];
        image[..4].copy_from_slice(b"MRPG");
        image[4..8].copy_from_slice(&4_u32.to_le_bytes());
        image[8..12].copy_from_slice(&24_u32.to_le_bytes());
        image[12..16].copy_from_slice(&4_u32.to_le_bytes());
        image[16..20].copy_from_slice(b"abc\0");
        image[20..24].copy_from_slice(&1_u32.to_le_bytes());

        let mut runtime =
            ExtRuntime::new(240, 320, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
        let address = runtime.allocate(image.len(), 8).unwrap();
        runtime.memory.write(address, &image).unwrap();
        let error = runtime
            .read_ram_package_file(address, image.len(), b"abc")
            .unwrap_err();

        assert!(error.to_string().contains("exceeds declared length"));
    }
}
