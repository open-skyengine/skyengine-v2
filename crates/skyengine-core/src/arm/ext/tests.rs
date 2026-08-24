use super::*;

struct StubServices;

std::thread_local! {
    static STUB_FRAMEBUFFER: std::cell::RefCell<Option<Vec<u8>>> = const {
        std::cell::RefCell::new(None)
    };
    static STUB_SOUND: std::cell::RefCell<Option<(SoundType, Vec<u8>, bool)>> = const {
        std::cell::RefCell::new(None)
    };
}

struct StubFramebufferCapture;

impl Drop for StubFramebufferCapture {
    fn drop(&mut self) {
        STUB_FRAMEBUFFER.with(|framebuffer| *framebuffer.borrow_mut() = None);
    }
}

fn capture_stub_framebuffer(framebuffer: Vec<u8>) -> StubFramebufferCapture {
    STUB_FRAMEBUFFER.with(|captured| *captured.borrow_mut() = Some(framebuffer));
    StubFramebufferCapture
}

fn load_test_module(runtime: &mut ExtRuntime) {
    let mut image = b"MRPGCMAP".to_vec();
    image.extend_from_slice(&0xe12f_ff1e_u32.to_le_bytes());
    runtime
        .load_and_call_entry(&image, 0, &mut StubServices)
        .unwrap();
}

#[test]
fn semihosting_exit_unwinds_the_guest_call_and_requests_runtime_exit() {
    let mut image = b"MRPGCMAP".to_vec();
    image.extend_from_slice(&0xe3a0_0018_u32.to_le_bytes()); // mov r0, #24
    image.extend_from_slice(&0xe3a0_1026_u32.to_le_bytes()); // mov r1, #38
    image.extend_from_slice(&0xef12_3456_u32.to_le_bytes()); // svc #0x123456
    let mut runtime =
        ExtRuntime::new(240, 320, b"test.mrp", b"start.mr", DEFAULT_HEAP_LEN as u32).unwrap();

    assert_eq!(
        runtime
            .load_and_call_entry(&image, 0, &mut StubServices)
            .unwrap(),
        0
    );
    assert_eq!(
        runtime.lifecycle_request().unwrap(),
        Some(ExtLifecycleRequest::Exit)
    );
}

impl NativeServices for StubServices {
    fn resize_screen(&mut self, _width: u16, _height: u16) -> Result<()> {
        Ok(())
    }

    fn capture_framebuffer(&mut self) -> Result<Option<Vec<u8>>> {
        Ok(STUB_FRAMEBUFFER.with(|framebuffer| framebuffer.borrow().clone()))
    }

    fn read_package_file(&mut self, _package_name: &[u8], name: &[u8]) -> Result<Option<Vec<u8>>> {
        Ok(match name {
            b"owned.bin" => Some(b"guest-owned".to_vec()),
            b"package.mp3" => Some(b"ID3".to_vec()),
            _ => None,
        })
    }

    fn file_info(&mut self, _name: &[u8]) -> Result<i32> {
        Ok(-1)
    }

    fn remove_file(&mut self, _name: &[u8]) -> Result<i32> {
        Ok(0)
    }

    fn rename_file(&mut self, _source: &[u8], _destination: &[u8]) -> Result<i32> {
        Ok(0)
    }

    fn create_dir(&mut self, _name: &[u8]) -> Result<i32> {
        Ok(0)
    }

    fn remove_dir(&mut self, _name: &[u8]) -> Result<i32> {
        Ok(0)
    }

    fn open_file(&mut self, _name: &[u8], _mode: u32) -> Result<i32> {
        Ok(-1)
    }

    fn close_file(&mut self, _handle: i32) -> Result<i32> {
        Ok(0)
    }

    fn write_file(&mut self, _handle: i32, _bytes: &[u8]) -> Result<Option<usize>> {
        Ok(None)
    }

    fn read_file(&mut self, _handle: i32, _len: usize) -> Result<Option<Vec<u8>>> {
        Ok(None)
    }

    fn seek_file(&mut self, handle: i32, _offset: i32, _origin: u32) -> Result<Option<u64>> {
        Ok((handle == 123).then_some(456))
    }

    fn file_len(&mut self, name: &[u8]) -> Result<Option<u64>> {
        Ok((name == b"media/clip.mp3").then_some(4))
    }

    fn read_sound_file(&mut self, name: &[u8]) -> Result<Option<Vec<u8>>> {
        Ok((name == b"media/clip.mp3").then(|| b"ID3!".to_vec()))
    }

    fn play_sound(&mut self, sound_type: SoundType, data: &[u8], looped: bool) -> Result<()> {
        STUB_SOUND.with(|sound| {
            *sound.borrow_mut() = Some((sound_type, data.to_vec(), looped));
        });
        Ok(())
    }

    fn stop_sound(&mut self) -> Result<()> {
        STUB_SOUND.with(|sound| *sound.borrow_mut() = None);
        Ok(())
    }

    fn find_start(&mut self, _directory: &[u8]) -> Result<Option<(i32, Vec<u8>)>> {
        Ok(Some((7, b"entry.dat".to_vec())))
    }

    fn find_next(&mut self, _handle: i32) -> Result<Option<Vec<u8>>> {
        Ok(None)
    }

    fn find_stop(&mut self, handle: i32) -> Result<bool> {
        Ok(handle == 7)
    }

    fn char_bitmap(&mut self, codepoint: u32, font: u32) -> Result<Option<(Vec<u8>, u32, u32)>> {
        Ok(match (codepoint, font) {
            (0x2603, 2 | 7) | (0x786e | 0x5b9a, 1 | 2) => {
                Some((vec![0x01, 0x80, 0x96, 0x4b], 9, 2))
            }
            _ => None,
        })
    }

    fn draw_bitmap(
        &mut self,
        pixels: &[u8],
        _x: i32,
        _y: i32,
        _width: usize,
        _height: usize,
    ) -> Result<()> {
        STUB_FRAMEBUFFER.with(|framebuffer| {
            let mut framebuffer = framebuffer.borrow_mut();
            if framebuffer.is_some() {
                *framebuffer = Some(pixels.to_vec());
            }
        });
        Ok(())
    }
}

mod abi;
mod external_actions;
mod lifecycle;
mod services;
mod state;
