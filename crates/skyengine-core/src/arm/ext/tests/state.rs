use std::{io::Write, time::Instant};

use flate2::{Compression, write::GzEncoder};

use super::*;

#[test]
fn guest_allocator_reuses_and_merges_freed_blocks() {
    let mut runtime =
        ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();

    let first = runtime.allocate_guest_block(24).unwrap().unwrap();
    let second = runtime.allocate_guest_block(16).unwrap().unwrap();
    runtime.free_guest_block(first, 24).unwrap();
    let reused = runtime.allocate_guest_block(16).unwrap().unwrap();
    assert_eq!(reused, first);

    runtime.free_guest_block(reused, 16).unwrap();
    runtime.free_guest_block(second, 16).unwrap();
    let heap = runtime.guest_heap_state().unwrap();
    let (blocks, terminator, _) = runtime.read_free_blocks(heap).unwrap();
    assert_eq!(
        blocks,
        [FreeBlock {
            offset: 0,
            len: DEFAULT_HEAP_LEN as u32,
        }]
    );
    assert_eq!(terminator, DEFAULT_HEAP_LEN as u32);
    assert_eq!(heap.free_left, DEFAULT_HEAP_LEN as u32);
}

#[test]
fn internal_guest_heap_allocations_can_be_freed_by_the_guest() {
    let mut runtime =
        ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();

    let output = runtime.allocate(32, 8).unwrap();

    assert_eq!(runtime.guest_allocations.get(&output.0), Some(&32));
    runtime.free_guest_block(output, 32).unwrap();
    assert!(!runtime.guest_allocations.contains_key(&output.0));
}

#[test]
fn guest_managed_allocations_can_be_freed_with_an_explicit_length() {
    let mut runtime =
        ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    let output = runtime.allocate_guest_block(24).unwrap().unwrap();
    runtime.guest_allocations.remove(&output.0);

    runtime.free_guest_block(output, 24).unwrap();

    let heap = runtime.guest_heap_state().unwrap();
    let (blocks, _, _) = runtime.read_free_blocks(heap).unwrap();
    assert_eq!(
        blocks,
        [FreeBlock {
            offset: 0,
            len: DEFAULT_HEAP_LEN as u32,
        }]
    );
}

#[test]
fn guest_allocator_returns_raw_addresses_and_reclaims_small_blocks() {
    let mut runtime =
        ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();

    let first = runtime.allocate_guest_block(1).unwrap().unwrap();
    let second = runtime.allocate_guest_block(15).unwrap().unwrap();
    runtime.free_guest_block(GuestAddr(1), 0).unwrap();
    runtime
        .free_guest_block(HEAP_BASE.checked_add(DEFAULT_HEAP_LEN as u32).unwrap(), 0)
        .unwrap();
    assert_eq!(first, HEAP_BASE);
    assert_eq!(second, HEAP_BASE.checked_add(8).unwrap());

    runtime.free_guest_block(first, 1).unwrap();
    assert_eq!(runtime.allocate_guest_block(1).unwrap(), Some(first));
    runtime.free_guest_block(second, usize::MAX).unwrap();
    runtime.free_guest_block(second, 15).unwrap();
    runtime.memory.write_u32(data_slot_address(108), 0).unwrap();
    runtime.memory.write_u32(data_slot_address(110), 0).unwrap();
    runtime.free_guest_block(GuestAddr(0x3b108), 0).unwrap();
}

