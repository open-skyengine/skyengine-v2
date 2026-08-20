use std::{
    collections::BTreeMap,
    fs,
    path::{Component, Path, PathBuf},
    sync::Arc,
};

use encoding_rs::GBK;

use crate::{Framebuffer, Package, PlatformDisplay, Result};

use super::value::{Table, Value};

#[derive(Clone, Debug)]
struct Bitmap {
    width: usize,
    height: usize,
    pixels: Vec<u16>,
    frame_height: Option<usize>,
    transparent_color: u16,
}

#[derive(Clone, Copy, Debug)]
struct BlitRegion {
    source_x: usize,
    source_y: usize,
    width: usize,
    height: usize,
    destination_x: i32,
    destination_y: i32,
    transparent_color: Option<u16>,
}

struct DirectorySearch {
    entries: Vec<Arc<[u8]>>,
    next: usize,
}

pub(crate) struct MrHost {
    pub package: Arc<Package>,
    pub framebuffer: Framebuffer,
    pub display: Box<dyn PlatformDisplay>,
    pub work_dir: PathBuf,
    font: Arc<[u8]>,
    bitmaps: BTreeMap<i32, Bitmap>,
    directory_searches: BTreeMap<i32, DirectorySearch>,
    next_directory_handle: i32,
}

impl MrHost {
    pub fn new(
        package: Arc<Package>,
        framebuffer: Framebuffer,
        display: Box<dyn PlatformDisplay>,
        work_dir: PathBuf,
        font: Arc<[u8]>,
    ) -> Self {
        Self {
            package,
            framebuffer,
            display,
            work_dir,
            font,
            bitmaps: BTreeMap::new(),
            directory_searches: BTreeMap::new(),
            next_directory_handle: 1,
        }
    }

    pub fn call(&mut self, name: &str, args: &[Value]) -> Result<Vec<Value>> {
        match name {
            "sys_get_info" => {
                let info = Table::new();
                {
                    let mut values = info.borrow_mut();
                    values.set(
                        bytes(b"scrw"),
                        Value::Number(f64::from(self.framebuffer.width())),
                    );
                    values.set(
                        bytes(b"scrh"),
                        Value::Number(f64::from(self.framebuffer.height())),
                    );
                    values.set(bytes(b"IMEI"), bytes(b"000000000000000"));
                    values.set(bytes(b"IMSI"), bytes(b"460000000000000"));
                }
                Ok(vec![Value::Table(info)])
            }
            "sys_find_start" => self.find_start(args),
            "sys_find_next" => self.find_next(args),
            "sys_find_stop" => self.find_stop(args),
            "GetSysInfo" => {
                let table = Table::new();
                table.borrow_mut().set(
                    bytes(b"ScreenW"),
                    Value::Number(f64::from(self.framebuffer.width())),
                );
                table.borrow_mut().set(
                    bytes(b"ScreenH"),
                    Value::Number(f64::from(self.framebuffer.height())),
                );
                Ok(vec![Value::Table(table)])
            }
            "_platEx" => {
                let command = integer(args.first())?;
                match command {
                    1201 => Ok(vec![bytes(&[16, 16, 8, 16]), Value::Number(0.0)]),
                    _ => Err(crate::Error::Platform(format!(
                        "unsupported _platEx command {command}"
                    ))),
                }
            }
            "BitmapLoad" => {
                self.bitmap_load(args)?;
                Ok(Vec::new())
            }
            "BitmapShow" => {
                self.bitmap_show(args)?;
                Ok(Vec::new())
            }
            "SpriteSet" => {
                let id = integer(args.first())?;
                let frame_height = positive_usize(args.get(1), "sprite frame height")?;
                let bitmap = self.bitmaps.get_mut(&id).ok_or_else(|| {
                    crate::Error::Platform(format!("SpriteSet references missing bitmap {id}"))
                })?;
                bitmap.frame_height = Some(frame_height);
                Ok(Vec::new())
            }
            "SpriteDraw" => {
                self.sprite_draw(args)?;
                Ok(Vec::new())
            }
            "_drawRect" | "DrawRect" => {
                let x = integer(args.first())?;
                let y = integer(args.get(1))?;
                let width = integer(args.get(2))?;
                let height = integer(args.get(3))?;
                let color = color(args, 4)?;
                self.framebuffer.rect(x, y, width, height, color);
                Ok(Vec::new())
            }
            "_drawLine" | "DrawLine" => {
                let x0 = integer(args.first())?;
                let y0 = integer(args.get(1))?;
                let x1 = integer(args.get(2))?;
                let y1 = integer(args.get(3))?;
                let color = color(args, 4)?;
                self.framebuffer.line(x0, y0, x1, y1, color);
                Ok(Vec::new())
            }
            "DrawText" => {
                let text = value_bytes(args.first())?;
                let x = integer(args.get(1))?;
                let y = integer(args.get(2))?;
                let color = color(args, 3)?;
                self.draw_text(&text, x, y, color);
                Ok(Vec::new())
            }
            "_textWidth" => self.text_width(args),
            "DispUpEx" => {
                self.framebuffer.mark_presented();
                self.display.present(&self.framebuffer)?;
                Ok(Vec::new())
            }
            "TestCom" => Ok(vec![Value::Number(0.0)]),
            "_com" => self.com(args),
            "_strCom" => self.string_command(args),
            "LoadTable" => Ok(vec![Value::Nil]),
            "SaveTable" => Ok(vec![Value::Number(0.0)]),
            "LoadPack" => Ok(vec![Value::Nil]),
            "UAReset" => Ok(Vec::new()),
            "TimerStart" | "TimerStop" => Ok(vec![Value::Number(0.0)]),
            "_gc" => Ok(Vec::new()),
            "Exit" => Ok(Vec::new()),
            _ => Err(crate::Error::Platform(format!(
                "unsupported MR platform function {name}"
            ))),
        }
    }

