use super::stdlib::*;
use super::*;

struct LifecycleTestDisplay;

impl PlatformDisplay for LifecycleTestDisplay {
    fn present(&mut self, _framebuffer: &Framebuffer) -> Result<()> {
        Ok(())
    }

    fn poll_event(&mut self) -> Result<Option<crate::DisplayEvent>> {
        Ok(None)
    }

    fn wait_timeout(&mut self, _milliseconds: u32) {}
}

#[test]
fn v80_division_uses_integer_semantics_without_changing_v50() {
    assert!(
        arithmetic(
            MrProfile::V80,
            15,
            &Value::Number(2.0),
            &Value::Number(13.0),
        )
        .unwrap()
        .raw_equal(&Value::Number(0.0))
    );
    assert!(
        arithmetic(
            MrProfile::V80,
            15,
            &Value::Number(-15.0),
            &Value::Number(4.0),
        )
        .unwrap()
        .raw_equal(&Value::Number(-3.0))
    );
    assert!(
        arithmetic(
            MrProfile::V50,
            15,
            &Value::Number(2.0),
            &Value::Number(13.0),
        )
        .unwrap()
        .raw_equal(&Value::Number(2.0 / 13.0))
    );
}

#[test]
fn legacy_pcall_alias_is_registered_and_protects_native_calls() {
    let (mut vm, root, _) = immediate_restart_vm();
    assert!(vm.global(b"_pCall").raw_equal(&Value::Native("_pCall")));

    let result = vm
        .call_value(
            Value::Native("_pCall"),
            vec![Value::Native("error"), bytes(b"expected failure")],
            None,
            false,
        )
        .unwrap();
    let CallResult::Immediate(values) = result else {
        panic!("native protected call must return immediately");
    };
    assert!(values[0].raw_equal(&Value::Boolean(false)));
    assert!(
        values[1]
            .bytes()
            .is_some_and(|error| error.ends_with(b"expected failure"))
    );

    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn legacy_network_close_is_registered_and_returns_success() {
    let (mut vm, root, _) = immediate_restart_vm();
    assert!(
        vm.global(b"_closeNet")
            .raw_equal(&Value::Native("_closeNet"))
    );

    let result = vm
        .call_value(Value::Native("_closeNet"), Vec::new(), None, false)
        .unwrap();
    let CallResult::Immediate(values) = result else {
        panic!("native network close must return immediately");
    };
    assert_eq!(values.len(), 1);
    assert_eq!(values[0].number(), Some(0.0));

    std::fs::remove_dir_all(root).unwrap();
}

fn lifecycle_test_root(label: &str) -> std::path::PathBuf {
    use std::time::{SystemTime, UNIX_EPOCH};

    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "skyengine-lifecycle-{label}-{}-{nonce}",
        std::process::id()
    ))
}

fn immediate_self_restart_image() -> Vec<u8> {
    const MODULE_BASE: u32 = 0x1000_0000;
    const TRAP_BASE: u32 = 0xff00_0000;
    const LIFECYCLE_CALLBACK_DATA: u32 = 0x0100_1984;
    const APPLICATION_STATE_DATA: u32 = 0x0100_1980;

    let helper = MODULE_BASE + 40;
    let restart = MODULE_BASE + 84;
    let instructions = [
        0xe92d_4000, // entry: push {lr}
        0xe59f_000c, // ldr r0, [pc, #12] (helper)
        0xe3a0_1014, // mov r1, #20
        0xe59f_c008, // ldr ip, [pc, #8] (slot 25)
        0xe12f_ff3c, // blx ip
        0xe8bd_8000, // pop {pc}
        helper,
        TRAP_BASE + 25 * 4,
        0xe59f_0018, // helper: ldr r0, [pc, #24] (callback pointer)
        0xe59f_1018, // ldr r1, [pc, #24] ("restart")
        0xe580_1000, // str r1, [r0]
        0xe59f_0014, // ldr r0, [pc, #20] (application state)
        0xe3a0_1003, // mov r1, #3
        0xe580_1000, // str r1, [r0]
        0xe3a0_0000, // mov r0, #0
        0xe12f_ff1e, // bx lr
        LIFECYCLE_CALLBACK_DATA,
        restart,
        APPLICATION_STATE_DATA,
        u32::from_le_bytes(*b"rest"),
        u32::from_le_bytes(*b"art\0"),
    ];
    let mut image = b"MRPGCMAP".to_vec();
    image.extend(instructions.into_iter().flat_map(u32::to_le_bytes));
    image
}

