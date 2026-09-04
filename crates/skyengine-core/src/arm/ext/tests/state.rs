use std::{io::Write, time::Instant};

use flate2::{Compression, write::GzEncoder};

use super::*;

#[test]
fn discovers_repeating_timers_from_registered_runtime_data() {
    let mut runtime =
        ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    load_test_module(&mut runtime);
    let owner_generation = runtime.modules[0].generation;
    let parameter = runtime
        .allocate_guest_block_for_module(MODULE_PARAMETER_LEN, 0)
        .unwrap()
        .unwrap();
    let ext_chunk = runtime
        .allocate_guest_block_for_module(EXT_CHUNK_TIMER_STATE_LEN, 0)
        .unwrap()
        .unwrap();
    let static_base = runtime
        .allocate_guest_block_for_module(0x40, 0)
        .unwrap()
        .unwrap();
    let timer_reference = static_base.checked_add(0x2c).unwrap();
    let node = runtime
        .allocate_guest_block_for_module(COMPACT_TIMER_NODE_LEN, 0)
        .unwrap()
        .unwrap();
    let image_address = runtime
        .allocate_guest_block_for_module(64, 0)
        .unwrap()
        .unwrap();
    for (address, len) in [
        (parameter, MODULE_PARAMETER_LEN),
        (ext_chunk, EXT_CHUNK_TIMER_STATE_LEN),
        (static_base, 0x40),
        (node, COMPACT_TIMER_NODE_LEN),
        (image_address, 64),
    ] {
        runtime.memory.write(address, &vec![0; len]).unwrap();
    }
    runtime.memory.write_u32(static_base, 0xe12f_ff1e).unwrap();
    runtime
        .memory
        .add_permissions(static_base, 4, Permissions::EXECUTE)
        .unwrap();
    runtime.memory.write_u32(parameter, static_base.0).unwrap();
    runtime
        .memory
        .write_u32(
            parameter
                .checked_add(MODULE_PARAMETER_RW_LEN_OFFSET)
                .unwrap(),
            0x40,
        )
        .unwrap();
    runtime
        .memory
        .write_u32(
            parameter
                .checked_add(MODULE_PARAMETER_EXT_CHUNK_OFFSET)
                .unwrap(),
            ext_chunk.0,
        )
        .unwrap();
    runtime
        .memory
        .write_u32(ext_chunk, EXT_CHUNK_MAGIC)
        .unwrap();
    for (offset, value) in [
        (
            EXT_CHUNK_ENTRY_OFFSET,
            image_address
                .checked_add(DYNAMIC_IMAGE_ENTRY_OFFSET)
                .unwrap()
                .0,
        ),
        (EXT_CHUNK_IMAGE_ADDRESS_OFFSET, image_address.0),
        (EXT_CHUNK_IMAGE_LEN_OFFSET, 64),
        (EXT_CHUNK_PARAMETER_OFFSET, parameter.0),
        (EXT_CHUNK_PARAMETER_LEN_OFFSET, MODULE_PARAMETER_LEN as u32),
    ] {
        runtime
            .memory
            .write_u32(ext_chunk.checked_add(offset).unwrap(), value)
            .unwrap();
    }
    runtime
        .memory
        .write_u32(
            ext_chunk
                .checked_add(EXT_CHUNK_SUSPEND_DEPTH_OFFSET)
                .unwrap(),
            0,
        )
        .unwrap();
    runtime.memory.write_u32(timer_reference, node.0).unwrap();
    runtime
        .memory
        .write_u32(timer_reference.checked_add(4).unwrap(), node.0)
        .unwrap();
    runtime.memory.write_u32(node, COMPACT_TIMER_MAGIC).unwrap();
    runtime
        .memory
        .write_u32(node.checked_add(COMPACT_TIMER_PERIOD_OFFSET).unwrap(), 100)
        .unwrap();
    runtime
        .memory
        .write_u32(
            node.checked_add(COMPACT_TIMER_HANDLER_OFFSET).unwrap(),
            static_base.0,
        )
        .unwrap();
    runtime
        .memory
        .write_u32(node.checked_add(COMPACT_TIMER_REPEAT_OFFSET).unwrap(), 1)
        .unwrap();

    runtime.memory.write_u32(parameter, 0).unwrap();
    runtime
        .memory
        .write_u32(
            parameter
                .checked_add(MODULE_PARAMETER_RW_LEN_OFFSET)
                .unwrap(),
            0,
        )
        .unwrap();
    let mut image = vec![0xa5; 64];
    image[DYNAMIC_IMAGE_PARAMETER_OFFSET..DYNAMIC_IMAGE_PARAMETER_OFFSET + 4]
        .copy_from_slice(&parameter.0.to_le_bytes());
    assert_eq!(
        runtime.registered_dynamic_image_parameter(
            &image,
            image_address,
            image.len() as u32,
            owner_generation,
        ),
        Some(parameter)
    );
    runtime
        .memory
        .write_u32(
            ext_chunk.checked_add(EXT_CHUNK_IMAGE_LEN_OFFSET).unwrap(),
            63,
        )
        .unwrap();
    assert_eq!(
        runtime.registered_dynamic_image_parameter(
            &image,
            image_address,
            image.len() as u32,
            owner_generation,
        ),
        None
    );
    runtime
        .memory
        .write_u32(
            ext_chunk.checked_add(EXT_CHUNK_IMAGE_LEN_OFFSET).unwrap(),
            64,
        )
        .unwrap();
    runtime.memory.write_u32(parameter, static_base.0).unwrap();
    runtime
        .memory
        .write_u32(
            parameter
                .checked_add(MODULE_PARAMETER_RW_LEN_OFFSET)
                .unwrap(),
            0x40,
        )
        .unwrap();
    runtime.modules[0]
        .dynamic_executable_ranges
        .push(DynamicExecutableImageSlot(Some(DynamicExecutableImage {
            id: 7,
            intervals: vec![ExecutableRange {
                base: static_base,
                len: 4,
            }],
            module_parameter: Some(parameter),
            compact_repeating_timers: Vec::new(),
        })));

    runtime.discover_compact_repeating_timers();
    assert_eq!(
        runtime.modules[0].dynamic_executable_ranges[0]
            .as_ref()
            .unwrap()
            .compact_repeating_timers,
        vec![node]
    );
    runtime.modules[0].dynamic_executable_ranges[0]
        .as_mut()
        .unwrap()
        .compact_repeating_timers
        .clear();
    let wrapper_handler = runtime.modules[0].base.0 + 8;
    runtime
        .memory
        .write_u32(
            node.checked_add(COMPACT_TIMER_HANDLER_OFFSET).unwrap(),
            wrapper_handler,
        )
        .unwrap();
    runtime.discover_compact_repeating_timers();
    assert!(
        runtime.modules[0].dynamic_executable_ranges[0]
            .as_ref()
            .unwrap()
            .compact_repeating_timers
            .is_empty()
    );
    load_test_module(&mut runtime);
    assert_eq!(
        runtime.registered_dynamic_image_parameter(
            &image,
            image_address,
            image.len() as u32,
            runtime.modules[1].generation,
        ),
        None
    );
}

