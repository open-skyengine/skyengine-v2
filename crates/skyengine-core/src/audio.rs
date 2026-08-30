use std::{
    f32::consts::TAU,
    fs::File,
    io::Cursor,
    path::Path,
    sync::{Arc, Mutex, MutexGuard},
};

use midly::{MetaMessage, MidiMessage, Smf, Timing, TrackEventKind};
use rustysynth::{MidiFile, MidiFileSequencer, SoundFont, Synthesizer, SynthesizerSettings};
use symphonia::core::{
    audio::SampleBuffer, codecs::DecoderOptions, errors::Error as SymphoniaError,
    formats::FormatOptions, io::MediaSourceStream, meta::MetadataOptions, probe::Hint,
};

use crate::{Error, Result};

pub const AUDIO_SAMPLE_RATE: u32 = 44_100;
pub const AUDIO_CHANNELS: usize = 2;
const MAX_AUDIO_SECONDS: usize = 10 * 60;
const MAX_AUDIO_FRAMES: usize = AUDIO_SAMPLE_RATE as usize * MAX_AUDIO_SECONDS;
const MAX_SOURCE_SAMPLES: usize = 64 * 1024 * 1024;
const MAX_MIDI_EVENTS: usize = 1_000_000;
const MAX_MIDI_VOICES: usize = 128;
const MAX_SOUNDFONT_BYTES: u64 = 128 * 1024 * 1024;
const MIDI_TAIL_FRAMES: usize = AUDIO_SAMPLE_RATE as usize * 4;
const MIDI_PERCUSSION_RELEASE_FRAMES: usize = AUDIO_SAMPLE_RATE as usize / 100;
// Held-envelope value below which a naturally decaying voice counts as silent.
const MIDI_SILENCE_LEVEL: f32 = 5.0e-4;
const MIDI_PERCUSSION_GAIN: f32 = 0.28;
/// Final headroom scaler for the summed GM mix.
const MIDI_MASTER_GAIN: f32 = 0.62;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SoundType {
    Midi,
    Wav,
    Mp3,
}

pub trait PlatformAudio {
    fn play_sound(&mut self, sound_type: SoundType, data: &[u8], looped: bool) -> Result<()>;
    fn stop_sound(&mut self) -> Result<()>;
    fn is_active(&self) -> bool;
    fn set_volume(&mut self, volume: u8) -> Result<()>;
}

#[derive(Default)]
pub struct SilentAudio;

impl PlatformAudio for SilentAudio {
    fn play_sound(&mut self, _sound_type: SoundType, _data: &[u8], _looped: bool) -> Result<()> {
        Ok(())
    }

    fn stop_sound(&mut self) -> Result<()> {
        Ok(())
    }

    fn is_active(&self) -> bool {
        false
    }

    fn set_volume(&mut self, _volume: u8) -> Result<()> {
        Ok(())
    }
}

impl SoundType {
    pub fn from_mrp(value: u32) -> Option<Self> {
        match value {
            0 => Some(Self::Midi),
            1 => Some(Self::Wav),
            2 => Some(Self::Mp3),
            _ => None,
        }
    }
}

#[derive(Clone)]
pub struct AudioPlayer {
    inner: Arc<Mutex<Playback>>,
    sound_font: Option<Arc<SoundFont>>,
}

impl Default for AudioPlayer {
    fn default() -> Self {
        Self {
            inner: Arc::new(Mutex::new(Playback::default())),
            sound_font: None,
        }
    }
}

struct Playback {
    samples: Vec<i16>,
    position: usize,
    looped: bool,
    volume: f32,
}

impl Default for Playback {
    fn default() -> Self {
        Self {
            samples: Vec::new(),
            position: 0,
            looped: false,
            volume: 1.0,
        }
    }
}

impl AudioPlayer {
    /// Creates a player backed by an SF2 SoundFont for General MIDI synthesis.
    pub fn with_sound_font(data: &[u8]) -> Result<Self> {
        if data.len() as u64 > MAX_SOUNDFONT_BYTES {
            return Err(Error::ResourceLimit(format!(
                "SoundFont exceeds the {} MiB limit",
                MAX_SOUNDFONT_BYTES / 1024 / 1024
            )));
        }
        let sound_font = SoundFont::new(&mut Cursor::new(data))
            .map_err(|error| Error::Platform(format!("invalid SoundFont data: {error}")))?;
        Ok(Self {
            inner: Arc::new(Mutex::new(Playback::default())),
            sound_font: Some(Arc::new(sound_font)),
        })
    }

    /// Loads an SF2 SoundFont from disk and creates a General MIDI player.
    pub fn with_sound_font_file(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let metadata = std::fs::metadata(path).map_err(|source| Error::Io {
            path: path.to_path_buf(),
            source,
        })?;
        if metadata.len() > MAX_SOUNDFONT_BYTES {
            return Err(Error::ResourceLimit(format!(
                "SoundFont exceeds the {} MiB limit",
                MAX_SOUNDFONT_BYTES / 1024 / 1024
            )));
        }
        let mut file = File::open(path).map_err(|source| Error::Io {
            path: path.to_path_buf(),
            source,
        })?;
        let sound_font = SoundFont::new(&mut file)
            .map_err(|error| Error::Platform(format!("invalid SoundFont data: {error}")))?;
        Ok(Self {
            inner: Arc::new(Mutex::new(Playback::default())),
            sound_font: Some(Arc::new(sound_font)),
        })
    }

    pub fn play(&self, sound_type: SoundType, data: &[u8], looped: bool) -> Result<()> {
        let samples = match sound_type {
            SoundType::Midi => match &self.sound_font {
                Some(sound_font) => decode_midi_with_sound_font(data, sound_font)?,
                None => decode_midi(data)?,
            },
            SoundType::Wav => decode_wav(data)?,
            SoundType::Mp3 => decode_mp3(data)?,
        };
        if samples.is_empty() {
            return Err(Error::Platform("decoded audio contains no samples".into()));
        }
        let mut playback = lock(&self.inner);
        playback.samples = samples;
        playback.position = 0;
        playback.looped = looped;
        Ok(())
    }

    pub fn stop(&self) {
        let mut playback = lock(&self.inner);
        playback.samples.clear();
        playback.position = 0;
        playback.looped = false;
    }

    pub fn is_active(&self) -> bool {
        !lock(&self.inner).samples.is_empty()
    }

    pub fn set_volume(&self, volume: u8) {
        lock(&self.inner).volume = f32::from(volume.min(5)) / 5.0;
    }

    /// Renders interleaved stereo S16LE samples and returns the number of frames produced.
    /// Any unused portion of `output` is filled with silence.
    pub fn render(&self, output: &mut [i16]) -> usize {
        output.fill(0);
        let output_samples = output.len() / AUDIO_CHANNELS * AUDIO_CHANNELS;
        if output_samples == 0 {
            return 0;
        }

        let mut playback = lock(&self.inner);
        if playback.samples.is_empty() {
            return 0;
        }
        let mut written = 0;
        while written < output_samples && !playback.samples.is_empty() {
            let available = playback.samples.len().saturating_sub(playback.position);
            let copied = available.min(output_samples - written);
            let volume = playback.volume;
            for (destination, source) in output[written..written + copied]
                .iter_mut()
                .zip(&playback.samples[playback.position..playback.position + copied])
            {
                *destination = (f32::from(*source) * volume).round() as i16;
            }
            playback.position += copied;
            written += copied;
            if playback.position == playback.samples.len() {
                if playback.looped {
                    playback.position = 0;
                } else {
                    playback.samples.clear();
                    playback.position = 0;
                }
            }
        }
        written / AUDIO_CHANNELS
    }
}

impl PlatformAudio for AudioPlayer {
    fn play_sound(&mut self, sound_type: SoundType, data: &[u8], looped: bool) -> Result<()> {
        self.play(sound_type, data, looped)
    }

    fn stop_sound(&mut self) -> Result<()> {
        self.stop();
        Ok(())
    }

    fn is_active(&self) -> bool {
        AudioPlayer::is_active(self)
    }

