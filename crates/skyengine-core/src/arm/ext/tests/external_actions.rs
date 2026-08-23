use super::*;

const ACTION_OFFSET: u32 = 0;
const CALLBACK_OFFSET: u32 = 16;
const RETURN_OFFSET: u32 = 28;
const ACTION_KIND: u32 = 2;

fn fixture_image() -> Vec<u8> {
    let mut image = b"MRPGCMAP".to_vec();
    image.extend_from_slice(&0xe12f_ff1e_u32.to_le_bytes());
    image.extend_from_slice(&[0; 32]);
    image
}

fn runtime_with_modules(count: usize) -> ExtRuntime {
    let mut runtime =
        ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    for _ in 0..count {
        runtime
            .load_and_call_entry(&fixture_image(), 0, &mut StubServices)
            .unwrap();
    }
    runtime
}

fn write_c_string(runtime: &mut ExtRuntime, value: &[u8]) -> GuestAddr {
    let address = runtime.allocate(value.len() + 1, 1).unwrap();
    runtime.memory.write(address, value).unwrap();
    runtime
        .memory
        .write_u8(address.checked_add(value.len() as u32).unwrap(), 0)
        .unwrap();
    address
}

fn write_request(
    runtime: &mut ExtRuntime,
    kind: u32,
    identifier: &[u8],
    callback: u32,
    callback_data: u32,
) -> GuestAddr {
    let identifier = write_c_string(runtime, identifier);
    let description = write_c_string(runtime, b"bounded fixture payload");
    let request = runtime.allocate(44, 4).unwrap();
    for (index, word) in [
        0,
        0,
        20,
        kind,
        identifier.0,
        description.0,
        0,
        0,
        0,
        callback_data,
        callback,
    ]
    .into_iter()
    .enumerate()
    {
        runtime
            .memory
            .write_u32(request.checked_add((index * 4) as u32).unwrap(), word)
            .unwrap();
    }
    request
}

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
    let mut cpu = ArmCpu::new();
    cpu.set_register(0, 0);
    cpu.set_register(1, 9);
    cpu.set_register(2, address.0);
    cpu.set_register(3, len);
    runtime
        .dispatch(131, module, &mut cpu, &mut StubServices)
        .unwrap();
}

fn dynamic_fixture(runtime: &mut ExtRuntime, module: usize) -> GuestAddr {
    let arena = allocate_platform_arena(runtime, module, 64);
    runtime
        .memory
        .write(
            arena.checked_add(CALLBACK_OFFSET).unwrap(),
            &[
                0x00, 0x00, 0x81, 0xe5, // str r0, [r1]
                0x04, 0x90, 0x81, 0xe5, // str r9, [r1, #4]
                0x1e, 0xff, 0x2f, 0xe1, // bx lr
                0x1e, 0xff, 0x2f, 0xe1, // return target: bx lr
            ],
        )
        .unwrap();
    register_dynamic_image(runtime, module, arena, 64);
    arena
}

fn call_cpu(arena: GuestAddr, request: GuestAddr, r9: u32, thumb_entry: bool) -> ArmCpu {
    let mut cpu = ArmCpu::new();
    cpu.set_register(0, request.0);
    cpu.set_register(9, r9);
    cpu.set_register(14, arena.0 + RETURN_OFFSET);
    cpu.set_pc(arena.0 + ACTION_OFFSET + u32::from(thumb_entry));
    cpu
}

fn release_platform_arena(runtime: &mut ExtRuntime, module: usize, address: GuestAddr) {
    let mut cpu = ArmCpu::new();
    cpu.set_register(0, 1_015);
    cpu.set_register(1, address.0);
    cpu.set_register(2, 4);
    runtime
        .dispatch(38, module, &mut cpu, &mut StubServices)
        .unwrap();
}