#[test]
fn repeating_timer_discovery_rotates_its_shared_scan_budget() {
    const FIRST_RW_LEN: usize = 700 * 1024;
    const SECOND_RW_LEN: usize = 400 * 1024;

    let mut runtime =
        ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    load_test_module(&mut runtime);
    let owner_generation = runtime.modules[0].generation;
    let first_parameter = runtime
        .allocate_guest_block_for_module(MODULE_PARAMETER_LEN, 0)
        .unwrap()
        .unwrap();
    let first_ext_chunk = runtime
        .allocate_guest_block_for_module(EXT_CHUNK_TIMER_STATE_LEN, 0)
        .unwrap()
        .unwrap();
    let first_rw = runtime
        .allocate_guest_block_for_module(FIRST_RW_LEN, 0)
        .unwrap()
        .unwrap();
    let second_parameter = runtime
        .allocate_guest_block_for_module(MODULE_PARAMETER_LEN, 0)
        .unwrap()
        .unwrap();
    let second_ext_chunk = runtime
        .allocate_guest_block_for_module(EXT_CHUNK_TIMER_STATE_LEN, 0)
        .unwrap()
        .unwrap();
    let second_rw = runtime
        .allocate_guest_block_for_module(SECOND_RW_LEN, 0)
        .unwrap()
        .unwrap();
    let first_node = runtime
        .allocate_guest_block_for_module(COMPACT_TIMER_NODE_LEN, 0)
        .unwrap()
        .unwrap();
    let node = runtime
        .allocate_guest_block_for_module(COMPACT_TIMER_NODE_LEN, 0)
        .unwrap()
        .unwrap();
    for (address, len) in [
        (first_parameter, MODULE_PARAMETER_LEN),
        (first_ext_chunk, EXT_CHUNK_TIMER_STATE_LEN),
        (first_rw, FIRST_RW_LEN),
        (second_parameter, MODULE_PARAMETER_LEN),
        (second_ext_chunk, EXT_CHUNK_TIMER_STATE_LEN),
        (second_rw, SECOND_RW_LEN),
        (first_node, COMPACT_TIMER_NODE_LEN),
        (node, COMPACT_TIMER_NODE_LEN),
    ] {
        runtime.memory.write(address, &vec![0; len]).unwrap();
    }
    for (parameter, ext_chunk, rw, len) in [
        (first_parameter, first_ext_chunk, first_rw, FIRST_RW_LEN),
        (second_parameter, second_ext_chunk, second_rw, SECOND_RW_LEN),
    ] {
        runtime.memory.write_u32(parameter, rw.0).unwrap();
        runtime
            .memory
            .write_u32(
                parameter
                    .checked_add(MODULE_PARAMETER_RW_LEN_OFFSET)
                    .unwrap(),
                len as u32,
            )
            .unwrap();
        runtime
            .memory
            .write_u32(
                parameter
                    .checked_add(MODULE_PARAMETER_EXT_CHUNK_OFFSET)
                    .unwrap(),
                ext_chunk.0,
            )
            .unwrap();
        runtime
            .memory
            .write_u32(ext_chunk, EXT_CHUNK_MAGIC)
            .unwrap();
    }

    let first_handler = first_rw.checked_add(4).unwrap();
    runtime.memory.write_u32(first_rw, first_node.0).unwrap();
    runtime
        .memory
        .write_u32(first_handler, 0xe12f_ff1e)
        .unwrap();
    runtime
        .memory
        .add_permissions(first_handler, 4, Permissions::EXECUTE)
        .unwrap();
    runtime
        .memory
        .write_u32(first_node, COMPACT_TIMER_MAGIC)
        .unwrap();
    runtime
        .memory
        .write_u32(
            first_node.checked_add(COMPACT_TIMER_PERIOD_OFFSET).unwrap(),
            80,
        )
        .unwrap();
    runtime
        .memory
        .write_u32(
            first_node
                .checked_add(COMPACT_TIMER_HANDLER_OFFSET)
                .unwrap(),
            first_handler.0,
        )
        .unwrap();
    runtime
        .memory
        .write_u32(
            first_node.checked_add(COMPACT_TIMER_REPEAT_OFFSET).unwrap(),
            1,
        )
        .unwrap();

    let handler = second_rw.checked_add(4).unwrap();
    runtime.memory.write_u32(second_rw, node.0).unwrap();
    runtime.memory.write_u32(handler, 0xe12f_ff1e).unwrap();
    runtime
        .memory
        .add_permissions(handler, 4, Permissions::EXECUTE)
        .unwrap();
    runtime.memory.write_u32(node, COMPACT_TIMER_MAGIC).unwrap();
    runtime
        .memory
        .write_u32(node.checked_add(COMPACT_TIMER_PERIOD_OFFSET).unwrap(), 100)
        .unwrap();
    runtime
        .memory
        .write_u32(
            node.checked_add(COMPACT_TIMER_HANDLER_OFFSET).unwrap(),
            handler.0,
        )
        .unwrap();
    runtime
        .memory
        .write_u32(node.checked_add(COMPACT_TIMER_REPEAT_OFFSET).unwrap(), 1)
        .unwrap();
    runtime.modules[0].dynamic_executable_ranges.extend([
        DynamicExecutableImageSlot(Some(DynamicExecutableImage {
            id: 7,
            intervals: vec![ExecutableRange {
                base: first_handler,
                len: 4,
            }],
            module_parameter: Some(first_parameter),
            compact_repeating_timers: Vec::new(),
        })),
        DynamicExecutableImageSlot(Some(DynamicExecutableImage {
            id: 8,
            intervals: vec![ExecutableRange {
                base: handler,
                len: 4,
            }],
            module_parameter: Some(second_parameter),
            compact_repeating_timers: Vec::new(),
        })),
    ]);
    runtime.compact_timer_scan_cursor = 0;

    runtime.discover_compact_repeating_timers();
    assert_eq!(
        runtime.modules[0].dynamic_executable_ranges[0]
            .as_ref()
            .unwrap()
            .compact_repeating_timers,
        vec![first_node]
    );
    assert!(
        runtime.modules[0].dynamic_executable_ranges[1]
            .as_ref()
            .unwrap()
            .compact_repeating_timers
            .is_empty()
    );
    runtime.discover_compact_repeating_timers();
    assert_eq!(
        runtime.modules[0].dynamic_executable_ranges[1]
            .as_ref()
            .unwrap()
            .compact_repeating_timers,
        vec![node]
    );
    assert_eq!(runtime.modules[0].generation, owner_generation);

    let states = runtime.current_repeating_timer_states();
    runtime.modal_repeating_timers.push(states[0].clone());
    assert!(!runtime.modal_timer_state_fits_budget(&states[1]));
    runtime.modal_repeating_timers.clear();

    let entering = runtime.modal_timer_observations().unwrap();
    assert_eq!(entering.len(), 2);
    runtime
        .memory
        .write_u32(
            second_ext_chunk
                .checked_add(EXT_CHUNK_SUSPEND_DEPTH_OFFSET)
                .unwrap(),
            1,
        )
        .unwrap();
    runtime.finish_modal_timer_observations(entering).unwrap();
    assert_eq!(runtime.modal_repeating_timers.len(), 1);
    assert_eq!(runtime.modal_repeating_timers[0].image_id, 8);

    let leaving = runtime.modal_timer_observations().unwrap();
    runtime
        .memory
        .write_u32(node.checked_add(COMPACT_TIMER_PERIOD_OFFSET).unwrap(), 10)
        .unwrap();
    runtime
        .memory
        .write_u32(
            second_ext_chunk
                .checked_add(EXT_CHUNK_SUSPEND_DEPTH_OFFSET)
                .unwrap(),
            0,
        )
        .unwrap();
    runtime.finish_modal_timer_observations(leaving).unwrap();
    assert_eq!(
        runtime
            .memory
            .read_u32(node.checked_add(COMPACT_TIMER_PERIOD_OFFSET).unwrap())
            .unwrap(),
        100
    );
    assert!(runtime.modal_repeating_timers.is_empty());

    runtime
        .memory
        .write_u32(node.checked_add(COMPACT_TIMER_DATA_OFFSET).unwrap(), 0)
        .unwrap();
    runtime
        .memory
        .write_u32(node.checked_add(COMPACT_TIMER_PERIOD_OFFSET).unwrap(), 100)
        .unwrap();
    runtime.modules[0].dynamic_executable_ranges[1]
        .as_mut()
        .unwrap()
        .compact_repeating_timers = vec![node];
    runtime
        .memory
        .write_u32(
            second_ext_chunk
                .checked_add(EXT_CHUNK_SUSPEND_DEPTH_OFFSET)
                .unwrap(),
            1,
        )
        .unwrap();
    let first_suspended_observation = runtime.modal_timer_observations().unwrap();
    assert_eq!(runtime.modal_repeating_timers.len(), 1);
    runtime
        .memory
        .write_u32(node.checked_add(COMPACT_TIMER_PERIOD_OFFSET).unwrap(), 10)
        .unwrap();
    runtime
        .memory
        .write_u32(
            second_ext_chunk
                .checked_add(EXT_CHUNK_SUSPEND_DEPTH_OFFSET)
                .unwrap(),
            0,
        )
        .unwrap();
    runtime
        .finish_modal_timer_observations(first_suspended_observation)
        .unwrap();
    assert_eq!(
        runtime
            .memory
            .read_u32(node.checked_add(COMPACT_TIMER_PERIOD_OFFSET).unwrap())
            .unwrap(),
        100
    );

    let entering = runtime.modal_timer_observations().unwrap();
    runtime
        .memory
        .write_u32(
            second_ext_chunk
                .checked_add(EXT_CHUNK_SUSPEND_DEPTH_OFFSET)
                .unwrap(),
            1,
        )
        .unwrap();
    runtime.finish_modal_timer_observations(entering).unwrap();
    let obscured_exit = runtime.modal_timer_observations().unwrap();
    runtime
        .memory
        .write_u32(node.checked_add(COMPACT_TIMER_PERIOD_OFFSET).unwrap(), 10)
        .unwrap();
    runtime
        .memory
        .write_u32(
            second_ext_chunk
                .checked_add(EXT_CHUNK_SUSPEND_DEPTH_OFFSET)
                .unwrap(),
            0,
        )
        .unwrap();
    runtime.memory.write_u32(second_ext_chunk, 0).unwrap();
    runtime
        .finish_modal_timer_observations(obscured_exit)
        .unwrap();
    assert_eq!(runtime.modal_repeating_timers.len(), 1);

    runtime
        .memory
        .write_u32(second_ext_chunk, EXT_CHUNK_MAGIC)
        .unwrap();
    runtime.modal_timer_observations().unwrap();
    assert!(runtime.modal_repeating_timers.is_empty());
    assert_eq!(
        runtime
            .memory
            .read_u32(node.checked_add(COMPACT_TIMER_PERIOD_OFFSET).unwrap())
            .unwrap(),
        100
    );

    let replacement_rw = runtime
        .allocate_guest_block_for_module(SECOND_RW_LEN, 0)
        .unwrap()
        .unwrap();
    runtime
        .memory
        .write(replacement_rw, &vec![0; SECOND_RW_LEN])
        .unwrap();
    runtime.memory.write_u32(replacement_rw, node.0).unwrap();
    let entering = runtime.modal_timer_observations().unwrap();
    runtime
        .memory
        .write_u32(
            second_ext_chunk
                .checked_add(EXT_CHUNK_SUSPEND_DEPTH_OFFSET)
                .unwrap(),
            1,
        )
        .unwrap();
    runtime.finish_modal_timer_observations(entering).unwrap();
    let leaving = runtime.modal_timer_observations().unwrap();
    runtime
        .memory
        .write_u32(node.checked_add(COMPACT_TIMER_PERIOD_OFFSET).unwrap(), 10)
        .unwrap();
    runtime
        .memory
        .write_u32(second_parameter, replacement_rw.0)
        .unwrap();
    runtime
        .memory
        .write_u32(
            second_ext_chunk
                .checked_add(EXT_CHUNK_SUSPEND_DEPTH_OFFSET)
                .unwrap(),
            0,
        )
        .unwrap();
    runtime.finish_modal_timer_observations(leaving).unwrap();
    assert_eq!(
        runtime
            .memory
            .read_u32(node.checked_add(COMPACT_TIMER_PERIOD_OFFSET).unwrap())
            .unwrap(),
        10
    );
}

