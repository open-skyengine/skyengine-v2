use super::*;

impl ExtRuntime {
    pub(super) fn return_unavailable_platform_extension(&mut self, cpu: &mut ArmCpu) -> Result<()> {
        let output = GuestAddr(cpu.register(3));
        if output.0 != 0 {
            self.memory.write_u32(output, 0)?;
        }
        let output_len = GuestAddr(self.memory.read_u32(GuestAddr(cpu.register(13)))?);
        if output_len.0 != 0 {
            self.memory.write_u32(output_len, 0)?;
        }
        cpu.set_register(0, u32::MAX);
        Ok(())
    }

    pub(super) fn return_platform_sim_info(&mut self, cpu: &mut ArmCpu) -> Result<()> {
        let output = GuestAddr(cpu.register(3));
        if output.0 == 0 {
            return Err(Error::Abi(
                "platform SIM query has a null output pointer".into(),
            ));
        }
        let output_len = GuestAddr(self.memory.read_u32(GuestAddr(cpu.register(13)))?);
        if output_len.0 == 0 {
            return Err(Error::Abi(
                "platform SIM query has a null output-length pointer".into(),
            ));
        }
        self.memory.write_u32(output, PLATFORM_SIM_INFO_DATA.0)?;
        self.memory
            .write_u32(output_len, PLATFORM_SIM_INFO_LEN as u32)?;
        cpu.set_register(0, 0);
        Ok(())
    }

    pub(super) fn return_platform_storage_info(&mut self, cpu: &mut ArmCpu) -> Result<()> {
        let input_len = cpu.register(2) as usize;
        let input = self.memory.read(GuestAddr(cpu.register(1)), input_len)?;
        let supported_drive = matches!(
            input.as_slice(),
            [b'C' | b'X' | b'Y' | b'Z']
                | [b'C' | b'X' | b'Y' | b'Z', 0]
                | [b'C' | b'X' | b'Y' | b'Z', b':']
                | [b'C' | b'X' | b'Y' | b'Z', b':', 0]
        );
        if !supported_drive {
            cpu.set_register(0, u32::MAX);
            return Ok(());
        }
        let output = GuestAddr(cpu.register(3));
        if output.0 == 0 {
            return Err(Error::Abi(
                "platform storage query has a null output pointer".into(),
            ));
        }
        let output_len = GuestAddr(self.memory.read_u32(GuestAddr(cpu.register(13)))?);
        if output_len.0 == 0 {
            return Err(Error::Abi(
                "platform storage query has a null output-length pointer".into(),
            ));
        }
        self.memory
            .write_u32(output, PLATFORM_STORAGE_INFO_DATA.0)?;
        self.memory
            .write_u32(output_len, PLATFORM_STORAGE_INFO_LEN as u32)?;
        cpu.set_register(0, 0);
        Ok(())
    }

    pub(super) fn return_platform_storage_drive(&mut self, cpu: &mut ArmCpu) -> Result<()> {
        let input_len = cpu.register(2) as usize;
        let input = self.memory.read(GuestAddr(cpu.register(1)), input_len)?;
        match input.as_slice() {
            b"C" | b"X" | b"Y" | b"Z" => {}
            _ => {
                cpu.set_register(0, u32::MAX);
                return Ok(());
            }
        };
        let output = GuestAddr(cpu.register(3));
        let output_len = GuestAddr(self.memory.read_u32(GuestAddr(cpu.register(13)))?);
        if output.0 == 0 && output_len.0 == 0 {
            cpu.set_register(0, u32::MAX);
            return Ok(());
        }
        if output.0 == 0 {
            return Err(Error::Abi(
                "platform storage drive query has a null output pointer".into(),
            ));
        }
        if output_len.0 == 0 {
            return Err(Error::Abi(
                "platform storage drive query has a null output-length pointer".into(),
            ));
        }
        self.memory
            .write_u32(output, PLATFORM_STORAGE_DRIVE_DATA.0)?;
        self.memory
            .write_u32(output_len, PLATFORM_STORAGE_DRIVE_LEN as u32)?;
        cpu.set_register(0, 0);
        Ok(())
    }

    pub(super) fn allocate_platform_memory_extension(&mut self, cpu: &mut ArmCpu) -> Result<()> {
        let requested_len = cpu.register(2) as usize;
        if requested_len == 0 {
            return Err(Error::Abi(
                "platform memory extension requested zero bytes".into(),
            ));
        }
        let output = GuestAddr(cpu.register(3));
        if output.0 == 0 {
            return Err(Error::Abi(
                "platform memory extension has a null output pointer".into(),
            ));
        }
        let output_len = GuestAddr(self.memory.read_u32(GuestAddr(cpu.register(13)))?);
        if output_len.0 == 0 {
            return Err(Error::Abi(
                "platform memory extension has a null output-length pointer".into(),
            ));
        }

        let previous_cursor = self.platform_memory_cursor;
        let arena_value = previous_cursor
            .checked_add(0xfff)
            .map(|value| value & !0xfff)
            .ok_or_else(|| Error::ArmFault("platform memory alignment overflow".into()))?;
        let requested_len_u32 = u32::try_from(requested_len).map_err(|_| {
            Error::ArmFault(format!(
                "platform memory request {requested_len} does not fit u32"
            ))
        })?;
        let arena_end = arena_value
            .checked_add(requested_len_u32)
            .ok_or_else(|| Error::ArmFault("platform memory request overflow".into()))?;
        let arena = GuestAddr(arena_value);
        self.memory.map(
            arena,
            requested_len,
            Permissions::READ_WRITE,
            "platform memory extension",
        )?;
        self.platform_memory_cursor = arena_end;
        self.platform_memory_extensions
            .insert(arena.0, (requested_len, previous_cursor));
        self.memory.write_u32(output, arena.0)?;
        self.memory.write_u32(output_len, cpu.register(2))?;
        cpu.set_register(0, 0);
        Ok(())
    }

    pub(super) fn release_platform_memory_extension(&mut self, cpu: &mut ArmCpu) -> Result<()> {
        let arena = GuestAddr(cpu.register(1));
        if cpu.register(2) != 4 {
            return Err(Error::Abi(format!(
                "platform memory extension release input is {} bytes, expected 4",
                cpu.register(2)
            )));
        }
        let (len, previous_cursor) = self
            .platform_memory_extensions
            .remove(&arena.0)
            .ok_or_else(|| {
                Error::Abi(format!(
                    "platform memory extension release references unknown arena {:#010x}",
                    arena.0
                ))
            })?;
        let end = arena
            .0
            .checked_add(u32::try_from(len).map_err(|_| {
                Error::Abi(format!(
                    "platform memory extension length {len} exceeds u32"
                ))
            })?)
            .ok_or_else(|| Error::Abi("platform memory extension end overflow".into()))?;
        self.memory.unmap(arena, len)?;
        if end == self.platform_memory_cursor {
            self.platform_memory_cursor = previous_cursor;
        }
        cpu.set_register(0, 0);
        Ok(())
    }
}
