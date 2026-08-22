use super::*;

impl ExtRuntime {
    pub(super) fn dispatch(
        &mut self,
        slot: u32,
        module: usize,
        cpu: &mut ArmCpu,
        services: &mut dyn NativeServices,
    ) -> Result<()> {
        if std::env::var_os("SKYENGINE_TRACE_ARM").is_some() {
            eprintln!(
                "[arm-trap] module={module} slot={slot} r0={:#010x} r1={:#010x} r2={:#010x} r3={:#010x} r9={:#010x}",
                cpu.register(0),
                cpu.register(1),
                cpu.register(2),
                cpu.register(3),
                cpu.register(9),
            );
        }
        match slot {
            0..=20 => self.dispatch_libc(slot, cpu)?,
            25 => {
                let helper = cpu.register(0);
                let parameter_len = cpu.register(1).max(20) as usize;
                let parameter = self.allocate(parameter_len, 8)?;
                self.memory.write(parameter, &vec![0; parameter_len])?;
                let function = GuestFunction {
                    module,
                    address: helper,
                };
                let context = self.modules.get_mut(module).ok_or_else(|| {
                    Error::Abi(format!("helper registration for missing module {module}"))
                })?;
                context.helper = Some(function);
                context.helper_parameter = parameter;
                self.active_helper = Some(function);
                cpu.set_register(0, parameter.0);
            }
            26 => {
                let format = self.read_c_string(GuestAddr(cpu.register(0)), 64 * 1024)?;
                if std::env::var_os("SKYENGINE_TRACE_ARM").is_some() {
                    eprintln!(
                        "[guest-printf] format={:?} r1={:#010x} r2={:#010x} r3={:#010x}",
                        String::from_utf8_lossy(&format),
                        cpu.register(1),
                        cpu.register(2),
                        cpu.register(3)
                    );
                }
                cpu.set_register(0, format.len() as u32);
            }
            29 => {
                let source = GuestAddr(cpu.register(0));
                let x = cpu.register(1) as i32;
                let y = cpu.register(2) as i32;
                let width = cpu.register(3) as usize;
                let height = self.memory.read_u32(GuestAddr(cpu.register(13)))? as usize;
                let pixels = self.read_platform_draw_pixels(source, x, y, width, height)?;
                services.draw_bitmap(&pixels, x, y, width, height)?;
                cpu.set_register(0, 0);
            }
            30 => {
                let codepoint = cpu.register(0);
                let font = cpu.register(1);
                let width_out = GuestAddr(cpu.register(2));
                let height_out = GuestAddr(cpu.register(3));
                let key = (codepoint, font);
                let glyph = match self.glyphs.get(&key).copied() {
                    Some(glyph) => Some(glyph),
                    None => match services.char_bitmap(codepoint, font)? {
                        Some((bitmap, width, height)) => {
                            let bitmap =
                                bitmap.into_iter().map(u8::reverse_bits).collect::<Vec<_>>();
                            let address = self.allocate(bitmap.len(), 4)?;
                            self.memory.write(address, &bitmap)?;
                            let glyph = GuestGlyph {
                                address,
                                width,
                                height,
                            };
                            self.glyphs.insert(key, glyph);
                            Some(glyph)
                        }
                        None => None,
                    },
                };
                let (address, width, height) = glyph
                    .map(|glyph| (glyph.address.0, glyph.width, glyph.height))
                    .unwrap_or((0, 0, 0));
                if width_out.0 != 0 {
                    self.memory.write_u32(width_out, width)?;
                }
                if height_out.0 != 0 {
                    self.memory.write_u32(height_out, height)?;
                }
                cpu.set_register(0, address);
            }
            31 => {
                let delay_ms = cpu.register(0);
                let delay = Duration::from_millis(u64::from(delay_ms));
                self.timer_deadline = Instant::now().checked_add(delay);
                self.memory.write_u32(TIMER_ACTIVE_DATA, 1)?;
                cpu.set_register(0, 0);
            }
            32 => {
                self.timer_deadline = None;
                self.memory.write_u32(TIMER_ACTIVE_DATA, 0)?;
                cpu.set_register(0, 0);
            }
            33 => {
                cpu.set_register(0, self.clock_origin.elapsed().as_millis() as u32);
            }
            34 => {
                let output = GuestAddr(cpu.register(0));
                self.memory.write_u16(output, self.device_date.year)?;
                self.memory.write(
                    output.checked_add(2)?,
                    // month, day, hour, minute, second, weekday (Sunday = 0)
                    &[
                        self.device_date.month,
                        self.device_date.day,
                        0,
                        0,
                        0,
                        self.device_date.weekday(),
                    ],
                )?;
                cpu.set_register(0, 0);
            }
            35 => {
                let output = GuestAddr(cpu.register(0));
                match self.device_info_profile {
                    DeviceInfoProfile::Unavailable => {
                        // The baseline profile has no device-information provider.
                        // Leave the caller-owned output buffer untouched.
                        cpu.set_register(0, u32::MAX);
                    }
                    DeviceInfoProfile::DeterministicMtk if output.0 == 0 => {
                        cpu.set_register(0, u32::MAX);
                    }
                    DeviceInfoProfile::DeterministicMtk => {
                        self.memory.write(output, &platform_user_info())?;
                        cpu.set_register(0, 0);
                    }
                }
            }
            36 => {
                // The outer runtime owns scheduling; acknowledge guest sleeps
                // without blocking the event and control loops.
                cpu.set_register(0, 0);
            }
            37 => match (cpu.register(0), cpu.register(1)) {
                // Poll a non-blocking socket created through slots 84 and 85.
                (1_001, handle) => {
                    cpu.set_register(0, self.native_socket_state(handle as i32) as u32)
                }
                // Baseline SDK initialization notification; the return value is ignored.
                (1_106, 0) => cpu.set_register(0, 0),
                // Optional device metric. Repository EXT callers decode values above
                // 1000 and explicitly treat -1 as an unavailable neutral result.
                (1_101, 2) => cpu.set_register(0, u32::MAX),
                // Report the normal storage profile. 1002 denotes USB mass-storage
                // mode, in which applications must not access their regular volume.
                (1_218, 0) => cpu.set_register(0, 1_001),
                // RX initialization announces its default platform mode and does
                // not consume a result beyond whether the call is accepted.
                (1_214, 0) => cpu.set_register(0, 0),
                // Network request compatibility version used by message.ext.
                (1_205, 0) => cpu.set_register(0, 1_001),
                // Native audio wrappers use a five-step multimedia volume. The
                // deterministic profile has no output device, but still accepts
                // and acknowledges valid gain changes so guest audio state can
                // advance independently of the host sink.
                (1_302, volume) if volume <= 5 => cpu.set_register(0, 0),
                // Optional dual-SIM selection probe. A false result keeps the
                // guest on its default network selection path.
                (1_327, 0) => cpu.set_register(0, u32::MAX),
                // No explicit SIM/network selection is configured.
                (1_328, 0) => cpu.set_register(0, u32::MAX),
                (command, argument) => {
                    return Err(Error::Abi(format!(
                        "unsupported platform slot 37 command ({command}, {argument}) called by module {module} at LR {:#010x} (r2={:#010x}, r3={:#010x})",
                        cpu.register(14),
                        cpu.register(2),
                        cpu.register(3),
                    )));
                }
            },
            38 => match cpu.register(0) {
                // Requests an additional guest-memory arena. The requested byte
                // count is carried in input_len even though input is null; the
                // returned arena follows the normal mr_platEx output convention.
                1_014 if cpu.register(1) == 0 => self.allocate_platform_memory_extension(cpu)?,
                // Releases an arena returned by command 1014. The ABI carries
                // the 32-bit guest address as a four-byte input buffer.
                1_015 => self.release_platform_memory_extension(cpu)?,
                // Resolve the logical application storage volume to a drive.
                1_204 => self.return_platform_storage_drive(cpu)?,
                // Optional platform metadata query. No metadata provider is configured.
                1_222 => self.return_unavailable_platform_extension(cpu)?,
                // Optional device metadata blob used to enrich network requests.
                1_116 if cpu.register(1) == 0 && cpu.register(2) == 0 => {
                    self.return_unavailable_platform_extension(cpu)?
                }
                // Returns the available SIM slots. The headless baseline has no
                // carrier provider, so expose a valid empty result structure.
                1_307 if cpu.register(1) == 0 && cpu.register(2) == 0 => {
                    self.return_platform_sim_info(cpu)?
                }
                // Disk geometry used by the guest's startup space check.
                1_305 => self.return_platform_storage_info(cpu)?,
                // Optional platform control/query without input or output buffers.
                1_223 if cpu.register(1) == 0 && cpu.register(2) == 0 && cpu.register(3) == 0 => {
                    cpu.set_register(0, u32::MAX)
                }
                // Vendor initialization notification with no input or output buffers.
                2_011 if cpu.register(1) == 0 && cpu.register(2) == 0 && cpu.register(3) == 0 => {
                    cpu.set_register(0, 0)
                }
                // Optional vendor capability probe. The baseline headless profile
                // does not provide it, so report the ABI failure value.
                0x0009_0003
                    if cpu.register(1) == 0 && cpu.register(2) == 0 && cpu.register(3) == 0 =>
                {
                    cpu.set_register(0, u32::MAX)
                }
                // Observed optional vendor extension with an opaque input record
                // and no output buffer. This profile does not provide it.
                0x0009_0004
                    if cpu.register(1) != 0 && cpu.register(2) != 0 && cpu.register(3) == 0 =>
                {
                    cpu.set_register(0, u32::MAX)
                }
                // Optional vendor capability structure.
                0x0007_0001 if cpu.register(1) == 0 && cpu.register(2) == 0 => {
                    self.return_unavailable_platform_extension(cpu)?
                }
                command => {
                    return Err(Error::Abi(format!(
                        "unsupported platform slot 38 command {command} called by module {module}"
                    )));
                }
            },
            40 => {
                let name = self.read_c_string(GuestAddr(cpu.register(0)), 1024)?;
                cpu.set_register(0, services.open_file(&name, cpu.register(1))? as u32);
            }
            41 => {
                cpu.set_register(0, services.close_file(cpu.register(0) as i32)? as u32);
            }
            42 => {
                let name = self.read_c_string(GuestAddr(cpu.register(0)), 1024)?;
                cpu.set_register(0, services.file_info(&name)? as u32);
            }
            43 => {
                let handle = cpu.register(0) as i32;
                if handle < 0 {
                    cpu.set_register(0, u32::MAX);
                    return Ok(());
                }
                let bytes = self
                    .memory
                    .read(GuestAddr(cpu.register(1)), cpu.register(2) as usize)?;
                cpu.set_register(
                    0,
                    services
                        .write_file(handle, &bytes)?
                        .and_then(|written| u32::try_from(written).ok())
                        .unwrap_or(u32::MAX),
                );
            }
            44 => {
                let handle = cpu.register(0) as i32;
                let destination = GuestAddr(cpu.register(1));
                let len = cpu.register(2) as usize;
                match services.read_file(handle, len)? {
                    Some(bytes) => {
                        self.memory.write(destination, &bytes)?;
                        cpu.set_register(0, bytes.len() as u32);
                    }
                    None => cpu.set_register(0, u32::MAX),
                }
            }
            45 => {
                let succeeded = services.seek_file(
                    cpu.register(0) as i32,
                    cpu.register(1) as i32,
                    cpu.register(2),
                )?;
                cpu.set_register(0, if succeeded { 0 } else { u32::MAX });
            }
            46 => {
                let name = self.read_c_string(GuestAddr(cpu.register(0)), 1024)?;
                let result = services.file_len(&name)?;
                cpu.set_register(
                    0,
                    result
                        .and_then(|len| u32::try_from(len).ok())
                        .unwrap_or(u32::MAX),
                );
            }
            47 => {
                let name = self.read_c_string(GuestAddr(cpu.register(0)), 1024)?;
                cpu.set_register(0, services.remove_file(&name)? as u32);
            }
            48 => {
                let source = self.read_c_string(GuestAddr(cpu.register(0)), 1024)?;
                let destination = self.read_c_string(GuestAddr(cpu.register(1)), 1024)?;
                cpu.set_register(0, services.rename_file(&source, &destination)? as u32);
            }
            49 | 50 => {
                let name = self.read_c_string(GuestAddr(cpu.register(0)), 1024)?;
                let result = if slot == 49 {
                    services.create_dir(&name)?
                } else {
                    services.remove_dir(&name)?
                };
                cpu.set_register(0, result as u32);
            }
            51 => {
                let directory = self.read_c_string(GuestAddr(cpu.register(0)), 1024)?;
                let output = GuestAddr(cpu.register(1));
                let output_len = cpu.register(2) as usize;
                match services.find_start(&directory)? {
                    Some((handle, entry))
                        if self.write_directory_entry(output, output_len, &entry)? =>
                    {
                        cpu.set_register(0, handle as u32);
                    }
                    Some((handle, _)) => {
                        services.find_stop(handle)?;
                        cpu.set_register(0, u32::MAX);
                    }
                    None => cpu.set_register(0, u32::MAX),
                }
            }
            52 => {
                let handle = cpu.register(0) as i32;
                let output = GuestAddr(cpu.register(1));
                let output_len = cpu.register(2) as usize;
                let succeeded = match services.find_next(handle)? {
                    Some(entry) => self.write_directory_entry(output, output_len, &entry)?,
                    None => false,
                };
                cpu.set_register(0, if succeeded { 0 } else { u32::MAX });
            }
            53 => {
                let succeeded = services.find_stop(cpu.register(0) as i32)?;
                cpu.set_register(0, if succeeded { 0 } else { u32::MAX });
            }
            54 => {
                self.exit_requested = true;
                cpu.set_register(0, 0);
            }
            58 => {
                // The headless profile uses an explicit no-output audio sink.
                // Stopping an absent or completed sound remains idempotent.
                cpu.set_register(0, 0);
            }
            61 => {
                // The offline profile still exposes a deterministic default
                // network identity; connectivity is reported by socket calls.
                cpu.set_register(0, 0);
            }
            69 => {
                let title = self.read_wide_string_be(GuestAddr(cpu.register(0)), 1024)?;
                let message = self.read_wide_string_be(GuestAddr(cpu.register(1)), 16 * 1024)?;
                let style = cpu.register(2);
                let handle = self.create_platform_dialog(&title, &message, style, services)?;
                cpu.set_register(0, handle);
            }
            70 => {
                let handle = cpu.register(0);
                let Some(dialog) = self.dialogs.remove(&handle) else {
                    cpu.set_register(0, u32::MAX);
                    return Ok(());
                };
                self.memory.write(SCREEN_BASE, &dialog.previous_screen)?;
                self.present_screen(services)?;
                cpu.set_register(0, 0);
            }
            71 => {
                let handle = cpu.register(0);
                let Some(dialog) = self.dialogs.get(&handle) else {
                    cpu.set_register(0, u32::MAX);
                    return Ok(());
                };
                let screen = dialog.dialog_screen.clone();
                self.memory.write(SCREEN_BASE, &screen)?;
                self.present_screen(services)?;
                cpu.set_register(0, 0);
            }
            80 => {
                let info = GuestAddr(cpu.register(0));
                let width = self.memory.read_u32(data_slot_address(92))?;
                let height = self.memory.read_u32(data_slot_address(93))?;
                self.memory.write_u32(info, width)?;
                self.memory.write_u32(info.checked_add(4)?, height)?;
                cpu.set_register(0, 0);
            }
            81 => {
                // Initializing the network service does not imply that a link is
                // available. Later DNS/socket operations report connectivity.
                cpu.set_register(0, 0);
            }
            82 => {
                self.native_sockets.clear();
                cpu.set_register(0, 0);
            }
            83 => {
                let name = self.read_c_string(GuestAddr(cpu.register(0)), 1024)?;
                cpu.set_register(0, self.resolve_mapped_host(&name).unwrap_or(u32::MAX));
            }
            84 => {
                let socket_type = cpu.register(0);
                let protocol = cpu.register(1);
                let handle = if socket_type == 0 && protocol == 0 {
                    self.allocate_native_socket_handle()?
                } else {
                    None
                };
                cpu.set_register(0, handle.map_or(u32::MAX, |handle| handle as u32));
            }
            85 => {
                let (ip, port) = self.route_mapped_endpoint(cpu.register(1), cpu.register(2));
                let result =
                    self.connect_native_socket(cpu.register(0) as i32, ip, port, cpu.register(3));
                cpu.set_register(0, result as u32);
            }
            86 => {
                let handle = cpu.register(0) as i32;
                let result = if self.native_sockets.remove(&handle).is_some() {
                    0
                } else {
                    -1
                };
                cpu.set_register(0, result as u32);
            }
            87 => {
                let handle = cpu.register(0) as i32;
                let output = GuestAddr(cpu.register(1));
                let len = cpu.register(2) as usize;
                let bytes = self.receive_native_socket(handle, len);
                match bytes {
                    Some(bytes) => {
                        self.memory.write(output, &bytes)?;
                        cpu.set_register(0, bytes.len() as u32);
                    }
                    None => cpu.set_register(0, u32::MAX),
                }
            }
            88 | 90 => {
                // Datagram sockets are not part of the baseline stream profile.
                cpu.set_register(0, u32::MAX);
            }
            89 => {
                let len = cpu.register(2) as usize;
                let bytes = self.memory.read(GuestAddr(cpu.register(1)), len)?;
                let written = self.send_native_socket(cpu.register(0) as i32, &bytes);
                cpu.set_register(0, written.map_or(u32::MAX, |written| written as u32));
            }
            113 => {
                self.md5_init(GuestAddr(cpu.register(0)))?;
                cpu.set_register(0, 0);
            }
            114 => {
                let context = GuestAddr(cpu.register(0));
                let input = self
                    .memory
                    .read(GuestAddr(cpu.register(1)), cpu.register(2) as usize)?;
                self.md5_append(context, &input)?;
                cpu.set_register(0, 0);
            }
            115 => {
                self.md5_finish(GuestAddr(cpu.register(0)), GuestAddr(cpu.register(1)))?;
                cpu.set_register(0, 0);
            }
            119 => {
                let (width, height) = self.screen_dimensions()?;
                self.write_screen_pixel(
                    cpu.register(0) as i32,
                    cpu.register(1) as i32,
                    cpu.register(2) as u16,
                    width,
                    height,
                )?;
                cpu.set_register(0, 0);
            }
            120 => {
                let source = GuestAddr(cpu.register(0));
                let x = cpu.register(1) as i32;
                let y = cpu.register(2) as i32;
                let width = cpu.register(3) as usize;
                let stack = GuestAddr(cpu.register(13));
                let height = self.memory.read_u32(stack)? as usize;
                let mode = self.memory.read_u32(stack.checked_add(4)?)?;
                let transparent_color = self.memory.read_u32(stack.checked_add(8)?)? as u16;
                let source_x = self.memory.read_u32(stack.checked_add(12)?)? as usize;
                let source_y = self.memory.read_u32(stack.checked_add(16)?)? as usize;
                let source_stride = self.memory.read_u32(stack.checked_add(20)?)? as usize;
                let transparent_color = match mode {
                    2 => None,
                    6 => Some(transparent_color),
                    _ => {
                        return Err(Error::Abi(format!(
                            "unsupported bitmap drawing mode {mode} called by module {module}"
                        )));
                    }
                };
                let source_end_y = source_y
                    .checked_add(height)
                    .ok_or_else(|| Error::Abi("bitmap source height overflow".into()))?;
                let byte_len = width
                    .checked_mul(height)
                    .and_then(|pixels| pixels.checked_mul(2))
                    .ok_or_else(|| Error::Abi("bitmap source byte count overflow".into()))?;
                if byte_len > self.heap_len {
                    return Err(Error::Abi(format!(
                        "bitmap source region requires {byte_len} bytes"
                    )));
                }
                let pixels = if source_x == 0 && width == source_stride {
                    let byte_offset = source_y
                        .checked_mul(source_stride)
                        .and_then(|offset| offset.checked_mul(2))
                        .and_then(|offset| u32::try_from(offset).ok())
                        .ok_or_else(|| Error::Abi("bitmap source offset overflow".into()))?;
                    self.memory
                        .read(source.checked_add(byte_offset)?, byte_len)?
                } else {
                    let row_len = width
                        .checked_mul(2)
                        .ok_or_else(|| Error::Abi("bitmap source row overflow".into()))?;
                    let mut pixels = Vec::with_capacity(byte_len);
                    for row in source_y..source_end_y {
                        let byte_offset = row
                            .checked_mul(source_stride)
                            .and_then(|offset| offset.checked_add(source_x))
                            .and_then(|offset| offset.checked_mul(2))
                            .and_then(|offset| u32::try_from(offset).ok())
                            .ok_or_else(|| Error::Abi("bitmap source offset overflow".into()))?;
                        pixels.extend_from_slice(
                            &self
                                .memory
                                .read(source.checked_add(byte_offset)?, row_len)?,
                        );
                    }
                    pixels
                };
                self.draw_bitmap_region_to_screen(&pixels, x, y, width, height, transparent_color)?;
                cpu.set_register(0, 0);
            }
            121 => {
                let source = self.read_bitmap_descriptor(GuestAddr(cpu.register(0)))?;
                let destination = self.read_bitmap_descriptor(GuestAddr(cpu.register(1)))?;
                let stack = GuestAddr(cpu.register(13));
                let transform_address = GuestAddr(self.memory.read_u32(stack)?);
                let transform = self.read_bitmap_transform(transform_address)?;
                let transparent_color = self.memory.read_u32(stack.checked_add(4)?)? as u16;
                self.copy_transformed_bitmap(
                    destination,
                    source,
                    cpu.register(2) as usize,
                    cpu.register(3) as usize,
                    transform,
                    transparent_color,
                    module,
                )?;
                cpu.set_register(0, 0);
            }
            122 => {
                let stack = GuestAddr(cpu.register(13));
                let color = Framebuffer::rgb565(
                    self.memory.read_u32(stack)? as i32,
                    self.memory.read_u32(stack.checked_add(4)?)? as i32,
                    self.memory.read_u32(stack.checked_add(8)?)? as i32,
                );
                let x = cpu.register(0) as i32;
                let y = cpu.register(1) as i32;
                let width = cpu.register(2) as i32;
                let height = cpu.register(3) as i32;
                self.draw_rectangle_to_screen(x, y, width, height, color)?;
                cpu.set_register(0, 0);
            }
            123 => {
                let stack = GuestAddr(cpu.register(13));
                let flags = self.memory.read_u32(stack.checked_add(12)?)?;
                if flags > 2 {
                    return Err(Error::Abi(format!(
                        "unsupported text drawing flags {flags} called by module {module}"
                    )));
                }
                let text_address = GuestAddr(cpu.register(0));
                let text = if text_address.0 == 0 {
                    Vec::new()
                } else {
                    self.read_wide_string_be(text_address, 64 * 1024)?
                };
                let color = Framebuffer::rgb565(
                    cpu.register(3) as i32,
                    self.memory.read_u32(stack)? as i32,
                    self.memory.read_u32(stack.checked_add(4)?)? as i32,
                );
                self.draw_text_to_screen(
                    &text,
                    cpu.register(1) as i32,
                    cpu.register(2) as i32,
                    color,
                    self.memory.read_u32(stack.checked_add(8)?)?,
                    services,
                )?;
                cpu.set_register(0, 0);
            }
            125 => {
                let name = self.read_c_string(GuestAddr(cpu.register(0)), 1024)?;
                let ram_address = self.memory.read_u32(data_slot_address(104))?;
                let ram_len = self.memory.read_u32(data_slot_address(105))? as usize;
                if (ram_address == 0) != (ram_len == 0) {
                    return Err(Error::Abi(format!(
                        "RAM-backed MRP has inconsistent address {ram_address:#010x} and length {ram_len}"
                    )));
                }
                let use_ram_package = if ram_address == 0 {
                    false
                } else {
                    ram_len >= 24
                        && self.memory.read(GuestAddr(ram_address), 4)?.as_slice() == b"MRPG"
                };
                let bytes = if !use_ram_package {
                    let package_name = self.read_c_string(PACKAGE_NAME_DATA, 256)?;
                    services.read_package_file(&package_name, &name)?
                } else {
                    self.read_ram_package_file(GuestAddr(ram_address), ram_len, &name)?
                };
                if std::env::var_os("SKYENGINE_TRACE_ARM").is_some() {
                    eprintln!(
                        "[arm-package] name={:?} ram={ram_address:#010x}+{ram_len:#x} result_len={:?}",
                        String::from_utf8_lossy(&name),
                        bytes.as_ref().map(Vec::len),
                    );
                }
                let Some(bytes) = bytes else {
                    let len_pointer = GuestAddr(cpu.register(1));
                    if len_pointer.0 != 0 {
                        self.memory.write_u32(len_pointer, 0)?;
                    }
                    cpu.set_register(0, 0);
                    return Ok(());
                };
                let prepared_output = if use_ram_package {
                    self.compact_ram_output_target(GuestAddr(ram_address), ram_len, bytes.len())?
                } else {
                    None
                };
                let output = match prepared_output {
                    Some(output) => output,
                    None => {
                        let Some(output) = self.allocate_guest_block(bytes.len())? else {
                            let len_pointer = GuestAddr(cpu.register(1));
                            if len_pointer.0 != 0 {
                                self.memory.write_u32(len_pointer, 0)?;
                            }
                            cpu.set_register(0, 0);
                            return Ok(());
                        };
                        output
                    }
                };
                self.memory.write(output, &bytes)?;
                let len_pointer = GuestAddr(cpu.register(1));
                if len_pointer.0 != 0 {
                    self.memory.write_u32(len_pointer, bytes.len() as u32)?;
                }
                cpu.set_register(0, output.0);
            }
            130 => match (cpu.register(0), cpu.register(1), cpu.register(2)) {
                // Baseline SDK compatibility probe, equivalent to the MR TestCom stub.
                (0, 7, 9_999) => cpu.set_register(0, 0),
                (command, argument, fallback) => {
                    return Err(Error::Abi(format!(
                        "unsupported platform slot 130 command ({command}, {argument}, {fallback}) called by module {module}"
                    )));
                }
            },
            131 => match (
                cpu.register(0),
                cpu.register(1),
                cpu.register(2),
                cpu.register(3),
            ) {
                // Marks a dynamically loaded native module as executable.
                (0, 9, address, len) if len != 0 => {
                    let address = GuestAddr(address);
                    let image = self.memory.read(address, len as usize)?;
                    if std::env::var_os("SKYENGINE_TRACE_ARM").is_some() {
                        eprintln!(
                            "[arm-executable] address={:#010x} len={len:#x} head={:02x?}",
                            address.0,
                            &image[..image.len().min(64)]
                        );
                    }
                    self.memory
                        .add_permissions(address, len as usize, Permissions::EXECUTE)?;
                    cpu.set_register(0, 0);
                }
                (command, argument, address, len) => {
                    return Err(Error::Abi(format!(
                        "unsupported platform slot 131 command ({command}, {argument}, {address:#010x}, {len}) called by module {module}"
                    )));
                }
            },
            other => {
                let return_address = cpu.register(14) & !1;
                let caller_start = return_address.saturating_sub(24);
                let caller_bytes = self
                    .memory
                    .read(GuestAddr(caller_start), 48)
                    .map(|bytes| format!("{bytes:02x?}"))
                    .unwrap_or_else(|error| format!("unavailable: {error}"));
                let stack_words = (0..6)
                    .map(|index| {
                        self.memory
                            .read_u32(GuestAddr(cpu.register(13).wrapping_add(index * 4)))
                    })
                    .collect::<Result<Vec<_>>>()
                    .map(|words| format!("{words:08x?}"))
                    .unwrap_or_else(|error| format!("unavailable: {error}"));
                let argument_bytes = self
                    .memory
                    .read(GuestAddr(cpu.register(0)), 32)
                    .map(|bytes| format!("{bytes:02x?}"))
                    .unwrap_or_else(|error| format!("unavailable: {error}"));
                let second_argument_bytes = self
                    .memory
                    .read(GuestAddr(cpu.register(1)), 32)
                    .map(|bytes| format!("{bytes:02x?}"))
                    .unwrap_or_else(|error| format!("unavailable: {error}"));
                let stack_record_bytes = self
                    .memory
                    .read_u32(GuestAddr(cpu.register(13)))
                    .and_then(|address| self.memory.read(GuestAddr(address), 32))
                    .map(|bytes| format!("{bytes:02x?}"))
                    .unwrap_or_else(|error| format!("unavailable: {error}"));
                return Err(Error::Abi(format!(
                    "unsupported platform slot {other} called by module {module} at LR {:#010x} (r0={:#010x}, r1={:#010x}, r2={:#010x}, r3={:#010x}, sp={:#010x}, stack={stack_words}, r0-bytes={argument_bytes}, r1-bytes={second_argument_bytes}, stack-record={stack_record_bytes}); guest bytes at {caller_start:#010x}: {caller_bytes}",
                    cpu.register(14),
                    cpu.register(0),
                    cpu.register(1),
                    cpu.register(2),
                    cpu.register(3),
                    cpu.register(13),
                )));
            }
        }
        Ok(())
    }
}
