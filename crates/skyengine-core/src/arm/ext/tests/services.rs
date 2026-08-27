use std::{
    io::{Read, Write},
    net::{Ipv4Addr, TcpListener},
    thread,
    time::{Duration, Instant},
};

use super::*;

fn verified_platform_jpeg() -> Vec<u8> {
    let fixture =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test/fixtures/talkcat.mrp");
    Package::open(fixture, ResourceLimits::default())
        .unwrap()
        .read_named(b"startw.jpg")
        .unwrap()
}

fn dispatch_platform_extension(
    runtime: &mut ExtRuntime,
    command: u32,
    input: GuestAddr,
    input_len: u32,
    output: GuestAddr,
    output_len: GuestAddr,
) -> ArmCpu {
    let stack = runtime.allocate(8, 4).unwrap();
    runtime.memory.write_u32(stack, output_len.0).unwrap();
    runtime
        .memory
        .write_u32(stack.checked_add(4).unwrap(), 0xfeed_face)
        .unwrap();
    let mut cpu = ArmCpu::new();
    cpu.set_register(0, command);
    cpu.set_register(1, input.0);
    cpu.set_register(2, input_len);
    cpu.set_register(3, output.0);
    cpu.set_register(13, stack.0);
    runtime
        .dispatch(38, 0, &mut cpu, &mut StubServices)
        .unwrap();
    assert_eq!(
        runtime
            .memory
            .read_u32(stack.checked_add(4).unwrap())
            .unwrap(),
        0xfeed_face
    );
    cpu
}

#[test]
fn platform_jpeg_info_returns_verified_dimensions() {
    let mut runtime =
        ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    let jpeg = verified_platform_jpeg();
    let source = runtime.allocate(jpeg.len(), 4).unwrap();
    runtime.memory.write(source, &jpeg).unwrap();
    let input = runtime.allocate(12, 4).unwrap();
    for (index, value) in [source.0, jpeg.len() as u32, 1].into_iter().enumerate() {
        runtime
            .memory
            .write_u32(input.checked_add((index * 4) as u32).unwrap(), value)
            .unwrap();
    }
    let output = runtime.allocate(4, 4).unwrap();
    let output_len = runtime.allocate(4, 4).unwrap();

    let cpu = dispatch_platform_extension(&mut runtime, 3_001, input, 12, output, output_len);

    assert_eq!(cpu.register(0), 0);
    assert_eq!(runtime.memory.read_u32(output_len).unwrap(), 8);
    let info = GuestAddr(runtime.memory.read_u32(output).unwrap());
    assert_eq!(runtime.memory.read_u32(info).unwrap(), 240);
    assert_eq!(
        runtime
            .memory
            .read_u32(info.checked_add(4).unwrap())
            .unwrap(),
        320
    );
}

#[test]
fn platform_jpeg_info_failure_clears_output_fields() {
    let mut runtime =
        ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    let source = runtime.allocate(4, 4).unwrap();
    runtime
        .memory
        .write(source, &[0xff, 0xd8, 0xff, 0xd9])
        .unwrap();
    let input = runtime.allocate(12, 4).unwrap();
    for (index, value) in [source.0, 4, 1].into_iter().enumerate() {
        runtime
            .memory
            .write_u32(input.checked_add((index * 4) as u32).unwrap(), value)
            .unwrap();
    }
    let output = runtime.allocate(4, 4).unwrap();
    let output_len = runtime.allocate(4, 4).unwrap();
    runtime.memory.write_u32(output, 0xaaaa_aaaa).unwrap();
    runtime.memory.write_u32(output_len, 0xbbbb_bbbb).unwrap();

    let cpu = dispatch_platform_extension(&mut runtime, 3_001, input, 12, output, output_len);

    assert_eq!(cpu.register(0), u32::MAX);
    assert_eq!(runtime.memory.read_u32(output).unwrap(), 0);
    assert_eq!(runtime.memory.read_u32(output_len).unwrap(), 0);
}

#[test]
fn platform_jpeg_decode_writes_little_endian_rgb565() {
    let mut runtime =
        ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    let jpeg = verified_platform_jpeg();
    let source = runtime.allocate(jpeg.len(), 4).unwrap();
    runtime.memory.write(source, &jpeg).unwrap();
    let destination = runtime.allocate(240 * 320 * 2, 2).unwrap();
    let input = runtime.allocate(24, 4).unwrap();
    for (index, value) in [source.0, jpeg.len() as u32, 240, 320, 1, destination.0]
        .into_iter()
        .enumerate()
    {
        runtime
            .memory
            .write_u32(input.checked_add((index * 4) as u32).unwrap(), value)
            .unwrap();
    }

    let cpu =
        dispatch_platform_extension(&mut runtime, 3_002, input, 24, GuestAddr(0), GuestAddr(0));

    assert_eq!(cpu.register(0), 0);
    assert_eq!(
        runtime.memory.read(destination, 16).unwrap(),
        [
            0x62, 0x00, 0x62, 0x00, 0x62, 0x00, 0x62, 0x00, 0x62, 0x00, 0x62, 0x00, 0x62, 0x00,
            0x62, 0x00,
        ]
    );
}

#[test]
fn platform_jpeg_decode_rejects_dimension_mismatches_without_writing() {
    let mut runtime =
        ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    let jpeg = verified_platform_jpeg();
    let source = runtime.allocate(jpeg.len(), 4).unwrap();
    runtime.memory.write(source, &jpeg).unwrap();
    let destination = runtime.allocate(8, 2).unwrap();
    runtime.memory.write(destination, &[0xaa; 8]).unwrap();
    let input = runtime.allocate(24, 4).unwrap();
    for (index, value) in [source.0, jpeg.len() as u32, 1, 1, 1, destination.0]
        .into_iter()
        .enumerate()
    {
        runtime
            .memory
            .write_u32(input.checked_add((index * 4) as u32).unwrap(), value)
            .unwrap();
    }

    let cpu =
        dispatch_platform_extension(&mut runtime, 3_002, input, 24, GuestAddr(0), GuestAddr(0));

    assert_eq!(cpu.register(0), u32::MAX);
    assert_eq!(runtime.memory.read(destination, 8).unwrap(), [0xaa; 8]);
}

#[test]
fn platform_jpeg_decode_rejects_a_tracked_destination_that_is_too_small() {
    let mut runtime =
        ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    let jpeg = verified_platform_jpeg();
    let source = runtime.allocate(jpeg.len(), 4).unwrap();
    runtime.memory.write(source, &jpeg).unwrap();
    let destination = runtime.allocate(8, 2).unwrap();
    runtime.memory.write(destination, &[0xaa; 8]).unwrap();
    let input = runtime.allocate(24, 4).unwrap();
    for (index, value) in [source.0, jpeg.len() as u32, 240, 320, 1, destination.0]
        .into_iter()
        .enumerate()
    {
        runtime
            .memory
            .write_u32(input.checked_add((index * 4) as u32).unwrap(), value)
            .unwrap();
    }

    let cpu =
        dispatch_platform_extension(&mut runtime, 3_002, input, 24, GuestAddr(0), GuestAddr(0));

    assert_eq!(cpu.register(0), u32::MAX);
    assert_eq!(runtime.memory.read(destination, 8).unwrap(), [0xaa; 8]);
}

#[test]
fn platform_jpeg_handle_commands_remain_unsupported() {
    let mut runtime =
        ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    let mut cpu = ArmCpu::new();

    for command in [3_004, 3_005] {
        cpu.set_register(0, command);
        assert!(matches!(
            runtime.dispatch(38, 0, &mut cpu, &mut StubServices),
            Err(Error::Abi(message))
                if message.contains(&format!("JPEG handle command {command}"))
        ));
    }
}

#[test]
fn platform_command_2013_whitelists_only_the_parameterless_form() {
    let mut runtime =
        ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    let stack = runtime.allocate(8, 4).unwrap();
    runtime.memory.write(stack, &[0; 8]).unwrap();
    let mut cpu = ArmCpu::new();
    cpu.set_register(0, 2_013);
    cpu.set_register(13, stack.0);

    runtime
        .dispatch(38, 0, &mut cpu, &mut StubServices)
        .unwrap();

    assert_eq!(cpu.register(0), u32::MAX);

    for register in 1..=3 {
        cpu.set_register(0, 2_013);
        cpu.set_register(register, 1);
        assert!(matches!(
            runtime.dispatch(38, 0, &mut cpu, &mut StubServices),
            Err(Error::Abi(message)) if message.contains("command 2013")
        ));
        cpu.set_register(register, 0);
    }

    runtime.memory.write_u32(stack, 1).unwrap();
    cpu.set_register(0, 2_013);
    assert!(matches!(
        runtime.dispatch(38, 0, &mut cpu, &mut StubServices),
        Err(Error::Abi(message)) if message.contains("command 2013")
    ));

    runtime.memory.write_u32(stack, 0).unwrap();
    runtime
        .memory
        .write_u32(stack.checked_add(4).unwrap(), 1)
        .unwrap();
    cpu.set_register(0, 2_013);
    assert!(matches!(
        runtime.dispatch(38, 0, &mut cpu, &mut StubServices),
        Err(Error::Abi(message)) if message.contains("command 2013")
    ));
}

