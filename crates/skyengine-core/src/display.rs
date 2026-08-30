use crate::{Error, Result};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DisplayEvent {
    Quit,
    Key { code: i32, pressed: bool },
    Pointer { x: i32, y: i32, pressed: bool },
    PointerMove { x: i32, y: i32 },
    Motion { x: i32, y: i32, z: i32 },
    TextInput { text: String },
}

pub trait PlatformDisplay {
    fn resize(&mut self, _width: u16, _height: u16) -> Result<()> {
        Ok(())
    }

    fn start_shake(&mut self, _milliseconds: u32) -> Result<()> {
        Ok(())
    }

    fn stop_shake(&mut self) -> Result<()> {
        Ok(())
    }

    fn present(&mut self, framebuffer: &Framebuffer) -> Result<()>;
    fn poll_event(&mut self) -> Result<Option<DisplayEvent>>;
    fn wait_timeout(&mut self, milliseconds: u32);
}

#[derive(Clone, Debug)]
pub struct Framebuffer {
    width: u16,
    height: u16,
    pixels: Vec<u16>,
    draw_count: u64,
}

impl Framebuffer {
    pub fn new(width: u16, height: u16) -> Result<Self> {
        if width == 0 || height == 0 {
            return Err(Error::Config("screen dimensions must be non-zero".into()));
        }
        let len = usize::from(width)
            .checked_mul(usize::from(height))
            .ok_or_else(|| Error::Config("screen dimensions overflow".into()))?;
        Ok(Self {
            width,
            height,
            pixels: vec![0; len],
            draw_count: 0,
        })
    }

    pub fn width(&self) -> u16 {
        self.width
    }

    pub fn height(&self) -> u16 {
        self.height
    }

    pub fn resize(&mut self, width: u16, height: u16) -> Result<()> {
        if width == 0 || height == 0 {
            return Err(Error::Config("screen dimensions must be non-zero".into()));
        }
        let len = usize::from(width)
            .checked_mul(usize::from(height))
            .ok_or_else(|| Error::Config("screen dimensions overflow".into()))?;
        self.width = width;
        self.height = height;
        self.pixels = vec![0; len];
        Ok(())
    }

    pub fn pixels(&self) -> &[u16] {
        &self.pixels
    }

    pub fn draw_count(&self) -> u64 {
        self.draw_count
    }

    pub fn mark_presented(&mut self) {
        self.draw_count = self.draw_count.saturating_add(1);
    }

    pub fn clear(&mut self, color: u16) {
        self.pixels.fill(color);
    }

    pub fn point(&mut self, x: i32, y: i32, color: u16) {
        if x < 0 || y < 0 || x >= i32::from(self.width) || y >= i32::from(self.height) {
            return;
        }
        self.pixels[y as usize * usize::from(self.width) + x as usize] = color;
    }

    pub fn rect(&mut self, x: i32, y: i32, width: i32, height: i32, color: u16) {
        if width <= 0 || height <= 0 {
            return;
        }
        let x0 = x.clamp(0, i32::from(self.width));
        let y0 = y.clamp(0, i32::from(self.height));
        let x1 = x.saturating_add(width).clamp(0, i32::from(self.width));
        let y1 = y.saturating_add(height).clamp(0, i32::from(self.height));
        for row in y0..y1 {
            let start = row as usize * usize::from(self.width) + x0 as usize;
            self.pixels[start..start + (x1 - x0) as usize].fill(color);
        }
    }

    pub fn line(&mut self, mut x0: i32, mut y0: i32, x1: i32, y1: i32, color: u16) {
        let dx = (x1 - x0).abs();
        let sx = if x0 < x1 { 1 } else { -1 };
        let dy = -(y1 - y0).abs();
        let sy = if y0 < y1 { 1 } else { -1 };
        let mut error = dx + dy;
        loop {
            self.point(x0, y0, color);
            if x0 == x1 && y0 == y1 {
                break;
            }
            let twice = error.saturating_mul(2);
            if twice >= dy {
                error += dy;
                x0 += sx;
            }
            if twice <= dx {
                error += dx;
                y0 += sy;
            }
        }
    }

    pub fn rgb565(red: i32, green: i32, blue: i32) -> u16 {
        let red = red.clamp(0, 255) as u16;
        let green = green.clamp(0, 255) as u16;
        let blue = blue.clamp(0, 255) as u16;
        ((red >> 3) << 11) | ((green >> 2) << 5) | (blue >> 3)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resizing_clears_pixels_without_resetting_draw_count() {
        let mut framebuffer = Framebuffer::new(2, 3).unwrap();
        framebuffer.clear(0xffff);
        framebuffer.mark_presented();

        framebuffer.resize(3, 2).unwrap();

        assert_eq!((framebuffer.width(), framebuffer.height()), (3, 2));
        assert_eq!(framebuffer.pixels(), &[0; 6]);
        assert_eq!(framebuffer.draw_count(), 1);
    }
}
