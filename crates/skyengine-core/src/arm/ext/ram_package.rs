use super::*;

impl ExtRuntime {
    pub(super) fn read_ram_package_file(
        &self,
        address: GuestAddr,
        len: usize,
        name: &[u8],
    ) -> Result<Option<Vec<u8>>> {
        let image = self.memory.read(address, len)?;
        if image.len() < 24 || &image[..4] != b"MRPG" {
            return Err(Error::Package(
                "RAM-backed MRP is missing its 24-byte MRPG header".into(),
            ));
        }

        // Native wrappers use this compact one-file MRP while the current
        // package name is "$". The four-byte name precedes the stored length,
        // and the single payload follows the 24-byte header immediately.
        if read_le_u32(&image, 4)? == 4 && read_le_u32(&image, 12)? == 4 {
            let compact_name = image[16..20]
                .split(|byte| *byte == 0)
                .next()
                .unwrap_or_default();
            if compact_name.is_empty() || name != compact_name {
                return Ok(None);
            }
            let declared_len = read_le_u32(&image, 8)? as usize;
            let stored_len = read_le_u32(&image, 20)? as usize;
            let payload_end = 24_usize
                .checked_add(stored_len)
                .ok_or_else(|| Error::Package("compact RAM MRP payload range overflow".into()))?;
            if declared_len > image.len() || payload_end > declared_len {
                return Err(Error::Package(format!(
                    "compact RAM MRP payload 0x18..{payload_end:#x} exceeds declared length {declared_len}"
                )));
            }
            return expand_ram_payload(&image[24..payload_end], self.heap_len).map(Some);
        }

        let limits = ResourceLimits {
            max_package_len: self.heap_len,
            max_stored_file_len: self.heap_len,
            max_expanded_file_len: self.heap_len,
            max_total_expanded_len: self.heap_len,
            ..ResourceLimits::default()
        };
        let package = Package::parse(
            PathBuf::from("<guest-memory>.mrp"),
            Arc::from(image),
            limits,
        )?;
        match package.read_named(name) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(Error::EntryNotFound(_)) => Ok(None),
            Err(error) => Err(error),
        }
    }
}

pub(super) fn read_le_u32(bytes: &[u8], offset: usize) -> Result<u32> {
    let raw = bytes
        .get(offset..offset + 4)
        .ok_or_else(|| Error::Package(format!("truncated RAM MRP u32 at {offset:#x}")))?;
    Ok(u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]))
}

fn expand_ram_payload(stored: &[u8], limit: usize) -> Result<Vec<u8>> {
    if !stored.starts_with(&[0x1f, 0x8b, 0x08]) {
        return Ok(stored.to_vec());
    }
    let mut decoder = GzDecoder::new(stored).take((limit as u64).saturating_add(1));
    let mut output = Vec::new();
    decoder
        .read_to_end(&mut output)
        .map_err(|error| Error::Package(format!("invalid RAM MRP gzip payload: {error}")))?;
    if output.len() > limit {
        return Err(Error::ResourceLimit(format!(
            "expanded RAM MRP payload exceeds {limit} bytes"
        )));
    }
    Ok(output)
}
