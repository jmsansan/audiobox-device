//! One API for every output: sounds by id, voices by id, buses by name.
//!
//! A [`Player`] loads sounds and plays them. Natively it drives the
//! [`MixerHandle`](audiobox::mixer::MixerHandle) of a mixer that an
//! `AudioOutput` plays. In the browser the mixer runs inside an
//! `AudioWorklet`, in a WebAssembly instance of its own, and the player sends it commands;
//! sounds are loaded once and then named by id, since two instances cannot share memory.
//! A game writes the same code for both.
use std::collections::HashMap;
use std::sync::Arc;

use audiobox::mixer::{
    BusId as MixerBus, Fade, Instances, Loop, MixerConfig, MixerEvent, MixerHandle, Play, Sound,
    Variation, VoiceId as MixerVoice,
};
use audiobox::synth::{RenderOptions, Score, Synth};
use audiobox::{Audio, TimePosition};

use crate::{Error, ErrorCode, Result};

/// A sound loaded into a [`Player`].
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SoundId(pub(crate) u32);

impl SoundId {
    /// Instance limits keyed by this sound, so that its voices share them, natively and
    /// in the browser alike. Set them with [`Instances::max`], [`Instances::min_gap`]
    /// and [`Instances::policy`]. `audiobox`'s [`Instances::of`] keys an `Audio` by
    /// its identity instead, which does not cross to the browser's worklet.
    pub fn instances(self) -> Instances {
        Instances::new(u64::from(self.0))
    }
}

/// A voice started by [`Player::play`].
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct VoiceId(pub(crate) u64);

/// A bus: [`Bus::MASTER`], or a named bus of the mixer's configuration from
/// [`Player::bus`].
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Bus(pub(crate) u16);

impl Bus {
    pub const MASTER: Bus = Bus(0);
}

/// How a voice plays, as `audiobox`'s [`Play`] with a [`Bus`].
#[derive(Clone, Copy, Debug)]
pub struct PlayOptions {
    pub bus: Bus,
    /// Linear gain, at least 0. Default 1.
    pub gain: f32,
    /// -1 (left) to 1 (right). Default 0.
    pub pan: f32,
    pub looping: Loop,
    pub fade_in: Fade,
    /// Decides which voice gives way when too many play. Default 128.
    pub priority: u8,
    /// Playback rate, which changes speed and pitch together, 1/16..=8. Default 1.
    pub rate: f32,
    /// Pitch offset in semitones on top of the rate, -48..=48. Default 0.
    pub pitch: f32,
    /// Random variation of pitch and gain, drawn from the mixer's seed.
    pub variation: Variation,
    /// Where in the sound to start. Default the start.
    pub start: TimePosition,
    /// Limits on how many voices of the sound play at once and how often it starts;
    /// see [`SoundId::instances`]. Default none.
    pub instances: Option<Instances>,
    /// The sound's position relative to the listener, in the units of the mixer's
    /// [`Space2d`](audiobox::mixer::Space2d). It replaces `pan` and adds a distance
    /// gain. Default none.
    pub position: Option<[f32; 2]>,
}

impl Default for PlayOptions {
    fn default() -> Self {
        let play = Play::default();
        Self {
            bus: Bus::MASTER,
            gain: play.gain,
            pan: play.pan,
            looping: play.looping,
            fade_in: play.fade_in,
            priority: play.priority,
            rate: play.rate,
            pitch: play.pitch,
            variation: play.variation,
            start: play.start,
            instances: play.instances,
            position: play.position,
        }
    }
}

/// Something that happened to a voice, or a command that failed, reported by
/// [`Player::poll_event`].
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum PlayerEvent {
    /// The voice finished, was stopped, or finished fading out after being stolen.
    Ended(VoiceId),
    /// Another voice took this voice's place; it fades out, then ends.
    Stolen(VoiceId),
    /// The voice did not start because too many voices of higher priority play.
    Rejected(VoiceId),
    /// The voice did not start because of its sound's instance limits.
    Limited(VoiceId),
    /// The voice's stream failed and the voice stopped.
    Failed(VoiceId, audiobox::ErrorCode),
    /// A command failed where it ran. Natively a failing command returns its error, so
    /// this comes only from the browser's mixer.
    CommandFailed(Error),
}

