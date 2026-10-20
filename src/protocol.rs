//! The bytes that travel between a browser's main thread and its audio worklet.
//!
//! Each message from the main thread is one command; each message from the worklet is a
//! sequence of events. Numbers are little-endian. The first byte of a message is its
//! version, so a page and a worklet built from different versions fail clearly.
use audiobox::mixer::{Fade, Loop, MixerConfig};
use audiobox::synth::{Aliasing, RenderOptions};
use audiobox::{Audio, FadeCurve};

use crate::player::{Bus, Command, PlayOptions, PlayerEvent, SoundId, VoiceId};
use crate::{Error, ErrorCode, Result};

/// The protocol's version, the first byte of every message.
pub(crate) const VERSION: u8 = 1;

/// The first message to the worklet: the mixer to make.
pub(crate) fn encode_init(config: &MixerConfig) -> Vec<u8> {
    let mut out = Writer::new(0);
    out.u32(config.sample_rate);
    out.u8(config.channels as u8);
    out.u32(config.max_voices as u32);
    out.u32(config.queue as u32);
    out.u16(config.buses.len() as u16);
    for bus in &config.buses {
        out.text(bus);
    }
    out.0
}

pub(crate) enum Message {
    Init(MixerConfig),
    Command(Command),
}

pub(crate) fn encode_command(command: &Command) -> Vec<u8> {
    match command {
        Command::Load { sound, audio } => {
            let mut out = Writer::new(1);
            out.u32(sound.0);
            out.u32(audio.sample_rate());
            out.u8(audio.channels() as u8);
            out.u32(audio.frames() as u32);
            for channel in audio.all_channels() {
                for sample in channel {
                    out.f32(*sample);
                }
            }
            out.0
        }
        Command::LoadScore {
            sound,
            score,
            options,
        } => {
            let mut out = Writer::new(2);
            out.u32(sound.0);
            out.u8(match options.aliasing {
                Aliasing::BandLimited => 0,
                Aliasing::Naive => 1,
            });
            out.option_u64(options.loops.map(u64::from));
            out.f32(options.gain);
            out.text(score);
            out.0
        }
        Command::Unload { sound } => {
            let mut out = Writer::new(3);
            out.u32(sound.0);
            out.0
        }
        Command::Play {
            voice,
            sound,
            options,
            at,
        } => {
            let mut out = Writer::new(4);
            out.u64(voice.0);
            out.u32(sound.0);
            out.option_u64(*at);
            out.u16(options.bus.0);
            out.f32(options.gain);
            out.f32(options.pan);
            match options.looping {
                Loop::Off => out.u8(0),
                Loop::Whole => out.u8(1),
                Loop::Region { start, end } => {
                    out.u8(2);
                    out.u64(start);
                    out.u64(end);
                }
            }
            out.fade(options.fade_in);
            out.u8(options.priority);
            out.0
        }
        Command::Stop { voice, fade, at } => {
            let mut out = Writer::new(5);
            out.u64(voice.0);
            out.option_u64(*at);
            out.fade(*fade);
            out.0
        }
        Command::StopAll { fade, at } => {
            let mut out = Writer::new(6);
            out.option_u64(*at);
            out.fade(*fade);
            out.0
        }
        Command::Gain {
            voice,
            gain,
            fade,
            at,
        } => {
            let mut out = Writer::new(7);
            out.u64(voice.0);
            out.option_u64(*at);
            out.f32(*gain);
            out.fade(*fade);
            out.0
        }
        Command::Pan {
            voice,
            pan,
            fade,
            at,
        } => {
            let mut out = Writer::new(8);
            out.u64(voice.0);
            out.option_u64(*at);
            out.f32(*pan);
            out.fade(*fade);
            out.0
        }
        Command::BusGain {
            bus,
            gain,
            fade,
            at,
        } => {
            let mut out = Writer::new(9);
            out.u16(bus.0);
            out.option_u64(*at);
            out.f32(*gain);
            out.fade(*fade);
            out.0
        }
    }
}