    fn set_volume(&mut self, volume: u8) -> Result<()> {
        AudioPlayer::set_volume(self, volume);
        Ok(())
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[derive(Clone, Copy)]
enum MidiEvent {
    NoteOn {
        channel: u8,
        key: u8,
        velocity: u8,
    },
    NoteOff {
        channel: u8,
        key: u8,
    },
    Program {
        channel: u8,
        program: u8,
    },
    Controller {
        channel: u8,
        controller: u8,
        value: u8,
    },
    PitchBend {
        channel: u8,
        bend: i16,
    },
    Tempo(u32),
}

#[derive(Clone, Copy)]
struct TimedMidiEvent {
    tick: u64,
    order: usize,
    event: MidiEvent,
}

/// RPN parameter selectors carried by CC100/CC101 before data entry lands via CC6/CC38.
const RPN_PITCH_BEND_RANGE: u16 = 0x0000;
const RPN_FINE_TUNING: u16 = 0x0001;
const RPN_COARSE_TUNING: u16 = 0x0002;
const RPN_NULL: u16 = 0x3FFF;
const DEFAULT_PITCH_BEND_RANGE: f32 = 2.0;

struct MidiChannel {
    program: u8,
    volume: f32,
    expression: f32,
    pan: f32,
    /// Normalized bend in `-1.0..=1.0`.
    pitch_bend: f32,
    bend_range_semitones: f32,
    fine_tune_cents: f32,
    coarse_tune_semitones: f32,
    rpn_select: u16,
    data_entry_msb: u8,
    data_entry_lsb: u8,
    reverb_send: f32,
    chorus_send: f32,
    tremolo_depth: f32,
}

impl Default for MidiChannel {
    fn default() -> Self {
        Self {
            program: 0,
            volume: 100.0 / 127.0,
            expression: 1.0,
            pan: 0.5,
            pitch_bend: 0.0,
            bend_range_semitones: DEFAULT_PITCH_BEND_RANGE,
            fine_tune_cents: 0.0,
            coarse_tune_semitones: 0.0,
            rpn_select: RPN_NULL,
            data_entry_msb: 0,
            data_entry_lsb: 0,
            reverb_send: 0.0,
            chorus_send: 0.0,
            tremolo_depth: 0.0,
        }
    }
}

impl MidiChannel {
    /// Instantaneous frequency of `key` on this channel including bend range,
    /// RPN tuning and an extra per-voice vibrato offset in cents.
    fn frequency(&self, key: u8, vibrato_cents: f32) -> f32 {
        let tuning_cents =
            self.fine_tune_cents + self.coarse_tune_semitones * 100.0 + vibrato_cents;
        let semitones = f32::from(key) - 69.0
            + self.pitch_bend * self.bend_range_semitones
            + tuning_cents / 100.0;
        440.0 * (semitones / 12.0).exp2()
    }

    fn apply_data_entry(&mut self) {
        let value = (u16::from(self.data_entry_msb) << 7) | u16::from(self.data_entry_lsb);
        match self.rpn_select {
            RPN_PITCH_BEND_RANGE => {
                self.bend_range_semitones = f32::from(self.data_entry_msb);
            }
            RPN_FINE_TUNING => {
                self.fine_tune_cents = (f32::from(value) - 8_192.0) * 100.0 / 8_192.0;
            }
            RPN_COARSE_TUNING => {
                self.coarse_tune_semitones = f32::from(self.data_entry_msb) - 64.0;
            }
            _ => {}
        }
    }
}

/// Linear attack followed by exponential settling toward `sustain_level`;
/// a plain linear ramp handles the post note-off release.
struct Envelope {
    attack_frames: usize,
    /// Per-frame multiplier once the attack finished; `1.0` sustains forever.
    decay_factor: f32,
    sustain_level: f32,
    release_frames: usize,
    level: f32,
}

impl Envelope {
    fn advance(&mut self, age_frames: usize) {
        if age_frames < self.attack_frames {
            self.level = (age_frames + 1) as f32 / self.attack_frames as f32;
        } else if self.decay_factor < 1.0 && self.level > self.sustain_level {
            self.level = (self.level * self.decay_factor).max(self.sustain_level);
        }
    }
}

/// `[MidiChannel::default(); 16]` needs `Copy`; build the table instead.
fn default_channels() -> [MidiChannel; 16] {
    std::array::from_fn(|_| MidiChannel::default())
}

/// Exponentially decaying sinusoid partial with an independent ratio and lifetime.
struct Partial {
    ratio: f32,
    phase: f32,
    amplitude: f32,
    decay_factor: f32,
}

struct Lfo {
    rate_hz: f32,
    phase: f32,
}

impl Lfo {
    fn sine(&mut self) -> f32 {
        let value = (self.phase * TAU).sin();
        self.phase = (self.phase + self.rate_hz / AUDIO_SAMPLE_RATE as f32).fract();
        value
    }
}

#[derive(Clone, Copy)]
enum Shape {
    Sine,
    Saw,
    /// Band-limited rectangular wave with the given duty cycle.
    Pulse(f32),
}

const ENSEMBLE_OSCILLATORS: usize = 3;

enum Timbre {
    /// Karplus-Strong delay line; the pitch is fixed at note-on like a real string.
    Plucked {
        delay: Vec<f32>,
        cursor: usize,
        damping: f32,
        drive: f32,
    },
    /// Fixed additive stack with per-partial decay; used by mallets, bells, pianos.
    Harmonic {
        partials: [Partial; 4],
        motor: Option<Lfo>,
    },
    /// Detuned oscillator bank through a one-pole lowpass for sustained instruments.
    Ensemble {
        shape: Shape,
        detune_factors: [f32; ENSEMBLE_OSCILLATORS],
        phases: [f32; ENSEMBLE_OSCILLATORS],
        lowpass_alpha: f32,
        lowpass_target_alpha: f32,
        lowpass_state: f32,
        vibrato: Option<Lfo>,
        tremolo_rate: Option<f32>,
    },
    Drum,
}

struct Voice {
    channel: u8,
    key: u8,
    velocity_gain: f32,
    gain_trim: f32,
    age_frames: usize,
    release_frame: Option<usize>,
    envelope: Envelope,
    timbre: Timbre,
    /// Shared scratch phase plus LFO state for the drum kit voices.
    phase: f32,
    secondary_phase: f32,
    noise: u32,
    filtered_noise: f32,
}

fn decode_midi_with_sound_font(data: &[u8], sound_font: &Arc<SoundFont>) -> Result<Vec<i16>> {
    let smf =
        Smf::parse(data).map_err(|error| Error::Platform(format!("invalid MIDI data: {error}")))?;
    match smf.header.timing {
        Timing::Metrical(ticks) if ticks.as_int() != 0 => {}
        Timing::Metrical(_) => {
            return Err(Error::Platform("MIDI timing division is zero".into()));
        }
        Timing::Timecode(_, _) => {
            return Err(Error::Platform(
                "SMPTE-timed MIDI files are not supported".into(),
            ));
        }
    }

    let mut event_count = 0_usize;
    let mut has_channel_event = false;
    for track in &smf.tracks {
        event_count = event_count
            .checked_add(track.len())
            .ok_or_else(|| Error::ResourceLimit("MIDI event count overflows".into()))?;
        if event_count > MAX_MIDI_EVENTS {
            return Err(Error::ResourceLimit(format!(
                "MIDI event count exceeds {MAX_MIDI_EVENTS}"
            )));
        }
        has_channel_event |= track
            .iter()
            .any(|event| matches!(event.kind, TrackEventKind::Midi { .. }));
    }
    if !has_channel_event {
        return Err(Error::Platform("MIDI contains no channel events".into()));
    }

    let midi_file = Arc::new(
        MidiFile::new(&mut Cursor::new(data))
            .map_err(|error| Error::Platform(format!("invalid MIDI data: {error}")))?,
    );
    let duration = midi_file.get_length();
    if !duration.is_finite() || duration < 0.0 {
        return Err(Error::Platform("MIDI has an invalid duration".into()));
    }
    let body_frames_f64 = duration * f64::from(AUDIO_SAMPLE_RATE);
    if body_frames_f64 > MAX_AUDIO_FRAMES as f64 {
        return Err(Error::ResourceLimit(format!(
            "decoded audio exceeds the {MAX_AUDIO_SECONDS} second limit"
        )));
    }
    let body_frames = body_frames_f64.ceil() as usize;
    let total_frames = body_frames
        .checked_add(MIDI_TAIL_FRAMES)
        .filter(|frames| *frames <= MAX_AUDIO_FRAMES)
        .ok_or_else(|| {
            Error::ResourceLimit(format!(
                "decoded audio exceeds the {MAX_AUDIO_SECONDS} second limit"
            ))
        })?;

    let mut settings = SynthesizerSettings::new(AUDIO_SAMPLE_RATE as i32);
    settings.maximum_polyphony = MAX_MIDI_VOICES;
    let synthesizer = Synthesizer::new(sound_font, &settings).map_err(|error| {
        Error::Platform(format!("failed to initialize MIDI synthesizer: {error}"))
    })?;
    let mut sequencer = MidiFileSequencer::new(synthesizer);
    sequencer.play(&midi_file, false);

    const CHUNK_FRAMES: usize = 4_096;
    let mut samples = Vec::with_capacity(total_frames.saturating_mul(AUDIO_CHANNELS));
    let mut left = vec![0.0_f32; CHUNK_FRAMES];
    let mut right = vec![0.0_f32; CHUNK_FRAMES];
    let mut rendered = 0_usize;
    while rendered < total_frames {
        let frames = (total_frames - rendered).min(CHUNK_FRAMES);
        sequencer.render(&mut left[..frames], &mut right[..frames]);
        for (&left, &right) in left[..frames].iter().zip(&right[..frames]) {
            samples.push(float_to_i16(left));
            samples.push(float_to_i16(right));
        }
        rendered += frames;
    }

    let tail_start = body_frames.saturating_mul(AUDIO_CHANNELS);
    while samples.len() > tail_start
        && samples[samples.len() - AUDIO_CHANNELS..]
            .iter()
            .all(|sample| sample.abs() <= 2)
    {
        samples.truncate(samples.len() - AUDIO_CHANNELS);
    }
    Ok(samples)
}

fn decode_midi(data: &[u8]) -> Result<Vec<i16>> {
    let smf =
        Smf::parse(data).map_err(|error| Error::Platform(format!("invalid MIDI data: {error}")))?;
    let ticks_per_quarter = match smf.header.timing {
        Timing::Metrical(ticks) => u64::from(ticks.as_int()),
        Timing::Timecode(_, _) => {
            return Err(Error::Platform(
                "SMPTE-timed MIDI files are not supported".into(),
            ));
        }
    };
    if ticks_per_quarter == 0 {
        return Err(Error::Platform("MIDI timing division is zero".into()));
    }

    let mut events = Vec::new();
    let mut order = 0;
    for track in &smf.tracks {
        let mut tick = 0_u64;
        for track_event in track {
            tick = tick
                .checked_add(u64::from(track_event.delta.as_int()))
                .ok_or_else(|| Error::ResourceLimit("MIDI timeline overflows".into()))?;
            let event = match track_event.kind {
                TrackEventKind::Midi { channel, message } => match message {
                    MidiMessage::NoteOn { key, vel } if vel.as_int() != 0 => MidiEvent::NoteOn {
                        channel: channel.as_int(),
                        key: key.as_int(),
                        velocity: vel.as_int(),
                    },
                    MidiMessage::NoteOn { key, .. } | MidiMessage::NoteOff { key, .. } => {
                        MidiEvent::NoteOff {
                            channel: channel.as_int(),
                            key: key.as_int(),
                        }
                    }
                    MidiMessage::ProgramChange { program } => MidiEvent::Program {
                        channel: channel.as_int(),
                        program: program.as_int(),
                    },
                    MidiMessage::Controller { controller, value } => MidiEvent::Controller {
                        channel: channel.as_int(),
                        controller: controller.as_int(),
                        value: value.as_int(),
                    },
                    MidiMessage::PitchBend { bend } => MidiEvent::PitchBend {
                        channel: channel.as_int(),
                        // midly centers the bend for us: 0 = no bend.
                        bend: bend.as_int(),
                    },
                    _ => continue,
                },
                TrackEventKind::Meta(MetaMessage::Tempo(tempo)) => MidiEvent::Tempo(tempo.as_int()),
                _ => continue,
            };
            events.push(TimedMidiEvent { tick, order, event });
            if events.len() > MAX_MIDI_EVENTS {
                return Err(Error::ResourceLimit(format!(
                    "MIDI event count exceeds {MAX_MIDI_EVENTS}"
                )));
            }
            order += 1;
        }
    }
    events.sort_by_key(|event| (event.tick, event.order));

    // Effect hardware only spins up when the file actually requests it.
    let needs_reverb = events.iter().any(|event| {
        matches!(
            event.event,
            MidiEvent::Controller {
                channel: _,
                controller: 91,
                value: 1..
            }
        )
    });
    let needs_chorus = events.iter().any(|event| {
        matches!(
            event.event,
            MidiEvent::Controller {
                channel: _,
                controller: 93,
                value: 1..
            }
        )
    });

    let mut samples = Vec::new();
    let mut channels = default_channels();
    let mut voices = Vec::<Voice>::new();
    let mut reverb = needs_reverb.then(ReverbRack::default);
    let mut chorus = needs_chorus.then(ChorusRack::default);
    let mut tempo_micros = 500_000_u64;
    let mut current_tick = 0_u64;
    let mut fractional_frames = 0.0_f64;
    let mut index = 0;
    while index < events.len() {
        let tick = events[index].tick;
        let delta_ticks = tick.saturating_sub(current_tick);
        let exact_frames = delta_ticks as f64 * tempo_micros as f64 * AUDIO_SAMPLE_RATE as f64
            / (ticks_per_quarter as f64 * 1_000_000.0)
            + fractional_frames;
        let frame_count = exact_frames.floor() as usize;
        fractional_frames = exact_frames - frame_count as f64;
        render_midi_frames(
            &mut samples,
            frame_count,
            &mut voices,
            &channels,
            reverb.as_mut(),
            chorus.as_mut(),
        )?;
        current_tick = tick;

        while index < events.len() && events[index].tick == tick {
            match events[index].event {
                MidiEvent::NoteOn {
                    channel,
                    key,
                    velocity,
                } => {
                    start_midi_voice(&channels, &mut voices, channel, key, velocity);
                }
                MidiEvent::NoteOff { channel, key } => {
                    release_voices(&mut voices, channel, key);
                }
                MidiEvent::Program { channel, program } => {
                    channels[usize::from(channel)].program = program;
                }
                MidiEvent::Controller {
                    channel,
                    controller,
                    value,
                } => apply_controller(
                    &mut channels[usize::from(channel)],
                    &mut voices,
                    channel,
                    controller,
                    value,
                ),
                MidiEvent::PitchBend { channel, bend } => {
                    channels[usize::from(channel)].pitch_bend = f32::from(bend) / 8_192.0;
                }
                MidiEvent::Tempo(tempo) if tempo != 0 => tempo_micros = u64::from(tempo),
                MidiEvent::Tempo(_) => {}
            }
            index += 1;
        }
    }

    // The device stops the song at the last event: fade whatever still sounds and
    // keep rendering until the effect tails settle, then trim near-silence.
    if !voices.is_empty() {
        for voice in voices.iter_mut().filter(|voice| voice.channel != 9) {
            voice.release_frame.get_or_insert(voice.age_frames);
        }
        let tail_start = samples.len();
        'tail: while samples.len() / AUDIO_CHANNELS - tail_start / AUDIO_CHANNELS < MIDI_TAIL_FRAMES
        {
            let remaining_tail =
                MIDI_TAIL_FRAMES - (samples.len() / AUDIO_CHANNELS - tail_start / AUDIO_CHANNELS);
            render_midi_frames(
                &mut samples,
                remaining_tail.min(AUDIO_SAMPLE_RATE as usize / 8),
                &mut voices,
                &channels,
                reverb.as_mut(),
                chorus.as_mut(),
            )?;
            let chunk_start = samples
                .len()
                .saturating_sub(AUDIO_SAMPLE_RATE as usize / 8 * AUDIO_CHANNELS);
            if samples[chunk_start..]
                .iter()
                .all(|sample| sample.abs() <= 2)
            {
                break 'tail;
            }
        }
        while samples.len() > tail_start
            && samples[samples.len() - AUDIO_CHANNELS..]
                .iter()
                .all(|sample| sample.abs() <= 2)
        {
            samples.truncate(samples.len() - AUDIO_CHANNELS);
        }
    }
    Ok(samples)
}

fn apply_controller(
    channel: &mut MidiChannel,
    voices: &mut Vec<Voice>,
    channel_index: u8,
    controller: u8,
    value: u8,
) {
    match controller {
        6 => {
            channel.data_entry_msb = value;
            channel.apply_data_entry();
        }
        7 => channel.volume = f32::from(value) / 127.0,
        10 => channel.pan = f32::from(value) / 127.0,
        11 => channel.expression = f32::from(value) / 127.0,
        38 => {
            channel.data_entry_lsb = value;
            channel.apply_data_entry();
        }
        91 => channel.reverb_send = f32::from(value) / 127.0,
        92 => channel.tremolo_depth = f32::from(value) / 127.0,
        93 => channel.chorus_send = f32::from(value) / 127.0,
        100 => {
            channel.rpn_select = (channel.rpn_select & 0x3F80) | u16::from(value);
        }
        101 => {
            channel.rpn_select = (channel.rpn_select & 0x007F) | (u16::from(value) << 7);
        }
        120 => voices.retain(|voice| voice.channel != channel_index),
        123 => {
            for voice in voices
                .iter_mut()
                .filter(|voice| voice.channel == channel_index)
            {
                voice.release_frame.get_or_insert(voice.age_frames);
            }
        }
        _ => {}
    }
}

fn release_voices(voices: &mut [Voice], channel: u8, key: u8) {
    for voice in voices
        .iter_mut()
        .filter(|voice| voice.channel == channel && voice.key == key)
    {
        voice.release_frame.get_or_insert(voice.age_frames);
    }
}

fn start_midi_voice(
    channels: &[MidiChannel; 16],
    voices: &mut Vec<Voice>,
    channel: u8,
    key: u8,
    velocity: u8,
) {
    if channel == 9 {
        voices.retain(|voice| voice.channel != channel || !percussion_chokes(key, voice.key));
    } else {
        for voice in voices
            .iter_mut()
            .filter(|voice| voice.channel == channel && voice.key == key)
        {
            voice.release_frame.get_or_insert(voice.age_frames);
        }
    }
    if voices.len() == MAX_MIDI_VOICES {
        voices.remove(0);
    }
    voices.push(build_voice(
        &channels[usize::from(channel)],
        channel,
        key,
        velocity,
    ));
}

#[allow(clippy::too_many_lines)]
fn build_voice(channel: &MidiChannel, channel_index: u8, key: u8, velocity: u8) -> Voice {
    let velocity_gain = (f32::from(velocity) / 127.0).powf(1.35);
    // The drum kit is keyed by note number instead of programs; its shaping,
    // envelope and duration all live inside percussion_sample.
    if channel_index == 9 {
        return Voice {
            channel: channel_index,
            key,
            velocity_gain,
            gain_trim: MIDI_PERCUSSION_GAIN * percussion_mix_gain(key),
            age_frames: 0,
            release_frame: None,
            envelope: envelope(0.0, 1.0, 1.0, 0.01),
            timbre: Timbre::Drum,
            phase: 0.0,
            secondary_phase: 0.0,
            noise: 0x9e37_79b9 ^ (u32::from(channel_index) << 8) ^ u32::from(key),
            filtered_noise: 0.0,
        };
    }
    let program = channel.program;
    // Louder strikes also ring longer and brighter on real instruments.
    let sustain_scale = 0.55 + 0.9 * velocity_gain.sqrt();
    let frequency = channel.frequency(key, 0.0);

    // (timbre, envelope, trim) triples selected per GM program family.
    let (timbre, envelope, gain_trim) = match program {
        0..=3 => {
            let lifetime = 3.2 * sustain_scale * (262.0 / frequency.max(60.0)).powf(0.35);
            (
                Timbre::Harmonic {
                    partials: decaying_partials(
                        &[1.0, 2.0, 3.01, 4.02],
                        &[1.0, 0.45, 0.22, 0.1],
                        &[lifetime, lifetime * 0.6, lifetime * 0.4, lifetime * 0.3],
                    ),
                    motor: None,
                },
                envelope(0.002, lifetime, 0.0, 0.35),
                0.5,
            )
        }
        4..=5 => {
            let lifetime = 2.8 * sustain_scale;
            (
                Timbre::Harmonic {
                    partials: decaying_partials(
                        &[1.0, 3.53, 7.21],
                        &[1.0, 0.3, 0.09],
                        &[lifetime, lifetime * 0.22, lifetime * 0.09],
                    ),
                    motor: None,
                },
                envelope(0.002, lifetime, 0.0, 0.3),
                0.46,
            )
        }
        6..=7 => plucked_timbre(frequency, 1.3 * sustain_scale, 0.72, 1.0, 0.12, 0.42),
        8..=10 => {
            let (ratios, amplitudes, lifetimes): ([f32; 2], [f32; 2], [f32; 2]) = match program {
                8 => ([1.0, 4.04], [1.0, 0.2], [0.8, 0.3]),
                9 => ([1.0, 2.71], [1.0, 0.25], [1.1, 0.4]),
                _ => ([1.0, 3.41], [1.0, 0.22], [0.6, 0.2]),
            };
            let lifetime = lifetimes[0] * sustain_scale.min(1.2);
            (
                Timbre::Harmonic {
                    partials: decaying_partials(&ratios, &amplitudes, &[lifetime, lifetimes[1]]),
                    motor: None,
                },
                envelope(0.001, lifetime, 0.0, 0.3),
                0.52,
            )
        }
        11 => {
            let lifetime = 3.5 * sustain_scale.min(1.3);
            (
                Timbre::Harmonic {
                    partials: decaying_partials(
                        &[1.0, 3.98],
                        &[1.0, 0.18],
                        &[lifetime, lifetime * 0.35],
                    ),
                    motor: Some(Lfo {
                        rate_hz: 5.2,
                        phase: 0.0,
                    }),
                },
                envelope(0.004, lifetime, 0.0, 0.8),
                0.55,
            )
        }
        12 => {
            let lifetime = 0.42 * sustain_scale;
            (
                Timbre::Harmonic {
                    partials: decaying_partials(
                        &[1.0, 3.92, 9.1],
                        &[0.85, 0.3, 0.07],
                        &[lifetime, lifetime * 0.45, lifetime * 0.2],
                    ),
                    motor: None,
                },
                envelope(0.001, lifetime, 0.0, 0.25),
                0.58,
            )
        }
        13 => {
            let lifetime = 0.22 * sustain_scale;
            (
                Timbre::Harmonic {
                    partials: decaying_partials(
                        &[1.0, 3.36],
                        &[1.0, 0.2],
                        &[lifetime, lifetime * 0.45],
                    ),
                    motor: None,
                },
                envelope(0.001, lifetime, 0.0, 0.15),
                0.58,
            )
        }
        14 => {
            let lifetime = 4.0 * sustain_scale.min(1.3);
            (
                Timbre::Harmonic {
                    partials: decaying_partials(
                        &[1.0, 2.76, 5.4, 8.93],
                        &[0.6, 0.4, 0.25, 0.14],
                        &[lifetime, lifetime * 0.75, lifetime * 0.5, lifetime * 0.35],
                    ),
                    motor: None,
                },
                envelope(0.003, lifetime, 0.0, 0.9),
                0.5,
            )
        }
        15 => {
            let lifetime = 1.2 * sustain_scale;
            (
                Timbre::Harmonic {
                    partials: decaying_partials(
                        &[1.0, 2.0, 3.01],
                        &[1.0, 0.4, 0.2],
                        &[lifetime, lifetime * 0.5, lifetime * 0.25],
                    ),
                    motor: None,
                },
                envelope(0.001, lifetime, 0.0, 0.5),
                0.52,
            )
        }
        16..=23 => (
            Timbre::Ensemble {
                shape: Shape::Sine,
                detune_factors: [0.998, 1.0, 1.002],
                phases: [0.0; ENSEMBLE_OSCILLATORS],
                lowpass_alpha: 0.5,
                lowpass_target_alpha: 0.5,
                lowpass_state: 0.0,
                vibrato: Some(Lfo {
                    rate_hz: 6.0,
                    phase: 0.0,
                }),
                tremolo_rate: None,
            },
            envelope(0.012, 1.0, 1.0, 0.08),
            0.4,
        ),
        24..=31 => {
            let (lifetime, blend, drive, trim, release): (f32, f32, f32, f32, f32) = match program {
                24 => (2.2, 0.68, 1.0, 0.5, 0.12),
                25 => (2.6, 0.78, 1.0, 0.5, 0.12),
                26 => (1.6, 0.6, 1.0, 0.48, 0.1),
                27 => (2.4, 0.74, 1.0, 0.5, 0.12),
                28 => (0.45, 0.8, 1.0, 0.5, 0.06),
                29 => (3.5, 0.82, 2.5, 0.62, 0.2),
                30 => (4.0, 0.88, 4.0, 0.55, 0.2),
                _ => (0.8, 0.85, 1.0, 0.45, 0.15),
            };
            plucked_timbre(
                frequency,
                lifetime * sustain_scale,
                blend,
                drive,
                release,
                trim,
            )
        }
        32..=39 => {
            let (lifetime, blend, trim): (f32, f32, f32) = match program {
                32 => (3.0, 0.55, 0.54),
                33 => (3.2, 0.6, 0.54),
                34 => (2.8, 0.72, 0.54),
                35 => (3.0, 0.5, 0.5),
                36 | 37 => (1.8, 0.7, 0.52),
                _ => (4.0, 0.66, 0.48),
            };
            plucked_timbre(frequency, lifetime * sustain_scale, blend, 1.0, 0.1, trim)
        }
        40..=43 | 48..=55 => {
            let slow = program >= 48;
            (
                Timbre::Ensemble {
                    shape: Shape::Saw,
                    detune_factors: detune_factors(14.0),
                    phases: [0.0; ENSEMBLE_OSCILLATORS],
                    lowpass_alpha: lowpass_alpha(900.0),
                    lowpass_target_alpha: lowpass_alpha(3_400.0),
                    lowpass_state: 0.0,
                    vibrato: None,
                    tremolo_rate: None,
                },
                envelope(if slow { 0.2 } else { 0.09 }, 1.0, 1.0, 0.22),
                if slow { 0.3 } else { 0.34 },
            )
        }
        44 => (
            Timbre::Ensemble {
                shape: Shape::Saw,
                detune_factors: detune_factors(14.0),
                phases: [0.0; ENSEMBLE_OSCILLATORS],
                lowpass_alpha: lowpass_alpha(900.0),
                lowpass_target_alpha: lowpass_alpha(3_200.0),
                lowpass_state: 0.0,
                vibrato: None,
                tremolo_rate: Some(5.5),
            },
            envelope(0.09, 1.0, 1.0, 0.22),
            0.36,
        ),
        45 => plucked_timbre(frequency, 0.5 * sustain_scale, 0.8, 1.0, 0.2, 0.55),
        46 => plucked_timbre(frequency, 2.8 * sustain_scale, 0.7, 1.0, 0.3, 0.5),
        47 => {
            let lifetime = 1.1 * sustain_scale;
            (
                Timbre::Harmonic {
                    partials: decaying_partials(
                        &[1.0, 1.48, 2.19],
                        &[1.0, 0.5, 0.28],
                        &[lifetime, lifetime * 0.6, lifetime * 0.4],
                    ),
                    motor: None,
                },
                envelope(0.004, lifetime, 0.0, 0.35),
                0.6,
            )
        }
        56..=63 => (
            Timbre::Ensemble {
                shape: Shape::Saw,
                detune_factors: detune_factors(8.0),
                phases: [0.0; ENSEMBLE_OSCILLATORS],
                lowpass_alpha: lowpass_alpha(700.0),
                lowpass_target_alpha: lowpass_alpha(4_200.0),
                lowpass_state: 0.0,
                vibrato: Some(Lfo {
                    rate_hz: 5.0,
                    phase: 0.0,
                }),
                tremolo_rate: None,
            },
            envelope(0.055, 0.985, 0.72, 0.11),
            0.4,
        ),
        64..=69 => (
            Timbre::Ensemble {
                shape: Shape::Pulse(0.3),
                detune_factors: detune_factors(6.0),
                phases: [0.0; ENSEMBLE_OSCILLATORS],
                lowpass_alpha: lowpass_alpha(2_800.0),
                lowpass_target_alpha: lowpass_alpha(2_800.0),
                lowpass_state: 0.0,
                vibrato: Some(Lfo {
                    rate_hz: 5.2,
                    phase: 0.0,
                }),
                tremolo_rate: None,
            },
            envelope(0.025, 1.0, 1.0, 0.12),
            0.4,
        ),
        70..=79 => (
            Timbre::Ensemble {
                shape: Shape::Pulse(0.5),
                detune_factors: detune_factors(4.0),
                phases: [0.0; ENSEMBLE_OSCILLATORS],
                lowpass_alpha: lowpass_alpha(1_800.0),
                lowpass_target_alpha: lowpass_alpha(1_800.0),
                lowpass_state: 0.0,
                vibrato: Some(Lfo {
                    rate_hz: 4.6,
                    phase: 0.0,
                }),
                tremolo_rate: None,
            },
            envelope(0.035, 1.0, 1.0, 0.16),
            0.4,
        ),
        80..=87 => (
            Timbre::Ensemble {
                shape: Shape::Saw,
                detune_factors: detune_factors(10.0),
                phases: [0.0; ENSEMBLE_OSCILLATORS],
                lowpass_alpha: lowpass_alpha(1_200.0),
                lowpass_target_alpha: lowpass_alpha(5_000.0),
                lowpass_state: 0.0,
                vibrato: None,
                tremolo_rate: None,
            },
            envelope(0.015, 1.0, 1.0, 0.12),
            0.38,
        ),
        88..=95 => (
            Timbre::Ensemble {
                shape: Shape::Saw,
                detune_factors: detune_factors(18.0),
                phases: [0.0; ENSEMBLE_OSCILLATORS],
                lowpass_alpha: lowpass_alpha(600.0),
                lowpass_target_alpha: lowpass_alpha(2_200.0),
                lowpass_state: 0.0,
                vibrato: None,
                tremolo_rate: None,
            },
            envelope(0.28, 1.0, 1.0, 0.42),
            0.3,
        ),
        96..=103 => (
            Timbre::Ensemble {
                shape: Shape::Saw,
                detune_factors: detune_factors(16.0),
                phases: [0.0; ENSEMBLE_OSCILLATORS],
                lowpass_alpha: lowpass_alpha(450.0),
                lowpass_target_alpha: lowpass_alpha(1_400.0),
                lowpass_state: 0.0,
                vibrato: None,
                tremolo_rate: None,
            },
            envelope(0.25, 1.0, 1.0, 0.4),
            0.3,
        ),
        104..=111 => (
            Timbre::Ensemble {
                shape: Shape::Pulse(0.25),
                detune_factors: detune_factors(12.0),
                phases: [0.0; ENSEMBLE_OSCILLATORS],
                lowpass_alpha: lowpass_alpha(1_000.0),
                lowpass_target_alpha: lowpass_alpha(3_000.0),
                lowpass_state: 0.0,
                vibrato: Some(Lfo {
                    rate_hz: 0.8,
                    phase: 0.25,
                }),
                tremolo_rate: None,
            },
            envelope(0.05, 1.0, 1.0, 0.2),
            0.34,
        ),
        112..=118 => plucked_timbre(frequency, 1.8 * sustain_scale, 0.75, 1.0, 0.15, 0.48),
        119 => (
            Timbre::Ensemble {
                shape: Shape::Pulse(0.2),
                detune_factors: [1.0, 1.0, 1.0],
                phases: [0.0; ENSEMBLE_OSCILLATORS],
                lowpass_alpha: lowpass_alpha(1_900.0),
                lowpass_target_alpha: lowpass_alpha(1_900.0),
                lowpass_state: 0.0,
                vibrato: Some(Lfo {
                    rate_hz: 6.2,
                    phase: 0.0,
                }),
                tremolo_rate: None,
            },
            envelope(0.03, 1.0, 1.0, 0.14),
            0.38,
        ),
        120..=124 => {
            let lifetime = 0.9 * sustain_scale;
            (
                Timbre::Harmonic {
                    partials: decaying_partials(
                        &[1.0, 2.76, 5.4],
                        &[1.0, 0.35, 0.15],
                        &[lifetime, lifetime * 0.5, lifetime * 0.3],
                    ),
                    motor: None,
                },
                envelope(0.002, lifetime, 0.0, 0.3),
                0.5,
            )
        }
        _ => {
            let lifetime = 0.6 * sustain_scale;
            (
                Timbre::Harmonic {
                    partials: decaying_partials(
                        &[1.0, 2.0],
                        &[1.0, 0.3],
                        &[lifetime, lifetime * 0.5],
                    ),
                    motor: None,
                },
                envelope(0.002, lifetime, 0.0, 0.2),
                0.5,
            )
        }
    };

    Voice {
        channel: channel_index,
        key,
        velocity_gain,
        gain_trim,
        age_frames: 0,
        release_frame: None,
        envelope,
        timbre,
        phase: 0.0,
        secondary_phase: 0.0,
        noise: 0x9e37_79b9 ^ (u32::from(channel_index) << 8) ^ u32::from(key),
        filtered_noise: 0.0,
    }
}

fn envelope(
    attack_seconds: f32,
    decay_seconds: f32,
    sustain_level: f32,
    release_seconds: f32,
) -> Envelope {
    Envelope {
        attack_frames: (attack_seconds * AUDIO_SAMPLE_RATE as f32).max(1.0) as usize,
        decay_factor: if decay_seconds.is_infinite() || decay_seconds <= 0.0 {
            1.0
        } else {
            (-1.0 / (AUDIO_SAMPLE_RATE as f32 * decay_seconds)).exp()
        },
        sustain_level,
        release_frames: (release_seconds * AUDIO_SAMPLE_RATE as f32).max(1.0) as usize,
        level: 0.0,
    }
}

fn decaying_partials(ratios: &[f32], amplitudes: &[f32], lifetimes: &[f32]) -> [Partial; 4] {
    std::array::from_fn(|index| {
        if index < ratios.len().min(amplitudes.len()) {
            let lifetime = lifetimes[index.min(lifetimes.len() - 1)];
            Partial {
                ratio: ratios[index],
                phase: 0.0,
                amplitude: amplitudes[index],
                decay_factor: (-1.0 / (AUDIO_SAMPLE_RATE as f32 * lifetime.max(0.01))).exp(),
            }
        } else {
            Partial {
                ratio: 1.0,
                phase: 0.0,
                amplitude: 0.0,
                decay_factor: 0.0,
            }
        }
    })
}

fn detune_factors(cents: f32) -> [f32; ENSEMBLE_OSCILLATORS] {
    [
        (cents / 1_200.0).exp2().recip(),
        1.0,
        (cents / 1_200.0).exp2(),
    ]
}

fn lowpass_alpha(cutoff_hz: f32) -> f32 {
    1.0 - (-TAU * cutoff_hz / AUDIO_SAMPLE_RATE as f32).exp()
}

/// Builds a Karplus-Strong plucked-string voice whose decay time constant is
/// `lifetime` seconds, with optional waveshaping drive for overdriven guitars.
fn plucked_timbre(
    frequency: f32,
    lifetime: f32,
    excitation_blend: f32,
    drive: f32,
    release_seconds: f32,
    trim: f32,
) -> (Timbre, Envelope, f32) {
    let clamped_frequency = frequency.clamp(27.0, 4_186.0);
    let period = (AUDIO_SAMPLE_RATE as f32 / clamped_frequency)
        .round()
        .max(2.0);
    let damping = (-period / AUDIO_SAMPLE_RATE as f32 / lifetime.max(0.05))
        .exp()
        .min(0.999_999_5);
    let mut delay = vec![0.0_f32; period as usize];
    // Seeded excitation burst shaped by a pick-position comb filter.
    let mut seed = 0x2545_F491_4F6C_DD1Du64 ^ delay.len() as u64;
    let mut mean = 0.0;
    for slot in delay.iter_mut() {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        *slot = (seed >> 32) as f32 / u32::MAX as f32 - 0.5;
        mean += *slot;
    }
    mean /= delay.len() as f32;
    let mut previous = 0.0;
    for slot in delay.iter_mut() {
        *slot -= mean;
        *slot += excitation_blend * previous;
        previous = *slot;
    }
    (
        Timbre::Plucked {
            delay,
            cursor: 0,
            damping,
            drive,
        },
        // The envelope mirrors the string decay so voice_finished fires naturally.
        envelope(0.002, lifetime, 0.0, release_seconds),
        trim,
    )
}

fn percussion_chokes(trigger: u8, sounding: u8) -> bool {
    trigger == sounding || (matches!(trigger, 42 | 44 | 46) && matches!(sounding, 42 | 44 | 46))
}

fn percussion_mix_gain(key: u8) -> f32 {
    match key {
        35 | 36 | 38 | 40 | 41 | 43 | 45 | 47 | 48 | 50 => 1.0,
        _ => 0.5,
    }
}

#[allow(clippy::too_many_lines)]
fn render_midi_frames(
    output: &mut Vec<i16>,
    frames: usize,
    voices: &mut Vec<Voice>,
    channels: &[MidiChannel; 16],
    mut reverb: Option<&mut ReverbRack>,
    mut chorus: Option<&mut ChorusRack>,
) -> Result<()> {
    let current_frames = output.len() / AUDIO_CHANNELS;
    let allowed = MAX_AUDIO_FRAMES.saturating_sub(current_frames);
    if frames > allowed {
        return Err(Error::ResourceLimit(format!(
            "decoded audio exceeds the {MAX_AUDIO_SECONDS} second limit"
        )));
    }
    output.reserve(frames.saturating_mul(AUDIO_CHANNELS));
    let mut reverb_input = 0.0_f32;
    let mut chorus_input = 0.0_f32;
    for _ in 0..frames {
        let mut left = 0.0_f32;
        let mut right = 0.0_f32;
        for voice in voices.iter_mut() {
            if voice_finished(voice) {
                continue;
            }
            let channel = &channels[usize::from(voice.channel)];
            voice.envelope.advance(voice.age_frames);
            let sample = if matches!(voice.timbre, Timbre::Drum) {
                percussion_sample(voice)
            } else {
                match &mut voice.timbre {
                    Timbre::Drum => unreachable!("handled above"),
                    Timbre::Plucked {
                        delay,
                        cursor,
                        damping,
                        drive,
                    } => {
                        let period = delay.len();
                        let out = delay[*cursor];
                        let next = delay[(*cursor + 1) % period];
                        delay[*cursor] = (out + next) * 0.5 * *damping;
                        *cursor = (*cursor + 1) % period;
                        // Soft-clip saturation; dividing by the drive keeps the
                        // small-signal gain neutral so quiet passages stay quiet.
                        if *drive > 1.0 {
                            (out * *drive).tanh() / *drive
                        } else {
                            out
                        }
                    }
                    Timbre::Harmonic { partials, motor } => {
                        let frequency = channel.frequency(voice.key, 0.0);
                        let mut sample = 0.0;
                        for partial in partials.iter_mut() {
                            partial.phase = (partial.phase
                                + frequency * partial.ratio / AUDIO_SAMPLE_RATE as f32)
                                .fract();
                            sample += (partial.phase * TAU).sin() * partial.amplitude;
                            partial.amplitude *= partial.decay_factor;
                        }
                        if let Some(motor) = motor {
                            sample *= 0.82 + 0.18 * motor.sine();
                        }
                        sample
                    }
                    Timbre::Ensemble {
                        shape,
                        detune_factors,
                        phases,
                        lowpass_alpha,
                        lowpass_target_alpha,
                        lowpass_state,
                        vibrato,
                        tremolo_rate,
                    } => {
                        let vibrato_cents = vibrato
                            .as_mut()
                            .map(|lfo| 14.0 * lfo.sine())
                            .unwrap_or_default();
                        let frequency = channel.frequency(voice.key, vibrato_cents);
                        // The filter opens over the attack to mimic a real embouchure/bow.
                        if lowpass_alpha < lowpass_target_alpha {
                            *lowpass_alpha = (*lowpass_alpha + *lowpass_target_alpha * 0.000_2)
                                .min(*lowpass_target_alpha);
                        }
                        let mut sample = 0.0;
                        for (index, phase) in phases.iter_mut().enumerate() {
                            let oscillator_frequency = frequency * detune_factors[index];
                            let increment =
                                (oscillator_frequency / AUDIO_SAMPLE_RATE as f32).min(0.5);
                            *phase = (*phase + increment).fract();
                            sample += shape_sample(*shape, *phase, increment);
                        }
                        sample /= ENSEMBLE_OSCILLATORS as f32;
                        *lowpass_state += *lowpass_alpha * (sample - *lowpass_state);
                        let mut sample = *lowpass_state;
                        if let Some(rate) = *tremolo_rate {
                            let depth = 0.45;
                            sample *= 1.0 - depth
                                + depth
                                    * 0.5
                                    * ((voice.age_frames as f32 * rate * TAU
                                        / AUDIO_SAMPLE_RATE as f32)
                                        .sin());
                        }
                        sample
                            * (1.0 - channel.tremolo_depth
                                + channel.tremolo_depth
                                    * 0.5
                                    * ((voice.age_frames as f32 * 5.0 * TAU
                                        / AUDIO_SAMPLE_RATE as f32)
                                        .sin()))
                    }
                }
            };
            let dry_gain = voice.velocity_gain
                * voice.gain_trim
                * channel.volume
                * channel.expression
                * voice.envelope.level
                * release_envelope(voice, voice.envelope.release_frames);
            let pan_left = (1.0 - channel.pan).sqrt();
            let pan_right = channel.pan.sqrt();
            left += sample * dry_gain * pan_left;
            right += sample * dry_gain * pan_right;
            reverb_input += sample * dry_gain * channel.reverb_send;
            chorus_input += sample * dry_gain * channel.chorus_send;
            voice.age_frames = voice.age_frames.saturating_add(1);
        }
        if let Some(rack) = reverb.as_deref_mut() {
            let wet = rack.process(reverb_input) * 0.32;
            left += wet;
            right += wet;
        }
        if let Some(rack) = chorus.as_deref_mut() {
            let (wet_left, wet_right) = rack.process(chorus_input);
            left += wet_left * 0.4;
            right += wet_right * 0.4;
        }
        reverb_input = 0.0;
        chorus_input = 0.0;
        output.push(float_to_i16(left * MIDI_MASTER_GAIN));
        output.push(float_to_i16(right * MIDI_MASTER_GAIN));
    }
    voices.retain(|voice| !voice_finished(voice));
    Ok(())
}

fn voice_finished(voice: &Voice) -> bool {
    if matches!(voice.timbre, Timbre::Drum) {
        voice.age_frames >= percussion_duration_frames(voice.key)
            || release_finished(voice, MIDI_PERCUSSION_RELEASE_FRAMES)
    } else if let Some(release_frame) = voice.release_frame {
        voice.age_frames.saturating_sub(release_frame) >= voice.envelope.release_frames
    } else {
        // Naturally decaying voices (plucks, mallets) die out on their own.
        voice.envelope.sustain_level == 0.0
            && voice.envelope.decay_factor < 1.0
            && voice.age_frames > voice.envelope.attack_frames
            && voice.envelope.level <= MIDI_SILENCE_LEVEL
    }
}

fn release_finished(voice: &Voice, frames: usize) -> bool {
    voice
        .release_frame
        .is_some_and(|release_frame| voice.age_frames.saturating_sub(release_frame) >= frames)
}

fn release_envelope(voice: &Voice, frames: usize) -> f32 {
    voice.release_frame.map_or(1.0, |release_frame| {
        let elapsed = voice.age_frames.saturating_sub(release_frame);
        1.0 - elapsed.min(frames) as f32 / frames as f32
    })
}

#[allow(clippy::too_many_lines)]
fn percussion_sample(voice: &mut Voice) -> f32 {
    let duration = percussion_duration_frames(voice.key);
    let progress = voice.age_frames as f32 / duration as f32;
    let envelope =
        (1.0 - progress).max(0.0).powi(2) * release_envelope(voice, MIDI_PERCUSSION_RELEASE_FRAMES);
    let time = voice.age_frames as f32 / AUDIO_SAMPLE_RATE as f32;

    voice.noise ^= voice.noise << 13;
    voice.noise ^= voice.noise >> 17;
    voice.noise ^= voice.noise << 5;
    let white = voice.noise as i32 as f32 / i32::MAX as f32;
    voice.filtered_noise = voice.filtered_noise * 0.82 + white * 0.18;
    let bright_noise = white - voice.filtered_noise;
    let body_noise = white * 0.35 + voice.filtered_noise * 0.65;

    let sample = match voice.key {
        35 | 36 => {
            let frequency = 95.0 - progress * 50.0;
            voice.phase = (voice.phase + frequency / AUDIO_SAMPLE_RATE as f32).fract();
            (voice.phase * TAU).sin() * 0.9 + voice.filtered_noise * 0.1
        }
        37 => {
            // Side stick: a short woody blip over damped noise.
            let blip = (-time * 900.0 * TAU).exp() * (time * 380.0 * TAU).sin();
            blip * 0.6 + bright_noise * 0.25
        }
        38 | 40 => body_noise * 0.65 + (time * 180.0 * TAU).sin() * 0.35,
        41 | 43 | 45 | 47 | 48 | 50 => {
            let frequency = 210.0 * 2.0_f32.powf((f32::from(voice.key) - 47.0) / 12.0);
            voice.phase = (voice.phase + frequency / AUDIO_SAMPLE_RATE as f32).fract();
            (voice.phase * TAU).sin() * 0.75 + white * 0.25
        }
        42 | 44 | 46 => {
            // Metallic hat partials: inharmonic square stack under the bright noise.
            voice.phase = (voice.phase + 2_630.0 / AUDIO_SAMPLE_RATE as f32).fract();
            voice.secondary_phase =
                (voice.secondary_phase + 4_210.0 / AUDIO_SAMPLE_RATE as f32).fract();
            let metal = square_wave(voice.phase) * 0.5 + square_wave(voice.secondary_phase) * 0.5;
            bright_noise * 0.4 + metal * 0.18
        }
        49 | 51 | 52 | 55 | 57 | 59 => {
            voice.phase = (voice.phase + 1_180.0 / AUDIO_SAMPLE_RATE as f32).fract();
            voice.secondary_phase =
                (voice.secondary_phase + 2_370.0 / AUDIO_SAMPLE_RATE as f32).fract();
            let ping = if voice.key == 51 {
                (voice.phase * TAU).sin() * (-time * 4.5).exp() * 0.35
            } else {
                0.0
            };
            bright_noise * 0.4
                + (square_wave(voice.phase) * 0.5 + square_wave(voice.secondary_phase) * 0.5) * 0.14
                + ping
        }
        56 => {
            // Cowbell: two detuned square tones through a fast decay.
            voice.phase = (voice.phase + 560.0 / AUDIO_SAMPLE_RATE as f32).fract();
            voice.secondary_phase =
                (voice.secondary_phase + 845.0 / AUDIO_SAMPLE_RATE as f32).fract();
            (square_wave(voice.phase) + square_wave(voice.secondary_phase))
                * 0.35
                * (-time * 26.0).exp()
        }
        _ => body_noise * 0.7,
    };
    sample * envelope
}

fn square_wave(phase: f32) -> f32 {
    if phase < 0.5 { 0.7 } else { -0.7 }
}

fn percussion_duration_frames(key: u8) -> usize {
    let milliseconds = match key {
        35 | 36 => 260,
        37 => 80,
        38 | 40 => 220,
        42 | 44 => 80,
        46 => 360,
        49 | 51 | 52 | 55 | 57 | 59 => 900,
        56 => 160,
        41 | 43 | 45 | 47 | 48 | 50 => 300,
        _ => 180,
    };
    AUDIO_SAMPLE_RATE as usize * milliseconds / 1_000
}

/// Four-damped-comb, two-allpass reverberator fed by per-channel CC91 sends.
struct ReverbRack {
    combs: [(Vec<f32>, usize, f32); 4],
    allpasses: [(Vec<f32>, usize); 2],
}

impl Default for ReverbRack {
    fn default() -> Self {
        Self {
            combs: [1557, 1617, 1491, 1422].map(|length| (vec![0.0; length], 0, 0.0)),
            allpasses: [(vec![0.0; 225], 0), (vec![0.0; 556], 0)],
        }
    }
}

const REVERB_FEEDBACK: f32 = 0.774;
const REVERB_DAMP: f32 = 0.186;
const ALLPASS_GAIN: f32 = 0.5;

impl ReverbRack {
    fn process(&mut self, input: f32) -> f32 {
        let mut output = 0.0;
        for (buffer, cursor, filter_store) in self.combs.iter_mut() {
            let delayed = buffer[*cursor];
            *filter_store = REVERB_DAMP * input + (1.0 - REVERB_DAMP) * *filter_store;
            buffer[*cursor] = delayed * REVERB_FEEDBACK + *filter_store;
            *cursor = (*cursor + 1) % buffer.len();
            output += delayed;
        }
        output *= 0.25;
        for (buffer, cursor) in self.allpasses.iter_mut() {
            let buffered = buffer[*cursor];
            let allpassed = -output + buffered;
            buffer[*cursor] = output + buffered * ALLPASS_GAIN;
            *cursor = (*cursor + 1) % buffer.len();
            output = allpassed;
        }
        output
    }
}

/// Dual-tap modulated-delay chorus fed by per-channel CC93 sends.
struct ChorusRack {
    buffer: Vec<f32>,
    cursor: usize,
    first_lfo: f32,
    second_lfo: f32,
}

impl Default for ChorusRack {
    fn default() -> Self {
        Self {
            buffer: vec![0.0; 4_096],
            cursor: 0,
            first_lfo: 0.0,
            second_lfo: 0.31,
        }
    }
}

const CHORUS_BASE_DELAY: f32 = 480.0;
const CHORUS_DEPTH: f32 = 110.0;
const CHORUS_FIRST_RATE: f32 = 0.62;
const CHORUS_SECOND_RATE: f32 = 0.83;

impl ChorusRack {
    fn process(&mut self, input: f32) -> (f32, f32) {
        self.buffer[self.cursor] = input;
        self.cursor = (self.cursor + 1) % self.buffer.len();
        self.first_lfo = (self.first_lfo + CHORUS_FIRST_RATE / AUDIO_SAMPLE_RATE as f32).fract();
        self.second_lfo = (self.second_lfo + CHORUS_SECOND_RATE / AUDIO_SAMPLE_RATE as f32).fract();

        let read = |offset: f32| -> f32 {
            let position =
                self.cursor as f32 - CHORUS_BASE_DELAY - CHORUS_DEPTH * (0.5 + 0.5 * offset);
            let wrapped = position.rem_euclid(self.buffer.len() as f32);
            let index = wrapped.floor() as usize;
            let fraction = wrapped - index as f32;
            let next = (index + 1) % self.buffer.len();
            self.buffer[index] * (1.0 - fraction) + self.buffer[next] * fraction
        };
        (
            read(self.first_lfo * TAU).sin(),
            read(self.second_lfo * TAU).cos(),
        )
    }
}

/// Naive sawtooth with a polynomial band-limiting correction at the wrap edge.
fn bandlimited_saw(phase: f32, increment: f32) -> f32 {
    let naive = 2.0 * phase - 1.0;
    let correction = if increment > 0.0 && phase < increment {
        let t = phase / increment;
        t + t - t * t - 1.0
    } else if increment > 0.0 && phase > 1.0 - increment {
        let t = (phase - 1.0) / increment;
        t * t + t + t + 1.0
    } else {
        0.0
    };
    naive - correction
}

fn shape_sample(shape: Shape, phase: f32, increment: f32) -> f32 {
    match shape {
        Shape::Sine => (phase * TAU).sin(),
        Shape::Saw => bandlimited_saw(phase, increment),
        Shape::Pulse(width) => {
            0.5 * (bandlimited_saw(phase, increment)
                - bandlimited_saw((phase + width).fract(), increment))
        }
    }
}

fn float_to_i16(sample: f32) -> i16 {
    (sample.clamp(-1.0, 1.0) * f32::from(i16::MAX)).round() as i16
}

fn decode_wav(data: &[u8]) -> Result<Vec<i16>> {
    if data.len() < 12 || &data[..4] != b"RIFF" || &data[8..12] != b"WAVE" {
        return Err(Error::Platform("invalid WAV container".into()));
    }
    let riff_len = u32::from_le_bytes(data[4..8].try_into().unwrap()) as usize;
    let riff_end = 8_usize
        .checked_add(riff_len)
        .filter(|end| *end <= data.len())
        .ok_or_else(|| Error::Platform("truncated WAV container".into()))?;

    let mut format = None;
    let mut encoded_samples = None;
    let mut cursor = 12_usize;
    while cursor < riff_end {
        let header_end = cursor
            .checked_add(8)
            .filter(|end| *end <= riff_end)
            .ok_or_else(|| Error::Platform("truncated WAV chunk header".into()))?;
        let chunk_len =
            u32::from_le_bytes(data[cursor + 4..header_end].try_into().unwrap()) as usize;
        let chunk_start = header_end;
        let chunk_end = chunk_start
            .checked_add(chunk_len)
            .filter(|end| *end <= riff_end)
            .ok_or_else(|| Error::Platform("truncated WAV chunk".into()))?;
        match &data[cursor..cursor + 4] {
            b"fmt " if format.is_none() => {
                if chunk_len < 16 {
                    return Err(Error::Platform("WAV fmt chunk is too short".into()));
                }
                format = Some((
                    u16::from_le_bytes(data[chunk_start..chunk_start + 2].try_into().unwrap()),
                    u16::from_le_bytes(data[chunk_start + 2..chunk_start + 4].try_into().unwrap()),
                    u32::from_le_bytes(data[chunk_start + 4..chunk_start + 8].try_into().unwrap()),
                    u16::from_le_bytes(
                        data[chunk_start + 12..chunk_start + 14].try_into().unwrap(),
                    ),
                    u16::from_le_bytes(
                        data[chunk_start + 14..chunk_start + 16].try_into().unwrap(),
                    ),
                ));
            }
            b"data" if encoded_samples.is_none() => {
                encoded_samples = Some(&data[chunk_start..chunk_end]);
            }
            _ => {}
        }
        cursor = chunk_end
            .checked_add(chunk_len & 1)
            .filter(|next| *next <= riff_end)
            .ok_or_else(|| Error::Platform("truncated WAV chunk padding".into()))?;
    }

    let (encoding, channels, sample_rate, block_align, bits_per_sample) =
        format.ok_or_else(|| Error::Platform("WAV has no fmt chunk".into()))?;
    if encoding != 1 {
        return Err(Error::Platform(format!(
            "unsupported WAV encoding {encoding}"
        )));
    }
    let bytes_per_sample = match bits_per_sample {
        8 => 1_usize,
        16 => 2,
        24 => 3,
        32 => 4,
        _ => {
            return Err(Error::Platform(format!(
                "unsupported WAV sample width {bits_per_sample}"
            )));
        }
    };
    let channels = usize::from(channels);
    let expected_block_align = channels
        .checked_mul(bytes_per_sample)
        .filter(|align| channels != 0 && *align == usize::from(block_align))
        .ok_or_else(|| Error::Platform("WAV has invalid channel alignment".into()))?;
    if sample_rate == 0 {
        return Err(Error::Platform("WAV has an invalid sample rate".into()));
    }
    let encoded_samples =
        encoded_samples.ok_or_else(|| Error::Platform("WAV has no data chunk".into()))?;
    if encoded_samples.is_empty() || encoded_samples.len() % expected_block_align != 0 {
        return Err(Error::Platform(
            "WAV sample data is empty or misaligned".into(),
        ));
    }
    let sample_count = encoded_samples.len() / bytes_per_sample;
    if sample_count > MAX_SOURCE_SAMPLES {
        return Err(Error::ResourceLimit(
            "decoded WAV working set exceeds 128 MiB".into(),
        ));
    }

    let mut samples = Vec::with_capacity(sample_count);
    for encoded in encoded_samples.chunks_exact(bytes_per_sample) {
        let sample = match bits_per_sample {
            8 => (i16::from(encoded[0]) - 128) << 8,
            16 => i16::from_le_bytes(encoded.try_into().unwrap()),
            24 => {
                let value = i32::from(encoded[0])
                    | (i32::from(encoded[1]) << 8)
                    | (i32::from(encoded[2]) << 16);
                ((value << 8) >> 16) as i16
            }
            32 => (i32::from_le_bytes(encoded.try_into().unwrap()) >> 16) as i16,
            _ => unreachable!(),
        };
        samples.push(sample);
    }
    resample_to_stereo(&samples, sample_rate, channels)
}

fn decode_mp3(data: &[u8]) -> Result<Vec<i16>> {
    let source = MediaSourceStream::new(Box::new(Cursor::new(data.to_vec())), Default::default());
    let mut hint = Hint::new();
    hint.with_extension("mp3");
    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            source,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .map_err(|error| Error::Platform(format!("invalid MP3 data: {error}")))?;
    let mut format = probed.format;
    let track = format
        .default_track()
        .ok_or_else(|| Error::Platform("MP3 stream has no audio track".into()))?;
    let track_id = track.id;
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|error| Error::Platform(format!("unsupported MP3 stream: {error}")))?;

    let mut source_samples = Vec::<i16>::new();
    let mut source_rate = None;
    let mut source_channels = None;
    loop {
        let packet = match format.next_packet() {
            Ok(packet) => packet,
            Err(SymphoniaError::IoError(error))
                if error.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                break;
            }
            Err(error) => {
                return Err(Error::Platform(format!(
                    "failed to read MP3 packet: {error}"
                )));
            }
        };
        if packet.track_id() != track_id {
            continue;
        }
        let decoded = match decoder.decode(&packet) {
            Ok(decoded) => decoded,
            Err(SymphoniaError::DecodeError(_)) => continue,
            Err(error) => {
                return Err(Error::Platform(format!(
                    "failed to decode MP3 packet: {error}"
                )));
            }
        };
        let spec = *decoded.spec();
        let channel_count = spec.channels.count();
        if channel_count == 0 || spec.rate == 0 {
            return Err(Error::Platform(
                "MP3 stream has invalid audio parameters".into(),
            ));
        }
        if source_rate.is_some_and(|rate| rate != spec.rate)
            || source_channels.is_some_and(|channels| channels != channel_count)
        {
            return Err(Error::Platform(
                "MP3 stream changes audio parameters during playback".into(),
            ));
        }
        source_rate = Some(spec.rate);
        source_channels = Some(channel_count);
        let mut converted = SampleBuffer::<i16>::new(decoded.capacity() as u64, spec);
        converted.copy_interleaved_ref(decoded);
        if source_samples
            .len()
            .checked_add(converted.samples().len())
            .is_none_or(|len| len > MAX_SOURCE_SAMPLES)
        {
            return Err(Error::ResourceLimit(
                "decoded MP3 working set exceeds 128 MiB".into(),
            ));
        }
        source_samples.extend_from_slice(converted.samples());
        let frames = source_samples.len() / channel_count;
        let projected_frames = frames
            .saturating_mul(AUDIO_SAMPLE_RATE as usize)
            .saturating_div(spec.rate as usize);
        if projected_frames > MAX_AUDIO_FRAMES {
            return Err(Error::ResourceLimit(format!(
                "decoded audio exceeds the {MAX_AUDIO_SECONDS} second limit"
            )));
        }
    }

