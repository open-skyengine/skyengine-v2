use super::ram_package::read_le_u32;
use super::*;

impl ExtRuntime {
    pub(super) fn create_platform_dialog(
        &mut self,
        title: &[u16],
        message: &[u16],
        style: u32,
        services: &mut dyn NativeServices,
    ) -> Result<u32> {
        if style != 0 {
            return Err(Error::Abi(format!(
                "unsupported platform dialog style {style}"
            )));
        }
        let (width, height) = self.screen_dimensions()?;
        let screen_len = usize::try_from(width)
            .ok()
            .and_then(|width| {
                usize::try_from(height)
                    .ok()
                    .and_then(|height| width.checked_mul(height))
            })
            .and_then(|pixels| pixels.checked_mul(2))
            .ok_or_else(|| Error::Abi("platform dialog screen size overflow".into()))?;
        let previous_screen = self.memory.read(SCREEN_BASE, screen_len)?;

        let background = Framebuffer::rgb565(248, 252, 248);
        let accent = Framebuffer::rgb565(32, 160, 224);
        let accent_dark = Framebuffer::rgb565(0, 96, 176);
        let black = Framebuffer::rgb565(0, 0, 0);
        let white = Framebuffer::rgb565(255, 255, 255);
        self.draw_rectangle_to_screen(0, 0, width, height, background)?;
        self.draw_rectangle_to_screen(0, 0, width, 30, accent)?;
        self.draw_text_to_screen(title, 8, 7, white, 0, services)?;
        self.draw_wrapped_text_to_screen(message, 12, 48, width - 24, black, services)?;

        let button_width = 120.min(width.saturating_sub(24));
        let button_x = (width - button_width) / 2;
        let button_y = height.saturating_sub(68);
        self.draw_rectangle_to_screen(
            button_x - 1,
            button_y - 1,
            button_width + 2,
            32,
            accent_dark,
        )?;
        self.draw_rectangle_to_screen(button_x, button_y, button_width, 30, accent)?;
        self.draw_text_to_screen(
            &[0x786e, 0x5b9a],
            button_x + button_width / 2 - 16,
            button_y + 7,
            white,
            0,
            services,
        )?;

        let dialog_screen = self.memory.read(SCREEN_BASE, screen_len)?;
        let handle = self.allocate_ui_handle()?;
        self.dialogs.insert(
            handle,
            PlatformDialog {
                previous_screen,
                dialog_screen,
            },
        );
        self.present_screen(services)?;
        Ok(handle)
    }

    pub(super) fn draw_wrapped_text_to_screen(
        &mut self,
        text: &[u16],
        x: i32,
        mut y: i32,
        max_width: i32,
        color: u16,
        services: &mut dyn NativeServices,
    ) -> Result<()> {
        let mut line = Vec::new();
        let mut line_width = 0;
        for &codepoint in text {
            let glyph_width = if codepoint < 128 { 8 } else { 16 };
            if codepoint == b'\n' as u16
                || (!line.is_empty() && line_width + glyph_width > max_width)
            {
                self.draw_text_to_screen(&line, x, y, color, 0, services)?;
                line.clear();
                line_width = 0;
                y += 22;
                if codepoint == b'\n' as u16 {
                    continue;
                }
            }
            line.push(codepoint);
            line_width += glyph_width;
        }
        if !line.is_empty() {
            self.draw_text_to_screen(&line, x, y, color, 0, services)?;
        }
        Ok(())
    }

    pub(super) fn allocate_ui_handle(&mut self) -> Result<u32> {
        let start = self.next_ui_handle;
        loop {
            let handle = self.next_ui_handle;
            self.next_ui_handle = self.next_ui_handle.checked_add(1).unwrap_or(1);
            if handle != 0 && !self.dialogs.contains_key(&handle) {
                return Ok(handle);
            }
            if self.next_ui_handle == start {
                return Err(Error::ResourceLimit(
                    "no platform UI handles available".into(),
                ));
            }
        }
    }

