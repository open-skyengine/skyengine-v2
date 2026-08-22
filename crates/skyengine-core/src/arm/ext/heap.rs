use super::*;

impl ExtRuntime {
    pub(super) fn allocate(&mut self, len: usize, alignment: u32) -> Result<GuestAddr> {
        if alignment == 0 || !alignment.is_power_of_two() || alignment > HEAP_ALIGNMENT {
            return Err(Error::ArmFault(format!(
                "unsupported guest heap alignment {alignment}"
            )));
        }
        let len = u32::try_from(len.max(1)).map_err(|_| {
            Error::ArmFault(format!("guest allocation length {len} does not fit u32"))
        })?;
        let heap_base = self.read_platform_data_slot(108)?;
        let heap_end = self.read_platform_data_slot(110)?;
        if heap_base == 0 && heap_end == 0 {
            let block_len = align_heap_len(len)?;
            return self
                .allocate_detached_guest_block(block_len)?
                .ok_or_else(|| {
                    Error::ArmFault(format!(
                        "detached guest memory exhausted while allocating {len} bytes"
                    ))
                });
        }
        let (address, consumed) = self.allocate_heap_block(len, alignment)?.ok_or_else(|| {
            Error::ArmFault(format!("guest heap exhausted while allocating {len} bytes"))
        })?;
        self.track_guest_heap_allocation(address, consumed);
        Ok(address)
    }

    pub(super) fn allocate_guest_block(&mut self, len: usize) -> Result<Option<GuestAddr>> {
        let block_len = aligned_heap_len(len)?;
        let heap_base = self.read_platform_data_slot(108)?;
        let heap_end = self.read_platform_data_slot(110)?;
        if heap_base == 0 && heap_end == 0 {
            return self.allocate_detached_guest_block(block_len);
        }
        let allocation = self.allocate_heap_block(block_len, HEAP_ALIGNMENT)?;
        if let Some((address, consumed)) = allocation {
            self.track_guest_heap_allocation(address, consumed);
        }
        Ok(allocation.map(|(address, _)| address))
    }

    fn track_guest_heap_allocation(&mut self, address: GuestAddr, block_len: u32) {
        let allocation_start = u64::from(address.0);
        let allocation_end = allocation_start + u64::from(block_len);
        self.guest_allocations.retain(|tracked_start, tracked_len| {
            let tracked_start = u64::from(*tracked_start);
            let tracked_end = tracked_start + u64::from(*tracked_len);
            tracked_end <= allocation_start || tracked_start >= allocation_end
        });
        self.guest_allocations.insert(address.0, block_len);
    }

    fn allocate_detached_guest_block(&mut self, block_len: u32) -> Result<Option<GuestAddr>> {
        let active_len = self
            .detached_guest_allocations
            .values()
            .try_fold(0_usize, |total, (len, _)| total.checked_add(*len))
            .ok_or_else(|| Error::ArmFault("detached guest allocation length overflow".into()))?;
        let block_len_usize = block_len as usize;
        if active_len
            .checked_add(block_len_usize)
            .is_none_or(|total| total > self.heap_len)
        {
            return Ok(None);
        }

        let previous_cursor = self.detached_guest_allocation_cursor;
        let address = GuestAddr(previous_cursor);
        let end = previous_cursor
            .checked_add(block_len)
            .ok_or_else(|| Error::ArmFault("detached guest allocation address overflow".into()))?;
        self.memory.map(
            address,
            block_len_usize,
            Permissions::READ_WRITE,
            "detached guest allocation",
        )?;
        self.detached_guest_allocation_cursor = end;
        self.detached_guest_allocations
            .insert(address.0, (block_len_usize, previous_cursor));
        Ok(Some(address))
    }

