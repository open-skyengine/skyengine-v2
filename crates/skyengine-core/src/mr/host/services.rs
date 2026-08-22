use super::*;

const FILE_INFO_FILE: i32 = 1;
const FILE_INFO_DIRECTORY: i32 = 2;
const FILE_INFO_INVALID: i32 = 8;

pub(super) struct PackageServices<'a> {
    pub(super) package: Arc<Package>,
    pub(super) work_dir: PathBuf,
    pub(super) directory_searches: &'a mut BTreeMap<i32, DirectorySearch>,
    pub(super) next_directory_handle: &'a mut i32,
    pub(super) files: &'a mut BTreeMap<i32, NativeFile>,
    pub(super) next_file_handle: &'a mut i32,
    pub(super) font: &'a [u8],
    pub(super) framebuffer: &'a mut Framebuffer,
    pub(super) display: &'a mut dyn PlatformDisplay,
}

impl PackageServices<'_> {
    fn file_path(&self, name: &[u8]) -> Option<PathBuf> {
        native_file_path(
            &self.work_dir,
            self.package.path(),
            &self.package.header().internal_name,
            name,
        )
    }

    fn is_root_package(&self, package_name: &[u8]) -> bool {
        package_name == self.package.header().internal_name
            || self
                .package
                .path()
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| package_name == name.as_bytes())
    }

    fn read_current_package_file(&self, name: &[u8]) -> Result<Option<Vec<u8>>> {
        match self.package.read_named(name) {
            Ok(bytes) => return Ok(Some(bytes)),
            Err(crate::Error::EntryNotFound(_)) => {}
            Err(error) => return Err(error),
        }
        let Some(entry_name) = package_entry_path(&self.package.header().internal_name, name)
        else {
            return Ok(None);
        };
        if entry_name == name {
            return Ok(None);
        }
        match self.package.read_named(&entry_name) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(crate::Error::EntryNotFound(_)) => Ok(None),
            Err(error) => Err(error),
        }
    }
}

