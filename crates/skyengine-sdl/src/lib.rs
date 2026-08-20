use std::time::Duration;

use sdl2::{
    event::Event, keyboard::Keycode, pixels::PixelFormatEnum, render::Canvas, video::Window,
};
use skyengine_core::{DisplayEvent, Error, Framebuffer, PlatformDisplay, Result};

pub struct SdlDisplay {
    _sdl: sdl2::Sdl,
    canvas: Canvas<Window>,
    events: sdl2::EventPump,
}

impl SdlDisplay {
    pub fn new(width: u16, height: u16, scale: u32) -> Result<Self> {
        let sdl = sdl2::init().map_err(Error::Platform)?;
        let video = sdl.video().map_err(Error::Platform)?;
        let window = video
            .window(
                "SkyEngine",
                u32::from(width).saturating_mul(scale),
                u32::from(height).saturating_mul(scale),
            )
            .position_centered()
            .resizable()
            .build()
            .map_err(|error| Error::Platform(error.to_string()))?;
        let mut canvas = window
            .into_canvas()
            .accelerated()
            .present_vsync()
            .build()
            .or_else(|_| {
                video
                    .window(
                        "SkyEngine",
                        u32::from(width).saturating_mul(scale),
                        u32::from(height).saturating_mul(scale),
                    )
                    .position_centered()
                    .build()
                    .map_err(|error| error.to_string())?
                    .into_canvas()
                    .software()
                    .build()
                    .map_err(|error| error.to_string())
            })
            .map_err(|error| Error::Platform(error.to_string()))?;
        canvas
            .set_logical_size(u32::from(width), u32::from(height))
            .map_err(|error| Error::Platform(error.to_string()))?;
        let events = sdl.event_pump().map_err(Error::Platform)?;
        Ok(Self {
            _sdl: sdl,
            canvas,
            events,
        })
    }
}

impl PlatformDisplay for SdlDisplay {
    fn present(&mut self, framebuffer: &Framebuffer) -> Result<()> {
        let creator = self.canvas.texture_creator();
        let mut texture = creator
            .create_texture_streaming(
                PixelFormatEnum::RGB565,
                u32::from(framebuffer.width()),
                u32::from(framebuffer.height()),
            )
            .map_err(|error| Error::Platform(error.to_string()))?;
        texture
            .with_lock(None, |output, pitch| {
                for (row, source) in framebuffer
                    .pixels()
                    .chunks_exact(usize::from(framebuffer.width()))
                    .enumerate()
                {
                    let destination = &mut output[row * pitch..row * pitch + source.len() * 2];
                    for (pixel, output) in source.iter().zip(destination.chunks_exact_mut(2)) {
                        output.copy_from_slice(&pixel.to_ne_bytes());
                    }
                }
            })
            .map_err(Error::Platform)?;
        self.canvas.clear();
        self.canvas
            .copy(&texture, None, None)
            .map_err(Error::Platform)?;
        self.canvas.present();
        Ok(())
    }

    fn poll_event(&mut self) -> Result<Option<DisplayEvent>> {
        while let Some(event) = self.events.poll_event() {
            let mapped = match event {
                Event::Quit { .. } => Some(DisplayEvent::Quit),
                Event::KeyDown {
                    keycode: Some(key),
                    repeat: false,
                    ..
                } => key_code(key).map(|code| DisplayEvent::Key {
                    code,
                    pressed: true,
                }),
                Event::KeyUp {
                    keycode: Some(key),
                    repeat: false,
                    ..
                } => key_code(key).map(|code| DisplayEvent::Key {
                    code,
                    pressed: false,
                }),
                Event::MouseButtonDown { x, y, .. } => Some(DisplayEvent::Pointer {
                    x,
                    y,
                    pressed: true,
                }),
                Event::MouseButtonUp { x, y, .. } => Some(DisplayEvent::Pointer {
                    x,
                    y,
                    pressed: false,
                }),
                _ => None,
            };
            if mapped.is_some() {
                return Ok(mapped);
            }
        }
        Ok(None)
    }

    fn wait_timeout(&mut self, milliseconds: u32) {
        std::thread::sleep(Duration::from_millis(u64::from(milliseconds)));
    }
}

fn key_code(key: Keycode) -> Option<i32> {
    Some(match key {
        Keycode::Num0 | Keycode::Kp0 => 0,
        Keycode::Num1 | Keycode::Kp1 => 1,
        Keycode::Num2 | Keycode::Kp2 => 2,
        Keycode::Num3 | Keycode::Kp3 => 3,
        Keycode::Num4 | Keycode::Kp4 => 4,
        Keycode::Num5 | Keycode::Kp5 => 5,
        Keycode::Num6 | Keycode::Kp6 => 6,
        Keycode::Num7 | Keycode::Kp7 => 7,
        Keycode::Num8 | Keycode::Kp8 => 8,
        Keycode::Num9 | Keycode::Kp9 => 9,
        Keycode::Asterisk => 10,
        Keycode::Hash => 11,
        Keycode::Up => 12,
        Keycode::Down => 13,
        Keycode::Left => 14,
        Keycode::Right => 15,
        Keycode::Escape => 16,
        Keycode::F1 => 17,
        Keycode::F2 | Keycode::Backspace => 18,
        Keycode::Return | Keycode::KpEnter | Keycode::Space => 20,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dummy_driver_presents_an_rgb565_frame() {
        if std::env::var("SDL_VIDEODRIVER").as_deref() != Ok("dummy") {
            return;
        }

        let mut display = SdlDisplay::new(4, 3, 1).unwrap();
        let mut framebuffer = Framebuffer::new(4, 3).unwrap();
        framebuffer.rect(0, 0, 4, 3, Framebuffer::rgb565(24, 160, 200));
        display.present(&framebuffer).unwrap();
    }
}
