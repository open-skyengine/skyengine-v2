use std::{
    f32::consts::TAU,
    io::Cursor,
    sync::{Arc, Mutex, MutexGuard},
};

use midly::{MetaMessage, MidiMessage, Smf, Timing, TrackEventKind};
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
const MIDI_ATTACK_FRAMES: usize = AUDIO_SAMPLE_RATE as usize / 200;
const MIDI_RELEASE_FRAMES: usize = AUDIO_SAMPLE_RATE as usize / 20;
const MIDI_PERCUSSION_RELEASE_FRAMES: usize = AUDIO_SAMPLE_RATE as usize / 100;
const MIDI_TAIL_FRAMES: usize = AUDIO_SAMPLE_RATE as usize;
const MIDI_MELODIC_GAIN: f32 = 0.13;
const MIDI_PERCUSSION_GAIN: f32 = 0.14;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SoundType {
    Midi,
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
            2 => Some(Self::Mp3),
            _ => None,
        }
    }
}

#[derive(Clone, Default)]
pub struct AudioPlayer {
    inner: Arc<Mutex<Playback>>,
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
    pub fn play(&self, sound_type: SoundType, data: &[u8], looped: bool) -> Result<()> {
        let samples = match sound_type {
            SoundType::Midi => decode_midi(data)?,
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

#[derive(Clone, Copy)]
struct MidiChannel {
    program: u8,
    volume: f32,
    pan: f32,
    pitch_bend: f32,
}

impl Default for MidiChannel {
    fn default() -> Self {
        Self {
            program: 0,
            volume: 100.0 / 127.0,
            pan: 0.5,
            pitch_bend: 0.0,
        }
    }
}

struct Voice {
    channel: u8,
    key: u8,
    velocity: f32,
    phase: f32,
    age_frames: usize,
    release_frame: Option<usize>,
    noise: u32,
    filtered_noise: f32,
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
                        bend: bend.as_int() - 8_192,
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

    let mut samples = Vec::new();
    let mut channels = [MidiChannel::default(); 16];
    let mut voices = Vec::<Voice>::new();
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
        render_midi_frames(&mut samples, frame_count, &mut voices, &channels)?;
        current_tick = tick;

        while index < events.len() && events[index].tick == tick {
            match events[index].event {
                MidiEvent::NoteOn {
                    channel,
                    key,
                    velocity,
                } => {
                    start_midi_voice(&mut voices, channel, key, velocity);
                }
                MidiEvent::NoteOff { channel, key } => {
                    for voice in voices
                        .iter_mut()
                        .filter(|voice| voice.channel == channel && voice.key == key)
                    {
                        voice.release_frame.get_or_insert(voice.age_frames);
                    }
                }
                MidiEvent::Program { channel, program } => {
                    channels[usize::from(channel)].program = program;
                }
                MidiEvent::Controller {
                    channel,
                    controller,
                    value,
                } => match controller {
                    7 => channels[usize::from(channel)].volume = f32::from(value) / 127.0,
                    10 => channels[usize::from(channel)].pan = f32::from(value) / 127.0,
                    120 => voices.retain(|voice| voice.channel != channel),
                    123 => {
                        for voice in voices.iter_mut().filter(|voice| voice.channel == channel) {
                            voice.release_frame.get_or_insert(voice.age_frames);
                        }
                    }
                    _ => {}
                },
                MidiEvent::PitchBend { channel, bend } => {
                    channels[usize::from(channel)].pitch_bend = f32::from(bend) / 8_192.0 * 2.0;
                }
                MidiEvent::Tempo(tempo) if tempo != 0 => tempo_micros = u64::from(tempo),
                MidiEvent::Tempo(_) => {}
            }
            index += 1;
        }
    }

    // Release unterminated melodic notes and let one-shot percussion finish naturally.
    if !voices.is_empty() {
        for voice in voices.iter_mut().filter(|voice| voice.channel != 9) {
            voice.release_frame.get_or_insert(voice.age_frames);
        }
        let tail_start = samples.len();
        render_midi_frames(&mut samples, MIDI_TAIL_FRAMES, &mut voices, &channels)?;
        while samples.len() > tail_start
            && samples[samples.len() - AUDIO_CHANNELS..]
                .iter()
                .all(|sample| *sample == 0)
        {
            samples.truncate(samples.len() - AUDIO_CHANNELS);
        }
    }
    Ok(samples)
}

fn start_midi_voice(voices: &mut Vec<Voice>, channel: u8, key: u8, velocity: u8) {
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
    voices.push(Voice {
        channel,
        key,
        velocity: f32::from(velocity) / 127.0,
        phase: 0.0,
        age_frames: 0,
        release_frame: None,
        noise: 0x9e37_79b9 ^ (u32::from(channel) << 8) ^ u32::from(key),
        filtered_noise: 0.0,
    });
}

fn percussion_chokes(trigger: u8, sounding: u8) -> bool {
    trigger == sounding || (matches!(trigger, 42 | 44 | 46) && matches!(sounding, 42 | 44 | 46))
}

fn render_midi_frames(
    output: &mut Vec<i16>,
    frames: usize,
    voices: &mut Vec<Voice>,
    channels: &[MidiChannel; 16],
) -> Result<()> {
    let current_frames = output.len() / AUDIO_CHANNELS;
    let allowed = MAX_AUDIO_FRAMES.saturating_sub(current_frames);
    if frames > allowed {
        return Err(Error::ResourceLimit(format!(
            "decoded audio exceeds the {MAX_AUDIO_SECONDS} second limit"
        )));
    }
    output.reserve(frames.saturating_mul(AUDIO_CHANNELS));
    for _ in 0..frames {
        let mut left = 0.0_f32;
        let mut right = 0.0_f32;
        for voice in voices.iter_mut() {
            if voice_finished(voice) {
                continue;
            }
            let channel = channels[usize::from(voice.channel)];
            let sample = if voice.channel == 9 {
                percussion_sample(voice)
            } else {
                let note = f32::from(voice.key) + channel.pitch_bend;
                let frequency = 440.0 * 2.0_f32.powf((note - 69.0) / 12.0);
                voice.phase = (voice.phase + frequency / AUDIO_SAMPLE_RATE as f32).fract();
                instrument_sample(channel.program, voice.phase) * melodic_envelope(voice)
            };
            let voice_gain = if voice.channel == 9 {
                MIDI_PERCUSSION_GAIN
            } else {
                MIDI_MELODIC_GAIN
            };
            let gain = voice.velocity * channel.volume * voice_gain;
            left += sample * gain * (1.0 - channel.pan).sqrt();
            right += sample * gain * channel.pan.sqrt();
            voice.age_frames = voice.age_frames.saturating_add(1);
        }
        output.push(float_to_i16(left));
        output.push(float_to_i16(right));
    }
    voices.retain(|voice| !voice_finished(voice));
    Ok(())
}

fn melodic_envelope(voice: &Voice) -> f32 {
    let attack = (voice.age_frames + 1).min(MIDI_ATTACK_FRAMES) as f32 / MIDI_ATTACK_FRAMES as f32;
    attack * release_envelope(voice, MIDI_RELEASE_FRAMES)
}

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
        38 | 40 => body_noise * 0.65 + (time * 180.0 * TAU).sin() * 0.35,
        41 | 43 | 45 | 47 | 48 | 50 => {
            let frequency = 210.0 * 2.0_f32.powf((f32::from(voice.key) - 47.0) / 12.0);
            voice.phase = (voice.phase + frequency / AUDIO_SAMPLE_RATE as f32).fract();
            (voice.phase * TAU).sin() * 0.75 + white * 0.25
        }
        42 | 44 | 46 => bright_noise * 0.55,
        49 | 51 | 52 | 55 | 57 | 59 => bright_noise * 0.45 + body_noise * 0.15,
        _ => body_noise * 0.7,
    };
    sample * envelope
}

