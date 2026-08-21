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
        self.allocate_heap_block(len, alignment)?.ok_or_else(|| {
            Error::ArmFault(format!("guest heap exhausted while allocating {len} bytes"))
        })
    }

    pub(super) fn allocate_guest_block(&mut self, len: usize) -> Result<Option<GuestAddr>> {
        let required = aligned_heap_len(len)?;
        self.allocate_heap_block(required, HEAP_ALIGNMENT)
    }

    pub(super) fn allocate_heap_block(
        &mut self,
        required: u32,
        alignment: u32,
    ) -> Result<Option<GuestAddr>> {
        let heap = self.guest_heap_state()?;
        let (mut blocks, terminator) = self.read_free_blocks(heap)?;
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
        let free_left = heap.free_left.checked_sub(consumed).ok_or_else(|| {
            Error::Abi(format!(
                "guest free-byte count {:#x} is smaller than allocation cost {consumed:#x}",
                heap.free_left
            ))
        })?;
        blocks.splice(index..=index, replacement);
        self.write_free_blocks(heap, &blocks, terminator, free_left)?;
        let address = GuestAddr(heap.base.wrapping_add(start));
        self.memory.write(address, &vec![0; required as usize])?;
        Ok(Some(address))
    }

    pub(super) fn free_guest_block(&mut self, address: GuestAddr, len: usize) -> Result<()> {
        if address.0 == 0 {
            return Ok(());
        }
        self.clear_freed_ram_package(address, len)?;
        if len < FREE_BLOCK_HEADER_LEN as usize {
            return Ok(());
        }
        let block_len = aligned_heap_len(len)?;
        let heap = self.guest_heap_state()?;
        let offset = address.0.wrapping_sub(heap.base);
        let end = offset
            .checked_add(block_len)
            .ok_or_else(|| Error::Abi("freed guest block offset overflow".into()))?;
        if offset >= heap.span || end > heap.span || offset % HEAP_ALIGNMENT != 0 {
            return Err(Error::Abi(format!(
                "freed guest block {:#010x} ({} bytes) is outside the active heap {:#010x}..{:#010x}",
                address.0,
                len,
                heap.base,
                heap.base.wrapping_add(heap.span),
            )));
        }

        let (mut blocks, mut terminator) = self.read_free_blocks(heap)?;
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
            .checked_add(block_len)
            .ok_or_else(|| Error::Abi("guest free-byte count overflow".into()))?;
        self.write_free_blocks(heap, &merged, terminator, free_left)
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

    pub(super) fn read_free_blocks(&self, heap: GuestHeapState) -> Result<(Vec<FreeBlock>, u32)> {
        let mut blocks = Vec::new();
        let mut seen = BTreeSet::new();
        let mut offset = heap.head;
        loop {
            if offset == heap.span {
                return Ok((blocks, offset));
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
                return Ok((blocks, offset));
            }
            let block_end = offset
                .checked_add(len)
                .ok_or_else(|| Error::Abi("guest free-block range overflow".into()))?;
            if len < FREE_BLOCK_HEADER_LEN || block_end > heap.span {
                return Err(Error::Abi(format!(
                    "guest free block at offset {offset:#x} has invalid length {len:#x} for heap span {:#x}",
                    heap.span
                )));
            }
            if next <= offset || next > heap.span {
                return Err(Error::Abi(format!(
                    "guest free block at offset {offset:#x} has invalid next offset {next:#x}"
                )));
            }
            if next != heap.span && block_end > next {
                return Err(Error::Abi(format!(
                    "guest free block {offset:#x}..{block_end:#x} overlaps its successor {next:#x}"
                )));
            }
            blocks.push(FreeBlock { offset, len });
            offset = next;
        }
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
        let head = blocks.first().map_or(terminator, |block| block.offset);
        for (index, block) in blocks.iter().copied().enumerate() {
            let next = blocks
                .get(index + 1)
                .map_or(terminator, |block| block.offset);
            let block_end = block
                .offset
                .checked_add(block.len)
                .ok_or_else(|| Error::Abi("guest free-block range overflow".into()))?;
            if block_end > next {
                return Err(Error::Abi(format!(
                    "guest free block {:#x}..{block_end:#x} exceeds its successor {next:#x}",
                    block.offset
                )));
            }
            let address = GuestAddr(heap.base.wrapping_add(block.offset));
            self.memory.write_u32(address, next)?;
            self.memory.write_u32(address.checked_add(4)?, block.len)?;
        }
        self.memory.write_u32(heap.head_variable, head)?;
        self.memory.write_u32(heap.free_left_variable, free_left)
    }
}

pub(super) fn aligned_heap_len(len: usize) -> Result<u32> {
    let len = u32::try_from(len.max(1))
        .map_err(|_| Error::ArmFault(format!("guest allocation length {len} does not fit u32")))?;
    len.max(FREE_BLOCK_HEADER_LEN)
        .checked_add(HEAP_ALIGNMENT - 1)
        .map(|value| value & !(HEAP_ALIGNMENT - 1))
        .ok_or_else(|| Error::ArmFault("guest allocation length alignment overflow".into()))
}