    fn bitmap_load(&mut self, args: &[Value]) -> Result<()> {
        let id = integer(args.first())?;
        let name = value_bytes(args.get(1))?;
        let width = positive_usize(args.get(4), "bitmap width")?;
        let height = positive_usize(args.get(5), "bitmap height")?;
        let raw = self.package.read_named(&name)?;
        let pixel_count = width
            .checked_mul(height)
            .ok_or_else(|| crate::Error::Platform(format!("bitmap {id} dimensions overflow")))?;
        let byte_count = pixel_count
            .checked_mul(2)
            .ok_or_else(|| crate::Error::Platform(format!("bitmap {id} byte count overflow")))?;
        if raw.len() < byte_count {
            return Err(crate::Error::Platform(format!(
                "bitmap {} contains {} bytes, needs {byte_count}",
                String::from_utf8_lossy(&name),
                raw.len()
            )));
        }
        let pixels: Vec<u16> = raw[..byte_count]
            .chunks_exact(2)
            .map(|pixel| u16::from_le_bytes([pixel[0], pixel[1]]))
            .collect();
        let transparent_color = pixels.first().copied().unwrap_or(0);
        self.bitmaps.insert(
            id,
            Bitmap {
                width,
                height,
                pixels,
                frame_height: None,
                transparent_color,
            },
        );
        Ok(())
    }

    fn bitmap_show(&mut self, args: &[Value]) -> Result<()> {
        let id = integer(args.first())?;
        let x = integer(args.get(1))?;
        let y = integer(args.get(2))?;
        let bitmap = self.bitmaps.get(&id).ok_or_else(|| {
            crate::Error::Platform(format!("BitmapShow references missing bitmap {id}"))
        })?;
        blit(
            &mut self.framebuffer,
            bitmap,
            BlitRegion {
                source_x: 0,
                source_y: 0,
                width: bitmap.width,
                height: bitmap.height,
                destination_x: x,
                destination_y: y,
                transparent_color: None,
            },
        );
        Ok(())
    }

    fn sprite_draw(&mut self, args: &[Value]) -> Result<()> {
        let id = integer(args.first())?;
        let frame = integer(args.get(1))?.max(0) as usize;
        let x = integer(args.get(2))?;
        let y = integer(args.get(3))?;
        let bitmap = self.bitmaps.get(&id).ok_or_else(|| {
            crate::Error::Platform(format!("SpriteDraw references missing bitmap {id}"))
        })?;
        let frame_height = bitmap.frame_height.unwrap_or(bitmap.height);
        let source_y = frame.saturating_mul(frame_height);
        if source_y >= bitmap.height {
            return Ok(());
        }
        blit(
            &mut self.framebuffer,
            bitmap,
            BlitRegion {
                source_x: 0,
                source_y,
                width: bitmap.width,
                height: frame_height.min(bitmap.height - source_y),
                destination_x: x,
                destination_y: y,
                transparent_color: Some(bitmap.transparent_color),
            },
        );
        Ok(())
    }

