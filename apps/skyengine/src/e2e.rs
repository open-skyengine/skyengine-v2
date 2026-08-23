use std::{
    collections::VecDeque,
    fs::File,
    io::{BufRead, BufReader, BufWriter, Write},
    os::unix::net::{UnixListener, UnixStream},
    path::{Path, PathBuf},
    sync::{
        Arc, Condvar, Mutex,
        mpsc::{self, Receiver, Sender, TryRecvError},
    },
    thread,
    time::{Duration, Instant},
};

use skyengine_core::{DisplayEvent, Error, Framebuffer, PlatformDisplay, Result};

const FRAME_HISTORY_LIMIT: usize = 128;
const DEFAULT_KEY_HOLD_MS: u64 = 250;
const DEFAULT_POINTER_HOLD_MS: u64 = 80;
const MAX_CLIPBOARD_BYTES: usize = 64 * 1024;

#[derive(Clone)]
struct CapturedFrame {
    width: u16,
    height: u16,
    pixels: Vec<u16>,
}

#[derive(Default)]
struct CaptureData {
    draw_count: u64,
    latest: Option<CapturedFrame>,
    history: VecDeque<(u64, CapturedFrame)>,
    clipboard: String,
    exited: bool,
}

#[derive(Default)]
struct CaptureState {
    data: Mutex<CaptureData>,
    changed: Condvar,
}

enum ControlMessage {
    Event(DisplayEvent),
    Key {
        code: i32,
        hold: Duration,
        accepted: Sender<u64>,
    },
    Pointer {
        x: i32,
        y: i32,
        hold: Duration,
        accepted: Sender<u64>,
    },
}

pub(crate) struct E2eDisplay {
    state: Arc<CaptureState>,
    commands: Receiver<ControlMessage>,
    queued_events: VecDeque<DisplayEvent>,
    key_releases: Vec<(Instant, i32)>,
    pointer_releases: Vec<(Instant, i32, i32)>,
    yield_after_release: bool,
}

impl E2eDisplay {
    pub(crate) fn new(socket_path: PathBuf, width: u16, height: u16) -> Result<Self> {
        let listener = UnixListener::bind(&socket_path).map_err(|source| Error::Io {
            path: socket_path,
            source,
        })?;
        let initial = CapturedFrame {
            width,
            height,
            pixels: vec![0; usize::from(width) * usize::from(height)],
        };
        let mut data = CaptureData {
            latest: Some(initial.clone()),
            ..CaptureData::default()
        };
        data.history.push_back((0, initial));
        let state = Arc::new(CaptureState {
            data: Mutex::new(data),
            changed: Condvar::new(),
        });
        let (sender, commands) = mpsc::channel();
        let server_state = Arc::clone(&state);
        thread::Builder::new()
            .name("skyengine-e2e".into())
            .spawn(move || serve(listener, server_state, sender))
            .map_err(|error| Error::Platform(format!("failed to start E2E server: {error}")))?;
        Ok(Self {
            state,
            commands,
            queued_events: VecDeque::new(),
            key_releases: Vec::new(),
            pointer_releases: Vec::new(),
            yield_after_release: false,
        })
    }

    fn queue_due_releases(&mut self) {
        let now = Instant::now();
        let mut index = 0;
        while index < self.key_releases.len() {
            if self.key_releases[index].0 <= now {
                let (_, code) = self.key_releases.swap_remove(index);
                self.queued_events.push_back(DisplayEvent::Key {
                    code,
                    pressed: false,
                });
            } else {
                index += 1;
            }
        }
        let mut index = 0;
        while index < self.pointer_releases.len() {
            if self.pointer_releases[index].0 <= now {
                let (_, x, y) = self.pointer_releases.swap_remove(index);
                self.queued_events.push_back(DisplayEvent::Pointer {
                    x,
                    y,
                    pressed: false,
                });
            } else {
                index += 1;
            }
        }
    }
}

impl Drop for E2eDisplay {
    fn drop(&mut self) {
        if let Ok(mut data) = self.state.data.lock() {
            data.exited = true;
            self.state.changed.notify_all();
        }
    }
}