/// What a [`Player`] asks of a mixer. Natively it is applied at once; in the browser it is
/// encoded and applied by the worklet's mixer.
#[derive(Clone, Debug)]
pub(crate) enum Command {
    Load {
        sound: SoundId,
        audio: Audio,
    },
    LoadScore {
        sound: SoundId,
        score: String,
        options: RenderOptions,
    },
    Unload {
        sound: SoundId,
    },
    Play {
        voice: VoiceId,
        sound: SoundId,
        options: PlayOptions,
        at: Option<u64>,
    },
    Stop {
        voice: VoiceId,
        fade: Fade,
        at: Option<u64>,
    },
    StopAll {
        fade: Fade,
        at: Option<u64>,
    },
    Gain {
        voice: VoiceId,
        gain: f32,
        fade: Fade,
        at: Option<u64>,
    },
    Pan {
        voice: VoiceId,
        pan: f32,
        fade: Fade,
        at: Option<u64>,
    },
    BusGain {
        bus: Bus,
        gain: f32,
        fade: Fade,
        at: Option<u64>,
    },
    Rate {
        voice: VoiceId,
        rate: f32,
        fade: Fade,
        at: Option<u64>,
    },
    Pitch {
        voice: VoiceId,
        semitones: f32,
        fade: Fade,
        at: Option<u64>,
    },
    Position {
        voice: VoiceId,
        position: [f32; 2],
        fade: Fade,
        at: Option<u64>,
    },
    Pause {
        voice: VoiceId,
        fade: Fade,
        at: Option<u64>,
    },
    Resume {
        voice: VoiceId,
        fade: Fade,
        at: Option<u64>,
    },
    PauseBus {
        bus: Bus,
        fade: Fade,
        at: Option<u64>,
    },
    ResumeBus {
        bus: Bus,
        fade: Fade,
        at: Option<u64>,
    },
    Reseed {
        seed: u64,
    },
}

enum Stored {
    Memory(Arc<Audio>),
    Score(Arc<Score>, RenderOptions),
}

/// Applies commands to a mixer's handle, keeping the sounds and the map between the
/// player's voice ids and the mixer's. The same code runs natively and in the worklet.
pub(crate) struct Dispatcher {
    handle: MixerHandle,
    buses: Vec<MixerBus>,
    sounds: HashMap<SoundId, Stored>,
    to_mixer: HashMap<VoiceId, MixerVoice>,
    from_mixer: HashMap<MixerVoice, VoiceId>,
}

impl Dispatcher {
    pub(crate) fn new(handle: MixerHandle, bus_names: &[String]) -> Self {
        let buses = bus_names
            .iter()
            .filter_map(|name| handle.bus(name))
            .collect();
        Self {
            handle,
            buses,
            sounds: HashMap::new(),
            to_mixer: HashMap::new(),
            from_mixer: HashMap::new(),
        }
    }

    pub(crate) fn frame(&self) -> u64 {
        self.handle.frame()
    }

    fn bus(&self, bus: Bus) -> Result<MixerBus> {
        match bus.0 {
            0 => Ok(MixerBus::MASTER),
            index => self
                .buses
                .get(usize::from(index) - 1)
                .copied()
                .ok_or_else(|| Error::new(ErrorCode::InvalidArgument, format!("No bus {index}."))),
        }
    }

