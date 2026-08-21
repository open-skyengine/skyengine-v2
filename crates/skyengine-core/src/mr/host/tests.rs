use super::*;

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
fn resolves_other_relative_files_from_the_work_directory() {
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
}

#[test]
fn selects_device_info_from_the_declared_package_platform() {
    assert_eq!(device_info_profile(1), DeviceInfoProfile::DeterministicMtk);
    assert_eq!(device_info_profile(0), DeviceInfoProfile::Unavailable);
    assert_eq!(device_info_profile(2), DeviceInfoProfile::Unavailable);
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
    assert_eq!(safe_work_path(Path::new("device"), b"C:\\app.mrp"), None);
}

#[test]
fn tracks_nested_application_restarts_as_a_stack() {
    let mut stack = Vec::new();

    update_application_stack(&mut stack, b"a.mrp", b"a.mr", b"b.mrp", b"b.mr");
    update_application_stack(&mut stack, b"b.mrp", b"b.mr", b"c.mrp", b"c.mr");
    assert_eq!(
        stack,
        vec![
            (b"a.mrp".to_vec(), b"a.mr".to_vec()),
            (b"b.mrp".to_vec(), b"b.mr".to_vec()),
        ]
    );

    update_application_stack(&mut stack, b"c.mrp", b"c.mr", b"b.mrp", b"b.mr");
    assert_eq!(stack, vec![(b"a.mrp".to_vec(), b"a.mr".to_vec())]);

    update_application_stack(&mut stack, b"b.mrp", b"b.mr", b"a.mrp", b"a.mr");
    assert!(stack.is_empty());
}
