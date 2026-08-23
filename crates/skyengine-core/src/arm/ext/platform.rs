use super::*;
use encoding_rs::GBK;
use jpeg_decoder::{Decoder, ImageInfo, PixelFormat};

const MAX_PLATFORM_JPEG_SOURCE_LEN: usize = 32 * 1024 * 1024;
const MAX_PLATFORM_JPEG_DECODED_LEN: usize = 32 * 1024 * 1024;

impl ExtRuntime {
    fn read_platform_jpeg(
        &self,
        source: GuestAddr,
        source_len: usize,
    ) -> Result<Option<(Vec<u8>, ImageInfo)>> {
        if source.0 == 0 || !(3..=MAX_PLATFORM_JPEG_SOURCE_LEN).contains(&source_len) {
            return Ok(None);
        }
        let bytes = self.memory.read(source, source_len)?;
        if !bytes.starts_with(&[0xff, 0xd8, 0xff]) {
            return Ok(None);
        }
        let mut decoder = Decoder::new(std::io::Cursor::new(&bytes));
        if decoder.read_info().is_err() {
            return Ok(None);
        }
        let Some(info) = decoder.info() else {
            return Ok(None);
        };
        let pixel_count = usize::from(info.width)
            .checked_mul(usize::from(info.height))
            .ok_or_else(|| Error::ResourceLimit("platform JPEG dimensions overflow".into()))?;
        let output_len = pixel_count
            .checked_mul(2)
            .ok_or_else(|| Error::ResourceLimit("platform JPEG output length overflow".into()))?;
        let decoded_len = pixel_count
            .checked_mul(info.pixel_format.pixel_bytes())
            .ok_or_else(|| Error::ResourceLimit("platform JPEG decoded length overflow".into()))?;
        if info.width == 0
            || info.height == 0
            || output_len > MAX_PLATFORM_JPEG_DECODED_LEN
            || decoded_len > MAX_PLATFORM_JPEG_DECODED_LEN
            || matches!(info.pixel_format, PixelFormat::L16 | PixelFormat::CMYK32)
        {
            return Ok(None);
        }
        Ok(Some((bytes, info)))
    }

    fn decode_platform_jpeg_rgb565(bytes: Vec<u8>, info: ImageInfo) -> Option<Vec<u8>> {
        let pixel_count = usize::from(info.width).checked_mul(usize::from(info.height))?;
        let decoded_len = pixel_count.checked_mul(info.pixel_format.pixel_bytes())?;
        let output_len = pixel_count.checked_mul(2)?;
        let mut decoder = Decoder::new(std::io::Cursor::new(bytes));
        decoder.set_max_decoding_buffer_size(decoded_len);
        let mut pixels = decoder.decode().ok()?;
        if pixels.len() != decoded_len {
            return None;
        }
        match info.pixel_format {
            PixelFormat::RGB24 => {
                for index in 0..pixel_count {
                    let source = index * 3;
                    let destination = index * 2;
                    let color = Self::rgb888_to_rgb565(
                        pixels[source],
                        pixels[source + 1],
                        pixels[source + 2],
                    );
                    pixels[destination..destination + 2].copy_from_slice(&color.to_le_bytes());
                }
                pixels.truncate(output_len);
            }
            PixelFormat::L8 => {
                pixels.resize(output_len, 0);
                for index in (0..pixel_count).rev() {
                    let luminance = pixels[index];
                    let color = Self::rgb888_to_rgb565(luminance, luminance, luminance);
                    let destination = index * 2;
                    pixels[destination..destination + 2].copy_from_slice(&color.to_le_bytes());
                }
            }
            PixelFormat::L16 | PixelFormat::CMYK32 => return None,
        }
        Some(pixels)
    }

    fn rgb888_to_rgb565(red: u8, green: u8, blue: u8) -> u16 {
        (u16::from(red >> 3) << 11) | (u16::from(green >> 2) << 5) | u16::from(blue >> 3)
    }

    pub(super) fn return_platform_jpeg_info(&mut self, cpu: &mut ArmCpu) -> Result<()> {
        if cpu.register(2) != 12 {
            return Err(Error::Abi(format!(
                "platform JPEG info input is {} bytes, expected 12",
                cpu.register(2)
            )));
        }
        let output = GuestAddr(cpu.register(3));
        let output_len = GuestAddr(self.memory.read_u32(GuestAddr(cpu.register(13)))?);
        if output.0 == 0 || output_len.0 == 0 {
            return Err(Error::Abi(
                "platform JPEG info requires output and output-length pointers".into(),
            ));
        }
        self.memory.write_u32(output, 0)?;
        self.memory.write_u32(output_len, 0)?;

        let input = GuestAddr(cpu.register(1));
        if input.0 == 0 {
            cpu.set_register(0, u32::MAX);
            return Ok(());
        }
        let source = GuestAddr(self.memory.read_u32(input)?);
        let source_len = self.memory.read_u32(input.checked_add(4)?)? as usize;
        let codec = self.memory.read_u32(input.checked_add(8)?)?;
        let Some((_bytes, info)) = (codec == 1)
            .then(|| self.read_platform_jpeg(source, source_len))
            .transpose()?
            .flatten()
        else {
            cpu.set_register(0, u32::MAX);
            return Ok(());
        };

        self.memory
            .write_u32(PLATFORM_JPEG_INFO_DATA, u32::from(info.width))?;
        self.memory.write_u32(
            PLATFORM_JPEG_INFO_DATA.checked_add(4)?,
            u32::from(info.height),
        )?;
        self.memory.write_u32(output, PLATFORM_JPEG_INFO_DATA.0)?;
        self.memory
            .write_u32(output_len, PLATFORM_JPEG_INFO_LEN as u32)?;
        cpu.set_register(0, 0);
        Ok(())
    }