pub(crate) fn decode_message(bytes: &[u8]) -> Result<Message> {
    let mut input = Reader::new(bytes)?;
    let message = match input.u8()? {
        0 => {
            let sample_rate = input.u32()?;
            let channels = usize::from(input.u8()?);
            let max_voices = input.u32()? as usize;
            let queue = input.u32()? as usize;
            let count = input.u16()?;
            let mut buses = Vec::new();
            for _ in 0..count {
                buses.push(input.text()?);
            }
            Message::Init(MixerConfig {
                sample_rate,
                channels,
                max_voices,
                buses,
                queue,
            })
        }
        1 => {
            let sound = SoundId(input.u32()?);
            let sample_rate = input.u32()?;
            let channels = usize::from(input.u8()?);
            let frames = input.u32()? as usize;
            let mut data = Vec::with_capacity(channels);
            for _ in 0..channels {
                let mut channel = Vec::with_capacity(frames);
                for _ in 0..frames {
                    channel.push(input.f32()?);
                }
                data.push(channel);
            }
            let audio = Audio::new(data, sample_rate)?;
            Message::Command(Command::Load { sound, audio })
        }
        2 => {
            let sound = SoundId(input.u32()?);
            let aliasing = match input.u8()? {
                0 => Aliasing::BandLimited,
                1 => Aliasing::Naive,
                other => return Err(malformed(format!("aliasing {other}"))),
            };
            let loops = input.option_u64()?.map(|loops| loops as u32);
            let gain = input.f32()?;
            let score = input.text()?;
            Message::Command(Command::LoadScore {
                sound,
                score,
                options: RenderOptions {
                    aliasing,
                    loops,
                    gain,
                },
            })
        }
        3 => Message::Command(Command::Unload {
            sound: SoundId(input.u32()?),
        }),
        4 => {
            let voice = VoiceId(input.u64()?);
            let sound = SoundId(input.u32()?);
            let at = input.option_u64()?;
            let bus = Bus(input.u16()?);
            let gain = input.f32()?;
            let pan = input.f32()?;
            let looping = match input.u8()? {
                0 => Loop::Off,
                1 => Loop::Whole,
                2 => Loop::Region {
                    start: input.u64()?,
                    end: input.u64()?,
                },
                other => return Err(malformed(format!("loop {other}"))),
            };
            let fade_in = input.fade()?;
            let priority = input.u8()?;
            Message::Command(Command::Play {
                voice,
                sound,
                options: PlayOptions {
                    bus,
                    gain,
                    pan,
                    looping,
                    fade_in,
                    priority,
                },
                at,
            })
        }
        5 => Message::Command(Command::Stop {
            voice: VoiceId(input.u64()?),
            at: input.option_u64()?,
            fade: input.fade()?,
        }),
        6 => Message::Command(Command::StopAll {
            at: input.option_u64()?,
            fade: input.fade()?,
        }),
        7 => Message::Command(Command::Gain {
            voice: VoiceId(input.u64()?),
            at: input.option_u64()?,
            gain: input.f32()?,
            fade: input.fade()?,
        }),
        8 => Message::Command(Command::Pan {
            voice: VoiceId(input.u64()?),
            at: input.option_u64()?,
            pan: input.f32()?,
            fade: input.fade()?,
        }),
        9 => Message::Command(Command::BusGain {
            bus: Bus(input.u16()?),
            at: input.option_u64()?,
            gain: input.f32()?,
            fade: input.fade()?,
        }),
        other => return Err(malformed(format!("command {other}"))),
    };
    input.end()?;
    Ok(message)
}

/// What the worklet reports: events of voices, failed commands, and the frame count.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Report {
    Frame(u64),
    Event(PlayerEvent),
}

/// Appends a report to a message of reports, which starts with the version byte.
pub(crate) fn encode_report(out: &mut Vec<u8>, report: &Report) {
    if out.is_empty() {
        out.push(VERSION);
    }
    let mut writer = Writer(std::mem::take(out));
    match report {
        Report::Frame(frame) => {
            writer.u8(0);
            writer.u64(*frame);
        }
        Report::Event(PlayerEvent::Ended(voice)) => {
            writer.u8(1);
            writer.u64(voice.0);
        }
        Report::Event(PlayerEvent::Stolen(voice)) => {
            writer.u8(2);
            writer.u64(voice.0);
        }
        Report::Event(PlayerEvent::Rejected(voice)) => {
            writer.u8(3);
            writer.u64(voice.0);
        }
        Report::Event(PlayerEvent::Failed(voice, code)) => {
            writer.u8(4);
            writer.u64(voice.0);
            writer.u8(audio_code_to_byte(*code));
        }
        Report::Event(PlayerEvent::CommandFailed(error)) => {
            writer.u8(5);
            writer.u8(code_to_byte(error.code));
            writer.text(&error.message);
        }
    }
    *out = writer.0;
}

pub(crate) fn decode_reports(bytes: &[u8]) -> Result<Vec<Report>> {
    let mut input = Reader::new(bytes)?;
    let mut reports = Vec::new();
    while !input.is_empty() {
        reports.push(match input.u8()? {
            0 => Report::Frame(input.u64()?),
            1 => Report::Event(PlayerEvent::Ended(VoiceId(input.u64()?))),
            2 => Report::Event(PlayerEvent::Stolen(VoiceId(input.u64()?))),
            3 => Report::Event(PlayerEvent::Rejected(VoiceId(input.u64()?))),
            4 => {
                let voice = VoiceId(input.u64()?);
                Report::Event(PlayerEvent::Failed(voice, byte_to_audio_code(input.u8()?)))
            }
            5 => {
                let code = byte_to_code(input.u8()?);
                Report::Event(PlayerEvent::CommandFailed(Error::new(code, input.text()?)))
            }
            other => return Err(malformed(format!("report {other}"))),
        });
    }
    Ok(reports)
}

fn malformed(what: String) -> Error {
    Error::new(
        ErrorCode::InvalidArgument,
        format!("Malformed message: {what}."),
    )
}

