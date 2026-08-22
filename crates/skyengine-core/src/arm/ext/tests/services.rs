use std::{
    io::{Read, Write},
    net::{Ipv4Addr, TcpListener},
    thread,
    time::{Duration, Instant},
};

use super::*;

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
fn user_info_reports_unavailable_without_mutating_the_output() {
    let mut runtime =
        ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    let output = runtime.allocate(64, 4).unwrap();
    runtime.memory.write(output, &[0xaa; 64]).unwrap();
    let mut cpu = ArmCpu::new();
    cpu.set_register(0, output.0);

    runtime
        .dispatch(35, 0, &mut cpu, &mut StubServices)
        .unwrap();

    assert_eq!(cpu.register(0) as i32, -1);
    assert_eq!(runtime.memory.read(output, 64).unwrap(), vec![0xaa; 64]);
}

#[test]
fn mtk_user_info_returns_the_deterministic_virtual_device_profile() {
    let mut runtime =
        ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    runtime
        .set_device_info_profile(DeviceInfoProfile::DeterministicMtk)
        .unwrap();
    let output = runtime.allocate(PLATFORM_USER_INFO_LEN, 4).unwrap();
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
        runtime
            .memory
            .read_u32(output.checked_add(48).unwrap())
            .unwrap(),
        PLATFORM_USER_INFO_VERSION
    );

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
        .set_device_info_profile(DeviceInfoProfile::DeterministicMtk)
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
fn native_file_write_rejects_a_negative_handle_before_reading_input() {
    let mut runtime =
        ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    let mut cpu = ArmCpu::new();
    cpu.set_register(0, u32::MAX);
    cpu.set_register(1, u32::MAX);
    cpu.set_register(2, u32::MAX);

    runtime
        .dispatch(43, 0, &mut cpu, &mut StubServices)
        .unwrap();

    assert_eq!(cpu.register(0), u32::MAX);
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
    cpu.set_register(0, 1_214);

    runtime
        .dispatch(37, 0, &mut cpu, &mut StubServices)
        .unwrap();

    assert_eq!(cpu.register(0), 0);
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
fn platform_storage_info_reports_sufficient_available_space() {
    let mut runtime =
        ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    let drive = runtime.allocate(1, 1).unwrap();
    runtime.memory.write(drive, b"C").unwrap();
    let output = runtime.allocate(4, 4).unwrap();
    let output_len = runtime.allocate(4, 4).unwrap();
    let stack = runtime.allocate(4, 4).unwrap();
    runtime.memory.write_u32(stack, output_len.0).unwrap();

    let mut cpu = ArmCpu::new();
    cpu.set_register(0, 1_305);
    cpu.set_register(1, drive.0);
    cpu.set_register(2, 1);
    cpu.set_register(3, output.0);
    cpu.set_register(13, stack.0);
    runtime
        .dispatch(38, 0, &mut cpu, &mut StubServices)
        .unwrap();

    let info = GuestAddr(runtime.memory.read_u32(output).unwrap());
    let block_size = runtime
        .memory
        .read_u32(info.checked_add(8).unwrap())
        .unwrap();
    let available_blocks = runtime
        .memory
        .read_u32(info.checked_add(12).unwrap())
        .unwrap();
    assert_eq!(cpu.register(0), 0);
    assert_eq!(runtime.memory.read_u32(output_len).unwrap(), 16);
    assert_eq!(block_size, PLATFORM_STORAGE_BLOCK_SIZE);
    assert_eq!(available_blocks, PLATFORM_STORAGE_AVAILABLE_BLOCKS);
    assert!(u64::from(block_size) * u64::from(available_blocks) / 1024 > 2048);
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

    for supported_volume in [b'Y', b'Z', b'C'] {
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
            b"C:/mythroad/"
        );
    }

    runtime.memory.write(volume, b"X").unwrap();
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
fn text_drawing_accepts_the_baseline_wide_text_flags() {
    let mut runtime =
        ExtRuntime::new(8, 8, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();
    let text = runtime.allocate(4, 2).unwrap();
    runtime.memory.write(text, &[0, b'A', 0, 0]).unwrap();
    let stack = runtime.allocate(16, 4).unwrap();

    let mut cpu = ArmCpu::new();
    cpu.set_register(13, stack.0);
    for flags in 0..=2 {
        runtime
            .memory
            .write_u32(stack.checked_add(12).unwrap(), flags)
            .unwrap();
        cpu.set_register(0, text.0);
        runtime
            .dispatch(123, 0, &mut cpu, &mut StubServices)
            .unwrap();
        assert_eq!(cpu.register(0), 0);
    }

    runtime
        .memory
        .write_u32(stack.checked_add(12).unwrap(), 3)
        .unwrap();
    assert!(matches!(
        runtime.dispatch(123, 0, &mut cpu, &mut StubServices),
        Err(Error::Abi(message))
            if message == "unsupported text drawing flags 3 called by module 0"
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
