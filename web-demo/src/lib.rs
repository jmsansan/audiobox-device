//! A page that plays `audiobox-device`'s browser output: looping chiptune music and a jump
//! effect, through the same `Player` calls as a native game. `check.mjs` opens it in a
//! headless browser and reads `status()`.
#![cfg(target_arch = "wasm32")]

use std::cell::RefCell;

use audiobox::mixer::{Fade, Loop, MixerConfig, Variation};
use audiobox::sfx::{Effect, Preset};
use audiobox::synth::RenderOptions;
use audiobox_device::{Bus, PlayOptions, Player, PlayerEvent, SoundId, VoiceId, WebOutput};
use wasm_bindgen::prelude::*;
use web_sys::AnalyserNode;

const RATE: u32 = 48_000;
const SONG: &str = "
tempo 150
instrument lead wave=pulse duty=25 adsr=4,80,60,60 vibrato=12,170,200
instrument bass wave=triangle adsr=2,0,100,25
channel lead = @lead o5 l8 q7 L [c e g e >c< g e g]2 [d f a f >d< a f a]2
channel bass = @bass o2 l4 L [c c g g]2 [d d a a]2
";

struct Demo {
    output: WebOutput,
    player: Player,
    analyser: AnalyserNode,
    effects: Bus,
    jump: SoundId,
    jumps: u32,
    ended: u32,
    limited: u32,
    failed: Vec<String>,
    busy_voice: Option<VoiceId>,
    busy_ended: bool,
}

thread_local! {
    static DEMO: RefCell<Option<Demo>> = const { RefCell::new(None) };
}

fn error(error: audiobox_device::Error) -> JsValue {
    JsValue::from_str(&format!("{:?}: {}", error.code, error.message))
}

/// Opens the output, loads the music and the effect, and starts the music.
#[wasm_bindgen]
pub async fn start() -> Result<(), JsValue> {
    let config = MixerConfig {
        sample_rate: RATE,
        buses: vec!["music".into(), "effects".into()],
        // A queue of this size makes the worklet's first render grow the module's memory,
        // which the worklet's script must survive (the review of audiobox-device#4).
        queue: 336,
        ..MixerConfig::default()
    };
    let (output, mut player) = WebOutput::open(&config).await.map_err(error)?;
    let music = player.bus("music").unwrap();
    let effects = player.bus("effects").unwrap();
    let song = player
        .load_score(
            SONG,
            RenderOptions {
                loops: None,
                ..RenderOptions::default()
            },
        )
        .map_err(error)?;
    let jump = Effect::preset(Preset::Jump, 7)
        .render(RATE)
        .map_err(|e| JsValue::from_str(&e.message))?;
    let jump = player.load(jump).map_err(error)?;
    player
        .play(
            song,
            &PlayOptions {
                bus: music,
                looping: Loop::Off,
                priority: 255,
                ..PlayOptions::default()
            },
        )
        .map_err(error)?;
    let analyser = output.context().create_analyser()?;
    output.node().connect_with_audio_node(&analyser)?;
    output.resume();
    DEMO.with_borrow_mut(|demo| {
        *demo = Some(Demo {
            output,
            player,
            analyser,
            effects,
            jump,
            jumps: 0,
            ended: 0,
            limited: 0,
            failed: Vec::new(),
            busy_voice: None,
            busy_ended: false,
        })
    });
    Ok(())
}

/// Resumes audio; a page calls it from a click.
#[wasm_bindgen]
pub fn resume() {
    DEMO.with_borrow(|demo| {
        if let Some(demo) = demo {
            demo.output.resume();
        }
    });
}

/// Starts a ten-second tone, keeps the main thread busy for 300 ms, then stops the tone.
/// The worklet must apply the stop after the play: then the tone ends at once.
#[wasm_bindgen]
pub fn busy_sequence() -> Result<(), JsValue> {
    DEMO.with_borrow_mut(|demo| {
        let Some(demo) = demo else { return Ok(()) };
        let tone = audiobox::Audio::new(vec![vec![0.1; RATE as usize * 10]], RATE)
            .map_err(|e| JsValue::from_str(&e.message))?;
        let tone = demo.player.load(tone).map_err(error)?;
        let voice = demo
            .player
            .play(tone, &PlayOptions::default())
            .map_err(error)?;
        let start = js_sys::Date::now();
        while js_sys::Date::now() - start < 300.0 {}
        demo.player.stop(voice, Fade::NONE).map_err(error)?;
        demo.busy_voice = Some(voice);
        Ok(())
    })
}

/// Plays the jump effect, panned left, with a little variation of pitch. A jump less
/// than 0.75 s after the last one is refused by the sound's instance limits.
#[wasm_bindgen]
pub fn jump() -> Result<(), JsValue> {
    DEMO.with_borrow_mut(|demo| {
        let Some(demo) = demo else { return Ok(()) };
        demo.player
            .play(
                demo.jump,
                &PlayOptions {
                    bus: demo.effects,
                    pan: -0.5,
                    variation: Variation {
                        pitch: 1.0,
                        gain_db: 0.0,
                    },
                    instances: Some(demo.jump.instances().min_gap(0.75)),
                    ..PlayOptions::default()
                },
            )
            .map_err(error)?;
        demo.jumps += 1;
        Ok(())
    })
}

/// The output's state as JSON: whether it runs, the mixer's frames, the level that an
/// analyser hears, and the events so far.
#[wasm_bindgen]
pub fn status() -> String {
    DEMO.with_borrow_mut(|demo| {
        let Some(demo) = demo else {
            return "{\"started\":false}".to_owned();
        };
        while let Some(event) = demo.player.poll_event() {
            match event {
                PlayerEvent::Ended(voice) if Some(voice) == demo.busy_voice => demo.busy_ended = true,
                PlayerEvent::Ended(_) => demo.ended += 1,
                PlayerEvent::Limited(_) => demo.limited += 1,
                PlayerEvent::CommandFailed(error) => demo.failed.push(error.message),
                PlayerEvent::Failed(_, code) => demo.failed.push(format!("{code:?}")),
                _ => {}
            }
        }
        let mut samples = vec![0.0f32; demo.analyser.fft_size() as usize];
        demo.analyser.get_float_time_domain_data(&mut samples);
        let rms = (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt();
        format!(
            "{{\"started\":true,\"running\":{},\"sampleRate\":{},\"frame\":{},\"rms\":{:.4},\"jumps\":{},\"ended\":{},\"limited\":{},\"failed\":{:?},\"busyEnded\":{}}}",
            demo.output.is_running(),
            demo.output.sample_rate(),
            demo.player.frame(),
            rms,
            demo.jumps,
            demo.ended,
            demo.limited,
            demo.failed,
            demo.busy_ended,
        )
    })
}