const CODES: [ErrorCode; 9] = [
    ErrorCode::NoDevice,
    ErrorCode::DeviceNotFound,
    ErrorCode::DeviceUnavailable,
    ErrorCode::DeviceBusy,
    ErrorCode::PermissionDenied,
    ErrorCode::HostUnavailable,
    ErrorCode::UnsupportedConfig,
    ErrorCode::InvalidArgument,
    ErrorCode::StreamFailed,
];

fn code_to_byte(code: ErrorCode) -> u8 {
    CODES.iter().position(|c| *c == code).unwrap_or(7) as u8
}

fn byte_to_code(byte: u8) -> ErrorCode {
    CODES
        .get(usize::from(byte))
        .copied()
        .unwrap_or(ErrorCode::InvalidArgument)
}

const AUDIO_CODES: [audiobox::ErrorCode; 7] = [
    audiobox::ErrorCode::UnsupportedFormat,
    audiobox::ErrorCode::DecodeError,
    audiobox::ErrorCode::EncodeError,
    audiobox::ErrorCode::InvalidArgument,
    audiobox::ErrorCode::LimitExceeded,
    audiobox::ErrorCode::Io,
    audiobox::ErrorCode::Syntax,
];

fn audio_code_to_byte(code: audiobox::ErrorCode) -> u8 {
    AUDIO_CODES.iter().position(|c| *c == code).unwrap_or(3) as u8
}

fn byte_to_audio_code(byte: u8) -> audiobox::ErrorCode {
    AUDIO_CODES
        .get(usize::from(byte))
        .copied()
        .unwrap_or(audiobox::ErrorCode::InvalidArgument)
}

struct Writer(Vec<u8>);

impl Writer {
    fn new(kind: u8) -> Self {
        Self(vec![VERSION, kind])
    }
    fn u8(&mut self, value: u8) {
        self.0.push(value);
    }
    fn u16(&mut self, value: u16) {
        self.0.extend_from_slice(&value.to_le_bytes());
    }
    fn u32(&mut self, value: u32) {
        self.0.extend_from_slice(&value.to_le_bytes());
    }
    fn u64(&mut self, value: u64) {
        self.0.extend_from_slice(&value.to_le_bytes());
    }
    fn f32(&mut self, value: f32) {
        self.0.extend_from_slice(&value.to_le_bytes());
    }
    fn f64(&mut self, value: f64) {
        self.0.extend_from_slice(&value.to_le_bytes());
    }
    fn option_u64(&mut self, value: Option<u64>) {
        match value {
            None => self.u8(0),
            Some(value) => {
                self.u8(1);
                self.u64(value);
            }
        }
    }
    fn text(&mut self, text: &str) {
        self.u32(text.len() as u32);
        self.0.extend_from_slice(text.as_bytes());
    }
    fn fade(&mut self, fade: Fade) {
        self.f64(fade.seconds);
        self.u8(match fade.curve {
            FadeCurve::Linear => 0,
            FadeCurve::EqualPower => 1,
            FadeCurve::Exponential => 2,
        });
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Result<Self> {
        match bytes.split_first() {
            Some((&VERSION, rest)) => Ok(Self { bytes: rest }),
            Some((version, _)) => Err(malformed(format!(
                "version {version}, where this build speaks {VERSION}"
            ))),
            None => Err(malformed("an empty message".into())),
        }
    }
    fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }
    fn end(&self) -> Result<()> {
        if self.bytes.is_empty() {
            Ok(())
        } else {
            Err(malformed(format!("{} bytes too many", self.bytes.len())))
        }
    }
    fn take<const N: usize>(&mut self) -> Result<[u8; N]> {
        let (head, rest) = self
            .bytes
            .split_first_chunk::<N>()
            .ok_or_else(|| malformed("a message cut short".into()))?;
        self.bytes = rest;
        Ok(*head)
    }
    fn u8(&mut self) -> Result<u8> {
        Ok(self.take::<1>()?[0])
    }
    fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.take()?))
    }
    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take()?))
    }
    fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.take()?))
    }
    fn f32(&mut self) -> Result<f32> {
        Ok(f32::from_le_bytes(self.take()?))
    }
    fn f64(&mut self) -> Result<f64> {
        Ok(f64::from_le_bytes(self.take()?))
    }
    fn option_u64(&mut self) -> Result<Option<u64>> {
        match self.u8()? {
            0 => Ok(None),
            1 => Ok(Some(self.u64()?)),
            other => Err(malformed(format!("option {other}"))),
        }
    }
    fn text(&mut self) -> Result<String> {
        let length = self.u32()? as usize;
        if length > self.bytes.len() {
            return Err(malformed("a text cut short".into()));
        }
        let (text, rest) = self.bytes.split_at(length);
        self.bytes = rest;
        String::from_utf8(text.to_vec()).map_err(|_| malformed("text that is not UTF-8".into()))
    }
    fn fade(&mut self) -> Result<Fade> {
        let seconds = self.f64()?;
        let curve = match self.u8()? {
            0 => FadeCurve::Linear,
            1 => FadeCurve::EqualPower,
            2 => FadeCurve::Exponential,
            other => return Err(malformed(format!("fade curve {other}"))),
        };
        Ok(Fade { seconds, curve })
    }
}