    pub(super) fn allocate_heap_block(
        &mut self,
        required: u32,
        alignment: u32,
    ) -> Result<Option<(GuestAddr, u32)>> {
        let heap = self.guest_heap_state()?;
        let (mut blocks, terminator, recovered_len) = self.read_free_blocks(heap)?;
        let mask = alignment - 1;
        let Some((index, start)) = blocks.iter().enumerate().find_map(|(index, block)| {
            let start = block.offset.checked_add(mask).map(|value| value & !mask)?;
            let end = start.checked_add(required)?;
            let block_end = block.offset.checked_add(block.len)?;
            (end <= block_end).then_some((index, start))
        }) else {
            return Ok(None);
        };

        let block = blocks[index];
        let allocation_end = start
            .checked_add(required)
            .ok_or_else(|| Error::Abi("guest allocation end overflow".into()))?;
        let block_end = block
            .offset
            .checked_add(block.len)
            .ok_or_else(|| Error::Abi("guest free-block end overflow".into()))?;
        let prefix_len = start - block.offset;
        let suffix_len = block_end - allocation_end;
        let mut replacement = Vec::with_capacity(2);
        if prefix_len >= FREE_BLOCK_HEADER_LEN {
            replacement.push(FreeBlock {
                offset: block.offset,
                len: prefix_len,
            });
        }
        if suffix_len >= FREE_BLOCK_HEADER_LEN {
            replacement.push(FreeBlock {
                offset: allocation_end,
                len: suffix_len,
            });
        }
        let replacement_len = replacement.iter().try_fold(0_u32, |total, block| {
            total
                .checked_add(block.len)
                .ok_or_else(|| Error::Abi("guest replacement free-byte count overflow".into()))
        })?;
        let consumed = block.len - replacement_len;
        let discarded_prefix = if prefix_len < FREE_BLOCK_HEADER_LEN {
            prefix_len
        } else {
            0
        };
        let reclaim_len = consumed
            .checked_sub(discarded_prefix)
            .ok_or_else(|| Error::Abi("guest reclaimable allocation length underflow".into()))?;
        let free_left = heap
            .free_left
            .checked_sub(recovered_len)
            .and_then(|free_left| free_left.checked_sub(consumed))
            .ok_or_else(|| {
                Error::Abi(format!(
                    "guest free-byte count {:#x} is smaller than allocation cost {:#x}",
                    heap.free_left,
                    recovered_len.saturating_add(consumed),
                ))
            })?;
        blocks.splice(index..=index, replacement);
        self.write_free_blocks(heap, &blocks, terminator, free_left)?;
        let address = GuestAddr(heap.base.wrapping_add(start));
        self.memory.write(address, &vec![0; required as usize])?;
        Ok(Some((address, reclaim_len)))
    }

    pub(super) fn free_guest_block(&mut self, address: GuestAddr, len: usize) -> Result<()> {
        if address.0 <= 1 {
            return Ok(());
        }
        if let Some((len, previous_cursor)) =
            self.detached_guest_allocations.get(&address.0).copied()
        {
            let end = address
                .0
                .checked_add(u32::try_from(len).map_err(|_| {
                    Error::Abi(format!(
                        "detached guest allocation length {len} exceeds u32"
                    ))
                })?)
                .ok_or_else(|| Error::Abi("detached guest allocation end overflow".into()))?;
            self.memory.unmap(address, len)?;
            self.detached_guest_allocations.remove(&address.0);
            if end == self.detached_guest_allocation_cursor {
                self.detached_guest_allocation_cursor = previous_cursor;
            }
            return Ok(());
        }
        let heap_base = self.read_platform_data_slot(108)?;
        let heap_end = self.read_platform_data_slot(110)?;
        if heap_base == 0 && heap_end == 0 {
            return Ok(());
        }
        let heap = self.guest_heap_state()?;
        let Some(offset) = address.0.checked_sub(heap.base) else {
            self.guest_allocations.remove(&address.0);
            return Ok(());
        };
        if offset >= heap.span {
            self.guest_allocations.remove(&address.0);
            return Ok(());
        }
        let block_len = match self.guest_allocations.get(&address.0).copied() {
            Some(block_len) => block_len,
            None if len != 0 => aligned_heap_len(len)?,
            None => {
                return Err(Error::Abi(format!(
                    "free references unknown guest allocation {:#010x}",
                    address.0
                )));
            }
        };
        let end = offset
            .checked_add(block_len)
            .ok_or_else(|| Error::Abi("freed guest block offset overflow".into()))?;
        if block_len < FREE_BLOCK_HEADER_LEN
            || offset >= heap.span
            || end > heap.span
            || offset % HEAP_ALIGNMENT != 0
        {
            return Err(Error::Abi(format!(
                "freed guest block {:#010x} ({} bytes) is outside the active heap {:#010x}..{:#010x}",
                address.0,
                block_len,
                heap.base,
                heap.base.wrapping_add(heap.span),
            )));
        }

        let (mut blocks, mut terminator, recovered_len) = self.read_free_blocks(heap)?;
        if blocks.iter().any(|block| {
            let block_end = block.offset + block.len;
            block.offset <= offset && end <= block_end
        }) {
            self.guest_allocations.remove(&address.0);
            return Ok(());
        }
        blocks.push(FreeBlock {
            offset,
            len: block_len,
        });
        if offset >= terminator || end > terminator {
            terminator = heap.span;
        }
        blocks.sort_unstable_by_key(|block| block.offset);

        let mut merged: Vec<FreeBlock> = Vec::with_capacity(blocks.len());
        for block in blocks {
            if let Some(previous) = merged.last_mut() {
                let previous_end = previous
                    .offset
                    .checked_add(previous.len)
                    .ok_or_else(|| Error::Abi("guest free-block end overflow".into()))?;
                if block.offset < previous_end {
                    return Err(Error::Abi(format!(
                        "freed guest block at offset {:#x} overlaps free block {:#x}..{:#x}",
                        block.offset, previous.offset, previous_end
                    )));
                }
                if block.offset == previous_end {
                    previous.len = previous.len.checked_add(block.len).ok_or_else(|| {
                        Error::Abi("merged guest free-block length overflow".into())
                    })?;
                    continue;
                }
            }
            merged.push(block);
        }
        let free_left = heap
            .free_left
            .checked_sub(recovered_len)
            .and_then(|free_left| free_left.checked_add(block_len))
            .ok_or_else(|| Error::Abi("guest free-byte count overflow".into()))?;
        self.clear_freed_ram_package(address, block_len as usize)?;
        self.write_free_blocks(heap, &merged, terminator, free_left)?;
        self.guest_allocations.remove(&address.0);
        Ok(())
    }

