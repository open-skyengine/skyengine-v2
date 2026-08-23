use super::*;

struct TestDisplay;

impl PlatformDisplay for TestDisplay {
    fn present(&mut self, _framebuffer: &Framebuffer) -> Result<()> {
        Ok(())
    }

    fn poll_event(&mut self) -> Result<Option<crate::DisplayEvent>> {
        Ok(None)
    }

    fn wait_timeout(&mut self, _milliseconds: u32) {}
}

fn test_host() -> MrHost {
    MrHost::new(
        Arc::new(package_with_entries(&[])),
        Framebuffer::new(240, 320).unwrap(),
        Box::new(TestDisplay),
        MrHostConfig {
            work_dir: PathBuf::from("device"),
            font: Arc::from(&b""[..]),
            memory_limit: 1024 * 1024,
            dns_mappings: Vec::<DnsMapping>::new().into(),
            device_date: crate::DeviceDate::default(),
        },
    )
}

fn test_services(host: &mut MrHost) -> services::PackageServices<'_> {
    services::PackageServices {
        package: host.package.clone(),
        work_dir: host.work_dir.clone(),
        directory_searches: &mut host.directory_searches,
        next_directory_handle: &mut host.next_directory_handle,
        files: &mut host.native_files,
        next_file_handle: &mut host.next_native_file_handle,
        font: &host.font,
        framebuffer: &mut host.framebuffer,
        display: host.display.as_mut(),
    }
}

fn native_file_test_root(label: &str) -> PathBuf {
    use std::time::{SystemTime, UNIX_EPOCH};

    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "skyengine-native-file-{label}-{}-{nonce}",
        std::process::id()
    ))
}

fn package_with_entries(entries: &[(&[u8], &[u8])]) -> Package {
    let package = package_bytes_with_entries(b"test.mrp", entries);
    Package::parse(
        PathBuf::from("test.mrp"),
        package.into(),
        crate::ResourceLimits::default(),
    )
    .unwrap()
}

fn package_bytes_with_entries(internal_name: &[u8], entries: &[(&[u8], &[u8])]) -> Vec<u8> {
    const LIST_START: usize = 0xf0;

    let directory_len = entries
        .iter()
        .map(|(name, _)| 4 + name.len() + 1 + 12)
        .sum::<usize>();
    let payload_start = LIST_START + directory_len;
    let declared_len = payload_start + entries.iter().map(|(_, bytes)| bytes.len()).sum::<usize>();
    let mut package = vec![0_u8; declared_len];
    package[0..4].copy_from_slice(b"MRPG");
    package[4..8].copy_from_slice(&((payload_start - 8) as u32).to_le_bytes());
    package[8..12].copy_from_slice(&(declared_len as u32).to_le_bytes());
    package[12..16].copy_from_slice(&(LIST_START as u32).to_le_bytes());
    let internal_name_len = internal_name.len().min(12);
    package[0x10..0x10 + internal_name_len].copy_from_slice(&internal_name[..internal_name_len]);

    let mut directory_offset = LIST_START;
    let mut payload_offset = payload_start;
    for (name, payload) in entries {
        let name_len = name.len() + 1;
        package[directory_offset..directory_offset + 4]
            .copy_from_slice(&(name_len as u32).to_le_bytes());
        directory_offset += 4;
        package[directory_offset..directory_offset + name.len()].copy_from_slice(name);
        directory_offset += name_len;
        package[directory_offset..directory_offset + 4]
            .copy_from_slice(&(payload_offset as u32).to_le_bytes());
        package[directory_offset + 4..directory_offset + 8]
            .copy_from_slice(&(payload.len() as u32).to_le_bytes());
        directory_offset += 12;
        package[payload_offset..payload_offset + payload.len()].copy_from_slice(payload);
        payload_offset += payload.len();
    }

    package
}

fn write_test_package(path: &Path, internal_name: &[u8], entries: &[(&[u8], &[u8])]) {
    fs::write(path, package_bytes_with_entries(internal_name, entries)).unwrap();
}

#[test]
fn get_sys_info_advertises_the_baseline_vm_version_as_a_number() {
    let mut host = test_host();
    let values = host.call("GetSysInfo", &[]).unwrap();
    let Value::Table(info) = &values[0] else {
        panic!("GetSysInfo did not return a table");
    };

    assert_eq!(
        info.borrow().get(&bytes(b"vmver")).number(),
        Some(f64::from(BASELINE_VM_VERSION))
    );
}