fn percussion_duration_frames(key: u8) -> usize {
    let milliseconds = match key {
        35 | 36 => 260,
        38 | 40 => 220,
        42 | 44 => 80,
        46 => 360,
        49 | 51 | 52 | 55 | 57 | 59 => 900,
        41 | 43 | 45 | 47 | 48 | 50 => 300,
        _ => 180,
    };
    AUDIO_SAMPLE_RATE as usize * milliseconds / 1_000
}

fn voice_finished(voice: &Voice) -> bool {
    if voice.channel == 9 {
        voice.age_frames >= percussion_duration_frames(voice.key)
            || release_finished(voice, MIDI_PERCUSSION_RELEASE_FRAMES)
    } else {
        release_finished(voice, MIDI_RELEASE_FRAMES)
    }
}

fn release_envelope(voice: &Voice, frames: usize) -> f32 {
    voice.release_frame.map_or(1.0, |release_frame| {
        let elapsed = voice.age_frames.saturating_sub(release_frame);
        1.0 - elapsed.min(frames) as f32 / frames as f32
    })
}

fn release_finished(voice: &Voice, frames: usize) -> bool {
    voice
        .release_frame
        .is_some_and(|release_frame| voice.age_frames.saturating_sub(release_frame) >= frames)
}

fn instrument_sample(program: u8, phase: f32) -> f32 {
    match program {
        // Dota holds program 119 in long effect chords, so avoid the generic saw's wrap spike.
        119 => {
            (phase * TAU).sin() * 0.42
                + (phase * TAU * 2.0).sin() * 0.28
                + (phase * TAU * 3.0).sin() * 0.18
                + (phase * TAU * 5.0).sin() * 0.12
        }
        0..=31 => (phase * TAU).sin() * 0.85 + (phase * TAU * 2.0).sin() * 0.15,
        32..=63 => 1.0 - 4.0 * (phase - 0.5).abs(),
        64..=95 => {
            if phase < 0.5 {
                0.8
            } else {
                -0.8
            }
        }
        _ => phase * 2.0 - 1.0,
    }
}