    pub(super) fn clear_freed_ram_package(&mut self, address: GuestAddr, len: usize) -> Result<()> {
        let ram_address = self.memory.read_u32(data_slot_address(104))?;
        if ram_address == 0 {
            return Ok(());
        }
        let free_start = u64::from(address.0);
        let free_end = free_start
            .checked_add(len as u64)
            .ok_or_else(|| Error::Abi("freed RAM package range overflow".into()))?;
        if u64::from(ram_address) >= free_start && u64::from(ram_address) < free_end {
            self.memory.write_u32(data_slot_address(104), 0)?;
            self.memory.write_u32(data_slot_address(105), 0)?;
        }
        Ok(())
    }

    pub(super) fn reserve_guest_heap_range(&mut self, address: GuestAddr, len: u32) -> Result<()> {
        let heap = self.guest_heap_state()?;
        let Some(offset) = address.0.checked_sub(heap.base) else {
            return Ok(());
        };
        if offset >= heap.span {
            return Ok(());
        }
        let end = offset
            .checked_add(len)
            .ok_or_else(|| Error::Abi("reserved guest range overflow".into()))?;
        if end > heap.span {
            return Err(Error::Abi(format!(
                "reserved guest range {:#010x}..{:#010x} exceeds the active heap",
                address.0,
                address.0.wrapping_add(len),
            )));
        }

        let (blocks, terminator, recovered_len) = self.read_free_blocks(heap)?;
        let mut retained = Vec::with_capacity(blocks.len() + 1);
        let mut reserved_len = 0_u32;
        for block in blocks {
            let block_end = block
                .offset
                .checked_add(block.len)
                .ok_or_else(|| Error::Abi("guest free-block range overflow".into()))?;
            let overlap_start = block.offset.max(offset);
            let overlap_end = block_end.min(end);
            if overlap_start >= overlap_end {
                retained.push(block);
                continue;
            }

            let prefix_len = overlap_start - block.offset;
            let suffix_len = block_end - overlap_end;
            let mut retained_len = 0_u32;
            if prefix_len >= FREE_BLOCK_HEADER_LEN {
                retained.push(FreeBlock {
                    offset: block.offset,
                    len: prefix_len,
                });
                retained_len += prefix_len;
            }
            if suffix_len >= FREE_BLOCK_HEADER_LEN {
                retained.push(FreeBlock {
                    offset: overlap_end,
                    len: suffix_len,
                });
                retained_len += suffix_len;
            }
            reserved_len = reserved_len
                .checked_add(block.len - retained_len)
                .ok_or_else(|| Error::Abi("reserved guest byte count overflow".into()))?;
        }
        if recovered_len == 0 && reserved_len == 0 {
            return Ok(());
        }
        let free_left = heap
            .free_left
            .checked_sub(recovered_len)
            .and_then(|free_left| free_left.checked_sub(reserved_len))
            .ok_or_else(|| Error::Abi("guest free-byte count underflow while reserving".into()))?;
        self.write_free_blocks(heap, &retained, terminator, free_left)
    }