#[test]
fn strcom_801_numeric_table_maps_to_the_two_raw_helper_arguments() {
    let mut image = b"MRPGCMAP".to_vec();
    let instructions = [
        0xe92d_4000, // push {lr}
        0xe51f_c014, // ldr ip, [pc, #-20] (platform table at image base)
        0xe59c_c064, // ldr ip, [ip, #100] (slot 25)
        0xe28f_0008, // add r0, pc, #8 (helper)
        0xe3a0_1014, // mov r1, #20
        0xe12f_ff3c, // blx ip
        0xe8bd_8000, // pop {pc}
        0xe082_0403, // helper: add r0, r2, r3, lsl #8
        0xe12f_ff1e, // bx lr
    ];
    image.extend(instructions.into_iter().flat_map(u32::to_le_bytes));

    let mut host = test_host();
    host.call(
        "_strCom",
        &[Value::Number(800.0), bytes(&image), Value::Number(0.0)],
    )
    .unwrap();

    let table = Table::new();
    table
        .borrow_mut()
        .set(Value::Number(1.0), Value::Number(1.0));
    table.borrow_mut().set(
        Value::Number(2.0),
        Value::Number(f64::from(BASELINE_VM_VERSION)),
    );
    let result = host
        .call(
            "_strCom",
            &[
                Value::Number(801.0),
                Value::Table(table),
                Value::Number(6.0),
            ],
        )
        .unwrap();

    assert_eq!(result[0].bytes().as_deref(), Some(&b""[..]));
    assert_eq!(
        result[1].number(),
        Some(f64::from(1 + (BASELINE_VM_VERSION << 8)))
    );
}

#[test]
fn invalid_ext_range_arguments_do_not_drop_the_active_runtime() {
    let mut host = test_host();
    let parameter = std::array::from_fn(|index| (index as u8).wrapping_mul(17));
    let mut runtime = host.take_or_create_ext_runtime().unwrap();
    runtime.set_start_file_parameter(&parameter).unwrap();
    host.ext_runtime = Some(runtime);
    let range = Table::new();
    range
        .borrow_mut()
        .set(Value::Number(1.0), bytes(b"not-an-address"));
    range
        .borrow_mut()
        .set(Value::Number(2.0), Value::Number(4.0));

    assert!(
        host.call(
            "_strCom",
            &[
                Value::Number(800.0),
                Value::Table(range),
                Value::Number(0.0),
            ],
        )
        .is_err()
    );
    assert_eq!(
        host.ext_runtime
            .as_ref()
            .unwrap()
            .start_file_parameter()
            .unwrap(),
        parameter
    );
    // A protected MR call may consume the argument error and keep running.
    assert!(host.call("GetSysInfo", &[]).is_ok());
}

#[test]
fn resolves_the_current_package_name_to_its_installed_path() {
    assert_eq!(
        native_file_path(
            Path::new("device"),
            Path::new("device/mythroad/app.mrp"),
            b"installed.mrp",
            b"app.mrp",
        ),
        Some(PathBuf::from("device/mythroad/app.mrp"))
    );
}

#[test]
fn resolves_the_internal_package_name_to_its_installed_path() {
    assert_eq!(
        native_file_path(
            Path::new("device"),
            Path::new("device/mythroad/app-v2.mrp"),
            b"app.mrp",
            b"app.mrp",
        ),
        Some(PathBuf::from("device/mythroad/app-v2.mrp"))
    );
}

#[test]
fn resolves_other_relative_files_from_the_mythroad_directory() {
    assert_eq!(
        native_file_path(
            Path::new("device"),
            Path::new("device/mythroad/app.mrp"),
            b"installed.mrp",
            b"app/data.dat",
        ),
        Some(PathBuf::from("device/mythroad/app/data.dat"))
    );
}

