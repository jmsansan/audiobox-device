//! The browser's output: the mixer runs inside an `AudioWorklet`, on the browser's audio
//! thread, in a WebAssembly instance of its own; the page's [`Player`] sends it commands
//! through the worklet's message port.
//!
//! The worklet's module and script ship inside this crate, so a page serves nothing
//! besides its own files. The script is the one piece of JavaScript: it instantiates the
//! module, copies messages into its memory, and copies each rendered quantum to the
//! outputs.
use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use audiobox::mixer::MixerConfig;
use js_sys::{Array, Object, Reflect, Uint8Array, WebAssembly};
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::{
    AudioContext, AudioContextOptions, AudioContextState, AudioWorkletNode,
    AudioWorkletNodeOptions, Blob, BlobPropertyBag, MessageEvent, MessagePort, Url,
};

use crate::player::{Command, Player, PlayerEvent};
use crate::protocol::{self, Report};
use crate::{Error, ErrorCode, Result};

const WORKLET_SCRIPT: &str = include_str!("../web/worklet.js");
const WORKLET_MODULE: &[u8] = include_bytes!("../web/audiobox_device_worklet.wasm");
/// The processor's name, as the script registers it.
const PROCESSOR: &str = "audiobox-device";

/// Plays a mixer in the browser.
///
/// Browsers start audio only after a user gesture: call [`WebOutput::resume`] from an
/// input handler, such as the first click or key press, and check
/// [`WebOutput::is_running`].
pub struct WebOutput {
    context: AudioContext,
    node: AudioWorkletNode,
}

impl WebOutput {
    /// Makes the audio context at the mixer's sample rate, starts the worklet and its
    /// mixer, and returns the output and its player.
    pub async fn open(config: &MixerConfig) -> Result<(WebOutput, Player)> {
        if !(1..=2).contains(&config.channels) {
            return Err(Error::new(
                ErrorCode::InvalidArgument,
                "The mixer is mono or stereo.",
            ));
        }
        let options = AudioContextOptions::new();
        options.set_sample_rate(config.sample_rate as f32);
        let context = AudioContext::new_with_context_options(&options)
            .map_err(|error| js(ErrorCode::HostUnavailable, error))?;

        // The script, from a blob URL, so that nothing has to be served.
        let parts = Array::of1(&JsValue::from_str(WORKLET_SCRIPT));
        let bag = BlobPropertyBag::new();
        bag.set_type("text/javascript");
        let blob = Blob::new_with_str_sequence_and_options(&parts, &bag).map_err(stream)?;
        let url = Url::create_object_url_with_blob(&blob).map_err(stream)?;
        let added = context
            .audio_worklet()
            .map_err(stream)?
            .add_module(&url)
            .map_err(stream)?;
        JsFuture::from(added).await.map_err(stream)?;
        let _ = Url::revoke_object_url(&url);

        // The module, compiled here and passed to the worklet, which cannot fetch.
        let bytes = Uint8Array::from(WORKLET_MODULE);
        let module = JsFuture::from(WebAssembly::compile(&bytes.into()))
            .await
            .map_err(stream)?;
        let processor_options = Object::new();
        Reflect::set(&processor_options, &"module".into(), &module).map_err(stream)?;
        let node_options = AudioWorkletNodeOptions::new();
        node_options.set_number_of_inputs(0);
        node_options.set_number_of_outputs(1);
        node_options.set_output_channel_count(&Array::of1(&JsValue::from(config.channels as u32)));
        node_options.set_processor_options(Some(&processor_options));
        let node = AudioWorkletNode::new_with_options(&context, PROCESSOR, &node_options)
            .map_err(stream)?;
        node.connect_with_audio_node(&context.destination())
            .map_err(stream)?;

        let port = Port::new(node.port().map_err(stream)?);
        port.post(&protocol::encode_init(config))?;
        Ok((WebOutput { context, node }, Player::web(port, config)))
    }

    /// Starts or resumes audio. Call it from a user gesture's handler.
    pub fn resume(&self) {
        if let Ok(promise) = self.context.resume() {
            drop(JsFuture::from(promise));
        }
    }

    /// Pauses the audio context, and with it the mixer's time.
    pub fn suspend(&self) {
        if let Ok(promise) = self.context.suspend() {
            drop(JsFuture::from(promise));
        }
    }

    /// Whether the audio context is running, which needs a user gesture first.
    pub fn is_running(&self) -> bool {
        self.context.state() == AudioContextState::Running
    }

    /// The audio context's sample rate, which is the mixer's.
    pub fn sample_rate(&self) -> f32 {
        self.context.sample_rate()
    }

    /// The audio context, to connect other nodes.
    pub fn context(&self) -> &AudioContext {
        &self.context
    }

    /// The worklet's node, whose output goes to the context's destination; other nodes,
    /// such as an analyser, may listen to it too.
    pub fn node(&self) -> &AudioWorkletNode {
        &self.node
    }
}

impl Drop for WebOutput {
    fn drop(&mut self) {
        let _ = self.node.disconnect();
        if let Ok(promise) = self.context.close() {
            drop(JsFuture::from(promise));
        }
    }
}

#[derive(Default)]
struct State {
    events: VecDeque<PlayerEvent>,
    frame: u64,
}

/// The page's end of the worklet's message port.
pub(crate) struct Port {
    port: MessagePort,
    state: Rc<RefCell<State>>,
    _on_message: Closure<dyn FnMut(MessageEvent)>,
}

impl Port {
    fn new(port: MessagePort) -> Self {
        let state = Rc::new(RefCell::new(State::default()));
        let shared = Rc::clone(&state);
        let on_message = Closure::<dyn FnMut(MessageEvent)>::new(move |event: MessageEvent| {
            let bytes = Uint8Array::new(&event.data()).to_vec();
            let mut state = shared.borrow_mut();
            match protocol::decode_reports(&bytes) {
                Ok(reports) => {
                    for report in reports {
                        match report {
                            Report::Frame(frame) => state.frame = frame,
                            Report::Event(event) => state.events.push_back(event),
                        }
                    }
                }
                Err(error) => state.events.push_back(PlayerEvent::CommandFailed(error)),
            }
        });
        port.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
        port.start();
        Self {
            port,
            state,
            _on_message: on_message,
        }
    }

    fn post(&self, bytes: &[u8]) -> Result<()> {
        let array = Uint8Array::from(bytes);
        let buffer = array.buffer();
        self.port
            .post_message_with_transferable(&buffer, &Array::of1(&buffer))
            .map_err(stream)
    }

    pub(crate) fn send(&self, command: &Command) -> Result<()> {
        self.post(&protocol::encode_command(command))
    }

    pub(crate) fn poll_event(&mut self) -> Option<PlayerEvent> {
        self.state.borrow_mut().events.pop_front()
    }

    pub(crate) fn frame(&self) -> u64 {
        self.state.borrow().frame
    }
}

fn js(code: ErrorCode, value: JsValue) -> Error {
    let message = value
        .dyn_ref::<js_sys::Error>()
        .map(|error| String::from(error.message()))
        .unwrap_or_else(|| format!("{value:?}"));
    Error::new(code, message)
}

fn stream(value: JsValue) -> Error {
    js(ErrorCode::StreamFailed, value)
}