    fn draw_text(&mut self, encoded: &[u8], mut x: i32, y: i32, color: u16) {
        let (decoded, _, _) = GBK.decode(encoded);
        for character in decoded.chars() {
            let codepoint = character as usize;
            let width = if character.is_ascii() { 8 } else { 16 };
            let Some(start) = codepoint.checked_mul(32) else {
                continue;
            };
            let Some(glyph) = self.font.get(start..start + 32) else {
                x += width;
                continue;
            };
            for row in 0..16_i32 {
                let bits =
                    u16::from_be_bytes([glyph[row as usize * 2], glyph[row as usize * 2 + 1]]);
                for column in 0..width {
                    if bits & (0x8000_u16 >> column) != 0 {
                        self.framebuffer.point(x + column, y + row, color);
                    }
                }
            }
            x += width;
        }
    }

    fn text_width(&self, args: &[Value]) -> Result<Vec<Value>> {
        let width = match args.first() {
            Some(Value::Bytes(encoded)) => {
                let unicode = args.get(1).is_some_and(Value::truthy);
                if unicode {
                    encoded
                        .chunks_exact(2)
                        .take_while(|bytes| **bytes != [0, 0])
                        .map(|bytes| {
                            if u16::from_be_bytes([bytes[0], bytes[1]]) < 128 {
                                8
                            } else {
                                16
                            }
                        })
                        .sum::<i32>()
                } else {
                    let (decoded, _, _) = GBK.decode(encoded);
                    decoded
                        .chars()
                        .map(|character| if character.is_ascii() { 8 } else { 16 })
                        .sum::<i32>()
                }
            }
            Some(Value::Number(codepoint)) => {
                if *codepoint >= 0.0 && *codepoint < 128.0 {
                    8
                } else {
                    16
                }
            }
            other => {
                return Err(crate::Error::MrFault(format!(
                    "_textWidth expects string or character code, got {other:?}"
                )));
            }
        };
        Ok(vec![Value::Number(f64::from(width)), Value::Number(16.0)])
    }

    fn com(&mut self, args: &[Value]) -> Result<Vec<Value>> {
        let command = integer(args.first())?;
        match command {
            // UI reset and screen mode notifications used by the baseline SDK.
            0 | 1 | 403 => Ok(vec![Value::Number(0.0)]),
            other => Err(crate::Error::Platform(format!(
                "unsupported _com command {other}"
            ))),
        }
    }

    fn string_command(&mut self, args: &[Value]) -> Result<Vec<Value>> {
        let command = integer(args.first())?;
        match command {
            // Read a checked byte range from the currently loaded MRP file.
            600 => {
                let requested = value_bytes(args.get(1))?;
                if matches!(requested.as_ref(), [b'*', b'A'..=b'Z']) {
                    // M0 firmware slots are empty until a host registers one.
                    return Ok(Vec::new());
                }
                let requested = requested.strip_prefix(b"%").unwrap_or(&requested);
                let current = self
                    .package
                    .path()
                    .file_name()
                    .and_then(|name| name.to_str())
                    .ok_or_else(|| {
                        crate::Error::Platform("current package name is not valid Unicode".into())
                    })?;
                if requested != current.as_bytes() {
                    return Err(crate::Error::Platform(format!(
                        "_strCom 600 cannot read package {} while {} is loaded",
                        String::from_utf8_lossy(requested),
                        self.package.path().display()
                    )));
                }
                let offset = nonnegative_usize(args.get(2), "package offset")?;
                let len = nonnegative_usize(args.get(3), "package range length")?;
                Ok(vec![bytes(&self.package.read_raw_range(offset, len)?)])
            }
            // 601 reads a package resource into a VM byte string.
            601 => {
                let name = value_bytes(args.get(1))?;
                Ok(vec![bytes(&self.package.read_named(&name)?)])
            }
            // 800 initializes the compact key helper. Rendering does not depend on it.
            800 => Ok(vec![Value::Number(0.0), Value::Number(0.0)]),
            3 => Ok(vec![Value::Number(0.0)]),
            other => Err(crate::Error::Platform(format!(
                "unsupported _strCom command {other}"
            ))),
        }
    }

