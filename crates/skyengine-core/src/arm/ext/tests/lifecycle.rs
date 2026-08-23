use super::*;

fn allocate_platform_arena(runtime: &mut ExtRuntime, module: usize, len: u32) -> GuestAddr {
    let output = runtime.allocate(4, 4).unwrap();
    let output_len = runtime.allocate(4, 4).unwrap();
    let stack = runtime.allocate(4, 4).unwrap();
    runtime.memory.write_u32(stack, output_len.0).unwrap();

    let mut cpu = ArmCpu::new();
    cpu.set_register(0, 1_014);
    cpu.set_register(2, len);
    cpu.set_register(3, output.0);
    cpu.set_register(13, stack.0);
    runtime
        .dispatch(38, module, &mut cpu, &mut StubServices)
        .unwrap();
    GuestAddr(runtime.memory.read_u32(output).unwrap())
}

fn register_dynamic_image(runtime: &mut ExtRuntime, module: usize, address: GuestAddr, len: u32) {
    runtime
        .memory
        .write(address, &0xe12f_ff1e_u32.to_le_bytes())
        .unwrap();
    let mut cpu = ArmCpu::new();
    cpu.set_register(0, 0);
    cpu.set_register(1, 9);
    cpu.set_register(2, address.0);
    cpu.set_register(3, len);
    runtime
        .dispatch(131, module, &mut cpu, &mut StubServices)
        .unwrap();
}

fn register_helper(runtime: &mut ExtRuntime, module: usize, address: GuestAddr) {
    let mut cpu = ArmCpu::new();
    cpu.set_register(0, address.0);
    cpu.set_register(1, 20);
    runtime
        .dispatch(25, module, &mut cpu, &mut StubServices)
        .unwrap();
}

#[test]
fn raw_helper_arguments_are_forwarded_in_r2_and_r3() {
    let mut image = b"MRPGCMAP".to_vec();
    image.extend_from_slice(&0xe082_0403_u32.to_le_bytes()); // add r0, r2, r3, lsl #8
    image.extend_from_slice(&0xe12f_ff1e_u32.to_le_bytes()); // bx lr
    let mut runtime =
        ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    runtime
        .load_and_call_entry(&image, 0, &mut StubServices)
        .unwrap();
    let helper = runtime.modules[0].base.checked_add(8).unwrap();
    register_helper(&mut runtime, 0, helper);

    let (result, output) = runtime
        .call_active_helper_raw(6, [1, 2_000], &mut StubServices)
        .unwrap();

    assert_eq!(result, 1 + (2_000 << 8));
    assert!(output.is_empty());
}