#[test]
fn resolves_device_files_inside_the_mythroad_directory() {
    assert_eq!(
        safe_work_path(Path::new("device"), b"gxdzc\\res.temp"),
        Some(PathBuf::from("device/mythroad/gxdzc/res.temp"))
    );
    assert_eq!(
        safe_work_path(Path::new("device"), b"mythroad/gxdzc/res.pak"),
        Some(PathBuf::from("device/mythroad/gxdzc/res.pak"))
    );
    assert_eq!(
        safe_work_path(Path::new("device"), b"C:/mythroad/gxdzc/res.pak"),
        Some(PathBuf::from("device/mythroad/gxdzc/res.pak"))
    );
    assert_eq!(
        safe_work_path(Path::new("device"), b"c:\\mythroad\\gxdzc\\res.pak"),
        Some(PathBuf::from("device/mythroad/gxdzc/res.pak"))
    );
    assert_eq!(
        safe_work_path(Path::new("device"), b"C:/root.dat"),
        Some(PathBuf::from("device/root.dat"))
    );
    assert_eq!(
        safe_work_path(Path::new("device"), b"X:/cache/index.dat"),
        Some(PathBuf::from("device/disk/x/cache/index.dat"))
    );
    assert_eq!(
        safe_work_path(Path::new("device"), b"y:\\data\\save.dat"),
        Some(PathBuf::from("device/disk/y/data/save.dat"))
    );
    assert_eq!(
        safe_work_path(Path::new("device"), b"Z:/root.dat"),
        Some(PathBuf::from("device/disk/z/root.dat"))
    );
}

#[test]
fn read_create_mode_opens_an_existing_file_at_byte_zero() {
    let root = native_file_test_root("read-create-existing");
    let path = root.join("mythroad/gsha/global");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, b"save-data").unwrap();

    let mut host = test_host();
    host.work_dir = root.clone();
    let mut services = test_services(&mut host);
    let handle = services.open_file(b"gsha\\global", 1 | 8).unwrap();

    assert!(handle >= 0);
    assert_eq!(
        services.read_file(handle, 4).unwrap(),
        Some(b"save".to_vec())
    );
    assert_eq!(services.close_file(handle).unwrap(), 0);

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn read_create_mode_creates_a_missing_file_and_reopens_it_read_only() {
    let root = native_file_test_root("read-create-missing");
    let path = root.join("mythroad/gsha/global");

    let mut host = test_host();
    host.work_dir = root.clone();
    let mut services = test_services(&mut host);
    let handle = services.open_file(b"gsha/global", 1 | 8).unwrap();

    assert!(handle >= 0);
    assert_eq!(services.read_file(handle, 1).unwrap(), Some(Vec::new()));
    assert_eq!(services.write_file(handle, b"x").unwrap(), None);
    assert_eq!(services.close_file(handle).unwrap(), 0);
    assert_eq!(fs::read(&path).unwrap(), Vec::<u8>::new());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn selects_native_extension_profile_from_the_declared_package_platform_and_version() {
    assert_eq!(
        native_extension_profile(1, 1_000),
        NativeExtensionProfile::Mtk
    );
    assert_eq!(
        native_extension_profile(1, 999),
        NativeExtensionProfile::Baseline
    );
    assert_eq!(
        native_extension_profile(0, 1_000),
        NativeExtensionProfile::Baseline
    );
    assert_eq!(
        native_extension_profile(2, 1_000),
        NativeExtensionProfile::Baseline
    );
}

#[test]
fn resolves_application_directory_paths_to_package_entries() {
    assert_eq!(
        package_entry_path(b"gxdzc.mrp", b"gxdzc\\res.list"),
        Some(b"res.list".to_vec())
    );
    assert_eq!(
        package_entry_path(b"gxdzc.mrp", b"images\\title.bmp"),
        Some(b"images/title.bmp".to_vec())
    );
    assert_eq!(package_entry_path(b"gxdzc.mrp", b"..\\secret"), None);
}

#[test]
fn current_package_file_prefers_an_exact_name_over_logical_basename_matches() {
    let package = package_with_entries(&[
        (b"startw.jpg", b"jpg"),
        (b"startw.png", b"png"),
        (b"startw", b"exact"),
        (b"images/startw.jpg", b"path-jpg"),
        (b"images/startw", b"path-exact"),
    ]);

    assert_eq!(
        services::read_current_package_entry(&package, b"startw").unwrap(),
        Some(b"exact".to_vec())
    );
    assert_eq!(
        services::read_current_package_entry(&package, b"test\\images\\startw").unwrap(),
        Some(b"path-exact".to_vec())
    );
}

#[test]
fn current_package_file_does_not_guess_the_contents_of_other_logical_resource_formats() {
    let package = package_with_entries(&[
        (b"other/startw.dat", b"other"),
        (b"images/startw.dat", b"first"),
        (b"images/startw.dat", b"overlay"),
        (b"images/startw.", b"empty-suffix"),
        (b"images/startw.dat/thumb", b"child"),
        (b"images/startw.dat\\thumb", b"backslash-child"),
    ]);

    assert_eq!(
        services::read_current_package_entry(&package, b"test\\images\\startw").unwrap(),
        None
    );
}

#[test]
fn current_package_file_decodes_a_unique_jpeg_logical_resource_as_rgb565() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test/fixtures/talkcat.mrp");
    let package = Package::open(fixture, crate::ResourceLimits::default()).unwrap();
    let encoded = package.read_named(b"startw.jpg").unwrap();

    assert_eq!(
        services::read_current_package_entry(&package, b"startw.jpg").unwrap(),
        Some(encoded)
    );
    let decoded = services::read_current_package_entry(&package, b"startw")
        .unwrap()
        .unwrap();
    assert_eq!(decoded.len(), 240 * 320 * 2);
    assert_eq!(
        &decoded[..16],
        &[
            0x62, 0x00, 0x62, 0x00, 0x62, 0x00, 0x62, 0x00, 0x62, 0x00, 0x62, 0x00, 0x62, 0x00,
            0x62, 0x00,
        ]
    );
}

