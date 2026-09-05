use crate::{Error, Result};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
#[repr(transparent)]
pub struct GuestAddr(pub u32);

impl GuestAddr {
    pub fn checked_add(self, offset: u32) -> Result<Self> {
        self.0.checked_add(offset).map(Self).ok_or_else(|| {
            Error::ArmFault(format!("guest address overflow: {self:?} + {offset:#x}"))
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Permissions(u8);

impl Permissions {
    pub const READ: Self = Self(1 << 0);
    pub const WRITE: Self = Self(1 << 1);
    pub const EXECUTE: Self = Self(1 << 2);
    pub const READ_WRITE: Self = Self(Self::READ.0 | Self::WRITE.0);
    pub const READ_EXECUTE: Self = Self(Self::READ.0 | Self::EXECUTE.0);
    pub const READ_WRITE_EXECUTE: Self = Self(Self::READ.0 | Self::WRITE.0 | Self::EXECUTE.0);

    fn contains(self, required: Self) -> bool {
        self.0 & required.0 == required.0
    }

    fn union(self, additional: Self) -> Self {
        Self(self.0 | additional.0)
    }

    fn without(self, removed: Self) -> Self {
        Self(self.0 & !removed.0)
    }
}

#[derive(Debug)]
struct Region {
    base: u32,
    bytes: Vec<u8>,
    permissions: Permissions,
    name: String,
    mapping_id: u64,
    mapping_base: u32,
    mapping_len: usize,
}

impl Region {
    fn end(&self) -> u64 {
        u64::from(self.base) + self.bytes.len() as u64
    }
}

#[derive(Debug, Default)]
pub struct GuestMemory {
    regions: Vec<Region>,
    next_mapping_id: u64,
}

impl GuestMemory {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn map(
        &mut self,
        base: GuestAddr,
        len: usize,
        permissions: Permissions,
        name: impl Into<String>,
    ) -> Result<()> {
        self.map_bytes(base, vec![0; len], permissions, name)
    }

    pub fn map_bytes(
        &mut self,
        base: GuestAddr,
        bytes: Vec<u8>,
        permissions: Permissions,
        name: impl Into<String>,
    ) -> Result<()> {
        if bytes.is_empty() {
            return Err(Error::ArmFault("cannot map an empty guest region".into()));
        }
        let end = u64::from(base.0)
            .checked_add(bytes.len() as u64)
            .ok_or_else(|| Error::ArmFault("guest mapping range overflow".into()))?;
        if end > u64::from(u32::MAX) + 1 {
            return Err(Error::ArmFault(format!(
                "guest mapping {:#x}..{end:#x} exceeds the 32-bit address space",
                base.0
            )));
        }
        if let Some(region) = self
            .regions
            .iter()
            .find(|region| u64::from(base.0) < region.end() && u64::from(region.base) < end)
        {
            return Err(Error::ArmFault(format!(
                "guest mapping {:#x}..{end:#x} overlaps {} at {:#x}..{:#x}",
                base.0,
                region.name,
                region.base,
                region.end()
            )));
        }
        let mapping_id = self.next_mapping_id;
        self.next_mapping_id = self
            .next_mapping_id
            .checked_add(1)
            .ok_or_else(|| Error::ArmFault("guest mapping identifier overflow".into()))?;
        self.regions.push(Region {
            base: base.0,
            bytes,
            permissions,
            name: name.into(),
            mapping_id,
            mapping_base: base.0,
            mapping_len: usize::try_from(end - u64::from(base.0))
                .expect("mapping length originated as usize"),
        });
        self.regions.sort_unstable_by_key(|region| region.base);
        Ok(())
    }

    pub fn read_u8(&self, address: GuestAddr) -> Result<u8> {
        Ok(self.range(address, 1, Permissions::READ)?[0])
    }

    pub fn read_u16(&self, address: GuestAddr) -> Result<u16> {
        let bytes = self.read_array(address, Permissions::READ)?;
        Ok(u16::from_le_bytes(bytes))
    }

    pub fn read_u32(&self, address: GuestAddr) -> Result<u32> {
        let bytes = self.read_array(address, Permissions::READ)?;
        Ok(u32::from_le_bytes(bytes))
    }

    pub fn fetch_u16(&self, address: GuestAddr) -> Result<u16> {
        let bytes = self.read_array(address, Permissions::EXECUTE)?;
        Ok(u16::from_le_bytes(bytes))
    }

    pub fn fetch_u32(&self, address: GuestAddr) -> Result<u32> {
        let bytes = self.read_array(address, Permissions::EXECUTE)?;
        Ok(u32::from_le_bytes(bytes))
    }

    pub fn read(&self, address: GuestAddr, len: usize) -> Result<Vec<u8>> {
        match self.range(address, len, Permissions::READ) {
            Ok(bytes) => Ok(bytes.to_vec()),
            Err(_) => self.read_checked(address, len, Permissions::READ),
        }
    }

    pub fn write_u8(&mut self, address: GuestAddr, value: u8) -> Result<()> {
        self.range_mut(address, 1, Permissions::WRITE)?[0] = value;
        Ok(())
    }

    pub fn write_u16(&mut self, address: GuestAddr, value: u16) -> Result<()> {
        self.write(address, &value.to_le_bytes())
    }

    pub fn write_u32(&mut self, address: GuestAddr, value: u32) -> Result<()> {
        self.write(address, &value.to_le_bytes())
    }

    pub fn write(&mut self, address: GuestAddr, bytes: &[u8]) -> Result<()> {
        if let Ok(target) = self.range_mut(address, bytes.len(), Permissions::WRITE) {
            target.copy_from_slice(bytes);
            return Ok(());
        }
        let segments = self.segments(address, bytes.len(), Permissions::WRITE)?;
        let mut source_offset = 0;
        for (index, offset, len) in segments {
            self.regions[index].bytes[offset..offset + len]
                .copy_from_slice(&bytes[source_offset..source_offset + len]);
            source_offset += len;
        }
        Ok(())
    }

    pub fn check_range(
        &self,
        address: GuestAddr,
        len: usize,
        permissions: Permissions,
    ) -> Result<()> {
        self.segments(address, len, permissions).map(|_| ())
    }

    /// Check whether the full range is mapped, regardless of its permissions.
    pub fn is_mapped(&self, address: GuestAddr, len: usize) -> bool {
        self.segments(address, len, Permissions(0)).is_ok()
    }

    pub fn add_permissions(
        &mut self,
        address: GuestAddr,
        len: usize,
        permissions: Permissions,
    ) -> Result<()> {
        self.change_permissions(address, len, permissions, true)
    }

    pub fn remove_permissions(
        &mut self,
        address: GuestAddr,
        len: usize,
        permissions: Permissions,
    ) -> Result<()> {
        self.change_permissions(address, len, permissions, false)
    }

    fn change_permissions(
        &mut self,
        address: GuestAddr,
        len: usize,
        permissions: Permissions,
        add: bool,
    ) -> Result<()> {
        if len == 0 {
            return Err(Error::ArmFault(
                "cannot change permissions for an empty guest range".into(),
            ));
        }

        let segments = self.segments(address, len, Permissions(0))?;
        let mapping_id = self.regions[segments[0].0].mapping_id;
        if segments
            .iter()
            .any(|(index, _, _)| self.regions[*index].mapping_id != mapping_id)
        {
            return Err(Error::ArmFault(format!(
                "permission range at {:#010x} ({} bytes) crosses guest mappings",
                address.0, len
            )));
        }

        let requested_start = u64::from(address.0);
        let requested_end = requested_start + len as u64;
        let mut replacements = Vec::with_capacity(self.regions.len() + segments.len() * 2);
        for region in std::mem::take(&mut self.regions) {
            let region_start = u64::from(region.base);
            let region_end = region.end();
            if requested_start >= region_end || region_start >= requested_end {
                replacements.push(region);
                continue;
            }

            let overlap_start = (requested_start.max(region_start) - region_start) as usize;
            let overlap_end = (requested_end.min(region_end) - region_start) as usize;
            let Region {
                base,
                mut bytes,
                permissions: current_permissions,
                name,
                mapping_id,
                mapping_base,
                mapping_len,
            } = region;
            let after = bytes.split_off(overlap_end);
            let middle = bytes.split_off(overlap_start);
            if !bytes.is_empty() {
                replacements.push(Region {
                    base,
                    bytes,
                    permissions: current_permissions,
                    name: name.clone(),
                    mapping_id,
                    mapping_base,
                    mapping_len,
                });
            }
            replacements.push(Region {
                base: base + overlap_start as u32,
                bytes: middle,
                permissions: if add {
                    current_permissions.union(permissions)
                } else {
                    current_permissions.without(permissions)
                },
                name: name.clone(),
                mapping_id,
                mapping_base,
                mapping_len,
            });
            if !after.is_empty() {
                replacements.push(Region {
                    base: base + overlap_end as u32,
                    bytes: after,
                    permissions: current_permissions,
                    name,
                    mapping_id,
                    mapping_base,
                    mapping_len,
                });
            }
        }
        self.regions = replacements;
        Ok(())
    }

    pub fn unmap(&mut self, base: GuestAddr, len: usize) -> Result<()> {
        let requested_end = u64::from(base.0)
            .checked_add(len as u64)
            .ok_or_else(|| Error::ArmFault("guest unmap range overflow".into()))?;
        let Some(first) = self.regions.iter().position(|region| region.base == base.0) else {
            return Err(Error::ArmFault(format!(
                "guest unmap does not match a region at {:#010x} ({} bytes)",
                base.0, len
            )));
        };
        let mapping_id = self.regions[first].mapping_id;
        if self.regions[first].mapping_base != base.0 || self.regions[first].mapping_len != len {
            return Err(Error::ArmFault(format!(
                "guest unmap does not match the original mapping at {:#010x} ({} bytes)",
                base.0, len
            )));
        }
        let mut cursor = u64::from(base.0);
        let mut after_last = first;
        while cursor < requested_end {
            let Some(region) = self.regions.get(after_last) else {
                break;
            };
            if region.mapping_id != mapping_id
                || u64::from(region.base) != cursor
                || region.end() > requested_end
            {
                break;
            }
            cursor = region.end();
            after_last += 1;
        }
        if cursor != requested_end || first == after_last {
            return Err(Error::ArmFault(format!(
                "guest unmap does not match contiguous regions at {:#010x} ({} bytes)",
                base.0, len
            )));
        }
        self.regions.drain(first..after_last);
        Ok(())
    }

    fn read_array<const N: usize>(
        &self,
        address: GuestAddr,
        required: Permissions,
    ) -> Result<[u8; N]> {
        match self.range(address, N, required) {
            Ok(bytes) => Ok(bytes.try_into().expect("checked fixed-size range")),
            Err(_) => Ok(self
                .read_checked(address, N, required)?
                .try_into()
                .expect("checked fixed-size segmented range")),
        }
    }

    fn range(&self, address: GuestAddr, len: usize, required: Permissions) -> Result<&[u8]> {
        let (region, offset) = self.locate(address, len, required)?;
        Ok(&region.bytes[offset..offset + len])
    }

    fn range_mut(
        &mut self,
        address: GuestAddr,
        len: usize,
        required: Permissions,
    ) -> Result<&mut [u8]> {
        let index = self.locate_index(address, len, required)?;
        let region = &mut self.regions[index];
        let offset = (address.0 - region.base) as usize;
        Ok(&mut region.bytes[offset..offset + len])
    }

    fn locate(
        &self,
        address: GuestAddr,
        len: usize,
        required: Permissions,
    ) -> Result<(&Region, usize)> {
        let index = self.locate_index(address, len, required)?;
        let region = &self.regions[index];
        Ok((region, (address.0 - region.base) as usize))
    }

    fn read_checked(
        &self,
        address: GuestAddr,
        len: usize,
        required: Permissions,
    ) -> Result<Vec<u8>> {
        let segments = self.segments(address, len, required)?;
        let mut bytes = Vec::with_capacity(len);
        for (index, offset, len) in segments {
            bytes.extend_from_slice(&self.regions[index].bytes[offset..offset + len]);
        }
        Ok(bytes)
    }

    fn segments(
        &self,
        address: GuestAddr,
        len: usize,
        required: Permissions,
    ) -> Result<Vec<(usize, usize, usize)>> {
        let end = u64::from(address.0)
            .checked_add(len as u64)
            .ok_or_else(|| Error::ArmFault("guest access range overflow".into()))?;
        if end > u64::from(u32::MAX) + 1 {
            return Err(Error::ArmFault(
                "guest access exceeds the 32-bit address space".into(),
            ));
        }

        let mut cursor = u64::from(address.0);
        let mut segments = Vec::new();
        while cursor < end {
            let Some((index, region)) = self
                .regions
                .iter()
                .enumerate()
                .find(|(_, region)| cursor >= u64::from(region.base) && cursor < region.end())
            else {
                return Err(Error::ArmFault(format!(
                    "unmapped guest access at {cursor:#010x} ({} bytes)",
                    end - cursor
                )));
            };
            if !region.permissions.contains(required) {
                return Err(Error::ArmFault(format!(
                    "permission fault in {} at {cursor:#010x} (need {:?}, have {:?})",
                    region.name, required, region.permissions
                )));
            }
            let segment_end = end.min(region.end());
            segments.push((
                index,
                (cursor - u64::from(region.base)) as usize,
                (segment_end - cursor) as usize,
            ));
            cursor = segment_end;
        }
        Ok(segments)
    }

    fn locate_index(&self, address: GuestAddr, len: usize, required: Permissions) -> Result<usize> {
        let end = u64::from(address.0)
            .checked_add(len as u64)
            .ok_or_else(|| Error::ArmFault("guest access range overflow".into()))?;
        let Some((index, region)) = self
            .regions
            .iter()
            .enumerate()
            .find(|(_, region)| address.0 >= region.base && end <= region.end())
        else {
            return Err(Error::ArmFault(format!(
                "unmapped guest access at {:#010x} ({} bytes)",
                address.0, len
            )));
        };
        if !region.permissions.contains(required) {
            return Err(Error::ArmFault(format!(
                "permission fault in {} at {:#010x} (need {:?}, have {:?})",
                region.name, address.0, required, region.permissions
            )));
        }
        Ok(index)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mappings_check_overlap_permissions_and_bounds() {
        let mut memory = GuestMemory::new();
        memory
            .map(GuestAddr(0x1000), 16, Permissions::READ_WRITE, "data")
            .unwrap();
        memory.write_u32(GuestAddr(0x1004), 0x7856_3412).unwrap();
        assert_eq!(memory.read_u32(GuestAddr(0x1004)).unwrap(), 0x7856_3412);
        assert!(memory.fetch_u32(GuestAddr(0x1004)).is_err());
        assert!(memory.read_u32(GuestAddr(0x100e)).is_err());
        assert!(
            memory
                .map(GuestAddr(0x1008), 8, Permissions::READ, "overlap")
                .is_err()
        );
    }

    #[test]
    fn adds_permissions_only_to_the_requested_range() {
        let mut memory = GuestMemory::new();
        memory
            .map(GuestAddr(0x1000), 16, Permissions::READ_WRITE, "heap")
            .unwrap();
        memory
            .write(GuestAddr(0x1000), &[0x11, 0x22, 0x33, 0x44, 0x55, 0x66])
            .unwrap();

        assert!(memory.fetch_u16(GuestAddr(0x1002)).is_err());
        memory
            .add_permissions(GuestAddr(0x1002), 4, Permissions::EXECUTE)
            .unwrap();

        assert_eq!(memory.fetch_u16(GuestAddr(0x1002)).unwrap(), 0x4433);
        assert_eq!(memory.fetch_u16(GuestAddr(0x1004)).unwrap(), 0x6655);
        assert!(memory.fetch_u16(GuestAddr(0x1000)).is_err());
        assert!(memory.fetch_u32(GuestAddr(0x1004)).is_err());
        assert!(memory.fetch_u16(GuestAddr(0x1006)).is_err());
        assert_eq!(
            memory.read(GuestAddr(0x1000), 8).unwrap(),
            [0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0, 0]
        );
        assert_eq!(memory.read_u32(GuestAddr(0x1002)).unwrap(), 0x6655_4433);
        memory.write_u32(GuestAddr(0x1000), 0x4030_2010).unwrap();
        assert_eq!(memory.read_u32(GuestAddr(0x1000)).unwrap(), 0x4030_2010);
        memory.write_u16(GuestAddr(0x1002), 0x8877).unwrap();
        assert_eq!(memory.read_u16(GuestAddr(0x1002)).unwrap(), 0x8877);
    }

    #[test]
    fn extends_permissions_across_regions_split_by_an_earlier_change() {
        let mut memory = GuestMemory::new();
        memory
            .map(GuestAddr(0x1000), 16, Permissions::READ_WRITE, "heap")
            .unwrap();
        memory
            .add_permissions(GuestAddr(0x1004), 4, Permissions::EXECUTE)
            .unwrap();

        memory
            .add_permissions(GuestAddr(0x1002), 10, Permissions::EXECUTE)
            .unwrap();

        assert!(memory.fetch_u16(GuestAddr(0x1000)).is_err());
        assert_eq!(memory.fetch_u16(GuestAddr(0x1002)).unwrap(), 0);
        assert_eq!(memory.fetch_u16(GuestAddr(0x100a)).unwrap(), 0);
        assert!(memory.fetch_u16(GuestAddr(0x100c)).is_err());
    }

    #[test]
    fn rejects_permission_changes_outside_one_mapped_region() {
        let mut memory = GuestMemory::new();
        memory
            .map(GuestAddr(0x1000), 8, Permissions::READ_WRITE, "same name")
            .unwrap();
        memory
            .map(GuestAddr(0x1008), 8, Permissions::READ_WRITE, "same name")
            .unwrap();

        assert!(
            memory
                .add_permissions(GuestAddr(0x1004), 8, Permissions::EXECUTE)
                .is_err()
        );
        assert!(
            memory
                .add_permissions(GuestAddr(0x0ffc), 8, Permissions::EXECUTE)
                .is_err()
        );
        assert!(
            memory
                .add_permissions(GuestAddr(0x1000), 0, Permissions::EXECUTE)
                .is_err()
        );
    }

    #[test]
    fn removes_permissions_only_from_the_requested_mapping_range() {
        let mut memory = GuestMemory::new();
        memory
            .map(GuestAddr(0x1000), 16, Permissions::READ_WRITE, "heap")
            .unwrap();
        memory
            .add_permissions(GuestAddr(0x1004), 4, Permissions::EXECUTE)
            .unwrap();
        assert_eq!(memory.fetch_u32(GuestAddr(0x1004)).unwrap(), 0);

        memory
            .remove_permissions(GuestAddr(0x1004), 4, Permissions::EXECUTE)
            .unwrap();

        assert!(memory.fetch_u16(GuestAddr(0x1004)).is_err());
        assert_eq!(memory.read_u32(GuestAddr(0x1004)).unwrap(), 0);
        memory.write_u32(GuestAddr(0x1004), 1).unwrap();
    }

    #[test]
    fn exact_mappings_can_be_unmapped() {
        let mut memory = GuestMemory::new();
        memory
            .map(GuestAddr(0x1000), 16, Permissions::READ_WRITE, "temporary")
            .unwrap();

        assert!(memory.unmap(GuestAddr(0x1000), 8).is_err());
        memory.unmap(GuestAddr(0x1000), 16).unwrap();
        assert!(memory.read_u8(GuestAddr(0x1000)).is_err());
    }

    #[test]
    fn mappings_split_by_permissions_can_be_unmapped_as_one_exact_range() {
        let mut memory = GuestMemory::new();
        memory
            .map(GuestAddr(0x1000), 16, Permissions::READ_WRITE, "temporary")
            .unwrap();
        memory
            .add_permissions(GuestAddr(0x1004), 4, Permissions::EXECUTE)
            .unwrap();

        assert!(memory.unmap(GuestAddr(0x1000), 8).is_err());
        assert!(memory.unmap(GuestAddr(0x1004), 4).is_err());
        memory.unmap(GuestAddr(0x1000), 16).unwrap();

        assert!(memory.read_u8(GuestAddr(0x1000)).is_err());
        assert!(memory.fetch_u16(GuestAddr(0x1004)).is_err());
    }

    #[test]
    fn unmap_rejects_gaps_and_partial_boundary_regions() {
        let mut memory = GuestMemory::new();
        memory
            .map(GuestAddr(0x1000), 8, Permissions::READ_WRITE, "first")
            .unwrap();
        memory
            .map(GuestAddr(0x1010), 8, Permissions::READ_WRITE, "second")
            .unwrap();

        assert!(memory.unmap(GuestAddr(0x1000), 24).is_err());
        assert!(memory.unmap(GuestAddr(0x1000), 4).is_err());
        assert!(memory.unmap(GuestAddr(0x1004), 4).is_err());
        assert_eq!(memory.read_u8(GuestAddr(0x1000)).unwrap(), 0);
        assert_eq!(memory.read_u8(GuestAddr(0x1010)).unwrap(), 0);
    }

    #[test]
    fn unmap_rejects_adjacent_independent_mappings() {
        let mut memory = GuestMemory::new();
        memory
            .map(GuestAddr(0x1000), 8, Permissions::READ_WRITE, "temporary")
            .unwrap();
        memory
            .map(GuestAddr(0x1008), 8, Permissions::READ_WRITE, "temporary")
            .unwrap();

        assert!(memory.unmap(GuestAddr(0x1000), 16).is_err());
        assert_eq!(memory.read_u8(GuestAddr(0x1000)).unwrap(), 0);
        assert_eq!(memory.read_u8(GuestAddr(0x1008)).unwrap(), 0);
    }
}