    let source_rate = source_rate.ok_or_else(|| Error::Platform("MP3 contains no audio".into()))?;
    let source_channels = source_channels.expect("channels accompany the MP3 sample rate");
    resample_to_stereo(&source_samples, source_rate, source_channels)
}

fn resample_to_stereo(samples: &[i16], source_rate: u32, channels: usize) -> Result<Vec<i16>> {
    let source_frames = samples.len() / channels;
    if source_frames == 0 {
        return Ok(Vec::new());
    }
    let output_frames = source_frames
        .checked_mul(AUDIO_SAMPLE_RATE as usize)
        .ok_or_else(|| Error::ResourceLimit("resampled audio length overflows".into()))?
        .div_ceil(source_rate as usize);
    if output_frames > MAX_AUDIO_FRAMES {
        return Err(Error::ResourceLimit(format!(
            "decoded audio exceeds the {MAX_AUDIO_SECONDS} second limit"
        )));
    }

    let mut output = Vec::with_capacity(output_frames.saturating_mul(AUDIO_CHANNELS));
    for output_frame in 0..output_frames {
        let numerator = output_frame as u64 * u64::from(source_rate);
        let source_index = (numerator / u64::from(AUDIO_SAMPLE_RATE)) as usize;
        let next_index = (source_index + 1).min(source_frames - 1);
        let fraction = (numerator % u64::from(AUDIO_SAMPLE_RATE)) as f32 / AUDIO_SAMPLE_RATE as f32;
        let sample = |frame: usize, channel: usize| -> f32 {
            f32::from(samples[frame * channels + channel.min(channels - 1)])
        };
        let interpolate = |channel: usize| {
            sample(source_index.min(source_frames - 1), channel) * (1.0 - fraction)
                + sample(next_index, channel) * fraction
        };
        let left = interpolate(0);
        let right = if channels == 1 { left } else { interpolate(1) };
        output.push(left.round().clamp(f32::from(i16::MIN), f32::from(i16::MAX)) as i16);
        output.push(
            right
                .round()
                .clamp(f32::from(i16::MIN), f32::from(i16::MAX)) as i16,
        );
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIMPLE_MIDI: &[u8] = b"MThd\0\0\0\x06\0\0\0\x01\0\x60\
MTrk\0\0\0\x0c\0\x90\x45\x7f\x60\x80\x45\0\0\xff\x2f\0";

    fn pcm_stats(samples: &[i16]) -> (f64, i32, f64) {
        let rms = (samples
            .iter()
            .map(|sample| f64::from(*sample).powi(2))
            .sum::<f64>()
            / samples.len() as f64)
            .sqrt();
        let peak = samples
            .iter()
            .map(|sample| i32::from(*sample).abs())
            .max()
            .unwrap();
        // One-frame difference energy exposes sustained broadband noise without a reference PCM.
        let difference_rms = (samples[AUDIO_CHANNELS..]
            .iter()
            .zip(&samples[..samples.len() - AUDIO_CHANNELS])
            .map(|(sample, previous)| (f64::from(*sample) - f64::from(*previous)).powi(2))
            .sum::<f64>()
            / (samples.len() - AUDIO_CHANNELS) as f64)
            .sqrt();
        (rms, peak, difference_rms / rms)
    }

    fn test_drum_voice(key: u8) -> Voice {
        let mut voice = build_voice(&MidiChannel::default(), 9, key, 127);
        voice.age_frames = 0;
        voice
    }

    /// Renders the DOTA fixtures to /tmp WAV files for listening comparisons
    /// against real-device recordings. Run with:
    /// `cargo test -p skyengine-core --lib audio:: -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn render_dota_previews_for_listening() {
        let fixture =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test/fixtures/dota.mrp");
        let package = crate::Package::open(fixture, crate::ResourceLimits::default()).unwrap();
        for (name, out, seconds) in [
            (&b"music_title.mid"[..], "/tmp/preview_title.wav", 28.0),
            (&b"dkljngle.mid"[..], "/tmp/preview_gameplay.wav", 45.0),
        ] {
            let encoded = package.read_named(name).unwrap();
            let samples = decode_midi(&encoded).unwrap();
            let take =
                (seconds as usize * AUDIO_SAMPLE_RATE as usize * AUDIO_CHANNELS).min(samples.len());
            let mut wav = Vec::new();
            let data_len = (take * 2) as u32;
            wav.extend_from_slice(b"RIFF");
            wav.extend_from_slice(&(36 + data_len).to_le_bytes());
            wav.extend_from_slice(b"WAVEfmt ");
            wav.extend_from_slice(&16_u32.to_le_bytes());
            wav.extend_from_slice(&1_u16.to_le_bytes());
            wav.extend_from_slice(&2_u16.to_le_bytes());
            wav.extend_from_slice(&AUDIO_SAMPLE_RATE.to_le_bytes());
            wav.extend_from_slice(&(AUDIO_SAMPLE_RATE * 4).to_le_bytes());
            wav.extend_from_slice(&4_u16.to_le_bytes());
            wav.extend_from_slice(&16_u16.to_le_bytes());
            wav.extend_from_slice(b"data");
            wav.extend_from_slice(&data_len.to_le_bytes());
            for sample in &samples[..take] {
                wav.extend_from_slice(&sample.to_le_bytes());
            }
            std::fs::write(out, &wav).unwrap();
            println!("wrote {out} ({} frames)", take / AUDIO_CHANNELS);
        }
    }

    #[test]
    fn midi_decodes_to_non_silent_stereo_pcm() {
        let samples = decode_midi(SIMPLE_MIDI).unwrap();
        assert!(!samples.is_empty());
        assert_eq!(samples.len() % AUDIO_CHANNELS, 0);
        assert!(samples.iter().any(|sample| *sample != 0));
    }

    #[test]
    fn malformed_sound_font_is_rejected() {
        let error = AudioPlayer::with_sound_font(b"not an SF2 file")
            .err()
            .expect("malformed SoundFont should fail");
        assert!(error.to_string().contains("invalid SoundFont data"));
    }

    #[test]
    fn oversized_sound_font_file_is_rejected_before_parsing() {
        let path = std::env::temp_dir().join(format!(
            "skyengine-oversized-sound-font-{}.sf2",
            std::process::id()
        ));
        let file = File::create(&path).unwrap();
        file.set_len(MAX_SOUNDFONT_BYTES + 1).unwrap();
        drop(file);

        let error = AudioPlayer::with_sound_font_file(&path)
            .err()
            .expect("oversized SoundFont should fail");
        std::fs::remove_file(path).unwrap();
        assert!(matches!(error, Error::ResourceLimit(_)));
    }

    #[test]
    #[ignore]
    fn external_sound_font_renders_standard_and_packaged_midi() {
        let path = std::env::var_os("SKYENGINE_TEST_SOUNDFONT")
            .expect("set SKYENGINE_TEST_SOUNDFONT to an SF2 GM bank");
        let player = AudioPlayer::with_sound_font_file(path).unwrap();
        let fixture =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test/fixtures/dota.mrp");
        let package = crate::Package::open(fixture, crate::ResourceLimits::default()).unwrap();
        let mut inputs = vec![SIMPLE_MIDI.to_vec()];
        for name in [&b"music_title.mid"[..], &b"dkljngle.mid"[..]] {
            inputs.push(package.read_named(name).unwrap());
        }

        for input in inputs {
            player.play(SoundType::Midi, &input, false).unwrap();
            let mut output = vec![0; AUDIO_SAMPLE_RATE as usize * AUDIO_CHANNELS];
            assert!(player.render(&mut output) > 0);
            assert!(output.iter().any(|sample| *sample != 0));
            player.stop();
        }
    }

    #[test]
    fn percussion_without_note_off_is_a_finite_one_shot() {
        let mut samples = Vec::new();
        let mut voices = vec![test_drum_voice(49)];

        render_midi_frames(
            &mut samples,
            AUDIO_SAMPLE_RATE as usize * 2,
            &mut voices,
            &default_channels(),
            None,
            None,
        )
        .unwrap();

        let onset_len = AUDIO_SAMPLE_RATE as usize / 20 * AUDIO_CHANNELS;
        let silent_tail = AUDIO_SAMPLE_RATE as usize / 2 * AUDIO_CHANNELS;
        assert!(samples[..onset_len].iter().any(|sample| *sample != 0));
        let onset_peak = samples[..onset_len]
            .iter()
            .map(|sample| i32::from(*sample).abs())
            .max()
            .unwrap();
        assert!(
            (500..=4_000).contains(&onset_peak),
            "percussion onset peak {onset_peak} is outside the bounded mix range"
        );
        assert!(
            samples[samples.len() - silent_tail..]
                .iter()
                .all(|sample| *sample == 0)
        );
        assert!(voices.is_empty());
    }

    #[test]
    fn tonal_percussion_uses_the_restored_mix_level() {
        let mut samples = Vec::new();
        let mut voices = vec![test_drum_voice(36)];

        render_midi_frames(
            &mut samples,
            AUDIO_SAMPLE_RATE as usize / 20,
            &mut voices,
            &default_channels(),
            None,
            None,
        )
        .unwrap();

        let peak = samples
            .iter()
            .map(|sample| i32::from(*sample).abs())
            .max()
            .unwrap();
        assert!(
            (1_000..=8_000).contains(&peak),
            "tonal percussion peak {peak} is outside the restored mix range"
        );
    }

    #[test]
    fn percussion_note_off_releases_promptly() {
        let mut voice = test_drum_voice(49);
        voice.release_frame = Some(0);
        let mut voices = vec![voice];
        let mut samples = Vec::new();

        render_midi_frames(
            &mut samples,
            AUDIO_SAMPLE_RATE as usize / 10,
            &mut voices,
            &default_channels(),
            None,
            None,
        )
        .unwrap();

        assert!(
            samples[..MIDI_PERCUSSION_RELEASE_FRAMES * AUDIO_CHANNELS]
                .iter()
                .any(|sample| *sample != 0)
        );
        assert!(
            samples[MIDI_PERCUSSION_RELEASE_FRAMES * AUDIO_CHANNELS..]
                .iter()
                .all(|sample| *sample == 0)
        );
        assert!(voices.is_empty());
    }

    #[test]
    fn every_percussion_key_renders_an_audible_one_shot() {
        let channels = default_channels();
        for key in 21..=109_u8 {
            let mut voices = vec![build_voice(&channels[9], 9, key, 100)];
            let mut samples = Vec::new();
            render_midi_frames(
                &mut samples,
                AUDIO_SAMPLE_RATE as usize / 2,
                &mut voices,
                &channels,
                None,
                None,
            )
            .unwrap();
            let peak = samples
                .iter()
                .map(|sample| i32::from(*sample).abs())
                .max()
                .unwrap();
            assert!(
                peak > 200,
                "percussion key {key} renders nearly silent (peak {peak})"
            );
            assert!(
                samples
                    .iter()
                    .all(|sample| i32::from(*sample).abs() <= i32::from(i16::MAX)),
                "percussion key {key} overflows"
            );
        }
    }

    #[test]
    fn percussion_retrigger_chokes_the_previous_voice() {
        let mut voices = vec![test_drum_voice(42), test_drum_voice(46)];
        voices[0].age_frames = 100;
        voices[1].age_frames = 200;

        start_midi_voice(&default_channels(), &mut voices, 9, 42, 100);

        assert_eq!(voices.len(), 1);
        assert_eq!(voices[0].key, 42);
        assert_eq!(voices[0].age_frames, 0);
    }

    #[test]
    fn every_gm_program_renders_without_waveform_discontinuities() {
        let channels = default_channels();
        for program in 0..=u8::MAX {
            let channel = MidiChannel {
                program,
                ..MidiChannel::default()
            };
            let mut voices = vec![build_voice(&channel, 0, 69, 127)];
            let mut samples = Vec::new();
            render_midi_frames(&mut samples, 4_096, &mut voices, &channels, None, None).unwrap();
            // Skip the onset: pluck excitation bursts are legitimate transients.
            let steady = &samples[1_024 * AUDIO_CHANNELS..];
            let largest_jump = steady[AUDIO_CHANNELS..]
                .iter()
                .zip(steady.iter())
                .map(|(sample, previous)| i32::from(*sample - *previous).abs())
                .max()
                .unwrap();
            assert!(
                largest_jump <= 6_000,
                "program {program} jumps {largest_jump} between neighbouring samples"
            );
        }
    }

    #[test]
    fn rpn_pitch_bend_range_widens_the_bend_span() {
        // An unbent sustained organ note versus one bent upward under an
        // RPN-configured 12-semitone range: the span must approach an octave.
        let bend_up = |with_bend: bool| -> f64 {
            let mut track = Vec::new();
            if with_bend {
                track.extend_from_slice(&[0, 0xB0, 101, 0]);
                track.extend_from_slice(&[0, 0xB0, 100, 0]);
                track.extend_from_slice(&[0, 0xB0, 6, 12]);
                track.extend_from_slice(&[0, 0xE0, 0, 127]);
            }
            track.extend_from_slice(&[0, 0xC0, 16]);
            track.extend_from_slice(&[0, 0x90, 69, 100]);
            track.extend_from_slice(&[120, 0x80, 69, 0]);
            track.extend_from_slice(&[0, 0xFF, 0x2F, 0]);
            let mut data = b"MThd\0\0\0\x06\0\0\0\x01\0\x60MTrk".to_vec();
            data.extend_from_slice(&(track.len() as u32).to_be_bytes());
            data.extend_from_slice(&track);
            estimate_frequency(&decode_midi(&data).unwrap())
        };

        let unbent = bend_up(false);
        let bent_up = bend_up(true);
        assert!(
            (unbent > 0.0) && (bent_up / unbent - 2.0).abs() < 0.1,
            "bend range 12 did not raise the note by an octave ({unbent:.1} Hz -> {bent_up:.1} Hz)"
        );
    }

    /// Counts positive-going zero crossings over the middle half of the render.
    fn estimate_frequency(samples: &[i16]) -> f64 {
        let start = samples.len() / 4;
        let end = samples.len() * 3 / 4;
        let mut crossings = 0_usize;
        let mut previous = samples[start];
        for sample in &samples[start..end] {
            if *sample >= 0 && previous < 0 {
                crossings += 1;
            }
            previous = *sample;
        }
        // Samples are interleaved stereo: two samples per frame.
        let seconds = (end - start) as f64 / f64::from(AUDIO_SAMPLE_RATE * AUDIO_CHANNELS as u32);
        crossings as f64 / seconds
    }

    #[test]
    fn plucked_strings_decay_while_held() {
        let mut track = Vec::new();
        track.extend_from_slice(&[0, 0xC0, 24]);
        track.extend_from_slice(&[0, 0x90, 60, 100]);
        // Hold the key for 720 ticks while harmless ignored controllers flow.
        for _ in 0..12 {
            track.extend_from_slice(&[60, 0xB0, 64, 0]);
        }
        track.extend_from_slice(&[0, 0x80, 60, 0]);
        track.extend_from_slice(&[0, 0xFF, 0x2F, 0]);
        let mut data = b"MThd\0\0\0\x06\0\0\0\x01\0\x60MTrk".to_vec();
        data.extend_from_slice(&(track.len() as u32).to_be_bytes());
        data.extend_from_slice(&track);

        let samples = decode_midi(&data).unwrap();
        let window = AUDIO_SAMPLE_RATE as usize / 10 * AUDIO_CHANNELS;
        let rms = |range: &[i16]| -> f64 {
            (range.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>() / range.len() as f64).sqrt()
        };
        // Default tempo: one quarter (96 ticks) lasts 0.5 s.
        let note_off_frame = 720 * AUDIO_SAMPLE_RATE as usize / 96 / 2;
        let onset = rms(&samples[..window]);
        let late_window_start = (note_off_frame - AUDIO_SAMPLE_RATE as usize / 10) * AUDIO_CHANNELS;
        let late = rms(&samples[late_window_start..late_window_start + window]);
        assert!(
            onset > late * 2.0,
            "held nylon string did not decay naturally (onset {onset:.1}, late {late:.1})"
        );
    }

    #[test]
    fn reverb_send_leaves_an_audible_tail_after_the_last_note() {
        let mut track = Vec::new();
        track.extend_from_slice(&[0, 0xB0, 91, 127]);
        track.extend_from_slice(&[0, 0xC0, 40]);
        track.extend_from_slice(&[0, 0x90, 72, 110]);
        track.extend_from_slice(&[0x19, 0x80, 72, 0]);
        track.extend_from_slice(&[0, 0xFF, 0x2F, 0]);
        let mut data = b"MThd\0\0\0\x06\0\0\0\x01\0\x60MTrk".to_vec();
        data.extend_from_slice(&(track.len() as u32).to_be_bytes());
        data.extend_from_slice(&track);

        let samples = decode_midi(&data).unwrap();
        let note_end = 25 * AUDIO_SAMPLE_RATE as usize / 96 * AUDIO_CHANNELS;
        let probe_start = note_end + AUDIO_SAMPLE_RATE as usize / 10 * AUDIO_CHANNELS;
        assert!(
            samples.len() > probe_start,
            "render ended {} samples after the note instead of carrying a reverb tail",
            samples.len() - note_end
        );
        assert!(
            samples[note_end..probe_start]
                .iter()
                .any(|sample| sample.abs() > 16),
            "no audible reverb tail after the note ended"
        );
    }

    #[test]
    fn dota_title_midi_has_bounded_level_and_noise() {
        let fixture =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test/fixtures/dota.mrp");
        let package = crate::Package::open(fixture, crate::ResourceLimits::default()).unwrap();
        let encoded = package.read_named(b"music_title.mid").unwrap();

        let samples = decode_midi(&encoded).unwrap();
        let (rms, peak, roughness) = pcm_stats(&samples);

        assert!(
            (4_200.0..=8_000.0).contains(&rms),
            "DOTA title MIDI RMS {rms:.1} is outside the bounded mix range"
        );
        assert!(peak <= 24_000, "DOTA title MIDI peak {peak} is too loud");
        assert!(
            roughness <= 0.45,
            "DOTA title MIDI roughness {roughness:.4} indicates excessive high-frequency noise"
        );
    }

    #[test]
    fn dota_gameplay_midi_has_bounded_level_and_noise() {
        let fixture =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test/fixtures/dota.mrp");
        let package = crate::Package::open(fixture, crate::ResourceLimits::default()).unwrap();
        let encoded = package.read_named(b"dkljngle.mid").unwrap();

        let samples = decode_midi(&encoded).unwrap();
        let (rms, peak, roughness) = pcm_stats(&samples);
        assert!(
            (3_000.0..=6_000.0).contains(&rms),
            "DOTA gameplay MIDI RMS {rms:.1} is outside the bounded mix range"
        );
        assert!(peak <= 28_000, "DOTA gameplay MIDI peak {peak} is too loud");
        assert!(
            roughness <= 0.5,
            "DOTA gameplay MIDI roughness {roughness:.4} indicates excessive high-frequency noise"
        );
    }

    #[test]
    fn player_stops_at_the_end_of_non_looping_audio() {
        let player = AudioPlayer::default();
        player.play(SoundType::Midi, SIMPLE_MIDI, false).unwrap();
        assert!(player.is_active());
        let mut output = vec![0; AUDIO_SAMPLE_RATE as usize * AUDIO_CHANNELS];
        assert!(player.render(&mut output) > 0);
        assert!(!player.is_active());
        assert_eq!(player.render(&mut output), 0);
        assert!(output.iter().all(|sample| *sample == 0));
    }

    #[test]
    fn player_loops_and_stop_is_idempotent() {
        let player = AudioPlayer::default();
        player.play(SoundType::Midi, SIMPLE_MIDI, true).unwrap();
        let mut output = vec![0; AUDIO_SAMPLE_RATE as usize * AUDIO_CHANNELS * 2];
        assert_eq!(player.render(&mut output), output.len() / AUDIO_CHANNELS);
        assert!(player.is_active());
        player.stop();
        player.stop();
        assert!(!player.is_active());
    }

    #[test]
    fn invalid_encoded_audio_is_rejected_without_replacing_playback() {
        let player = AudioPlayer::default();
        player.play(SoundType::Midi, SIMPLE_MIDI, true).unwrap();
        assert!(player.play(SoundType::Mp3, b"not an mp3", false).is_err());
        assert!(player.is_active());
    }

    #[test]
    fn packaged_mp3_decodes_and_resamples_to_output_format() {
        let fixture =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test/fixtures/geyaxz.mrp");
        let package = crate::Package::open(fixture, crate::ResourceLimits::default()).unwrap();
        let encoded = package.read_named(b"select.mp3").unwrap();

        let samples = decode_mp3(&encoded).unwrap();

        assert!(!samples.is_empty());
        assert_eq!(samples.len() % AUDIO_CHANNELS, 0);
        assert!(samples.iter().any(|sample| *sample != 0));
    }

    #[test]
    fn packaged_gtdgdq_collision_wav_decodes_and_resamples_to_output_format() {
        let fixture =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test/fixtures/gtdgdq.mrp");
        let package = crate::Package::open(fixture, crate::ResourceLimits::default()).unwrap();
        let encoded = package.read_named(b"zhuangji.wav").unwrap();

        assert_eq!(SoundType::from_mrp(1), Some(SoundType::Wav));
        let samples = decode_wav(&encoded).unwrap();
        assert!(!samples.is_empty());
        assert_eq!(samples.len() % AUDIO_CHANNELS, 0);
        assert!(samples.iter().any(|sample| *sample != 0));

        let player = AudioPlayer::default();
        player.play(SoundType::Wav, &encoded, false).unwrap();
        let mut rendered = vec![0; samples.len()];
        assert_eq!(player.render(&mut rendered), samples.len() / AUDIO_CHANNELS);
        assert_eq!(rendered, samples);
    }
}