impl PlatformDisplay for E2eDisplay {
    fn present(&mut self, framebuffer: &Framebuffer) -> Result<()> {
        let frame = CapturedFrame {
            width: framebuffer.width(),
            height: framebuffer.height(),
            pixels: framebuffer.pixels().to_vec(),
        };
        let mut data = self
            .state
            .data
            .lock()
            .map_err(|_| Error::Platform("E2E capture state is poisoned".into()))?;
        let draw_count = framebuffer.draw_count();
        data.draw_count = draw_count;
        data.latest = Some(frame.clone());
        data.history.push_back((draw_count, frame));
        while data.history.len() > FRAME_HISTORY_LIMIT {
            data.history.pop_front();
        }
        self.state.changed.notify_all();
        Ok(())
    }

    fn poll_event(&mut self) -> Result<Option<DisplayEvent>> {
        self.queue_due_releases();
        if let Some(event) = self.queued_events.pop_front() {
            self.yield_after_release = true;
            return Ok(Some(event));
        }
        if self.yield_after_release {
            self.yield_after_release = false;
            return Ok(None);
        }
        // KEY and CLICK model complete high-level input strokes. Keep a command
        // queued until the previous stroke's release has reached the runtime;
        // callers that need a long press can still select its hold duration.
        if !self.key_releases.is_empty() || !self.pointer_releases.is_empty() {
            return Ok(None);
        }
        match self.commands.try_recv() {
            Ok(ControlMessage::Event(event)) => Ok(Some(event)),
            Ok(ControlMessage::Key {
                code,
                hold,
                accepted,
            }) => {
                let deadline = Instant::now()
                    .checked_add(hold)
                    .unwrap_or_else(Instant::now);
                self.key_releases.push((deadline, code));
                let draw_count = self
                    .state
                    .data
                    .lock()
                    .map_err(|_| Error::Platform("E2E capture state is poisoned".into()))?
                    .draw_count;
                let _ = accepted.send(draw_count);
                Ok(Some(DisplayEvent::Key {
                    code,
                    pressed: true,
                }))
            }
            Ok(ControlMessage::Pointer {
                x,
                y,
                hold,
                accepted,
            }) => {
                let deadline = Instant::now()
                    .checked_add(hold)
                    .unwrap_or_else(Instant::now);
                self.pointer_releases.push((deadline, x, y));
                let draw_count = self
                    .state
                    .data
                    .lock()
                    .map_err(|_| Error::Platform("E2E capture state is poisoned".into()))?
                    .draw_count;
                let _ = accepted.send(draw_count);
                Ok(Some(DisplayEvent::Pointer {
                    x,
                    y,
                    pressed: true,
                }))
            }
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => Ok(Some(DisplayEvent::Quit)),
        }
    }

    fn wait_timeout(&mut self, milliseconds: u32) {
        let requested = Duration::from_millis(u64::from(milliseconds));
        let now = Instant::now();
        let until_release = self
            .key_releases
            .iter()
            .map(|(deadline, _)| deadline.saturating_duration_since(now))
            .chain(
                self.pointer_releases
                    .iter()
                    .map(|(deadline, _, _)| deadline.saturating_duration_since(now)),
            )
            .min();
        thread::sleep(until_release.map_or(requested, |delay| delay.min(requested)));
    }
}

fn serve(listener: UnixListener, state: Arc<CaptureState>, sender: Sender<ControlMessage>) {
    for stream in listener.incoming() {
        let Ok(mut stream) = stream else {
            continue;
        };
        let response = read_command(&mut stream)
            .and_then(|command| {
                handle_command(command.trim_end_matches(['\r', '\n']), &state, &sender)
            })
            .unwrap_or_else(|error| format!("ERR {}", one_line(&error)));
        let _ = writeln!(stream, "{response}");
    }
}

fn read_command(stream: &mut UnixStream) -> std::result::Result<String, String> {
    let mut command = String::new();
    BufReader::new(stream)
        .read_line(&mut command)
        .map_err(|error| format!("read_failed {error}"))?;
    if command.is_empty() {
        return Err("empty_command".into());
    }
    Ok(command)
}