fn float_to_i16(sample: f32) -> i16 {
    (sample.clamp(-1.0, 1.0) * f32::from(i16::MAX)).round() as i16
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

    fn test_voice(channel: u8, key: u8) -> Voice {
        Voice {
            channel,
            key,
            velocity: 1.0,
            phase: 0.0,
            age_frames: 0,
            release_frame: None,
            noise: 0x9e37_79b9 ^ (u32::from(channel) << 8) ^ u32::from(key),
            filtered_noise: 0.0,
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
    fn percussion_without_note_off_is_a_finite_one_shot() {
        let mut samples = Vec::new();
        let mut voices = vec![test_voice(9, 49)];

        render_midi_frames(
            &mut samples,
            AUDIO_SAMPLE_RATE as usize * 2,
            &mut voices,
            &[MidiChannel::default(); 16],
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
            (1_000..=3_000).contains(&onset_peak),
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
    fn percussion_note_off_releases_promptly() {
        let mut voice = test_voice(9, 49);
        voice.release_frame = Some(0);
        let mut voices = vec![voice];
        let mut samples = Vec::new();

        render_midi_frames(
            &mut samples,
            AUDIO_SAMPLE_RATE as usize / 10,
            &mut voices,
            &[MidiChannel::default(); 16],
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
    fn percussion_retrigger_chokes_the_previous_voice() {
        let mut voices = vec![test_voice(9, 42), test_voice(9, 46)];
        voices[0].age_frames = 100;
        voices[1].age_frames = 200;

        start_midi_voice(&mut voices, 9, 42, 100);

        assert_eq!(voices.len(), 1);
        assert_eq!(voices[0].key, 42);
        assert_eq!(voices[0].age_frames, 0);
    }

    #[test]
    fn dota_effect_timbre_has_no_sawtooth_wrap_spike() {
        let steps = 1_024;
        let mut previous = instrument_sample(119, 0.0);
        let mut largest_jump = 0.0_f32;
        for step in 1..=steps {
            let phase = (step as f32 / steps as f32).fract();
            let sample = instrument_sample(119, phase);
            largest_jump = largest_jump.max((sample - previous).abs());
            previous = sample;
        }

        assert!(
            largest_jump < 0.05,
            "DOTA effect timbre has a discontinuity of {largest_jump:.3}"
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
            (2_000.0..=3_000.0).contains(&rms),
            "DOTA title MIDI RMS {rms:.1} is outside the bounded mix range"
        );
        assert!(peak <= 12_000, "DOTA title MIDI peak {peak} is too loud");
        assert!(
            roughness <= 0.22,
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
            (2_500.0..=3_500.0).contains(&rms),
            "DOTA gameplay MIDI RMS {rms:.1} is outside the bounded mix range"
        );
        assert!(peak <= 20_000, "DOTA gameplay MIDI peak {peak} is too loud");
        assert!(
            roughness <= 0.20,
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
}
