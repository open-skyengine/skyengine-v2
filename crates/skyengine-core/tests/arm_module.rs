use skyengine_core::arm::{ArmCpu, GuestAddr, GuestMemory, Permissions};
use skyengine_core::{Error, Result};

#[test]
fn reexported_cpu_and_memory_interoperate_with_the_standalone_crate() -> Result<()> {
    let mut memory: GuestMemory = skyengine_arm::GuestMemory::new();
    memory.map_bytes(
        GuestAddr(0x1000),
        0xe3a0_002a_u32.to_le_bytes().to_vec(), // mov r0, #42
        Permissions::READ_EXECUTE,
        "code",
    )?;
    let mut cpu: ArmCpu = skyengine_arm::ArmCpu::new();
    cpu.set_pc(0x1000);
    cpu.step(&mut memory)?;
    assert_eq!(cpu.register(0), 42);
    assert_eq!(cpu.pc(), skyengine_arm::GuestAddr(0x1004));
    Ok(())
}

#[test]
fn cpu_and_memory_faults_preserve_core_error_kind_and_message() {
    fn fetch_unmapped_instruction() -> Result<()> {
        let mut cpu = ArmCpu::new();
        cpu.set_pc(0x1000);
        cpu.step(&mut GuestMemory::new())?;
        Ok(())
    }

    let error = fetch_unmapped_instruction().unwrap_err();
    assert!(
        matches!(&error, Error::ArmFault(message) if message.contains("unmapped guest access"))
    );
    assert_eq!(
        error.to_string(),
        "ARM fault: unmapped guest access at 0x00001000 (4 bytes)"
    );

    let memory = GuestMemory::new();
    let arm_error = memory.read_u32(GuestAddr(0x2000)).unwrap_err();
    let message = arm_error.to_string();
    let core_error = Error::from(arm_error);
    assert!(matches!(core_error, Error::ArmFault(_)));
    assert_eq!(core_error.to_string(), message);
}
