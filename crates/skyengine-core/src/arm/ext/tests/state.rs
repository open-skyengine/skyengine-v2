use std::{io::Write, time::Instant};

use flate2::{Compression, write::GzEncoder};

use super::*;

#[test]
fn legacy_keypad_registers_default_to_idle_and_track_key_events() {
    let mut runtime =
        ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();

    for (code, offset, mask) in [(0, 4, 1_u16), (17, 8, 2_u16), (41, 12, 512_u16)] {
        let register = LEGACY_KEYPAD_REGISTERS.checked_add(offset).unwrap();
        assert_eq!(runtime.memory.read_u16(register).unwrap(), u16::MAX);
        runtime
            .route_key_event(code, true, &mut StubServices)
            .unwrap();
        assert_eq!(runtime.memory.read_u16(register).unwrap(), !mask);
        runtime
            .route_key_event(code, false, &mut StubServices)
            .unwrap();
        assert_eq!(runtime.memory.read_u16(register).unwrap(), u16::MAX);
    }
}

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
fn libc_free_and_realloc_reject_allocations_owned_by_another_module() {
    let mut runtime =
        ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    load_test_module(&mut runtime);
    load_test_module(&mut runtime);

    let mut allocate = ArmCpu::new();
    allocate.set_register(0, 32);
    runtime.dispatch_libc(0, 0, &mut allocate).unwrap();
    let address = allocate.register(0);

    let mut foreign_free = ArmCpu::new();
    foreign_free.set_register(0, address);
    foreign_free.set_register(1, 32);
    let error = runtime.dispatch_libc(1, 1, &mut foreign_free).unwrap_err();
    assert!(error.to_string().contains("owned by another module"));
    assert!(runtime.guest_allocations.contains_key(&address));

    foreign_free.set_register(0, address + 8);
    foreign_free.set_register(1, 8);
    let error = runtime.dispatch_libc(1, 1, &mut foreign_free).unwrap_err();
    assert!(error.to_string().contains("owned by another module"));
    assert!(runtime.guest_allocations.contains_key(&address));

    let mut foreign_realloc = ArmCpu::new();
    foreign_realloc.set_register(0, address);
    foreign_realloc.set_register(1, 32);
    foreign_realloc.set_register(2, 16);
    let error = runtime
        .dispatch_libc(2, 1, &mut foreign_realloc)
        .unwrap_err();
    assert!(error.to_string().contains("owned by another module"));
    assert!(runtime.guest_allocations.contains_key(&address));

    let mut interior_owner_free = ArmCpu::new();
    interior_owner_free.set_register(0, address + 8);
    interior_owner_free.set_register(1, 8);
    let error = runtime
        .dispatch_libc(1, 0, &mut interior_owner_free)
        .unwrap_err();
    assert!(error.to_string().contains("start of a tracked allocation"));
    assert!(runtime.guest_allocations.contains_key(&address));

    let mut owner_free = ArmCpu::new();
    owner_free.set_register(0, address);
    owner_free.set_register(1, 32);
    runtime.dispatch_libc(1, 0, &mut owner_free).unwrap();
    assert!(!runtime.guest_allocations.contains_key(&address));
}