#[test]
fn bounded_dynamic_callback_abi_uses_internal_policy() {
    let mut runtime = runtime_with_modules(1);
    let arena = dynamic_fixture(&mut runtime, 0);
    let output = runtime.allocate(8, 4).unwrap();
    runtime.memory.write(output, &[0xff; 8]).unwrap();
    let request = write_request(
        &mut runtime,
        ACTION_KIND,
        b"opaque-id-not-used-for-routing",
        arena.0 + CALLBACK_OFFSET,
        output.0,
    );
    let captured_r9 = 0x2468_ace0;
    let mut cpu = call_cpu(arena, request, captured_r9, true);

    assert!(
        runtime
            .try_dispatch_legacy_external_action(0, false, &mut cpu)
            .unwrap()
    );
    assert_eq!(cpu.register(0), 1);
    assert_eq!(cpu.pc().0, arena.0 + RETURN_OFFSET);
    assert_eq!(runtime.memory.read(output, 8).unwrap(), [0xff; 8]);

    runtime.modules[0].static_base_r9 = 0xdead_beef;
    assert!(
        runtime
            .dispatch_pending_external_action(&mut StubServices)
            .unwrap()
    );
    assert_eq!(runtime.memory.read_u32(output).unwrap(), 0);
    assert_eq!(
        runtime
            .memory
            .read_u32(output.checked_add(4).unwrap())
            .unwrap(),
        captured_r9
    );
}

#[test]
fn static_callees_and_cross_image_control_flow_are_not_mocked() {
    let mut runtime = runtime_with_modules(2);
    let local = dynamic_fixture(&mut runtime, 0);
    let foreign = dynamic_fixture(&mut runtime, 1);
    let output = runtime.allocate(8, 4).unwrap();

    let local_request = write_request(
        &mut runtime,
        ACTION_KIND,
        b"local",
        local.0 + CALLBACK_OFFSET,
        output.0,
    );
    let mut static_cpu = call_cpu(local, local_request, 1, false);
    static_cpu.set_pc(runtime.modules[0].base.0 + 12);
    assert!(
        !runtime
            .try_dispatch_legacy_external_action(0, false, &mut static_cpu)
            .unwrap()
    );

    let foreign_request = write_request(
        &mut runtime,
        ACTION_KIND,
        b"foreign",
        foreign.0 + CALLBACK_OFFSET,
        output.0,
    );
    let mut foreign_callback = call_cpu(local, foreign_request, 2, false);
    assert!(
        !runtime
            .try_dispatch_legacy_external_action(0, false, &mut foreign_callback)
            .unwrap()
    );

    let mut foreign_return = call_cpu(local, local_request, 3, false);
    foreign_return.set_register(14, foreign.0 + RETURN_OFFSET);
    assert!(
        !runtime
            .try_dispatch_legacy_external_action(0, false, &mut foreign_return)
            .unwrap()
    );

    let second_local = dynamic_fixture(&mut runtime, 0);
    let cross_image_request = write_request(
        &mut runtime,
        ACTION_KIND,
        b"same-module-different-image",
        second_local.0 + CALLBACK_OFFSET,
        output.0,
    );
    let mut cross_image_callback = call_cpu(local, cross_image_request, 4, false);
    assert!(
        !runtime
            .try_dispatch_legacy_external_action(0, false, &mut cross_image_callback)
            .unwrap()
    );
    assert!(runtime.pending_external_action_completions.is_empty());
}

#[test]
fn unaligned_a32_callback_and_return_addresses_are_not_mocked() {
    let mut runtime = runtime_with_modules(1);
    let arena = dynamic_fixture(&mut runtime, 0);
    let output = runtime.allocate(8, 4).unwrap();

    let request = write_request(
        &mut runtime,
        ACTION_KIND,
        b"unaligned-callback",
        arena.0 + CALLBACK_OFFSET + 2,
        output.0,
    );
    let mut callback = call_cpu(arena, request, 1, false);
    assert!(
        !runtime
            .try_dispatch_legacy_external_action(0, false, &mut callback)
            .unwrap()
    );

    let request = write_request(
        &mut runtime,
        ACTION_KIND,
        b"unaligned-return",
        arena.0 + CALLBACK_OFFSET,
        output.0,
    );
    let mut return_address = call_cpu(arena, request, 2, false);
    return_address.set_register(14, arena.0 + RETURN_OFFSET + 2);
    assert!(
        !runtime
            .try_dispatch_legacy_external_action(0, false, &mut return_address)
            .unwrap()
    );
    assert!(runtime.pending_external_action_completions.is_empty());
}