fn failing_native_init_image() -> Vec<u8> {
    const TRAP_BASE: u32 = 0xff00_0000;
    let instructions = [
        0xe59f_c000, // ldr ip, [pc] (unsupported platform trap)
        0xe12f_ff3c, // blx ip
        TRAP_BASE + 21 * 4,
    ];
    let mut image = b"MRPGCMAP".to_vec();
    image.extend(instructions.into_iter().flat_map(u32::to_le_bytes));
    image
}

fn write_lifecycle_test_package(path: &std::path::Path, image: &[u8]) {
    const LIST_START: usize = 0xf0;
    const ENTRY: &[u8] = b"start.mr";
    let directory_len = 4 + ENTRY.len() + 1 + 12;
    let payload_start = LIST_START + directory_len;
    let mut package = vec![0_u8; payload_start + image.len()];
    let package_len = package.len() as u32;
    package[0..4].copy_from_slice(b"MRPG");
    package[4..8].copy_from_slice(&((payload_start - 8) as u32).to_le_bytes());
    package[8..12].copy_from_slice(&package_len.to_le_bytes());
    package[12..16].copy_from_slice(&(LIST_START as u32).to_le_bytes());
    package[0x10..0x18].copy_from_slice(b"self.mrp");
    package[LIST_START..LIST_START + 4].copy_from_slice(&((ENTRY.len() + 1) as u32).to_le_bytes());
    let name = LIST_START + 4;
    package[name..name + ENTRY.len()].copy_from_slice(ENTRY);
    let fields = name + ENTRY.len() + 1;
    package[fields..fields + 4].copy_from_slice(&(payload_start as u32).to_le_bytes());
    package[fields + 4..fields + 8].copy_from_slice(&(image.len() as u32).to_le_bytes());
    package[payload_start..].copy_from_slice(image);
    std::fs::write(path, package).unwrap();
}

fn immediate_restart_vm() -> (MrVm, std::path::PathBuf, std::path::PathBuf) {
    let root = lifecycle_test_root("immediate-restart");
    let mythroad = root.join("mythroad");
    std::fs::create_dir_all(&mythroad).unwrap();
    let package_path = mythroad.join("self.mrp");
    write_lifecycle_test_package(&package_path, &immediate_self_restart_image());
    let limits = ResourceLimits::default();
    let package = Arc::new(Package::open(&package_path, limits.clone()).unwrap());
    let vm = MrVm::new(
        package,
        Framebuffer::new(240, 320).unwrap(),
        Box::new(LifecycleTestDisplay),
        Box::new(crate::SilentAudio),
        MrHostConfig {
            work_dir: root.clone(),
            font: Arc::from(&b""[..]),
            memory_limit: 2 * 1024 * 1024,
            dns_mappings: Vec::<crate::DnsMapping>::new().into(),
            device_date: crate::DeviceDate::default(),
            wap_proxy_endpoint: None,
        },
        limits,
    );
    (vm, root, package_path)
}

