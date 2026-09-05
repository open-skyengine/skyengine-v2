//! ARM/Thumb instruction execution and permission-checked guest memory.
//!
//! This crate owns CPU state, instruction decoding, and guest memory. Hosts
//! provide code and data mappings and drive execution with [`ArmCpu::step`].
//! Package loading, ABI dispatch, and platform services belong to the host.
//!
//! ```
//! use skyengine_arm::{ArmCpu, GuestAddr, GuestMemory, Permissions};
//!
//! let mut memory = GuestMemory::new();
//! memory.map_bytes(
//!     GuestAddr(0x1000),
//!     0xe3a0_002a_u32.to_le_bytes().to_vec(), // mov r0, #42
//!     Permissions::READ_EXECUTE,
//!     "code",
//! )?;
//! let mut cpu = ArmCpu::new();
//! cpu.set_pc(0x1000);
//! cpu.step(&mut memory)?;
//! assert_eq!(cpu.register(0), 42);
//! # Ok::<(), skyengine_arm::Error>(())
//! ```

mod cpu;
mod error;
mod memory;

pub use cpu::ArmCpu;
pub use error::{Error, Result};
pub use memory::{GuestAddr, GuestMemory, Permissions};