#[test]
fn helper_returns_before_a_committed_restart_reaches_the_dispatch_boundary() {
    let instructions = [
        0xe12f_ff1e, // entry: bx lr
        0xe59f_000c, // helper: ldr r0, [pc, #12] (application state)
        0xe3a0_1003, // mov r1, #3
        0xe580_1000, // str r1, [r0]
        0xe3a0_002a, // mov r0, #42
        0xe12f_ff1e, // bx lr
        APPLICATION_STATE_DATA.0,
    ];
    let mut image = b"MRPGCMAP".to_vec();
    image.extend(instructions.into_iter().flat_map(u32::to_le_bytes));
    let mut runtime =
        ExtRuntime::new(8, 8, b"parent.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    runtime
        .load_and_call_entry(&image, 0, &mut StubServices)
        .unwrap();
    let helper = runtime.modules[0].base.checked_add(12).unwrap();
    register_helper(&mut runtime, 0, helper);
    let callback = runtime.allocate(8, 4).unwrap();
    runtime.memory.write(callback, b"restart\0").unwrap();
    runtime
        .memory
        .write_u32(LIFECYCLE_CALLBACK_DATA, callback.0)
        .unwrap();
    write_platform_string(&mut runtime.memory, PACKAGE_NAME_DATA, b"child.mrp").unwrap();
    write_platform_string(&mut runtime.memory, START_NAME_DATA, b"main.mr").unwrap();

    assert_eq!(
        runtime
            .call_active_helper(0, &[], &mut StubServices)
            .unwrap(),
        (42, Vec::new())
    );
    assert_eq!(
        runtime.lifecycle_request().unwrap(),
        Some(ExtLifecycleRequest::Restart {
            package: b"child.mrp".to_vec(),
            entry: b"main.mr".to_vec(),
        })
    );
}

fn release_platform_arena(runtime: &mut ExtRuntime, module: usize, arena: GuestAddr) {
    let mut cpu = ArmCpu::new();
    cpu.set_register(0, 1_015);
    cpu.set_register(1, arena.0);
    cpu.set_register(2, 4);
    runtime
        .dispatch(38, module, &mut cpu, &mut StubServices)
        .unwrap();
}

fn failing_initialization_image(module_base: u32) -> Vec<u8> {
    let helper = module_base + 44;
    let instructions = [
        0xe59f_0020, // ldr r0, [pc, #32] (helper)
        0xe3a0_1014, // mov r1, #20
        0xe59f_c01c, // ldr ip, [pc, #28] (slot 25)
        0xe12f_ff3c, // blx ip
        0xe3a0_0040, // mov r0, #64
        0xe59f_c014, // ldr ip, [pc, #20] (slot 0)
        0xe12f_ff3c, // blx ip
        0xe59f_c010, // ldr ip, [pc, #16] (unsupported slot 21)
        0xe12f_ff3c, // blx ip
        0xe12f_ff1e, // helper: bx lr
        helper,
        TRAP_BASE + 25 * 4,
        TRAP_BASE,
        TRAP_BASE + 21 * 4,
    ];
    let mut image = b"MRPGCMAP".to_vec();
    image.extend(instructions.into_iter().flat_map(u32::to_le_bytes));
    image
}

#[test]
fn releasing_a_dynamic_image_invalidates_its_helper_before_slot_reuse() {
    let mut runtime =
        ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    load_test_module(&mut runtime);
    let arena = allocate_platform_arena(&mut runtime, 0, 64);
    register_dynamic_image(&mut runtime, 0, arena, 64);
    register_helper(&mut runtime, 0, arena);
    assert_eq!(
        runtime.modules[0].helper.unwrap().expected_image,
        Some(ExecutableImage::Dynamic(0))
    );

    release_platform_arena(&mut runtime, 0, arena);
    assert!(runtime.modules[0].helper.is_none());
    assert!(runtime.active_helper.is_none());

    let reused = allocate_platform_arena(&mut runtime, 0, 64);
    assert_eq!(reused, arena);
    register_dynamic_image(&mut runtime, 0, reused, 64);
    assert_eq!(
        runtime.modules[0].dynamic_executable_ranges,
        [Some(ExecutableRange {
            base: reused,
            len: 64,
        })]
    );
    assert!(matches!(
        runtime.call_active_helper(0, &[], &mut StubServices),
        Err(Error::Abi(message)) if message.contains("no EXT helper")
    ));
}

#[test]
fn a_non_executable_helper_cannot_be_revived_by_reusing_its_address() {
    let mut runtime =
        ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    load_test_module(&mut runtime);
    let arena = allocate_platform_arena(&mut runtime, 0, 64);
    let allocations_before = runtime.guest_allocations.clone();

    let mut cpu = ArmCpu::new();
    cpu.set_register(0, arena.0);
    cpu.set_register(1, 20);
    assert!(matches!(
        runtime.dispatch(25, 0, &mut cpu, &mut StubServices),
        Err(Error::Abi(message)) if message.contains("outside module 0 executable images")
    ));
    assert!(runtime.active_helper.is_none());
    assert_eq!(runtime.guest_allocations, allocations_before);

    release_platform_arena(&mut runtime, 0, arena);
    let reused = allocate_platform_arena(&mut runtime, 0, 64);
    assert_eq!(reused, arena);
    register_dynamic_image(&mut runtime, 0, reused, 64);
    assert!(matches!(
        runtime.call_active_helper(0, &[], &mut StubServices),
        Err(Error::Abi(message)) if message.contains("no EXT helper")
    ));
}

#[test]
fn a_freed_heap_image_loses_execute_permission_before_address_reuse() {
    let mut runtime =
        ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    load_test_module(&mut runtime);
    let arena = runtime
        .allocate_guest_block_for_module(64, 0)
        .unwrap()
        .unwrap();
    register_dynamic_image(&mut runtime, 0, arena, 64);
    assert!(runtime.memory.fetch_u32(arena).is_ok());

    runtime.free_guest_block_for_module(arena, 64, 0).unwrap();
    assert!(runtime.memory.read_u32(arena).is_ok());
    assert!(runtime.memory.fetch_u32(arena).is_err());

    let reused = runtime
        .allocate_guest_block_for_module(64, 0)
        .unwrap()
        .unwrap();
    assert_eq!(reused, arena);
    assert!(runtime.memory.fetch_u32(reused).is_err());
}

#[test]
fn stale_tracked_tail_loses_execute_permission_on_explicit_short_free() {
    let mut runtime =
        ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    load_test_module(&mut runtime);
    let output = runtime
        .allocate_guest_block_for_module(0x100, 0)
        .unwrap()
        .unwrap();
    register_dynamic_image(&mut runtime, 0, output, 0x100);
    let tail_callback = output.checked_add(0xc0).unwrap();
    register_helper(&mut runtime, 0, tail_callback);
    let callback = runtime.modules[0].helper.unwrap();
    runtime
        .pending_external_action_completions
        .push_back(PendingExternalActionCompletion {
            owner_generation: runtime.modules[0].generation,
            callback,
            callback_data: 0,
        });

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

    runtime
        .free_guest_block_for_module(output, 0x80, 0)
        .unwrap();

    assert!(runtime.memory.read_u32(tail_callback).is_ok());
    assert!(runtime.memory.fetch_u32(tail_callback).is_err());
    assert!(runtime.modules[0].helper.is_none());
    assert!(runtime.active_helper.is_none());
    assert!(runtime.pending_external_action_completions.is_empty());
}

#[test]
fn heap_reset_reuse_revokes_the_complete_superseded_executable_extent() {
    let mut runtime =
        ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    load_test_module(&mut runtime);
    let output = runtime
        .allocate_guest_block_for_module(0x100, 0)
        .unwrap()
        .unwrap();
    register_dynamic_image(&mut runtime, 0, output, 0x100);
    let tail_callback = output.checked_add(0xc0).unwrap();
    register_helper(&mut runtime, 0, tail_callback);

    let reset_offset = output.0 - HEAP_BASE.0;
    runtime
        .memory
        .write_u32(output, DEFAULT_HEAP_LEN as u32)
        .unwrap();
    runtime
        .memory
        .write_u32(
            output.checked_add(4).unwrap(),
            DEFAULT_HEAP_LEN as u32 - reset_offset,
        )
        .unwrap();
    runtime
        .memory
        .write_u32(data_slot_address(146), reset_offset)
        .unwrap();
    runtime
        .memory
        .write_u32(
            data_slot_address(111),
            DEFAULT_HEAP_LEN as u32 - reset_offset,
        )
        .unwrap();

    let reused = runtime
        .allocate_guest_block_for_module(8, 0)
        .unwrap()
        .unwrap();

    assert_eq!(reused, output);
    assert_eq!(runtime.guest_allocations.get(&reused.0), Some(&8));
    assert!(runtime.memory.read_u32(tail_callback).is_ok());
    assert!(runtime.memory.fetch_u32(tail_callback).is_err());
    assert!(runtime.modules[0].helper.is_none());
    assert!(runtime.active_helper.is_none());
}

#[test]
fn partial_overwrite_preserves_tail_callbacks_until_the_backing_is_freed() {
    let mut runtime =
        ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    load_test_module(&mut runtime);
    let backing = runtime
        .allocate_guest_block_for_module(128, 0)
        .unwrap()
        .unwrap();
    let image_base = backing.checked_add(8).unwrap();
    let callback = image_base.checked_add(72).unwrap();
    register_dynamic_image(&mut runtime, 0, image_base, 96);
    register_helper(&mut runtime, 0, callback);
    let callback_function = runtime.modules[0].helper.unwrap();
    let original_image = callback_function.expected_image.unwrap();
    runtime
        .pending_external_action_completions
        .push_back(PendingExternalActionCompletion {
            owner_generation: runtime.modules[0].generation,
            callback: callback_function,
            callback_data: 0,
        });

    runtime
        .revoke_executable_ranges_in(ExecutableRange {
            base: image_base,
            len: 64,
        })
        .unwrap();

    assert!(runtime.memory.fetch_u32(image_base).is_err());
    assert!(runtime.memory.fetch_u32(callback).is_ok());
    assert_eq!(
        runtime.modules[0]
            .executable_image(callback.0)
            .map(|(image, _)| image),
        Some(original_image)
    );
    assert!(runtime.modules[0].helper.is_some_and(|helper| {
        helper.address == callback_function.address
            && helper.expected_image == callback_function.expected_image
    }));
    assert!(runtime.active_helper.is_some_and(|helper| {
        helper.address == callback_function.address
            && helper.expected_image == callback_function.expected_image
    }));
    assert_eq!(runtime.pending_external_action_completions.len(), 1);
    assert_eq!(
        runtime.modules[0].dynamic_executable_ranges[0]
            .as_ref()
            .unwrap()
            .intervals,
        [ExecutableRange {
            base: image_base.checked_add(64).unwrap(),
            len: 32,
        }]
    );

    register_dynamic_image(&mut runtime, 0, image_base, 64);
    let replacement_image = runtime.modules[0]
        .executable_image(image_base.0)
        .map(|(image, _)| image)
        .unwrap();
    assert_ne!(replacement_image, original_image);
    assert_eq!(
        runtime.modules[0]
            .executable_image(callback.0)
            .map(|(image, _)| image),
        Some(original_image)
    );

    runtime
        .free_guest_block_for_module(backing, 128, 0)
        .unwrap();
    assert!(runtime.memory.fetch_u32(image_base).is_err());
    assert!(runtime.memory.fetch_u32(callback).is_err());
    assert!(runtime.modules[0].helper.is_none());
    assert!(runtime.active_helper.is_none());
    assert!(runtime.pending_external_action_completions.is_empty());
    assert!(
        runtime.modules[0]
            .dynamic_executable_ranges
            .iter()
            .all(DynamicExecutableImageSlot::is_none)
    );
}

#[test]
fn releasing_an_unrelated_dynamic_image_preserves_a_static_helper() {
    let mut runtime =
        ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    load_test_module(&mut runtime);
    let static_helper = runtime.modules[0].base.checked_add(8).unwrap();
    register_helper(&mut runtime, 0, static_helper);

    let arena = allocate_platform_arena(&mut runtime, 0, 64);
    register_dynamic_image(&mut runtime, 0, arena, 64);
    release_platform_arena(&mut runtime, 0, arena);

    assert_eq!(runtime.active_helper.unwrap().address, static_helper.0);
    assert_eq!(runtime.modules[0].helper.unwrap().address, static_helper.0);
    assert!(
        runtime
            .call_active_helper(0, &[], &mut StubServices)
            .is_ok()
    );
}

#[test]
fn failed_module_initialization_restores_the_previous_helper_and_owned_state() {
    let mut runtime =
        ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    load_test_module(&mut runtime);
    let first_helper = runtime.modules[0].base.checked_add(8).unwrap();
    register_helper(&mut runtime, 0, first_helper);
    let allocations_before = runtime.guest_allocations.clone();
    let owners_before = runtime.guest_allocation_owners.clone();
    let free_before = runtime.guest_heap_state().unwrap().free_left;
    let failed_module_base = MODULE_BASE + MODULE_STRIDE;
    let failed_generation = runtime.next_module_generation;

    assert!(matches!(
        runtime.load_and_call_entry(
            &failing_initialization_image(failed_module_base),
            0,
            &mut StubServices,
        ),
        Err(Error::Abi(message)) if message.contains("unsupported platform slot 21")
    ));

    assert_eq!(runtime.modules.len(), 1);
    assert_eq!(runtime.active_helper.unwrap().address, first_helper.0);
    assert_eq!(runtime.guest_allocations, allocations_before);
    assert_eq!(runtime.guest_allocation_owners, owners_before);
    assert_eq!(runtime.guest_heap_state().unwrap().free_left, free_before);
    assert!(
        runtime
            .memory
            .read(GuestAddr(failed_module_base), 1)
            .is_err()
    );

    load_test_module(&mut runtime);
    assert_eq!(runtime.modules.len(), 2);
    assert!(runtime.modules[1].generation > failed_generation);
}

#[test]
fn generation_rollback_removes_dynamic_ranges_arenas_and_pending_callbacks() {
    let mut runtime =
        ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    runtime
        .set_native_extension_profile(NativeExtensionProfile::Mtk)
        .unwrap();
    let snapshot = ModuleLoadSnapshot {
        active_helper: runtime.active_helper,
        detached_guest_allocation_cursor: runtime.detached_guest_allocation_cursor,
        platform_memory_cursor: runtime.platform_memory_cursor,
        mtk_native_extension_owner: runtime.mtk_native_extension_owner,
    };
    load_test_module(&mut runtime);
    let generation = runtime.modules[0].generation;
    let module_base = runtime.modules[0].base;
    let arena = allocate_platform_arena(&mut runtime, 0, 64);
    register_dynamic_image(&mut runtime, 0, arena, 4);
    register_dynamic_image(&mut runtime, 0, MTK_NATIVE_EXTENSION_BASE, 4);
    assert!(runtime.memory.fetch_u32(MTK_NATIVE_EXTENSION_BASE).is_ok());
    register_helper(&mut runtime, 0, arena);
    let window = runtime.create_native_window(0).unwrap();
    let callback = runtime.modules[0].helper.unwrap();
    runtime
        .pending_external_action_completions
        .push_back(PendingExternalActionCompletion {
            owner_generation: generation,
            callback,
            callback_data: 0,
        });

    runtime
        .rollback_module_initialization(0, generation, snapshot)
        .unwrap();

    assert!(runtime.modules.is_empty());
    assert!(runtime.active_helper.is_none());
    assert!(runtime.pending_external_action_completions.is_empty());
    assert!(!runtime.native_windows.contains_key(&window));
    assert!(runtime.platform_memory_extensions.is_empty());
    assert!(runtime.guest_allocation_owners.is_empty());
    assert!(runtime.detached_guest_allocation_owners.is_empty());
    assert!(runtime.memory.read(arena, 1).is_err());
    assert!(runtime.memory.fetch_u32(arena).is_err());
    assert!(runtime.memory.read(module_base, 1).is_err());
    assert!(runtime.memory.read_u32(MTK_NATIVE_EXTENSION_BASE).is_ok());
    assert!(runtime.memory.fetch_u32(MTK_NATIVE_EXTENSION_BASE).is_err());
    assert_eq!(runtime.mtk_native_extension_owner, None);
}

#[test]
fn failed_rollback_preserves_owned_heap_tracking() {
    let mut runtime =
        ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    let snapshot = ModuleLoadSnapshot {
        active_helper: runtime.active_helper,
        detached_guest_allocation_cursor: runtime.detached_guest_allocation_cursor,
        platform_memory_cursor: runtime.platform_memory_cursor,
        mtk_native_extension_owner: runtime.mtk_native_extension_owner,
    };
    load_test_module(&mut runtime);
    let generation = runtime.modules[0].generation;
    runtime
        .allocate_guest_block_for_module(64, 0)
        .unwrap()
        .unwrap();
    let allocations_before = runtime.guest_allocations.clone();
    let owners_before = runtime.guest_allocation_owners.clone();
    let heap = runtime.guest_heap_state().unwrap();
    let free_header = GuestAddr(heap.base + heap.head);
    runtime.memory.write_u32(free_header, heap.head).unwrap();

    assert!(matches!(
        runtime.rollback_module_initialization(0, generation, snapshot),
        Err(Error::Abi(message)) if message.contains("free-list contains a cycle")
    ));

    assert!(runtime.modules.is_empty());
    assert_eq!(runtime.guest_allocations, allocations_before);
    assert_eq!(runtime.guest_allocation_owners, owners_before);
}

#[test]
fn failed_platform_unmap_preserves_arena_tracking_and_cursor() {
    let mut runtime =
        ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    let snapshot = ModuleLoadSnapshot {
        active_helper: runtime.active_helper,
        detached_guest_allocation_cursor: runtime.detached_guest_allocation_cursor,
        platform_memory_cursor: runtime.platform_memory_cursor,
        mtk_native_extension_owner: runtime.mtk_native_extension_owner,
    };
    load_test_module(&mut runtime);
    let generation = runtime.modules[0].generation;
    let arena = allocate_platform_arena(&mut runtime, 0, 64);
    let cursor_after_allocation = runtime.platform_memory_cursor;
    runtime.memory.unmap(arena, 64).unwrap();

    assert!(
        runtime
            .rollback_module_initialization(0, generation, snapshot)
            .is_err()
    );

    assert!(runtime.modules.is_empty());
    assert!(runtime.platform_memory_extensions.contains_key(&arena.0));
    assert_eq!(runtime.platform_memory_cursor, cursor_after_allocation);
}