    pub(super) fn decode_platform_jpeg(&mut self, cpu: &mut ArmCpu) -> Result<()> {
        if cpu.register(2) != 24 {
            return Err(Error::Abi(format!(
                "platform JPEG decode input is {} bytes, expected 24",
                cpu.register(2)
            )));
        }
        let output_len = self.memory.read_u32(GuestAddr(cpu.register(13)))?;
        if cpu.register(3) != 0 || output_len != 0 {
            return Err(Error::Abi(
                "platform JPEG decode does not accept output buffers".into(),
            ));
        }
        let input = GuestAddr(cpu.register(1));
        if input.0 == 0 {
            cpu.set_register(0, u32::MAX);
            return Ok(());
        }
        let source = GuestAddr(self.memory.read_u32(input)?);
        let source_len = self.memory.read_u32(input.checked_add(4)?)? as usize;
        let width = self.memory.read_u32(input.checked_add(8)?)?;
        let height = self.memory.read_u32(input.checked_add(12)?)?;
        let codec = self.memory.read_u32(input.checked_add(16)?)?;
        let destination = GuestAddr(self.memory.read_u32(input.checked_add(20)?)?);
        let Some((bytes, info)) = (codec == 1)
            .then(|| self.read_platform_jpeg(source, source_len))
            .transpose()?
            .flatten()
        else {
            cpu.set_register(0, u32::MAX);
            return Ok(());
        };
        if width != u32::from(info.width) || height != u32::from(info.height) {
            cpu.set_register(0, u32::MAX);
            return Ok(());
        }
        let output_len = usize::from(info.width)
            .checked_mul(usize::from(info.height))
            .and_then(|pixels| pixels.checked_mul(2))
            .ok_or_else(|| Error::ResourceLimit("platform JPEG output length overflow".into()))?;
        if destination.0 == 0 {
            cpu.set_register(0, u32::MAX);
            return Ok(());
        }
        if self
            .tracked_guest_allocation_len(destination)
            .is_some_and(|allocated| output_len > allocated as usize)
        {
            cpu.set_register(0, u32::MAX);
            return Ok(());
        }
        self.memory
            .check_range(destination, output_len, Permissions::WRITE)?;
        let Some(pixels) = Self::decode_platform_jpeg_rgb565(bytes, info) else {
            cpu.set_register(0, u32::MAX);
            return Ok(());
        };
        self.memory.write(destination, &pixels)?;
        cpu.set_register(0, 0);
        Ok(())
    }

    fn encode_legacy_string_as_ucs2(legacy: &[u8]) -> Vec<u8> {
        let (decoded, _, _) = GBK.decode(legacy);
        let mut encoded = Vec::with_capacity(decoded.len().saturating_add(1).saturating_mul(2));
        for unit in decoded.encode_utf16() {
            encoded.extend_from_slice(&unit.to_be_bytes());
        }
        encoded.extend_from_slice(&[0, 0]);
        encoded
    }

    pub(super) fn convert_legacy_string_to_ucs2(
        &mut self,
        module: usize,
        cpu: &mut ArmCpu,
    ) -> Result<()> {
        let error_output = GuestAddr(cpu.register(1));
        if error_output.0 != 0 {
            self.memory.write_u32(error_output, u32::MAX)?;
        }

        let input = GuestAddr(cpu.register(0));
        if input.0 == 0 {
            cpu.set_register(0, 0);
            return Ok(());
        }
        let legacy = self.read_c_string(input, 64 * 1024)?;
        let encoded = Self::encode_legacy_string_as_ucs2(&legacy);

        let size_output = GuestAddr(cpu.register(2));
        if size_output.0 != 0 {
            self.memory.write_u32(
                size_output,
                u32::try_from(encoded.len()).map_err(|_| {
                    Error::Abi("legacy string conversion output length exceeds u32".into())
                })?,
            )?;
        }
        let Some(output) = self.allocate_guest_block_for_module(encoded.len(), module)? else {
            cpu.set_register(0, 0);
            return Ok(());
        };
        self.memory.write(output, &encoded)?;
        cpu.set_register(0, output.0);
        Ok(())
    }