#[test]
fn modal_return_restores_only_the_repeating_timer_period() {
    let mut runtime =
        ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    load_test_module(&mut runtime);
    let parameter = runtime
        .allocate_guest_block_for_module(MODULE_PARAMETER_LEN, 0)
        .unwrap()
        .unwrap();
    let ext_chunk = runtime
        .allocate_guest_block_for_module(EXT_CHUNK_TIMER_STATE_LEN, 0)
        .unwrap()
        .unwrap();
    let static_base = runtime
        .allocate_guest_block_for_module(0x40, 0)
        .unwrap()
        .unwrap();
    let scheduler = static_base.checked_add(0x20).unwrap();
    let node = runtime
        .allocate_guest_block_for_module(COMPACT_TIMER_NODE_LEN, 0)
        .unwrap()
        .unwrap();
    let second_node = runtime
        .allocate_guest_block_for_module(COMPACT_TIMER_NODE_LEN, 0)
        .unwrap()
        .unwrap();
    for (address, len) in [
        (parameter, MODULE_PARAMETER_LEN),
        (ext_chunk, EXT_CHUNK_TIMER_STATE_LEN),
        (static_base, 0x40),
        (node, COMPACT_TIMER_NODE_LEN),
        (second_node, COMPACT_TIMER_NODE_LEN),
    ] {
        runtime.memory.write(address, &vec![0; len]).unwrap();
    }
    runtime.memory.write_u32(static_base, 0xe12f_ff1e).unwrap();
    runtime
        .memory
        .add_permissions(static_base, 4, Permissions::EXECUTE)
        .unwrap();
    runtime.memory.write_u32(parameter, static_base.0).unwrap();
    runtime
        .memory
        .write_u32(
            parameter
                .checked_add(MODULE_PARAMETER_RW_LEN_OFFSET)
                .unwrap(),
            0x40,
        )
        .unwrap();
    runtime
        .memory
        .write_u32(parameter.checked_add(12).unwrap(), ext_chunk.0)
        .unwrap();
    runtime
        .memory
        .write_u32(ext_chunk, EXT_CHUNK_MAGIC)
        .unwrap();
    runtime
        .memory
        .write_u32(ext_chunk.checked_add(0x34).unwrap(), 0)
        .unwrap();
    runtime
        .memory
        .write_u32(scheduler.checked_add(8).unwrap(), node.0)
        .unwrap();
    runtime
        .memory
        .write_u32(scheduler.checked_add(12).unwrap(), 0)
        .unwrap();
    runtime.memory.write_u32(node, COMPACT_TIMER_MAGIC).unwrap();
    runtime
        .memory
        .write_u32(node.checked_add(COMPACT_TIMER_PERIOD_OFFSET).unwrap(), 100)
        .unwrap();
    runtime
        .memory
        .write_u32(
            node.checked_add(COMPACT_TIMER_HANDLER_OFFSET).unwrap(),
            static_base.0,
        )
        .unwrap();
    runtime
        .memory
        .write_u32(node.checked_add(8).unwrap(), 100)
        .unwrap();
    runtime
        .memory
        .write_u32(node.checked_add(COMPACT_TIMER_REPEAT_OFFSET).unwrap(), 1)
        .unwrap();
    runtime
        .memory
        .write_u32(scheduler.checked_add(12).unwrap(), second_node.0)
        .unwrap();
    runtime
        .memory
        .write_u32(second_node, COMPACT_TIMER_MAGIC)
        .unwrap();
    runtime
        .memory
        .write_u32(
            second_node
                .checked_add(COMPACT_TIMER_PERIOD_OFFSET)
                .unwrap(),
            250,
        )
        .unwrap();
    runtime
        .memory
        .write_u32(
            second_node
                .checked_add(COMPACT_TIMER_HANDLER_OFFSET)
                .unwrap(),
            static_base.0,
        )
        .unwrap();
    runtime
        .memory
        .write_u32(
            second_node
                .checked_add(COMPACT_TIMER_REPEAT_OFFSET)
                .unwrap(),
            1,
        )
        .unwrap();
    runtime.modules[0]
        .dynamic_executable_ranges
        .push(DynamicExecutableImageSlot(Some(DynamicExecutableImage {
            id: 7,
            intervals: vec![ExecutableRange {
                base: static_base,
                len: 4,
            }],
            module_parameter: Some(parameter),
            compact_repeating_timers: vec![node, second_node],
        })));

    let live_timers = runtime.current_repeating_timer_states().remove(0);
    let mut stale_timers = live_timers.clone();
    stale_timers.image_id += 1;
    runtime.modal_repeating_timers.push(stale_timers);
    assert_eq!(
        runtime.modal_timer_observations().unwrap()[0].timers,
        live_timers
    );
    assert!(runtime.modal_repeating_timers.is_empty());

    let entering = runtime.modal_timer_observations().unwrap();
    runtime
        .memory
        .write_u32(ext_chunk.checked_add(0x34).unwrap(), 1)
        .unwrap();
    runtime.finish_modal_timer_observations(entering).unwrap();

    // A foreground browser can reuse this executable arena. The saved timer
    // identity must remain authoritative while its live cache is unavailable.
    runtime.modules[0].dynamic_executable_ranges[0]
        .as_mut()
        .unwrap()
        .compact_repeating_timers = Vec::new();

    let leaving = runtime.modal_timer_observations().unwrap();
    runtime
        .memory
        .write_u32(node.checked_add(COMPACT_TIMER_PERIOD_OFFSET).unwrap(), 10)
        .unwrap();
    runtime
        .memory
        .write_u32(node.checked_add(8).unwrap(), 10)
        .unwrap();
    runtime
        .memory
        .write_u32(
            second_node
                .checked_add(COMPACT_TIMER_PERIOD_OFFSET)
                .unwrap(),
            25,
        )
        .unwrap();
    runtime
        .memory
        .write_u32(
            node.checked_add(COMPACT_TIMER_TAIL_OFFSET).unwrap(),
            second_node.0,
        )
        .unwrap();
    runtime
        .memory
        .write_u32(ext_chunk.checked_add(0x34).unwrap(), 0)
        .unwrap();
    runtime.finish_modal_timer_observations(leaving).unwrap();

    assert_eq!(
        runtime
            .memory
            .read_u32(
                second_node
                    .checked_add(COMPACT_TIMER_PERIOD_OFFSET)
                    .unwrap(),
            )
            .unwrap(),
        250
    );
    assert_eq!(
        runtime
            .memory
            .read_u32(node.checked_add(COMPACT_TIMER_PERIOD_OFFSET).unwrap())
            .unwrap(),
        100
    );
    assert_eq!(
        runtime
            .memory
            .read_u32(node.checked_add(8).unwrap())
            .unwrap(),
        10
    );
    assert_eq!(
        runtime
            .memory
            .read_u32(node.checked_add(COMPACT_TIMER_TAIL_OFFSET).unwrap())
            .unwrap(),
        second_node.0
    );
    assert!(runtime.modal_repeating_timers.is_empty());
    assert_eq!(
        runtime.modules[0].dynamic_executable_ranges[0]
            .as_ref()
            .unwrap()
            .compact_repeating_timers,
        vec![node, second_node]
    );

    runtime
        .memory
        .write_u32(node.checked_add(COMPACT_TIMER_PERIOD_OFFSET).unwrap(), 100)
        .unwrap();
    let entering = runtime.modal_timer_observations().unwrap();
    runtime
        .memory
        .write_u32(ext_chunk.checked_add(0x34).unwrap(), 1)
        .unwrap();
    runtime.finish_modal_timer_observations(entering).unwrap();
    let leaving = runtime.modal_timer_observations().unwrap();
    runtime
        .memory
        .write_u32(node.checked_add(COMPACT_TIMER_PERIOD_OFFSET).unwrap(), 10)
        .unwrap();
    runtime.memory.write_u32(node, 0).unwrap();
    runtime
        .memory
        .write_u32(ext_chunk.checked_add(0x34).unwrap(), 0)
        .unwrap();
    runtime.finish_modal_timer_observations(leaving).unwrap();
    assert_eq!(
        runtime
            .memory
            .read_u32(node.checked_add(COMPACT_TIMER_PERIOD_OFFSET).unwrap())
            .unwrap(),
        10
    );
    assert!(runtime.modal_repeating_timers.is_empty());

    runtime.memory.write_u32(node, COMPACT_TIMER_MAGIC).unwrap();
    runtime
        .memory
        .write_u32(node.checked_add(COMPACT_TIMER_PERIOD_OFFSET).unwrap(), 100)
        .unwrap();
    let entering = runtime.modal_timer_observations().unwrap();
    runtime
        .memory
        .write_u32(ext_chunk.checked_add(0x34).unwrap(), 1)
        .unwrap();
    runtime.finish_modal_timer_observations(entering).unwrap();
    let leaving = runtime.modal_timer_observations().unwrap();
    runtime
        .memory
        .write_u32(node.checked_add(COMPACT_TIMER_PERIOD_OFFSET).unwrap(), 10)
        .unwrap();
    runtime
        .memory
        .write_u32(node.checked_add(COMPACT_TIMER_DATA_OFFSET).unwrap(), 1)
        .unwrap();
    runtime
        .memory
        .write_u32(ext_chunk.checked_add(0x34).unwrap(), 0)
        .unwrap();
    runtime.finish_modal_timer_observations(leaving).unwrap();
    assert_eq!(
        runtime
            .memory
            .read_u32(node.checked_add(COMPACT_TIMER_PERIOD_OFFSET).unwrap())
            .unwrap(),
        10
    );
    assert!(runtime.modal_repeating_timers.is_empty());
}

#[test]
fn modal_screen_composition_preserves_exactly_the_pixels_presented_on_return() {
    let pixels = |values: &[u16]| {
        values
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect::<Vec<_>>()
    };
    let previous = PlatformScreenSnapshot {
        width: 3,
        height: 2,
        pixels: pixels(&[1, 2, 3, 4, 5, 6]),
    };
    let returned = PlatformScreenSnapshot {
        width: 3,
        height: 2,
        pixels: pixels(&[9, 8, 7, 6, 5, 4]),
    };
    let presented = PresentedScreenPixels {
        width: 3,
        height: 2,
        // Pixel 0 models a same-color write: a before/after diff could miss it,
        // while the platform presentation boundary records it authoritatively.
        dirty: vec![true, true, false, false, true, false],
        compatible: true,
    };

    let composed = ExtRuntime::compose_modal_return_screen(
        previous.clone(),
        returned.clone(),
        Some(&presented),
    );
    assert_eq!(composed.pixels, pixels(&[9, 8, 3, 4, 5, 6]));

    let rotated = PlatformScreenSnapshot {
        width: 2,
        height: 3,
        pixels: previous.pixels,
    };
    assert_eq!(
        ExtRuntime::compose_modal_return_screen(rotated, returned.clone(), Some(&presented)),
        returned
    );
}