#[test]
fn current_package_file_applies_the_expanded_file_limit_to_logical_jpegs() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test/fixtures/talkcat.mrp");
    let limits = crate::ResourceLimits {
        max_expanded_file_len: 240 * 320 * 2 - 1,
        ..crate::ResourceLimits::default()
    };
    let package = Package::open(fixture, limits).unwrap();

    assert!(matches!(
        services::read_current_package_entry(&package, b"startw"),
        Err(crate::Error::ResourceLimit(message))
            if message.contains("decoded JPEG resource") && message.contains("153600")
    ));
}

#[test]
fn current_package_file_rejects_ambiguous_logical_basename_matches() {
    let package = package_with_entries(&[
        (b"images/startw.jpg", b"jpg"),
        (b"images/startw.jpg", b"overlay"),
        (b"images/startw.png", b"png"),
    ]);

    assert_eq!(
        services::read_current_package_entry(&package, b"test/images/startw").unwrap(),
        None
    );
}

#[test]
fn identifies_nested_package_paths_case_insensitively() {
    assert!(is_mrp_file_name(b"gwy.mrp"));
    assert!(is_mrp_file_name(b"C:\\mythroad\\plugins\\PAY.MRP"));
    assert!(!is_mrp_file_name(b"gxdzc\\res.list"));
}

#[test]
fn rejects_parent_paths_for_native_files() {
    assert_eq!(
        native_file_path(
            Path::new("device"),
            Path::new("device/mythroad/app.mrp"),
            b"installed.mrp",
            b"../app.mrp",
        ),
        None
    );
    assert_eq!(safe_work_path(Path::new("device"), b"..\\app.mrp"), None);
    assert_eq!(safe_work_path(Path::new("device"), b"D:\\app.mrp"), None);
    assert_eq!(safe_work_path(Path::new("device"), b"C:app.mrp"), None);
    assert_eq!(safe_work_path(Path::new("device"), b"C:/../app.mrp"), None);
}

#[test]
fn native_path_component_prefers_an_exact_name() {
    assert_eq!(
        select_ascii_case_component(
            OsStr::new("uid.scene"),
            [OsString::from("UID.scene"), OsString::from("uid.scene")],
        ),
        NativePathComponent::Match(OsString::from("uid.scene"))
    );
}

#[test]
fn native_path_component_accepts_one_ascii_case_match() {
    assert_eq!(
        select_ascii_case_component(OsStr::new("UID.scene"), [OsString::from("uid.scene")],),
        NativePathComponent::Match(OsString::from("uid.scene"))
    );
}

#[test]
fn native_path_component_rejects_ambiguous_ascii_case_matches() {
    assert_eq!(
        select_ascii_case_component(
            OsStr::new("Uid.Scene"),
            [OsString::from("UID.scene"), OsString::from("uid.scene")],
        ),
        NativePathComponent::Ambiguous
    );
}

#[test]
fn native_work_path_leaves_an_external_package_path_exact() {
    assert_eq!(
        resolve_native_work_path(Path::new("device"), Path::new("packages/app.mrp")),
        Some(PathBuf::from("packages/app.mrp"))
    );
}