    pub(super) fn convert_platform_ucs2_to_legacy(&mut self, cpu: &mut ArmCpu) -> Result<()> {
        let input_len = cpu.register(2) as usize;
        if input_len > 64 * 1024 {
            return Err(Error::ResourceLimit(format!(
                "platform string conversion input is {input_len} bytes (limit 65536)"
            )));
        }
        let output_len = GuestAddr(self.memory.read_u32(GuestAddr(cpu.register(13)))?);
        if output_len.0 == 0 {
            cpu.set_register(0, u32::MAX);
            return Ok(());
        }
        self.memory.write_u32(output_len, 0)?;
        let output_field = GuestAddr(cpu.register(3));
        if output_field.0 == 0 || input_len & 1 != 0 {
            cpu.set_register(0, u32::MAX);
            return Ok(());
        }
        let output = GuestAddr(self.memory.read_u32(output_field)?);
        if output.0 == 0 {
            cpu.set_register(0, u32::MAX);
            return Ok(());
        }
        let input = GuestAddr(cpu.register(1));
        if input.0 == 0 && input_len != 0 {
            cpu.set_register(0, u32::MAX);
            return Ok(());
        }
        let wide = self.memory.read(input, input_len)?;
        let units = wide
            .as_chunks::<2>()
            .0
            .iter()
            .map(|bytes| u16::from_be_bytes(*bytes))
            .collect::<Vec<_>>();
        let decoded = String::from_utf16_lossy(&units);
        let (legacy, _, _) = GBK.encode(&decoded);
        self.memory.write(output, legacy.as_ref())?;
        self.memory
            .write(output.checked_add(legacy.len() as u32)?, &[0])?;
        self.memory.write_u32(
            output_len,
            u32::try_from(legacy.len())
                .map_err(|_| Error::Abi("converted platform string exceeds u32".into()))?,
        )?;
        cpu.set_register(0, 0);
        Ok(())
    }

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

    pub(super) fn return_platform_runtime_profile(&mut self, cpu: &mut ArmCpu) -> Result<()> {
        let output = GuestAddr(cpu.register(3));
        let output_len = GuestAddr(self.memory.read_u32(GuestAddr(cpu.register(13)))?);
        if output.0 == 0 || output_len.0 == 0 {
            if output_len.0 != 0 {
                self.memory.write_u32(output_len, 0)?;
            }
            cpu.set_register(0, u32::MAX);
            return Ok(());
        }
        self.memory
            .write_u32(output, PLATFORM_RUNTIME_PROFILE_DATA.0)?;
        self.memory
            .write_u32(output_len, PLATFORM_RUNTIME_PROFILE_LEN as u32)?;
        cpu.set_register(0, 0);
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
        if !matches!(input.as_slice(), b"C" | b"X" | b"Y" | b"Z") {
            cpu.set_register(0, u32::MAX);
            return Ok(());
        }
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

    pub(super) fn allocate_platform_memory_extension(
        &mut self,
        module: usize,
        cpu: &mut ArmCpu,
    ) -> Result<()> {
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
        let owner_generation = self
            .modules
            .get(module)
            .map(|module| module.generation)
            .ok_or_else(|| {
                Error::Abi(format!("platform allocation for missing module {module}"))
            })?;

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
        self.platform_memory_extensions.insert(
            arena.0,
            PlatformMemoryExtension {
                len: requested_len,
                previous_cursor,
                owner_generation,
            },
        );
        self.memory.write_u32(output, arena.0)?;
        self.memory.write_u32(output_len, cpu.register(2))?;
        cpu.set_register(0, 0);
        Ok(())
    }

    pub(super) fn release_platform_memory_extension(
        &mut self,
        module: usize,
        cpu: &mut ArmCpu,
    ) -> Result<()> {
        let arena = GuestAddr(cpu.register(1));
        if cpu.register(2) != 4 {
            return Err(Error::Abi(format!(
                "platform memory extension release input is {} bytes, expected 4",
                cpu.register(2)
            )));
        }
        let extension = self
            .platform_memory_extensions
            .get(&arena.0)
            .copied()
            .ok_or_else(|| {
                Error::Abi(format!(
                    "platform memory extension release references unknown arena {:#010x}",
                    arena.0
                ))
            })?;
        let owner_generation = self
            .modules
            .get(module)
            .map(|module| module.generation)
            .ok_or_else(|| Error::Abi(format!("platform release for missing module {module}")))?;
        if extension.owner_generation != owner_generation {
            return Err(Error::Abi(format!(
                "module {module} cannot release platform arena {:#010x} owned by another module",
                arena.0
            )));
        }
        let PlatformMemoryExtension {
            len,
            previous_cursor,
            ..
        } = extension;
        let end = arena
            .0
            .checked_add(u32::try_from(len).map_err(|_| {
                Error::Abi(format!(
                    "platform memory extension length {len} exceeds u32"
                ))
            })?)
            .ok_or_else(|| Error::Abi("platform memory extension end overflow".into()))?;
        self.revoke_executable_ranges_in(ExecutableRange { base: arena, len })?;
        self.memory.unmap(arena, len)?;
        self.platform_memory_extensions.remove(&arena.0);
        self.guest_allocation_views
            .retain(|_, view| view.backing_base != arena.0);
        if end == self.platform_memory_cursor {
            self.platform_memory_cursor = previous_cursor;
        }
        cpu.set_register(0, 0);
        Ok(())
    }
}
