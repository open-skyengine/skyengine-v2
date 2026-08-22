use super::*;

struct StubServices;

impl NativeServices for StubServices {
    fn read_package_file(&mut self, _package_name: &[u8], name: &[u8]) -> Result<Option<Vec<u8>>> {
        Ok((name == b"owned.bin").then(|| b"guest-owned".to_vec()))
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

    fn seek_file(&mut self, _handle: i32, _offset: i32, _origin: u32) -> Result<bool> {
        Ok(false)
    }

    fn file_len(&mut self, _name: &[u8]) -> Result<Option<u64>> {
        Ok(None)
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
        Ok((codepoint == 0x2603 && font == 7).then(|| (vec![0x01, 0x80, 0x96, 0x4b], 9, 2)))
    }

    fn draw_bitmap(
        &mut self,
        _pixels: &[u8],
        _x: i32,
        _y: i32,
        _width: usize,
        _height: usize,
    ) -> Result<()> {
        Ok(())
    }
}

mod abi;
mod services;
mod state;