#[test]
fn malformed_or_unlisted_requests_continue_into_guest_code() {
    let mut runtime = runtime_with_modules(1);
    let arena = dynamic_fixture(&mut runtime, 0);
    let output = runtime.allocate(8, 4).unwrap();

    let valid = write_request(
        &mut runtime,
        ACTION_KIND,
        b"valid-for-pointer-checks",
        arena.0 + CALLBACK_OFFSET,
        output.0,
    );
    let mut null_request = call_cpu(arena, valid, 0, false);
    null_request.set_register(0, 0);
    assert!(
        !runtime
            .try_dispatch_legacy_external_action(0, false, &mut null_request)
            .unwrap()
    );
    let mut unaligned_request = call_cpu(arena, valid, 0, false);
    unaligned_request.set_register(0, valid.0 + 2);
    assert!(
        !runtime
            .try_dispatch_legacy_external_action(0, false, &mut unaligned_request)
            .unwrap()
    );

    for identifier in [Vec::new(), vec![b'i'; 65]] {
        let request = write_request(
            &mut runtime,
            ACTION_KIND,
            &identifier,
            arena.0 + CALLBACK_OFFSET,
            output.0,
        );
        let mut cpu = call_cpu(arena, request, 0, false);
        assert!(
            !runtime
                .try_dispatch_legacy_external_action(0, false, &mut cpu)
                .unwrap()
        );
    }

    let long_description = write_c_string(&mut runtime, &vec![b'd'; 257]);
    let request = write_request(
        &mut runtime,
        ACTION_KIND,
        b"long-description",
        arena.0 + CALLBACK_OFFSET,
        output.0,
    );
    runtime
        .memory
        .write_u32(request.checked_add(20).unwrap(), long_description.0)
        .unwrap();
    let mut cpu = call_cpu(arena, request, 0, false);
    assert!(
        !runtime
            .try_dispatch_legacy_external_action(0, false, &mut cpu)
            .unwrap()
    );

    for kind in [0, 1, 3, u32::MAX] {
        let unlisted = write_request(
            &mut runtime,
            kind,
            b"unlisted-kind",
            arena.0 + CALLBACK_OFFSET,
            output.0,
        );
        let mut cpu = call_cpu(arena, unlisted, 1, false);
        assert!(
            !runtime
                .try_dispatch_legacy_external_action(0, false, &mut cpu)
                .unwrap()
        );
    }

    let malformed = write_request(
        &mut runtime,
        ACTION_KIND,
        b"reserved-field",
        arena.0 + CALLBACK_OFFSET,
        output.0,
    );
    runtime
        .memory
        .write_u32(malformed.checked_add(24).unwrap(), 1)
        .unwrap();
    let mut cpu = call_cpu(arena, malformed, 2, false);
    assert!(
        !runtime
            .try_dispatch_legacy_external_action(0, false, &mut cpu)
            .unwrap()
    );

    let bad_pointer = write_request(
        &mut runtime,
        ACTION_KIND,
        b"bad-pointer",
        arena.0 + CALLBACK_OFFSET,
        output.0,
    );
    runtime
        .memory
        .write_u32(bad_pointer.checked_add(16).unwrap(), 0xffff_0000)
        .unwrap();
    let mut cpu = call_cpu(arena, bad_pointer, 3, false);
    assert!(
        !runtime
            .try_dispatch_legacy_external_action(0, false, &mut cpu)
            .unwrap()
    );
    assert!(runtime.pending_external_action_completions.is_empty());
}

#[test]
fn queue_limit_rejects_a_recognized_action_without_running_unknown_code() {
    let mut runtime = runtime_with_modules(1);
    let arena = dynamic_fixture(&mut runtime, 0);
    let output = runtime.allocate(8, 4).unwrap();
    let request = write_request(
        &mut runtime,
        ACTION_KIND,
        b"queue-limit",
        arena.0 + CALLBACK_OFFSET,
        output.0,
    );
    let callback = GuestFunction {
        module: 0,
        address: arena.0 + CALLBACK_OFFSET,
        expected_image: Some(ExecutableImage::Dynamic(0)),
        captured_r9: Some(0),
    };
    for _ in 0..MAX_PENDING_EXTERNAL_ACTIONS {
        runtime
            .pending_external_action_completions
            .push_back(PendingExternalActionCompletion {
                owner_generation: runtime.modules[0].generation,
                callback,
                callback_data: output.0,
            });
    }
    let mut cpu = call_cpu(arena, request, 4, false);

    assert!(
        runtime
            .try_dispatch_legacy_external_action(0, false, &mut cpu)
            .unwrap()
    );
    assert_eq!(cpu.register(0), 0);
    assert_eq!(cpu.pc().0, arena.0 + RETURN_OFFSET);
    assert_eq!(
        runtime.pending_external_action_completions.len(),
        MAX_PENDING_EXTERNAL_ACTIONS
    );
}

