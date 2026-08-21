use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
    sync::Arc,
};

use encoding_rs::GBK;
use flate2::read::GzDecoder;
use serde::Serialize;

use crate::error::{Error, Result};

const HEADER_MIN_LEN: usize = 0xd1;

#[derive(Clone, Debug)]
pub struct ResourceLimits {
    pub max_package_len: usize,
    pub max_entries: usize,
    pub max_name_len: usize,
    pub max_stored_file_len: usize,
    pub max_expanded_file_len: usize,
    pub max_total_expanded_len: usize,
    pub max_mr_prototypes: usize,
    pub max_mr_depth: usize,
    pub max_mr_items: usize,
    pub max_mr_string_len: usize,
    pub max_mr_instructions: u64,
}

impl Default for ResourceLimits {
    fn default() -> Self {
        Self {
            max_package_len: 64 * 1024 * 1024,
            max_entries: 4096,
            max_name_len: 1024,
            max_stored_file_len: 32 * 1024 * 1024,
            max_expanded_file_len: 64 * 1024 * 1024,
            max_total_expanded_len: 128 * 1024 * 1024,
            max_mr_prototypes: 16_384,
            max_mr_depth: 256,
            max_mr_items: 4 * 1024 * 1024,
            max_mr_string_len: 16 * 1024 * 1024,
            max_mr_instructions: 20_000_000,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct PackageHeader {
    pub file_start: u32,
    pub declared_len: u32,
    pub list_start: u32,
    pub internal_name: Vec<u8>,
    pub display_name: Vec<u8>,
    pub app_id: u32,
    pub version: u32,
    pub flags: u32,
    pub screen_width: u16,
    pub screen_height: u16,
    pub platform: u8,
}

impl PackageHeader {
    pub fn display_name(&self) -> String {
        let (decoded, _, _) = GBK.decode(&self.display_name);
        decoded.into_owned()
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct PackageEntry {
    pub name: Vec<u8>,
    pub payload_offset: u32,
    pub stored_len: u32,
    pub reserved: u32,
    pub compressed: bool,
}

impl PackageEntry {
    pub fn display_name(&self) -> String {
        String::from_utf8_lossy(&self.name).into_owned()
    }
}

#[derive(Clone, Debug)]
pub struct Package {
    path: PathBuf,
    bytes: Arc<[u8]>,
    header: PackageHeader,
    entries: Vec<PackageEntry>,
    limits: ResourceLimits,
}

impl Package {
    pub fn open(path: impl AsRef<Path>, limits: ResourceLimits) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let bytes = fs::read(&path).map_err(|source| Error::Io {
            path: path.clone(),
            source,
        })?;
        Self::parse(path, bytes.into(), limits)
    }

    pub fn parse(path: PathBuf, bytes: Arc<[u8]>, limits: ResourceLimits) -> Result<Self> {
        if bytes.len() > limits.max_package_len {
            return Err(Error::ResourceLimit(format!(
                "package is {} bytes (limit {})",
                bytes.len(),
                limits.max_package_len
            )));
        }
        if bytes.len() < HEADER_MIN_LEN {
            return Err(Error::Package(format!(
                "header is truncated: {} bytes",
                bytes.len()
            )));
        }
        if &bytes[0..4] != b"MRPG" {
            return Err(Error::Package("bad MRPG signature".into()));
        }

        let file_start = read_u32(&bytes, 4)?;
        let declared_len = read_u32(&bytes, 8)?;
        let list_start = read_u32(&bytes, 12)?;
        let actual_end = bytes.len();
        let declared_end = usize::try_from(declared_len)
            .map_err(|_| Error::Package("declared length does not fit the host".into()))?;
        if declared_end > actual_end {
            return Err(Error::Package(format!(
                "declared length {declared_end} exceeds actual length {actual_end}"
            )));
        }
        let directory_end = usize::try_from(file_start)
            .ok()
            .and_then(|value| value.checked_add(8))
            .ok_or_else(|| Error::Package("directory end overflow".into()))?;
        let mut cursor = usize::try_from(list_start)
            .map_err(|_| Error::Package("directory start does not fit the host".into()))?;
        if cursor < 0xd0 || cursor > directory_end || directory_end > declared_end {
            return Err(Error::Package(format!(
                "invalid directory range {cursor:#x}..{directory_end:#x}"
            )));
        }

        let header = PackageHeader {
            file_start,
            declared_len,
            list_start,
            internal_name: fixed_string(&bytes[0x10..0x1c]),
            display_name: fixed_string(&bytes[0x1c..0x34]),
            app_id: read_u32(&bytes, 0x44)?,
            version: read_u32(&bytes, 0x48)?,
            flags: read_u32(&bytes, 0x4c)?,
            screen_width: read_u16(&bytes, 0xcc)?,
            screen_height: read_u16(&bytes, 0xce)?,
            platform: bytes[0xd0],
        };

        let mut entries = Vec::new();
        while cursor < directory_end {
            if entries.len() >= limits.max_entries {
                return Err(Error::ResourceLimit(format!(
                    "more than {} directory entries",
                    limits.max_entries
                )));
            }
            let name_len = usize::try_from(read_u32(&bytes, cursor)?)
                .map_err(|_| Error::Package("entry name length does not fit the host".into()))?;
            if name_len == 0 || name_len > limits.max_name_len {
                return Err(Error::ResourceLimit(format!(
                    "invalid entry name length {name_len}"
                )));
            }
            let name_start = cursor
                .checked_add(4)
                .ok_or_else(|| Error::Package("entry offset overflow".into()))?;
            let fields_start = name_start
                .checked_add(name_len)
                .ok_or_else(|| Error::Package("entry name range overflow".into()))?;
            let next = fields_start
                .checked_add(12)
                .ok_or_else(|| Error::Package("directory entry range overflow".into()))?;
            if next > directory_end {
                return Err(Error::Package(format!(
                    "directory entry at {cursor:#x} crosses directory end"
                )));
            }
            let mut name = bytes[name_start..fields_start].to_vec();
            if name.last() == Some(&0) {
                name.pop();
            }
            let payload_offset = read_u32(&bytes, fields_start)?;
            let stored_len = read_u32(&bytes, fields_start + 4)?;
            let reserved = read_u32(&bytes, fields_start + 8)?;
            let stored_len_usize = usize::try_from(stored_len)
                .map_err(|_| Error::Package("stored length does not fit the host".into()))?;
            if stored_len_usize > limits.max_stored_file_len {
                return Err(Error::ResourceLimit(format!(
                    "entry {} stores {stored_len_usize} bytes",
                    String::from_utf8_lossy(&name)
                )));
            }
            let payload_start = usize::try_from(payload_offset)
                .map_err(|_| Error::Package("payload offset does not fit the host".into()))?;
            let payload_end = payload_start
                .checked_add(stored_len_usize)
                .ok_or_else(|| Error::Package("payload range overflow".into()))?;
            if payload_end > declared_end {
                return Err(Error::Package(format!(
                    "entry {} payload {payload_start:#x}..{payload_end:#x} is outside the package",
                    String::from_utf8_lossy(&name)
                )));
            }
            let compressed = bytes
                .get(payload_start..payload_start.saturating_add(3))
                .is_some_and(|magic| magic == [0x1f, 0x8b, 0x08]);
            entries.push(PackageEntry {
                name,
                payload_offset,
                stored_len,
                reserved,
                compressed,
            });
            cursor = next;
        }
        if cursor != directory_end {
            return Err(Error::Package(
                "directory did not end on an entry boundary".into(),
            ));
        }

        Ok(Self {
            path,
            bytes,
            header,
            entries,
            limits,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn header(&self) -> &PackageHeader {
        &self.header
    }

    pub fn entries(&self) -> &[PackageEntry] {
        &self.entries
    }

    pub fn find_unique(&self, name: &[u8]) -> Result<&PackageEntry> {
        let mut matches = self.entries.iter().filter(|entry| entry.name == name);
        let entry = matches
            .next()
            .ok_or_else(|| Error::EntryNotFound(String::from_utf8_lossy(name).into_owned()))?;
        if matches.next().is_some() {
            return Err(Error::AmbiguousEntry(
                String::from_utf8_lossy(name).into_owned(),
            ));
        }
        Ok(entry)
    }

    /// Resolves a resource using the baseline SDK overlay rule.
    ///
    /// Updated packages can prepend replacement entries before the retained
    /// original directory. Runtime lookup observes the first matching entry,
    /// while `find_unique` remains available for ambiguity diagnostics.
    pub fn resolve(&self, name: &[u8]) -> Result<&PackageEntry> {
        self.entries
            .iter()
            .find(|entry| entry.name == name)
            .ok_or_else(|| Error::EntryNotFound(String::from_utf8_lossy(name).into_owned()))
    }

    pub fn read_entry(&self, entry: &PackageEntry) -> Result<Vec<u8>> {
        let start = entry.payload_offset as usize;
        let end = start + entry.stored_len as usize;
        let stored = &self.bytes[start..end];
        if !entry.compressed {
            return Ok(stored.to_vec());
        }

        let max = self.limits.max_expanded_file_len;
        let mut decoder = GzDecoder::new(stored).take((max as u64).saturating_add(1));
        let mut output = Vec::new();
        decoder
            .read_to_end(&mut output)
            .map_err(|error| Error::Package(format!("invalid gzip payload: {error}")))?;
        if output.len() > max {
            return Err(Error::ResourceLimit(format!(
                "expanded file exceeds {max} bytes"
            )));
        }
        Ok(output)
    }

    pub fn read_named(&self, name: &[u8]) -> Result<Vec<u8>> {
        self.read_entry(self.resolve(name)?)
    }

    pub fn read_raw_range(&self, offset: usize, len: usize) -> Result<Vec<u8>> {
        let end = offset
            .checked_add(len)
            .ok_or_else(|| Error::Package("raw package range overflow".into()))?;
        self.bytes
            .get(offset..end)
            .map(<[u8]>::to_vec)
            .ok_or_else(|| {
                Error::Package(format!(
                    "raw package range {offset:#x}..{end:#x} exceeds {} bytes",
                    self.bytes.len()
                ))
            })
    }
}

fn fixed_string(bytes: &[u8]) -> Vec<u8> {
    let end = bytes
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(bytes.len());
    bytes[..end].to_vec()
}

fn read_u16(bytes: &[u8], offset: usize) -> Result<u16> {
    let raw = bytes
        .get(offset..offset + 2)
        .ok_or_else(|| Error::Package(format!("truncated u16 at {offset:#x}")))?;
    Ok(u16::from_le_bytes([raw[0], raw[1]]))
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32> {
    let raw = bytes
        .get(offset..offset + 4)
        .ok_or_else(|| Error::Package(format!("truncated u32 at {offset:#x}")))?;
    Ok(u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_a_truncated_header() {
        let error = Package::parse(
            PathBuf::from("bad.mrp"),
            Arc::from(&b"MRPG"[..]),
            ResourceLimits::default(),
        )
        .unwrap_err();
        assert!(error.to_string().contains("header is truncated"));
    }

    #[test]
    fn rejects_a_payload_outside_declared_length() {
        let mut bytes = vec![0_u8; 300];
        bytes[0..4].copy_from_slice(b"MRPG");
        bytes[4..8].copy_from_slice(&257_u32.to_le_bytes());
        bytes[8..12].copy_from_slice(&300_u32.to_le_bytes());
        bytes[12..16].copy_from_slice(&240_u32.to_le_bytes());
        bytes[240..244].copy_from_slice(&2_u32.to_le_bytes());
        bytes[244..246].copy_from_slice(b"x\0");
        bytes[246..250].copy_from_slice(&299_u32.to_le_bytes());
        bytes[250..254].copy_from_slice(&2_u32.to_le_bytes());

        let error = Package::parse(
            PathBuf::from("bad.mrp"),
            bytes.into(),
            ResourceLimits::default(),
        )
        .unwrap_err();
        assert!(error.to_string().contains("outside the package"));
    }

    #[test]
    fn raw_range_is_checked() {
        let mut bytes = vec![0_u8; HEADER_MIN_LEN];
        bytes[0..4].copy_from_slice(b"MRPG");
        bytes[4..8].copy_from_slice(&((HEADER_MIN_LEN - 8) as u32).to_le_bytes());
        bytes[8..12].copy_from_slice(&(HEADER_MIN_LEN as u32).to_le_bytes());
        bytes[12..16].copy_from_slice(&(HEADER_MIN_LEN as u32).to_le_bytes());
        let package = Package::parse(
            PathBuf::from("range.mrp"),
            bytes.into(),
            ResourceLimits::default(),
        )
        .unwrap();

        assert_eq!(package.read_raw_range(0, 4).unwrap(), b"MRPG");
        assert!(package.read_raw_range(HEADER_MIN_LEN - 1, 2).is_err());
        assert!(package.read_raw_range(usize::MAX, 2).is_err());
    }

    #[test]
    fn baseline_resolution_uses_the_first_duplicate_entry() {
        const LIST_START: usize = 240;
        const ENTRY_LEN: usize = 25;
        let mut bytes = vec![0_u8; 400];
        bytes[0..4].copy_from_slice(b"MRPG");
        bytes[4..8].copy_from_slice(&282_u32.to_le_bytes());
        bytes[8..12].copy_from_slice(&400_u32.to_le_bytes());
        bytes[12..16].copy_from_slice(&(LIST_START as u32).to_le_bytes());
        for (index, (payload_offset, payload)) in [(320_u32, b"old"), (323_u32, b"new")]
            .into_iter()
            .enumerate()
        {
            let offset = LIST_START + index * ENTRY_LEN;
            bytes[offset..offset + 4].copy_from_slice(&9_u32.to_le_bytes());
            bytes[offset + 4..offset + 13].copy_from_slice(b"start.mr\0");
            bytes[offset + 13..offset + 17].copy_from_slice(&payload_offset.to_le_bytes());
            bytes[offset + 17..offset + 21].copy_from_slice(&3_u32.to_le_bytes());
            bytes[payload_offset as usize..payload_offset as usize + 3].copy_from_slice(payload);
        }

        let package = Package::parse(
            PathBuf::from("duplicate.mrp"),
            bytes.into(),
            ResourceLimits::default(),
        )
        .unwrap();
        assert!(matches!(
            package.find_unique(b"start.mr"),
            Err(Error::AmbiguousEntry(_))
        ));
        assert_eq!(package.read_named(b"start.mr").unwrap(), b"old");
    }
}