#[test]
fn modal_screen_capture_uses_host_geometry_when_guest_reuses_dimension_slots() {
    let mut runtime =
        ExtRuntime::new(240, 320, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    runtime.memory.write_u32(data_slot_address(92), 19).unwrap();
    runtime.memory.write_u32(data_slot_address(93), 21).unwrap();
    let host_screen = vec![0x5a; 240 * 320 * 2];
    let _capture = capture_stub_framebuffer(host_screen.clone());

    let snapshot = runtime
        .capture_modal_screen_snapshot(&mut StubServices)
        .unwrap();

    assert_eq!((snapshot.width, snapshot.height), (240, 320));
    assert_eq!(snapshot.pixels, host_screen);
    assert_eq!(runtime.screen_dimensions().unwrap(), (19, 21));

    let restored_screen = vec![0xa5; 240 * 320 * 2];
    runtime
        .restore_modal_screen(
            PlatformScreenSnapshot {
                width: 240,
                height: 320,
                pixels: restored_screen.clone(),
            },
            &mut StubServices,
        )
        .unwrap();
    assert_eq!(
        STUB_FRAMEBUFFER.with(|framebuffer| framebuffer.borrow().clone()),
        Some(restored_screen.clone())
    );
    assert_eq!(
        runtime
            .memory
            .read(runtime.screen_base, restored_screen.len())
            .unwrap(),
        restored_screen
    );
    assert_eq!(runtime.screen_dimensions().unwrap(), (19, 21));
}

#[test]
fn modal_screen_keeps_return_draws_while_suspend_state_is_temporarily_unavailable() {
    let pixels = |values: &[u16]| {
        values
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect::<Vec<_>>()
    };
    let mut runtime =
        ExtRuntime::new(2, 2, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    load_test_module(&mut runtime);
    let owner_generation = runtime.modules[0].generation;
    let parameter = runtime
        .allocate_guest_block_for_module(MODULE_PARAMETER_LEN, 0)
        .unwrap()
        .unwrap();
    let ext_chunk = runtime
        .allocate_guest_block_for_module(EXT_CHUNK_TIMER_STATE_LEN, 0)
        .unwrap()
        .unwrap();
    runtime
        .memory
        .write(parameter, &[0; MODULE_PARAMETER_LEN])
        .unwrap();
    runtime
        .memory
        .write(ext_chunk, &[0; EXT_CHUNK_TIMER_STATE_LEN])
        .unwrap();
    runtime
        .memory
        .write_u32(
            parameter
                .checked_add(MODULE_PARAMETER_EXT_CHUNK_OFFSET)
                .unwrap(),
            ext_chunk.0,
        )
        .unwrap();
    runtime.modules[0]
        .dynamic_executable_ranges
        .push(DynamicExecutableImageSlot(Some(DynamicExecutableImage {
            id: 7,
            intervals: Vec::new(),
            module_parameter: Some(parameter),
            compact_repeating_timers: Vec::new(),
        })));
    let key = (owner_generation, 7, parameter);
    runtime.modal_screens.push(ModalScreenState {
        active: [key].into_iter().collect(),
        previous_screen: PlatformScreenSnapshot {
            width: 2,
            height: 2,
            pixels: pixels(&[1, 2, 3, 4]),
        },
        pending_return_screen: None,
    });

    // The image remains registered but its extChunk magic is temporarily hidden.
    // Preserve the one pixel actually presented by this uncertain return call.
    let returned = pixels(&[9, 8, 9, 9]);
    let _capture = capture_stub_framebuffer(returned);
    runtime
        .finish_modal_screen_transition(
            ModalTimerTransitions::default(),
            None,
            Some(PresentedScreenPixels {
                width: 2,
                height: 2,
                dirty: vec![false, true, false, false],
                compatible: true,
            }),
            &mut StubServices,
        )
        .unwrap();
    assert_eq!(runtime.modal_screens.len(), 1);
    assert_eq!(
        runtime.modal_screens[0]
            .pending_return_screen
            .as_ref()
            .unwrap()
            .pixels,
        pixels(&[1, 8, 3, 4])
    );

    runtime
        .memory
        .write_u32(ext_chunk, EXT_CHUNK_MAGIC)
        .unwrap();
    runtime
        .memory
        .write_u32(
            ext_chunk
                .checked_add(EXT_CHUNK_SUSPEND_DEPTH_OFFSET)
                .unwrap(),
            0,
        )
        .unwrap();
    runtime
        .refresh_modal_screen_states(&mut StubServices)
        .unwrap();
    assert!(runtime.modal_screens.is_empty());
    assert_eq!(
        runtime.memory.read(SCREEN_BASE, 2 * 2 * 2).unwrap(),
        pixels(&[1, 8, 3, 4])
    );
}

#[test]
fn gzip_guest_call_recovers_the_platform_screen_buffer_capacity_from_abi_data() {
    let mut runtime =
        ExtRuntime::new(13, 9, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    let mut module = b"MRPGCMAP".to_vec();
    module.extend_from_slice(&0xe12f_ff1e_u32.to_le_bytes()); // entry: bx lr
    module.extend_from_slice(&0xe92d_4000_u32.to_le_bytes()); // caller: push {lr}
    module.extend_from_slice(&0xe24d_d020_u32.to_le_bytes()); // sub sp, sp, #32
    module.extend_from_slice(&0xeb00_0001_u32.to_le_bytes()); // bl callee
    module.extend_from_slice(&0xe28d_d020_u32.to_le_bytes()); // add sp, sp, #32
    module.extend_from_slice(&0xe8bd_8000_u32.to_le_bytes()); // pop {pc}
    module.extend_from_slice(&0xe12f_ff1e_u32.to_le_bytes()); // callee: bx lr
    runtime
        .load_and_call_entry(&module, 0, &mut StubServices)
        .unwrap();

    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(&[0x5a; 173]).unwrap();
    let gzip = encoder.finish().unwrap();
    let source = runtime.allocate(gzip.len(), 4).unwrap();
    runtime.memory.write(source, &gzip).unwrap();

    // The active-frame holders deliberately are not adjacent, so recovery
    // relies on the ABI rather than a compiler's stack layout or instruction window.
    let output_pointer = STACK_BASE.checked_add(STACK_LEN as u32 - 36).unwrap();
    let output_len_pointer = STACK_BASE.checked_add(STACK_LEN as u32 - 20).unwrap();
    runtime
        .memory
        .write_u32(output_pointer, SCREEN_BASE.0)
        .unwrap();
    runtime.memory.write_u32(output_len_pointer, 1).unwrap();

    runtime
        .call_guest(
            GuestFunction {
                module: 0,
                address: MODULE_BASE + 12,
                expected_image: Some(ExecutableImage::Static),
                captured_r9: None,
            },
            [
                source.0,
                gzip.len() as u32,
                output_pointer.0,
                output_len_pointer.0,
            ],
            &[],
            &mut StubServices,
        )
        .unwrap();
    assert_eq!(
        runtime.memory.read_u32(output_len_pointer).unwrap(),
        SCREEN_STAGING_CAPACITY as u32
    );
}

#[test]
fn gzip_screen_capacity_recovery_rejects_unowned_and_oversized_outputs() {
    let mut runtime =
        ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    let output_pointer = STACK_BASE.checked_add(0x100).unwrap();
    let output_len_pointer = STACK_BASE.checked_add(0x120).unwrap();
    let mut cpu = ArmCpu::new();
    cpu.set_register(2, output_pointer.0);
    cpu.set_register(3, output_len_pointer.0);
    cpu.set_register(13, STACK_BASE.0);

    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder
        .write_all(&vec![0; SCREEN_STAGING_CAPACITY + 1])
        .unwrap();
    let oversized = encoder.finish().unwrap();
    let source = runtime.allocate(oversized.len(), 4).unwrap();
    runtime.memory.write(source, &oversized).unwrap();
    cpu.set_register(0, source.0);
    cpu.set_register(1, oversized.len() as u32);
    runtime
        .memory
        .write_u32(output_pointer, SCREEN_BASE.0)
        .unwrap();
    runtime.memory.write_u32(output_len_pointer, 1).unwrap();
    runtime
        .prepare_guest_gzip_screen_buffer_capacity(&cpu)
        .unwrap();
    assert_eq!(runtime.memory.read_u32(output_len_pointer).unwrap(), 1);

    runtime
        .memory
        .write_u32(output_len_pointer, u32::MAX)
        .unwrap();
    runtime
        .prepare_guest_gzip_screen_buffer_capacity(&cpu)
        .unwrap();
    assert_eq!(
        runtime.memory.read_u32(output_len_pointer).unwrap(),
        SCREEN_STAGING_CAPACITY as u32
    );

    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(&[0x5a; 64]).unwrap();
    let fitting = encoder.finish().unwrap();
    let fitting_source = runtime.allocate(fitting.len(), 4).unwrap();
    runtime.memory.write(fitting_source, &fitting).unwrap();
    cpu.set_register(0, fitting_source.0);
    cpu.set_register(1, fitting.len() as u32);

    let short_source = runtime.allocate(7, 4).unwrap();
    runtime
        .memory
        .write(short_source, &[0x1f, 0x8b, 0x08, 64, 0, 0, 0])
        .unwrap();
    runtime.memory.write_u32(output_len_pointer, 1).unwrap();
    cpu.set_register(0, short_source.0);
    cpu.set_register(1, 7);
    runtime
        .prepare_guest_gzip_screen_buffer_capacity(&cpu)
        .unwrap();
    assert_eq!(runtime.memory.read_u32(output_len_pointer).unwrap(), 1);
    cpu.set_register(0, fitting_source.0);
    cpu.set_register(1, fitting.len() as u32);

    let guest_buffer = runtime.allocate(8 * 8 * 2 + 1, 4).unwrap();
    runtime
        .memory
        .write_u32(output_pointer, guest_buffer.0)
        .unwrap();
    runtime.memory.write_u32(output_len_pointer, 1).unwrap();
    runtime
        .prepare_guest_gzip_screen_buffer_capacity(&cpu)
        .unwrap();
    assert_eq!(runtime.memory.read_u32(output_len_pointer).unwrap(), 1);

    runtime
        .memory
        .write_u32(output_pointer, SCREEN_BASE.0)
        .unwrap();
    runtime.memory.write_u32(output_len_pointer, 100).unwrap();
    runtime
        .prepare_guest_gzip_screen_buffer_capacity(&cpu)
        .unwrap();
    assert_eq!(runtime.memory.read_u32(output_len_pointer).unwrap(), 100);

    let valid_staging_capacity = 800 * 1024;
    runtime
        .memory
        .write_u32(output_len_pointer, valid_staging_capacity)
        .unwrap();
    runtime
        .prepare_guest_gzip_screen_buffer_capacity(&cpu)
        .unwrap();
    assert_eq!(
        runtime.memory.read_u32(output_len_pointer).unwrap(),
        valid_staging_capacity
    );

    runtime.memory.write_u32(output_len_pointer, 1).unwrap();
    runtime.memory.write(fitting_source, &[0]).unwrap();
    runtime
        .prepare_guest_gzip_screen_buffer_capacity(&cpu)
        .unwrap();
    assert_eq!(runtime.memory.read_u32(output_len_pointer).unwrap(), 1);

    runtime.memory.write(fitting_source, &[0x1f]).unwrap();
    runtime.memory.write(SCREEN_BASE, &fitting).unwrap();
    runtime.memory.write_u32(output_len_pointer, 1).unwrap();
    cpu.set_register(0, SCREEN_BASE.0);
    runtime
        .prepare_guest_gzip_screen_buffer_capacity(&cpu)
        .unwrap();
    assert_eq!(runtime.memory.read_u32(output_len_pointer).unwrap(), 1);

    let non_stack_holders = runtime.allocate(8, 4).unwrap();
    let non_stack_len = non_stack_holders.checked_add(4).unwrap();
    runtime
        .memory
        .write_u32(non_stack_holders, SCREEN_BASE.0)
        .unwrap();
    runtime.memory.write_u32(non_stack_len, 1).unwrap();
    cpu.set_register(0, fitting_source.0);
    cpu.set_register(2, non_stack_holders.0);
    cpu.set_register(3, non_stack_len.0);
    runtime
        .prepare_guest_gzip_screen_buffer_capacity(&cpu)
        .unwrap();
    assert_eq!(runtime.memory.read_u32(non_stack_len).unwrap(), 1);
}

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
fn ext_runtime_keeps_null_memory_unmapped_for_host_accesses() {
    let mut runtime =
        ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();

    assert!(runtime.memory.read_u16(GuestAddr(0)).is_err());
    assert!(runtime.memory.write_u16(GuestAddr(0), 1).is_err());
    assert!(runtime.memory.fetch_u16(GuestAddr(0)).is_err());
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
fn guest_allocator_returns_none_when_the_free_counter_cannot_cover_a_block() {
    let mut runtime =
        ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    let heap = runtime.guest_heap_state().unwrap();
    runtime
        .write_free_blocks(
            heap,
            &[FreeBlock {
                offset: 0,
                len: heap.span,
            }],
            heap.span,
            8,
        )
        .unwrap();

    assert_eq!(runtime.allocate_guest_block(16).unwrap(), None);

    let heap = runtime.guest_heap_state().unwrap();
    let (blocks, terminator, recovered_len) = runtime.read_free_blocks(heap).unwrap();
    assert_eq!(
        blocks,
        [FreeBlock {
            offset: 0,
            len: heap.span,
        }]
    );
    assert_eq!(terminator, heap.span);
    assert_eq!(recovered_len, 0);
    assert_eq!(heap.free_left, 8);
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
fn guest_allocator_recovers_a_legacy_header_backed_payload_free() {
    let mut runtime =
        ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    let span = DEFAULT_HEAP_LEN as u32;
    let allocation_len = 0x38;
    let successor_offset = allocation_len + 0x10;
    let heap = runtime.guest_heap_state().unwrap();
    runtime
        .write_free_blocks(
            heap,
            &[
                FreeBlock {
                    offset: 0,
                    len: allocation_len + 8,
                },
                FreeBlock {
                    offset: successor_offset,
                    len: span - successor_offset,
                },
            ],
            span,
            span - 8,
        )
        .unwrap();

    let backing = runtime.allocate_guest_block(0x36).unwrap().unwrap();
    assert_eq!(backing, HEAP_BASE);
    let snapshot_free_left = runtime.guest_heap_state().unwrap().free_left;
    let old_head = allocation_len;
    let payload_offset = 4;

    // The guest wrapper returns backing+4 and later links that payload into its
    // private free-list using the complete aligned backing length.
    runtime
        .memory
        .write_u32(HEAP_BASE.checked_add(old_head).unwrap(), payload_offset)
        .unwrap();
    runtime
        .memory
        .write_u32(
            HEAP_BASE.checked_add(payload_offset).unwrap(),
            successor_offset,
        )
        .unwrap();
    runtime
        .memory
        .write_u32(
            HEAP_BASE.checked_add(payload_offset + 4).unwrap(),
            allocation_len,
        )
        .unwrap();
    runtime
        .memory
        .write_u32(
            data_slot_address(111),
            snapshot_free_left + allocation_len * 2,
        )
        .unwrap();

    let small = runtime.allocate_guest_block(1).unwrap().unwrap();

    assert_eq!(small, HEAP_BASE.checked_add(old_head).unwrap());
    let heap = runtime.guest_heap_state().unwrap();
    let (blocks, terminator, recovered_len) = runtime.read_free_blocks(heap).unwrap();
    assert_eq!(
        blocks,
        [
            FreeBlock {
                offset: payload_offset,
                len: allocation_len - 4,
            },
            FreeBlock {
                offset: successor_offset,
                len: span - successor_offset,
            },
        ]
    );
    assert_eq!(terminator, span);
    assert_eq!(recovered_len, 0);
    assert_eq!(heap.free_left, snapshot_free_left + allocation_len - 4 - 8);
    assert!(runtime.guest_allocations.contains_key(&backing.0));

    runtime.free_guest_block(backing, 0x36).unwrap();

    let heap = runtime.guest_heap_state().unwrap();
    let (blocks, terminator, recovered_len) = runtime.read_free_blocks(heap).unwrap();
    assert_eq!(
        blocks,
        [
            FreeBlock {
                offset: 0,
                len: allocation_len,
            },
            FreeBlock {
                offset: successor_offset,
                len: span - successor_offset,
            },
        ]
    );
    assert_eq!(terminator, span);
    assert_eq!(recovered_len, 0);
    assert_eq!(heap.free_left, snapshot_free_left + allocation_len - 8);
    assert!(!runtime.guest_allocations.contains_key(&backing.0));
}

#[test]
fn guest_allocator_preserves_non_overlapping_guest_counter_staging() {
    let mut runtime =
        ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    let backing_len = 0x38;
    let backing = runtime
        .allocate_guest_block(backing_len as usize)
        .unwrap()
        .unwrap();
    let _first_guard = runtime.allocate_guest_block(0x10).unwrap().unwrap();
    let tiny = runtime.allocate_guest_block(8).unwrap().unwrap();
    let _second_guard = runtime.allocate_guest_block(0x10).unwrap().unwrap();
    runtime.free_guest_block(tiny, 8).unwrap();

    let heap = runtime.guest_heap_state().unwrap();
    let snapshot_free_left = heap.free_left;
    let payload_offset = backing.0 - heap.base + 4;
    let tiny_offset = tiny.0 - heap.base;
    let tail_offset = tiny_offset + 0x18;
    runtime.memory.write_u32(tiny, payload_offset).unwrap();
    runtime
        .memory
        .write_u32(GuestAddr(heap.base + payload_offset), tail_offset)
        .unwrap();
    runtime
        .memory
        .write_u32(GuestAddr(heap.base + payload_offset + 4), backing_len)
        .unwrap();
    runtime
        .memory
        .write_u32(data_slot_address(111), snapshot_free_left + backing_len * 2)
        .unwrap();

    let small = runtime.allocate_guest_block(1).unwrap().unwrap();

    assert_eq!(small, tiny);
    let heap = runtime.guest_heap_state().unwrap();
    let (blocks, terminator, recovered_len) = runtime.read_free_blocks(heap).unwrap();
    assert_eq!(
        blocks,
        [
            FreeBlock {
                offset: payload_offset,
                len: backing_len,
            },
            FreeBlock {
                offset: tail_offset,
                len: heap.span - tail_offset,
            },
        ]
    );
    assert_eq!(terminator, heap.span);
    assert_eq!(recovered_len, 0);
    assert_eq!(heap.free_left, snapshot_free_left + backing_len * 2 - 8);
}

#[test]
fn guest_allocator_preserves_an_untracked_doubled_free_counter() {
    let mut runtime =
        ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    let heap = runtime.guest_heap_state().unwrap();
    let added_offset = 0x200;
    let added_len = 0x80;
    let tiny_offset = 0x1000;
    let tail_offset = 0x2000;
    let tail_len = heap.span - tail_offset;
    runtime
        .write_free_blocks(
            heap,
            &[
                FreeBlock {
                    offset: tiny_offset,
                    len: 8,
                },
                FreeBlock {
                    offset: tail_offset,
                    len: tail_len,
                },
            ],
            heap.span,
            8 + tail_len,
        )
        .unwrap();
    let snapshot_free_left = runtime.guest_heap_state().unwrap().free_left;
    runtime
        .memory
        .write_u32(GuestAddr(heap.base + tiny_offset), added_offset)
        .unwrap();
    runtime
        .memory
        .write_u32(GuestAddr(heap.base + added_offset), tail_offset)
        .unwrap();
    runtime
        .memory
        .write_u32(GuestAddr(heap.base + added_offset + 4), added_len)
        .unwrap();
    runtime
        .memory
        .write_u32(data_slot_address(111), snapshot_free_left + added_len * 2)
        .unwrap();

    let small = runtime.allocate_guest_block(1).unwrap().unwrap();

    assert_eq!(small, GuestAddr(heap.base + tiny_offset));
    let heap = runtime.guest_heap_state().unwrap();
    let (blocks, terminator, recovered_len) = runtime.read_free_blocks(heap).unwrap();
    assert_eq!(
        blocks,
        [
            FreeBlock {
                offset: added_offset,
                len: added_len,
            },
            FreeBlock {
                offset: tail_offset,
                len: tail_len,
            },
        ]
    );
    assert_eq!(terminator, heap.span);
    assert_eq!(recovered_len, 0);
    assert_eq!(heap.free_left, snapshot_free_left + added_len * 2 - 8);
}

#[test]
fn freeing_a_guest_allocation_recovers_a_wrapped_double_decrement_counter() {
    let mut runtime = ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", 0x10_0000).unwrap();
    load_test_module(&mut runtime);
    let prefix_request_len = 0x9cfc0;
    let request_len = 0x36966;
    let block_len = heap::aligned_heap_len(request_len).unwrap();
    let _prefix = runtime
        .allocate_guest_block(prefix_request_len)
        .unwrap()
        .unwrap();
    let allocation = runtime
        .allocate_guest_block_for_module(request_len, 0)
        .unwrap()
        .unwrap();
    let heap = runtime.guest_heap_state().unwrap();
    let allocation_offset = allocation.0 - heap.base;
    let tail_len = heap.span - allocation_offset - block_len;
    assert_eq!(heap.free_left, tail_len);

    runtime
        .memory
        .write_u32(data_slot_address(111), tail_len.wrapping_sub(block_len))
        .unwrap();

    runtime
        .free_guest_block_for_module(allocation, request_len, 0)
        .unwrap();

    let heap = runtime.guest_heap_state().unwrap();
    let (blocks, terminator, recovered_len) = runtime.read_free_blocks(heap).unwrap();
    assert_eq!(
        blocks,
        [FreeBlock {
            offset: allocation_offset,
            len: heap.span - allocation_offset,
        }]
    );
    assert_eq!(terminator, heap.span);
    assert_eq!(recovered_len, 0);
    assert_eq!(heap.free_left, heap.span - allocation_offset);
    assert!(!runtime.guest_allocations.contains_key(&allocation.0));
}

#[test]
fn freeing_a_guest_allocation_rejects_an_unrelated_wrapped_counter() {
    let mut runtime = ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", 0x10_0000).unwrap();
    load_test_module(&mut runtime);
    let prefix_request_len = 0x9cfc0;
    let request_len = 0x36966;
    let block_len = heap::aligned_heap_len(request_len).unwrap();
    let _prefix = runtime
        .allocate_guest_block(prefix_request_len)
        .unwrap()
        .unwrap();
    let allocation = runtime
        .allocate_guest_block_for_module(request_len, 0)
        .unwrap()
        .unwrap();
    let heap = runtime.guest_heap_state().unwrap();
    let allocation_offset = allocation.0 - heap.base;
    let tail_len = heap.span - allocation_offset - block_len;
    runtime
        .memory
        .write_u32(
            data_slot_address(111),
            tail_len.wrapping_sub(block_len).wrapping_add(8),
        )
        .unwrap();

    assert!(matches!(
        runtime.free_guest_block_for_module(allocation, request_len, 0),
        Err(Error::Abi(message)) if message == "guest free-byte count overflow"
    ));
    assert!(runtime.guest_allocations.contains_key(&allocation.0));
}

#[test]
fn guest_allocator_recovers_the_tail_after_a_legacy_payload_is_zeroed() {
    assert_guest_allocator_recovers_tail_after_legacy_payload_withdrawal([0; 8]);
}

#[test]
fn guest_allocator_recovers_the_tail_after_a_legacy_payload_is_poisoned() {
    assert_guest_allocator_recovers_tail_after_legacy_payload_withdrawal([
        0xfd, 0xa5, 0xfd, 0xa5, 0xfd, 0xa5, 0xfd, 0xa5,
    ]);
}

fn assert_guest_allocator_recovers_tail_after_legacy_payload_withdrawal(header: [u8; 8]) {
    let mut runtime =
        ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    let span = DEFAULT_HEAP_LEN as u32;
    let allocation_len = 0x38;
    let payload_offset = 4;
    let tiny_offset = allocation_len;
    let tail_offset = allocation_len + 0x10;
    let heap = runtime.guest_heap_state().unwrap();
    runtime
        .write_free_blocks(
            heap,
            &[
                FreeBlock {
                    offset: 0,
                    len: allocation_len + 8,
                },
                FreeBlock {
                    offset: tail_offset,
                    len: span - tail_offset,
                },
            ],
            span,
            span - 8,
        )
        .unwrap();

    let backing = runtime.allocate_guest_block(0x36).unwrap().unwrap();
    let snapshot_free_left = runtime.guest_heap_state().unwrap().free_left;
    runtime
        .memory
        .write_u32(GuestAddr(heap.base + tiny_offset), payload_offset)
        .unwrap();
    runtime
        .memory
        .write_u32(GuestAddr(heap.base + payload_offset), tail_offset)
        .unwrap();
    runtime
        .memory
        .write_u32(GuestAddr(heap.base + payload_offset + 4), allocation_len)
        .unwrap();
    runtime
        .memory
        .write_u32(
            data_slot_address(111),
            snapshot_free_left + allocation_len * 2,
        )
        .unwrap();

    let small = runtime.allocate_guest_block(1).unwrap().unwrap();
    let heap = runtime.guest_heap_state().unwrap();
    let reconciled_free_left = heap.free_left;
    runtime.guest_allocations.remove(&backing.0);
    runtime
        .memory
        .write(GuestAddr(heap.base + payload_offset), &header)
        .unwrap();
    runtime
        .memory
        .write_u32(
            data_slot_address(111),
            reconciled_free_left - (allocation_len - 4),
        )
        .unwrap();

    runtime.free_guest_block(small, 8).unwrap();

    let heap = runtime.guest_heap_state().unwrap();
    let (blocks, terminator, recovered_len) = runtime.read_free_blocks(heap).unwrap();
    assert_eq!(
        blocks,
        [
            FreeBlock {
                offset: tiny_offset,
                len: 8,
            },
            FreeBlock {
                offset: tail_offset,
                len: span - tail_offset,
            },
        ]
    );
    assert_eq!(terminator, span);
    assert_eq!(recovered_len, 0);
    assert_eq!(
        heap.free_left,
        reconciled_free_left - (allocation_len - 4) + 8
    );
    assert!(!runtime.guest_allocations.contains_key(&backing.0));
}

#[test]
fn freeing_a_legacy_wrapper_restores_its_truncated_tail_across_repeated_reuse() {
    let mut runtime =
        ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    load_test_module(&mut runtime);
    let payload_len = 0x36_u32;
    let wrapper_len = payload_len + 4;
    let block_len = heap::aligned_heap_len(wrapper_len as usize).unwrap();
    let initial_free_left = runtime.guest_heap_state().unwrap().free_left;
    let mut backing = runtime
        .allocate_guest_block_for_module(wrapper_len as usize, 0)
        .unwrap()
        .unwrap();
    for _ in 0..4 {
        runtime.memory.write_u32(backing, payload_len).unwrap();

        let heap = runtime.guest_heap_state().unwrap();
        let backing_offset = backing.0 - heap.base;
        let successor_offset = backing_offset + block_len;
        runtime
            .memory
            .write_u32(
                GuestAddr(heap.base).checked_add(successor_offset).unwrap(),
                heap.span,
            )
            .unwrap();
        runtime
            .memory
            .write_u32(
                GuestAddr(heap.base)
                    .checked_add(successor_offset + 4)
                    .unwrap(),
                16,
            )
            .unwrap();
        runtime
            .memory
            .write_u32(data_slot_address(111), heap.free_left - 4)
            .unwrap();

        runtime
            .free_guest_block_for_module(backing, wrapper_len as usize, 0)
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
        assert_eq!(heap.free_left, initial_free_left);

        let reused = runtime
            .allocate_guest_block_for_module(wrapper_len as usize, 0)
            .unwrap()
            .unwrap();
        assert_eq!(reused, backing);
        backing = reused;
    }
}

#[test]
fn freeing_a_legacy_wrapper_does_not_restore_an_accounted_tail_allocation() {
    let mut runtime =
        ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    load_test_module(&mut runtime);
    let payload_len = 0x36_u32;
    let wrapper_len = payload_len + 4;
    let block_len = heap::aligned_heap_len(wrapper_len as usize).unwrap();
    let backing = runtime
        .allocate_guest_block_for_module(wrapper_len as usize, 0)
        .unwrap()
        .unwrap();
    runtime.memory.write_u32(backing, payload_len).unwrap();

    let heap = runtime.guest_heap_state().unwrap();
    let backing_offset = backing.0 - heap.base;
    let successor_offset = backing_offset + block_len;
    runtime
        .memory
        .write_u32(
            GuestAddr(heap.base).checked_add(successor_offset).unwrap(),
            heap.span,
        )
        .unwrap();
    runtime
        .memory
        .write_u32(
            GuestAddr(heap.base)
                .checked_add(successor_offset + 4)
                .unwrap(),
            16,
        )
        .unwrap();
    runtime
        .memory
        .write_u32(data_slot_address(111), 16)
        .unwrap();

    runtime
        .free_guest_block_for_module(backing, wrapper_len as usize, 0)
        .unwrap();

    let heap = runtime.guest_heap_state().unwrap();
    let (blocks, _, _) = runtime.read_free_blocks(heap).unwrap();
    assert_eq!(
        blocks,
        [FreeBlock {
            offset: backing_offset,
            len: block_len + 16,
        }]
    );
    assert_eq!(heap.free_left, block_len + 16);
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

    runtime.memory.write_u32(output, 0xaaaa_aaaa).unwrap();
    runtime.memory.write_u32(output_len, 0xbbbb_bbbb).unwrap();
    cpu.set_register(0, 1_223);
    cpu.set_register(1, 0);
    cpu.set_register(2, 0);
    cpu.set_register(3, output.0);
    runtime
        .dispatch(38, 0, &mut cpu, &mut StubServices)
        .unwrap();
    assert_eq!(cpu.register(0) as i32, -1);
    assert_eq!(runtime.memory.read_u32(output).unwrap(), 0);
    assert_eq!(runtime.memory.read_u32(output_len).unwrap(), 0);

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
fn completed_menu_selection_detaches_the_menu_and_restores_guest_input() {
    let mut runtime =
        ExtRuntime::new(240, 320, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    runtime.memory.write_u16(SCREEN_BASE, 0x1234).unwrap();
    let handle = runtime
        .create_platform_menu(vec![0x83dc, 0x5355], 1)
        .unwrap();
    runtime.menus.get_mut(&handle).unwrap().items = vec![Some(vec![0x5f00, 0x59cb])];
    let mut services = StubServices;
    runtime.show_platform_menu(handle, &mut services).unwrap();

    assert_eq!(
        runtime.route_key_event(20, true, &mut services).unwrap(),
        Some((4, 0, 0))
    );
    assert_eq!(runtime.pending_platform_menu_selection, Some(handle));

    runtime.finish_platform_event(&mut services).unwrap();

    assert!(runtime.active_platform_ui.is_empty());
    assert_eq!(runtime.pending_platform_menu_selection, None);
    assert!(runtime.menus[&handle].modal_detached);
    assert_eq!(runtime.memory.read_u16(SCREEN_BASE).unwrap(), 0x1234);
    assert_eq!(
        runtime
            .route_pointer_event(105, 111, true, &mut services)
            .unwrap(),
        Some((2, 105, 111))
    );
    assert_eq!(
        runtime
            .route_pointer_event(105, 111, false, &mut services)
            .unwrap(),
        Some((3, 105, 111))
    );

    assert!(
        runtime
            .refresh_platform_menu(handle, &mut services)
            .unwrap()
    );
    assert_eq!(runtime.active_platform_ui, [ActivePlatformUi::Menu(handle)]);
    assert!(!runtime.menus[&handle].modal_detached);
}

#[test]
fn completed_menu_selection_preserves_a_screen_drawn_by_the_guest_callback() {
    let mut runtime =
        ExtRuntime::new(240, 320, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    runtime.memory.write_u16(SCREEN_BASE, 0x1234).unwrap();
    let handle = runtime.create_platform_menu(Vec::new(), 1).unwrap();
    runtime.menus.get_mut(&handle).unwrap().items = vec![Some(vec![0x5f00, 0x59cb])];
    let mut services = StubServices;
    runtime.show_platform_menu(handle, &mut services).unwrap();
    assert_eq!(
        runtime.route_key_event(20, true, &mut services).unwrap(),
        Some((4, 0, 0))
    );

    runtime.memory.write_u16(SCREEN_BASE, 0xabcd).unwrap();
    runtime.finish_platform_event(&mut services).unwrap();

    assert!(runtime.active_platform_ui.is_empty());
    assert_eq!(runtime.memory.read_u16(SCREEN_BASE).unwrap(), 0xabcd);
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
fn refreshing_a_modal_detached_menu_reattaches_it_above_its_parent() {
    let mut runtime =
        ExtRuntime::new(240, 320, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    let parent = runtime
        .create_platform_menu(vec![0x7236, 0x83dc], 1)
        .unwrap();
    runtime.menus.get_mut(&parent).unwrap().items = vec![Some(vec![0x5b50, 0x83dc])];
    let child = runtime
        .create_platform_menu(vec![0x5b50, 0x83dc], 2)
        .unwrap();
    runtime.menus.get_mut(&child).unwrap().items =
        vec![Some(vec![0x7b2c, 0x4e00]), Some(vec![0x7b2c, 0x4e8c])];
    let mut services = StubServices;
    runtime.show_platform_menu(parent, &mut services).unwrap();
    runtime.show_platform_menu(child, &mut services).unwrap();
    let child_screen = runtime.menus[&child].menu_screen.clone().unwrap();
    assert_eq!(
        runtime.route_key_event(20, true, &mut services).unwrap(),
        Some((4, 0, 0))
    );

    let dialog = runtime
        .create_platform_dialog(&[], &[], 0, &mut services)
        .unwrap();
    runtime
        .release_platform_dialog(dialog, &mut services)
        .unwrap();
    assert_eq!(runtime.active_platform_ui, [ActivePlatformUi::Menu(parent)]);
    assert!(runtime.menus[&child].modal_detached);

    assert!(runtime.refresh_platform_menu(child, &mut services).unwrap());
    assert_eq!(
        runtime.active_platform_ui,
        [
            ActivePlatformUi::Menu(parent),
            ActivePlatformUi::Menu(child)
        ]
    );
    assert!(!runtime.menus[&child].modal_detached);
    assert_eq!(runtime.pending_platform_menu_selection, None);
    assert_eq!(
        runtime
            .memory
            .read(SCREEN_BASE, child_screen.len())
            .unwrap(),
        child_screen
    );

    assert!(runtime.release_platform_menu(child, &mut services).unwrap());
    assert_eq!(runtime.active_platform_ui, [ActivePlatformUi::Menu(parent)]);
    assert_eq!(runtime.pending_platform_menu_returns, 0);
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
        runtime.route_key_event(17, true, &mut services).unwrap(),
        None
    );
    assert_eq!(
        runtime.route_key_event(17, false, &mut services).unwrap(),
        None
    );
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
fn platform_text_viewer_accepts_verified_styles_and_rejects_unknown_styles() {
    let mut runtime =
        ExtRuntime::new(240, 320, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    let mut services = StubServices;

    for style in [1, 2] {
        let handle = runtime
            .create_platform_text_viewer(&[], &[], style, &mut services)
            .unwrap();
        assert_eq!(
            runtime.active_platform_ui,
            [ActivePlatformUi::TextViewer(handle)]
        );
        assert!(
            runtime
                .release_platform_text_viewer(handle, &mut services)
                .unwrap()
        );
    }

    assert!(matches!(
        runtime.create_platform_text_viewer(&[], &[], 3, &mut services),
        Err(Error::Abi(message)) if message == "unsupported platform text-viewer style 3"
    ));
    assert!(runtime.text_viewers.is_empty());
    assert!(runtime.active_platform_ui.is_empty());
}

#[test]
fn platform_text_viewer_style_one_routes_confirm_and_return_actions() {
    let mut runtime =
        ExtRuntime::new(240, 320, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    let mut services = StubServices;
    let handle = runtime
        .create_platform_text_viewer(&[], &[], 1, &mut services)
        .unwrap();

    for code in [17, 20] {
        assert_eq!(
            runtime.route_key_event(code, true, &mut services).unwrap(),
            Some((6, 0, 0))
        );
        assert_eq!(
            runtime.route_key_event(code, false, &mut services).unwrap(),
            None
        );
    }

    runtime
        .route_pointer_event(20, 306, true, &mut services)
        .unwrap();
    assert_eq!(
        runtime
            .route_pointer_event(20, 306, false, &mut services)
            .unwrap(),
        Some((6, 0, 0))
    );
    runtime
        .route_pointer_event(220, 306, true, &mut services)
        .unwrap();
    assert_eq!(
        runtime
            .route_pointer_event(220, 306, false, &mut services)
            .unwrap(),
        Some((6, 1, 0))
    );
    assert_eq!(
        runtime.active_platform_ui,
        [ActivePlatformUi::TextViewer(handle)]
    );
}

#[test]
fn platform_text_viewer_scrolls_long_text_without_overwriting_the_softkey_bar() {
    let mut runtime =
        ExtRuntime::new(240, 320, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    runtime.memory.write_u16(SCREEN_BASE, 0x1234).unwrap();
    let mut text = Vec::new();
    for line in 1..=14 {
        text.extend(std::iter::repeat_n(0x2603, line));
        if line != 14 {
            text.push(b'\n' as u16);
        }
    }
    let mut services = StubServices;

    let handle = runtime
        .create_platform_text_viewer(&[b'T' as u16], &text, 2, &mut services)
        .unwrap();

    assert_eq!(runtime.text_viewers[&handle].lines.len(), 14);
    assert_eq!(runtime.text_viewers[&handle].first_visible_line, 0);
    assert_eq!(
        runtime
            .memory
            .read_u16(runtime.screen_address(15, 274, 240).unwrap())
            .unwrap(),
        Framebuffer::rgb565(0, 252, 0)
    );
    assert_eq!(
        runtime
            .memory
            .read_u16(runtime.screen_address(15, 296, 240).unwrap())
            .unwrap(),
        Framebuffer::rgb565(0, 0, 0)
    );
    assert_eq!(
        runtime
            .memory
            .read_u16(runtime.screen_address(234, 32, 240).unwrap())
            .unwrap(),
        Framebuffer::rgb565(0, 252, 0)
    );
    let first_screen = runtime.text_viewers[&handle].viewer_screen.clone();

    assert!(
        runtime
            .move_platform_text_viewer(handle, 1, &mut services)
            .unwrap()
    );
    assert_eq!(runtime.text_viewers[&handle].first_visible_line, 1);
    assert_ne!(runtime.text_viewers[&handle].viewer_screen, first_screen);
    assert_eq!(
        runtime
            .memory
            .read_u16(runtime.screen_address(234, 32, 240).unwrap())
            .unwrap(),
        Framebuffer::rgb565(0, 0, 0)
    );
    assert!(
        runtime
            .move_platform_text_viewer(handle, 1, &mut services)
            .unwrap()
    );
    assert_eq!(runtime.text_viewers[&handle].first_visible_line, 2);
    assert!(
        !runtime
            .move_platform_text_viewer(handle, 1, &mut services)
            .unwrap()
    );

    assert!(
        runtime
            .move_platform_text_viewer(handle, -1, &mut services)
            .unwrap()
    );
    assert!(
        runtime
            .move_platform_text_viewer(handle, -1, &mut services)
            .unwrap()
    );
    assert_eq!(runtime.text_viewers[&handle].first_visible_line, 0);
    assert_eq!(runtime.text_viewers[&handle].viewer_screen, first_screen);
    assert!(
        !runtime
            .move_platform_text_viewer(handle, -1, &mut services)
            .unwrap()
    );
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
fn compact_ram_package_writes_into_a_legacy_prefix_length_wrapper() {
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
        .allocate_guest_block_for_module(expected.len() + 4, 0)
        .unwrap()
        .unwrap();
    let prepared = backing.checked_add(4).unwrap();
    runtime
        .memory
        .write_u32(backing, expected.len() as u32)
        .unwrap();

    let package = runtime.allocate(image.len(), 8).unwrap();
    runtime.memory.write(package, &image).unwrap();
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
    let heap = runtime.guest_heap_state().unwrap();
    let (mut blocks, terminator, recovered_len) = runtime.read_free_blocks(heap).unwrap();
    assert_eq!(recovered_len, 0);
    blocks.insert(
        0,
        FreeBlock {
            offset: prepared.0 - heap.base,
            len: expected.len() as u32,
        },
    );
    runtime
        .write_free_blocks(
            heap,
            &blocks,
            terminator,
            heap.free_left + expected.len() as u32,
        )
        .unwrap();
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
    let heap = runtime.guest_heap_state().unwrap();
    let (blocks, _, recovered_len) = runtime.read_free_blocks(heap).unwrap();
    assert_eq!(recovered_len, 0);
    assert!(
        blocks
            .iter()
            .all(|block| block.offset != prepared.0 - heap.base)
    );
    assert!(runtime.guest_allocations.contains_key(&backing.0));
    assert!(!runtime.guest_allocation_views.contains_key(&prepared.0));

    runtime
        .free_guest_block_for_module(backing, expected.len() + 4, 0)
        .unwrap();
    assert!(!runtime.guest_allocations.contains_key(&backing.0));
}

#[test]
fn compact_ram_package_prefers_an_explicit_descriptor_over_a_legacy_wrapper() {
    let mut runtime =
        ExtRuntime::new(240, 320, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    load_test_module(&mut runtime);
    let output_len = 0x30_u32;
    let stale_wrapper = runtime
        .allocate_guest_block_for_module((output_len + 4) as usize, 0)
        .unwrap()
        .unwrap();
    runtime.memory.write_u32(stale_wrapper, output_len).unwrap();

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

    assert_eq!(
        runtime
            .compact_ram_output_target(
                package,
                compact_header.len(),
                output_len as usize,
                0,
                GuestAddr(0),
            )
            .unwrap(),
        Some(prepared)
    );
}

#[test]
fn compact_ram_package_uses_the_current_length_pointer_to_disambiguate_descriptors() {
    let mut runtime =
        ExtRuntime::new(240, 320, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    load_test_module(&mut runtime);
    let output_len = 0x30_u32;

    let stale_output = runtime
        .allocate_guest_block_for_module(output_len as usize, 0)
        .unwrap()
        .unwrap();
    runtime
        .memory
        .write_u32(stale_output.checked_add(4).unwrap(), output_len)
        .unwrap();
    let stale_descriptor = runtime.allocate(8, 4).unwrap();
    runtime
        .memory
        .write_u32(stale_descriptor, stale_output.0)
        .unwrap();
    runtime
        .memory
        .write_u32(stale_descriptor.checked_add(4).unwrap(), output_len)
        .unwrap();

    let current_output = runtime
        .allocate_guest_block_for_module(output_len as usize, 0)
        .unwrap()
        .unwrap();
    runtime
        .memory
        .write_u32(current_output.checked_add(4).unwrap(), output_len)
        .unwrap();
    let current_descriptor = runtime.allocate(8, 4).unwrap();
    runtime
        .memory
        .write_u32(current_descriptor, current_output.0)
        .unwrap();
    let current_length_pointer = current_descriptor.checked_add(4).unwrap();
    runtime
        .memory
        .write_u32(current_length_pointer, output_len)
        .unwrap();

    let package = runtime.allocate(24, 8).unwrap();
    let mut compact_header = [0_u8; 24];
    compact_header[..4].copy_from_slice(b"MRPG");
    compact_header[4..8].copy_from_slice(&4_u32.to_le_bytes());
    compact_header[12..16].copy_from_slice(&4_u32.to_le_bytes());
    runtime.memory.write(package, &compact_header).unwrap();

    assert!(matches!(
        runtime.compact_ram_output_target(
            package,
            compact_header.len(),
            output_len as usize,
            0,
            GuestAddr(0),
        ),
        Err(Error::Abi(message)) if message.contains("ambiguous prepared buffers")
    ));
    assert_eq!(
        runtime
            .compact_ram_output_target(
                package,
                compact_header.len(),
                output_len as usize,
                0,
                current_length_pointer,
            )
            .unwrap(),
        Some(current_output)
    );
}

#[test]
fn compact_ram_package_only_uses_the_current_argument_record_pointer_slot() {
    let mut runtime =
        ExtRuntime::new(240, 320, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    load_test_module(&mut runtime);
    let output_len = 0x30_u32;

    let stale_output = runtime
        .allocate_guest_block_for_module(output_len as usize, 0)
        .unwrap()
        .unwrap();
    runtime
        .memory
        .write_u32(stale_output.checked_add(4).unwrap(), output_len)
        .unwrap();
    let stale_descriptor = runtime.allocate(8, 4).unwrap();
    runtime
        .memory
        .write_u32(stale_descriptor, stale_output.0)
        .unwrap();
    runtime
        .memory
        .write_u32(stale_descriptor.checked_add(4).unwrap(), output_len)
        .unwrap();

    let current_output = runtime
        .allocate_guest_block_for_module(output_len as usize, 0)
        .unwrap()
        .unwrap();
    runtime
        .memory
        .write_u32(current_output.checked_add(4).unwrap(), output_len)
        .unwrap();
    let current_descriptor = runtime.allocate(8, 4).unwrap();
    runtime
        .memory
        .write_u32(current_descriptor, current_output.0)
        .unwrap();
    runtime
        .memory
        .write_u32(current_descriptor.checked_add(4).unwrap(), output_len)
        .unwrap();

    runtime.allocate(512, 4).unwrap();
    let output_len_pointer = runtime.allocate(44, 4).unwrap();
    runtime
        .memory
        .write_u32(output_len_pointer, output_len)
        .unwrap();

    let package = runtime.allocate(24, 8).unwrap();
    let mut compact_header = [0_u8; 24];
    compact_header[..4].copy_from_slice(b"MRPG");
    compact_header[4..8].copy_from_slice(&4_u32.to_le_bytes());
    compact_header[12..16].copy_from_slice(&4_u32.to_le_bytes());
    runtime.memory.write(package, &compact_header).unwrap();

    assert!(matches!(
        runtime.compact_ram_output_target(
            package,
            compact_header.len(),
            output_len as usize,
            0,
            GuestAddr(0),
        ),
        Err(Error::Abi(message)) if message.contains("ambiguous prepared buffers")
    ));

    let unsupported_slot = output_len_pointer.checked_add(8).unwrap();
    runtime
        .memory
        .write_u32(unsupported_slot, current_output.0)
        .unwrap();
    assert!(matches!(
        runtime.compact_ram_output_target(
            package,
            compact_header.len(),
            output_len as usize,
            0,
            output_len_pointer,
        ),
        Err(Error::Abi(message)) if message.contains("ambiguous prepared buffers")
    ));
    runtime.memory.write_u32(unsupported_slot, 0).unwrap();

    let adjacent_slot = output_len_pointer.checked_add(4).unwrap();
    runtime
        .memory
        .write_u32(adjacent_slot, current_output.0)
        .unwrap();
    assert_eq!(
        runtime
            .compact_ram_output_target(
                package,
                compact_header.len(),
                output_len as usize,
                0,
                output_len_pointer,
            )
            .unwrap(),
        Some(current_output)
    );
    runtime.memory.write_u32(adjacent_slot, 0).unwrap();

    let trailing_slot = output_len_pointer.checked_add(40).unwrap();
    runtime
        .memory
        .write_u32(trailing_slot, current_output.0)
        .unwrap();
    assert!(matches!(
        runtime.compact_ram_output_target(
            package,
            compact_header.len(),
            output_len as usize,
            0,
            output_len_pointer,
        ),
        Err(Error::Abi(message)) if message.contains("ambiguous prepared buffers")
    ));

    runtime
        .memory
        .write_u32(adjacent_slot, current_output.0)
        .unwrap();
    assert_eq!(
        runtime
            .compact_ram_output_target(
                package,
                compact_header.len(),
                output_len as usize,
                0,
                output_len_pointer,
            )
            .unwrap(),
        Some(current_output)
    );
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
        runtime.compact_ram_output_target(
            package,
            compact_header.len(),
            output_len as usize,
            1,
            GuestAddr(0),
        ),
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
            reclaimable_prefix_len: None,
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
fn expanded_compact_output_view_can_restore_its_previous_tail_boundary() {
    let mut runtime =
        ExtRuntime::new(240, 320, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    load_test_module(&mut runtime);
    load_test_module(&mut runtime);
    let backing = runtime
        .allocate_guest_block_for_module(0x300, 0)
        .unwrap()
        .unwrap();
    let prepared = backing.checked_add(0x28).unwrap();
    let owner_generation = runtime.modules[0].generation;

    runtime
        .claim_prepared_output_for_module(prepared, 0x80, 0)
        .unwrap();
    runtime
        .claim_prepared_output_for_module(prepared, 0xc0, 0)
        .unwrap();
    assert_eq!(
        runtime.guest_allocation_views.get(&prepared.0),
        Some(&GuestAllocationView {
            len: 0xc0,
            backing_base: backing.0,
            owner_generation,
            reclaimable_prefix_len: Some(0x80),
        })
    );

    let mut executable = ArmCpu::new();
    executable.set_register(0, 0);
    executable.set_register(1, 9);
    executable.set_register(2, prepared.0);
    executable.set_register(3, 0xc0);
    runtime
        .dispatch(131, 0, &mut executable, &mut StubServices)
        .unwrap();
    assert_eq!(
        runtime.modules[0].dynamic_executable_ranges[0],
        Some(ExecutableRange {
            base: prepared,
            len: 0xc0,
        })
    );

    let free_start = prepared.checked_add(0x80).unwrap();
    let heap_before = runtime.guest_heap_state().unwrap();
    assert!(matches!(
        runtime.free_guest_block_for_module(backing.checked_add(0x20).unwrap(), 0x100, 0),
        Err(Error::Abi(message)) if message.contains("active guest allocation view")
    ));
    assert!(matches!(
        runtime.free_guest_block_for_module(prepared.checked_add(0x88).unwrap(), 0x78, 0),
        Err(Error::Abi(message)) if message.contains("active guest allocation view")
    ));
    assert!(matches!(
        runtime.free_guest_block_for_module(free_start, 0x20, 0),
        Err(Error::Abi(message)) if message.contains("active guest allocation view")
    ));
    assert!(matches!(
        runtime.free_guest_block_for_module(free_start, 0x80, 1),
        Err(Error::Abi(message)) if message.contains("another module")
    ));
    let heap_after_rejected_frees = runtime.guest_heap_state().unwrap();
    assert_eq!(heap_after_rejected_frees.head, heap_before.head);
    assert_eq!(heap_after_rejected_frees.free_left, heap_before.free_left);
    assert!(runtime.memory.fetch_u32(free_start).is_ok());

    runtime
        .memory
        .write_u32(data_slot_address(104), free_start.0)
        .unwrap();
    runtime
        .memory
        .write_u32(data_slot_address(105), 0x80)
        .unwrap();
    runtime
        .free_guest_block_for_module(free_start, 0x80, 0)
        .unwrap();

    assert_eq!(
        runtime.guest_allocation_views.get(&prepared.0),
        Some(&GuestAllocationView {
            len: 0x80,
            backing_base: backing.0,
            owner_generation,
            reclaimable_prefix_len: None,
        })
    );
    assert_eq!(
        runtime.modules[0].dynamic_executable_ranges[0],
        Some(ExecutableRange {
            base: prepared,
            len: 0x80,
        })
    );
    assert!(runtime.memory.fetch_u32(prepared).is_ok());
    assert!(runtime.memory.fetch_u32(free_start).is_err());
    assert_eq!(runtime.memory.read_u32(data_slot_address(104)).unwrap(), 0);
    assert_eq!(runtime.memory.read_u32(data_slot_address(105)).unwrap(), 0);
    assert_eq!(runtime.guest_allocations.get(&backing.0), Some(&0x300));
    assert_eq!(
        runtime.guest_allocation_owners.get(&backing.0),
        Some(&owner_generation)
    );

    let heap = runtime.guest_heap_state().unwrap();
    let (blocks, terminator, recovered_len) = runtime.read_free_blocks(heap).unwrap();
    assert_eq!(
        blocks,
        [
            FreeBlock {
                offset: free_start.0 - heap.base,
                len: 0x80,
            },
            FreeBlock {
                offset: backing.0 + 0x300 - heap.base,
                len: heap.span - (backing.0 + 0x300 - heap.base),
            },
        ]
    );
    assert_eq!(terminator, heap.span);
    assert_eq!(recovered_len, 0);
    assert_eq!(heap.free_left, heap_before.free_left + 0x80);
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
            reclaimable_prefix_len: None,
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
            .compact_ram_output_target(
                package,
                compact_header.len(),
                output_len as usize,
                0,
                GuestAddr(0),
            )
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
fn keeps_the_legacy_screen_address_and_complete_ram_window_for_ordinary_heaps() {
    let mut runtime =
        ExtRuntime::new(240, 320, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    let final_ram_page = GuestAddr(SCREEN_BASE.0 - GUEST_MEMORY_GUARD_LEN);

    assert_eq!(runtime.screen_base, SCREEN_BASE);
    assert_eq!(
        runtime.memory.read_u32(data_slot_address(91)).unwrap(),
        SCREEN_BASE.0
    );
    runtime
        .memory
        .write_u32(final_ram_page, 0x1234_5678)
        .unwrap();
    assert_eq!(
        runtime.memory.read_u32(final_ram_page).unwrap(),
        0x1234_5678
    );
    assert!(runtime.memory.read(GuestAddr(SCREEN_BASE.0 - 1), 1).is_ok());
    assert!(runtime.memory.read(SCREEN_BASE, 1).is_ok());
}

#[test]
fn selects_the_expanded_screen_only_above_the_legacy_ram_window() {
    let legacy = ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", MIN_GUEST_RAM_LEN as u32).unwrap();
    let expanded =
        ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", MIN_GUEST_RAM_LEN as u32 + 1).unwrap();

    assert_eq!(legacy.screen_base, SCREEN_BASE);
    assert_eq!(expanded.screen_base, EXPANDED_SCREEN_BASE);
}

#[test]
fn keeps_an_unmapped_guard_between_the_largest_heap_and_expanded_screen() {
    let runtime = ExtRuntime::new(
        240,
        320,
        b"test.mrp",
        b"start.mr",
        MAX_GUEST_HEAP_LEN as u32,
    )
    .unwrap();
    let heap_end = GuestAddr(HEAP_BASE.0 + MAX_GUEST_HEAP_LEN as u32);

    assert_eq!(runtime.screen_base, EXPANDED_SCREEN_BASE);
    assert_eq!(
        runtime.memory.read_u32(data_slot_address(91)).unwrap(),
        EXPANDED_SCREEN_BASE.0
    );
    assert_eq!(EXPANDED_SCREEN_BASE.0 - heap_end.0, GUEST_MEMORY_GUARD_LEN);
    assert!(runtime.memory.read(GuestAddr(heap_end.0 - 1), 1).is_ok());
    assert!(runtime.memory.read(heap_end, 1).is_err());
    assert!(
        runtime
            .memory
            .read(heap_end, GUEST_MEMORY_GUARD_LEN as usize)
            .is_err()
    );
    assert!(runtime.memory.read(EXPANDED_SCREEN_BASE, 1).is_ok());
}

#[test]
fn draws_and_presents_from_the_expanded_screen_address() {
    let mut runtime =
        ExtRuntime::new(2, 2, b"test.mrp", b"start.mr", MAX_GUEST_HEAP_LEN as u32).unwrap();
    let _capture = capture_stub_framebuffer(vec![0; 2 * 2 * 2]);
    let legacy_heap_marker = 0x55aa;
    let screen_color = 0x1234;
    runtime
        .memory
        .write_u16(SCREEN_BASE, legacy_heap_marker)
        .unwrap();

    runtime
        .draw_rectangle_to_screen(0, 0, 1, 1, screen_color)
        .unwrap();
    runtime.present_screen(&mut StubServices).unwrap();

    assert_eq!(
        runtime.screen_address(0, 0, 2).unwrap(),
        EXPANDED_SCREEN_BASE
    );
    assert_eq!(
        runtime.memory.read_u16(EXPANDED_SCREEN_BASE).unwrap(),
        screen_color
    );
    assert_eq!(
        runtime.memory.read_u16(SCREEN_BASE).unwrap(),
        legacy_heap_marker
    );
    assert_eq!(
        STUB_FRAMEBUFFER.with(|framebuffer| framebuffer.borrow().clone()),
        Some(vec![0x34, 0x12, 0, 0, 0, 0, 0, 0])
    );
}

#[test]
fn maps_screen_staging_capacity_beyond_the_visible_framebuffer() {
    let mut runtime =
        ExtRuntime::new(240, 320, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    let last_staging_byte = runtime
        .screen_base
        .checked_add(SCREEN_STAGING_CAPACITY as u32 - 1)
        .unwrap();
    let after_staging = runtime
        .screen_base
        .checked_add(SCREEN_STAGING_CAPACITY as u32)
        .unwrap();

    assert_eq!(runtime.screen_memory_len, SCREEN_STAGING_CAPACITY);
    runtime.memory.write(last_staging_byte, &[0x5a]).unwrap();
    assert_eq!(runtime.memory.read(last_staging_byte, 1).unwrap(), [0x5a]);
    assert!(runtime.memory.write(after_staging, &[0xa5]).is_err());
}

#[test]
fn rejects_heaps_that_would_cross_the_guarded_memory_layout() {
    assert!(matches!(
        ExtRuntime::new(
            240,
            320,
            b"test.mrp",
            b"start.mr",
            MAX_GUEST_HEAP_LEN as u32 + 1,
        ),
        Err(Error::ArmFault(message)) if message.contains("exceeds supported maximum")
    ));
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