#[cfg(unix)]
#[test]
fn native_work_path_rejects_a_symlink_escape() {
    use std::os::unix::fs::symlink;
    use std::time::{SystemTime, UNIX_EPOCH};

    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "skyengine-native-path-{}-{nonce}",
        std::process::id()
    ));
    let work_dir = root.join("work");
    let outside = root.join("outside");
    fs::create_dir_all(&work_dir).unwrap();
    fs::create_dir_all(&outside).unwrap();
    fs::write(outside.join("secret.bin"), b"secret").unwrap();
    symlink(&outside, work_dir.join("Data")).unwrap();

    assert_eq!(
        resolve_native_work_path(&work_dir, &work_dir.join("data/secret.bin")),
        None
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn tracks_nested_application_restarts_as_a_stack() {
    let mut stack = Vec::new();

    let ApplicationStackTransition::Push(parent) = application_stack_transition(
        &stack,
        b"a.mrp",
        b"a.mr",
        Path::new("fixtures/a.mrp"),
        Path::new("device/mythroad/b.mrp"),
        b"b.mr",
    ) else {
        panic!("A -> B did not push A");
    };
    stack.push(parent);
    let ApplicationStackTransition::Push(parent) = application_stack_transition(
        &stack,
        b"b.mrp",
        b"b.mr",
        Path::new("device/mythroad/b.mrp"),
        Path::new("device/mythroad/c.mrp"),
        b"c.mr",
    ) else {
        panic!("B -> C did not push B");
    };
    stack.push(parent);
    assert_eq!(
        stack,
        vec![
            (
                b"a.mrp".to_vec(),
                b"a.mr".to_vec(),
                PathBuf::from("fixtures/a.mrp"),
            ),
            (
                b"b.mrp".to_vec(),
                b"b.mr".to_vec(),
                PathBuf::from("device/mythroad/b.mrp"),
            ),
        ]
    );

    assert_eq!(
        application_stack_transition(
            &stack,
            b"c.mrp",
            b"c.mr",
            Path::new("device/mythroad/c.mrp"),
            Path::new("device/mythroad/b.mrp"),
            b"b.mr",
        ),
        ApplicationStackTransition::Pop
    );
    stack.pop();
    assert_eq!(
        stack,
        vec![(
            b"a.mrp".to_vec(),
            b"a.mr".to_vec(),
            PathBuf::from("fixtures/a.mrp"),
        )]
    );

    assert_eq!(
        application_stack_transition(
            &stack,
            b"b.mrp",
            b"b.mr",
            Path::new("device/mythroad/b.mrp"),
            Path::new("device/mythroad/b.mrp"),
            b"b.mr",
        ),
        ApplicationStackTransition::Stay
    );
    assert_eq!(stack.len(), 1);

    assert_eq!(
        application_stack_transition(
            &stack,
            b"b.mrp",
            b"b.mr",
            Path::new("device/mythroad/b.mrp"),
            Path::new("fixtures/a.mrp"),
            b"a.mr",
        ),
        ApplicationStackTransition::Pop
    );
    stack.pop();
    assert!(stack.is_empty());
}

#[test]
fn identical_application_identity_at_a_different_path_does_not_pop_the_parent() {
    let stack = vec![(
        b"same.mrp".to_vec(),
        b"start.mr".to_vec(),
        PathBuf::from("device/mythroad/parent.mrp"),
    )];

    assert!(matches!(
        application_stack_transition(
            &stack,
            b"child.mrp",
            b"start.mr",
            Path::new("device/mythroad/child.mrp"),
            Path::new("device/mythroad/other.mrp"),
            b"start.mr",
        ),
        ApplicationStackTransition::Push(_)
    ));
}

#[test]
fn prepare_restart_honors_an_explicit_path_when_the_parent_has_the_same_identity() {
    let root = native_file_test_root("same-identity-explicit-path");
    let mythroad = root.join("mythroad");
    let parent_dir = mythroad.join("parent");
    let other_dir = mythroad.join("other");
    fs::create_dir_all(&parent_dir).unwrap();
    fs::create_dir_all(&other_dir).unwrap();
    let image = [b"MRPGCMAP".as_slice(), &0xe12f_ff1e_u32.to_le_bytes()].concat();
    let child_path = mythroad.join("child.mrp");
    let parent_path = parent_dir.join("same.mrp");
    let explicit_path = other_dir.join("same.mrp");
    write_test_package(&child_path, b"child.mrp", &[(b"start.mr", &image)]);
    write_test_package(&parent_path, b"same.mrp", &[(b"start.mr", &image)]);
    write_test_package(&explicit_path, b"same.mrp", &[(b"start.mr", &image)]);
    let limits = crate::ResourceLimits::default();
    let package = Arc::new(Package::open(&child_path, limits.clone()).unwrap());
    let mut host = MrHost::new(
        package.clone(),
        Framebuffer::new(240, 320).unwrap(),
        Box::new(TestDisplay),
        MrHostConfig {
            work_dir: root.clone(),
            font: Arc::from(&b""[..]),
            memory_limit: 2 * 1024 * 1024,
            dns_mappings: Vec::<DnsMapping>::new().into(),
            device_date: crate::DeviceDate::default(),
        },
    );
    host.application_stack.push((
        b"same.mrp".to_vec(),
        b"start.mr".to_vec(),
        parent_path.clone(),
    ));

    let explicit = host
        .prepare_restart(b"mythroad/other/same.mrp", b"start.mr", &limits)
        .unwrap();
    assert_eq!(explicit.package.path(), explicit_path);
    assert!(matches!(
        explicit.stack_transition,
        ApplicationStackTransition::Push(_)
    ));
    assert!(Arc::ptr_eq(&host.package, &package));
    assert_eq!(host.application_stack.len(), 1);

    let identity_only = host
        .prepare_restart(b"same.mrp", b"start.mr", &limits)
        .unwrap();
    assert_eq!(identity_only.package.path(), parent_path);
    assert_eq!(
        identity_only.stack_transition,
        ApplicationStackTransition::Pop
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn application_replacement_is_cold_and_carries_the_latest_session_parameter() {
    let fixture_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test/fixtures");
    let parent_path = fixture_root.join("cookie_v6110.mrp");
    let limits = crate::ResourceLimits::default();
    let parent_package = Arc::new(Package::open(&parent_path, limits.clone()).unwrap());
    let parent_identity = parent_package.header().internal_name.clone();
    let mut host = MrHost::new(
        parent_package,
        Framebuffer::new(240, 320).unwrap(),
        Box::new(TestDisplay),
        MrHostConfig {
            work_dir: fixture_root,
            font: Arc::from(&b""[..]),
            memory_limit: 2 * 1024 * 1024,
            dns_mappings: Vec::<DnsMapping>::new().into(),
            device_date: crate::DeviceDate::default(),
        },
    );
    let parameter = std::array::from_fn(|index| (index as u8).wrapping_mul(29));
    let mut runtime = host.take_or_create_ext_runtime().unwrap();
    runtime.set_start_file_parameter(&parameter).unwrap();
    host.ext_runtime = Some(runtime);
    host.sdk_key = Some(17);
    host.bitmaps.insert(
        3,
        Bitmap {
            width: 1,
            height: 1,
            pixels: vec![0x1234],
            frame_height: None,
            transparent_color: 0,
        },
    );
    host.directory_searches.insert(
        5,
        DirectorySearch {
            entries: vec![Arc::from(&b"entry"[..])],
            next: 0,
        },
    );
    host.native_files
        .insert(7, NativeFile::Host(File::open(&parent_path).unwrap()));
    host.next_directory_handle = 9;
    host.next_native_file_handle = 11;
    host.framebuffer.point(3, 4, 0x1234);

    let stack_before = host.application_stack.clone();
    let previous_before = host.previous_application.clone();
    let package_before = host.package.path().to_path_buf();
    assert!(
        host.prepare_restart(b"missing-target.mrp", b"start.mr", &limits)
            .is_err()
    );
    assert_eq!(host.application_stack, stack_before);
    assert_eq!(host.previous_application, previous_before);
    assert_eq!(host.package.path(), package_before);
    assert_eq!(
        host.ext_runtime
            .as_ref()
            .unwrap()
            .start_file_parameter()
            .unwrap(),
        parameter
    );

    let child = host
        .prepare_restart(b"mythroad/dsm_gm.mrp", b"start.mr", &limits)
        .unwrap();
    let child_identity = child.package.header().internal_name.clone();
    assert_eq!(child.start_file_parameter, parameter);
    assert_eq!(host.application_stack, stack_before);
    let _ = host.commit_application(child);

    assert_eq!(
        host.previous_application,
        Some((parent_identity.clone(), b"start.mr".to_vec()))
    );
    assert_eq!(host.application_stack.len(), 1);
    assert!(host.ext_runtime.is_none());
    assert!(host.bitmaps.is_empty());
    assert!(host.directory_searches.is_empty());
    assert!(host.native_files.is_empty());
    assert_eq!(host.next_directory_handle, 1);
    assert_eq!(host.next_native_file_handle, 1);
    assert_eq!(host.sdk_key, None);
    assert_eq!(host.framebuffer.pixels()[4 * 240 + 3], 0x1234);

    let updated_parameter = std::array::from_fn(|index| 255_u8.wrapping_sub(index as u8));
    let mut child_runtime = host.take_or_create_ext_runtime().unwrap();
    assert_eq!(child_runtime.start_file_parameter().unwrap(), parameter);
    child_runtime
        .set_start_file_parameter(&updated_parameter)
        .unwrap();
    host.ext_runtime = Some(child_runtime);

    let restarted_child = host
        .prepare_restart(&child_identity, b"start.mr", &limits)
        .unwrap();
    assert_eq!(
        restarted_child.stack_transition,
        ApplicationStackTransition::Stay
    );
    assert_eq!(restarted_child.start_file_parameter, updated_parameter);
    let _ = host.commit_application(restarted_child);
    assert_eq!(host.application_stack.len(), 1);
    assert_eq!(
        host.previous_application,
        Some((parent_identity.clone(), b"start.mr".to_vec()))
    );
    let child_runtime = host.take_or_create_ext_runtime().unwrap();
    assert_eq!(
        child_runtime.start_file_parameter().unwrap(),
        updated_parameter
    );
    host.ext_runtime = Some(child_runtime);

    let returned_parent = host
        .prepare_restart(&parent_identity, b"start.mr", &limits)
        .unwrap();
    assert_eq!(
        returned_parent.stack_transition,
        ApplicationStackTransition::Pop
    );
    assert_eq!(returned_parent.start_file_parameter, updated_parameter);
    let _ = host.commit_application(returned_parent);

    assert!(host.application_stack.is_empty());
    assert_eq!(
        host.previous_application,
        Some((child_identity, b"start.mr".to_vec()))
    );
    assert_eq!(host.package.path(), parent_path);
    let parent_runtime = host.take_or_create_ext_runtime().unwrap();
    assert_eq!(
        parent_runtime.start_file_parameter().unwrap(),
        updated_parameter
    );
}

#[cfg(unix)]
#[test]
fn restart_rejects_a_symlink_target_without_committing() {
    use std::os::unix::fs::symlink;

    let root = native_file_test_root("restart-symlink");
    let mythroad = root.join("mythroad");
    let outside = root.with_extension("outside");
    fs::create_dir_all(&mythroad).unwrap();
    fs::create_dir_all(&outside).unwrap();
    let image = [b"MRPGCMAP".as_slice(), &0xe12f_ff1e_u32.to_le_bytes()].concat();
    let parent_path = mythroad.join("parent.mrp");
    let outside_path = outside.join("outside.mrp");
    write_test_package(&parent_path, b"parent.mrp", &[(b"start.mr", &image)]);
    write_test_package(&outside_path, b"outside.mrp", &[(b"start.mr", &image)]);
    symlink(&outside_path, mythroad.join("escape.mrp")).unwrap();
    let limits = crate::ResourceLimits::default();
    let package = Arc::new(Package::open(&parent_path, limits.clone()).unwrap());
    let host_config = MrHostConfig {
        work_dir: root.clone(),
        font: Arc::from(&b""[..]),
        memory_limit: 2 * 1024 * 1024,
        dns_mappings: Vec::<DnsMapping>::new().into(),
        device_date: crate::DeviceDate::default(),
    };
    let host = MrHost::new(
        package.clone(),
        Framebuffer::new(240, 320).unwrap(),
        Box::new(TestDisplay),
        host_config,
    );

    assert!(
        host.prepare_restart(b"escape.mrp", b"start.mr", &limits)
            .is_err()
    );
    assert!(Arc::ptr_eq(&host.package, &package));
    assert!(host.application_stack.is_empty());
    assert!(host.previous_application.is_none());

    fs::remove_dir_all(root).unwrap();
    fs::remove_dir_all(outside).unwrap();
}

#[test]
fn failed_staging_preserves_the_running_application_and_resources() {
    let root = native_file_test_root("failed-restart-staging");
    let mythroad = root.join("mythroad");
    fs::create_dir_all(&mythroad).unwrap();
    let image = [b"MRPGCMAP".as_slice(), &0xe12f_ff1e_u32.to_le_bytes()].concat();
    let parent_path = mythroad.join("parent.mrp");
    write_test_package(&parent_path, b"parent.mrp", &[(b"start.mr", &image)]);
    write_test_package(
        &mythroad.join("bad-gzip.mrp"),
        b"bad-gzip",
        &[(b"start.mr", b"\x1f\x8b\x08broken")],
    );
    write_test_package(
        &mythroad.join("bad-mr.mrp"),
        b"bad-mr.mrp",
        &[(b"start.mr", b"\x1bMRP\x50")],
    );
    write_test_package(
        &mythroad.join("bad-ext-marker.mrp"),
        b"bad-marker",
        &[(b"start.mr", b"MRPGCMAP")],
    );
    let mut oversized_image = b"MRPGCMAP".to_vec();
    oversized_image.resize(0x0010_0001, 0);
    write_test_package(
        &mythroad.join("oversized-ext.mrp"),
        b"oversized",
        &[(b"start.mr", &oversized_image)],
    );
    let long_entry = vec![b'x'; 256];
    write_test_package(
        &mythroad.join("bad-ext.mrp"),
        b"bad-ext.mrp",
        &[(long_entry.as_slice(), &image)],
    );
    let limits = crate::ResourceLimits::default();
    let package = Arc::new(Package::open(&parent_path, limits.clone()).unwrap());
    let mut host = MrHost::new(
        package.clone(),
        Framebuffer::new(240, 320).unwrap(),
        Box::new(TestDisplay),
        MrHostConfig {
            work_dir: root.clone(),
            font: Arc::from(&b""[..]),
            memory_limit: 2 * 1024 * 1024,
            dns_mappings: Vec::<DnsMapping>::new().into(),
            device_date: crate::DeviceDate::default(),
        },
    );
    let parameter = std::array::from_fn(|index| index as u8 ^ 0xa5);
    let mut runtime = host.take_or_create_ext_runtime().unwrap();
    runtime.set_start_file_parameter(&parameter).unwrap();
    host.ext_runtime = Some(runtime);
    host.bitmaps.insert(
        7,
        Bitmap {
            width: 1,
            height: 1,
            pixels: vec![0x1234],
            frame_height: None,
            transparent_color: 0,
        },
    );
    host.directory_searches.insert(
        8,
        DirectorySearch {
            entries: vec![Arc::from(&b"entry"[..])],
            next: 0,
        },
    );
    let open_path = root.join("open.bin");
    fs::write(&open_path, b"open").unwrap();
    host.native_files
        .insert(9, NativeFile::Host(File::open(open_path).unwrap()));
    host.next_directory_handle = 10;
    host.next_native_file_handle = 11;
    host.sdk_key = Some(12);

    for (package_name, entry) in [
        (b"bad-gzip.mrp".as_slice(), b"start.mr".as_slice()),
        (b"bad-mr.mrp".as_slice(), b"start.mr".as_slice()),
        (b"bad-ext-marker.mrp".as_slice(), b"start.mr".as_slice()),
        (b"oversized-ext.mrp".as_slice(), b"start.mr".as_slice()),
        (b"bad-ext.mrp".as_slice(), long_entry.as_slice()),
    ] {
        assert!(host.prepare_restart(package_name, entry, &limits).is_err());
        assert!(Arc::ptr_eq(&host.package, &package));
        assert_eq!(host.current_entry, b"start.mr");
        assert!(host.application_stack.is_empty());
        assert!(host.previous_application.is_none());
        assert!(host.bitmaps.contains_key(&7));
        assert!(host.directory_searches.contains_key(&8));
        assert!(host.native_files.contains_key(&9));
        assert_eq!(host.next_directory_handle, 10);
        assert_eq!(host.next_native_file_handle, 11);
        assert_eq!(host.sdk_key, Some(12));
        assert_eq!(
            host.ext_runtime
                .as_ref()
                .unwrap()
                .start_file_parameter()
                .unwrap(),
            parameter
        );
        assert!(host.call("GetSysInfo", &[]).is_ok());
    }

    fs::remove_dir_all(root).unwrap();
}