    pub(crate) fn apply(&mut self, command: Command) -> Result<()> {
        match command {
            Command::Load { sound, audio } => {
                self.sounds.insert(sound, Stored::Memory(Arc::new(audio)));
            }
            Command::LoadScore {
                sound,
                score,
                options,
            } => {
                let parsed: Score = score.parse().map_err(|error: audiobox::Error| {
                    Error::new(ErrorCode::InvalidArgument, error.to_string())
                })?;
                self.sounds
                    .insert(sound, Stored::Score(Arc::new(parsed), options));
            }
            Command::Unload { sound } => {
                self.sounds.remove(&sound);
            }
            Command::Play {
                voice,
                sound,
                options,
                at,
            } => {
                let source = match self.sounds.get(&sound) {
                    Some(Stored::Memory(audio)) => Sound::Memory(Arc::clone(audio)),
                    Some(Stored::Score(score, render)) => {
                        Sound::stream(Synth::new(score, self.handle.sample_rate(), render)?)
                    }
                    None => {
                        return Err(Error::new(
                            ErrorCode::InvalidArgument,
                            format!("No sound {} is loaded.", sound.0),
                        ));
                    }
                };
                let play = Play {
                    bus: self.bus(options.bus)?,
                    gain: options.gain,
                    pan: options.pan,
                    looping: options.looping,
                    fade_in: options.fade_in,
                    priority: options.priority,
                    rate: options.rate,
                    pitch: options.pitch,
                    variation: options.variation,
                    start: options.start,
                    instances: options.instances,
                    position: options.position,
                };
                let id = match at {
                    None => self.handle.play(source, &play)?,
                    Some(frame) => self.handle.at(frame).play(source, &play)?,
                };
                self.to_mixer.insert(voice, id);
                self.from_mixer.insert(id, voice);
            }
            // A voice that already ended is gone from the map; a change to it does nothing,
            // as it does to the mixer.
            Command::Stop { voice, fade, at } => {
                if let Some(id) = self.to_mixer.get(&voice).copied() {
                    match at {
                        None => self.handle.stop(id, fade)?,
                        Some(frame) => self.handle.at(frame).stop(id, fade)?,
                    }
                }
            }
            Command::StopAll { fade, at } => match at {
                None => self.handle.stop_all(fade)?,
                Some(frame) => self.handle.at(frame).stop_all(fade)?,
            },
            Command::Gain {
                voice,
                gain,
                fade,
                at,
            } => {
                if let Some(id) = self.to_mixer.get(&voice).copied() {
                    match at {
                        None => self.handle.set_gain(id, gain, fade)?,
                        Some(frame) => self.handle.at(frame).set_gain(id, gain, fade)?,
                    }
                }
            }
            Command::Pan {
                voice,
                pan,
                fade,
                at,
            } => {
                if let Some(id) = self.to_mixer.get(&voice).copied() {
                    match at {
                        None => self.handle.set_pan(id, pan, fade)?,
                        Some(frame) => self.handle.at(frame).set_pan(id, pan, fade)?,
                    }
                }
            }
            Command::BusGain {
                bus,
                gain,
                fade,
                at,
            } => {
                let bus = self.bus(bus)?;
                match at {
                    None => self.handle.set_bus_gain(bus, gain, fade)?,
                    Some(frame) => self.handle.at(frame).set_bus_gain(bus, gain, fade)?,
                }
            }
            Command::Rate {
                voice,
                rate,
                fade,
                at,
            } => {
                if let Some(id) = self.to_mixer.get(&voice).copied() {
                    match at {
                        None => self.handle.set_rate(id, rate, fade)?,
                        Some(frame) => self.handle.at(frame).set_rate(id, rate, fade)?,
                    }
                }
            }
            Command::Pitch {
                voice,
                semitones,
                fade,
                at,
            } => {
                if let Some(id) = self.to_mixer.get(&voice).copied() {
                    match at {
                        None => self.handle.set_pitch(id, semitones, fade)?,
                        Some(frame) => self.handle.at(frame).set_pitch(id, semitones, fade)?,
                    }
                }
            }
            Command::Position {
                voice,
                position,
                fade,
                at,
            } => {
                if let Some(id) = self.to_mixer.get(&voice).copied() {
                    match at {
                        None => self.handle.set_position(id, position, fade)?,
                        Some(frame) => self.handle.at(frame).set_position(id, position, fade)?,
                    }
                }
            }
            Command::Pause { voice, fade, at } => {
                if let Some(id) = self.to_mixer.get(&voice).copied() {
                    match at {
                        None => self.handle.pause(id, fade)?,
                        Some(frame) => self.handle.at(frame).pause(id, fade)?,
                    }
                }
            }
            Command::Resume { voice, fade, at } => {
                if let Some(id) = self.to_mixer.get(&voice).copied() {
                    match at {
                        None => self.handle.resume(id, fade)?,
                        Some(frame) => self.handle.at(frame).resume(id, fade)?,
                    }
                }
            }
            Command::PauseBus { bus, fade, at } => {
                let bus = self.bus(bus)?;
                match at {
                    None => self.handle.pause_bus(bus, fade)?,
                    Some(frame) => self.handle.at(frame).pause_bus(bus, fade)?,
                }
            }
            Command::ResumeBus { bus, fade, at } => {
                let bus = self.bus(bus)?;
                match at {
                    None => self.handle.resume_bus(bus, fade)?,
                    Some(frame) => self.handle.at(frame).resume_bus(bus, fade)?,
                }
            }
            Command::Reseed { seed } => self.handle.reseed(seed),
        }
        Ok(())
    }

