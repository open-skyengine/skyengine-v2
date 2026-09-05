use super::*;

impl ExtRuntime {
    pub(super) fn dispatch(
        &mut self,
        slot: u32,
        module: usize,
        cpu: &mut ArmCpu,
        services: &mut dyn NativeServices,
    ) -> Result<()> {
        let trace_arm = std::env::var_os("SKYENGINE_TRACE_ARM").is_some();
        if trace_arm {
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
            0..=20 => self.dispatch_libc(slot, module, cpu)?,
            25 => {
                let helper = cpu.register(0);
                let expected_image = self
                    .modules
                    .get(module)
                    .ok_or_else(|| {
                        Error::Abi(format!("helper registration for missing module {module}"))
                    })?
                    .executable_image(helper)
                    .map(|(image, _)| image)
                    .ok_or_else(|| {
                        Error::Abi(format!(
                            "helper {helper:#010x} is outside module {module} executable images"
                        ))
                    })?;
                let parameter_len = cpu.register(1).max(20) as usize;
                let parameter = self
                    .allocate_guest_block_for_module(parameter_len, module)?
                    .ok_or_else(|| {
                        Error::ArmFault(
                            "guest heap exhausted while allocating helper parameter".into(),
                        )
                    })?;
                self.memory.write(parameter, &vec![0; parameter_len])?;
                let context = self.modules.get_mut(module).ok_or_else(|| {
                    Error::Abi(format!("helper registration for missing module {module}"))
                })?;
                let function = GuestFunction {
                    module,
                    address: helper,
                    expected_image: Some(expected_image),
                    captured_r9: None,
                };
                context.helper = Some(function);
                context.helper_parameter = parameter;
                self.active_helper = Some(function);
                cpu.set_register(0, parameter.0);
            }
            26 => {
                let format = self.read_c_string(GuestAddr(cpu.register(0)), 64 * 1024)?;
                if trace_arm {
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
                self.draw_platform_bitmap(&pixels, x, y, width, height, services)?;
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
                            if width == 0 || width > 16 || height == 0 || height > 16 {
                                return Err(Error::Abi(format!(
                                    "unsupported character bitmap dimensions {width}x{height} for {codepoint:#06x}"
                                )));
                            }
                            let height_usize = height as usize;
                            let required = height_usize.checked_mul(2).ok_or_else(|| {
                                Error::Abi("character bitmap size overflow".into())
                            })?;
                            if bitmap.len() < required {
                                return Err(Error::Abi(format!(
                                    "character bitmap for {codepoint:#06x} has {} bytes, needs {required}",
                                    bitmap.len()
                                )));
                            }
                            let guest_stride = width.div_ceil(8) as usize;
                            let mut guest_bitmap = Vec::with_capacity(guest_stride * height_usize);
                            for row in bitmap[..required].as_chunks::<2>().0 {
                                guest_bitmap.extend(
                                    row[..guest_stride].iter().map(|byte| byte.reverse_bits()),
                                );
                            }
                            let address = self.allocate(guest_bitmap.len(), 4)?;
                            self.memory.write(address, &guest_bitmap)?;
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
                self.discover_compact_repeating_timers();
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
                let device_datetime = self.current_device_datetime();
                self.memory.write_u16(output, device_datetime.year)?;
                self.memory.write(
                    output.checked_add(2)?,
                    // month, day, hour, minute, second, weekday (Sunday = 0)
                    &[
                        device_datetime.month,
                        device_datetime.day,
                        device_datetime.hour,
                        device_datetime.minute,
                        device_datetime.second,
                        device_datetime.weekday(),
                    ],
                )?;
                cpu.set_register(0, 0);
            }
            35 => {
                let output = GuestAddr(cpu.register(0));
                if output.0 == 0 {
                    cpu.set_register(0, u32::MAX);
                } else {
                    self.memory.write(output, &platform_user_info())?;
                    cpu.set_register(0, 0);
                }
            }
            36 => {
                // The outer runtime owns scheduling; acknowledge guest sleeps
                // without blocking the event and control loops.
                cpu.set_register(0, 0);
            }
            37 => match (cpu.register(0), cpu.register(1)) {
                // Select the normal portrait mode or the rotated landscape mode.
                // The command changes dimensions, not the screen-buffer address.
                (101, mode @ (0 | 3)) => {
                    self.set_screen_orientation(mode == 3, services)?;
                    cpu.set_register(0, 0);
                }
                // Poll a non-blocking socket created through slots 84 and 85.
                (1_001, handle) => {
                    cpu.set_register(0, self.native_socket_state(handle as i32) as u32)
                }
                // Baseline SDK initialization notification; the return value is ignored.
                (1_106, 0) => cpu.set_register(0, 0),
                // Network/payment helpers announce their foreground operation.
                // The headless host has no separate foreground UI state.
                (1_011, mode) if mode <= 1 => cpu.set_register(0, 0),
                // Optional device metric. Repository EXT callers decode values above
                // 1000 and explicitly treat -1 as an unavailable neutral result.
                (1_101, 2) => cpu.set_register(0, u32::MAX),
                // Optional runtime profile query. The caller has a defined
                // fallback state for values other than the two vendor profiles.
                (1_100, 0) => cpu.set_register(0, u32::MAX),
                // Report the normal storage profile. 1002 denotes USB mass-storage
                // mode, in which applications must not access their regular volume.
                (1_218, 0) => cpu.set_register(0, 1_001),
                // RX initialization announces its default platform mode and does
                // not consume a result beyond whether the call is accepted.
                (1_214, mode) if mode <= 1 => cpu.set_register(0, 0),
                // Network request compatibility version used by message.ext.
                (1_205, 0) => cpu.set_register(0, 1_001),
                // Initialize the motion-event provider. Samples remain disabled until
                // the guest selects the verified event-driven mode below.
                (1_206, 0) => {
                    self.motion_active = false;
                    cpu.set_register(0, 0);
                }
                // Optional device effects used by talkcat's downloaded belly and
                // fart actions. The deterministic headless profile has no provider.
                (1_211, 2 | 3) => cpu.set_register(0, 0),
                // Query and configure the same deterministic motion provider.
                // Mode 2 enables event delivery; command 4003 disables it when
                // the guest declines motion input or leaves the active mode.
                (4_002, 0) => cpu.set_register(0, 0),
                (4_003, 0) => {
                    self.motion_active = false;
                    cpu.set_register(0, 0);
                }
                (4_005, 2) => {
                    self.motion_active = true;
                    cpu.set_register(0, 0);
                }
                // Native audio wrappers use a five-step multimedia volume.
                (1_302, volume) if volume <= 5 => {
                    services.set_sound_volume(volume as u8)?;
                    cpu.set_register(0, 0);
                }
                // Optional dual-SIM selection probe. A false result keeps the
                // guest on its default network selection path.
                (1_327, 0) => cpu.set_register(0, u32::MAX),
                // No explicit SIM/network selection is configured.
                (1_328, 0) => cpu.set_register(0, u32::MAX),
                // Optional runtime-service notifications with no headless state.
                // Accept only the observed parameterless forms.
                (1_016 | 1_018 | 1_215 | 1_216, 0) => cpu.set_register(0, 0),
                // Parameterless platform notification. Its wrapper normalizes the
                // result to 0/-1, and every verified caller discards that result.
                (2_703, 0) => cpu.set_register(0, 0),
                // The network-state notification uses 1 for the normal profile;
                // callers branch away from startup unless they receive 1 or 1000.
                (1_020, 0) => cpu.set_register(0, 1),
                // Legacy file-position query. Successful positions are encoded
                // with a 1000 bias; invalid handles retain the normal -1 result.
                (1_231, handle) => {
                    let result = services
                        .seek_file(handle as i32, 0, 1)?
                        .and_then(|position| position.checked_add(1_000))
                        .and_then(|position| u32::try_from(position).ok())
                        .unwrap_or(u32::MAX);
                    cpu.set_register(0, result);
                }
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
                // Requests an additional guest-memory arena. Verified capacity
                // probes carry a signed target-minus-current delta in input_len
                // even though input is null; positive values remain byte counts.
                1_014 if cpu.register(1) == 0 => {
                    self.allocate_platform_memory_extension(module, cpu)?
                }
                // Releases an arena returned by command 1014. The ABI carries
                // the 32-bit guest address as a four-byte input buffer.
                1_015 => self.release_platform_memory_extension(module, cpu)?,
                // Resolve the logical application storage volume to a drive.
                1_204 => self.return_platform_storage_drive(cpu)?,
                // Convert caller-owned UCS-2BE into a caller-owned legacy C string.
                1_207 => self.convert_platform_ucs2_to_legacy(cpu)?,
                // Return the deterministic baseline runtime-state record.
                1_224 if cpu.register(1) == 0 && cpu.register(2) == 0 => {
                    self.return_platform_runtime_profile(cpu)?
                }
                // Optional platform metadata query. No metadata provider is configured.
                1_222 => self.return_unavailable_platform_extension(cpu)?,
                // Optional hardware and callback-backed capability queries. The
                // baseline profile exposes neither, and their callers support -1.
                1_017 | 1_324 if cpu.register(1) == 0 && cpu.register(2) == 0 => {
                    self.return_unavailable_platform_extension(cpu)?
                }
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
                // Synchronous JPEG metadata and RGB565 decode operations. These
                // commands use caller-owned source and destination buffers.
                3_001 => self.return_platform_jpeg_info(cpu)?,
                3_002 => self.decode_platform_jpeg(cpu)?,
                // The handle-backed decoder uses an opaque platform object whose
                // full layout and lifetime contract are not part of the verified subset.
                command @ (3_004 | 3_005) => {
                    return Err(Error::Abi(format!(
                        "unsupported platform JPEG handle command {command} called by module {module}"
                    )));
                }
                // Optional platform-state query. Both the parameterless probe and
                // the normal output/output-length form use the unavailable result.
                1_223 if cpu.register(1) == 0 && cpu.register(2) == 0 => {
                    self.return_unavailable_platform_extension(cpu)?
                }
                // Optional parameterless vendor notification. The baseline has no
                // corresponding provider and reports the standard unavailable value.
                2_013
                    if cpu.register(1) == 0
                        && cpu.register(2) == 0
                        && cpu.register(3) == 0
                        && self.memory.read_u32(GuestAddr(cpu.register(13)))? == 0
                        && self
                            .memory
                            .read_u32(GuestAddr(cpu.register(13)).checked_add(4)?)?
                            == 0 =>
                {
                    cpu.set_register(0, u32::MAX)
                }
                // File-backed MP3 playback uses a caller-owned path. The headless
                // profile verifies the resource and consumes it through a silent sink.
                2_023 => {
                    let path_address = GuestAddr(cpu.register(1));
                    let path_len = cpu.register(2) as usize;
                    let stack_pointer = GuestAddr(cpu.register(13));
                    if path_address.0 == 0
                        || path_len == 0
                        || path_len > 4 * 1024
                        || cpu.register(3) != 0
                        || self.memory.read_u32(stack_pointer)? != 0
                        || self.memory.read_u32(stack_pointer.checked_add(4)?)? != 0
                    {
                        return Err(Error::Abi(format!(
                            "unsupported platform MP3 request called by module {module}"
                        )));
                    }
                    let path = self.memory.read(path_address, path_len)?;
                    let components = path
                        .split(|byte| matches!(byte, b'/' | b'\\'))
                        .collect::<Vec<_>>();
                    let file_name = components.last().copied().unwrap_or_default();
                    if path.contains(&0)
                        || components.iter().any(|component| {
                            component.is_empty() || matches!(*component, b"." | b"..")
                        })
                        || file_name.len() <= 4
                        || !file_name[file_name.len() - 4..].eq_ignore_ascii_case(b".mp3")
                    {
                        return Err(Error::Abi(format!(
                            "unsupported platform MP3 path called by module {module}"
                        )));
                    }
                    let mut sound = services.read_sound_file(&path)?;
                    if sound.as_ref().is_none_or(Vec::is_empty) {
                        let package_name = self.read_c_string(PACKAGE_NAME_DATA, 256)?;
                        sound = services.read_package_file(&package_name, file_name)?;
                    }
                    let succeeded = match sound.filter(|bytes| !bytes.is_empty()) {
                        Some(sound) => services.play_sound(SoundType::Mp3, &sound, false).is_ok(),
                        None => false,
                    };
                    cpu.set_register(0, if succeeded { 0 } else { u32::MAX });
                }
                // Starts the file-backed player prepared by 2023. Playback already
                // begins in the host adapter, so this transition must preserve it.
                2_043 => {
                    let input_address = GuestAddr(cpu.register(1));
                    let input_len = cpu.register(2) as usize;
                    let stack_pointer = GuestAddr(cpu.register(13));
                    let has_empty_input = input_address.0 == 0 && input_len == 0;
                    let has_zeroed_player_options = input_address.0 != 0
                        && input_len == 12
                        && self
                            .memory
                            .read(input_address, input_len)?
                            .iter()
                            .all(|byte| *byte == 0);
                    if (!has_empty_input && !has_zeroed_player_options)
                        || cpu.register(3) != 0
                        || self.memory.read_u32(stack_pointer)? != 0
                        || self.memory.read_u32(stack_pointer.checked_add(4)?)? != 0
                    {
                        return Err(Error::Abi(format!(
                            "unsupported platform player-start request called by module {module}"
                        )));
                    }
                    cpu.set_register(0, 0);
                }
                // Stops and releases the file-backed multimedia player. Talkcat
                // issues both transitions before starting its face interaction.
                // The host has no separate prepared-player handle, so both
                // transitions idempotently clear the active sink.
                2_073 | 2_083
                    if cpu.register(1) == 0
                        && cpu.register(2) == 0
                        && cpu.register(3) == 0
                        && self.memory.read_u32(GuestAddr(cpu.register(13)))? == 0
                        && self
                            .memory
                            .read_u32(GuestAddr(cpu.register(13)).checked_add(4)?)?
                            == 0 =>
                {
                    services.stop_sound()?;
                    cpu.set_register(0, 0)
                }
                // Parameterless multimedia-state query. Local callers consistently
                // recognize 1003 as the idle state for the headless audio profile.
                2_093
                    if cpu.register(1) == 0
                        && cpu.register(2) == 0
                        && cpu.register(3) == 0
                        && self.memory.read_u32(GuestAddr(cpu.register(13)))? == 0
                        && self
                            .memory
                            .read_u32(GuestAddr(cpu.register(13)).checked_add(4)?)?
                            == 0 =>
                {
                    cpu.set_register(
                        0,
                        if services.sound_is_active() {
                            1_001
                        } else {
                            1_003
                        },
                    )
                }
                // Caller-owned WAV recording request. The headless profile has no
                // capture provider, so the verified request shape reports unavailable.
                2_700 => {
                    let input_address = GuestAddr(cpu.register(1));
                    let stack_pointer = GuestAddr(cpu.register(13));
                    if input_address.0 == 0
                        || cpu.register(2) != 16
                        || cpu.register(3) != 0
                        || self.memory.read_u32(stack_pointer)? != 0
                        || self.memory.read_u32(stack_pointer.checked_add(4)?)? != 0
                    {
                        return Err(Error::Abi(format!(
                            "unsupported platform WAV recording request called by module {module}"
                        )));
                    }
                    let path_address = GuestAddr(self.memory.read_u32(input_address)?);
                    let reserved_1 = self.memory.read_u32(input_address.checked_add(4)?)?;
                    let reserved_2 = self.memory.read_u32(input_address.checked_add(8)?)?;
                    let mode = self.memory.read_u32(input_address.checked_add(12)?)?;
                    if path_address.0 == 0 || reserved_1 != 0 || reserved_2 != 0 || mode != 1 {
                        return Err(Error::Abi(format!(
                            "unsupported platform WAV recording request called by module {module}"
                        )));
                    }
                    let path = self.read_c_string(path_address, 4 * 1024)?;
                    let components = path
                        .split(|byte| matches!(byte, b'/' | b'\\'))
                        .collect::<Vec<_>>();
                    let file_name = components.last().copied().unwrap_or_default();
                    if components
                        .iter()
                        .any(|component| component.is_empty() || matches!(*component, b"." | b".."))
                        || file_name.len() <= 4
                        || !file_name[file_name.len() - 4..].eq_ignore_ascii_case(b".wav")
                    {
                        return Err(Error::Abi(format!(
                            "unsupported platform WAV recording path called by module {module}"
                        )));
                    }
                    cpu.set_register(0, u32::MAX);
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
                // Optional runtime profile query. The caller has a defined
                // fallback for values other than its two recognized profiles.
                4_033
                    if cpu.register(1) == 0
                        && cpu.register(2) == 0
                        && cpu.register(3) == 0
                        && self.memory.read_u32(GuestAddr(cpu.register(13)))? == 0 =>
                {
                    cpu.set_register(0, u32::MAX)
                }
                command => {
                    let input_len = (cpu.register(2) as usize).min(32);
                    let input = self
                        .memory
                        .read(GuestAddr(cpu.register(1)), input_len)
                        .map(|bytes| format!("{bytes:02x?}"))
                        .unwrap_or_else(|error| format!("unavailable: {error}"));
                    let stack = (0..2)
                        .map(|index| {
                            self.memory
                                .read_u32(GuestAddr(cpu.register(13).wrapping_add(index * 4)))
                        })
                        .collect::<skyengine_arm::Result<Vec<_>>>()
                        .map(|words| format!("{words:08x?}"))
                        .unwrap_or_else(|error| format!("unavailable: {error}"));
                    return Err(Error::Abi(format!(
                        "unsupported platform slot 38 command {command} called by module {module} (input={input}, stack={stack})"
                    )));
                }
            },
            40 => {
                let name = self.read_c_string(GuestAddr(cpu.register(0)), 1024)?;
                let mode = cpu.register(1);
                let result = services.open_file(&name, mode)?;
                cpu.set_register(0, native_file_open_result(result));
            }
            41 => {
                cpu.set_register(0, services.close_file(cpu.register(0) as i32)? as u32);
            }
            42 => {
                let name = self.read_c_string(GuestAddr(cpu.register(0)), 1024)?;
                let result = services.file_info(&name)?;
                cpu.set_register(0, result as u32);
            }
            43 => {
                let handle = cpu.register(0) as i32;
                let input = GuestAddr(cpu.register(1));
                let len = cpu.register(2) as i32;
                if handle < 0 || input.0 == 0 || len < 0 {
                    cpu.set_register(0, u32::MAX);
                    return Ok(());
                }
                let bytes = self.memory.read(input, len as usize)?;
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
                let position = services.seek_file(
                    cpu.register(0) as i32,
                    cpu.register(1) as i32,
                    cpu.register(2),
                )?;
                cpu.set_register(0, if position.is_some() { 0 } else { u32::MAX });
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
            55 => {
                services.start_shake(cpu.register(0))?;
                cpu.set_register(0, 0);
            }
            56 => {
                services.stop_shake()?;
                cpu.set_register(0, 0);
            }
            57 => {
                let sound_type = cpu.register(0);
                let sound = GuestAddr(cpu.register(1));
                let len = cpu.register(2) as usize;
                let looped = cpu.register(3);
                let Some(sound_type) = SoundType::from_mrp(sound_type) else {
                    return Err(Error::Abi(format!(
                        "unsupported headless sound request (type {}, address {:#010x}, len {len}, looped {looped}) called by module {module}",
                        cpu.register(0),
                        sound.0,
                    )));
                };
                if sound.0 == 0 || len == 0 || !matches!(looped, 0 | 1 | u32::MAX) {
                    return Err(Error::Abi(format!(
                        "unsupported headless sound request (type {}, address {:#010x}, len {len}, looped {looped}) called by module {module}",
                        cpu.register(0),
                        sound.0,
                    )));
                }
                let data = self.memory.read(sound, len)?;
                let succeeded = services.play_sound(sound_type, &data, looped != 0).is_ok();
                cpu.set_register(0, if succeeded { 0 } else { u32::MAX });
            }
            58 => {
                services.stop_sound()?;
                cpu.set_register(0, 0);
            }
            59 => {
                let number = GuestAddr(cpu.register(0));
                let message = GuestAddr(cpu.register(1));
                let message_len = cpu.register(2) as usize;
                if number.0 == 0 || message.0 == 0 || message_len == 0 || message_len > 64 * 1024 {
                    cpu.set_register(0, u32::MAX);
                    return Ok(());
                }
                let number = self.read_c_string(number, 64)?;
                if number.is_empty() {
                    cpu.set_register(0, u32::MAX);
                    return Ok(());
                }
                // Consume the bounded request without exposing a host messaging
                // capability. The deterministic headless provider reports both
                // synchronous acceptance and an asynchronous successful result.
                let _ = self.memory.read(message, message_len)?;
                if let Some(context) = self.modules.get(module) {
                    if self.pending_sms_results.len() >= MAX_PENDING_SMS_RESULTS {
                        cpu.set_register(0, u32::MAX);
                        return Ok(());
                    }
                    if let Some(helper) = context.helper {
                        self.pending_sms_results.push_back(PendingSmsResult {
                            owner_generation: context.generation,
                            helper,
                            result: 0,
                        });
                    }
                }
                cpu.set_register(0, 0);
            }
            61 => {
                // The offline profile still exposes a deterministic default
                // network identity; connectivity is reported by socket calls.
                cpu.set_register(0, 0);
            }
            63 => {
                let title = self.read_wide_string_be(GuestAddr(cpu.register(0)), 1024)?;
                let item_count = cpu.register(1) as usize;
                let handle = self.create_platform_menu(title, item_count)?;
                cpu.set_register(0, handle);
            }
            64 => {
                let handle = cpu.register(0);
                let text = self.read_wide_string_be(GuestAddr(cpu.register(1)), 1024)?;
                let index = cpu.register(2) as usize;
                let succeeded = self.set_platform_menu_item(handle, index, text);
                cpu.set_register(0, if succeeded { 0 } else { u32::MAX });
            }
            65 => {
                let succeeded = self.show_platform_menu(cpu.register(0), services)?;
                cpu.set_register(0, if succeeded { 0 } else { u32::MAX });
            }
            67 => {
                let succeeded = self.release_platform_menu(cpu.register(0), services)?;
                cpu.set_register(0, if succeeded { 0 } else { u32::MAX });
            }
            68 => {
                let succeeded = self.refresh_platform_menu(cpu.register(0), services)?;
                cpu.set_register(0, if succeeded { 0 } else { u32::MAX });
            }
            69 => {
                let title = self.read_wide_string_be(GuestAddr(cpu.register(0)), 1024)?;
                let message = self.read_wide_string_be(GuestAddr(cpu.register(1)), 16 * 1024)?;
                let style = cpu.register(2);
                let handle = self.create_platform_dialog(&title, &message, style, services)?;
                cpu.set_register(0, handle);
            }
            70 => {
                let succeeded = self.release_platform_dialog(cpu.register(0), services)?;
                cpu.set_register(0, if succeeded { 0 } else { u32::MAX });
            }
            71 => {
                let handle = cpu.register(0);
                let Some(dialog) = self.dialogs.get(&handle) else {
                    cpu.set_register(0, u32::MAX);
                    return Ok(());
                };
                let screen = dialog.dialog_screen.clone();
                self.memory.write(self.screen_base, &screen)?;
                self.present_screen(services)?;
                cpu.set_register(0, 0);
            }
            72 => {
                let title = self.read_wide_string_be(GuestAddr(cpu.register(0)), 1024)?;
                let text = self.read_wide_string_be(GuestAddr(cpu.register(1)), 16 * 1024)?;
                let style = cpu.register(2);
                let handle = self.create_platform_text_viewer(&title, &text, style, services)?;
                cpu.set_register(0, handle);
            }
            73 => {
                let succeeded = self.release_platform_text_viewer(cpu.register(0), services)?;
                cpu.set_register(0, if succeeded { 0 } else { u32::MAX });
            }
            74 => {
                let succeeded = self.refresh_platform_text_viewer(cpu.register(0), services)?;
                cpu.set_register(0, if succeeded { 0 } else { u32::MAX });
            }
            75 => {
                let max_code_units = cpu.register(3) as usize;
                if max_code_units > MAX_PLATFORM_EDITOR_CODE_UNITS {
                    return Err(Error::ResourceLimit(format!(
                        "platform editor requested {max_code_units} code units (limit {MAX_PLATFORM_EDITOR_CODE_UNITS})"
                    )));
                }
                let title_address = GuestAddr(cpu.register(0));
                let text_address = GuestAddr(cpu.register(1));
                let title = if title_address.0 == 0 {
                    Vec::new()
                } else {
                    self.read_wide_string_be(title_address, 1024)?
                };
                let text = if text_address.0 == 0 {
                    Vec::new()
                } else {
                    self.read_wide_string_be(text_address, max_code_units.saturating_add(1))?
                };
                let handle = self.create_platform_editor(
                    module,
                    title,
                    text,
                    cpu.register(2),
                    max_code_units,
                )?;
                cpu.set_register(0, handle);
            }
            76 => {
                let succeeded = self.release_platform_editor(module, cpu.register(0))?;
                cpu.set_register(0, if succeeded { 0 } else { u32::MAX });
            }
            77 => {
                let text = self.platform_editor_text(module, cpu.register(0))?;
                cpu.set_register(0, text.map_or(0, |address| address.0));
            }
            78 => {
                // Headless windows are opaque lifetime tokens. The guest requires
                // a positive handle even though no host-native window is created.
                cpu.set_register(0, self.create_native_window(module)?);
            }
            79 => {
                let succeeded = self.release_native_window(module, cpu.register(0))?;
                cpu.set_register(0, if succeeded { 0 } else { u32::MAX });
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
                let result: i32 = if self.native_sockets.remove(&handle).is_some() {
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
                let draw_mode = match mode {
                    0 => BitmapDrawMode::Or,
                    2 => BitmapDrawMode::Copy,
                    6 => BitmapDrawMode::Transparent(transparent_color),
                    8 => BitmapDrawMode::Gray(transparent_color),
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
                self.draw_bitmap_region_to_screen(&pixels, x, y, width, height, draw_mode)?;
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
                // The vendor routine clobbers this volatile argument register.
                // Some legacy callers rely on that before omitting a later r3 argument.
                cpu.set_register(3, 0);
            }
            123 => {
                let stack = GuestAddr(cpu.register(13));
                let is_unicode = self.memory.read_u32(stack.checked_add(8)?)?;
                let font = self.memory.read_u32(stack.checked_add(12)?)?;
                if font > 2 {
                    return Err(Error::Abi(format!(
                        "unsupported text drawing font {font} called by module {module}"
                    )));
                }
                let text_address = GuestAddr(cpu.register(0));
                let text = if text_address.0 == 0 {
                    Vec::new()
                } else if is_unicode == 0 {
                    let encoded = self.read_c_string(text_address, 64 * 1024)?;
                    let (decoded, _, _) = encoding_rs::GBK.decode(&encoded);
                    decoded.encode_utf16().collect()
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
                    font,
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
                    self.compact_ram_output_target(
                        GuestAddr(ram_address),
                        ram_len,
                        bytes.len(),
                        module,
                        GuestAddr(cpu.register(1)),
                    )?
                } else {
                    None
                };
                let output = match prepared_output {
                    Some(output) => output,
                    None => {
                        let Some(output) =
                            self.allocate_guest_block_for_module(bytes.len(), module)?
                        else {
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
                if prepared_output.is_some() {
                    let block_len = heap::aligned_heap_len(bytes.len())?;
                    self.claim_prepared_output_for_module(output, block_len, module)?;
                }
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
                    let requested = ExecutableRange {
                        base: address,
                        len: len as usize,
                    };
                    let Some(context_generation) =
                        self.modules.get(module).map(|context| context.generation)
                    else {
                        return Err(Error::Abi(format!(
                            "executable-range registration for missing module {module}"
                        )));
                    };
                    let requested_end = requested.end().ok_or_else(|| {
                        Error::Abi("dynamic executable image range overflow".into())
                    })?;
                    let mtk_window = ExecutableRange {
                        base: MTK_NATIVE_EXTENSION_BASE,
                        len: MTK_NATIVE_EXTENSION_LEN,
                    };
                    let uses_mtk_window = mtk_window.contains_range(requested);
                    let allocation_owner = if uses_mtk_window {
                        self.mtk_native_extension_owner
                            .unwrap_or(context_generation)
                    } else {
                        self.allocation_owner_for_range(requested).ok_or_else(|| {
                            Error::Abi(format!(
                                "dynamic executable image {:#010x}..{requested_end:#010x} is not inside memory allocated by module {module}",
                                address.0
                            ))
                        })?
                    };
                    if allocation_owner != context_generation {
                        return Err(Error::Abi(format!(
                            "dynamic executable image {:#010x}..{requested_end:#010x} belongs to another module",
                            address.0
                        )));
                    }

                    let mut compatible_images = Vec::new();
                    for (owner, candidate) in self.modules.iter().enumerate() {
                        if candidate.image_range().overlaps(requested) {
                            return Err(Error::Abi(format!(
                                "dynamic executable image {:#010x}..{requested_end:#010x} overlaps module {owner}",
                                address.0,
                            )));
                        }
                        for (image_index, image) in
                            candidate.dynamic_executable_ranges.iter().enumerate()
                        {
                            let Some(image) = image.as_ref() else {
                                continue;
                            };
                            if !image
                                .intervals
                                .iter()
                                .any(|range| range.overlaps(requested))
                            {
                                continue;
                            }
                            if owner == module {
                                compatible_images.push(image_index);
                                continue;
                            }
                            return Err(Error::Abi(format!(
                                "dynamic executable image {:#010x}..{requested_end:#010x} overlaps module {owner} image {image_index}",
                                address.0,
                            )));
                        }
                    }

                    let mut extended_image = None;
                    let new_intervals = if compatible_images.len() == 1 {
                        let image_index = compatible_images[0];
                        let mut intervals = self.modules[module].dynamic_executable_ranges
                            [image_index]
                            .as_ref()
                            .expect("compatible dynamic image exists")
                            .intervals
                            .clone();
                        intervals.push(requested);
                        let intervals = merge_executable_intervals(intervals);
                        let merged_base = intervals
                            .first()
                            .map(|range| range.base.0)
                            .unwrap_or(requested.base.0);
                        let merged_end = intervals
                            .iter()
                            .filter_map(|range| range.end())
                            .max()
                            .unwrap_or(requested_end);
                        let merged_bounds = ExecutableRange {
                            base: GuestAddr(merged_base),
                            len: (merged_end - merged_base) as usize,
                        };
                        let merged_owner = if mtk_window.contains_range(merged_bounds) {
                            self.mtk_native_extension_owner
                                .unwrap_or(context_generation)
                        } else {
                            self.allocation_owner_for_range(merged_bounds).ok_or_else(|| {
                                Error::Abi(format!(
                                    "merged dynamic executable image {merged_base:#010x}..{merged_end:#010x} is not inside one tracked allocation"
                                ))
                            })?
                        };
                        if merged_owner != context_generation {
                            return Err(Error::Abi(format!(
                                "merged dynamic executable image {merged_base:#010x}..{merged_end:#010x} belongs to another module"
                            )));
                        }
                        extended_image = Some((image_index, intervals));
                        Vec::new()
                    } else if compatible_images.is_empty() {
                        vec![requested]
                    } else {
                        // Slot 131 grants execute permission; it does not replace code that
                        // was already registered. Preserve every existing image identity so
                        // live callbacks remain tied to the bytes that established them, and
                        // track only newly covered gaps as a distinct image.
                        let mut uncovered = vec![requested];
                        for image_index in &compatible_images {
                            let image = self.modules[module].dynamic_executable_ranges
                                [*image_index]
                                .as_ref()
                                .expect("compatible dynamic image exists");
                            for interval in &image.intervals {
                                uncovered = uncovered
                                    .into_iter()
                                    .flat_map(|range| range.subtract(*interval))
                                    .collect();
                            }
                        }
                        merge_executable_intervals(uncovered)
                    };
                    if !new_intervals.is_empty()
                        && !self.modules[module]
                            .dynamic_executable_ranges
                            .iter()
                            .any(DynamicExecutableImageSlot::is_none)
                        && self.modules[module].dynamic_executable_ranges.len() >= 64
                    {
                        return Err(Error::Abi(format!(
                            "module {module} exceeded 64 dynamic executable images"
                        )));
                    }
                    let vacant_image = self.modules[module]
                        .dynamic_executable_ranges
                        .iter()
                        .position(DynamicExecutableImageSlot::is_none);
                    let new_image = if !new_intervals.is_empty() {
                        let id = self.modules[module].next_dynamic_executable_image_id;
                        let next_id = id.checked_add(1).ok_or_else(|| {
                            Error::Abi("dynamic executable image identifier overflow".into())
                        })?;
                        Some((vacant_image, id, next_id))
                    } else {
                        None
                    };
                    let image = self.memory.read(address, len as usize)?;
                    let module_parameter = self.registered_dynamic_image_parameter(
                        &image,
                        address,
                        len,
                        context_generation,
                    );
                    if std::env::var_os("SKYENGINE_TRACE_ARM").is_some() {
                        eprintln!(
                            "[arm-executable] address={:#010x} len={len:#x} head={:02x?}",
                            address.0,
                            &image[..image.len().min(64)]
                        );
                    }
                    self.memory
                        .add_permissions(address, len as usize, Permissions::EXECUTE)?;
                    if uses_mtk_window && self.mtk_native_extension_owner.is_none() {
                        self.mtk_native_extension_owner = Some(context_generation);
                    }
                    if let Some((image_index, intervals)) = extended_image {
                        let image = self.modules[module].dynamic_executable_ranges[image_index]
                            .as_mut()
                            .expect("compatible dynamic image exists");
                        image.intervals = intervals;
                        if image.module_parameter.is_none() {
                            image.module_parameter = module_parameter;
                        }
                    } else if let Some((vacant_image, id, next_id)) = new_image {
                        let dynamic_image = DynamicExecutableImage {
                            id,
                            intervals: new_intervals,
                            module_parameter,
                            compact_repeating_timers: Vec::new(),
                        };
                        self.modules[module].next_dynamic_executable_image_id = next_id;
                        if let Some(image_index) = vacant_image {
                            self.modules[module].dynamic_executable_ranges[image_index].0 =
                                Some(dynamic_image);
                        } else {
                            self.modules[module]
                                .dynamic_executable_ranges
                                .push(DynamicExecutableImageSlot(Some(dynamic_image)));
                        }
                    }
                    cpu.set_register(0, 0);
                }
                (command, argument, address, len) => {
                    return Err(Error::Abi(format!(
                        "unsupported platform slot 131 command ({command}, {argument}, {address:#010x}, {len}) called by module {module}"
                    )));
                }
            },
            132 => self.convert_legacy_string_to_ucs2(module, cpu)?,
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
                    .collect::<skyengine_arm::Result<Vec<_>>>()
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
