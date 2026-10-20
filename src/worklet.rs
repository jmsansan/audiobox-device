//! What runs inside the browser's audio worklet: the mixer, with the commands that the
//! main thread sends it. It is ordinary Rust, so it is tested natively; the crate
//! `audiobox-device-worklet` wraps it in the WebAssembly module that the worklet loads.
use audiobox::mixer::Mixer;

use crate::Error;
use crate::player::{Dispatcher, PlayerEvent};
use crate::protocol::{self, Message, Report};

/// Frames between two reports of the mixer's frame count: a tenth of a second at 48 kHz.
const FRAME_REPORT: u64 = 4_800;

/// The worklet's mixer and the state of its commands.
#[derive(Default)]
pub struct Engine {
    mixer: Option<(Mixer, Box<Dispatcher>)>,
    channels: usize,
    output: Vec<f32>,
    reports: Vec<u8>,
    last_report: u64,
}

impl Engine {
    pub fn new() -> Self {
        Self::default()
    }

    /// The mixer's channels, once it is made; 0 before.
    pub fn channels(&self) -> usize {
        self.channels
    }

    /// Applies one message from the main thread. A command that fails is reported, not
    /// returned: the main thread learns of it through its events.
    pub fn apply(&mut self, bytes: &[u8]) {
        let result = protocol::decode_message(bytes).and_then(|message| match message {
            Message::Init(config) => {
                let (mixer, handle) = Mixer::new(config.clone())?;
                self.channels = mixer.channels();
                self.mixer = Some((mixer, Box::new(Dispatcher::new(handle, &config.buses))));
                Ok(())
            }
            Message::Command(command) => match &mut self.mixer {
                Some((_, dispatcher)) => dispatcher.apply(command),
                None => Err(Error::new(
                    crate::ErrorCode::InvalidArgument,
                    "A command came before the mixer was made.",
                )),
            },
        });
        if let Err(error) = result {
            protocol::encode_report(
                &mut self.reports,
                &Report::Event(PlayerEvent::CommandFailed(error)),
            );
        }
    }

    /// Renders `frames` interleaved frames of the mixer's channels, silence before it is
    /// made, and gathers the events that follow.
    pub fn render(&mut self, frames: usize) -> &[f32] {
        let samples = frames * self.channels.max(1);
        if self.output.len() < samples {
            self.output.resize(samples, 0.0);
        }
        let output = &mut self.output[..samples];
        match &mut self.mixer {
            None => output.fill(0.0),
            Some((mixer, dispatcher)) => {
                mixer.render(output);
                while let Some(event) = dispatcher.poll_event() {
                    protocol::encode_report(&mut self.reports, &Report::Event(event));
                }
                let frame = mixer.frame();
                if frame >= self.last_report + FRAME_REPORT {
                    self.last_report = frame;
                    protocol::encode_report(&mut self.reports, &Report::Frame(frame));
                }
            }
        }
        &self.output[..samples]
    }

    /// The reports gathered since [`Engine::clear_reports`], as one message.
    pub fn reports(&self) -> &[u8] {
        &self.reports
    }

    pub fn clear_reports(&mut self) {
        self.reports.clear();
    }
}