#[test]
fn guest_allocator_ignores_platform_owned_memory() {
    let mut runtime =
        ExtRuntime::new(240, 320, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    runtime.memory.write(SCREEN_BASE, &[0xaa; 16]).unwrap();

    runtime.free_guest_block(SCREEN_BASE, 16).unwrap();

    assert_eq!(runtime.memory.read(SCREEN_BASE, 16).unwrap(), &[0xaa; 16]);
    let heap = runtime.guest_heap_state().unwrap();
    let (blocks, _, _) = runtime.read_free_blocks(heap).unwrap();
    assert_eq!(
        blocks,
        [FreeBlock {
            offset: 0,
            len: DEFAULT_HEAP_LEN as u32,
        }]
    );
}

#[test]
fn guest_allocator_accepts_an_acyclic_out_of_order_free_list() {
    let mut runtime =
        ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    let span = DEFAULT_HEAP_LEN as u32;
    for (offset, next, len) in [(0x20, 0, 8), (0, 0x28, 0x10), (0x28, span, span - 0x28)] {
        let address = HEAP_BASE.checked_add(offset).unwrap();
        runtime.memory.write_u32(address, next).unwrap();
        runtime
            .memory
            .write_u32(address.checked_add(4).unwrap(), len)
            .unwrap();
    }
    runtime
        .memory
        .write_u32(data_slot_address(146), 0x20)
        .unwrap();

    let heap = runtime.guest_heap_state().unwrap();
    let (blocks, terminator, _) = runtime.read_free_blocks(heap).unwrap();
    assert_eq!(
        blocks,
        [
            FreeBlock {
                offset: 0x20,
                len: 8
            },
            FreeBlock {
                offset: 0,
                len: 0x10
            },
            FreeBlock {
                offset: 0x28,
                len: span - 0x28
            }
        ]
    );
    assert_eq!(terminator, span);
    assert_eq!(
        runtime.allocate_guest_block(8).unwrap(),
        Some(HEAP_BASE.checked_add(0x20).unwrap())
    );
}

#[test]
fn guest_allocator_does_not_attach_a_discarded_prefix_to_the_returned_block() {
    let mut runtime =
        ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    let span = DEFAULT_HEAP_LEN as u32;
    runtime
        .memory
        .write_u32(HEAP_BASE.checked_add(4).unwrap(), span)
        .unwrap();
    runtime
        .memory
        .write_u32(HEAP_BASE.checked_add(8).unwrap(), span - 4)
        .unwrap();
    runtime.memory.write_u32(data_slot_address(146), 4).unwrap();
    runtime
        .memory
        .write_u32(data_slot_address(111), span - 4)
        .unwrap();

    let output = runtime.allocate_guest_block(1).unwrap().unwrap();

    assert_eq!(output, HEAP_BASE.checked_add(8).unwrap());
    assert_eq!(runtime.guest_allocations.get(&output.0), Some(&8));
    runtime.free_guest_block(output, 1).unwrap();
    let heap = runtime.guest_heap_state().unwrap();
    let (blocks, _, _) = runtime.read_free_blocks(heap).unwrap();
    assert_eq!(
        blocks,
        [FreeBlock {
            offset: 8,
            len: span - 8,
        }]
    );
}

#[test]
fn guest_allocator_reconciles_tracking_after_guest_heap_reset() {
    let mut runtime =
        ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    let first = runtime.allocate_guest_block(16).unwrap().unwrap();
    assert!(runtime.guest_allocations.contains_key(&first.0));

    runtime
        .memory
        .write_u32(HEAP_BASE, DEFAULT_HEAP_LEN as u32)
        .unwrap();
    runtime
        .memory
        .write_u32(HEAP_BASE.checked_add(4).unwrap(), DEFAULT_HEAP_LEN as u32)
        .unwrap();
    runtime.memory.write_u32(data_slot_address(146), 0).unwrap();
    runtime
        .memory
        .write_u32(data_slot_address(111), DEFAULT_HEAP_LEN as u32)
        .unwrap();

    let reused = runtime.allocate_guest_block(8).unwrap().unwrap();
    assert_eq!(reused, first);
    assert_eq!(runtime.guest_allocations.get(&reused.0), Some(&8));
    runtime.free_guest_block(reused, 8).unwrap();
}

#[test]
fn guest_allocator_excludes_an_active_ram_package_that_overwrites_a_free_header() {
    let mut runtime =
        ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    let first = runtime.allocate_guest_block(24).unwrap().unwrap();
    let free_head = runtime.read_platform_data_slot(146).unwrap();
    let header = HEAP_BASE.checked_add(free_head).unwrap();
    runtime
        .memory
        .write_u32(data_slot_address(104), header.0)
        .unwrap();
    runtime
        .memory
        .write_u32(data_slot_address(105), 32)
        .unwrap();
    runtime.memory.write(header, &[0xaa; 32]).unwrap();

    let second = runtime.allocate_guest_block(8).unwrap().unwrap();

    assert_eq!(first, HEAP_BASE);
    assert_eq!(second, HEAP_BASE.checked_add(56).unwrap());
    let heap = runtime.guest_heap_state().unwrap();
    let (blocks, terminator, _) = runtime.read_free_blocks(heap).unwrap();
    assert_eq!(
        blocks,
        [FreeBlock {
            offset: 64,
            len: DEFAULT_HEAP_LEN as u32 - 64,
        }]
    );
    assert_eq!(terminator, DEFAULT_HEAP_LEN as u32);
}

#[test]
fn guest_allocator_follows_staged_and_switched_heap_variables() {
    let mut runtime =
        ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    let initial_span = DEFAULT_HEAP_LEN as u32;
    let staged_free_left = initial_span + 0x100;

    runtime
        .memory
        .write_u32(data_slot_address(110), PLATFORM_MEMORY_BASE.0 + 0x100)
        .unwrap();
    runtime
        .memory
        .write_u32(data_slot_address(111), staged_free_left)
        .unwrap();
    assert_eq!(runtime.allocate_guest_block(16).unwrap(), Some(HEAP_BASE));
    let staged = runtime.guest_heap_state().unwrap();
    let (_, staged_terminator, _) = runtime.read_free_blocks(staged).unwrap();
    assert_eq!(staged_terminator, initial_span);
    assert_eq!(staged.free_left, staged_free_left - 16);

    runtime
        .memory
        .map(
            PLATFORM_MEMORY_BASE,
            0x100,
            Permissions::READ_WRITE,
            "test external arena",
        )
        .unwrap();
    runtime
        .memory
        .write_u32(PLATFORM_MEMORY_BASE, 0x100)
        .unwrap();
    runtime
        .memory
        .write_u32(PLATFORM_MEMORY_BASE.checked_add(4).unwrap(), 0x100)
        .unwrap();
    runtime
        .memory
        .write_u32(data_slot_address(108), PLATFORM_MEMORY_BASE.0)
        .unwrap();
    runtime
        .memory
        .write_u32(data_slot_address(110), PLATFORM_MEMORY_BASE.0 + 0x100)
        .unwrap();
    runtime
        .memory
        .write_u32(data_slot_address(111), 0x100)
        .unwrap();
    runtime.memory.write_u32(data_slot_address(146), 0).unwrap();

    assert_eq!(
        runtime.allocate_guest_block(16).unwrap(),
        Some(PLATFORM_MEMORY_BASE)
    );
    assert_eq!(runtime.read_platform_data_slot(146).unwrap(), 16);
    assert_eq!(runtime.read_platform_data_slot(111).unwrap(), 0xf0);
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

    cpu.set_register(0, 2_011);
    cpu.set_register(1, 0);
    cpu.set_register(2, 0);
    cpu.set_register(3, 0);
    runtime
        .dispatch(38, 0, &mut cpu, &mut StubServices)
        .unwrap();
    assert_eq!(cpu.register(0), 0);

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
fn package_file_buffers_are_guest_owned_and_can_be_freed() {
    let mut runtime =
        ExtRuntime::new(240, 320, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    let name = runtime.allocate(10, 1).unwrap();
    runtime.memory.write(name, b"owned.bin\0").unwrap();
    let output_len = runtime.allocate(4, 4).unwrap();
    let mut cpu = ArmCpu::new();
    cpu.set_register(0, name.0);
    cpu.set_register(1, output_len.0);

    runtime
        .dispatch(125, 0, &mut cpu, &mut StubServices)
        .unwrap();

    let output = GuestAddr(cpu.register(0));
    assert_eq!(runtime.memory.read(output, 11).unwrap(), b"guest-owned");
    assert_eq!(runtime.memory.read_u32(output_len).unwrap(), 11);
    assert!(runtime.guest_allocations.contains_key(&output.0));

    cpu.set_register(0, output.0);
    runtime.dispatch(1, 0, &mut cpu, &mut StubServices).unwrap();
    assert_eq!(cpu.register(0), 0);
    assert!(!runtime.guest_allocations.contains_key(&output.0));
}

#[test]
fn package_file_read_uses_detached_allocation_after_guest_heap_teardown() {
    let mut runtime =
        ExtRuntime::new(240, 320, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    let name = runtime.allocate(10, 1).unwrap();
    runtime.memory.write(name, b"owned.bin\0").unwrap();
    let output_len = runtime.allocate(4, 4).unwrap();
    runtime.memory.write_u32(output_len, u32::MAX).unwrap();
    runtime.memory.write_u32(data_slot_address(108), 0).unwrap();
    runtime.memory.write_u32(data_slot_address(110), 0).unwrap();

    let mut cpu = ArmCpu::new();
    cpu.set_register(0, name.0);
    cpu.set_register(1, output_len.0);
    runtime
        .dispatch(125, 0, &mut cpu, &mut StubServices)
        .unwrap();

    let output = GuestAddr(cpu.register(0));
    assert_eq!(output, DETACHED_GUEST_ALLOCATION_BASE);
    assert_eq!(runtime.memory.read(output, 11).unwrap(), b"guest-owned");
    assert_eq!(runtime.memory.read_u32(output_len).unwrap(), 11);
    assert!(runtime.detached_guest_allocations.contains_key(&output.0));

    cpu.set_register(0, output.0);
    runtime.dispatch(1, 0, &mut cpu, &mut StubServices).unwrap();
    assert!(!runtime.detached_guest_allocations.contains_key(&output.0));
    assert!(runtime.memory.read(output, 1).is_err());
}

#[test]
fn internal_allocator_uses_detached_memory_after_guest_heap_teardown() {
    let mut runtime =
        ExtRuntime::new(240, 320, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    runtime.memory.write_u32(data_slot_address(108), 0).unwrap();
    runtime.memory.write_u32(data_slot_address(110), 0).unwrap();

    let output = runtime.allocate(5, 4).unwrap();

    assert_eq!(output, DETACHED_GUEST_ALLOCATION_BASE);
    assert_eq!(runtime.memory.read(output, 8).unwrap(), vec![0; 8]);
    runtime.free_guest_block(output, 5).unwrap();
    assert!(runtime.memory.read(output, 1).is_err());
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
            ExtRuntime::new(240, 320, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
        if alignment == 4 {
            runtime.allocate(4, 4).unwrap();
        }
        let aligned_len = (expected.len() + 7) & !7;
        let prepared = runtime.allocate(aligned_len, alignment).unwrap();
        assert_eq!(prepared.0 % 8, if alignment == 4 { 4 } else { 0 });
        runtime
            .memory
            .write_u32(prepared, if alignment == 4 { 0 } else { 0x40 })
            .unwrap();
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
fn compact_ram_package_accepts_a_prepared_platform_memory_target() {
    let mut runtime =
        ExtRuntime::new(240, 320, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    runtime
        .set_device_info_profile(DeviceInfoProfile::DeterministicMtk)
        .unwrap();
    let output_len = 32_u32;
    runtime
        .memory
        .write_u32(MTK_NATIVE_EXTENSION_BASE, 0)
        .unwrap();
    runtime
        .memory
        .write_u32(
            MTK_NATIVE_EXTENSION_BASE.checked_add(4).unwrap(),
            output_len,
        )
        .unwrap();

    let descriptor = runtime.allocate(8, 4).unwrap();
    runtime
        .memory
        .write_u32(descriptor, MTK_NATIVE_EXTENSION_BASE.0)
        .unwrap();
    runtime
        .memory
        .write_u32(descriptor.checked_add(4).unwrap(), output_len)
        .unwrap();
    let package = runtime.allocate(24, 8).unwrap();
    let mut compact_header = [0_u8; 24];
    compact_header[..4].copy_from_slice(b"MRPG");
    compact_header[4..8].copy_from_slice(&4_u32.to_le_bytes());
    compact_header[12..16].copy_from_slice(&4_u32.to_le_bytes());
    runtime.memory.write(package, &compact_header).unwrap();

    assert_eq!(
        runtime
            .compact_ram_output_target(package, compact_header.len(), output_len as usize)
            .unwrap(),
        Some(MTK_NATIVE_EXTENSION_BASE)
    );
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