fn handle_command(
    command: &str,
    state: &CaptureState,
    sender: &Sender<ControlMessage>,
) -> std::result::Result<String, String> {
    if command == "SET_CLIPBOARD" || command.starts_with("SET_CLIPBOARD ") {
        let text = command.strip_prefix("SET_CLIPBOARD").unwrap();
        let text = text.strip_prefix(' ').unwrap_or(text);
        if text.len() > MAX_CLIPBOARD_BYTES {
            return Err("clipboard_too_large".into());
        }
        let mut data = state
            .data
            .lock()
            .map_err(|_| "capture_state_poisoned".to_string())?;
        data.clipboard.clear();
        data.clipboard.push_str(text);
        return Ok("OK clipboard".into());
    }
    if command == "PASTE_SHORTCUT" {
        let text = state
            .data
            .lock()
            .map_err(|_| "capture_state_poisoned".to_string())?
            .clipboard
            .clone();
        sender
            .send(ControlMessage::Event(DisplayEvent::TextInput { text }))
            .map_err(|_| "runtime_exited".to_string())?;
        return Ok("OK paste".into());
    }
    if command == "DRAW_COUNT" {
        let data = state
            .data
            .lock()
            .map_err(|_| "capture_state_poisoned".to_string())?;
        return Ok(format!("OK draw_count {}", data.draw_count));
    }
    if let Some(arguments) = command.strip_prefix("WAIT_DRAW ") {
        let mut arguments = arguments.split_whitespace();
        let previous = parse_u64(arguments.next(), "draw_count")?;
        let timeout_ms = parse_u64(arguments.next(), "timeout_ms")?;
        if arguments.next().is_some() {
            return Err("invalid_wait_draw".into());
        }
        return wait_draw(state, previous, Duration::from_millis(timeout_ms));
    }
    if let Some(arguments) = command.strip_prefix("SCREEN_DRAW ") {
        let (draw, path) = arguments
            .split_once(' ')
            .ok_or_else(|| "invalid_screen_draw".to_string())?;
        let draw = draw
            .parse::<u64>()
            .map_err(|_| "invalid_screen_draw_count".to_string())?;
        let frame = {
            let data = state
                .data
                .lock()
                .map_err(|_| "capture_state_poisoned".to_string())?;
            data.history
                .iter()
                .find(|(count, _)| *count == draw)
                .map(|(_, frame)| frame.clone())
                .ok_or_else(|| format!("frame_not_found {draw}"))?
        };
        write_ppm(Path::new(path), &frame).map_err(|error| format!("screen_failed {error}"))?;
        return Ok(format!("OK screen_draw {draw}"));
    }
    if let Some(path) = command.strip_prefix("SCREEN ") {
        let needs_first_frame = state
            .data
            .lock()
            .map_err(|_| "capture_state_poisoned".to_string())?
            .draw_count
            == 0;
        if needs_first_frame {
            // The zero-count frame is only a transport placeholder. A normal
            // screenshot represents application output; SCREEN_DRAW 0 remains
            // available to tests that explicitly need the placeholder.
            wait_draw(state, 0, Duration::from_secs(25))?;
        }
        let frame = {
            let data = state
                .data
                .lock()
                .map_err(|_| "capture_state_poisoned".to_string())?;
            data.latest.clone().ok_or_else(|| "no_frame".to_string())?
        };
        write_ppm(Path::new(path), &frame).map_err(|error| format!("screen_failed {error}"))?;
        return Ok("OK screen".into());
    }
    if let Some(arguments) = command.strip_prefix("KEY ") {
        let mut arguments = arguments.split_whitespace();
        let name = arguments
            .next()
            .ok_or_else(|| "missing_key_name".to_string())?;
        let hold = match arguments.next() {
            Some(value) => Duration::from_millis(
                value
                    .parse::<u64>()
                    .map_err(|_| "invalid_key_hold".to_string())?,
            ),
            None => Duration::from_millis(default_key_hold_ms()),
        };
        if arguments.next().is_some() {
            return Err("invalid_key".into());
        }
        let (accepted, acceptance) = mpsc::channel();
        sender
            .send(ControlMessage::Key {
                code: key_code(name)?,
                hold,
                accepted,
            })
            .map_err(|_| "runtime_exited".to_string())?;
        let draw_count = acceptance
            .recv_timeout(Duration::from_secs(30))
            .map_err(|_| "key_accept_timeout".to_string())?;
        return Ok(format!("OK key draw_count {draw_count}"));
    }
    if let Some(arguments) = command.strip_prefix("CLICK ") {
        let mut arguments = arguments.split_whitespace();
        let x = parse_i32(arguments.next(), "pointer_x")?;
        let y = parse_i32(arguments.next(), "pointer_y")?;
        let hold = match arguments.next() {
            Some(value) => Duration::from_millis(
                value
                    .parse::<u64>()
                    .map_err(|_| "invalid_pointer_hold".to_string())?,
            ),
            None => Duration::from_millis(default_pointer_hold_ms()),
        };
        if arguments.next().is_some() {
            return Err("invalid_click".into());
        }
        let (accepted, acceptance) = mpsc::channel();
        sender
            .send(ControlMessage::Pointer {
                x,
                y,
                hold,
                accepted,
            })
            .map_err(|_| "runtime_exited".to_string())?;
        let draw_count = acceptance
            .recv_timeout(Duration::from_secs(30))
            .map_err(|_| "pointer_accept_timeout".to_string())?;
        return Ok(format!("OK click draw_count {draw_count}"));
    }
    if command == "QUIT" {
        sender
            .send(ControlMessage::Event(DisplayEvent::Quit))
            .map_err(|_| "runtime_exited".to_string())?;
        return Ok("OK quit".into());
    }
    Err("unknown_command".into())
}

