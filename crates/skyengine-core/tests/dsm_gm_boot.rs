use std::path::PathBuf;

use skyengine_core::{Framebuffer, PlatformDisplay, Result, Runtime, RuntimeConfig, RuntimeState};

struct HeadlessDisplay;

impl PlatformDisplay for HeadlessDisplay {
    fn present(&mut self, _framebuffer: &Framebuffer) -> Result<()> {
        Ok(())
    }

    fn poll_event(&mut self) -> Result<Option<skyengine_core::DisplayEvent>> {
        Ok(None)
    }

    fn wait_timeout(&mut self, _milliseconds: u32) {}
}

#[test]
fn dsm_gm_runs_the_real_start_chain_and_renders_its_first_frame() {
    let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut config = RuntimeConfig::for_app(workspace.join("test/fixtures/dsm_gm.mrp"));
    config.font_path = workspace.join("test/fixtures/fonts/gb16.uc2");
    config.work_dir = std::env::temp_dir().join(format!(
        "skyengine-v2-empty-work-dir-{}",
        std::process::id()
    ));

    let mut runtime = Runtime::load(config, Box::new(HeadlessDisplay)).unwrap();
    runtime.start().unwrap();

    assert_eq!(runtime.state(), RuntimeState::Running);
    let framebuffer = runtime.framebuffer();
    assert_eq!((framebuffer.width(), framebuffer.height()), (240, 320));
    assert!(framebuffer.draw_count() > 0);
    assert!(framebuffer.pixels().iter().any(|pixel| *pixel != 0));
    assert_eq!(frame_signature(framebuffer), 0xb729_06a0_04fd_fc2a);
}

fn frame_signature(framebuffer: &Framebuffer) -> u64 {
    framebuffer
        .pixels()
        .iter()
        .flat_map(|pixel| pixel.to_le_bytes())
        .fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
        })
}