#[test]
fn owned_allocation_suffix_free_reconciles_tracking_at_the_free_list_boundary() {
    let heap_len = 1024 * 1024;
    let mut runtime = ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", heap_len).unwrap();
    load_test_module(&mut runtime);
    load_test_module(&mut runtime);
    let backing_len = 0x1000;
    let suffix_request_len = 0x815;
    let suffix_len = 0x818;
    let retained_len = backing_len - suffix_len;
    let backing = runtime
        .allocate_guest_block_for_module(backing_len as usize, 0)
        .unwrap()
        .unwrap();
    let suffix = backing.checked_add(retained_len).unwrap();
    let blocker = runtime
        .allocate_guest_block_for_module(0x80, 0)
        .unwrap()
        .unwrap();
    assert_eq!(blocker, backing.checked_add(backing_len).unwrap());
    let owner_generation = runtime.modules[0].generation;
    let heap_before = runtime.guest_heap_state().unwrap();
    let allocations_before = runtime.guest_allocations.clone();
    let owners_before = runtime.guest_allocation_owners.clone();

    assert!(matches!(
        runtime.free_guest_block_for_module(suffix, suffix_request_len as usize, 0),
        Err(Error::Abi(message)) if message.contains("start of a tracked allocation")
    ));
    assert_eq!(runtime.guest_allocations, allocations_before);
    assert_eq!(runtime.guest_allocation_owners, owners_before);
    let heap_after_blocked_free = runtime.guest_heap_state().unwrap();
    assert_eq!(heap_after_blocked_free.head, heap_before.head);
    assert_eq!(heap_after_blocked_free.free_left, heap_before.free_left);

    assert!(matches!(
        runtime.free_guest_block_for_module(suffix, suffix_request_len as usize, 1),
        Err(Error::Abi(message)) if message.contains("owned by another module")
    ));
    assert_eq!(runtime.guest_allocations, allocations_before);
    let heap_after_foreign_free = runtime.guest_heap_state().unwrap();
    assert_eq!(heap_after_foreign_free.head, heap_before.head);
    assert_eq!(heap_after_foreign_free.free_left, heap_before.free_left);

    runtime
        .free_guest_block_for_module(blocker, 0x80, 0)
        .unwrap();
    runtime
        .free_guest_block_for_module(suffix, suffix_request_len as usize, 0)
        .unwrap();

    assert_eq!(
        runtime.guest_allocations.get(&backing.0),
        Some(&retained_len)
    );
    assert_eq!(
        runtime.guest_allocation_owners.get(&backing.0),
        Some(&owner_generation)
    );
    let heap = runtime.guest_heap_state().unwrap();
    let (blocks, terminator, recovered_len) = runtime.read_free_blocks(heap).unwrap();
    assert_eq!(
        blocks,
        [FreeBlock {
            offset: suffix.0 - heap.base,
            len: heap.span - (suffix.0 - heap.base),
        }]
    );
    assert_eq!(terminator, heap.span);
    assert_eq!(recovered_len, 0);
    assert_eq!(heap.free_left, heap.span - (suffix.0 - heap.base));

    assert!(matches!(
        runtime.free_guest_block_for_module(backing, retained_len as usize, 1),
        Err(Error::Abi(message)) if message.contains("owned by another module")
    ));

    let reused = runtime
        .allocate_guest_block_for_module(suffix_request_len as usize, 1)
        .unwrap()
        .unwrap();
    assert_eq!(reused, suffix);
    assert_eq!(
        runtime.guest_allocations.get(&backing.0),
        Some(&retained_len)
    );
    assert_eq!(
        runtime.guest_allocation_owners.get(&backing.0),
        Some(&owner_generation)
    );
    assert_eq!(runtime.guest_allocations.get(&reused.0), Some(&suffix_len));
    assert_eq!(
        runtime.guest_allocation_owners.get(&reused.0),
        Some(&runtime.modules[1].generation)
    );
    assert!(matches!(
        runtime.free_guest_block_for_module(reused, suffix_request_len as usize, 0),
        Err(Error::Abi(message)) if message.contains("owned by another module")
    ));
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
fn explicit_free_length_reconciles_a_stale_host_allocation_extent() {
    let mut runtime =
        ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    let output = runtime.allocate_guest_block(0x100).unwrap().unwrap();
    let span = DEFAULT_HEAP_LEN as u32;

    // The guest allocator returns the tail of a host allocation to its own
    // free-list, so the host's original extent is now stale.
    runtime
        .memory
        .write_u32(output.checked_add(0x80).unwrap(), span)
        .unwrap();
    runtime
        .memory
        .write_u32(output.checked_add(0x84).unwrap(), span - 0x80)
        .unwrap();
    runtime
        .memory
        .write_u32(data_slot_address(146), 0x80)
        .unwrap();
    runtime
        .memory
        .write_u32(data_slot_address(111), span - 0x80)
        .unwrap();

    runtime.free_guest_block(output, 0x80).unwrap();

    let heap = runtime.guest_heap_state().unwrap();
    let (blocks, _, _) = runtime.read_free_blocks(heap).unwrap();
    assert_eq!(
        blocks,
        [FreeBlock {
            offset: 0,
            len: span,
        }]
    );
    assert_eq!(heap.free_left, span);
    assert!(!runtime.guest_allocations.contains_key(&output.0));
}

#[test]
fn explicit_free_length_cannot_hide_a_partial_free_list_overlap() {
    let mut runtime =
        ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    let output = runtime.allocate_guest_block(0x100).unwrap().unwrap();
    let span = DEFAULT_HEAP_LEN as u32;
    runtime
        .memory
        .write_u32(output.checked_add(0x80).unwrap(), span)
        .unwrap();
    runtime
        .memory
        .write_u32(output.checked_add(0x84).unwrap(), span - 0x80)
        .unwrap();
    runtime
        .memory
        .write_u32(data_slot_address(146), 0x80)
        .unwrap();
    runtime
        .memory
        .write_u32(data_slot_address(111), span - 0x80)
        .unwrap();

    let error = runtime.free_guest_block(output, 0x90).unwrap_err();

    assert!(matches!(error, Error::Abi(message) if message.contains("overlaps free block")));
    assert!(runtime.guest_allocations.contains_key(&output.0));
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
            .read_u16(runtime.screen_address(0, 294, 240).unwrap())
            .unwrap(),
        Framebuffer::rgb565(0, 252, 0)
    );

    cpu.set_register(0, handle);
    runtime
        .dispatch(70, 0, &mut cpu, &mut StubServices)
        .unwrap();
    assert_eq!(cpu.register(0), 0);
    assert_eq!(
        runtime
            .memory
            .read_u16(runtime.screen_address(0, 294, 240).unwrap())
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
fn platform_menu_create_captures_title_items_and_uses_shared_ui_handles() {
    let mut runtime =
        ExtRuntime::new(240, 320, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    runtime.dialogs.insert(
        1,
        PlatformDialog {
            previous_screen: Vec::new(),
            dialog_screen: Vec::new(),
        },
    );
    let title = runtime.allocate(8, 2).unwrap();
    runtime
        .memory
        .write(title, &[0x6e, 0x38, 0x62, 0x0f, 0, 0, 0, 0])
        .unwrap();
    let mut cpu = ArmCpu::new();
    cpu.set_register(0, title.0);
    cpu.set_register(1, 2);

    runtime
        .dispatch(63, 0, &mut cpu, &mut StubServices)
        .unwrap();

    let handle = cpu.register(0);
    assert_eq!(handle, 2);
    let menu = runtime.menus.get(&handle).unwrap();
    assert_eq!(menu.title, [0x6e38, 0x620f]);
    assert_eq!(menu.items, [None, None]);
    assert_eq!(menu.focused_item, 0);
    assert_eq!(menu.first_visible_item, 0);
    assert_eq!(menu.previous_screen, None);
    assert_eq!(menu.menu_screen, None);
    assert!(matches!(
        runtime.create_platform_menu(Vec::new(), MAX_PLATFORM_MENU_ITEMS + 1),
        Err(Error::ResourceLimit(_))
    ));
}

#[test]
fn platform_ui_live_handle_limit_is_shared_and_checked_before_side_effects() {
    let mut runtime =
        ExtRuntime::new(240, 320, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    load_test_module(&mut runtime);
    let handles = (0..MAX_PLATFORM_UI_HANDLES)
        .map(|_| runtime.create_platform_menu(Vec::new(), 0).unwrap())
        .collect::<Vec<_>>();
    let active = *handles.last().unwrap();
    runtime
        .active_platform_ui
        .push(ActivePlatformUi::Menu(active));
    runtime.pending_platform_menu_selection = Some(active);
    runtime.memory.write_u16(SCREEN_BASE, 0x1234).unwrap();

    assert!(matches!(
        runtime.create_platform_dialog(&[], &[], 0, &mut StubServices),
        Err(Error::ResourceLimit(message))
            if message.contains("platform UI") && message.contains("limit 64")
    ));
    assert!(matches!(
        runtime.create_platform_text_viewer(&[], &[], 2, &mut StubServices),
        Err(Error::ResourceLimit(_))
    ));
    assert!(matches!(
        runtime.create_platform_editor(0, Vec::new(), Vec::new(), 0, 32),
        Err(Error::ResourceLimit(_))
    ));
    assert!(matches!(
        runtime.create_native_window(0),
        Err(Error::ResourceLimit(_))
    ));
    assert_eq!(runtime.memory.read_u16(SCREEN_BASE).unwrap(), 0x1234);
    assert_eq!(runtime.active_platform_ui, [ActivePlatformUi::Menu(active)]);
    assert_eq!(runtime.pending_platform_menu_selection, Some(active));
    assert!(!runtime.menus[&active].modal_detached);
    assert!(runtime.dialogs.is_empty());
    assert!(runtime.text_viewers.is_empty());

    assert!(
        runtime
            .release_platform_menu(handles[0], &mut StubServices)
            .unwrap()
    );
    let replacement = runtime.create_native_window(0).unwrap();
    assert_ne!(replacement, 0);
    assert_eq!(runtime.menus.len(), MAX_PLATFORM_UI_HANDLES - 1);
    assert_eq!(runtime.native_windows.len(), 1);
}

#[test]
fn platform_menu_show_draws_focus_and_captures_the_previous_screen() {
    let mut runtime =
        ExtRuntime::new(240, 320, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    runtime.memory.write_u16(SCREEN_BASE, 0x1234).unwrap();
    let handle = runtime
        .create_platform_menu(vec![0x83dc, 0x5355], 2)
        .unwrap();
    runtime.menus.get_mut(&handle).unwrap().items =
        vec![Some(vec![0x7b2c, 0x4e00]), Some(vec![0x7b2c, 0x4e8c])];
    let mut cpu = ArmCpu::new();
    cpu.set_register(0, handle);

    runtime
        .dispatch(65, 0, &mut cpu, &mut StubServices)
        .unwrap();

    assert_eq!(cpu.register(0), 0);
    assert_eq!(runtime.active_platform_ui, [ActivePlatformUi::Menu(handle)]);
    let menu = runtime.menus.get(&handle).unwrap();
    assert_eq!(
        u16::from_le_bytes(
            menu.previous_screen.as_ref().unwrap()[..2]
                .try_into()
                .unwrap()
        ),
        0x1234
    );
    assert_eq!(
        runtime
            .memory
            .read_u16(runtime.screen_address(144, 50, 240).unwrap())
            .unwrap(),
        Framebuffer::rgb565(0, 0, 248)
    );
    assert_eq!(
        runtime
            .memory
            .read_u16(runtime.screen_address(0, 294, 240).unwrap())
            .unwrap(),
        Framebuffer::rgb565(0, 252, 0)
    );
    assert_eq!(menu.menu_screen.as_ref().unwrap().len(), 240 * 320 * 2);

    cpu.set_register(0, handle + 1);
    runtime
        .dispatch(65, 0, &mut cpu, &mut StubServices)
        .unwrap();
    assert_eq!(cpu.register(0), u32::MAX);
    assert_eq!(runtime.active_platform_ui, [ActivePlatformUi::Menu(handle)]);
}

#[test]
fn platform_menu_restores_the_presented_framebuffer_instead_of_stale_screen_memory() {
    let mut runtime =
        ExtRuntime::new(240, 320, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    let mut host_screen = vec![0_u8; 240 * 320 * 2];
    host_screen[..2].copy_from_slice(&0x5678_u16.to_le_bytes());
    let _capture = capture_stub_framebuffer(host_screen.clone());
    let mut services = StubServices;
    let handle = runtime
        .create_platform_menu(vec![0x83dc, 0x5355], 1)
        .unwrap();
    runtime.menus.get_mut(&handle).unwrap().items = vec![Some(vec![0x9879])];

    runtime.show_platform_menu(handle, &mut services).unwrap();

    assert_eq!(
        runtime.menus[&handle].previous_screen,
        Some(host_screen.clone())
    );
    assert_eq!(runtime.memory.read_u16(SCREEN_BASE).unwrap(), 0);
    assert_eq!(
        runtime.route_key_event(18, true, &mut services).unwrap(),
        Some((5, 0, 0))
    );
    assert_eq!(runtime.memory.read_u16(SCREEN_BASE).unwrap(), 0);
    assert!(
        runtime
            .release_platform_menu(handle, &mut services)
            .unwrap()
    );
    assert_eq!(runtime.memory.read_u16(SCREEN_BASE).unwrap(), 0x5678);
    assert_eq!(
        STUB_FRAMEBUFFER.with(|framebuffer| framebuffer.borrow().clone()),
        Some(host_screen)
    );
}

#[test]
fn platform_menu_set_item_requires_the_exact_handle_and_index() {
    let mut runtime =
        ExtRuntime::new(240, 320, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    let handle = runtime
        .create_platform_menu(vec![0x83dc, 0x5355], 2)
        .unwrap();
    let text = runtime.allocate(6, 2).unwrap();
    runtime
        .memory
        .write(
            text,
            &[0x9009, 0x62e9, 0]
                .into_iter()
                .flat_map(u16::to_be_bytes)
                .collect::<Vec<_>>(),
        )
        .unwrap();
    let mut cpu = ArmCpu::new();
    cpu.set_register(0, handle);
    cpu.set_register(1, text.0);
    cpu.set_register(2, 1);

    runtime
        .dispatch(64, 0, &mut cpu, &mut StubServices)
        .unwrap();
    assert_eq!(cpu.register(0), 0);
    assert_eq!(
        runtime.menus[&handle].items,
        [None, Some(vec![0x9009, 0x62e9])]
    );

    cpu.set_register(0, handle + 1);
    runtime
        .dispatch(64, 0, &mut cpu, &mut StubServices)
        .unwrap();
    assert_eq!(cpu.register(0), u32::MAX);
    cpu.set_register(0, handle);
    cpu.set_register(2, 2);
    runtime
        .dispatch(64, 0, &mut cpu, &mut StubServices)
        .unwrap();
    assert_eq!(cpu.register(0), u32::MAX);
    assert_eq!(
        runtime.menus[&handle].items,
        [None, Some(vec![0x9009, 0x62e9])]
    );
}

#[test]
fn platform_menu_routes_keys_to_focus_select_and_return() {
    let mut runtime =
        ExtRuntime::new(240, 320, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    let handle = runtime
        .create_platform_menu(vec![0x83dc, 0x5355], 2)
        .unwrap();
    runtime.menus.get_mut(&handle).unwrap().items =
        vec![Some(vec![0x7b2c, 0x4e00]), Some(vec![0x7b2c, 0x4e8c])];
    let mut services = StubServices;
    runtime.show_platform_menu(handle, &mut services).unwrap();

    assert_eq!(
        runtime.route_key_event(13, true, &mut services).unwrap(),
        None
    );
    assert_eq!(runtime.menus[&handle].focused_item, 1);
    assert_eq!(
        runtime
            .memory
            .read_u16(runtime.screen_address(144, 50, 240).unwrap())
            .unwrap(),
        Framebuffer::rgb565(0, 0, 0)
    );
    assert_eq!(
        runtime
            .memory
            .read_u16(runtime.screen_address(144, 74, 240).unwrap())
            .unwrap(),
        Framebuffer::rgb565(0, 0, 248)
    );
    assert_eq!(
        runtime.route_key_event(13, false, &mut services).unwrap(),
        None
    );
    assert_eq!(
        runtime.route_key_event(20, true, &mut services).unwrap(),
        Some((4, 1, 0))
    );
    assert_eq!(
        runtime.route_key_event(20, false, &mut services).unwrap(),
        None
    );
    assert_eq!(
        runtime.route_key_event(18, true, &mut services).unwrap(),
        Some((5, 0, 0))
    );
    assert_eq!(runtime.active_platform_ui, [ActivePlatformUi::Menu(handle)]);
    assert_eq!(
        runtime.memory.read(SCREEN_BASE, 240 * 320 * 2).unwrap(),
        runtime.menus[&handle].menu_screen.clone().unwrap()
    );
    assert!(
        runtime
            .release_platform_menu(handle, &mut services)
            .unwrap()
    );
    assert!(runtime.active_platform_ui.is_empty());
    assert_eq!(runtime.memory.read_u16(SCREEN_BASE).unwrap(), 0);
}

#[test]
fn platform_menu_pointer_selects_the_hit_item_and_softkey_half() {
    let mut runtime =
        ExtRuntime::new(240, 320, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    let handle = runtime
        .create_platform_menu(vec![0x83dc, 0x5355], 2)
        .unwrap();
    runtime.menus.get_mut(&handle).unwrap().items =
        vec![Some(vec![0x7b2c, 0x4e00]), Some(vec![0x7b2c, 0x4e8c])];
    runtime.menus.get_mut(&handle).unwrap().focused_item = 1;
    let mut services = StubServices;
    runtime.show_platform_menu(handle, &mut services).unwrap();

    assert_eq!(
        runtime
            .route_pointer_event(144, 50, true, &mut services)
            .unwrap(),
        None
    );
    assert_eq!(runtime.menus[&handle].focused_item, 0);
    assert_eq!(
        runtime
            .route_pointer_event(144, 50, false, &mut services)
            .unwrap(),
        Some((4, 0, 0))
    );

    assert_eq!(
        runtime
            .route_pointer_event(144, 50, true, &mut services)
            .unwrap(),
        None
    );
    assert_eq!(
        runtime
            .route_pointer_event(144, 74, false, &mut services)
            .unwrap(),
        None
    );

    runtime
        .route_pointer_event(20, 306, true, &mut services)
        .unwrap();
    assert_eq!(
        runtime
            .route_pointer_event(20, 306, false, &mut services)
            .unwrap(),
        Some((4, 0, 0))
    );
    runtime
        .route_pointer_event(220, 306, true, &mut services)
        .unwrap();
    assert_eq!(
        runtime
            .route_pointer_event(220, 306, false, &mut services)
            .unwrap(),
        Some((5, 0, 0))
    );
    assert_eq!(runtime.active_platform_ui, [ActivePlatformUi::Menu(handle)]);
}

#[test]
fn platform_menu_release_restores_nested_screens_and_invalidates_the_handle() {
    let mut runtime =
        ExtRuntime::new(240, 320, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    runtime.memory.write_u16(SCREEN_BASE, 0x1234).unwrap();
    let parent = runtime
        .create_platform_menu(vec![0x7236, 0x83dc], 1)
        .unwrap();
    runtime.menus.get_mut(&parent).unwrap().items = vec![Some(vec![0x9879])];
    let child = runtime
        .create_platform_menu(vec![0x5b50, 0x83dc], 1)
        .unwrap();
    runtime.menus.get_mut(&child).unwrap().items = vec![Some(vec![0x9879])];
    let mut services = StubServices;
    runtime.show_platform_menu(parent, &mut services).unwrap();
    let parent_screen = runtime.memory.read(SCREEN_BASE, 240 * 320 * 2).unwrap();
    runtime.show_platform_menu(child, &mut services).unwrap();
    let mut cpu = ArmCpu::new();
    cpu.set_register(0, child);

    runtime.dispatch(67, 0, &mut cpu, &mut services).unwrap();
    assert_eq!(cpu.register(0), 0);
    assert_eq!(
        runtime
            .memory
            .read(SCREEN_BASE, parent_screen.len())
            .unwrap(),
        parent_screen
    );
    assert_eq!(runtime.active_platform_ui, [ActivePlatformUi::Menu(parent)]);

    cpu.set_register(0, parent);
    runtime.dispatch(67, 0, &mut cpu, &mut services).unwrap();
    assert_eq!(cpu.register(0), 0);
    assert_eq!(runtime.memory.read_u16(SCREEN_BASE).unwrap(), 0x1234);
    assert!(runtime.active_platform_ui.is_empty());

    cpu.set_register(0, parent);
    runtime.dispatch(67, 0, &mut cpu, &mut services).unwrap();
    assert_eq!(cpu.register(0), u32::MAX);
}

#[test]
fn releasing_a_modal_detached_menu_unwinds_the_parent_menu_stack() {
    let mut runtime =
        ExtRuntime::new(240, 320, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    let parent = runtime
        .create_platform_menu(vec![0x7236, 0x83dc], 1)
        .unwrap();
    runtime.menus.get_mut(&parent).unwrap().items = vec![Some(vec![0x5b50, 0x83dc])];
    let child = runtime
        .create_platform_menu(vec![0x5b50, 0x83dc], 1)
        .unwrap();
    runtime.menus.get_mut(&child).unwrap().items = vec![Some(vec![0x4fdd, 0x5b58])];
    let mut services = StubServices;
    runtime.show_platform_menu(parent, &mut services).unwrap();
    let parent_screen = runtime.memory.read(SCREEN_BASE, 240 * 320 * 2).unwrap();
    runtime.show_platform_menu(child, &mut services).unwrap();
    assert_eq!(
        runtime.route_key_event(20, true, &mut services).unwrap(),
        Some((4, 0, 0))
    );
    assert_eq!(runtime.pending_platform_menu_selection, Some(child));
    let title = runtime.allocate(2, 2).unwrap();
    let message = runtime.allocate(2, 2).unwrap();
    runtime.memory.write(title, &[0, 0]).unwrap();
    runtime.memory.write(message, &[0, 0]).unwrap();
    let mut cpu = ArmCpu::new();
    cpu.set_register(0, title.0);
    cpu.set_register(1, message.0);

    runtime.dispatch(69, 0, &mut cpu, &mut services).unwrap();

    let dialog = cpu.register(0);
    assert_eq!(
        runtime.active_platform_ui,
        [
            ActivePlatformUi::Menu(parent),
            ActivePlatformUi::Dialog(dialog)
        ]
    );
    assert_eq!(runtime.pending_platform_menu_selection, None);
    assert_eq!(runtime.dialogs[&dialog].previous_screen, parent_screen);

    cpu.set_register(0, dialog);
    runtime.dispatch(70, 0, &mut cpu, &mut services).unwrap();
    assert_eq!(runtime.active_platform_ui, [ActivePlatformUi::Menu(parent)]);
    assert_eq!(
        runtime
            .memory
            .read(SCREEN_BASE, parent_screen.len())
            .unwrap(),
        parent_screen
    );

    cpu.set_register(0, child);
    runtime.dispatch(67, 0, &mut cpu, &mut services).unwrap();
    assert_eq!(cpu.register(0), 0);
    assert!(runtime.active_platform_ui.is_empty());
    assert_eq!(runtime.pending_platform_menu_returns, 1);
    assert_eq!(runtime.memory.read_u16(SCREEN_BASE).unwrap(), 0);
}

#[test]
fn platform_text_viewer_reports_cancel_then_guest_release_restores_the_screen() {
    let mut runtime =
        ExtRuntime::new(240, 320, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    runtime.memory.write_u16(SCREEN_BASE, 0x1234).unwrap();
    let title = runtime.allocate(4, 2).unwrap();
    let text = runtime.allocate(4, 2).unwrap();
    runtime.memory.write(title, &[0, b'T', 0, 0]).unwrap();
    runtime.memory.write(text, &[0, b'B', 0, 0]).unwrap();
    let mut services = StubServices;
    let mut cpu = ArmCpu::new();
    cpu.set_register(0, title.0);
    cpu.set_register(1, text.0);
    cpu.set_register(2, 2);

    runtime.dispatch(72, 0, &mut cpu, &mut services).unwrap();

    let handle = cpu.register(0);
    assert!(runtime.text_viewers.contains_key(&handle));
    assert_eq!(
        runtime.active_platform_ui,
        [ActivePlatformUi::TextViewer(handle)]
    );
    assert_eq!(
        runtime
            .memory
            .read_u16(runtime.screen_address(0, 294, 240).unwrap())
            .unwrap(),
        Framebuffer::rgb565(0, 252, 0)
    );

    runtime.memory.write_u16(SCREEN_BASE, 0xffff).unwrap();
    cpu.set_register(0, handle);
    runtime.dispatch(74, 0, &mut cpu, &mut services).unwrap();
    assert_eq!(cpu.register(0), 0);
    assert_eq!(runtime.memory.read_u16(SCREEN_BASE).unwrap(), 0);
    assert_eq!(
        runtime.route_key_event(18, true, &mut services).unwrap(),
        Some((6, 1, 0))
    );
    assert_eq!(
        runtime.active_platform_ui,
        [ActivePlatformUi::TextViewer(handle)]
    );

    cpu.set_register(0, handle);
    runtime.dispatch(73, 0, &mut cpu, &mut services).unwrap();
    assert_eq!(cpu.register(0), 0);
    assert!(runtime.text_viewers.is_empty());
    assert!(runtime.active_platform_ui.is_empty());
    assert_eq!(runtime.memory.read_u16(SCREEN_BASE).unwrap(), 0x1234);

    cpu.set_register(0, handle);
    runtime.dispatch(73, 0, &mut cpu, &mut services).unwrap();
    assert_eq!(cpu.register(0), u32::MAX);
}

#[test]
fn platform_editor_routes_bounded_text_and_enforces_handle_ownership() {
    let mut runtime =
        ExtRuntime::new(240, 320, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    load_test_module(&mut runtime);
    load_test_module(&mut runtime);
    let handle = runtime
        .create_platform_editor(0, vec![b'T' as u16], Vec::new(), 0, 4)
        .unwrap();

    assert_eq!(
        runtime.active_platform_ui,
        [ActivePlatformUi::Editor(handle)]
    );
    assert_eq!(
        runtime.route_text_input("A\u{1f600}BC").unwrap(),
        Some((6, 0, 0))
    );

    let mut cpu = ArmCpu::new();
    cpu.set_register(0, handle);
    runtime
        .dispatch(77, 0, &mut cpu, &mut StubServices)
        .unwrap();
    let buffer = GuestAddr(cpu.register(0));
    assert_ne!(buffer.0, 0);
    assert_eq!(
        runtime.memory.read(buffer, 10).unwrap(),
        [0, b'A', 0xd8, 0x3d, 0xde, 0x00, 0, b'B', 0, 0]
    );

    cpu.set_register(0, handle);
    runtime
        .dispatch(77, 1, &mut cpu, &mut StubServices)
        .unwrap();
    assert_eq!(cpu.register(0), 0);
    cpu.set_register(0, handle);
    runtime
        .dispatch(76, 1, &mut cpu, &mut StubServices)
        .unwrap();
    assert_eq!(cpu.register(0), u32::MAX);

    cpu.set_register(0, handle);
    runtime
        .dispatch(76, 0, &mut cpu, &mut StubServices)
        .unwrap();
    assert_eq!(cpu.register(0), 0);
    assert!(runtime.editors.is_empty());
    assert!(runtime.active_platform_ui.is_empty());
    assert!(runtime.tracked_guest_allocation_len(buffer).is_none());
    assert_eq!(runtime.route_text_input("ignored").unwrap(), None);
}

#[test]
fn platform_editor_rejects_oversized_limits_before_reading_guest_strings() {
    let mut runtime =
        ExtRuntime::new(240, 320, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    load_test_module(&mut runtime);
    let mut cpu = ArmCpu::new();
    cpu.set_register(0, u32::MAX);
    cpu.set_register(1, u32::MAX);
    cpu.set_register(3, MAX_PLATFORM_EDITOR_CODE_UNITS as u32 + 1);

    assert!(matches!(
        runtime.dispatch(75, 0, &mut cpu, &mut StubServices),
        Err(Error::ResourceLimit(message)) if message.contains("platform editor")
    ));
    assert!(runtime.editors.is_empty());
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
    runtime.active_platform_ui.push(ActivePlatformUi::Dialog(1));
    let mut services = StubServices;

    assert_eq!(
        runtime.route_key_event(17, true, &mut services).unwrap(),
        Some((6, 1, 0))
    );
    assert_eq!(
        runtime.route_key_event(17, false, &mut services).unwrap(),
        None
    );
    assert_eq!(
        runtime.route_key_event(18, true, &mut services).unwrap(),
        Some((6, 0, 0))
    );
    assert_eq!(
        runtime.route_key_event(18, false, &mut services).unwrap(),
        None
    );

    runtime
        .route_pointer_event(120, 266, true, &mut services)
        .unwrap();
    assert_eq!(
        runtime
            .route_pointer_event(120, 266, false, &mut services)
            .unwrap(),
        Some((6, 1, 0))
    );
    runtime
        .route_pointer_event(220, 306, true, &mut services)
        .unwrap();
    assert_eq!(
        runtime
            .route_pointer_event(220, 306, false, &mut services)
            .unwrap(),
        Some((6, 0, 0))
    );

    runtime.dialogs.clear();
    runtime.active_platform_ui.clear();
    assert_eq!(
        runtime.route_key_event(12, true, &mut services).unwrap(),
        Some((0, 12, 0))
    );
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
    load_test_module(&mut runtime);
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
fn incomplete_ram_package_state_is_rejected() {
    for (ram_address, ram_len) in [(HEAP_BASE.0, 0), (0, 24)] {
        let mut runtime =
            ExtRuntime::new(240, 320, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
        load_test_module(&mut runtime);
        let name = runtime.allocate(10, 1).unwrap();
        runtime.memory.write(name, b"owned.bin\0").unwrap();
        let output_len = runtime.allocate(4, 4).unwrap();
        runtime.memory.write_u32(output_len, u32::MAX).unwrap();
        runtime
            .memory
            .write_u32(data_slot_address(104), ram_address)
            .unwrap();
        runtime
            .memory
            .write_u32(data_slot_address(105), ram_len)
            .unwrap();
        let mut cpu = ArmCpu::new();
        cpu.set_register(0, name.0);
        cpu.set_register(1, output_len.0);

        assert!(matches!(
            runtime.dispatch(125, 0, &mut cpu, &mut StubServices),
            Err(Error::Abi(message))
                if message.contains("RAM-backed MRP has inconsistent address")
        ));
        assert_eq!(cpu.register(0), name.0);
        assert_eq!(runtime.memory.read_u32(output_len).unwrap(), u32::MAX);
    }
}

#[test]
fn package_file_read_uses_detached_allocation_after_guest_heap_teardown() {
    let mut runtime =
        ExtRuntime::new(240, 320, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    load_test_module(&mut runtime);
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
        load_test_module(&mut runtime);
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
        assert_eq!(
            runtime.guest_allocation_owners.get(&prepared.0),
            Some(&runtime.modules[0].generation)
        );
        assert!(runtime.memory.fetch_u16(prepared).is_err());

        cpu.set_register(0, 0);
        cpu.set_register(1, 9);
        cpu.set_register(2, prepared.0);
        cpu.set_register(3, expected.len() as u32);
        runtime
            .dispatch(131, 0, &mut cpu, &mut StubServices)
            .unwrap();
    }
}

#[test]
fn compact_ram_write_preserves_a_live_enclosing_executable_allocation() {
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
    load_test_module(&mut runtime);
    let backing = runtime
        .allocate_guest_block_for_module(0x400, 0)
        .unwrap()
        .unwrap();
    runtime
        .memory
        .write(backing, &0xe12f_ff1e_u32.to_le_bytes())
        .unwrap();
    let mut cpu = ArmCpu::new();
    cpu.set_register(0, 0);
    cpu.set_register(1, 9);
    cpu.set_register(2, backing.0);
    cpu.set_register(3, 0x400);
    runtime
        .dispatch(131, 0, &mut cpu, &mut StubServices)
        .unwrap();

    let aligned_len = heap::aligned_heap_len(expected.len()).unwrap();
    let prepared = backing.checked_add(0x80).unwrap();
    runtime.memory.write_u32(prepared, 0).unwrap();
    runtime
        .memory
        .write_u32(prepared.checked_add(4).unwrap(), aligned_len)
        .unwrap();
    let package = runtime.allocate(image.len(), 8).unwrap();
    runtime.memory.write(package, &image).unwrap();
    let descriptor = runtime.allocate(8, 4).unwrap();
    runtime.memory.write_u32(descriptor, prepared.0).unwrap();
    runtime
        .memory
        .write_u32(descriptor.checked_add(4).unwrap(), aligned_len)
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

    cpu.set_register(0, name.0);
    cpu.set_register(1, output_len.0);
    runtime
        .dispatch(125, 0, &mut cpu, &mut StubServices)
        .unwrap();

    assert_eq!(cpu.register(0), prepared.0);
    assert_eq!(
        runtime.memory.read(prepared, expected.len()).unwrap(),
        expected
    );
    assert!(runtime.memory.fetch_u16(prepared).is_ok());
    assert_eq!(
        runtime.modules[0].dynamic_executable_ranges,
        [Some(ExecutableRange {
            base: backing,
            len: 0x400,
        })]
    );

    runtime
        .free_guest_block_for_module(prepared, expected.len(), 0)
        .unwrap();
    assert!(runtime.memory.fetch_u16(prepared).is_err());
    assert_eq!(
        runtime.modules[0].dynamic_executable_ranges[0]
            .as_ref()
            .unwrap()
            .intervals,
        [
            ExecutableRange {
                base: backing,
                len: 0x80,
            },
            ExecutableRange {
                base: prepared.checked_add(aligned_len).unwrap(),
                len: 0x400 - 0x80 - aligned_len as usize,
            },
        ]
    );
}

#[test]
fn compact_ram_package_ignores_a_partial_screen_allocation_candidate() {
    let expected = vec![0x5a; 0x60];
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(&expected).unwrap();
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
    load_test_module(&mut runtime);
    let owner_generation = runtime.modules[0].generation;
    runtime
        .track_guest_heap_allocation(SCREEN_BASE, 0x20, Some(owner_generation))
        .unwrap();
    runtime
        .memory
        .write_u32(SCREEN_BASE.checked_add(4).unwrap(), expected.len() as u32)
        .unwrap();

    let package = runtime.allocate(image.len(), 8).unwrap();
    runtime.memory.write(package, &image).unwrap();
    let descriptor = runtime.allocate(8, 4).unwrap();
    runtime.memory.write_u32(descriptor, SCREEN_BASE.0).unwrap();
    runtime
        .memory
        .write_u32(descriptor.checked_add(4).unwrap(), expected.len() as u32)
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

    let output = GuestAddr(cpu.register(0));
    assert_ne!(output, SCREEN_BASE);
    assert_eq!(
        runtime.memory.read(output, expected.len()).unwrap(),
        expected
    );
    assert_eq!(
        runtime.memory.read_u32(output_len).unwrap(),
        expected.len() as u32
    );
}

#[test]
fn compact_ram_package_preserves_prepared_output_ownership_errors() {
    let mut runtime =
        ExtRuntime::new(240, 320, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    load_test_module(&mut runtime);
    load_test_module(&mut runtime);
    let output_len = 0x20_u32;
    let prepared = runtime
        .allocate_guest_block_for_module(output_len as usize, 0)
        .unwrap()
        .unwrap();
    runtime
        .memory
        .write_u32(prepared.checked_add(4).unwrap(), output_len)
        .unwrap();
    let descriptor = runtime.allocate(8, 4).unwrap();
    runtime.memory.write_u32(descriptor, prepared.0).unwrap();
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

    assert!(matches!(
        runtime.compact_ram_output_target(package, compact_header.len(), output_len as usize, 1),
        Err(Error::Abi(message)) if message.contains("belongs to another module")
    ));
}

#[test]
fn compact_output_view_preserves_its_owned_backing_allocation() {
    let mut runtime =
        ExtRuntime::new(240, 320, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    load_test_module(&mut runtime);
    load_test_module(&mut runtime);
    let backing = runtime
        .allocate_guest_block_for_module(0x200, 0)
        .unwrap()
        .unwrap();
    let prepared = backing.checked_add(0x28).unwrap();
    let allocations = runtime.guest_allocations.clone();
    let owners = runtime.guest_allocation_owners.clone();

    runtime
        .claim_prepared_output_for_module(prepared, 0x80, 0)
        .unwrap();

    assert_eq!(runtime.guest_allocations, allocations);
    assert_eq!(runtime.guest_allocation_owners, owners);
    assert_eq!(runtime.guest_allocations.get(&backing.0), Some(&0x200));
    assert!(!runtime.guest_allocations.contains_key(&prepared.0));

    assert!(matches!(
        runtime.claim_prepared_output_for_module(prepared, 0x80, 1),
        Err(Error::Abi(message)) if message.contains("another module")
    ));
    assert_eq!(runtime.guest_allocations, allocations);
    assert_eq!(runtime.guest_allocation_owners, owners);
}

#[test]
fn compact_output_view_is_removed_from_a_rebuilt_guest_free_list() {
    let mut runtime =
        ExtRuntime::new(240, 320, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    load_test_module(&mut runtime);
    let backing = runtime
        .allocate_guest_block_for_module(0x200, 0)
        .unwrap()
        .unwrap();
    let prepared = backing.checked_add(0x28).unwrap();
    let heap = runtime.guest_heap_state().unwrap();
    let backing_offset = backing.0 - heap.base;
    let prepared_offset = prepared.0 - heap.base;
    let suffix_offset = backing_offset + 0x200;
    let suffix_len = heap.span - suffix_offset;
    runtime
        .write_free_blocks(
            heap,
            &[
                FreeBlock {
                    offset: prepared_offset,
                    len: 0x80,
                },
                FreeBlock {
                    offset: suffix_offset,
                    len: suffix_len,
                },
            ],
            heap.span,
            0x80 + suffix_len,
        )
        .unwrap();
    let allocations = runtime.guest_allocations.clone();
    let owners = runtime.guest_allocation_owners.clone();

    runtime
        .claim_prepared_output_for_module(prepared, 0x80, 0)
        .unwrap();

    let heap = runtime.guest_heap_state().unwrap();
    let (blocks, terminator, recovered_len) = runtime.read_free_blocks(heap).unwrap();
    assert_eq!(
        blocks,
        [FreeBlock {
            offset: suffix_offset,
            len: suffix_len,
        }]
    );
    assert_eq!(terminator, heap.span);
    assert_eq!(recovered_len, 0);
    assert_eq!(heap.free_left, suffix_len);
    assert_eq!(runtime.guest_allocations, allocations);
    assert_eq!(runtime.guest_allocation_owners, owners);
    assert_eq!(
        runtime.nested_guest_heaps.get(&backing.0),
        Some(&NestedGuestHeap {
            owner_generation: runtime.modules[0].generation,
            heap_base: heap.base,
            heap_span: heap.span,
        })
    );
    assert_eq!(
        runtime.guest_allocation_views.get(&prepared.0),
        Some(&GuestAllocationView {
            len: 0x80,
            backing_base: backing.0,
            owner_generation: runtime.modules[0].generation,
        })
    );

    let nested = prepared.checked_add(0x80).unwrap();
    assert!(matches!(
        runtime.free_guest_block_for_module(prepared.checked_add(0x40).unwrap(), 0x40, 0),
        Err(Error::Abi(message)) if message.contains("active guest allocation view")
    ));
    let mut cpu = ArmCpu::new();
    cpu.set_register(0, 0);
    cpu.set_register(1, 9);
    cpu.set_register(2, prepared.0);
    cpu.set_register(3, 4);
    runtime
        .dispatch(131, 0, &mut cpu, &mut StubServices)
        .unwrap();
    assert!(runtime.memory.fetch_u32(prepared).is_ok());

    runtime.free_guest_block_for_module(prepared, 1, 0).unwrap();
    assert!(runtime.memory.fetch_u32(prepared).is_err());
    assert!(!runtime.guest_allocation_views.contains_key(&prepared.0));
    assert_eq!(runtime.guest_allocations, allocations);
    assert_eq!(runtime.guest_allocation_owners, owners);

    runtime
        .free_guest_block_for_module(nested, 0x40, 0)
        .unwrap();

    runtime
        .free_guest_block_for_module(backing, 0x200, 0)
        .unwrap();
    let heap = runtime.guest_heap_state().unwrap();
    let (blocks, terminator, recovered_len) = runtime.read_free_blocks(heap).unwrap();
    assert_eq!(
        blocks,
        [FreeBlock {
            offset: backing_offset,
            len: heap.span - backing_offset,
        }]
    );
    assert_eq!(terminator, heap.span);
    assert_eq!(recovered_len, 0);
    assert_eq!(heap.free_left, heap.span - backing_offset);
    assert!(!runtime.guest_allocations.contains_key(&backing.0));
    assert!(!runtime.guest_allocation_owners.contains_key(&backing.0));
    assert!(!runtime.nested_guest_heaps.contains_key(&backing.0));
}

#[test]
fn compact_output_in_a_staged_platform_heap_reserves_and_tracks_its_view() {
    let mut runtime =
        ExtRuntime::new(240, 320, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    load_test_module(&mut runtime);
    let old_allocation = runtime
        .allocate_guest_block_for_module(8, 0)
        .unwrap()
        .unwrap();
    let owner_generation = runtime.modules[0].generation;
    let arena = PLATFORM_MEMORY_BASE;
    let arena_len = 0x200_usize;
    runtime
        .memory
        .map(
            arena,
            arena_len,
            Permissions::READ_WRITE,
            "test staged platform heap",
        )
        .unwrap();
    runtime.platform_memory_extensions.insert(
        arena.0,
        PlatformMemoryExtension {
            len: arena_len,
            previous_cursor: arena.0,
            owner_generation,
        },
    );

    let prepared = arena.checked_add(0x80).unwrap();
    let staged_span = arena.0 + arena_len as u32 - HEAP_BASE.0;
    let prepared_offset = prepared.0 - HEAP_BASE.0;
    runtime.memory.write_u32(prepared, staged_span).unwrap();
    runtime
        .memory
        .write_u32(prepared.checked_add(4).unwrap(), 0x180)
        .unwrap();
    runtime
        .memory
        .write_u32(data_slot_address(110), arena.0 + arena_len as u32)
        .unwrap();
    runtime
        .memory
        .write_u32(data_slot_address(111), 0x180)
        .unwrap();
    runtime
        .memory
        .write_u32(data_slot_address(146), prepared_offset)
        .unwrap();

    runtime
        .claim_prepared_output_for_module(prepared, 0x40, 0)
        .unwrap();
    runtime.memory.write(prepared, b"MRPGCMAP").unwrap();

    assert_eq!(
        runtime.guest_allocation_views.get(&prepared.0),
        Some(&GuestAllocationView {
            len: 0x40,
            backing_base: arena.0,
            owner_generation,
        })
    );
    let heap = runtime.guest_heap_state().unwrap();
    let (blocks, _, _) = runtime.read_free_blocks(heap).unwrap();
    assert_eq!(
        blocks,
        [FreeBlock {
            offset: prepared_offset + 0x40,
            len: 0x140,
        }]
    );

    runtime
        .free_guest_block_for_module(old_allocation, 8, 0)
        .unwrap();
    runtime
        .free_guest_block_for_module(prepared, 0x40, 0)
        .unwrap();
    assert!(!runtime.guest_allocation_views.contains_key(&prepared.0));
}

#[test]
fn compact_output_in_a_staged_heap_reserves_the_mtk_window_before_writing() {
    let mut runtime =
        ExtRuntime::new(240, 320, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    runtime
        .set_native_extension_profile(NativeExtensionProfile::Mtk)
        .unwrap();
    load_test_module(&mut runtime);
    let live_allocation = runtime
        .allocate_guest_block_for_module(8, 0)
        .unwrap()
        .unwrap();
    let initial_heap = runtime.guest_heap_state().unwrap();
    let live_offset = live_allocation.0 - initial_heap.base;
    let (initial_blocks, _, recovered_len) = runtime.read_free_blocks(initial_heap).unwrap();
    assert_eq!(recovered_len, 0);
    assert_eq!(initial_blocks.len(), 1);

    let prepared = MTK_NATIVE_EXTENSION_BASE;
    let prepared_offset = prepared.0 - initial_heap.base;
    let prepared_free_len = 0x1fd8;
    let payload_len = 0x1870;
    let staged_span = prepared_offset + prepared_free_len;
    runtime
        .memory
        .write_u32(data_slot_address(110), initial_heap.base + staged_span)
        .unwrap();
    let staged_heap = runtime.guest_heap_state().unwrap();
    let initial_free_len = initial_blocks[0].len;
    runtime
        .write_free_blocks(
            staged_heap,
            &[
                FreeBlock {
                    offset: prepared_offset,
                    len: prepared_free_len,
                },
                initial_blocks[0],
            ],
            staged_span,
            initial_free_len + 2 * prepared_free_len,
        )
        .unwrap();

    runtime
        .claim_prepared_output_for_module(prepared, payload_len, 0)
        .unwrap();
    runtime.memory.write(prepared, b"MRPGCMAP").unwrap();
    // Re-claiming an already reserved fixed-window range is valid and must not
    // attempt to interpret the payload as a free-block header.
    runtime
        .claim_prepared_output_for_module(prepared, payload_len, 0)
        .unwrap();

    let heap = runtime.guest_heap_state().unwrap();
    let (blocks, terminator, recovered_len) = runtime.read_free_blocks(heap).unwrap();
    assert_eq!(recovered_len, 0);
    assert_eq!(terminator, staged_span);
    assert_eq!(
        blocks,
        [
            FreeBlock {
                offset: prepared_offset + payload_len,
                len: prepared_free_len - payload_len,
            },
            initial_blocks[0],
        ]
    );
    assert_eq!(
        runtime.mtk_native_extension_owner,
        Some(runtime.modules[0].generation)
    );
    assert_eq!(
        runtime.guest_allocations.get(&prepared.0),
        Some(&payload_len)
    );
    assert_eq!(
        runtime.guest_allocation_owners.get(&prepared.0),
        Some(&runtime.modules[0].generation)
    );

    runtime
        .free_guest_block_for_module(live_allocation, 8, 0)
        .unwrap();
    let heap = runtime.guest_heap_state().unwrap();
    let (blocks, _, recovered_len) = runtime.read_free_blocks(heap).unwrap();
    assert_eq!(recovered_len, 0);
    assert_eq!(
        blocks,
        [
            FreeBlock {
                offset: live_offset,
                len: DEFAULT_HEAP_LEN as u32 - live_offset,
            },
            FreeBlock {
                offset: prepared_offset + payload_len,
                len: prepared_free_len - payload_len,
            },
        ]
    );

    runtime
        .free_guest_block_for_module(prepared, payload_len as usize, 0)
        .unwrap();
    assert!(!runtime.guest_allocations.contains_key(&prepared.0));
    assert!(!runtime.guest_allocation_owners.contains_key(&prepared.0));
    assert_eq!(
        runtime.mtk_native_extension_owner,
        Some(runtime.modules[0].generation)
    );
    let heap = runtime.guest_heap_state().unwrap();
    let (blocks, _, recovered_len) = runtime.read_free_blocks(heap).unwrap();
    assert_eq!(recovered_len, 0);
    assert_eq!(
        blocks,
        [
            FreeBlock {
                offset: live_offset,
                len: DEFAULT_HEAP_LEN as u32 - live_offset,
            },
            FreeBlock {
                offset: prepared_offset,
                len: prepared_free_len,
            },
        ]
    );
}

#[test]
fn compact_ram_package_accepts_a_prepared_platform_memory_target() {
    let mut runtime =
        ExtRuntime::new(240, 320, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    runtime
        .set_native_extension_profile(NativeExtensionProfile::Mtk)
        .unwrap();
    load_test_module(&mut runtime);
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
            .compact_ram_output_target(package, compact_header.len(), output_len as usize, 0)
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
    let firmware_slots = runtime.memory.read_u32(INTERNAL_TABLE_DATA).unwrap();
    assert_eq!(firmware_slots, FIRMWARE_SLOT_DATA.0);
    for index in 0..FIRMWARE_SLOT_COUNT {
        assert_eq!(
            runtime
                .memory
                .read_u32(FIRMWARE_SLOT_DATA.checked_add(index * 4).unwrap())
                .unwrap(),
            0,
            "firmware slot {index}"
        );
    }
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
    write_platform_string(&mut runtime.memory, PACKAGE_NAME_DATA, b"child.mrp").unwrap();
    write_platform_string(&mut runtime.memory, START_NAME_DATA, b"main.mr").unwrap();

    assert_eq!(runtime.lifecycle_request().unwrap(), None);

    runtime
        .memory
        .write_u32(APPLICATION_STATE_DATA, APPLICATION_STATE_RESTART_PENDING)
        .unwrap();
    assert_eq!(
        runtime.lifecycle_request().unwrap(),
        Some(ExtLifecycleRequest::Restart {
            package: b"child.mrp".to_vec(),
            entry: b"main.mr".to_vec(),
        })
    );

    runtime.clear_lifecycle_request().unwrap();
    assert_eq!(runtime.memory.read_u32(LIFECYCLE_CALLBACK_DATA).unwrap(), 0);
    assert_eq!(
        runtime.memory.read_u32(APPLICATION_STATE_DATA).unwrap(),
        APPLICATION_STATE_NORMAL
    );
    assert_eq!(runtime.lifecycle_request().unwrap(), None);
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
fn platform_data_slots_use_non_overlapping_resource_backings() {
    let runtime =
        ExtRuntime::new(240, 320, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    let resources = [
        (95, BITMAP_ARRAY_DATA),
        (96, TILE_ARRAY_DATA),
        (97, MAP_ARRAY_DATA),
        (98, SOUND_ARRAY_DATA),
        (99, SPRITE_ARRAY_DATA),
        (112, SMS_CONFIG_DATA),
    ];

    for (slot, expected) in resources {
        assert_eq!(
            runtime.memory.read_u32(table_slot_address(slot)).unwrap(),
            expected.0,
            "slot {slot}"
        );
    }
    for (index, (_, left)) in resources.iter().enumerate() {
        let left_end = left.0 + PLATFORM_RESOURCE_BACKING_LEN as u32;
        for (_, right) in &resources[index + 1..] {
            assert!(
                left_end <= right.0 || right.0 + PLATFORM_RESOURCE_BACKING_LEN as u32 <= left.0
            );
        }
        for slot in [91, 92, 93, 94, 104, 105, 106, 107, 108, 109, 110, 111] {
            let scalar = data_slot_address(slot).0;
            assert!(scalar < left.0 || scalar >= left_end, "slot {slot}");
        }
    }

    assert_eq!(
        runtime.memory.read_u32(table_slot_address(100)).unwrap(),
        PACKAGE_NAME_DATA.0
    );
    assert_eq!(
        runtime.memory.read_u32(table_slot_address(101)).unwrap(),
        START_NAME_DATA.0
    );
    assert_eq!(
        runtime.memory.read_u32(table_slot_address(102)).unwrap(),
        PREVIOUS_PACKAGE_NAME_DATA.0
    );
    assert_eq!(
        runtime.memory.read_u32(table_slot_address(103)).unwrap(),
        PREVIOUS_START_NAME_DATA.0
    );
}

#[test]
fn start_file_parameter_round_trips_without_aliasing_adjacent_slots() {
    let mut runtime =
        ExtRuntime::new(240, 320, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();

    assert_eq!(
        runtime.memory.read_u32(table_slot_address(138)).unwrap(),
        START_FILE_PARAMETER_DATA.0
    );
    assert_eq!(
        runtime.start_file_parameter().unwrap(),
        [0; START_FILE_PARAMETER_LEN]
    );

    let adjacent = [(139, 0x1122_3344), (140, 0x5566_7788)];
    for (slot, value) in adjacent {
        let address = GuestAddr(runtime.memory.read_u32(table_slot_address(slot)).unwrap());
        runtime.memory.write_u32(address, value).unwrap();
    }

    let parameter = std::array::from_fn(|index| (index as u8).wrapping_mul(37));
    runtime.set_start_file_parameter(&parameter).unwrap();
    assert_eq!(runtime.start_file_parameter().unwrap(), parameter);
    assert_eq!(
        runtime
            .memory
            .read(START_FILE_PARAMETER_DATA, START_FILE_PARAMETER_LEN)
            .unwrap(),
        parameter
    );

    let parameter_end = START_FILE_PARAMETER_DATA.0 + START_FILE_PARAMETER_LEN as u32;
    for slot in [135, 136, 139, 140, 142, 143, 144, 146] {
        let address = runtime.memory.read_u32(table_slot_address(slot)).unwrap();
        assert!(
            address < START_FILE_PARAMETER_DATA.0 || address >= parameter_end,
            "slot {slot} aliases the start-file parameter"
        );
    }
    for (slot, expected) in adjacent {
        let address = GuestAddr(runtime.memory.read_u32(table_slot_address(slot)).unwrap());
        assert_eq!(runtime.memory.read_u32(address).unwrap(), expected);
    }
}

#[test]
fn resource_array_writes_do_not_corrupt_platform_scalars() {
    let mut runtime =
        ExtRuntime::new(240, 320, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    let scalar_slots = [91, 92, 93, 94, 104, 105, 106, 107, 108, 109, 110, 111];
    let scalar_values =
        scalar_slots.map(|slot| runtime.memory.read_u32(data_slot_address(slot)).unwrap());
    let screen_bitmap = BITMAP_ARRAY_DATA
        .checked_add(SCREEN_BITMAP_ID * BITMAP_ENTRY_SIZE)
        .unwrap();
    let screen_descriptor = runtime.memory.read(screen_bitmap, 16).unwrap();
    let resource_arrays = [
        BITMAP_ARRAY_DATA,
        TILE_ARRAY_DATA,
        MAP_ARRAY_DATA,
        SOUND_ARRAY_DATA,
        SPRITE_ARRAY_DATA,
    ];

    for (index, address) in resource_arrays.into_iter().enumerate() {
        let pattern = vec![0x20 + index as u8; 16];
        runtime.memory.write(address, &pattern).unwrap();
        runtime
            .memory
            .write(
                address
                    .checked_add(PLATFORM_RESOURCE_BACKING_LEN as u32 - 16)
                    .unwrap(),
                &pattern,
            )
            .unwrap();
    }
    runtime
        .memory
        .write(SMS_CONFIG_DATA, &vec![0x5a; PLATFORM_RESOURCE_BACKING_LEN])
        .unwrap();

    assert_eq!(
        runtime.memory.read(screen_bitmap, 16).unwrap(),
        screen_descriptor
    );
    for (slot, expected) in scalar_slots.into_iter().zip(scalar_values) {
        assert_eq!(
            runtime.memory.read_u32(data_slot_address(slot)).unwrap(),
            expected,
            "slot {slot}"
        );
    }

    runtime.memory.write(screen_bitmap, &[0xa5; 16]).unwrap();
    for (slot, expected) in scalar_slots.into_iter().zip(scalar_values) {
        assert_eq!(
            runtime.memory.read_u32(data_slot_address(slot)).unwrap(),
            expected,
            "bitmap descriptor corrupted slot {slot}"
        );
    }
    for (index, address) in resource_arrays.into_iter().enumerate() {
        assert_eq!(
            runtime.memory.read(address, 16).unwrap(),
            vec![0x20 + index as u8; 16]
        );
    }
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
        .as_chunks::<2>()
        .0
        .iter()
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