    pub(super) fn present_screen(&self, services: &mut dyn NativeServices) -> Result<()> {
        let (width, height) = self.screen_dimensions()?;
        let byte_len = usize::try_from(width)
            .ok()
            .and_then(|width| {
                usize::try_from(height)
                    .ok()
                    .and_then(|height| width.checked_mul(height))
            })
            .and_then(|pixels| pixels.checked_mul(2))
            .ok_or_else(|| Error::Abi("screen presentation size overflow".into()))?;
        let pixels = self.memory.read(SCREEN_BASE, byte_len)?;
        services.draw_bitmap(&pixels, 0, 0, width as usize, height as usize)
    }

    pub(super) fn read_platform_draw_pixels(
        &self,
        source: GuestAddr,
        x: i32,
        y: i32,
        width: usize,
        height: usize,
    ) -> Result<Vec<u8>> {
        let byte_len = width
            .checked_mul(height)
            .and_then(|pixels| pixels.checked_mul(2))
            .ok_or_else(|| Error::Abi("mr_drawBitmap dimensions overflow".into()))?;
        if byte_len > self.heap_len {
            return Err(Error::Abi(format!(
                "mr_drawBitmap source is {byte_len} bytes"
            )));
        }
        if source != SCREEN_BASE {
            return self.memory.read(source, byte_len);
        }

        let (screen_width, screen_height) = self.screen_dimensions()?;
        let region_width = i64::try_from(width)
            .map_err(|_| Error::Abi("mr_drawBitmap width exceeds i64".into()))?;
        let region_height = i64::try_from(height)
            .map_err(|_| Error::Abi("mr_drawBitmap height exceeds i64".into()))?;
        let region_end_x = i64::from(x) + region_width;
        let region_end_y = i64::from(y) + region_height;
        if x < 0
            || y < 0
            || region_end_x > i64::from(screen_width)
            || region_end_y > i64::from(screen_height)
        {
            return Err(Error::Abi(format!(
                "mr_drawBitmap screen region ({x}, {y}) {width}x{height} exceeds {screen_width}x{screen_height}"
            )));
        }

        let row_byte_len = width
            .checked_mul(2)
            .ok_or_else(|| Error::Abi("mr_drawBitmap row size overflow".into()))?;
        let mut pixels = Vec::with_capacity(byte_len);
        for row in 0..height {
            let row = i32::try_from(row)
                .map_err(|_| Error::Abi("mr_drawBitmap row exceeds i32".into()))?;
            let row_address = self.screen_address(x, y + row, screen_width)?;
            pixels.extend(self.memory.read(row_address, row_byte_len)?);
        }
        Ok(pixels)
    }

    pub(super) fn compact_ram_output_target(
        &self,
        package_address: GuestAddr,
        package_len: usize,
        output_len: usize,
    ) -> Result<Option<GuestAddr>> {
        if package_len < 24 {
            return Ok(None);
        }
        let header = self.memory.read(package_address, 24)?;
        if &header[..4] != b"MRPG"
            || read_le_u32(&header, 4)? != 4
            || read_le_u32(&header, 12)? != 4
        {
            return Ok(None);
        }

        let output_len = u32::try_from(output_len)
            .map_err(|_| Error::Abi("compact RAM MRP output length exceeds u32".into()))?;
        let aligned_len = output_len
            .checked_add(7)
            .map(|len| len & !7)
            .ok_or_else(|| Error::Abi("compact RAM MRP output alignment overflow".into()))?;
        let heap_end = HEAP_BASE.0 + self.heap_len as u32;
        let mut candidates = Vec::new();
        for descriptor_len_address in (HEAP_BASE.0 + 4..heap_end).step_by(4) {
            let recorded_len = self.memory.read_u32(GuestAddr(descriptor_len_address))?;
            if recorded_len != aligned_len {
                continue;
            }
            let candidate = self
                .memory
                .read_u32(GuestAddr(descriptor_len_address - 4))?;
            if candidate & 3 != 0 {
                continue;
            }
            let candidate = GuestAddr(candidate);
            if self
                .memory
                .check_range(candidate, output_len as usize, Permissions::READ_WRITE)
                .is_err()
            {
                continue;
            }
            if self.memory.read_u32(candidate)? == 0
                && self.memory.read_u32(candidate.checked_add(4)?)? == aligned_len
            {
                candidates.push(candidate);
            }
        }
        candidates.sort_unstable();
        candidates.dedup();
        match candidates.as_slice() {
            [] => Ok(None),
            [candidate] => Ok(Some(*candidate)),
            _ => Err(Error::Abi(format!(
                "compact RAM MRP output has ambiguous prepared buffers: {candidates:?}"
            ))),
        }
    }

