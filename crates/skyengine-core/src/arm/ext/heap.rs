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
                .allocate_detached_guest_block(block_len, None)?
                .ok_or_else(|| {
                    Error::ArmFault(format!(
                        "detached guest memory exhausted while allocating {len} bytes"
                    ))
                });
        }
        let (address, consumed) = self.allocate_heap_block(len, alignment)?.ok_or_else(|| {
            Error::ArmFault(format!("guest heap exhausted while allocating {len} bytes"))
        })?;
        self.track_guest_heap_allocation(address, consumed, None)?;
        Ok(address)
    }

    #[cfg(test)]
    pub(super) fn allocate_guest_block(&mut self, len: usize) -> Result<Option<GuestAddr>> {
        self.allocate_guest_block_owned(len, None)
    }

    pub(super) fn allocate_guest_block_for_module(
        &mut self,
        len: usize,
        module: usize,
    ) -> Result<Option<GuestAddr>> {
        let owner_generation = self
            .modules
            .get(module)
            .map(|module| module.generation)
            .ok_or_else(|| Error::Abi(format!("allocation for missing module {module}")))?;
        self.allocate_guest_block_owned(len, Some(owner_generation))
    }

    pub(super) fn validate_guest_allocation_owner(
        &self,
        address: GuestAddr,
        len: usize,
        module: usize,
        operation: &str,
    ) -> Result<()> {
        if address.0 <= 1 {
            return Ok(());
        }
        let caller = self
            .modules
            .get(module)
            .map(|module| module.generation)
            .ok_or_else(|| Error::Abi(format!("{operation} for missing module {module}")))?;
        let requested_start = u64::from(address.0);
        if let Some(view) = self.guest_allocation_views.get(&address.0) {
            if view.owner_generation != caller {
                return Err(Error::Abi(format!(
                    "{operation} references an allocation view owned by another module"
                )));
            }
            return Ok(());
        }
        for (base, view) in &self.guest_allocation_views {
            let view_end = u64::from(*base) + u64::from(view.len);
            if requested_start > u64::from(*base) && requested_start < view_end {
                if view.owner_generation != caller {
                    return Err(Error::Abi(format!(
                        "{operation} references an allocation view owned by another module"
                    )));
                }
                return Err(Error::Abi(format!(
                    "{operation} must reference the start of a tracked allocation view"
                )));
            }
        }
        let mut tracked_ranges = Vec::with_capacity(
            self.guest_allocations.len() + self.detached_guest_allocations.len(),
        );
        tracked_ranges.extend(self.guest_allocations.iter().map(|(base, len)| {
            (
                *base,
                u64::from(*len),
                self.guest_allocation_owners.get(base).copied(),
            )
        }));
        for (base, (len, _)) in &self.detached_guest_allocations {
            tracked_ranges.push((
                *base,
                u64::try_from(*len).map_err(|_| {
                    Error::Abi(format!("{operation} allocation length does not fit u64"))
                })?,
                self.detached_guest_allocation_owners.get(base).copied(),
            ));
        }

        for (base, allocation_len, owner) in &tracked_ranges {
            let allocation_start = u64::from(*base);
            let allocation_end = allocation_start + *allocation_len;
            if requested_start < allocation_start || requested_start >= allocation_end {
                continue;
            }
            if owner.is_some_and(|owner| owner != caller) {
                return Err(Error::Abi(format!(
                    "{operation} references an allocation owned by another module"
                )));
            }
            if address.0 != *base {
                return Err(Error::Abi(format!(
                    "{operation} must reference the start of a tracked allocation"
                )));
            }
            // The allocator reconciles a stale tracked extent with the explicit
            // guest length. Once the base is exact, leave that choice to free/realloc.
            return Ok(());
        }

        let requested_len = if len == 0 {
            1
        } else {
            u64::from(aligned_heap_len(len)?)
        };
        let requested_end = requested_start
            .checked_add(requested_len)
            .filter(|end| *end <= u64::from(u32::MAX) + 1)
            .ok_or_else(|| Error::Abi(format!("{operation} range exceeds guest memory")))?;
        for (base, allocation_len, owner) in tracked_ranges {
            let allocation_start = u64::from(base);
            let allocation_end = allocation_start + allocation_len;
            if requested_start >= allocation_end || allocation_start >= requested_end {
                continue;
            }
            if owner.is_some_and(|owner| owner != caller) {
                return Err(Error::Abi(format!(
                    "{operation} references an allocation owned by another module"
                )));
            }
            return Err(Error::Abi(format!(
                "{operation} range overlaps a tracked allocation"
            )));
        }
        Ok(())
    }

    pub(super) fn free_guest_block_for_module(
        &mut self,
        address: GuestAddr,
        len: usize,
        module: usize,
    ) -> Result<()> {
        if let Some(view) = self.guest_allocation_views.get(&address.0).copied() {
            let caller = self
                .modules
                .get(module)
                .map(|module| module.generation)
                .ok_or_else(|| Error::Abi(format!("free for missing module {module}")))?;
            if view.owner_generation != caller {
                return Err(Error::Abi(
                    "free references an allocation view owned by another module".into(),
                ));
            }
            self.free_guest_allocation_view(address, view)?;
            self.guest_allocation_views.remove(&address.0);
            return Ok(());
        }
        if let Some(nested) = self.nested_guest_suballocation(address, len, module, "free")? {
            let heap = self.guest_heap_state()?;
            let (blocks, terminator, recovered_len) = self.read_free_blocks(heap)?;
            self.return_guest_heap_range(
                address,
                nested.block_len,
                heap,
                (blocks, terminator, recovered_len),
                false,
            )?;
            if let Some((view_base, restored_len)) = nested.restored_view {
                let view = self
                    .guest_allocation_views
                    .get_mut(&view_base)
                    .expect("validated allocation view remains until nested free completes");
                view.len = restored_len;
                view.reclaimable_prefix_len = None;
            }
            return Ok(());
        }
        if self.reconcile_owned_guest_allocation_suffix(address, len, module)? {
            return Ok(());
        }
        self.validate_guest_allocation_owner(address, len, module, "free")?;
        self.free_guest_block(address, len)
    }

    fn reconcile_owned_guest_allocation_suffix(
        &mut self,
        address: GuestAddr,
        len: usize,
        module: usize,
    ) -> Result<bool> {
        if len == 0 {
            return Ok(false);
        }
        let caller = self
            .modules
            .get(module)
            .map(|module| module.generation)
            .ok_or_else(|| Error::Abi(format!("free for missing module {module}")))?;
        let block_len = aligned_heap_len(len)?;
        let requested = ExecutableRange {
            base: address,
            len: block_len as usize,
        };
        let Some((backing_base, backing_len)) =
            self.guest_allocations.iter().find_map(|(base, len)| {
                let backing_end = base.checked_add(*len)?;
                let requested_end = address.0.checked_add(block_len)?;
                (address.0 > *base && requested_end == backing_end).then_some((*base, *len))
            })
        else {
            return Ok(false);
        };
        match self.guest_allocation_owners.get(&backing_base).copied() {
            Some(owner) if owner != caller => {
                return Err(Error::Abi(
                    "free references an allocation owned by another module".into(),
                ));
            }
            Some(_) => {}
            None => return Ok(false),
        }
        if self.guest_allocation_views.iter().any(|(base, view)| {
            ExecutableRange {
                base: GuestAddr(*base),
                len: view.len as usize,
            }
            .overlaps(requested)
        }) {
            return Err(Error::Abi(
                "free overlaps an active guest allocation view".into(),
            ));
        }

        let heap = self.guest_heap_state()?;
        let heap_range = ExecutableRange {
            base: GuestAddr(heap.base),
            len: heap.span as usize,
        };
        if !heap_range.contains_range(requested) {
            return Ok(false);
        }
        let (blocks, terminator, recovered_len) = self.read_free_blocks(heap)?;
        let requested_end = address
            .0
            .checked_add(block_len)
            .ok_or_else(|| Error::Abi("freed guest block end overflow".into()))?;
        let requested_end_offset = requested_end - heap.base;
        let overlaps_free = blocks.iter().any(|block| {
            let block_end = block.offset + block.len;
            let requested_offset = address.0 - heap.base;
            block.offset < requested_end_offset && requested_offset < block_end
        });
        let abuts_free = blocks
            .iter()
            .any(|block| block.offset == requested_end_offset);
        if overlaps_free || !abuts_free {
            return Ok(false);
        }

        self.return_guest_heap_range(
            address,
            block_len,
            heap,
            (blocks, terminator, recovered_len),
            false,
        )?;
        let retained_len = address.0 - backing_base;
        debug_assert!(retained_len < backing_len);
        self.guest_allocations.insert(backing_base, retained_len);
        Ok(true)
    }

    fn nested_guest_suballocation(
        &self,
        address: GuestAddr,
        len: usize,
        module: usize,
        operation: &str,
    ) -> Result<Option<NestedGuestSuballocation>> {
        if len == 0 {
            return Ok(None);
        }
        let caller = self
            .modules
            .get(module)
            .map(|module| module.generation)
            .ok_or_else(|| Error::Abi(format!("{operation} for missing module {module}")))?;
        let block_len = aligned_heap_len(len)?;
        let requested = ExecutableRange {
            base: address,
            len: block_len as usize,
        };
        let Some((backing_base, owner)) = self.guest_allocations.iter().find_map(|(base, len)| {
            let backing = ExecutableRange {
                base: GuestAddr(*base),
                len: *len as usize,
            };
            (address.0 != *base && backing.contains_range(requested))
                .then(|| (*base, self.guest_allocation_owners.get(base).copied()))
        }) else {
            return Ok(None);
        };
        if owner != Some(caller) {
            return Ok(None);
        }
        let Some(nested_heap) = self.nested_guest_heaps.get(&backing_base) else {
            return Ok(None);
        };
        if nested_heap.owner_generation != caller {
            return Err(Error::Abi(format!(
                "{operation} references a nested heap owned by another module"
            )));
        }
        let heap = self.guest_heap_state()?;
        if heap.base != nested_heap.heap_base || heap.span != nested_heap.heap_span {
            return Err(Error::Abi(format!(
                "{operation} references a nested allocation outside its active heap"
            )));
        }
        let overlapping_views = self
            .guest_allocation_views
            .iter()
            .filter(|(base, view)| {
                ExecutableRange {
                    base: GuestAddr(**base),
                    len: view.len as usize,
                }
                .overlaps(requested)
            })
            .collect::<Vec<_>>();
        let restored_view = match overlapping_views.as_slice() {
            [] => None,
            [(view_base, view)] => {
                let Some(restored_len) = address.0.checked_sub(**view_base) else {
                    return Err(Error::Abi(format!(
                        "{operation} overlaps an active guest allocation view"
                    )));
                };
                let view_end = view_base.checked_add(view.len);
                let requested_end = requested.end();
                let can_restore = view.backing_base == backing_base
                    && view.owner_generation == caller
                    && view.reclaimable_prefix_len == Some(restored_len)
                    && view_end.is_some_and(|view_end| {
                        requested_end.is_some_and(|requested_end| view_end <= requested_end)
                    });
                if !can_restore {
                    return Err(Error::Abi(format!(
                        "{operation} overlaps an active guest allocation view"
                    )));
                }
                Some((**view_base, restored_len))
            }
            _ => {
                return Err(Error::Abi(format!(
                    "{operation} overlaps an active guest allocation view"
                )));
            }
        };
        Ok(Some(NestedGuestSuballocation {
            block_len,
            restored_view,
        }))
    }

    pub(super) fn tracked_guest_allocation_len(&self, address: GuestAddr) -> Option<u32> {
        self.guest_allocation_views
            .get(&address.0)
            .map(|view| view.len)
            .or_else(|| self.guest_allocations.get(&address.0).copied())
            .or_else(|| {
                self.detached_guest_allocations
                    .get(&address.0)
                    .and_then(|(len, _)| u32::try_from(*len).ok())
            })
    }

    fn allocate_guest_block_owned(
        &mut self,
        len: usize,
        owner_generation: Option<u64>,
    ) -> Result<Option<GuestAddr>> {
        let block_len = aligned_heap_len(len)?;
        let heap_base = self.read_platform_data_slot(108)?;
        let heap_end = self.read_platform_data_slot(110)?;
        if heap_base == 0 && heap_end == 0 {
            return self.allocate_detached_guest_block(block_len, owner_generation);
        }
        let allocation = self.allocate_heap_block(block_len, HEAP_ALIGNMENT)?;
        if let Some((address, consumed)) = allocation {
            self.track_guest_heap_allocation(address, consumed, owner_generation)?;
        }
        Ok(allocation.map(|(address, _)| address))
    }

    pub(super) fn track_guest_heap_allocation(
        &mut self,
        address: GuestAddr,
        block_len: u32,
        owner_generation: Option<u64>,
    ) -> Result<()> {
        let allocation_start = u64::from(address.0);
        let allocation_end = allocation_start + u64::from(block_len);
        let requested = ExecutableRange {
            base: address,
            len: block_len as usize,
        };
        let superseded = self
            .guest_allocations
            .iter()
            .filter_map(|(tracked_start, tracked_len)| {
                let tracked = ExecutableRange {
                    base: GuestAddr(*tracked_start),
                    len: *tracked_len as usize,
                };
                tracked.overlaps(requested).then_some(tracked)
            })
            .chain(std::iter::once(requested))
            .collect::<Vec<_>>();
        for range in merge_executable_intervals(superseded) {
            self.revoke_executable_ranges_in(range)?;
        }
        self.guest_allocations.retain(|tracked_start, tracked_len| {
            let tracked_start = u64::from(*tracked_start);
            let tracked_end = tracked_start + u64::from(*tracked_len);
            tracked_end <= allocation_start || tracked_start >= allocation_end
        });
        self.guest_allocation_owners
            .retain(|tracked_start, _| self.guest_allocations.contains_key(tracked_start));
        self.guest_allocation_views.retain(|_, view| {
            self.guest_allocations.contains_key(&view.backing_base)
                || self
                    .platform_memory_extensions
                    .contains_key(&view.backing_base)
        });
        self.nested_guest_heaps
            .retain(|backing_base, _| self.guest_allocations.contains_key(backing_base));
        self.guest_allocations.insert(address.0, block_len);
        if let Some(owner_generation) = owner_generation {
            self.guest_allocation_owners
                .insert(address.0, owner_generation);
        }
        Ok(())
    }

    pub(super) fn prepared_output_candidate_is_claimable_by_module(
        &self,
        address: GuestAddr,
        block_len: u32,
        module: usize,
    ) -> Result<bool> {
        let requested = ExecutableRange {
            base: address,
            len: block_len as usize,
        };
        requested
            .end()
            .ok_or_else(|| Error::Abi("compact RAM output range overflow".into()))?;
        let owner_generation = self
            .modules
            .get(module)
            .map(|module| module.generation)
            .ok_or_else(|| Error::Abi(format!("compact RAM output for missing module {module}")))?;

        if self
            .legacy_wrapper_backing(address, block_len, owner_generation)?
            .is_some()
        {
            return Ok(true);
        }

        let mtk_window = ExecutableRange {
            base: MTK_NATIVE_EXTENSION_BASE,
            len: MTK_NATIVE_EXTENSION_LEN,
        };
        if mtk_window.contains_range(requested) {
            if self
                .mtk_native_extension_owner
                .is_some_and(|owner| owner != owner_generation)
            {
                return Err(Error::Abi(
                    "compact RAM output belongs to another module".into(),
                ));
            }
            let heap_base = self.read_platform_data_slot(108)?;
            let heap_end = self.read_platform_data_slot(110)?;
            if heap_base != 0 || heap_end != 0 {
                let heap = self.guest_heap_state()?;
                let heap_range = ExecutableRange {
                    base: GuestAddr(heap.base),
                    len: heap.span as usize,
                };
                if heap_range.overlaps(requested) && !heap_range.contains_range(requested) {
                    return Ok(false);
                }
            }
            return Ok(true);
        }

        if let Some((base, _)) = self.guest_allocations.iter().find(|(base, len)| {
            ExecutableRange {
                base: GuestAddr(**base),
                len: **len as usize,
            }
            .contains_range(requested)
        }) {
            let base = *base;
            match self.guest_allocation_owners.get(&base).copied() {
                Some(owner) if owner != owner_generation => {
                    return Err(Error::Abi(
                        "compact RAM output belongs to another module".into(),
                    ));
                }
                None if address.0 != base => return Ok(false),
                None | Some(_) => {}
            }
            self.validate_guest_allocation_view(address, block_len, base, owner_generation)?;
            return Ok(true);
        }
        if self.guest_allocations.iter().any(|(base, len)| {
            ExecutableRange {
                base: GuestAddr(*base),
                len: *len as usize,
            }
            .overlaps(requested)
        }) {
            return Ok(false);
        }

        if let Some((base, _)) = self
            .detached_guest_allocations
            .iter()
            .find(|(base, (len, _))| {
                ExecutableRange {
                    base: GuestAddr(**base),
                    len: *len,
                }
                .contains_range(requested)
            })
        {
            let base = *base;
            match self.detached_guest_allocation_owners.get(&base).copied() {
                Some(owner) if owner != owner_generation => {
                    return Err(Error::Abi(
                        "compact RAM output belongs to another module".into(),
                    ));
                }
                None if address.0 != base => return Ok(false),
                None | Some(_) => return Ok(true),
            }
        }
        if self
            .detached_guest_allocations
            .iter()
            .any(|(base, (len, _))| {
                ExecutableRange {
                    base: GuestAddr(*base),
                    len: *len,
                }
                .overlaps(requested)
            })
        {
            return Ok(false);
        }

        if let Some((base, extension)) =
            self.platform_memory_extensions
                .iter()
                .find(|(base, extension)| {
                    ExecutableRange {
                        base: GuestAddr(**base),
                        len: extension.len,
                    }
                    .contains_range(requested)
                })
        {
            if extension.owner_generation != owner_generation {
                return Err(Error::Abi(
                    "compact RAM output belongs to another module".into(),
                ));
            }
            self.validate_guest_allocation_view(address, block_len, *base, owner_generation)?;
            return Ok(true);
        }
        if self
            .platform_memory_extensions
            .iter()
            .any(|(base, extension)| {
                ExecutableRange {
                    base: GuestAddr(*base),
                    len: extension.len,
                }
                .overlaps(requested)
            })
        {
            return Ok(false);
        }

        let heap_base = self.read_platform_data_slot(108)?;
        let heap_end = self.read_platform_data_slot(110)?;
        if heap_base == 0 && heap_end == 0 {
            return Ok(false);
        }
        let heap = self.guest_heap_state()?;
        let heap_range = ExecutableRange {
            base: GuestAddr(heap.base),
            len: heap.span as usize,
        };
        if !heap_range.contains_range(requested) {
            return Ok(false);
        }
        let (blocks, _, _) = self.read_free_blocks(heap)?;
        Ok(blocks.iter().any(|block| {
            ExecutableRange {
                base: GuestAddr(heap.base + block.offset),
                len: block.len as usize,
            }
            .contains_range(requested)
        }))
    }

    pub(super) fn claim_prepared_output_for_module(
        &mut self,
        address: GuestAddr,
        block_len: u32,
        module: usize,
    ) -> Result<()> {
        let requested = ExecutableRange {
            base: address,
            len: block_len as usize,
        };
        let requested_end = requested
            .end()
            .ok_or_else(|| Error::Abi("compact RAM output range overflow".into()))?;
        let owner_generation = self
            .modules
            .get(module)
            .map(|module| module.generation)
            .ok_or_else(|| Error::Abi(format!("compact RAM output for missing module {module}")))?;

        if self
            .legacy_wrapper_backing(address, block_len, owner_generation)?
            .is_some()
        {
            // The wrapper frees its original backing address, so do not replace
            // its allocation tracking with an interior payload view.
            return Ok(());
        }

        let mtk_window = ExecutableRange {
            base: MTK_NATIVE_EXTENSION_BASE,
            len: MTK_NATIVE_EXTENSION_LEN,
        };
        if mtk_window.contains_range(requested) {
            if self
                .mtk_native_extension_owner
                .is_some_and(|owner| owner != owner_generation)
            {
                return Err(Error::Abi(
                    "compact RAM output belongs to another module".into(),
                ));
            }

            let heap_base = self.read_platform_data_slot(108)?;
            let heap_end = self.read_platform_data_slot(110)?;
            if heap_base != 0 || heap_end != 0 {
                let heap = self.guest_heap_state()?;
                let heap_range = ExecutableRange {
                    base: GuestAddr(heap.base),
                    len: heap.span as usize,
                };
                if heap_range.overlaps(requested) {
                    if !heap_range.contains_range(requested) {
                        return Err(Error::Abi(format!(
                            "compact RAM output {:#010x}..{requested_end:#010x} crosses the active heap boundary",
                            address.0,
                        )));
                    }
                    // The fixed native-extension window may also be staged into
                    // the guest heap. Remove its payload range from the free list
                    // before the compact image overwrites a free-block header.
                    // `None` is valid when the range was already reserved.
                    if let Some(reclaim_len) = self.reserve_guest_heap_range(address, block_len)? {
                        self.track_guest_heap_allocation(
                            address,
                            reclaim_len,
                            Some(owner_generation),
                        )?;
                    }
                }
            }
            self.mtk_native_extension_owner = Some(owner_generation);
            return Ok(());
        }

        if let Some((base, len)) = self.guest_allocations.iter().find(|(base, len)| {
            ExecutableRange {
                base: GuestAddr(**base),
                len: **len as usize,
            }
            .contains_range(requested)
        }) {
            let base = *base;
            let len = *len;
            let claim_owner = match self.guest_allocation_owners.get(&base).copied() {
                Some(owner) if owner != owner_generation => {
                    return Err(Error::Abi(
                        "compact RAM output belongs to another module".into(),
                    ));
                }
                None if address.0 != base => {
                    return Err(Error::Abi(format!(
                        "compact RAM output {:#010x}..{requested_end:#010x} is an interior view of an unowned allocation",
                        address.0,
                    )));
                }
                None => true,
                Some(_) => false,
            };
            debug_assert!(
                ExecutableRange {
                    base: GuestAddr(base),
                    len: len as usize,
                }
                .contains_range(requested)
            );
            self.validate_guest_allocation_view(address, block_len, base, owner_generation)?;
            // Guest allocators may rebuild their free list inside a still-tracked
            // host backing allocation. Reconcile that current view before the
            // compact payload overwrites its free-block header.
            let view_len = self
                .reserve_guest_heap_range(address, block_len)?
                .unwrap_or(block_len);
            if claim_owner {
                self.guest_allocation_owners.insert(base, owner_generation);
            }
            if address.0 != base {
                let heap = self.guest_heap_state()?;
                if (ExecutableRange {
                    base: GuestAddr(heap.base),
                    len: heap.span as usize,
                })
                .contains_range(requested)
                {
                    self.nested_guest_heaps.insert(
                        base,
                        NestedGuestHeap {
                            owner_generation,
                            heap_base: heap.base,
                            heap_span: heap.span,
                        },
                    );
                }
                self.record_guest_allocation_view(
                    GuestAddr(base),
                    address,
                    view_len,
                    owner_generation,
                );
            }
            return Ok(());
        }
        if self.guest_allocations.iter().any(|(base, len)| {
            ExecutableRange {
                base: GuestAddr(*base),
                len: *len as usize,
            }
            .overlaps(requested)
        }) {
            return Err(Error::Abi(format!(
                "compact RAM output {:#010x}..{requested_end:#010x} partially overlaps a tracked allocation",
                address.0,
            )));
        }

        if let Some((base, (_len, _))) =
            self.detached_guest_allocations
                .iter()
                .find(|(base, (len, _))| {
                    ExecutableRange {
                        base: GuestAddr(**base),
                        len: *len,
                    }
                    .contains_range(requested)
                })
        {
            let base = *base;
            match self.detached_guest_allocation_owners.get(&base).copied() {
                Some(owner) if owner != owner_generation => {
                    return Err(Error::Abi(
                        "compact RAM output belongs to another module".into(),
                    ));
                }
                None if address.0 != base => {
                    return Err(Error::Abi(format!(
                        "compact RAM output {:#010x}..{requested_end:#010x} is an interior view of an unowned detached allocation",
                        address.0,
                    )));
                }
                None => {
                    self.detached_guest_allocation_owners
                        .insert(base, owner_generation);
                }
                Some(_) => {}
            }
            return Ok(());
        }
        if self
            .detached_guest_allocations
            .iter()
            .any(|(base, (len, _))| {
                ExecutableRange {
                    base: GuestAddr(*base),
                    len: *len,
                }
                .overlaps(requested)
            })
        {
            return Err(Error::Abi(format!(
                "compact RAM output {:#010x}..{requested_end:#010x} partially overlaps a tracked detached allocation",
                address.0,
            )));
        }

        if let Some((base, extension)) =
            self.platform_memory_extensions
                .iter()
                .find_map(|(base, extension)| {
                    ExecutableRange {
                        base: GuestAddr(*base),
                        len: extension.len,
                    }
                    .contains_range(requested)
                    .then_some((*base, *extension))
                })
        {
            if extension.owner_generation != owner_generation {
                return Err(Error::Abi(
                    "compact RAM output belongs to another module".into(),
                ));
            }
            self.validate_guest_allocation_view(address, block_len, base, owner_generation)?;
            let view_len = self
                .reserve_guest_heap_range(address, block_len)?
                .unwrap_or(block_len);
            self.record_guest_allocation_view(GuestAddr(base), address, view_len, owner_generation);
            return Ok(());
        }
        if self
            .platform_memory_extensions
            .iter()
            .any(|(base, extension)| {
                ExecutableRange {
                    base: GuestAddr(*base),
                    len: extension.len,
                }
                .overlaps(requested)
            })
        {
            return Err(Error::Abi(format!(
                "compact RAM output {:#010x}..{requested_end:#010x} partially overlaps a platform allocation",
                address.0,
            )));
        }

        let heap = self.guest_heap_state()?;
        let heap_range = ExecutableRange {
            base: GuestAddr(heap.base),
            len: heap.span as usize,
        };
        if !heap_range.contains_range(requested) {
            return Err(Error::Abi(format!(
                "compact RAM output {:#010x}..{requested_end:#010x} is outside module-owned memory",
                address.0,
            )));
        }
        let Some(reclaim_len) = self.reserve_guest_heap_range(address, block_len)? else {
            return Err(Error::Abi(format!(
                "compact RAM output {:#010x}..{requested_end:#010x} is neither allocated nor free",
                address.0,
            )));
        };
        self.track_guest_heap_allocation(address, reclaim_len, Some(owner_generation))
    }

    fn legacy_wrapper_backing(
        &self,
        address: GuestAddr,
        block_len: u32,
        owner_generation: u64,
    ) -> Result<Option<GuestAddr>> {
        let Some(base) = address.0.checked_sub(4) else {
            return Ok(None);
        };
        let Some(&backing_len) = self.guest_allocations.get(&base) else {
            return Ok(None);
        };
        match self.guest_allocation_owners.get(&base).copied() {
            Some(owner) if owner != owner_generation => {
                return Err(Error::Abi(
                    "compact RAM output belongs to another module".into(),
                ));
            }
            Some(_) => {}
            None => return Ok(None),
        }

        let payload_len = self.memory.read_u32(GuestAddr(base))?;
        let Some(wrapper_len) = payload_len.checked_add(4) else {
            return Ok(None);
        };
        let Ok(payload_block_len) = aligned_heap_len(payload_len as usize) else {
            return Ok(None);
        };
        let Ok(expected_backing_len) = aligned_heap_len(wrapper_len as usize) else {
            return Ok(None);
        };
        if payload_block_len != block_len || expected_backing_len != backing_len {
            return Ok(None);
        }
        let backing_end = base
            .checked_add(backing_len)
            .ok_or_else(|| Error::Abi("compact RAM wrapper range overflow".into()))?;
        let payload_end = address
            .0
            .checked_add(payload_len)
            .ok_or_else(|| Error::Abi("compact RAM wrapper payload overflow".into()))?;
        Ok((payload_end <= backing_end).then_some(GuestAddr(base)))
    }

    fn validate_guest_allocation_view(
        &self,
        address: GuestAddr,
        len: u32,
        backing_base: u32,
        owner_generation: u64,
    ) -> Result<()> {
        if address.0 == backing_base {
            return Ok(());
        }
        let requested = ExecutableRange {
            base: address,
            len: len as usize,
        };
        let mut replaces_existing = false;
        for (base, view) in &self.guest_allocation_views {
            let existing = ExecutableRange {
                base: GuestAddr(*base),
                len: view.len as usize,
            };
            if !existing.overlaps(requested) {
                continue;
            }
            if view.backing_base != backing_base || view.owner_generation != owner_generation {
                return Err(Error::Abi(
                    "compact RAM output overlaps another allocation view".into(),
                ));
            }
            replaces_existing = true;
        }
        if !replaces_existing && self.guest_allocation_views.len() >= MAX_GUEST_ALLOCATION_VIEWS {
            return Err(Error::ResourceLimit(format!(
                "guest allocation views exceeded {MAX_GUEST_ALLOCATION_VIEWS}"
            )));
        }
        Ok(())
    }

    fn record_guest_allocation_view(
        &mut self,
        backing: GuestAddr,
        address: GuestAddr,
        len: u32,
        owner_generation: u64,
    ) {
        let reclaimable_prefix_len = self
            .guest_allocation_views
            .get(&address.0)
            .filter(|view| {
                view.backing_base == backing.0 && view.owner_generation == owner_generation
            })
            .and_then(|view| match len.cmp(&view.len) {
                std::cmp::Ordering::Greater => Some(view.len),
                std::cmp::Ordering::Equal => view.reclaimable_prefix_len,
                std::cmp::Ordering::Less => view
                    .reclaimable_prefix_len
                    .filter(|prefix_len| *prefix_len < len),
            });
        let requested = ExecutableRange {
            base: address,
            len: len as usize,
        };
        self.guest_allocation_views.retain(|base, view| {
            view.backing_base != backing.0
                || !ExecutableRange {
                    base: GuestAddr(*base),
                    len: view.len as usize,
                }
                .overlaps(requested)
        });
        self.guest_allocation_views.insert(
            address.0,
            GuestAllocationView {
                len,
                backing_base: backing.0,
                owner_generation,
                reclaimable_prefix_len,
            },
        );
    }

    fn allocate_detached_guest_block(
        &mut self,
        block_len: u32,
        owner_generation: Option<u64>,
    ) -> Result<Option<GuestAddr>> {
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
        if let Some(owner_generation) = owner_generation {
            self.detached_guest_allocation_owners
                .insert(address.0, owner_generation);
        }
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
        let free_left_after_recovery = heap
            .free_left
            .checked_sub(recovered_len)
            .ok_or_else(|| {
                Error::Abi(format!(
                    "guest free-byte count {:#x} is smaller than recovery correction {recovered_len:#x}",
                    heap.free_left
                ))
            })?;
        let Some(free_left) = free_left_after_recovery.checked_sub(consumed) else {
            return Ok(None);
        };
        blocks.splice(index..=index, replacement);
        self.write_free_blocks(heap, &blocks, terminator, free_left)?;
        let address = GuestAddr(heap.base.wrapping_add(start));
        self.memory.write(address, &vec![0; required as usize])?;
        Ok(Some((address, reclaim_len)))
    }

    fn free_guest_allocation_view(
        &mut self,
        address: GuestAddr,
        view: GuestAllocationView,
    ) -> Result<()> {
        let released = ExecutableRange {
            base: address,
            len: view.len as usize,
        };
        let heap_base = self.read_platform_data_slot(108)?;
        let heap_end = self.read_platform_data_slot(110)?;
        if heap_base == 0 && heap_end == 0 {
            self.revoke_executable_ranges_in(released)?;
            return self.clear_freed_ram_package(address, view.len as usize);
        }

        let heap = self.guest_heap_state()?;
        let heap_range = ExecutableRange {
            base: GuestAddr(heap.base),
            len: heap.span as usize,
        };
        if heap_range.contains_range(released) {
            let (blocks, terminator, recovered_len) = self.read_free_blocks(heap)?;
            return self.return_guest_heap_range(
                address,
                view.len,
                heap,
                (blocks, terminator, recovered_len),
                false,
            );
        }
        if heap_range.overlaps(released) {
            return Err(Error::Abi(format!(
                "allocation view {:#010x}..{:#010x} crosses the active heap boundary",
                address.0,
                address.0.wrapping_add(view.len),
            )));
        }
        self.revoke_executable_ranges_in(released)?;
        self.clear_freed_ram_package(address, view.len as usize)
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
            self.revoke_executable_ranges_in(ExecutableRange { base: address, len })?;
            self.clear_freed_ram_package(address, len)?;
            self.memory.unmap(address, len)?;
            self.detached_guest_allocations.remove(&address.0);
            self.detached_guest_allocation_owners.remove(&address.0);
            self.guest_allocation_views
                .retain(|_, view| view.backing_base != address.0);
            self.nested_guest_heaps.remove(&address.0);
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
        let mut heap = self.guest_heap_state()?;
        let Some(offset) = address.0.checked_sub(heap.base) else {
            if let Some(tracked_len) = self.guest_allocations.get(&address.0).copied() {
                self.revoke_executable_ranges_in(ExecutableRange {
                    base: address,
                    len: tracked_len as usize,
                })?;
                self.clear_freed_ram_package(address, tracked_len as usize)?;
                self.guest_allocations.remove(&address.0);
            }
            self.guest_allocation_owners.remove(&address.0);
            self.guest_allocation_views
                .retain(|_, view| view.backing_base != address.0);
            self.nested_guest_heaps.remove(&address.0);
            return Ok(());
        };
        if offset >= heap.span {
            if let Some(tracked_len) = self.guest_allocations.get(&address.0).copied() {
                self.revoke_executable_ranges_in(ExecutableRange {
                    base: address,
                    len: tracked_len as usize,
                })?;
                self.clear_freed_ram_package(address, tracked_len as usize)?;
                self.guest_allocations.remove(&address.0);
            }
            self.guest_allocation_owners.remove(&address.0);
            self.guest_allocation_views
                .retain(|_, view| view.backing_base != address.0);
            self.nested_guest_heaps.remove(&address.0);
            return Ok(());
        }
        let tracked_len = self.guest_allocations.get(&address.0).copied();
        let explicit_len = (len != 0).then(|| aligned_heap_len(len));
        let (mut blocks, terminator, recovered_len) = self.read_free_blocks(heap)?;
        if let Some(tracked_len) = tracked_len
            && explicit_len.as_ref().is_none_or(|explicit_len| {
                explicit_len
                    .as_ref()
                    .is_ok_and(|explicit_len| *explicit_len == tracked_len)
            })
        {
            if let Some(restored_free_left) = self.restore_truncated_legacy_wrapper_tail(
                address,
                len,
                tracked_len,
                heap,
                &mut blocks,
                terminator,
            )? {
                heap.free_left = restored_free_left;
            }
            let tracked_end = offset
                .checked_add(tracked_len)
                .ok_or_else(|| Error::Abi("tracked guest allocation end overflow".into()))?;
            let payload_offset = offset.checked_add(4);
            let returned_payload = payload_offset.and_then(|payload_offset| {
                blocks.iter().position(|block| {
                    let block_end = block.offset.checked_add(block.len);
                    block.offset == payload_offset
                        && block_end.is_some_and(|block_end| block_end >= tracked_end)
                })
            });
            if let Some(index) = returned_payload {
                // The payload view recovered by read_free_blocks leaves only the
                // legacy wrapper's four-byte length header allocated. A later
                // platform free of the backing block releases that prefix by
                // extending the already-free payload range backwards.
                blocks[index].offset = offset;
                blocks[index].len = blocks[index]
                    .len
                    .checked_add(4)
                    .ok_or_else(|| Error::Abi("guest free-block length overflow".into()))?;
                validate_free_block_ranges(&blocks, heap.span)?;
                let free_left = heap
                    .free_left
                    .checked_sub(recovered_len)
                    .and_then(|free_left| free_left.checked_add(4))
                    .ok_or_else(|| Error::Abi("guest free-byte count overflow".into()))?;
                self.revoke_executable_ranges_in(ExecutableRange {
                    base: address,
                    len: tracked_len as usize,
                })?;
                self.clear_freed_ram_package(address, tracked_len as usize)?;
                self.write_free_blocks(heap, &blocks, terminator, free_left)?;
                self.guest_allocations.remove(&address.0);
                self.guest_allocation_owners.remove(&address.0);
                self.guest_allocation_views
                    .retain(|_, view| view.backing_base != address.0);
                self.nested_guest_heaps.remove(&address.0);
                return Ok(());
            }
        }
        let block_len = match tracked_len {
            Some(block_len)
                if free_candidate_is_available(offset, block_len, heap.span, &blocks) =>
            {
                block_len
            }
            Some(block_len) => explicit_len
                .and_then(Result::ok)
                .filter(|explicit_len| {
                    free_candidate_is_available(offset, *explicit_len, heap.span, &blocks)
                })
                .unwrap_or(block_len),
            None => match explicit_len {
                Some(block_len) => block_len?,
                None => {
                    return Err(Error::Abi(format!(
                        "free references unknown guest allocation {:#010x}",
                        address.0
                    )));
                }
            },
        };
        if let Some(tracked_len) = tracked_len {
            // The guest free-list can make the host's tracked extent stale and
            // then free only its remaining prefix explicitly. Revoke the complete
            // old extent before dropping its bookkeeping so no free tail remains RX.
            self.revoke_executable_ranges_in(ExecutableRange {
                base: address,
                len: tracked_len as usize,
            })?;
            self.clear_freed_ram_package(address, tracked_len as usize)?;
        }
        let allow_contained_free = self.nested_guest_heaps.contains_key(&address.0);
        self.return_guest_heap_range(
            address,
            block_len,
            heap,
            (blocks, terminator, recovered_len),
            allow_contained_free,
        )?;
        self.guest_allocations.remove(&address.0);
        self.guest_allocation_owners.remove(&address.0);
        self.guest_allocation_views
            .retain(|_, view| view.backing_base != address.0);
        self.nested_guest_heaps.remove(&address.0);
        Ok(())
    }

    fn restore_truncated_legacy_wrapper_tail(
        &self,
        address: GuestAddr,
        explicit_len: usize,
        tracked_len: u32,
        heap: GuestHeapState,
        blocks: &mut [FreeBlock],
        terminator: u32,
    ) -> Result<Option<u32>> {
        let payload_len = self.memory.read_u32(address)?;
        let Some(wrapper_len) = payload_len.checked_add(4) else {
            return Ok(None);
        };
        if usize::try_from(wrapper_len).ok() != Some(explicit_len)
            || aligned_heap_len(wrapper_len as usize).ok() != Some(tracked_len)
            || blocks.len() != 1
            || terminator != heap.span
        {
            return Ok(None);
        }

        let Some(offset) = address.0.checked_sub(heap.base) else {
            return Ok(None);
        };
        let Some(successor_offset) = offset.checked_add(tracked_len) else {
            return Ok(None);
        };
        let block = &mut blocks[0];
        if block.offset != successor_offset {
            return Ok(None);
        }
        let Some(full_tail_len) = heap.span.checked_sub(successor_offset) else {
            return Ok(None);
        };
        let Some(unaccounted_len) = full_tail_len.checked_sub(heap.free_left) else {
            return Ok(None);
        };
        // The wrapper can leave a tiny but valid successor header while its
        // counter still accounts for the complete tail. Restore only that sole
        // tail block, allowing at most the allocator's alignment slop.
        if block.len >= full_tail_len || unaccounted_len >= HEAP_ALIGNMENT {
            return Ok(None);
        }

        let restored = ExecutableRange {
            base: GuestAddr(heap.base + successor_offset),
            len: full_tail_len as usize,
        };
        if self.guest_allocations.iter().any(|(base, len)| {
            *base != address.0
                && ExecutableRange {
                    base: GuestAddr(*base),
                    len: *len as usize,
                }
                .overlaps(restored)
        }) || self.guest_allocation_views.iter().any(|(base, view)| {
            ExecutableRange {
                base: GuestAddr(*base),
                len: view.len as usize,
            }
            .overlaps(restored)
        }) {
            return Ok(None);
        }
        if self
            .memory
            .check_range(restored.base, restored.len, Permissions::READ_WRITE)
            .is_err()
        {
            return Ok(None);
        }

        block.len = full_tail_len;
        validate_free_block_ranges(blocks, heap.span)?;
        Ok(Some(full_tail_len))
    }

    fn return_guest_heap_range(
        &mut self,
        address: GuestAddr,
        block_len: u32,
        heap: GuestHeapState,
        free_list: (Vec<FreeBlock>, u32, u32),
        allow_contained_free: bool,
    ) -> Result<()> {
        let (mut blocks, mut terminator, recovered_len) = free_list;
        let offset = address
            .0
            .checked_sub(heap.base)
            .ok_or_else(|| Error::Abi("freed guest block starts before the active heap".into()))?;
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

        if blocks.iter().any(|block| {
            let block_end = block.offset + block.len;
            block.offset <= offset && end <= block_end
        }) {
            self.revoke_executable_ranges_in(ExecutableRange {
                base: address,
                len: block_len as usize,
            })?;
            self.clear_freed_ram_package(address, block_len as usize)?;
            return Ok(());
        }

        let mut already_free = 0_u32;
        let mut retained = Vec::with_capacity(blocks.len() + 1);
        for block in blocks {
            let block_end = block
                .offset
                .checked_add(block.len)
                .ok_or_else(|| Error::Abi("guest free-block end overflow".into()))?;
            if offset <= block.offset && block_end <= end {
                if !allow_contained_free {
                    return Err(Error::Abi(format!(
                        "freed guest block at offset {offset:#x} contains free block {:#x}..{block_end:#x}",
                        block.offset,
                    )));
                }
                already_free = already_free
                    .checked_add(block.len)
                    .ok_or_else(|| Error::Abi("contained guest free-byte count overflow".into()))?;
                continue;
            }
            if block.offset < end && offset < block_end {
                return Err(Error::Abi(format!(
                    "freed guest block at offset {offset:#x} partially overlaps free block {:#x}..{block_end:#x}",
                    block.offset,
                )));
            }
            retained.push(block);
        }
        blocks = retained;
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
            .and_then(|free_left| free_left.checked_add(block_len - already_free))
            .ok_or_else(|| Error::Abi("guest free-byte count overflow".into()))?;
        self.revoke_executable_ranges_in(ExecutableRange {
            base: address,
            len: block_len as usize,
        })?;
        self.clear_freed_ram_package(address, block_len as usize)?;
        self.write_free_blocks(heap, &merged, terminator, free_left)?;
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

    pub(super) fn reserve_guest_heap_range(
        &mut self,
        address: GuestAddr,
        len: u32,
    ) -> Result<Option<u32>> {
        let heap = self.guest_heap_state()?;
        let Some(offset) = address.0.checked_sub(heap.base) else {
            return Ok(None);
        };
        if offset >= heap.span {
            return Ok(None);
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

        let (mut blocks, terminator, recovered_len) = self.read_free_blocks(heap)?;
        let overlapping = blocks
            .iter()
            .enumerate()
            .filter_map(|(index, block)| {
                let block_end = block.offset.checked_add(block.len)?;
                (block.offset < end && offset < block_end).then_some((index, block_end))
            })
            .collect::<Vec<_>>();
        let Some(&(index, block_end)) = overlapping.first() else {
            if recovered_len != 0 {
                let free_left = heap.free_left.checked_sub(recovered_len).ok_or_else(|| {
                    Error::Abi("guest free-byte count underflow while recovering".into())
                })?;
                self.write_free_blocks(heap, &blocks, terminator, free_left)?;
            }
            return Ok(None);
        };
        let block = blocks[index];
        if overlapping.len() != 1 || block.offset > offset || block_end < end {
            return Err(Error::Abi(format!(
                "prepared guest range {:#010x}..{:#010x} is only partially free",
                address.0,
                address.0.wrapping_add(len),
            )));
        }

        let prefix_len = offset - block.offset;
        let suffix_len = block_end - end;
        let mut replacement = Vec::with_capacity(2);
        if prefix_len >= FREE_BLOCK_HEADER_LEN {
            replacement.push(FreeBlock {
                offset: block.offset,
                len: prefix_len,
            });
        }
        if suffix_len >= FREE_BLOCK_HEADER_LEN {
            replacement.push(FreeBlock {
                offset: end,
                len: suffix_len,
            });
        }
        let retained_len = replacement.iter().try_fold(0_u32, |total, block| {
            total
                .checked_add(block.len)
                .ok_or_else(|| Error::Abi("reserved guest byte count overflow".into()))
        })?;
        let reserved_len = block.len - retained_len;
        let discarded_prefix = if prefix_len < FREE_BLOCK_HEADER_LEN {
            prefix_len
        } else {
            0
        };
        let reclaim_len = reserved_len
            .checked_sub(discarded_prefix)
            .ok_or_else(|| Error::Abi("reserved guest allocation length underflow".into()))?;
        blocks.splice(index..=index, replacement);
        let free_left = heap
            .free_left
            .checked_sub(recovered_len)
            .and_then(|free_left| free_left.checked_sub(reserved_len))
            .ok_or_else(|| Error::Abi("guest free-byte count underflow while reserving".into()))?;
        self.write_free_blocks(heap, &blocks, terminator, free_left)?;
        Ok(Some(reclaim_len))
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
                return self.finish_free_block_read(heap, blocks, offset, recovered_len);
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
                // A legacy allocator can zero a withdrawn payload header without
                // advancing the head. Preserve successors that still match our snapshot.
                if let Some((recovered, terminator, counter_correction)) =
                    self.recover_withdrawn_legacy_payload_head(heap, &blocks, offset)
                {
                    recovered_len =
                        recovered_len
                            .checked_add(counter_correction)
                            .ok_or_else(|| {
                                Error::Abi("recovered guest free-byte count overflow".into())
                            })?;
                    return Ok((recovered, terminator, recovered_len));
                }
                return self.finish_free_block_read(heap, blocks, offset, recovered_len);
            }
            let block_end = offset
                .checked_add(len)
                .ok_or_else(|| Error::Abi("guest free-block range overflow".into()))?;
            if len < FREE_BLOCK_HEADER_LEN || block_end > heap.span {
                if let Some((recovered, terminator, counter_correction)) =
                    self.recover_withdrawn_legacy_payload_head(heap, &blocks, offset)
                {
                    recovered_len =
                        recovered_len
                            .checked_add(counter_correction)
                            .ok_or_else(|| {
                                Error::Abi("recovered guest free-byte count overflow".into())
                            })?;
                    return Ok((recovered, terminator, recovered_len));
                }
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
                if let Some((recovered, terminator, counter_correction)) =
                    self.recover_withdrawn_legacy_payload_head(heap, &blocks, offset)
                {
                    recovered_len =
                        recovered_len
                            .checked_add(counter_correction)
                            .ok_or_else(|| {
                                Error::Abi("recovered guest free-byte count overflow".into())
                            })?;
                    return Ok((recovered, terminator, recovered_len));
                }
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

    fn finish_free_block_read(
        &self,
        heap: GuestHeapState,
        mut blocks: Vec<FreeBlock>,
        terminator: u32,
        mut recovered_len: u32,
    ) -> Result<(Vec<FreeBlock>, u32, u32)> {
        if let Err(error) = validate_free_block_ranges(&blocks, heap.span) {
            let Some((recovered, counter_correction)) =
                self.recover_guest_allocation_header_overlap(heap, &blocks, terminator)
            else {
                return Err(error);
            };
            blocks = recovered;
            recovered_len = recovered_len
                .checked_add(counter_correction)
                .ok_or_else(|| Error::Abi("recovered guest free-byte count overflow".into()))?;
            validate_free_block_ranges(&blocks, heap.span)?;
        }
        Ok((blocks, terminator, recovered_len))
    }

    fn recover_guest_allocation_header_overlap(
        &self,
        heap: GuestHeapState,
        blocks: &[FreeBlock],
        terminator: u32,
    ) -> Option<(Vec<FreeBlock>, u32)> {
        let snapshot = self.guest_heap_snapshot.as_ref()?;
        if snapshot.base != heap.base
            || snapshot.span != heap.span
            || snapshot.head != heap.head
            || snapshot.terminator != terminator
            || blocks.len() != snapshot.blocks.len().checked_add(1)?
            || !snapshot.blocks.iter().all(|block| blocks.contains(block))
        {
            return None;
        }

        let mut added = blocks
            .iter()
            .enumerate()
            .filter(|(_, block)| !snapshot.blocks.contains(block));
        let (added_index, added_block) = added.next()?;
        if added.next().is_some() {
            return None;
        }

        // Some legacy wrappers allocate size+4, store the payload length in the
        // first word, and return base+4. Their private free-list can later recycle
        // that payload address with the complete host allocation length. Accept
        // only that tracked shape and keep the four-byte wrapper header reserved.
        let backing_offset = added_block.offset.checked_sub(4)?;
        let backing = heap.base.checked_add(backing_offset)?;
        if self.guest_allocations.get(&backing) != Some(&added_block.len) {
            return None;
        }
        let usable_len = added_block.len.checked_sub(4)?;
        if usable_len < FREE_BLOCK_HEADER_LEN {
            return None;
        }
        let successor_offset = added_block.offset.checked_add(usable_len)?;
        if !snapshot
            .blocks
            .iter()
            .any(|block| block.offset == successor_offset)
        {
            return None;
        }

        let mut recovered = blocks.to_vec();
        recovered[added_index].len = usable_len;
        validate_free_block_ranges(&recovered, heap.span).ok()?;

        // Preserve any staged difference in the guest counter, but remove the
        // duplicate increment observed when this header-backed block is returned.
        let expected_free_left = snapshot.free_left.checked_add(usable_len)?;
        let counter_correction = heap.free_left.checked_sub(expected_free_left)?;
        Some((recovered, counter_correction))
    }

    fn recover_withdrawn_legacy_payload_head(
        &self,
        heap: GuestHeapState,
        preceding: &[FreeBlock],
        withdrawn_offset: u32,
    ) -> Option<(Vec<FreeBlock>, u32, u32)> {
        if !preceding.is_empty() {
            return None;
        }
        let snapshot = self.guest_heap_snapshot.as_ref()?;
        if snapshot.base != heap.base
            || snapshot.span != heap.span
            || snapshot.head != heap.head
            || snapshot.blocks.first()?.offset != withdrawn_offset
        {
            return None;
        }

        let payload = snapshot.blocks[0];
        let backing_len = payload.len.checked_add(4)?;
        let withdrawn_len = snapshot.free_left.checked_sub(heap.free_left)?;
        if withdrawn_len != payload.len && withdrawn_len != backing_len {
            return None;
        }

        let mut recovered = snapshot.blocks.get(1..)?.to_vec();
        if recovered.is_empty() {
            return None;
        }
        for (index, block) in recovered.iter().enumerate() {
            let address = GuestAddr(heap.base.checked_add(block.offset)?);
            let expected_next = recovered
                .get(index + 1)
                .map_or(snapshot.terminator, |next| next.offset);
            if self.memory.read_u32(address).ok()? != expected_next
                || self.memory.read_u32(address.checked_add(4).ok()?).ok()? != block.len
            {
                return None;
            }
        }

        let recovered_total = recovered
            .iter()
            .try_fold(0_u32, |total, block| total.checked_add(block.len))?;
        let counter_correction = match heap.free_left.cmp(&recovered_total) {
            std::cmp::Ordering::Greater | std::cmp::Ordering::Equal => {
                let correction = heap.free_left - recovered_total;
                if correction > payload.len {
                    return None;
                }
                correction
            }
            std::cmp::Ordering::Less => {
                let alignment_slop = recovered_total - heap.free_left;
                if alignment_slop >= HEAP_ALIGNMENT {
                    return None;
                }
                let tail = recovered.last_mut()?;
                tail.len = tail.len.checked_sub(alignment_slop)?;
                if tail.len < FREE_BLOCK_HEADER_LEN {
                    return None;
                }
                0
            }
        };
        validate_free_block_ranges(&recovered, heap.span).ok()?;
        Some((recovered, snapshot.terminator, counter_correction))
    }

    fn recover_corrupted_free_header(
        &self,
        heap: GuestHeapState,
        corrupted_offset: u32,
    ) -> Option<(Vec<FreeBlock>, u32, u32)> {
        let snapshot = self.guest_heap_snapshot.as_ref()?;
        if snapshot.base != heap.base || snapshot.span != heap.span || snapshot.head != heap.head {
            return None;
        }

        let index = snapshot
            .blocks
            .iter()
            .position(|block| block.offset == corrupted_offset)?;
        let block = snapshot.blocks[index];
        let next = snapshot
            .blocks
            .get(index + 1)
            .map_or(snapshot.terminator, |block| block.offset);

        if snapshot.free_left != heap.free_left {
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

fn free_candidate_is_available(
    offset: u32,
    block_len: u32,
    heap_span: u32,
    free_blocks: &[FreeBlock],
) -> bool {
    let Some(end) = offset.checked_add(block_len) else {
        return false;
    };
    block_len >= FREE_BLOCK_HEADER_LEN
        && offset < heap_span
        && end <= heap_span
        && offset.is_multiple_of(HEAP_ALIGNMENT)
        && free_blocks.iter().all(|block| {
            let block_end = block.offset + block.len;
            end <= block.offset || offset >= block_end
        })
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