    pub(crate) fn poll_event(&mut self) -> Option<PlayerEvent> {
        while let Some(event) = self.handle.poll_event() {
            let (id, ends) = match event {
                MixerEvent::Ended(id)
                | MixerEvent::Rejected(id)
                | MixerEvent::Limited(id)
                | MixerEvent::Failed(id, _) => (id, true),
                MixerEvent::Stolen(id) => (id, false),
                // An event that a later audiobox adds is not one a player reports yet.
                _ => continue,
            };
            let voice = if ends {
                self.from_mixer.remove(&id).inspect(|voice| {
                    self.to_mixer.remove(voice);
                })
            } else {
                self.from_mixer.get(&id).copied()
            };
            let Some(voice) = voice else { continue };
            return Some(match event {
                MixerEvent::Ended(_) => PlayerEvent::Ended(voice),
                MixerEvent::Stolen(_) => PlayerEvent::Stolen(voice),
                MixerEvent::Rejected(_) => PlayerEvent::Rejected(voice),
                MixerEvent::Limited(_) => PlayerEvent::Limited(voice),
                MixerEvent::Failed(_, code) => PlayerEvent::Failed(voice, code),
                _ => continue,
            });
        }
        None
    }
}

enum Sink {
    /// The mixer's handle, in this process.
    Local(Box<Dispatcher>),
    /// The browser's worklet, through its message port.
    #[cfg(all(target_arch = "wasm32", feature = "web"))]
    Web(crate::web::Port),
}

/// Loads sounds and plays them, natively or in the browser, with the same calls.
///
/// Natively, make one with [`Player::new`] from the handle of a mixer that an
/// `AudioOutput` plays, or with
/// `AudioOutput::open_player`. In the browser,
/// `WebOutput::open` returns one.
pub struct Player {
    sink: Sink,
    buses: Vec<String>,
    sample_rate: u32,
    next_sound: u32,
    next_voice: u64,
}

impl Player {
    /// A player for the mixer whose handle this is, made with `config`.
    pub fn new(handle: MixerHandle, config: &MixerConfig) -> Self {
        Self {
            sink: Sink::Local(Box::new(Dispatcher::new(handle, &config.buses))),
            buses: config.buses.clone(),
            sample_rate: config.sample_rate,
            next_sound: 0,
            next_voice: 0,
        }
    }

    #[cfg(all(target_arch = "wasm32", feature = "web"))]
    pub(crate) fn web(port: crate::web::Port, config: &MixerConfig) -> Self {
        Self {
            sink: Sink::Web(port),
            buses: config.buses.clone(),
            sample_rate: config.sample_rate,
            next_sound: 0,
            next_voice: 0,
        }
    }

    /// The mixer's sample rate.
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// The named bus of the mixer's configuration.
    pub fn bus(&self, name: &str) -> Option<Bus> {
        self.buses
            .iter()
            .position(|bus| bus == name)
            .map(|index| Bus(index as u16 + 1))
    }

    /// Frames the mixer has rendered. In the browser it is the last count the worklet
    /// reported, a few times a second.
    pub fn frame(&self) -> u64 {
        match &self.sink {
            Sink::Local(dispatcher) => dispatcher.frame(),
            #[cfg(all(target_arch = "wasm32", feature = "web"))]
            Sink::Web(port) => port.frame(),
        }
    }

    /// Loads decoded audio, mono or stereo, to play by id. In the browser its samples
    /// are sent to the worklet once.
    pub fn load(&mut self, audio: Audio) -> Result<SoundId> {
        if !(1..=2).contains(&audio.channels()) {
            return Err(Error::new(
                ErrorCode::InvalidArgument,
                "A sound is mono or stereo.",
            ));
        }
        let sound = self.new_sound();
        self.send(Command::Load { sound, audio })?;
        Ok(sound)
    }