impl NativeServices for PackageServices<'_> {
    fn read_package_file(&mut self, package_name: &[u8], name: &[u8]) -> Result<Option<Vec<u8>>> {
        let nested_package;
        let package = if self.is_root_package(package_name) {
            self.package.as_ref()
        } else {
            let Some(path) = self.file_path(package_name) else {
                return Ok(None);
            };
            nested_package = match Package::open(path, self.package.limits().clone()) {
                Ok(package) => package,
                Err(crate::Error::Io { source, .. })
                    if source.kind() == std::io::ErrorKind::NotFound =>
                {
                    return Ok(None);
                }
                Err(error) => return Err(error),
            };
            &nested_package
        };
        match package.read_named(name) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(crate::Error::EntryNotFound(_)) => Ok(None),
            Err(error) => Err(error),
        }
    }

    fn file_info(&mut self, name: &[u8]) -> Result<i32> {
        let Some(path) = self.file_path(name) else {
            return Ok(-1);
        };
        match fs::metadata(path) {
            Ok(metadata) if metadata.is_file() => Ok(FILE_INFO_FILE),
            Ok(metadata) if metadata.is_dir() => Ok(FILE_INFO_DIRECTORY),
            Ok(_) => Ok(FILE_INFO_INVALID),
            Err(_) if is_mrp_file_name(name) => Ok(FILE_INFO_INVALID),
            Err(_) => Ok(if self.read_current_package_file(name)?.is_some() {
                FILE_INFO_FILE
            } else {
                FILE_INFO_INVALID
            }),
        }
    }

    fn remove_file(&mut self, name: &[u8]) -> Result<i32> {
        let Some(path) = self.file_path(name) else {
            return Ok(-1);
        };
        Ok(if fs::remove_file(path).is_ok() { 0 } else { -1 })
    }

    fn rename_file(&mut self, source: &[u8], destination: &[u8]) -> Result<i32> {
        let (Some(source), Some(destination)) =
            (self.file_path(source), self.file_path(destination))
        else {
            return Ok(-1);
        };
        Ok(if fs::rename(source, destination).is_ok() {
            0
        } else {
            -1
        })
    }

    fn create_dir(&mut self, name: &[u8]) -> Result<i32> {
        let Some(path) = self.file_path(name) else {
            return Ok(-1);
        };
        Ok(if fs::create_dir(path).is_ok() { 0 } else { -1 })
    }

    fn remove_dir(&mut self, name: &[u8]) -> Result<i32> {
        let Some(path) = self.file_path(name) else {
            return Ok(-1);
        };
        Ok(if fs::remove_dir(path).is_ok() { 0 } else { -1 })
    }

    fn open_file(&mut self, name: &[u8], mode: u32) -> Result<i32> {
        let Some(path) = self.file_path(name) else {
            return Ok(-1);
        };
        if mode & !0x3f != 0 {
            return Err(crate::Error::Abi(format!(
                "unsupported native file open mode {mode}"
            )));
        }
        let read = mode & 1 != 0 || mode & 4 != 0;
        let write = mode & 2 != 0 || mode & 4 != 0;
        if !read && !write {
            return Ok(-1);
        }
        if mode & 8 != 0 {
            let Some(parent) = path.parent() else {
                return Ok(-1);
            };
            if fs::create_dir_all(parent).is_err() {
                return Ok(-1);
            }
        }
        let host_file = OpenOptions::new()
            .read(read)
            .write(write)
            .create(mode & 8 != 0)
            .open(path);
        let file = match host_file {
            Ok(file) => NativeFile::Host(file),
            Err(_) if read && !write && mode & 8 == 0 => {
                let Some(bytes) = self.read_current_package_file(name)? else {
                    return Ok(-1);
                };
                NativeFile::Package(Cursor::new(bytes))
            }
            Err(_) => return Ok(-1),
        };
        let start = *self.next_file_handle;
        loop {
            let handle = *self.next_file_handle;
            *self.next_file_handle = self.next_file_handle.checked_add(1).unwrap_or(1);
            if let std::collections::btree_map::Entry::Vacant(entry) = self.files.entry(handle) {
                entry.insert(file);
                return Ok(handle);
            }
            if *self.next_file_handle == start {
                return Err(crate::Error::ResourceLimit(
                    "no native file handles available".into(),
                ));
            }
        }
    }

    fn close_file(&mut self, handle: i32) -> Result<i32> {
        Ok(if self.files.remove(&handle).is_some() {
            0
        } else {
            -1
        })
    }

    fn write_file(&mut self, handle: i32, bytes: &[u8]) -> Result<Option<usize>> {
        let Some(file) = self.files.get_mut(&handle) else {
            return Ok(None);
        };
        Ok(match file {
            NativeFile::Host(file) => file.write(bytes).ok(),
            NativeFile::Package(_) => None,
        })
    }

    fn read_file(&mut self, handle: i32, len: usize) -> Result<Option<Vec<u8>>> {
        let Some(file) = self.files.get_mut(&handle) else {
            return Ok(None);
        };
        let mut bytes = vec![0; len];
        let result = match file {
            NativeFile::Host(file) => file.read(&mut bytes),
            NativeFile::Package(file) => file.read(&mut bytes),
        };
        match result {
            Ok(read) => {
                bytes.truncate(read);
                Ok(Some(bytes))
            }
            Err(_) => Ok(None),
        }
    }

    fn seek_file(&mut self, handle: i32, offset: i32, origin: u32) -> Result<bool> {
        let Some(file) = self.files.get_mut(&handle) else {
            return Ok(false);
        };
        let position = match origin {
            0 => SeekFrom::Start(offset as u32 as u64),
            1 => SeekFrom::Current(i64::from(offset)),
            2 => SeekFrom::End(i64::from(offset)),
            _ => return Ok(false),
        };
        Ok(match file {
            NativeFile::Host(file) => file.seek(position).is_ok(),
            NativeFile::Package(file) => file.seek(position).is_ok(),
        })
    }

    fn file_len(&mut self, name: &[u8]) -> Result<Option<u64>> {
        let Some(path) = self.file_path(name) else {
            return Ok(None);
        };
        if let Ok(metadata) = fs::metadata(path) {
            return Ok(metadata.is_file().then_some(metadata.len()));
        }
        Ok(self
            .read_current_package_file(name)?
            .and_then(|bytes| u64::try_from(bytes.len()).ok()))
    }

    fn find_start(&mut self, directory: &[u8]) -> Result<Option<(i32, Vec<u8>)>> {
        let Some(path) = safe_work_path(&self.work_dir, directory) else {
            return Ok(None);
        };
        let Ok(entries) = fs::read_dir(path) else {
            return Ok(None);
        };
        let mut names = entries
            .filter_map(|entry| entry.ok())
            .map(|entry| {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                let (encoded, _, _) = GBK.encode(&name);
                Arc::<[u8]>::from(encoded.as_ref())
            })
            .collect::<Vec<_>>();
        names.sort();

        let start = *self.next_directory_handle;
        let handle = loop {
            let handle = *self.next_directory_handle;
            *self.next_directory_handle = self.next_directory_handle.checked_add(1).unwrap_or(1);
            if !self.directory_searches.contains_key(&handle) {
                break handle;
            }
            if *self.next_directory_handle == start {
                return Err(crate::Error::ResourceLimit(
                    "no directory search handles available".into(),
                ));
            }
        };
        let first = names.first().map_or_else(Vec::new, |name| name.to_vec());
        self.directory_searches.insert(
            handle,
            DirectorySearch {
                entries: names,
                next: 1,
            },
        );
        Ok(Some((handle, first)))
    }

    fn find_next(&mut self, handle: i32) -> Result<Option<Vec<u8>>> {
        let Some(search) = self.directory_searches.get_mut(&handle) else {
            return Ok(None);
        };
        let Some(name) = search.entries.get(search.next) else {
            return Ok(None);
        };
        search.next += 1;
        Ok(Some(name.to_vec()))
    }

    fn find_stop(&mut self, handle: i32) -> Result<bool> {
        Ok(self.directory_searches.remove(&handle).is_some())
    }

    fn char_bitmap(&mut self, codepoint: u32, _font: u32) -> Result<Option<(Vec<u8>, u32, u32)>> {
        let Some(start) = usize::try_from(codepoint)
            .ok()
            .and_then(|codepoint| codepoint.checked_mul(32))
        else {
            return Ok(None);
        };
        let Some(bitmap) = self.font.get(start..start + 32) else {
            return Ok(None);
        };
        let width = if codepoint < 128 { 8 } else { 16 };
        Ok(Some((bitmap.to_vec(), width, 16)))
    }

    fn draw_bitmap(
        &mut self,
        pixels: &[u8],
        x: i32,
        y: i32,
        width: usize,
        height: usize,
    ) -> Result<()> {
        for row in 0..height {
            for column in 0..width {
                let offset = (row * width + column) * 2;
                let color = u16::from_le_bytes([pixels[offset], pixels[offset + 1]]);
                self.framebuffer
                    .point(x + column as i32, y + row as i32, color);
            }
        }
        self.framebuffer.mark_presented();
        self.display.present(self.framebuffer)
    }
}