#[test]
fn release_and_owner_generation_revoke_queued_callbacks() {
    let mut runtime = runtime_with_modules(1);
    let arena = dynamic_fixture(&mut runtime, 0);
    let output = runtime.allocate(8, 4).unwrap();
    runtime.memory.write(output, &[0xff; 8]).unwrap();
    let request = write_request(
        &mut runtime,
        ACTION_KIND,
        b"release",
        arena.0 + CALLBACK_OFFSET,
        output.0,
    );
    let mut cpu = call_cpu(arena, request, 5, false);
    assert!(
        runtime
            .try_dispatch_legacy_external_action(0, false, &mut cpu)
            .unwrap()
    );
    release_platform_arena(&mut runtime, 0, arena);
    assert!(runtime.pending_external_action_completions.is_empty());

    let second = dynamic_fixture(&mut runtime, 0);
    let request = write_request(
        &mut runtime,
        ACTION_KIND,
        b"stale-generation",
        second.0 + CALLBACK_OFFSET,
        output.0,
    );
    let mut cpu = call_cpu(second, request, 6, false);
    assert!(
        runtime
            .try_dispatch_legacy_external_action(0, false, &mut cpu)
            .unwrap()
    );
    runtime.modules[0].generation += 1;
    assert!(
        runtime
            .dispatch_pending_external_action(&mut StubServices)
            .unwrap()
    );
    assert_eq!(runtime.memory.read(output, 8).unwrap(), [0xff; 8]);
}

#[test]
fn executable_image_slots_are_reused_after_release() {
    let mut runtime = runtime_with_modules(1);

    for _ in 0..=64 {
        let arena = dynamic_fixture(&mut runtime, 0);
        release_platform_arena(&mut runtime, 0, arena);
    }

    assert_eq!(runtime.modules[0].dynamic_executable_ranges.len(), 1);
    assert_eq!(runtime.modules[0].dynamic_executable_ranges, [None]);
}

#[test]
fn a_logically_freed_heap_image_revokes_its_queued_callback() {
    let mut runtime = runtime_with_modules(1);
    let arena = runtime
        .allocate_guest_block_for_module(64, 0)
        .unwrap()
        .unwrap();
    runtime
        .memory
        .write(
            arena.checked_add(CALLBACK_OFFSET).unwrap(),
            &[
                0x00, 0x00, 0x81, 0xe5, 0x04, 0x90, 0x81, 0xe5, 0x1e, 0xff, 0x2f, 0xe1, 0x1e, 0xff,
                0x2f, 0xe1,
            ],
        )
        .unwrap();
    register_dynamic_image(&mut runtime, 0, arena, 64);
    assert!(runtime.memory.fetch_u32(arena).is_ok());
    let output = runtime.allocate(8, 4).unwrap();
    let request = write_request(
        &mut runtime,
        ACTION_KIND,
        b"logically-freed",
        arena.0 + CALLBACK_OFFSET,
        output.0,
    );
    let mut cpu = call_cpu(arena, request, 7, false);
    assert!(
        runtime
            .try_dispatch_legacy_external_action(0, false, &mut cpu)
            .unwrap()
    );

    let heap = runtime.guest_heap_state().unwrap();
    runtime
        .write_free_blocks(
            heap,
            &[FreeBlock {
                offset: 0,
                len: heap.span,
            }],
            heap.span,
            heap.span,
        )
        .unwrap();
    runtime.free_guest_block(arena, 64).unwrap();

    assert_eq!(runtime.modules[0].dynamic_executable_ranges, [None]);
    assert!(runtime.pending_external_action_completions.is_empty());
    assert!(runtime.memory.read_u32(arena).is_ok());
    assert!(runtime.memory.fetch_u32(arena).is_err());
}