    pub(super) fn guest_heap_state(&self) -> Result<GuestHeapState> {
        let base = self.read_platform_data_slot(108)?;
        let end = self.read_platform_data_slot(110)?;
        let span = end.wrapping_sub(base);
        if span < FREE_BLOCK_HEADER_LEN {
            return Err(Error::Abi(format!(
                "active guest heap {base:#010x}..{end:#010x} is too small"
            )));
        }
        let head_variable = self.platform_data_slot_address(146)?;
        let head = self.memory.read_u32(head_variable)?;
        if head > span {
            return Err(Error::Abi(format!(
                "guest free-list head {head:#x} exceeds heap span {span:#x}"
            )));
        }
        let free_left_variable = self.platform_data_slot_address(111)?;
        Ok(GuestHeapState {
            base,
            span,
            head,
            head_variable,
            free_left: self.memory.read_u32(free_left_variable)?,
            free_left_variable,
        })
    }

    pub(super) fn read_free_blocks(
        &self,
        heap: GuestHeapState,
    ) -> Result<(Vec<FreeBlock>, u32, u32)> {
        let mut blocks = Vec::new();
        let mut seen = BTreeSet::new();
        let mut offset = heap.head;
        let mut recovered_len = 0_u32;
        loop {
            if offset == heap.span {
                validate_free_block_ranges(&blocks, heap.span)?;
                return Ok((blocks, offset, recovered_len));
            }
            let header_end = offset
                .checked_add(FREE_BLOCK_HEADER_LEN)
                .ok_or_else(|| Error::Abi("guest free-block header overflow".into()))?;
            if header_end > heap.span {
                return Err(Error::Abi(format!(
                    "guest free-list offset {offset:#x} is outside heap span {:#x}",
                    heap.span
                )));
            }
            if !seen.insert(offset) {
                return Err(Error::Abi(format!(
                    "guest free-list contains a cycle at offset {offset:#x}"
                )));
            }
            let address = GuestAddr(heap.base.wrapping_add(offset));
            let next = self.memory.read_u32(address)?;
            let len = self.memory.read_u32(address.checked_add(4)?)?;
            if next == 0 && len == 0 {
                validate_free_block_ranges(&blocks, heap.span)?;
                return Ok((blocks, offset, recovered_len));
            }
            let block_end = offset
                .checked_add(len)
                .ok_or_else(|| Error::Abi("guest free-block range overflow".into()))?;
            if len < FREE_BLOCK_HEADER_LEN || block_end > heap.span {
                let (recovered, next, removed_len) = self
                    .recover_corrupted_free_header(heap, offset)
                    .ok_or_else(|| {
                        Error::Abi(format!(
                            "guest free block at offset {offset:#x} has invalid length {len:#x} for heap span {:#x}",
                            heap.span,
                        ))
                    })?;
                blocks.extend(recovered);
                recovered_len = recovered_len
                    .checked_add(removed_len)
                    .ok_or_else(|| Error::Abi("recovered guest free-byte count overflow".into()))?;
                offset = next;
                continue;
            }
            if next > heap.span {
                let (recovered, next, removed_len) = self
                    .recover_corrupted_free_header(heap, offset)
                    .ok_or_else(|| {
                        Error::Abi(format!(
                            "guest free block at offset {offset:#x} has invalid next offset {next:#x}"
                        ))
                    })?;
                blocks.extend(recovered);
                recovered_len = recovered_len
                    .checked_add(removed_len)
                    .ok_or_else(|| Error::Abi("recovered guest free-byte count overflow".into()))?;
                offset = next;
                continue;
            }
            blocks.push(FreeBlock { offset, len });
            offset = next;
        }
    }