    /// Loads a score of `audiobox`'s synthesizer, played as it is synthesized. In the
    /// browser only its text travels, a few kilobytes, and the worklet synthesizes it. The
    /// score is checked here, so its errors return at once.
    pub fn load_score(&mut self, score: &str, options: RenderOptions) -> Result<SoundId> {
        score
            .parse::<Score>()
            .map_err(|error| Error::new(ErrorCode::InvalidArgument, error.to_string()))?;
        let sound = self.new_sound();
        self.send(Command::LoadScore {
            sound,
            score: score.to_owned(),
            options,
        })?;
        Ok(sound)
    }

    /// Forgets a sound. Voices that play it keep playing.
    pub fn unload(&mut self, sound: SoundId) -> Result<()> {
        self.send(Command::Unload { sound })
    }

    /// Starts a voice now.
    pub fn play(&mut self, sound: SoundId, options: &PlayOptions) -> Result<VoiceId> {
        self.play_at(None, sound, options)
    }

    /// Stops a voice, after a fade.
    pub fn stop(&mut self, voice: VoiceId, fade: Fade) -> Result<()> {
        self.send(Command::Stop {
            voice,
            fade,
            at: None,
        })
    }

    /// Stops every voice, after a fade.
    pub fn stop_all(&mut self, fade: Fade) -> Result<()> {
        self.send(Command::StopAll { fade, at: None })
    }

    pub fn set_gain(&mut self, voice: VoiceId, gain: f32, fade: Fade) -> Result<()> {
        self.send(Command::Gain {
            voice,
            gain,
            fade,
            at: None,
        })
    }

    pub fn set_pan(&mut self, voice: VoiceId, pan: f32, fade: Fade) -> Result<()> {
        self.send(Command::Pan {
            voice,
            pan,
            fade,
            at: None,
        })
    }

    pub fn set_bus_gain(&mut self, bus: Bus, gain: f32, fade: Fade) -> Result<()> {
        self.send(Command::BusGain {
            bus,
            gain,
            fade,
            at: None,
        })
    }

    /// Changes a voice's playback rate, after a ramp of the fade's length.
    pub fn set_rate(&mut self, voice: VoiceId, rate: f32, fade: Fade) -> Result<()> {
        self.send(Command::Rate {
            voice,
            rate,
            fade,
            at: None,
        })
    }

    /// Changes a voice's pitch offset in semitones, after a ramp of the fade's length.
    pub fn set_pitch(&mut self, voice: VoiceId, semitones: f32, fade: Fade) -> Result<()> {
        self.send(Command::Pitch {
            voice,
            semitones,
            fade,
            at: None,
        })
    }

    /// Moves a voice to a position relative to the listener, which sets its pan and its
    /// distance gain, after a ramp of the fade's length.
    pub fn set_position(&mut self, voice: VoiceId, position: [f32; 2], fade: Fade) -> Result<()> {
        self.send(Command::Position {
            voice,
            position,
            fade,
            at: None,
        })
    }

    /// Pauses a voice, after a fade; it keeps its place until it resumes.
    pub fn pause(&mut self, voice: VoiceId, fade: Fade) -> Result<()> {
        self.send(Command::Pause {
            voice,
            fade,
            at: None,
        })
    }

    /// Resumes a paused voice, fading in.
    pub fn resume(&mut self, voice: VoiceId, fade: Fade) -> Result<()> {
        self.send(Command::Resume {
            voice,
            fade,
            at: None,
        })
    }

    /// Pauses every voice on a bus, after a fade, such as the music and the effects while
    /// a game's pause menu shows; voices on other buses play on.
    pub fn pause_bus(&mut self, bus: Bus, fade: Fade) -> Result<()> {
        self.send(Command::PauseBus {
            bus,
            fade,
            at: None,
        })
    }

    /// Resumes the voices of a paused bus, fading in.
    pub fn resume_bus(&mut self, bus: Bus, fade: Fade) -> Result<()> {
        self.send(Command::ResumeBus {
            bus,
            fade,
            at: None,
        })
    }

    /// Restarts the generator of variations from `seed`, so that the same starts give
    /// the same variations again.
    pub fn reseed(&mut self, seed: u64) -> Result<()> {
        self.send(Command::Reseed { seed })
    }