fn wait_draw(
    state: &CaptureState,
    previous: u64,
    timeout: Duration,
) -> std::result::Result<String, String> {
    let deadline = Instant::now()
        .checked_add(timeout)
        .unwrap_or_else(Instant::now);
    let mut data = state
        .data
        .lock()
        .map_err(|_| "capture_state_poisoned".to_string())?;
    while data.draw_count <= previous && !data.exited {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        let (next, wait) = state
            .changed
            .wait_timeout(data, remaining)
            .map_err(|_| "capture_state_poisoned".to_string())?;
        data = next;
        if wait.timed_out() {
            break;
        }
    }
    if data.draw_count > previous {
        Ok(format!("OK draw_count {}", data.draw_count))
    } else if data.exited {
        Ok(format!("OK draw_count {} exited", data.draw_count))
    } else {
        Err(format!("wait_draw_timeout draw_count {}", data.draw_count))
    }
}

fn write_ppm(path: &Path, frame: &CapturedFrame) -> std::io::Result<()> {
    let mut output = BufWriter::new(File::create(path)?);
    writeln!(output, "P6\n{} {}\n255", frame.width, frame.height)?;
    for pixel in &frame.pixels {
        let red = ((pixel >> 11) & 0x1f) as u8;
        let green = ((pixel >> 5) & 0x3f) as u8;
        let blue = (pixel & 0x1f) as u8;
        output.write_all(&[red << 3, green << 2, blue << 3])?;
    }
    output.flush()
}

fn parse_u64(value: Option<&str>, label: &str) -> std::result::Result<u64, String> {
    value
        .ok_or_else(|| format!("missing_{label}"))?
        .parse::<u64>()
        .map_err(|_| format!("invalid_{label}"))
}

fn parse_i32(value: Option<&str>, label: &str) -> std::result::Result<i32, String> {
    value
        .ok_or_else(|| format!("missing_{label}"))?
        .parse::<i32>()
        .map_err(|_| format!("invalid_{label}"))
}

fn default_key_hold_ms() -> u64 {
    std::env::var("SKYENGINE_E2E_KEY_HOLD_MS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(DEFAULT_KEY_HOLD_MS)
}

fn default_pointer_hold_ms() -> u64 {
    std::env::var("SKYENGINE_E2E_HOLD_MS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(DEFAULT_POINTER_HOLD_MS)
}

fn key_code(name: &str) -> std::result::Result<i32, String> {
    let upper = name.to_ascii_uppercase();
    let code = match upper.as_str() {
        "ENTER" | "SELECT" => 20,
        "ESC" | "ESCAPE" | "POWER" => 16,
        "SOFTLEFT" | "LEFT_SOFT" => 17,
        "SOFTRIGHT" | "RIGHT_SOFT" => 18,
        "UP" => 12,
        "DOWN" => 13,
        "LEFT" => 14,
        "RIGHT" => 15,
        "SEND" => 19,
        "STAR" | "*" => 10,
        "POUND" | "HASH" | "#" => 11,
        digit if digit.len() == 1 && digit.as_bytes()[0].is_ascii_digit() => {
            i32::from(digit.as_bytes()[0] - b'0')
        }
        _ => return Err(format!("unknown_key {name}")),
    };
    Ok(code)
}

fn one_line(error: &str) -> String {
    error.replace(['\r', '\n'], " ")
}
