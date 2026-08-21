use super::*;

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
fn bitmap_trap_accepts_a_linear_source_offset_at_the_stride_boundary() {
    let mut runtime =
        ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    let source = runtime.allocate(24 * 2, 2).unwrap();
    for (index, color) in (1_u16..=24).enumerate() {
        runtime
            .memory
            .write_u16(source.checked_add((index * 2) as u32).unwrap(), color)
            .unwrap();
    }
    let stack = runtime.allocate(24, 4).unwrap();
    for (index, value) in [2_u32, 2, 0, 4, 1, 4].into_iter().enumerate() {
        runtime
            .memory
            .write_u32(stack.checked_add((index * 4) as u32).unwrap(), value)
            .unwrap();
    }

    let mut cpu = ArmCpu::new();
    cpu.set_register(0, source.0);
    cpu.set_register(1, 0);
    cpu.set_register(2, 0);
    cpu.set_register(3, 4);
    cpu.set_register(13, stack.0);
    runtime
        .dispatch(120, 0, &mut cpu, &mut StubServices)
        .unwrap();

    let first_row = read_bitmap_pixels(&runtime, SCREEN_BASE, 8, 1);
    let second_row = read_bitmap_pixels(&runtime, SCREEN_BASE.checked_add(8 * 2).unwrap(), 8, 1);
    assert_eq!(&first_row[..4], [9, 10, 11, 12]);
    assert_eq!(&second_row[..4], [13, 14, 15, 16]);
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