    fn recover_corrupted_free_header(
        &self,
        heap: GuestHeapState,
        corrupted_offset: u32,
    ) -> Option<(Vec<FreeBlock>, u32, u32)> {
        let snapshot = self.guest_heap_snapshot.as_ref()?;
        if snapshot.base != heap.base
            || snapshot.span != heap.span
            || snapshot.head != heap.head
            || snapshot.free_left != heap.free_left
        {
            return None;
        }

        let ram_address = self.memory.read_u32(data_slot_address(104)).ok()?;
        let ram_len = self.memory.read_u32(data_slot_address(105)).ok()?;
        if ram_address == 0 || ram_len == 0 {
            return None;
        }
        let ram_start = ram_address.checked_sub(heap.base)? & !(HEAP_ALIGNMENT - 1);
        let ram_end = ram_address
            .checked_add(ram_len)?
            .checked_sub(heap.base)?
            .checked_add(HEAP_ALIGNMENT - 1)?
            & !(HEAP_ALIGNMENT - 1);
        if ram_end > heap.span || corrupted_offset < ram_start || corrupted_offset >= ram_end {
            return None;
        }

        let index = snapshot
            .blocks
            .iter()
            .position(|block| block.offset == corrupted_offset)?;
        let block = snapshot.blocks[index];
        let block_end = block.offset.checked_add(block.len)?;
        let overlap_end = block_end.min(ram_end);
        if overlap_end <= block.offset {
            return None;
        }
        let suffix_len = block_end - overlap_end;
        let recovered = if suffix_len >= FREE_BLOCK_HEADER_LEN {
            vec![FreeBlock {
                offset: overlap_end,
                len: suffix_len,
            }]
        } else {
            Vec::new()
        };
        let retained_len = recovered.first().map_or(0, |block| block.len);
        let removed_len = block.len.checked_sub(retained_len)?;
        let next = snapshot
            .blocks
            .get(index + 1)
            .map_or(snapshot.terminator, |block| block.offset);
        Some((recovered, next, removed_len))
    }

    pub(super) fn write_free_blocks(
        &mut self,
        heap: GuestHeapState,
        blocks: &[FreeBlock],
        terminator: u32,
        free_left: u32,
    ) -> Result<()> {
        if terminator > heap.span {
            return Err(Error::Abi(format!(
                "guest free-list terminator {terminator:#x} exceeds heap span {:#x}",
                heap.span
            )));
        }
        validate_free_block_ranges(blocks, heap.span)?;
        let head = blocks.first().map_or(terminator, |block| block.offset);
        for (index, block) in blocks.iter().copied().enumerate() {
            let next = blocks
                .get(index + 1)
                .map_or(terminator, |block| block.offset);
            let address = GuestAddr(heap.base.wrapping_add(block.offset));
            self.memory.write_u32(address, next)?;
            self.memory.write_u32(address.checked_add(4)?, block.len)?;
        }
        self.memory.write_u32(heap.head_variable, head)?;
        self.memory.write_u32(heap.free_left_variable, free_left)?;
        self.guest_heap_snapshot = Some(GuestHeapSnapshot {
            base: heap.base,
            span: heap.span,
            head,
            free_left,
            blocks: blocks.to_vec(),
            terminator,
        });
        Ok(())
    }
}

fn validate_free_block_ranges(blocks: &[FreeBlock], heap_span: u32) -> Result<()> {
    let mut ordered = blocks.to_vec();
    ordered.sort_unstable_by_key(|block| block.offset);
    let mut previous_end = 0;
    for block in ordered {
        let end = block
            .offset
            .checked_add(block.len)
            .ok_or_else(|| Error::Abi("guest free-block range overflow".into()))?;
        if end > heap_span {
            return Err(Error::Abi(format!(
                "guest free block at offset {:#x} exceeds heap span {heap_span:#x}",
                block.offset
            )));
        }
        if block.offset < previous_end {
            return Err(Error::Abi(format!(
                "guest free block at offset {:#x} overlaps the preceding range ending at {previous_end:#x}",
                block.offset
            )));
        }
        previous_end = end;
    }
    Ok(())
}

pub(super) fn aligned_heap_len(len: usize) -> Result<u32> {
    let len = u32::try_from(len.max(1))
        .map_err(|_| Error::ArmFault(format!("guest allocation length {len} does not fit u32")))?;
    align_heap_len(len.max(FREE_BLOCK_HEADER_LEN))
}

fn align_heap_len(len: u32) -> Result<u32> {
    len.checked_add(HEAP_ALIGNMENT - 1)
        .map(|value| value & !(HEAP_ALIGNMENT - 1))
        .ok_or_else(|| Error::ArmFault("guest allocation length alignment overflow".into()))
}