    pub(super) fn draw_bitmap_region_to_screen(
        &mut self,
        pixels: &[u8],
        x: i32,
        y: i32,
        width: usize,
        height: usize,
        transparent_color: Option<u16>,
    ) -> Result<()> {
        let (screen_width, screen_height) = self.screen_dimensions()?;
        let destination_x0 = i64::from(x).max(0);
        let destination_y0 = i64::from(y).max(0);
        let destination_x1 = (i64::from(x) + width as i64).min(i64::from(screen_width));
        let destination_y1 = (i64::from(y) + height as i64).min(i64::from(screen_height));
        if destination_x0 >= destination_x1 || destination_y0 >= destination_y1 {
            return Ok(());
        }

        let visible_width = usize::try_from(destination_x1 - destination_x0)
            .map_err(|_| Error::Abi("visible bitmap width exceeds usize".into()))?;
        let source_x = usize::try_from(destination_x0 - i64::from(x))
            .map_err(|_| Error::Abi("visible bitmap source x exceeds usize".into()))?;
        let source_y = usize::try_from(destination_y0 - i64::from(y))
            .map_err(|_| Error::Abi("visible bitmap source y exceeds usize".into()))?;
        let row_byte_len = visible_width
            .checked_mul(2)
            .ok_or_else(|| Error::Abi("visible bitmap row byte count overflow".into()))?;

        for visible_row in 0..usize::try_from(destination_y1 - destination_y0)
            .map_err(|_| Error::Abi("visible bitmap height exceeds usize".into()))?
        {
            let source_offset = (source_y + visible_row)
                .checked_mul(width)
                .and_then(|offset| offset.checked_add(source_x))
                .and_then(|offset| offset.checked_mul(2))
                .ok_or_else(|| Error::Abi("visible bitmap source offset overflow".into()))?;
            let source_row = &pixels[source_offset..source_offset + row_byte_len];
            let destination_address = self.screen_address(
                destination_x0 as i32,
                destination_y0 as i32 + visible_row as i32,
                screen_width,
            )?;
            if let Some(transparent_color) = transparent_color {
                let mut destination_row = self.memory.read(destination_address, row_byte_len)?;
                for (source, destination) in source_row
                    .chunks_exact(2)
                    .zip(destination_row.chunks_exact_mut(2))
                {
                    let color = u16::from_le_bytes([source[0], source[1]]);
                    if color != transparent_color {
                        destination.copy_from_slice(source);
                    }
                }
                self.memory.write(destination_address, &destination_row)?;
            } else {
                self.memory.write(destination_address, source_row)?;
            }
        }
        Ok(())
    }

    pub(super) fn read_bitmap_descriptor(&self, address: GuestAddr) -> Result<BitmapDescriptor> {
        Ok(BitmapDescriptor {
            pixels: GuestAddr(self.memory.read_u32(address)?),
            width: usize::from(self.memory.read_u16(address.checked_add(4)?)?),
            height: usize::from(self.memory.read_u16(address.checked_add(6)?)?),
            x: i32::from(self.memory.read_u16(address.checked_add(8)?)? as i16),
            y: i32::from(self.memory.read_u16(address.checked_add(10)?)? as i16),
        })
    }