    fn find_start(&mut self, args: &[Value]) -> Result<Vec<Value>> {
        let directory = value_bytes(args.first())?;
        let Some(path) = safe_work_path(&self.work_dir, &directory) else {
            return Ok(vec![Value::Number(-1.0), bytes(b"")]);
        };
        let Ok(entries) = fs::read_dir(path) else {
            return Ok(vec![Value::Number(-1.0), bytes(b"")]);
        };
        let mut names = entries
            .filter_map(|entry| entry.ok())
            .map(|entry| {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                let (encoded, _, _) = GBK.encode(&name);
                Arc::<[u8]>::from(encoded.as_ref())
            })
            .collect::<Vec<_>>();
        names.sort();

        let handle = self.allocate_directory_handle()?;
        let first = names
            .first()
            .cloned()
            .unwrap_or_else(|| Arc::from(&b""[..]));
        self.directory_searches.insert(
            handle,
            DirectorySearch {
                entries: names,
                next: 1,
            },
        );
        Ok(vec![Value::Number(f64::from(handle)), Value::Bytes(first)])
    }

    fn find_next(&mut self, args: &[Value]) -> Result<Vec<Value>> {
        let handle = integer(args.first())?;
        let Some(search) = self.directory_searches.get_mut(&handle) else {
            return Ok(vec![Value::Nil]);
        };
        let Some(name) = search.entries.get(search.next).cloned() else {
            return Ok(vec![Value::Nil]);
        };
        search.next += 1;
        Ok(vec![Value::Bytes(name)])
    }

    fn find_stop(&mut self, args: &[Value]) -> Result<Vec<Value>> {
        let handle = integer(args.first())?;
        Ok(vec![Value::Number(
            if self.directory_searches.remove(&handle).is_some() {
                0.0
            } else {
                -1.0
            },
        )])
    }

    fn allocate_directory_handle(&mut self) -> Result<i32> {
        let start = self.next_directory_handle;
        loop {
            let handle = self.next_directory_handle;
            self.next_directory_handle = self.next_directory_handle.checked_add(1).unwrap_or(1);
            if !self.directory_searches.contains_key(&handle) {
                return Ok(handle);
            }
            if self.next_directory_handle == start {
                return Err(crate::Error::ResourceLimit(
                    "no directory search handles available".into(),
                ));
            }
        }
    }
}

fn safe_work_path(work_dir: &Path, bytes: &[u8]) -> Option<PathBuf> {
    let path = std::str::from_utf8(bytes).ok()?;
    let path = Path::new(path);
    if path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, Component::ParentDir))
    {
        return None;
    }
    Some(work_dir.join(path))
}

fn blit(framebuffer: &mut Framebuffer, bitmap: &Bitmap, region: BlitRegion) {
    for row in 0..region.height {
        for column in 0..region.width {
            let pixel =
                bitmap.pixels[(region.source_y + row) * bitmap.width + region.source_x + column];
            if Some(pixel) != region.transparent_color {
                framebuffer.point(
                    region.destination_x + column as i32,
                    region.destination_y + row as i32,
                    pixel,
                );
            }
        }
    }
}

fn color(args: &[Value], offset: usize) -> Result<u16> {
    Ok(Framebuffer::rgb565(
        integer(args.get(offset))?,
        integer(args.get(offset + 1))?,
        integer(args.get(offset + 2))?,
    ))
}

fn integer(value: Option<&Value>) -> Result<i32> {
    let value = value.ok_or_else(|| crate::Error::MrFault("missing numeric argument".into()))?;
    let number = value
        .number()
        .ok_or_else(|| crate::Error::MrFault(format!("expected number, got {value:?}")))?;
    if !number.is_finite() || number < i32::MIN as f64 || number > i32::MAX as f64 {
        return Err(crate::Error::MrFault(format!(
            "number {number} does not fit i32"
        )));
    }
    Ok(number as i32)
}

fn positive_usize(value: Option<&Value>, label: &str) -> Result<usize> {
    let value = integer(value)?;
    usize::try_from(value)
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| crate::Error::MrFault(format!("invalid {label}: {value}")))
}

fn nonnegative_usize(value: Option<&Value>, label: &str) -> Result<usize> {
    let value = integer(value)?;
    usize::try_from(value).map_err(|_| crate::Error::MrFault(format!("invalid {label}: {value}")))
}

fn value_bytes(value: Option<&Value>) -> Result<Arc<[u8]>> {
    let value = value.ok_or_else(|| crate::Error::MrFault("missing string argument".into()))?;
    value
        .bytes()
        .ok_or_else(|| crate::Error::MrFault(format!("expected string, got {value:?}")))
}

fn bytes(value: &[u8]) -> Value {
    Value::Bytes(Arc::from(value))
}
