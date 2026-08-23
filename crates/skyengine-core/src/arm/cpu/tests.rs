use super::*;
use crate::arm::Permissions;

fn code_memory(instructions: &[u32]) -> GuestMemory {
    let bytes = instructions
        .iter()
        .flat_map(|instruction| instruction.to_le_bytes())
        .collect();
    let mut memory = GuestMemory::new();
    memory
        .map_bytes(
            GuestAddr(0x1000),
            bytes,
            Permissions::READ_EXECUTE,
            "test code",
        )
        .unwrap();
    memory
        .map(
            GuestAddr(0x2000),
            0x100,
            Permissions::READ_WRITE,
            "test stack",
        )
        .unwrap();
    memory
}

fn thumb_code_memory(instructions: &[u16]) -> GuestMemory {
    let bytes = instructions
        .iter()
        .flat_map(|instruction| instruction.to_le_bytes())
        .collect();
    let mut memory = GuestMemory::new();
    memory
        .map_bytes(
            GuestAddr(0x1000),
            bytes,
            Permissions::READ_EXECUTE,
            "Thumb test code",
        )
        .unwrap();
    memory
}

#[test]
fn data_processing_updates_flags_and_honors_conditions() {
    let mut memory = code_memory(&[
        0xe3a0_0001,
        0xe280_0002,
        0xe350_0003,
        0x03a0_1007,
        0x13a0_1009,
    ]);
    let mut cpu = ArmCpu::new();
    cpu.set_pc(0x1000);
    for _ in 0..5 {
        cpu.step(&mut memory).unwrap();
    }
    assert_eq!(cpu.register(0), 3);
    assert_eq!(cpu.register(1), 7);
    assert!(cpu.zero);
}

#[test]
fn block_transfer_round_trips_a_push_and_pop() {
    let mut memory = code_memory(&[0xe3a0_3012, 0xe92d_4008, 0xe3a0_3000, 0xe8bd_4008]);
    let mut cpu = ArmCpu::new();
    cpu.set_pc(0x1000);
    cpu.set_register(13, 0x2100);
    cpu.set_register(14, 0x1234_5678);
    for _ in 0..4 {
        cpu.step(&mut memory).unwrap();
    }
    assert_eq!(cpu.register(3), 0x12);
    assert_eq!(cpu.register(14), 0x1234_5678);
    assert_eq!(cpu.register(13), 0x2100);
}

#[test]
fn long_multiply_supports_signed_results_and_accumulation() {
    let mut memory = code_memory(&[
        0xe0c1_c093, // smull r12, r1, r3, r0
        0xe0a5_4190, // umlal r4, r5, r0, r1
    ]);
    let mut cpu = ArmCpu::new();
    cpu.set_pc(0x1000);
    cpu.set_register(0, (-7_i32) as u32);
    cpu.set_register(3, 3);
    cpu.step(&mut memory).unwrap();
    assert_eq!(cpu.register(12), (-21_i32) as u32);
    assert_eq!(cpu.register(1), u32::MAX);

    cpu.set_register(0, u32::MAX);
    cpu.set_register(1, 2);
    cpu.set_register(4, 2);
    cpu.set_register(5, 1);
    cpu.step(&mut memory).unwrap();
    assert_eq!(cpu.register(4), 0);
    assert_eq!(cpu.register(5), 3);
}

#[test]
fn thumb_arithmetic_and_conditional_branch_follow_pipeline_pc() {
    let mut memory = thumb_code_memory(&[
        0x2001, // movs r0, #1
        0x3002, // adds r0, #2
        0x2803, // cmp r0, #3
        0xd000, // beq to current PC + 4
        0x2109, // skipped
        0x2107, // movs r1, #7
    ]);
    let mut cpu = ArmCpu::new();
    cpu.set_pc(0x1001);
    for _ in 0..5 {
        cpu.step(&mut memory).unwrap();
    }
    assert!(cpu.is_thumb());
    assert_eq!(cpu.register(0), 3);
    assert_eq!(cpu.register(1), 7);
    assert_eq!(cpu.pc(), GuestAddr(0x100c));
}

#[test]
fn thumb_semihosting_character_write_validates_its_input() {
    let mut memory = thumb_code_memory(&[0xdfab, 0xdfab, 0xdf00]);
    memory
        .map(
            GuestAddr(0x2000),
            1,
            Permissions::READ_WRITE,
            "semihosting character",
        )
        .unwrap();
    memory.write_u8(GuestAddr(0x2000), b'x').unwrap();
    let mut cpu = ArmCpu::new();
    cpu.set_pc(0x1001);
    cpu.set_register(0, 3);
    cpu.set_register(1, 0x2000);

    cpu.step(&mut memory).unwrap();
    assert_eq!(cpu.pc(), GuestAddr(0x1002));

    cpu.set_register(1, 0);
    assert!(matches!(cpu.step(&mut memory), Err(Error::ArmFault(_))));

    cpu.set_pc(0x1005);
    cpu.set_register(1, 0x2000);
    assert!(
        matches!(cpu.step(&mut memory), Err(Error::ArmFault(message)) if message.contains("unsupported Thumb-1 instruction"))
    );
}

#[test]
fn arm_semihosting_exit_is_reported_and_other_forms_still_fault() {
    let mut memory = code_memory(&[0xef12_3456, 0xef12_3455, 0xef12_3456]);
    let mut cpu = ArmCpu::new();
    cpu.set_pc(0x1000);
    cpu.set_register(0, 0x18);
    cpu.set_register(1, 0x0002_0026);

    cpu.step(&mut memory).unwrap();
    assert_eq!(cpu.take_semihosting_exit_reason(), Some(0x0002_0026));
    assert_eq!(cpu.take_semihosting_exit_reason(), None);

    assert!(matches!(
        cpu.step(&mut memory),
        Err(Error::ArmFault(message)) if message.contains("unsupported A32 instruction")
    ));

    cpu.set_register(0, 0x17);
    assert!(matches!(
        cpu.step(&mut memory),
        Err(Error::ArmFault(message)) if message.contains("unsupported A32 instruction")
    ));
}

#[test]
fn thumb_ldmia_preserves_a_loaded_base_register() {
    let mut memory = thumb_code_memory(&[
        0xca07, // ldmia r2!, {r0, r1, r2}
        0xca03, // ldmia r2!, {r0, r1}
    ]);
    memory
        .map(
            GuestAddr(0x2000),
            0x20,
            Permissions::READ_WRITE,
            "Thumb test data",
        )
        .unwrap();
    memory.write_u32(GuestAddr(0x2000), 0x1111_1111).unwrap();
    memory.write_u32(GuestAddr(0x2004), 0x2222_2222).unwrap();
    memory.write_u32(GuestAddr(0x2008), 0x200c).unwrap();
    memory.write_u32(GuestAddr(0x200c), 0x3333_3333).unwrap();
    memory.write_u32(GuestAddr(0x2010), 0x4444_4444).unwrap();

    let mut cpu = ArmCpu::new();
    cpu.set_pc(0x1001);
    cpu.set_register(2, 0x2000);
    cpu.step(&mut memory).unwrap();
    assert_eq!(cpu.register(0), 0x1111_1111);
    assert_eq!(cpu.register(1), 0x2222_2222);
    assert_eq!(cpu.register(2), 0x200c);

    cpu.step(&mut memory).unwrap();
    assert_eq!(cpu.register(0), 0x3333_3333);
    assert_eq!(cpu.register(1), 0x4444_4444);
    assert_eq!(cpu.register(2), 0x2014);
}