    pub(super) fn read_bitmap_transform(&self, address: GuestAddr) -> Result<BitmapTransform> {
        let read_field = |offset| {
            self.memory
                .read_u16(address.checked_add(offset)?)
                .map(|value| value as i16)
        };
        Ok(BitmapTransform {
            a: read_field(0)?,
            b: read_field(2)?,
            c: read_field(4)?,
            d: read_field(6)?,
            mode: read_field(8)?,
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn copy_transformed_bitmap(
        &mut self,
        destination: BitmapDescriptor,
        source: BitmapDescriptor,
        width: usize,
        height: usize,
        transform: BitmapTransform,
        transparent_color: u16,
        module: usize,
    ) -> Result<()> {
        let transparent_color = match transform.mode {
            2 => None,
            6 => Some(transparent_color),
            mode => {
                return Err(Error::Abi(format!(
                    "unsupported transformed bitmap mode {mode} called by module {module}"
                )));
            }
        };
        if width == 0 || height == 0 {
            return Ok(());
        }

        let source_x = usize::try_from(source.x).map_err(|_| {
            Error::Abi(format!("negative transformed bitmap source x {}", source.x))
        })?;
        let source_y = usize::try_from(source.y).map_err(|_| {
            Error::Abi(format!("negative transformed bitmap source y {}", source.y))
        })?;
        let source_end_x = source_x
            .checked_add(width)
            .ok_or_else(|| Error::Abi("transformed bitmap source width overflow".into()))?;
        let source_end_y = source_y
            .checked_add(height)
            .ok_or_else(|| Error::Abi("transformed bitmap source height overflow".into()))?;
        if source_end_x > source.width || source_end_y > source.height {
            return Err(Error::Abi(format!(
                "transformed bitmap source region ({source_x}, {source_y}) {width}x{height} exceeds {}x{} bitmap",
                source.width, source.height
            )));
        }
        let pixel_count = width
            .checked_mul(height)
            .ok_or_else(|| Error::Abi("transformed bitmap region dimensions overflow".into()))?;
        if pixel_count > self.heap_len / 2 {
            return Err(Error::Abi(format!(
                "transformed bitmap region requires {pixel_count} pixels"
            )));
        }

        // Source and destination can refer to the same bitmap. Capture the
        // complete source region before changing any destination pixel.
        let mut pixels = Vec::with_capacity(pixel_count);
        for row in 0..height {
            for column in 0..width {
                let address = bitmap_pixel_address(
                    source.pixels,
                    source.width,
                    source_x + column,
                    source_y + row,
                )?;
                pixels.push(self.memory.read_u16(address)?);
            }
        }

        let last_x = i64::try_from(width - 1)
            .map_err(|_| Error::Abi("transformed bitmap width exceeds i64".into()))?;
        let last_y = i64::try_from(height - 1)
            .map_err(|_| Error::Abi("transformed bitmap height exceeds i64".into()))?;
        let corners = [
            transform.apply(0, 0),
            transform.apply(last_x, 0),
            transform.apply(0, last_y),
            transform.apply(last_x, last_y),
        ];
        let minimum_x = corners
            .iter()
            .map(|(x, _)| *x)
            .min()
            .expect("four transform corners");
        let minimum_y = corners
            .iter()
            .map(|(_, y)| *y)
            .min()
            .expect("four transform corners");

        for row in 0..height {
            for column in 0..width {
                let color = pixels[row * width + column];
                if Some(color) == transparent_color {
                    continue;
                }
                let (transformed_x, transformed_y) = transform.apply(column as i64, row as i64);
                let destination_x = i64::from(destination.x) + transformed_x - minimum_x;
                let destination_y = i64::from(destination.y) + transformed_y - minimum_y;
                if destination_x < 0
                    || destination_y < 0
                    || destination_x >= destination.width as i64
                    || destination_y >= destination.height as i64
                {
                    continue;
                }
                let address = bitmap_pixel_address(
                    destination.pixels,
                    destination.width,
                    destination_x as usize,
                    destination_y as usize,
                )?;
                self.memory.write_u16(address, color)?;
            }
        }
        Ok(())
    }

    pub(super) fn draw_rectangle_to_screen(
        &mut self,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        color: u16,
    ) -> Result<()> {
        if width <= 0 || height <= 0 {
            return Ok(());
        }
        let (screen_width, screen_height) = self.screen_dimensions()?;
        let x0 = x.clamp(0, screen_width);
        let y0 = y.clamp(0, screen_height);
        let x1 = x.saturating_add(width).clamp(0, screen_width);
        let y1 = y.saturating_add(height).clamp(0, screen_height);
        if x0 >= x1 || y0 >= y1 {
            return Ok(());
        }
        let color = color.to_le_bytes();
        let mut row = Vec::with_capacity((x1 - x0) as usize * 2);
        for _ in x0..x1 {
            row.extend_from_slice(&color);
        }
        for screen_y in y0..y1 {
            let address = self.screen_address(x0, screen_y, screen_width)?;
            self.memory.write(address, &row)?;
        }
        Ok(())
    }

    pub(super) fn draw_text_to_screen(
        &mut self,
        text: &[u16],
        mut x: i32,
        y: i32,
        color: u16,
        font: u32,
        services: &mut dyn NativeServices,
    ) -> Result<()> {
        let (screen_width, screen_height) = self.screen_dimensions()?;
        for &codepoint in text {
            let Some((glyph, width, height)) = services.char_bitmap(u32::from(codepoint), font)?
            else {
                x += if codepoint < 128 { 8 } else { 16 };
                continue;
            };
            let width = width.min(16) as i32;
            let height = height.min(16) as usize;
            let required = height
                .checked_mul(2)
                .ok_or_else(|| Error::Abi("character bitmap size overflow".into()))?;
            if glyph.len() < required {
                return Err(Error::Abi(format!(
                    "character bitmap for {codepoint:#06x} has {} bytes, needs {required}",
                    glyph.len()
                )));
            }
            for row in 0..height as i32 {
                let offset = row as usize * 2;
                let bits = u16::from_be_bytes([glyph[offset], glyph[offset + 1]]);
                for column in 0..width {
                    if bits & (0x8000_u16 >> column) != 0 {
                        self.write_screen_pixel(
                            x + column,
                            y + row,
                            color,
                            screen_width,
                            screen_height,
                        )?;
                    }
                }
            }
            x += width;
        }
        Ok(())
    }

    pub(super) fn write_screen_pixel(
        &mut self,
        x: i32,
        y: i32,
        color: u16,
        width: i32,
        height: i32,
    ) -> Result<()> {
        if x < 0 || y < 0 || x >= width || y >= height {
            return Ok(());
        }
        let address = self.screen_address(x, y, width)?;
        self.memory.write_u16(address, color)
    }

    pub(super) fn screen_dimensions(&self) -> Result<(i32, i32)> {
        let width = self.memory.read_u32(data_slot_address(92))?;
        let height = self.memory.read_u32(data_slot_address(93))?;
        Ok((
            i32::try_from(width)
                .map_err(|_| Error::Abi(format!("screen width {width} exceeds i32")))?,
            i32::try_from(height)
                .map_err(|_| Error::Abi(format!("screen height {height} exceeds i32")))?,
        ))
    }

    pub(super) fn screen_address(&self, x: i32, y: i32, width: i32) -> Result<GuestAddr> {
        let offset = y
            .checked_mul(width)
            .and_then(|offset| offset.checked_add(x))
            .and_then(|offset| offset.checked_mul(2))
            .and_then(|offset| u32::try_from(offset).ok())
            .ok_or_else(|| Error::Abi("screen pixel offset overflow".into()))?;
        SCREEN_BASE.checked_add(offset)
    }
}

impl BitmapTransform {
    fn apply(self, x: i64, y: i64) -> (i64, i64) {
        (
            (i64::from(self.a) * x + i64::from(self.b) * y) >> 8,
            (i64::from(self.c) * x + i64::from(self.d) * y) >> 8,
        )
    }
}

fn bitmap_pixel_address(pixels: GuestAddr, stride: usize, x: usize, y: usize) -> Result<GuestAddr> {
    let byte_offset = y
        .checked_mul(stride)
        .and_then(|offset| offset.checked_add(x))
        .and_then(|offset| offset.checked_mul(2))
        .and_then(|offset| u32::try_from(offset).ok())
        .ok_or_else(|| Error::Abi("bitmap pixel offset overflow".into()))?;
    pixels.checked_add(byte_offset)
}