#[test]
fn immediate_self_restart_commits_at_most_once_per_dispatch() {
    let (mut vm, root, _) = immediate_restart_vm();
    vm.run_entry(b"start.mr").unwrap();
    assert!(matches!(
        vm.host.lifecycle_request().unwrap(),
        Some(ExtLifecycleRequest::Restart { .. })
    ));

    assert_eq!(
        vm.process_lifecycle_request().unwrap(),
        LifecycleOutcome::Continue
    );
    assert!(matches!(
        vm.host.lifecycle_request().unwrap(),
        Some(ExtLifecycleRequest::Restart { .. })
    ));
    assert_eq!(
        vm.process_lifecycle_request().unwrap(),
        LifecycleOutcome::Continue
    );
    assert!(matches!(
        vm.host.lifecycle_request().unwrap(),
        Some(ExtLifecycleRequest::Restart { .. })
    ));

    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn failed_restart_preparation_acknowledges_the_request_and_keeps_the_old_app_runnable() {
    let (mut vm, root, package_path) = immediate_restart_vm();
    vm.run_entry(b"start.mr").unwrap();
    std::fs::remove_file(package_path).unwrap();

    assert!(matches!(
        vm.process_lifecycle_request(),
        Err(LifecycleError::BeforeCommit(_))
    ));
    assert_eq!(vm.host.lifecycle_request().unwrap(), None);
    vm.host.dispatch_native_event(1, 2, 3).unwrap();
    assert!(matches!(
        vm.host.lifecycle_request().unwrap(),
        Some(ExtLifecycleRequest::Restart { .. })
    ));

    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn native_init_fault_after_restart_commit_is_terminal_for_the_lifecycle_dispatch() {
    let (mut vm, root, package_path) = immediate_restart_vm();
    vm.run_entry(b"start.mr").unwrap();
    write_lifecycle_test_package(&package_path, &failing_native_init_image());

    assert!(matches!(
        vm.process_lifecycle_request(),
        Err(LifecycleError::AfterCommit(_))
    ));
    assert_eq!(vm.host.lifecycle_request().unwrap(), None);

    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn table_insert_shifts_sequence_values() {
    let table = Table::new();
    table.borrow_mut().set(Value::Number(1.0), bytes(b"a"));
    table.borrow_mut().set(Value::Number(2.0), bytes(b"c"));
    table_insert(&[Value::Table(table.clone()), Value::Number(2.0), bytes(b"b")]).unwrap();
    assert_eq!(table.borrow().sequence_len(), 3);
    assert!(
        table
            .borrow()
            .get(&Value::Number(2.0))
            .raw_equal(&bytes(b"b"))
    );
}

#[test]
fn native_number_supports_radix() {
    assert!(native_tonumber(&[bytes(b"7f"), Value::Number(16.0)]).raw_equal(&Value::Number(127.0)));
    assert!(matches!(
        native_tonumber(&[bytes(b"not-a-number")]),
        Value::Nil
    ));
}

#[test]
fn mythroad_string_conversion_matches_tostring() {
    assert!(native_tostring(&[]).raw_equal(&bytes(b"nil")));
    assert!(native_tostring(&[Value::Number(54_892_597.0)]).raw_equal(&bytes(b"54892597")));
    assert!(native_tostring(&[Value::Boolean(true)]).raw_equal(&bytes(b"true")));
}

#[test]
fn c_string_helpers_stop_at_nul() {
    let value = bytes(b"abc\0def");
    assert_eq!(
        string_clen(std::slice::from_ref(&value)).unwrap()[0].number(),
        Some(3.0)
    );
    assert!(string_cstr(&[value]).unwrap()[0].raw_equal(&bytes(b"abc")));
}

#[test]
fn mutable_string_update_uses_one_based_offsets() {
    let buffer = Value::Buffer(std::rc::Rc::new(std::cell::RefCell::new(vec![0; 5])));
    string_update(&[buffer.clone(), bytes(b"abc"), Value::Number(2.0)]).unwrap();
    assert_eq!(buffer.bytes().unwrap().as_ref(), b"\0abc\0");
}

#[test]
fn mutable_string_update_moves_an_exclusive_source_range_with_overlap() {
    let buffer = Value::Buffer(std::rc::Rc::new(std::cell::RefCell::new(
        b"0123456789".to_vec(),
    )));
    string_update(&[
        buffer.clone(),
        buffer.clone(),
        Value::Number(1.0),
        Value::Number(4.0),
        Value::Number(8.0),
    ])
    .unwrap();
    assert_eq!(buffer.bytes().unwrap().as_ref(), b"3456456789");

    let unchanged = buffer.bytes().unwrap();
    string_update(&[
        buffer.clone(),
        buffer.clone(),
        Value::Number(1.0),
        Value::Number(8.0),
        Value::Number(8.0),
    ])
    .unwrap();
    assert_eq!(buffer.bytes().unwrap().as_ref(), unchanged.as_ref());
}

#[test]
fn mutable_string_update_copies_a_source_suffix_with_destination_truncation() {
    let buffer = Value::Buffer(std::rc::Rc::new(std::cell::RefCell::new(vec![0; 4])));
    string_update(&[
        buffer.clone(),
        bytes(b"abcdef"),
        Value::Number(2.0),
        Value::Number(3.0),
    ])
    .unwrap();
    assert_eq!(buffer.bytes().unwrap().as_ref(), b"\0cde");
}

#[test]
fn empty_string_update_is_a_noop_after_network_cleanup() {
    string_update(&[
        bytes(b""),
        bytes(b""),
        Value::Number(1.0),
        Value::Number(4_814.0),
    ])
    .unwrap();
}

#[test]
fn pure_mr_file_objects_write_and_remove_work_files() {
    let (mut vm, root, _) = immediate_restart_vm();
    let Value::Table(sys) = vm.global(b"sys") else {
        panic!("sys must be a table");
    };
    assert!(
        sys.borrow()
            .get(&bytes(b"rm"))
            .raw_equal(&Value::Native("file_remove"))
    );

    let file = vm
        .call_native("file_open", &[bytes(b"applist.mrp"), Value::Number(10.0)])
        .unwrap()
        .remove(0);
    assert!(matches!(file, Value::Table(_)));
    let written = vm
        .call_native("file_write", &[file.clone(), bytes(b"MRPG\0")])
        .unwrap();
    assert_eq!(written[0].number(), Some(5.0));
    assert_eq!(
        vm.call_native("file_close", &[file]).unwrap()[0].number(),
        Some(0.0)
    );
    let path = root.join("mythroad/applist.mrp");
    assert_eq!(std::fs::read(&path).unwrap(), b"MRPG\0");

    let file = vm
        .call_native("file_open", &[bytes(b"applist.mrp"), Value::Number(1.0)])
        .unwrap()
        .remove(0);
    assert_eq!(
        vm.call_native(
            "file_seek",
            &[file.clone(), Value::Number(1.0), Value::Number(0.0)],
        )
        .unwrap()[0]
            .number(),
        Some(1.0)
    );
    assert!(
        vm.call_native("file_read", &[file.clone(), Value::Number(3.0)])
            .unwrap()[0]
            .raw_equal(&bytes(b"RPG"))
    );
    vm.call_native("file_close", &[file]).unwrap();
    assert_eq!(
        vm.call_native("file_remove", &[bytes(b"applist.mrp")])
            .unwrap()[0]
            .number(),
        Some(0.0)
    );
    assert!(!path.exists());

    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn test_com_300_compiles_an_executable_network_callback() {
    let source = br#"print("code frame received")
def progress(data)
  p = tonumber(data)
  if p == nil then
    p = 0
  end
  if g_dialog then
    if g_dialog.update then
      g_dialog.update(g_dialog, nil, data .. "%", p)
    end
  end
  if win then
    if win.refresh then
      win.refresh()
    end
  end
end
cmd.progress = progress"#;
    let (mut vm, root, _) = immediate_restart_vm();
    let commands = Table::new();
    vm.set_global(b"cmd", Value::Table(commands.clone()))
        .unwrap();

    let compiled = vm
        .call_native(
            "TestCom1",
            &[
                Value::Number(300.0),
                Value::Bytes(Arc::from(source.as_slice())),
            ],
        )
        .unwrap()
        .remove(0);
    assert!(matches!(compiled, Value::Closure(_)));
    assert!(vm.global(b"_loads").raw_equal(&Value::Native("_loads")));
    let compiled = vm.call_native("_loads", &[compiled]).unwrap().remove(0);
    let CallResult::Pushed = vm.call_value(compiled, Vec::new(), None, false).unwrap() else {
        panic!("compiled source must execute as an MR frame");
    };
    vm.run_frames().unwrap();
    assert!(matches!(
        commands.borrow().get(&bytes(b"progress")),
        Value::Closure(_)
    ));

    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn text_network_callbacks_share_assigned_global_state() {
    let source = br#"dl_written = 0
def dl_file(data)
  dl_written = dl_written + data
end
cmd.file = dl_file"#;
    let (mut vm, root, _) = immediate_restart_vm();
    let commands = Table::new();
    vm.set_global(b"cmd", Value::Table(commands.clone()))
        .unwrap();

    let compiled = vm
        .call_native(
            "TestCom1",
            &[
                Value::Number(300.0),
                Value::Bytes(Arc::from(source.as_slice())),
            ],
        )
        .unwrap()
        .remove(0);
    let compiled = vm.call_native("_loads", &[compiled]).unwrap().remove(0);
    let CallResult::Pushed = vm.call_value(compiled, Vec::new(), None, false).unwrap() else {
        panic!("compiled source must execute as an MR frame");
    };
    vm.run_frames().unwrap();

    let callback = commands.borrow().get(&bytes(b"file"));
    let CallResult::Pushed = vm
        .call_value(callback.clone(), vec![Value::Number(17.0)], None, false)
        .unwrap()
    else {
        panic!("file callback must execute as an MR frame");
    };
    vm.run_frames().unwrap();
    let CallResult::Pushed = vm
        .call_value(callback, vec![Value::Number(25.0)], None, false)
        .unwrap()
    else {
        panic!("file callback must execute as an MR frame");
    };
    vm.run_frames().unwrap();

    assert_eq!(vm.global(b"dl_written").number(), Some(42.0));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn load_pack_installs_a_text_entry_loader_until_the_handle_is_released() {
    let (mut vm, root, _) = immediate_restart_vm();
    let package_path = root.join("mythroad/applist.mrp");
    write_lifecycle_test_package(
        &package_path,
        br#"list = {{t = "APP", e = "talkcat", ic = 1}}"#,
    );

    let handle = vm
        .call_native("LoadPack", &[bytes(b"applist.mrp")])
        .unwrap()
        .remove(0);
    assert!(handle.raw_equal(&Value::Native("loaded_pack")));
    assert!(
        vm.global(b"loadfile")
            .raw_equal(&Value::Native("load_pack_file"))
    );
    let entry = vm
        .call_native("load_pack_file", &[bytes(b"start.mr")])
        .unwrap()
        .remove(0);
    let CallResult::Pushed = vm.call_value(entry, Vec::new(), None, false).unwrap() else {
        panic!("loaded text entry must execute as an MR frame");
    };
    vm.run_frames().unwrap();
    let Value::Table(list) = vm.global(b"list") else {
        panic!("loaded entry must create the application list");
    };
    let Value::Table(application) = list.borrow().get(&Value::Number(1.0)) else {
        panic!("application list must contain the first record");
    };
    assert!(
        application
            .borrow()
            .get(&bytes(b"e"))
            .raw_equal(&bytes(b"talkcat"))
    );

    vm.call_native("LoadPack", &[handle]).unwrap();
    assert!(vm.global(b"loadfile").raw_equal(&Value::Nil));
    assert!(vm.host.read_loaded_pack(b"start.mr").is_err());

    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn sub_value_splits_numbers_and_byte_strings_into_little_endian_words() {
    let number_bits = 0x1122_3344_aabb_ccdd_u64;
    let number = string_sub_value(&[Value::Number(f64::from_bits(number_bits))]).unwrap();
    assert_eq!(number[0].number(), Some(0xaabb_ccdd_u32 as f64));
    assert_eq!(number[1].number(), Some(0x1122_3344_u32 as f64));

    let string = string_sub_value(&[bytes(b"unknow")]).unwrap();
    assert_eq!(string[0].number(), Some(0x6e6b_6e75_u32 as f64));
    assert_eq!(string[1].number(), Some(0x0000_776f_u32 as f64));
}

#[test]
fn work_paths_map_the_guest_c_drive_to_the_runtime_root() {
    let root = std::path::Path::new("device");
    assert_eq!(
        safe_work_path(root, b"C:/mythroad/system/font.uc2"),
        Some(PathBuf::from("device/mythroad/system/font.uc2"))
    );
    assert_eq!(
        safe_work_path(root, b"c:\\mythroad\\system\\font.uc2"),
        Some(PathBuf::from("device/mythroad/system/font.uc2"))
    );
    assert_eq!(
        safe_work_path(root, b"X:/mythroad/file"),
        Some(root.join("disk/x/mythroad/file"))
    );
    assert_eq!(
        safe_work_path(root, b"y:\\cache\\file"),
        Some(root.join("disk/y/cache/file"))
    );
    assert_eq!(
        safe_work_path(root, b"Z:/data/file"),
        Some(root.join("disk/z/data/file"))
    );
    assert_eq!(safe_work_path(root, b"D:/mythroad/file"), None);
    assert_eq!(safe_work_path(root, b"C:/../file"), None);
}