    /// Changes that apply at an exact frame of the mixer, counted from its first rendered
    /// frame, whatever the buffer sizes.
    pub fn at(&mut self, frame: u64) -> Scheduled<'_> {
        Scheduled {
            player: self,
            frame,
        }
    }

    /// The next event, or `None`. Call it regularly, such as once per frame.
    pub fn poll_event(&mut self) -> Option<PlayerEvent> {
        match &mut self.sink {
            Sink::Local(dispatcher) => dispatcher.poll_event(),
            #[cfg(all(target_arch = "wasm32", feature = "web"))]
            Sink::Web(port) => port.poll_event(),
        }
    }

    fn play_at(
        &mut self,
        at: Option<u64>,
        sound: SoundId,
        options: &PlayOptions,
    ) -> Result<VoiceId> {
        let voice = VoiceId(self.next_voice);
        self.next_voice += 1;
        self.send(Command::Play {
            voice,
            sound,
            options: *options,
            at,
        })?;
        Ok(voice)
    }

    fn new_sound(&mut self) -> SoundId {
        let sound = SoundId(self.next_sound);
        self.next_sound += 1;
        sound
    }

    fn send(&mut self, command: Command) -> Result<()> {
        match &mut self.sink {
            Sink::Local(dispatcher) => dispatcher.apply(command),
            #[cfg(all(target_arch = "wasm32", feature = "web"))]
            Sink::Web(port) => port.send(&command),
        }
    }
}

/// Changes scheduled at a frame, from [`Player::at`].
pub struct Scheduled<'a> {
    player: &'a mut Player,
    frame: u64,
}

impl Scheduled<'_> {
    pub fn play(self, sound: SoundId, options: &PlayOptions) -> Result<VoiceId> {
        self.player.play_at(Some(self.frame), sound, options)
    }
    pub fn stop(self, voice: VoiceId, fade: Fade) -> Result<()> {
        self.player.send(Command::Stop {
            voice,
            fade,
            at: Some(self.frame),
        })
    }
    pub fn stop_all(self, fade: Fade) -> Result<()> {
        self.player.send(Command::StopAll {
            fade,
            at: Some(self.frame),
        })
    }
    pub fn set_gain(self, voice: VoiceId, gain: f32, fade: Fade) -> Result<()> {
        self.player.send(Command::Gain {
            voice,
            gain,
            fade,
            at: Some(self.frame),
        })
    }
    pub fn set_pan(self, voice: VoiceId, pan: f32, fade: Fade) -> Result<()> {
        self.player.send(Command::Pan {
            voice,
            pan,
            fade,
            at: Some(self.frame),
        })
    }
    pub fn set_bus_gain(self, bus: Bus, gain: f32, fade: Fade) -> Result<()> {
        self.player.send(Command::BusGain {
            bus,
            gain,
            fade,
            at: Some(self.frame),
        })
    }
    pub fn set_rate(self, voice: VoiceId, rate: f32, fade: Fade) -> Result<()> {
        self.player.send(Command::Rate {
            voice,
            rate,
            fade,
            at: Some(self.frame),
        })
    }
    pub fn set_pitch(self, voice: VoiceId, semitones: f32, fade: Fade) -> Result<()> {
        self.player.send(Command::Pitch {
            voice,
            semitones,
            fade,
            at: Some(self.frame),
        })
    }
    pub fn set_position(self, voice: VoiceId, position: [f32; 2], fade: Fade) -> Result<()> {
        self.player.send(Command::Position {
            voice,
            position,
            fade,
            at: Some(self.frame),
        })
    }
    pub fn pause(self, voice: VoiceId, fade: Fade) -> Result<()> {
        self.player.send(Command::Pause {
            voice,
            fade,
            at: Some(self.frame),
        })
    }
    pub fn resume(self, voice: VoiceId, fade: Fade) -> Result<()> {
        self.player.send(Command::Resume {
            voice,
            fade,
            at: Some(self.frame),
        })
    }
    pub fn pause_bus(self, bus: Bus, fade: Fade) -> Result<()> {
        self.player.send(Command::PauseBus {
            bus,
            fade,
            at: Some(self.frame),
        })
    }
    pub fn resume_bus(self, bus: Bus, fade: Fade) -> Result<()> {
        self.player.send(Command::ResumeBus {
            bus,
            fade,
            at: Some(self.frame),
        })
    }
}