#[test]
fn platform_command_2023_uses_a_bounded_existing_mp3_path() {
    let mut runtime =
        ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    let stack = runtime.allocate(8, 4).unwrap();
    runtime.memory.write(stack, &[0; 8]).unwrap();
    let path = runtime.allocate(32, 1).unwrap();
    runtime.memory.write(path, b"media/clip.mp3").unwrap();
    let mut cpu = ArmCpu::new();
    cpu.set_register(0, 2_023);
    cpu.set_register(1, path.0);
    cpu.set_register(2, 14);
    cpu.set_register(13, stack.0);

    runtime
        .dispatch(38, 0, &mut cpu, &mut StubServices)
        .unwrap();

    assert_eq!(cpu.register(0), 0);

    runtime.memory.write(path, b"media/missing.mp3").unwrap();
    cpu.set_register(0, 2_023);
    cpu.set_register(2, 17);
    runtime
        .dispatch(38, 0, &mut cpu, &mut StubServices)
        .unwrap();
    assert_eq!(cpu.register(0), u32::MAX);

    runtime.memory.write(path, b"media/package.mp3").unwrap();
    cpu.set_register(0, 2_023);
    cpu.set_register(2, 17);
    runtime
        .dispatch(38, 0, &mut cpu, &mut StubServices)
        .unwrap();
    assert_eq!(cpu.register(0), 0);
}

#[test]
fn platform_command_2023_rejects_unverified_request_shapes() {
    let invalid_paths = [
        b"media/clip.wav".as_slice(),
        b"media/clip.mp3\0suffix",
        b".mp3",
        b"../clip.mp3",
        b"media//clip.mp3",
    ];
    for invalid_path in invalid_paths {
        let mut runtime =
            ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
        let stack = runtime.allocate(8, 4).unwrap();
        runtime.memory.write(stack, &[0; 8]).unwrap();
        let path = runtime.allocate(invalid_path.len(), 1).unwrap();
        runtime.memory.write(path, invalid_path).unwrap();
        let mut cpu = ArmCpu::new();
        cpu.set_register(0, 2_023);
        cpu.set_register(1, path.0);
        cpu.set_register(2, invalid_path.len() as u32);
        cpu.set_register(13, stack.0);

        assert!(matches!(
            runtime.dispatch(38, 0, &mut cpu, &mut StubServices),
            Err(Error::Abi(message)) if message.contains("platform MP3 path")
        ));
    }

    let mut runtime =
        ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    let stack = runtime.allocate(8, 4).unwrap();
    runtime.memory.write(stack, &[0; 8]).unwrap();
    let path = runtime.allocate(4_097, 1).unwrap();
    runtime.memory.write(path, &vec![b'a'; 4_097]).unwrap();
    let mut cpu = ArmCpu::new();
    cpu.set_register(0, 2_023);
    cpu.set_register(1, path.0);
    cpu.set_register(2, 4_097);
    cpu.set_register(13, stack.0);
    assert!(matches!(
        runtime.dispatch(38, 0, &mut cpu, &mut StubServices),
        Err(Error::Abi(message)) if message.contains("platform MP3 request")
    ));

    cpu.set_register(0, 2_023);
    cpu.set_register(2, 14);
    cpu.set_register(3, 1);
    assert!(matches!(
        runtime.dispatch(38, 0, &mut cpu, &mut StubServices),
        Err(Error::Abi(message)) if message.contains("platform MP3 request")
    ));
}

#[test]
fn platform_command_2043_whitelists_only_the_parameterless_form() {
    let mut runtime =
        ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    let stack = runtime.allocate(8, 4).unwrap();
    runtime.memory.write(stack, &[0; 8]).unwrap();
    let mut cpu = ArmCpu::new();
    cpu.set_register(0, 2_043);
    cpu.set_register(13, stack.0);

    runtime
        .dispatch(38, 0, &mut cpu, &mut StubServices)
        .unwrap();
    assert_eq!(cpu.register(0), 0);

    for register in 1..=3 {
        cpu.set_register(0, 2_043);
        cpu.set_register(register, 1);
        assert!(matches!(
            runtime.dispatch(38, 0, &mut cpu, &mut StubServices),
            Err(Error::Abi(message)) if message.contains("command 2043")
        ));
        cpu.set_register(register, 0);
    }

    for offset in [0, 4] {
        runtime
            .memory
            .write_u32(stack.checked_add(offset).unwrap(), 1)
            .unwrap();
        cpu.set_register(0, 2_043);
        assert!(matches!(
            runtime.dispatch(38, 0, &mut cpu, &mut StubServices),
            Err(Error::Abi(message)) if message.contains("command 2043")
        ));
        runtime
            .memory
            .write_u32(stack.checked_add(offset).unwrap(), 0)
            .unwrap();
    }
}

#[test]
fn platform_command_2093_whitelists_only_the_parameterless_idle_query() {
    let mut runtime =
        ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    let stack = runtime.allocate(8, 4).unwrap();
    runtime.memory.write(stack, &[0; 8]).unwrap();
    let mut cpu = ArmCpu::new();
    cpu.set_register(0, 2_093);
    cpu.set_register(13, stack.0);

    runtime
        .dispatch(38, 0, &mut cpu, &mut StubServices)
        .unwrap();
    assert_eq!(cpu.register(0), 1_003);

    for register in 1..=3 {
        cpu.set_register(0, 2_093);
        cpu.set_register(register, 1);
        assert!(matches!(
            runtime.dispatch(38, 0, &mut cpu, &mut StubServices),
            Err(Error::Abi(message)) if message.contains("command 2093")
        ));
        cpu.set_register(register, 0);
    }

    for offset in [0, 4] {
        runtime
            .memory
            .write_u32(stack.checked_add(offset).unwrap(), 1)
            .unwrap();
        cpu.set_register(0, 2_093);
        assert!(matches!(
            runtime.dispatch(38, 0, &mut cpu, &mut StubServices),
            Err(Error::Abi(message)) if message.contains("command 2093")
        ));
        runtime
            .memory
            .write_u32(stack.checked_add(offset).unwrap(), 0)
            .unwrap();
    }
}

#[test]
fn platform_command_2700_accepts_only_the_verified_wav_recording_shape() {
    let mut runtime =
        ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    let path = runtime.allocate(17, 1).unwrap();
    runtime.memory.write(path, b"media/record.WAV\0").unwrap();
    let input = runtime.allocate(16, 4).unwrap();
    for (index, value) in [path.0, 0, 0, 1].into_iter().enumerate() {
        runtime
            .memory
            .write_u32(input.checked_add((index * 4) as u32).unwrap(), value)
            .unwrap();
    }
    let stack = runtime.allocate(8, 4).unwrap();
    runtime.memory.write(stack, &[0; 8]).unwrap();
    let mut cpu = ArmCpu::new();
    cpu.set_register(0, 2_700);
    cpu.set_register(1, input.0);
    cpu.set_register(2, 16);
    cpu.set_register(13, stack.0);

    runtime
        .dispatch(38, 0, &mut cpu, &mut StubServices)
        .unwrap();

    assert_eq!(cpu.register(0), u32::MAX);
}

