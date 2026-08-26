#[test]
fn dump_dota_midis() {
    for name in [b"music_title.mid".to_vec(), b"dkljngle.mid".to_vec()] {
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test/fixtures/dota.mrp");
        let package = skyengine_core::Package::open(fixture, skyengine_core::ResourceLimits::default()).unwrap();
        let data = package.read_named(&name).unwrap();
        let out = std::path::Path::new("/tmp").join(String::from_utf8(name.clone()).unwrap());
        std::fs::write(&out, &data).unwrap();
        println!("wrote {} ({} bytes)", out.display(), data.len());
    }
}