#[test]
fn platform_command_2700_rejects_unverified_fields_and_paths() {
    let mut runtime =
        ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    let path = runtime.allocate(17, 1).unwrap();
    runtime.memory.write(path, b"media/record.wav\0").unwrap();
    let input = runtime.allocate(16, 4).unwrap();
    let stack = runtime.allocate(8, 4).unwrap();
    runtime.memory.write(stack, &[0; 8]).unwrap();
    let mut cpu = ArmCpu::new();
    cpu.set_register(13, stack.0);

    let configure_valid_request = |runtime: &mut ExtRuntime, cpu: &mut ArmCpu| {
        for (index, value) in [path.0, 0, 0, 1].into_iter().enumerate() {
            runtime
                .memory
                .write_u32(input.checked_add((index * 4) as u32).unwrap(), value)
                .unwrap();
        }
        cpu.set_register(0, 2_700);
        cpu.set_register(1, input.0);
        cpu.set_register(2, 16);
        cpu.set_register(3, 0);
    };

    for (register, value) in [(1, 0), (2, 15), (3, 1)] {
        configure_valid_request(&mut runtime, &mut cpu);
        cpu.set_register(register, value);
        assert!(matches!(
            runtime.dispatch(38, 0, &mut cpu, &mut StubServices),
            Err(Error::Abi(message)) if message.contains("WAV recording request")
        ));
    }

    for offset in [0, 4] {
        configure_valid_request(&mut runtime, &mut cpu);
        runtime
            .memory
            .write_u32(stack.checked_add(offset).unwrap(), 1)
            .unwrap();
        assert!(matches!(
            runtime.dispatch(38, 0, &mut cpu, &mut StubServices),
            Err(Error::Abi(message)) if message.contains("WAV recording request")
        ));
        runtime
            .memory
            .write_u32(stack.checked_add(offset).unwrap(), 0)
            .unwrap();
    }

    for (offset, value) in [(0, 0), (4, 1), (8, 1), (12, 0), (12, 2)] {
        configure_valid_request(&mut runtime, &mut cpu);
        runtime
            .memory
            .write_u32(input.checked_add(offset).unwrap(), value)
            .unwrap();
        assert!(matches!(
            runtime.dispatch(38, 0, &mut cpu, &mut StubServices),
            Err(Error::Abi(message)) if message.contains("WAV recording request")
        ));
    }

    for invalid_path in [
        b".wav".as_slice(),
        b"../record.wav",
        b"media//record.wav",
        b"media/record.mp3",
    ] {
        let address = runtime.allocate(invalid_path.len() + 1, 1).unwrap();
        runtime.memory.write(address, invalid_path).unwrap();
        runtime
            .memory
            .write_u8(address.checked_add(invalid_path.len() as u32).unwrap(), 0)
            .unwrap();
        configure_valid_request(&mut runtime, &mut cpu);
        runtime.memory.write_u32(input, address.0).unwrap();
        assert!(matches!(
            runtime.dispatch(38, 0, &mut cpu, &mut StubServices),
            Err(Error::Abi(message)) if message.contains("WAV recording path")
        ));
    }

    let unterminated_path = runtime.allocate(4 * 1024, 1).unwrap();
    runtime
        .memory
        .write(unterminated_path, &vec![b'a'; 4 * 1024])
        .unwrap();
    configure_valid_request(&mut runtime, &mut cpu);
    runtime
        .memory
        .write_u32(input, unterminated_path.0)
        .unwrap();
    assert!(matches!(
        runtime.dispatch(38, 0, &mut cpu, &mut StubServices),
        Err(Error::Abi(message)) if message.contains("exceeds 4096 bytes")
    ));
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
fn datetime_uses_the_configured_device_date() {
    let mut runtime =
        ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    runtime.set_device_date(DeviceDate::new(2000, 2, 29).unwrap());
    let output = runtime.allocate(8, 2).unwrap();
    let mut cpu = ArmCpu::new();
    cpu.set_register(0, output.0);

    runtime
        .dispatch(34, 0, &mut cpu, &mut StubServices)
        .unwrap();

    assert_eq!(
        runtime.memory.read(output, 8).unwrap(),
        [0xd0, 0x07, 2, 29, 0, 0, 0, 2]
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
fn guest_ascii_character_bitmap_uses_one_byte_per_scanline() {
    let mut runtime =
        ExtRuntime::new(16, 16, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    let width_out = runtime.allocate(4, 4).unwrap();
    let height_out = runtime.allocate(4, 4).unwrap();
    let mut cpu = ArmCpu::new();
    cpu.set_register(0, u32::from(b'%'));
    cpu.set_register(1, 1);
    cpu.set_register(2, width_out.0);
    cpu.set_register(3, height_out.0);

    runtime
        .dispatch(30, 0, &mut cpu, &mut StubServices)
        .unwrap();

    let bitmap = GuestAddr(cpu.register(0));
    assert_ne!(bitmap.0, 0);
    assert_eq!(runtime.memory.read(bitmap, 2).unwrap(), [0x01, 0x02]);
    assert_eq!(runtime.memory.read_u32(width_out).unwrap(), 8);
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
fn gray_bitmap_mode_preserves_the_key_and_clips_the_source_region() {
    let mut runtime =
        ExtRuntime::new(3, 2, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    let transparent = 0xf81f_u16;
    let red = Framebuffer::rgb565(255, 0, 0);
    let blue = Framebuffer::rgb565(0, 0, 255);
    let source = runtime.allocate(12, 2).unwrap();
    let source_pixels = [0_u16, transparent, red, 0, blue, transparent];
    runtime
        .memory
        .write(
            source,
            &source_pixels
                .into_iter()
                .flat_map(u16::to_le_bytes)
                .collect::<Vec<_>>(),
        )
        .unwrap();
    let preserved = 0x1234;
    runtime
        .memory
        .write_u16(runtime.screen_address(0, 1, 3).unwrap(), preserved)
        .unwrap();
    let stack = runtime.allocate(24, 4).unwrap();
    for (offset, value) in [2_u32, 8, u32::from(transparent), 1, 0, 3]
        .into_iter()
        .enumerate()
    {
        runtime
            .memory
            .write_u32(stack.checked_add((offset * 4) as u32).unwrap(), value)
            .unwrap();
    }
    let mut cpu = ArmCpu::new();
    cpu.set_register(0, source.0);
    cpu.set_register(1, u32::MAX);
    cpu.set_register(2, 0);
    cpu.set_register(3, 2);
    cpu.set_register(13, stack.0);

    runtime
        .dispatch(120, 0, &mut cpu, &mut StubServices)
        .unwrap();

    assert_eq!(cpu.register(0), 0);
    assert_eq!(
        runtime
            .memory
            .read_u16(runtime.screen_address(0, 0, 3).unwrap())
            .unwrap(),
        Framebuffer::rgb565(77, 77, 77)
    );
    assert_eq!(
        runtime
            .memory
            .read_u16(runtime.screen_address(0, 1, 3).unwrap())
            .unwrap(),
        preserved
    );
}

#[test]
fn user_info_returns_the_deterministic_headless_baseline() {
    let mut runtime =
        ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    let output = runtime.allocate(PLATFORM_USER_INFO_LEN, 4).unwrap();
    runtime
        .memory
        .write(output, &[0xaa; PLATFORM_USER_INFO_LEN])
        .unwrap();
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
        &runtime.memory.read(output, PLATFORM_USER_INFO_LEN).unwrap()[16..31],
        VIRTUAL_IMSI
    );
    assert_eq!(
        runtime
            .memory
            .read_u32(output.checked_add(48).unwrap())
            .unwrap(),
        PLATFORM_USER_INFO_VERSION
    );
}

#[test]
fn user_info_rejects_a_null_output() {
    let mut runtime =
        ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    let mut cpu = ArmCpu::new();
    cpu.set_register(0, 0);

    runtime
        .dispatch(35, 0, &mut cpu, &mut StubServices)
        .unwrap();

    assert_eq!(cpu.register(0) as i32, -1);
}

#[test]
fn mtk_profile_reserves_an_extension_window_until_slot_131_marks_code_executable() {
    let mut runtime =
        ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    runtime
        .set_native_extension_profile(NativeExtensionProfile::Mtk)
        .unwrap();
    let mut module = b"MRPGCMAP".to_vec();
    module.extend_from_slice(&0xe12f_ff1e_u32.to_le_bytes());
    runtime
        .load_and_call_entry(&module, 0, &mut StubServices)
        .unwrap();
    runtime
        .memory
        .write(MTK_NATIVE_EXTENSION_BASE, &[0x70, 0x47])
        .unwrap();

    assert!(runtime.memory.fetch_u16(MTK_NATIVE_EXTENSION_BASE).is_err());

    let mut cpu = ArmCpu::new();
    cpu.set_register(0, 0);
    cpu.set_register(1, 9);
    cpu.set_register(2, MTK_NATIVE_EXTENSION_BASE.0);
    cpu.set_register(3, 2);
    runtime
        .dispatch(131, 0, &mut cpu, &mut StubServices)
        .unwrap();

    assert_eq!(cpu.register(0), 0);
    assert_eq!(
        runtime.memory.fetch_u16(MTK_NATIVE_EXTENSION_BASE).unwrap(),
        0x4770
    );
    assert_eq!(
        runtime.modules[0].dynamic_executable_ranges,
        [Some(ExecutableRange {
            base: MTK_NATIVE_EXTENSION_BASE,
            len: 2,
        })]
    );

    cpu.set_register(0, 0);
    cpu.set_register(3, 4);
    runtime
        .dispatch(131, 0, &mut cpu, &mut StubServices)
        .unwrap();
    assert_eq!(
        runtime.modules[0].dynamic_executable_ranges,
        [Some(ExecutableRange {
            base: MTK_NATIVE_EXTENSION_BASE,
            len: 4,
        })]
    );

    runtime
        .load_and_call_entry(&module, 0, &mut StubServices)
        .unwrap();
    cpu.set_register(0, 0);
    cpu.set_register(3, 2);
    assert!(matches!(
        runtime.dispatch(131, 1, &mut cpu, &mut StubServices),
        Err(Error::Abi(message)) if message.contains("another module")
    ));
    assert!(runtime.modules[1].dynamic_executable_ranges.is_empty());
}

#[test]
fn optional_device_metric_reports_unavailable() {
    let mut runtime =
        ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    let mut cpu = ArmCpu::new();
    cpu.set_register(0, 1_101);
    cpu.set_register(1, 2);

    runtime
        .dispatch(37, 0, &mut cpu, &mut StubServices)
        .unwrap();

    assert_eq!(cpu.register(0) as i32, -1);
}

#[test]
fn optional_runtime_profile_reports_unavailable() {
    let mut runtime =
        ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    let mut cpu = ArmCpu::new();
    cpu.set_register(0, 1_100);
    cpu.set_register(1, 0);

    runtime
        .dispatch(37, 0, &mut cpu, &mut StubServices)
        .unwrap();
    assert_eq!(cpu.register(0), u32::MAX);

    cpu.set_register(0, 1_100);
    cpu.set_register(1, 1);
    assert!(matches!(
        runtime.dispatch(37, 0, &mut cpu, &mut StubServices),
        Err(Error::Abi(message)) if message.contains("command (1100, 1)")
    ));
}

#[test]
fn platform_screen_mode_updates_dimensions_and_screen_bitmap() {
    let mut runtime =
        ExtRuntime::new(320, 480, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    let bitmap_table = GuestAddr(runtime.memory.read_u32(table_slot_address(95)).unwrap());
    let screen_bitmap = bitmap_table
        .checked_add(SCREEN_BITMAP_ID * BITMAP_ENTRY_SIZE)
        .unwrap();
    let mut cpu = ArmCpu::new();

    cpu.set_register(0, 101);
    cpu.set_register(1, 3);
    runtime
        .dispatch(37, 0, &mut cpu, &mut StubServices)
        .unwrap();
    assert_eq!(cpu.register(0), 0);
    assert_eq!(runtime.screen_dimensions().unwrap(), (480, 320));
    assert_eq!(runtime.memory.read_u16(screen_bitmap).unwrap(), 480);
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
        320 * 480 * 2
    );

    cpu.set_register(0, 101);
    cpu.set_register(1, 0);
    runtime
        .dispatch(37, 0, &mut cpu, &mut StubServices)
        .unwrap();
    assert_eq!(cpu.register(0), 0);
    assert_eq!(runtime.screen_dimensions().unwrap(), (320, 480));
    assert_eq!(runtime.memory.read_u16(screen_bitmap).unwrap(), 320);
    assert_eq!(
        runtime
            .memory
            .read_u16(screen_bitmap.checked_add(2).unwrap())
            .unwrap(),
        480
    );
}

#[test]
fn platform_screen_mode_rejects_unverified_values() {
    let mut runtime =
        ExtRuntime::new(320, 480, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    let mut cpu = ArmCpu::new();
    cpu.set_register(0, 101);
    cpu.set_register(1, 2);

    assert!(matches!(
        runtime.dispatch(37, 0, &mut cpu, &mut StubServices),
        Err(Error::Abi(message)) if message.contains("unsupported platform slot 37 command (101, 2)")
    ));
    assert_eq!(runtime.screen_dimensions().unwrap(), (320, 480));
}

#[test]
fn platform_payment_foreground_notification_is_accepted() {
    let mut runtime =
        ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    for mode in 0..=1 {
        let mut cpu = ArmCpu::new();
        cpu.set_register(0, 1_011);
        cpu.set_register(1, mode);

        runtime
            .dispatch(37, 0, &mut cpu, &mut StubServices)
            .unwrap();

        assert_eq!(cpu.register(0), 0);
    }
}

#[test]
fn parameterless_runtime_service_notifications_are_strictly_accepted() {
    let mut runtime =
        ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    let mut cpu = ArmCpu::new();

    for command in [1_016, 1_020, 1_215] {
        cpu.set_register(0, command);
        cpu.set_register(1, 0);
        runtime
            .dispatch(37, 0, &mut cpu, &mut StubServices)
            .unwrap();
        assert_eq!(
            cpu.register(0),
            if command == 1_020 { 1 } else { 0 },
            "command {command}"
        );

        cpu.set_register(0, command);
        cpu.set_register(1, 1);
        assert!(matches!(
            runtime.dispatch(37, 0, &mut cpu, &mut StubServices),
            Err(Error::Abi(message)) if message.contains(&format!("command ({command}, 1)"))
        ));
    }
}

#[test]
fn platform_command_2703_whitelists_only_the_parameterless_form() {
    let mut runtime =
        ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    let mut cpu = ArmCpu::new();
    cpu.set_register(0, 2_703);
    cpu.set_register(1, 0);

    runtime
        .dispatch(37, 0, &mut cpu, &mut StubServices)
        .unwrap();
    assert_eq!(cpu.register(0), 0);

    cpu.set_register(0, 2_703);
    cpu.set_register(1, 1);
    assert!(matches!(
        runtime.dispatch(37, 0, &mut cpu, &mut StubServices),
        Err(Error::Abi(message)) if message.contains("command (2703, 1)")
    ));
}

#[test]
fn unavailable_buffered_capabilities_clear_outputs_and_reject_inputs() {
    let mut runtime =
        ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    let output = runtime.allocate(4, 4).unwrap();
    let output_len = runtime.allocate(4, 4).unwrap();
    let stack = runtime.allocate(4, 4).unwrap();
    runtime.memory.write_u32(stack, output_len.0).unwrap();
    let mut cpu = ArmCpu::new();
    cpu.set_register(3, output.0);
    cpu.set_register(13, stack.0);

    for command in [1_017, 1_324] {
        runtime.memory.write_u32(output, 0xaaaa_aaaa).unwrap();
        runtime.memory.write_u32(output_len, 0xbbbb_bbbb).unwrap();
        cpu.set_register(0, command);
        cpu.set_register(1, 0);
        cpu.set_register(2, 0);
        runtime
            .dispatch(38, 0, &mut cpu, &mut StubServices)
            .unwrap();
        assert_eq!(cpu.register(0), u32::MAX, "command {command}");
        assert_eq!(runtime.memory.read_u32(output).unwrap(), 0);
        assert_eq!(runtime.memory.read_u32(output_len).unwrap(), 0);

        cpu.set_register(0, command);
        cpu.set_register(1, 1);
        assert!(matches!(
            runtime.dispatch(38, 0, &mut cpu, &mut StubServices),
            Err(Error::Abi(message)) if message.contains(&format!("command {command}"))
        ));
    }
}

#[test]
fn optional_runtime_profile_query_uses_the_unavailable_fallback() {
    let mut runtime =
        ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    let stack = runtime.allocate(4, 4).unwrap();
    runtime.memory.write_u32(stack, 0).unwrap();
    let mut cpu = ArmCpu::new();
    cpu.set_register(0, 4_033);
    cpu.set_register(13, stack.0);

    runtime
        .dispatch(38, 0, &mut cpu, &mut StubServices)
        .unwrap();
    assert_eq!(cpu.register(0) as i32, -1);

    cpu.set_register(0, 4_033);
    cpu.set_register(1, 1);
    assert!(matches!(
        runtime.dispatch(38, 0, &mut cpu, &mut StubServices),
        Err(Error::Abi(message)) if message.contains("command 4033")
    ));
}

#[test]
fn platform_file_position_uses_the_legacy_bias_and_rejects_invalid_handles() {
    let mut runtime =
        ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    let mut cpu = ArmCpu::new();
    cpu.set_register(0, 1_231);
    cpu.set_register(1, 123);

    runtime
        .dispatch(37, 0, &mut cpu, &mut StubServices)
        .unwrap();
    assert_eq!(cpu.register(0), 1_456);

    cpu.set_register(0, 1_231);
    cpu.set_register(1, u32::MAX);
    runtime
        .dispatch(37, 0, &mut cpu, &mut StubServices)
        .unwrap();
    assert_eq!(cpu.register(0), u32::MAX);
}

#[test]
fn runtime_shutdown_notification_does_not_change_lifecycle_state() {
    let mut runtime =
        ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    let mut cpu = ArmCpu::new();
    cpu.set_register(0, 1_018);
    cpu.set_register(1, 0);
    cpu.set_register(2, 0xff00_0094);
    cpu.set_register(3, 0xff00_0038);

    runtime
        .dispatch(37, 0, &mut cpu, &mut StubServices)
        .unwrap();

    assert_eq!(cpu.register(0), 0);
    assert_eq!(runtime.lifecycle_request().unwrap(), None);
    assert!(runtime.pending_external_action_completions.is_empty());
    assert!(!runtime.exit_requested);

    cpu.set_register(0, 1_018);
    cpu.set_register(1, 1);
    assert!(matches!(
        runtime.dispatch(37, 0, &mut cpu, &mut StubServices),
        Err(Error::Abi(message)) if message.contains("command (1018, 1)")
    ));
}

#[test]
fn platform_runtime_profile_returns_a_stable_twelve_byte_record() {
    let mut runtime =
        ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    let output = runtime.allocate(4, 4).unwrap();
    let output_len = runtime.allocate(4, 4).unwrap();
    let stack = runtime.allocate(8, 4).unwrap();
    runtime.memory.write_u32(stack, output_len.0).unwrap();
    runtime
        .memory
        .write_u32(stack.checked_add(4).unwrap(), 0xfeed_face)
        .unwrap();
    let mut cpu = ArmCpu::new();
    cpu.set_register(0, 1_224);
    cpu.set_register(1, 0);
    cpu.set_register(2, 0);
    cpu.set_register(3, output.0);
    cpu.set_register(13, stack.0);

    runtime
        .dispatch(38, 0, &mut cpu, &mut StubServices)
        .unwrap();

    assert_eq!(cpu.register(0), 0);
    assert_eq!(
        runtime.memory.read_u32(output).unwrap(),
        PLATFORM_RUNTIME_PROFILE_DATA.0
    );
    assert_eq!(runtime.memory.read_u32(output_len).unwrap(), 12);
    assert_eq!(
        runtime
            .memory
            .read(PLATFORM_RUNTIME_PROFILE_DATA, PLATFORM_RUNTIME_PROFILE_LEN)
            .unwrap(),
        [0; PLATFORM_RUNTIME_PROFILE_LEN]
    );
    assert_eq!(
        runtime
            .memory
            .read_u32(stack.checked_add(4).unwrap())
            .unwrap(),
        0xfeed_face
    );
}

#[test]
fn headless_native_windows_use_nonzero_owned_handles() {
    let mut runtime =
        ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    load_test_module(&mut runtime);
    let mut cpu = ArmCpu::new();

    runtime
        .dispatch(78, 0, &mut cpu, &mut StubServices)
        .unwrap();
    let handle = cpu.register(0);
    assert_ne!(handle, 0);
    assert!(runtime.native_windows.contains_key(&handle));

    cpu.set_register(0, handle);
    runtime
        .dispatch(79, 0, &mut cpu, &mut StubServices)
        .unwrap();
    assert_eq!(cpu.register(0), 0);
    assert!(!runtime.native_windows.contains_key(&handle));

    cpu.set_register(0, handle);
    runtime
        .dispatch(79, 0, &mut cpu, &mut StubServices)
        .unwrap();
    assert_eq!(cpu.register(0), u32::MAX);
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
fn headless_audio_accepts_in_memory_midi_without_producing_output() {
    let mut runtime =
        ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    let midi = runtime.allocate(14, 4).unwrap();
    runtime
        .memory
        .write(midi, b"MThd\0\0\0\x06\0\0\0\x01\0\x78")
        .unwrap();
    let mut cpu = ArmCpu::new();
    cpu.set_register(0, 0);
    cpu.set_register(1, midi.0);
    cpu.set_register(2, 14);
    cpu.set_register(3, 1);

    runtime
        .dispatch(57, 0, &mut cpu, &mut StubServices)
        .unwrap();

    assert_eq!(cpu.register(0), 0);
    STUB_SOUND.with(|sound| {
        let sound = sound.borrow();
        let (sound_type, data, looped) = sound.as_ref().unwrap();
        assert_eq!(*sound_type, SoundType::Midi);
        assert_eq!(data, b"MThd\0\0\0\x06\0\0\0\x01\0\x78");
        assert!(*looped);
    });

    cpu.set_register(3, u32::MAX);
    runtime
        .dispatch(57, 0, &mut cpu, &mut StubServices)
        .unwrap();
    assert_eq!(cpu.register(0), 0);
    STUB_SOUND.with(|sound| assert!(sound.borrow().as_ref().unwrap().2));

    cpu.set_register(0, 2);
    cpu.set_register(3, 0);
    runtime
        .dispatch(57, 0, &mut cpu, &mut StubServices)
        .unwrap();
    assert_eq!(cpu.register(0), 0);
    STUB_SOUND.with(|sound| {
        let sound = sound.borrow();
        assert_eq!(sound.as_ref().unwrap().0, SoundType::Mp3);
        assert!(!sound.as_ref().unwrap().2);
    });

    runtime
        .dispatch(58, 0, &mut cpu, &mut StubServices)
        .unwrap();
    STUB_SOUND.with(|sound| assert!(sound.borrow().is_none()));
}

#[test]
fn headless_audio_rejects_unverified_request_forms() {
    let mut runtime =
        ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    let sound = runtime.allocate(4, 4).unwrap();
    runtime.memory.write(sound, b"data").unwrap();
    let mut cpu = ArmCpu::new();
    cpu.set_register(1, sound.0);
    cpu.set_register(2, 4);

    for (sound_type, looped) in [(1, 0), (3, 0), (0, 2)] {
        cpu.set_register(0, sound_type);
        cpu.set_register(3, looped);
        assert!(matches!(
            runtime.dispatch(57, 0, &mut cpu, &mut StubServices),
            Err(Error::Abi(message)) if message.contains("unsupported headless sound request")
        ));
    }
}

#[test]
fn native_file_write_rejects_invalid_arguments_before_reading_input() {
    let mut runtime =
        ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    let input = runtime.allocate(4, 1).unwrap();
    let mut cpu = ArmCpu::new();
    for (handle, source, len) in [(u32::MAX, u32::MAX, 4), (7, 0, 4), (7, input.0, u32::MAX)] {
        cpu.set_register(0, handle);
        cpu.set_register(1, source);
        cpu.set_register(2, len);

        runtime
            .dispatch(43, 0, &mut cpu, &mut StubServices)
            .unwrap();

        assert_eq!(cpu.register(0), u32::MAX);
    }
}

#[test]
fn dns_fails_deterministically_without_a_resolver_provider() {
    let mut runtime =
        ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    let hostname = runtime.allocate(16, 1).unwrap();
    runtime
        .memory
        .write(hostname, b"missing.invalid\0")
        .unwrap();
    let mut cpu = ArmCpu::new();
    cpu.set_register(0, hostname.0);

    runtime
        .dispatch(83, 0, &mut cpu, &mut StubServices)
        .unwrap();

    assert_eq!(cpu.register(0) as i32, -1);
}

#[test]
fn dns_mapping_returns_a_network_order_ipv4_address() {
    let mut runtime =
        ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    runtime.set_dns_mappings(Arc::from([DnsMapping {
        source: "spd.skymobiapp.com".into(),
        address: Ipv4Addr::new(159, 75, 119, 124),
        port: None,
    }]));
    let hostname = runtime.allocate(21, 1).unwrap();
    runtime
        .memory
        .write(hostname, b"SPD.SkyMobiApp.com.\0")
        .unwrap();
    let mut cpu = ArmCpu::new();
    cpu.set_register(0, hostname.0);
    cpu.set_register(1, 0x1000_0009);

    runtime
        .dispatch(83, 0, &mut cpu, &mut StubServices)
        .unwrap();

    assert_eq!(cpu.register(0), u32::from_be_bytes([159, 75, 119, 124]));
}

#[test]
fn endpoint_mapping_can_redirect_an_ip_and_port() {
    let mut runtime =
        ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    runtime.set_dns_mappings(Arc::from([DnsMapping {
        source: "211.155.236.18".into(),
        address: Ipv4Addr::LOCALHOST,
        port: Some(8088),
    }]));

    assert_eq!(
        runtime.route_mapped_endpoint(u32::from_be_bytes([211, 155, 236, 18]), 6009),
        (u32::from_be_bytes([127, 0, 0, 1]), 8088)
    );
}

#[test]
fn wap_gateway_routes_to_the_internal_proxy_unless_explicitly_mapped() {
    let mut runtime =
        ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    let proxy = SocketAddrV4::new(Ipv4Addr::LOCALHOST, 32_123);
    runtime.set_wap_proxy_endpoint(Some(proxy));

    assert_eq!(
        runtime.route_mapped_endpoint(u32::from_be_bytes([10, 0, 0, 172]), 80),
        (
            u32::from_be_bytes(Ipv4Addr::LOCALHOST.octets()),
            u32::from(proxy.port()),
        )
    );

    runtime.set_dns_mappings(Arc::from([DnsMapping {
        source: "10.0.0.172".into(),
        address: Ipv4Addr::new(192, 0, 2, 10),
        port: Some(8080),
    }]));
    assert_eq!(
        runtime.route_mapped_endpoint(u32::from_be_bytes([10, 0, 0, 172]), 80),
        (u32::from_be_bytes([192, 0, 2, 10]), 8080)
    );
}

#[test]
fn native_stream_socket_connects_polls_and_transfers_on_loopback() {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0_u8; 4];
        stream.read_exact(&mut request).unwrap();
        assert_eq!(&request, b"ping");
        stream.write_all(b"pong").unwrap();
    });
    let mut runtime =
        ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    let mut cpu = ArmCpu::new();
    cpu.set_register(0, 0x1000_0009);

    runtime
        .dispatch(81, 0, &mut cpu, &mut StubServices)
        .unwrap();

    assert_eq!(cpu.register(0), 0);

    cpu.set_register(0, 0);
    cpu.set_register(1, 0);
    runtime
        .dispatch(84, 0, &mut cpu, &mut StubServices)
        .unwrap();
    let handle = cpu.register(0);
    assert_ne!(handle as i32, -1);

    cpu.set_register(0, handle);
    cpu.set_register(1, u32::from_be_bytes(Ipv4Addr::LOCALHOST.octets()));
    cpu.set_register(2, u32::from(port));
    cpu.set_register(3, 1);
    runtime
        .dispatch(85, 0, &mut cpu, &mut StubServices)
        .unwrap();
    assert_eq!(cpu.register(0), 2);

    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        cpu.set_register(0, 1_001);
        cpu.set_register(1, handle);
        runtime
            .dispatch(37, 0, &mut cpu, &mut StubServices)
            .unwrap();
        if cpu.register(0) == 0 {
            break;
        }
        assert_eq!(cpu.register(0), 1);
        assert!(Instant::now() < deadline, "loopback connect timed out");
        thread::sleep(Duration::from_millis(1));
    }

    let payload = runtime.allocate(4, 1).unwrap();
    runtime.memory.write(payload, b"ping").unwrap();
    cpu.set_register(0, handle);
    cpu.set_register(1, payload.0);
    cpu.set_register(2, 4);
    runtime
        .dispatch(89, 0, &mut cpu, &mut StubServices)
        .unwrap();
    assert_eq!(cpu.register(0), 4);

    let output = runtime.allocate(4, 1).unwrap();
    loop {
        cpu.set_register(0, handle);
        cpu.set_register(1, output.0);
        cpu.set_register(2, 4);
        runtime
            .dispatch(87, 0, &mut cpu, &mut StubServices)
            .unwrap();
        if cpu.register(0) == 4 {
            break;
        }
        assert_eq!(cpu.register(0) as i32, -1);
        assert!(Instant::now() < deadline, "loopback receive timed out");
        thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(runtime.memory.read(output, 4).unwrap(), b"pong");

    cpu.set_register(0, handle);
    runtime
        .dispatch(86, 0, &mut cpu, &mut StubServices)
        .unwrap();
    assert_eq!(cpu.register(0), 0);
    server.join().unwrap();

    runtime
        .dispatch(82, 0, &mut cpu, &mut StubServices)
        .unwrap();
    assert_eq!(cpu.register(0), 0);
}

#[test]
fn http_host_mapping_reroutes_a_proxy_connection_after_a_split_header() {
    let proxy = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let proxy_port = proxy.local_addr().unwrap().port();
    let proxy_server = thread::spawn(move || {
        let (mut stream, _) = proxy.accept().unwrap();
        let mut bytes = Vec::new();
        stream.read_to_end(&mut bytes).unwrap();
        bytes
    });
    let target = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let target_port = target.local_addr().unwrap().port();
    let expected = b"POST /resource HTTP/1.1\r\nHost: service.test\r\nContent-Length: 0\r\n\r\n";
    let target_server = thread::spawn(move || {
        let (mut stream, _) = target.accept().unwrap();
        let mut request = vec![0; expected.len()];
        stream.read_exact(&mut request).unwrap();
        stream.write_all(b"ok").unwrap();
        request
    });

    let mut runtime =
        ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    runtime.set_dns_mappings(Arc::from([DnsMapping {
        source: "service.test".into(),
        address: Ipv4Addr::LOCALHOST,
        port: Some(target_port),
    }]));
    let handle = runtime.allocate_native_socket_handle().unwrap().unwrap();
    assert_eq!(
        runtime.connect_native_socket(
            handle,
            u32::from_be_bytes(Ipv4Addr::LOCALHOST.octets()),
            u32::from(proxy_port),
            1,
        ),
        2
    );
    let deadline = Instant::now() + Duration::from_secs(2);
    while runtime.native_socket_state(handle) == 1 {
        assert!(Instant::now() < deadline, "proxy connect timed out");
        thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(runtime.native_socket_state(handle), 0);

    let split = expected
        .windows(b"Host".len())
        .position(|window| window == b"Host")
        .unwrap();
    assert_eq!(
        runtime.send_native_socket(handle, &expected[..split]),
        Some(split)
    );
    assert_eq!(
        runtime.send_native_socket(handle, &expected[split..]),
        Some(expected.len() - split)
    );

    let response = loop {
        if let Some(response) = runtime.receive_native_socket(handle, 2) {
            break response;
        }
        assert!(Instant::now() < deadline, "mapped response timed out");
        thread::sleep(Duration::from_millis(1));
    };
    assert_eq!(response, b"ok");
    assert_eq!(target_server.join().unwrap(), expected);
    assert!(proxy_server.join().unwrap().is_empty());
}

#[test]
fn failed_http_host_mapping_does_not_fall_back_to_the_proxy() {
    let proxy = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let proxy_port = proxy.local_addr().unwrap().port();
    let proxy_server = thread::spawn(move || {
        let (mut stream, _) = proxy.accept().unwrap();
        let mut bytes = Vec::new();
        stream.read_to_end(&mut bytes).unwrap();
        bytes
    });
    let unavailable = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let unavailable_port = unavailable.local_addr().unwrap().port();
    drop(unavailable);

    let mut runtime =
        ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    runtime.set_dns_mappings(Arc::from([DnsMapping {
        source: "unavailable.test".into(),
        address: Ipv4Addr::LOCALHOST,
        port: Some(unavailable_port),
    }]));
    let handle = runtime.allocate_native_socket_handle().unwrap().unwrap();
    assert_eq!(
        runtime.connect_native_socket(
            handle,
            u32::from_be_bytes(Ipv4Addr::LOCALHOST.octets()),
            u32::from(proxy_port),
            1,
        ),
        2
    );
    let deadline = Instant::now() + Duration::from_secs(2);
    while runtime.native_socket_state(handle) == 1 {
        assert!(Instant::now() < deadline, "proxy connect timed out");
        thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(runtime.native_socket_state(handle), 0);

    let request = b"POST /pay HTTP/1.1\r\nHost: unavailable.test\r\nContent-Length: 0\r\n\r\n";
    assert_eq!(runtime.send_native_socket(handle, request), None);
    assert_eq!(runtime.native_socket_state(handle), -1);
    assert_eq!(runtime.receive_native_socket(handle, 64), None);
    assert!(proxy_server.join().unwrap().is_empty());
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
fn platform_rx_initialization_accepts_the_default_mode() {
    let mut runtime =
        ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    let mut cpu = ArmCpu::new();
    for mode in 0..=1 {
        cpu.set_register(0, 1_214);
        cpu.set_register(1, mode);
        runtime
            .dispatch(37, 0, &mut cpu, &mut StubServices)
            .unwrap();
        assert_eq!(cpu.register(0), 0, "mode {mode}");
    }
}

#[test]
fn platform_motion_initialization_uses_the_silent_headless_provider() {
    let mut runtime =
        ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    let mut cpu = ArmCpu::new();
    cpu.set_register(0, 1_206);
    cpu.set_register(1, 0);

    runtime
        .dispatch(37, 0, &mut cpu, &mut StubServices)
        .unwrap();

    assert_eq!(cpu.register(0), 0);

    cpu.set_register(0, 1_206);
    cpu.set_register(1, 1);
    assert!(matches!(
        runtime.dispatch(37, 0, &mut cpu, &mut StubServices),
        Err(Error::Abi(message)) if message.contains("command (1206, 1)")
    ));

    for (command, argument) in [(4_002, 0), (4_005, 2)] {
        cpu.set_register(0, command);
        cpu.set_register(1, argument);
        runtime
            .dispatch(37, 0, &mut cpu, &mut StubServices)
            .unwrap();
        assert_eq!(cpu.register(0), 0, "command {command}, argument {argument}");
    }

    cpu.set_register(0, 4_005);
    cpu.set_register(1, 1);
    assert!(matches!(
        runtime.dispatch(37, 0, &mut cpu, &mut StubServices),
        Err(Error::Abi(message)) if message.contains("command (4005, 1)")
    ));
}

#[test]
fn platform_audio_volume_accepts_supported_levels() {
    let mut runtime =
        ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    let mut cpu = ArmCpu::new();

    for volume in 0..=5 {
        cpu.set_register(0, 1_302);
        cpu.set_register(1, volume);
        runtime
            .dispatch(37, 0, &mut cpu, &mut StubServices)
            .unwrap();
        assert_eq!(cpu.register(0), 0, "volume {volume}");
    }

    cpu.set_register(0, 1_302);
    cpu.set_register(1, 6);
    assert!(matches!(
        runtime.dispatch(37, 0, &mut cpu, &mut StubServices),
        Err(Error::Abi(message)) if message.contains("command (1302, 6)")
    ));
}

#[test]
fn platform_sms_uses_a_bounded_headless_sink() {
    let mut runtime =
        ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    load_test_module(&mut runtime);
    let mut register = ArmCpu::new();
    register.set_register(0, runtime.modules[0].base.0 + 8);
    register.set_register(1, 20);
    runtime
        .dispatch(25, 0, &mut register, &mut StubServices)
        .unwrap();
    let number = runtime.allocate(9, 1).unwrap();
    let message = runtime.allocate(24, 1).unwrap();
    runtime.memory.write(number, b"10668001\0").unwrap();
    runtime
        .memory
        .write(message, b"bounded-sms-payload-data")
        .unwrap();

    let mut cpu = ArmCpu::new();
    cpu.set_register(0, number.0);
    cpu.set_register(1, message.0);
    cpu.set_register(2, 24);
    runtime
        .dispatch(59, 0, &mut cpu, &mut StubServices)
        .unwrap();
    assert_eq!(cpu.register(0), 0);
    let completion = runtime.pending_sms_results.front().unwrap();
    assert_eq!(completion.owner_generation, runtime.modules[0].generation);
    let helper = runtime.modules[0].helper.unwrap();
    assert_eq!(completion.helper.module, helper.module);
    assert_eq!(completion.helper.address, helper.address);
    assert_eq!(completion.result, 0);

    assert!(
        runtime
            .dispatch_pending_platform_event(&mut StubServices)
            .unwrap()
    );
    assert!(runtime.pending_sms_results.is_empty());
    assert!(
        !runtime
            .dispatch_pending_platform_event(&mut StubServices)
            .unwrap()
    );

    cpu.set_register(0, number.0);
    cpu.set_register(1, message.0);
    cpu.set_register(2, 0);
    runtime
        .dispatch(59, 0, &mut cpu, &mut StubServices)
        .unwrap();
    assert_eq!(cpu.register(0) as i32, -1);
    assert!(runtime.pending_sms_results.is_empty());
}

#[test]
fn platform_storage_info_reports_sufficient_available_space() {
    let mut runtime =
        ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    let drive = runtime.allocate(3, 1).unwrap();
    let output = runtime.allocate(4, 4).unwrap();
    let output_len = runtime.allocate(4, 4).unwrap();
    let stack = runtime.allocate(4, 4).unwrap();
    runtime.memory.write_u32(stack, output_len.0).unwrap();

    let mut cpu = ArmCpu::new();
    cpu.set_register(1, drive.0);
    cpu.set_register(3, output.0);
    cpu.set_register(13, stack.0);

    for selector in [
        &b"C"[..],
        &b"C\0"[..],
        &b"C:"[..],
        &b"C:\0"[..],
        &b"X\0"[..],
        &b"Y:"[..],
        &b"Z:\0"[..],
    ] {
        runtime.memory.write(drive, selector).unwrap();
        cpu.set_register(0, 1_305);
        cpu.set_register(2, selector.len() as u32);
        runtime
            .dispatch(38, 0, &mut cpu, &mut StubServices)
            .unwrap();
        assert_eq!(cpu.register(0), 0, "selector {selector:?}");
    }

    let info = GuestAddr(runtime.memory.read_u32(output).unwrap());
    let geometry = (0..4)
        .map(|index| {
            runtime
                .memory
                .read_u32(info.checked_add(index * 4).unwrap())
                .unwrap()
        })
        .collect::<Vec<_>>();
    assert_eq!(runtime.memory.read_u32(output_len).unwrap(), 16);
    assert_eq!(
        geometry,
        [
            PLATFORM_STORAGE_TOTAL_BLOCKS,
            PLATFORM_STORAGE_BLOCK_SIZE,
            PLATFORM_STORAGE_BLOCK_SIZE,
            PLATFORM_STORAGE_AVAILABLE_BLOCKS,
        ]
    );
    let total_bytes = u64::from(geometry[0]) * u64::from(geometry[1]);
    let available_bytes = u64::from(geometry[2]) * u64::from(geometry[3]);
    assert_eq!(total_bytes, 256 * 1024 * 1024);
    assert_eq!(available_bytes, 128 * 1024 * 1024);
    assert!(available_bytes <= total_bytes);

    runtime.memory.write(drive, b"D\0").unwrap();
    cpu.set_register(0, 1_305);
    cpu.set_register(2, 2);
    runtime
        .dispatch(38, 0, &mut cpu, &mut StubServices)
        .unwrap();
    assert_eq!(cpu.register(0) as i32, -1);
}

#[test]
fn sprintf_honors_numeric_width_and_zero_padding() {
    let mut runtime =
        ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    let destination = runtime.allocate(32, 1).unwrap();
    let format = runtime.allocate(16, 1).unwrap();
    runtime.memory.write(format, b"%04d%02d%02d\0").unwrap();
    let stack = runtime.allocate(4, 4).unwrap();
    runtime.memory.write_u32(stack, 20).unwrap();
    let mut cpu = ArmCpu::new();
    cpu.set_register(0, destination.0);
    cpu.set_register(1, format.0);
    cpu.set_register(2, 2012);
    cpu.set_register(3, 6);
    cpu.set_register(13, stack.0);

    runtime
        .dispatch(17, 0, &mut cpu, &mut StubServices)
        .unwrap();

    assert_eq!(cpu.register(0), 8);
    assert_eq!(runtime.memory.read(destination, 9).unwrap(), b"20120620\0");
}

#[test]
fn sprintf_combines_flags_width_and_precision() {
    let mut runtime =
        ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    let destination = runtime.allocate(64, 1).unwrap();
    let format = runtime.allocate(40, 1).unwrap();
    runtime
        .memory
        .write(format, b"%+05d|% 4d|%-4u|%#06x|%08.5d\0")
        .unwrap();
    let stack = runtime.allocate(12, 4).unwrap();
    runtime.memory.write_u32(stack, 3).unwrap();
    runtime
        .memory
        .write_u32(stack.checked_add(4).unwrap(), 0x2a)
        .unwrap();
    runtime
        .memory
        .write_u32(stack.checked_add(8).unwrap(), 12)
        .unwrap();
    let mut cpu = ArmCpu::new();
    cpu.set_register(0, destination.0);
    cpu.set_register(1, format.0);
    cpu.set_register(2, (-12_i32) as u32);
    cpu.set_register(3, 7);
    cpu.set_register(13, stack.0);

    runtime
        .dispatch(17, 0, &mut cpu, &mut StubServices)
        .unwrap();

    let expected = b"-0012|   7|3   |0x002a|   00012\0";
    assert_eq!(cpu.register(0), (expected.len() - 1) as u32);
    assert_eq!(
        runtime.memory.read(destination, expected.len()).unwrap(),
        expected
    );
}

#[test]
fn sprintf_sdk_m_is_literal_and_does_not_consume_an_argument() {
    let mut runtime =
        ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    let destination = runtime.allocate(32, 1).unwrap();
    let format = runtime.allocate(24, 1).unwrap();
    let suffix = runtime.allocate(8, 1).unwrap();
    runtime
        .memory
        .write(format, b"%mexit%s|%m%d.jpg\0")
        .unwrap();
    runtime.memory.write(suffix, b".slg\0").unwrap();
    let mut cpu = ArmCpu::new();
    cpu.set_register(0, destination.0);
    cpu.set_register(1, format.0);
    cpu.set_register(2, suffix.0);
    cpu.set_register(3, 7);

    runtime
        .dispatch(17, 0, &mut cpu, &mut StubServices)
        .unwrap();

    let expected = b"mexit.slg|m7.jpg\0";
    assert_eq!(cpu.register(0), (expected.len() - 1) as u32);
    assert_eq!(
        runtime.memory.read(destination, expected.len()).unwrap(),
        expected
    );
}

#[test]
fn sprintf_sdk_m_rejects_unverified_modifiers_without_writing_output() {
    for unverified_format in [b"%1m\0".as_slice(), b"%-m\0", b"%.m\0", b"%lm\0"] {
        let mut runtime =
            ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
        let destination = runtime.allocate(9, 1).unwrap();
        let format = runtime.allocate(unverified_format.len(), 1).unwrap();
        runtime.memory.write(destination, b"unchanged").unwrap();
        runtime.memory.write(format, unverified_format).unwrap();
        let mut cpu = ArmCpu::new();
        cpu.set_register(0, destination.0);
        cpu.set_register(1, format.0);

        let error = runtime
            .dispatch(17, 0, &mut cpu, &mut StubServices)
            .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("unsupported sprintf modifiers for '%m'"),
            "format {unverified_format:?}: {error}"
        );
        assert_eq!(runtime.memory.read(destination, 9).unwrap(), b"unchanged");
    }
}

#[test]
fn platform_storage_drive_query_resolves_supported_volumes() {
    let mut runtime =
        ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    let volume = runtime.allocate(1, 1).unwrap();
    let output = runtime.allocate(4, 4).unwrap();
    let output_len = runtime.allocate(4, 4).unwrap();
    let stack = runtime.allocate(4, 4).unwrap();
    runtime.memory.write_u32(stack, output_len.0).unwrap();

    let mut cpu = ArmCpu::new();
    cpu.set_register(1, volume.0);
    cpu.set_register(2, 1);
    cpu.set_register(3, output.0);
    cpu.set_register(13, stack.0);

    for supported_volume in *b"CXYZ" {
        runtime.memory.write(volume, &[supported_volume]).unwrap();
        cpu.set_register(0, 1_204);
        runtime
            .dispatch(38, 0, &mut cpu, &mut StubServices)
            .unwrap();

        let drive = GuestAddr(runtime.memory.read_u32(output).unwrap());
        assert_eq!(cpu.register(0), 0, "volume {}", supported_volume as char);
        assert_eq!(
            runtime.memory.read_u32(output_len).unwrap(),
            PLATFORM_STORAGE_DRIVE_LEN as u32
        );
        assert_eq!(
            runtime
                .memory
                .read(drive, PLATFORM_STORAGE_DRIVE_LEN)
                .unwrap(),
            PLATFORM_STORAGE_DRIVE
        );
    }

    runtime.memory.write(volume, b"D").unwrap();
    cpu.set_register(0, 1_204);
    runtime
        .dispatch(38, 0, &mut cpu, &mut StubServices)
        .unwrap();
    assert_eq!(cpu.register(0) as i32, -1);

    runtime.memory.write(volume, b"C").unwrap();
    runtime.memory.write_u32(stack, 0).unwrap();
    cpu.set_register(0, 1_204);
    cpu.set_register(3, 0);
    runtime
        .dispatch(38, 0, &mut cpu, &mut StubServices)
        .unwrap();
    assert_eq!(cpu.register(0) as i32, -1);
}

#[test]
fn text_drawing_decodes_legacy_gbk_and_uses_the_font_argument() {
    let mut runtime =
        ExtRuntime::new(24, 16, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    let text = runtime.allocate(5, 1).unwrap();
    runtime
        .memory
        .write(text, &[0xc8, 0xb7, 0xb6, 0xa8, 0])
        .unwrap();
    let stack = runtime.allocate(16, 4).unwrap();
    runtime
        .memory
        .write_u32(stack.checked_add(8).unwrap(), 0)
        .unwrap();
    runtime
        .memory
        .write_u32(stack.checked_add(12).unwrap(), 1)
        .unwrap();

    let mut cpu = ArmCpu::new();
    cpu.set_register(0, text.0);
    cpu.set_register(3, 255);
    cpu.set_register(13, stack.0);

    runtime
        .dispatch(123, 0, &mut cpu, &mut StubServices)
        .unwrap();

    assert_eq!(cpu.register(0), 0);
    assert_eq!(
        runtime
            .memory
            .read_u16(runtime.screen_address(7, 0, 24).unwrap())
            .unwrap(),
        0xf800
    );
    assert_eq!(
        runtime
            .memory
            .read_u16(runtime.screen_address(16, 0, 24).unwrap())
            .unwrap(),
        0xf800
    );
}

#[test]
fn text_drawing_reads_ucs2_be_when_requested() {
    let mut runtime =
        ExtRuntime::new(24, 16, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    let text = runtime.allocate(6, 2).unwrap();
    runtime
        .memory
        .write(text, &[0x78, 0x6e, 0x5b, 0x9a, 0, 0])
        .unwrap();
    let stack = runtime.allocate(16, 4).unwrap();
    runtime
        .memory
        .write_u32(stack.checked_add(8).unwrap(), 1)
        .unwrap();
    runtime
        .memory
        .write_u32(stack.checked_add(12).unwrap(), 2)
        .unwrap();

    let mut cpu = ArmCpu::new();
    cpu.set_register(0, text.0);
    cpu.set_register(3, 255);
    cpu.set_register(13, stack.0);

    runtime
        .dispatch(123, 0, &mut cpu, &mut StubServices)
        .unwrap();

    assert_eq!(cpu.register(0), 0);
    assert_eq!(
        runtime
            .memory
            .read_u16(runtime.screen_address(7, 0, 24).unwrap())
            .unwrap(),
        0xf800
    );
    assert_eq!(
        runtime
            .memory
            .read_u16(runtime.screen_address(16, 0, 24).unwrap())
            .unwrap(),
        0xf800
    );
}

#[test]
fn text_drawing_rejects_invalid_font_arguments() {
    let mut runtime =
        ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    let text = runtime.allocate(4, 2).unwrap();
    runtime.memory.write(text, &[0, b'A', 0, 0]).unwrap();
    let stack = runtime.allocate(16, 4).unwrap();
    let mut cpu = ArmCpu::new();
    cpu.set_register(0, text.0);
    cpu.set_register(13, stack.0);

    runtime
        .memory
        .write_u32(stack.checked_add(8).unwrap(), 1)
        .unwrap();
    runtime
        .memory
        .write_u32(stack.checked_add(12).unwrap(), 3)
        .unwrap();
    assert!(matches!(
        runtime.dispatch(123, 0, &mut cpu, &mut StubServices),
        Err(Error::Abi(message))
            if message == "unsupported text drawing font 3 called by module 0"
    ));
}

#[test]
fn text_drawing_treats_a_null_text_pointer_as_empty_text() {
    let mut runtime =
        ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    let stack = runtime.allocate(16, 4).unwrap();
    let mut cpu = ArmCpu::new();
    cpu.set_register(0, 0);
    cpu.set_register(13, stack.0);

    runtime
        .dispatch(123, 0, &mut cpu, &mut StubServices)
        .unwrap();

    assert_eq!(cpu.register(0), 0);
}

#[test]
fn legacy_string_conversion_returns_module_owned_ucs2_be() {
    let mut runtime =
        ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    load_test_module(&mut runtime);
    let input = runtime.allocate(6, 1).unwrap();
    let error_output = runtime.allocate(4, 4).unwrap();
    let size_output = runtime.allocate(4, 4).unwrap();
    runtime
        .memory
        .write(input, &[b'A', 0xd6, 0xd0, 0x80, 0, 0])
        .unwrap();
    runtime.memory.write_u32(error_output, 0).unwrap();
    runtime.memory.write_u32(size_output, 0).unwrap();
    let mut cpu = ArmCpu::new();
    cpu.set_register(0, input.0);
    cpu.set_register(1, error_output.0);
    cpu.set_register(2, size_output.0);

    runtime
        .dispatch(132, 0, &mut cpu, &mut StubServices)
        .unwrap();

    let output = GuestAddr(cpu.register(0));
    assert_ne!(output.0, 0);
    assert_eq!(runtime.memory.read_u32(error_output).unwrap(), u32::MAX);
    assert_eq!(runtime.memory.read_u32(size_output).unwrap(), 8);
    assert_eq!(
        runtime.memory.read(output, 8).unwrap(),
        [0x00, 0x41, 0x4e, 0x2d, 0x20, 0xac, 0x00, 0x00]
    );
    assert_eq!(
        runtime.guest_allocation_owners.get(&output.0),
        Some(&runtime.modules[0].generation)
    );
}

#[test]
fn legacy_string_conversion_handles_empty_invalid_and_optional_outputs() {
    let mut runtime =
        ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    load_test_module(&mut runtime);
    let input = runtime.allocate(2, 1).unwrap();
    let mut cpu = ArmCpu::new();
    cpu.set_register(0, input.0);

    runtime.memory.write(input, &[0, 0]).unwrap();
    runtime
        .dispatch(132, 0, &mut cpu, &mut StubServices)
        .unwrap();
    assert_eq!(
        runtime.memory.read(GuestAddr(cpu.register(0)), 2).unwrap(),
        [0, 0]
    );

    runtime.memory.write(input, &[0x81, 0]).unwrap();
    cpu.set_register(0, input.0);
    runtime
        .dispatch(132, 0, &mut cpu, &mut StubServices)
        .unwrap();
    assert_eq!(
        runtime.memory.read(GuestAddr(cpu.register(0)), 4).unwrap(),
        [0xff, 0xfd, 0, 0]
    );

    cpu.set_register(0, 0);
    runtime
        .dispatch(132, 0, &mut cpu, &mut StubServices)
        .unwrap();
    assert_eq!(cpu.register(0), 0);
}

#[test]
fn buffered_ucs2_conversion_uses_the_caller_owned_legacy_output() {
    let mut runtime =
        ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    let input = runtime.allocate(4, 2).unwrap();
    runtime
        .memory
        .write(input, &[0x00, b'A', 0x4e, 0x2d])
        .unwrap();
    let destination = runtime.allocate(16, 2).unwrap();
    let output_field = runtime.allocate(4, 4).unwrap();
    runtime
        .memory
        .write_u32(output_field, destination.0)
        .unwrap();
    let output_len = runtime.allocate(4, 4).unwrap();
    let stack = runtime.allocate(4, 4).unwrap();
    runtime.memory.write_u32(stack, output_len.0).unwrap();
    let mut cpu = ArmCpu::new();
    cpu.set_register(0, 1_207);
    cpu.set_register(1, input.0);
    cpu.set_register(2, 4);
    cpu.set_register(3, output_field.0);
    cpu.set_register(13, stack.0);

    runtime
        .dispatch(38, 0, &mut cpu, &mut StubServices)
        .unwrap();

    assert_eq!(cpu.register(0), 0);
    assert_eq!(
        runtime.memory.read_u32(output_field).unwrap(),
        destination.0
    );
    assert_eq!(runtime.memory.read_u32(output_len).unwrap(), 3);
    assert_eq!(
        runtime.memory.read(destination, 4).unwrap(),
        [0x41, 0xd6, 0xd0, 0x00]
    );

    runtime.memory.write_u32(output_len, 99).unwrap();
    cpu.set_register(0, 1_207);
    cpu.set_register(2, 3);
    runtime
        .dispatch(38, 0, &mut cpu, &mut StubServices)
        .unwrap();
    assert_eq!(cpu.register(0), u32::MAX);
    assert_eq!(runtime.memory.read_u32(output_len).unwrap(), 0);

    runtime.memory.write_u32(output_field, 0).unwrap();
    runtime.memory.write_u32(output_len, 99).unwrap();
    cpu.set_register(0, 1_207);
    cpu.set_register(2, 4);
    runtime
        .dispatch(38, 0, &mut cpu, &mut StubServices)
        .unwrap();
    assert_eq!(cpu.register(0), u32::MAX);
    assert_eq!(runtime.memory.read_u32(output_len).unwrap(), 0);
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
    load_test_module(&mut runtime);
    let output = runtime.allocate(4, 4).unwrap();
    let output_len = runtime.allocate(4, 4).unwrap();
    let stack = runtime.allocate(4, 4).unwrap();
    runtime.memory.write_u32(stack, output_len.0).unwrap();
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
    assert_eq!(arena, PLATFORM_MEMORY_BASE);
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
    assert_eq!(
        runtime.memory.read_u32(data_slot_address(111)).unwrap(),
        free_before
    );
    assert!(runtime.memory.read(arena, 32).is_err());
    cpu.set_register(0, 1_015);
    assert!(matches!(
        runtime.dispatch(38, 0, &mut cpu, &mut StubServices),
        Err(Error::Abi(message)) if message.contains("unknown arena")
    ));
}

#[test]
fn module_cannot_register_another_modules_platform_arena_as_executable() {
    let mut runtime =
        ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    load_test_module(&mut runtime);
    load_test_module(&mut runtime);
    let output = runtime.allocate(4, 4).unwrap();
    let output_len = runtime.allocate(4, 4).unwrap();
    let stack = runtime.allocate(4, 4).unwrap();
    runtime.memory.write_u32(stack, output_len.0).unwrap();
    let mut cpu = ArmCpu::new();
    cpu.set_register(0, 1_014);
    cpu.set_register(2, 32);
    cpu.set_register(3, output.0);
    cpu.set_register(13, stack.0);
    runtime
        .dispatch(38, 0, &mut cpu, &mut StubServices)
        .unwrap();
    let arena = GuestAddr(runtime.memory.read_u32(output).unwrap());
    runtime.memory.write(arena, &[0x70, 0x47]).unwrap();

    cpu.set_register(0, 0);
    cpu.set_register(1, 9);
    cpu.set_register(2, arena.0);
    cpu.set_register(3, 2);
    assert!(matches!(
        runtime.dispatch(131, 1, &mut cpu, &mut StubServices),
        Err(Error::Abi(message)) if message.contains("belongs to another module")
    ));
    assert!(runtime.modules[1].dynamic_executable_ranges.is_empty());
    assert!(runtime.memory.fetch_u16(arena).is_err());
}

#[test]
fn module_merges_overlapping_dynamic_ranges_inside_one_owned_allocation() {
    let mut runtime =
        ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    load_test_module(&mut runtime);
    let output = runtime.allocate(4, 4).unwrap();
    let output_len = runtime.allocate(4, 4).unwrap();
    let stack = runtime.allocate(4, 4).unwrap();
    runtime.memory.write_u32(stack, output_len.0).unwrap();
    let mut cpu = ArmCpu::new();
    cpu.set_register(0, 1_014);
    cpu.set_register(2, 32);
    cpu.set_register(3, output.0);
    cpu.set_register(13, stack.0);
    runtime
        .dispatch(38, 0, &mut cpu, &mut StubServices)
        .unwrap();
    let arena = GuestAddr(runtime.memory.read_u32(output).unwrap());

    cpu.set_register(0, 0);
    cpu.set_register(1, 9);
    cpu.set_register(2, arena.0 + 4);
    cpu.set_register(3, 16);
    runtime
        .dispatch(131, 0, &mut cpu, &mut StubServices)
        .unwrap();

    cpu.set_register(0, 0);
    cpu.set_register(2, arena.0 + 8);
    cpu.set_register(3, 8);
    runtime
        .dispatch(131, 0, &mut cpu, &mut StubServices)
        .unwrap();
    assert_eq!(cpu.register(0), 0);
    assert_eq!(
        runtime.modules[0].dynamic_executable_ranges,
        [Some(ExecutableRange {
            base: GuestAddr(arena.0 + 4),
            len: 16,
        })]
    );

    cpu.set_register(0, 0);
    cpu.set_register(2, arena.0 + 16);
    cpu.set_register(3, 8);
    runtime
        .dispatch(131, 0, &mut cpu, &mut StubServices)
        .unwrap();
    assert_eq!(
        runtime.modules[0].dynamic_executable_ranges,
        [Some(ExecutableRange {
            base: GuestAddr(arena.0 + 4),
            len: 20,
        })]
    );

    cpu.set_register(0, 0);
    cpu.set_register(2, arena.0);
    cpu.set_register(3, 8);
    runtime
        .dispatch(131, 0, &mut cpu, &mut StubServices)
        .unwrap();
    assert_eq!(
        runtime.modules[0].dynamic_executable_ranges,
        [Some(ExecutableRange {
            base: arena,
            len: 24,
        })]
    );
}

#[test]
fn module_preserves_existing_image_identities_when_registration_bridges_them() {
    let mut runtime =
        ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    load_test_module(&mut runtime);
    let allocation = runtime
        .allocate_guest_block_for_module(64, 0)
        .unwrap()
        .unwrap();
    let mut cpu = ArmCpu::new();
    cpu.set_register(0, 0);
    cpu.set_register(1, 9);

    for (offset, len) in [(0, 8), (16, 8)] {
        cpu.set_register(2, allocation.0 + offset);
        cpu.set_register(3, len);
        runtime
            .dispatch(131, 0, &mut cpu, &mut StubServices)
            .unwrap();
    }
    assert_eq!(
        runtime.modules[0].executable_image(allocation.0),
        Some((ExecutableImage::Dynamic(0), 0))
    );
    assert_eq!(
        runtime.modules[0].executable_image(allocation.0 + 16),
        Some((ExecutableImage::Dynamic(1), 0))
    );

    cpu.set_register(2, allocation.0);
    cpu.set_register(3, 24);
    runtime
        .dispatch(131, 0, &mut cpu, &mut StubServices)
        .unwrap();

    assert_eq!(
        runtime.modules[0].executable_image(allocation.0),
        Some((ExecutableImage::Dynamic(0), 0))
    );
    assert_eq!(
        runtime.modules[0].executable_image(allocation.0 + 8),
        Some((ExecutableImage::Dynamic(2), 0))
    );
    assert_eq!(
        runtime.modules[0].executable_image(allocation.0 + 16),
        Some((ExecutableImage::Dynamic(1), 0))
    );
    assert_eq!(
        runtime
            .memory
            .fetch_u32(allocation.checked_add(8).unwrap())
            .unwrap(),
        0
    );
}
