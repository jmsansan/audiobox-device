//! The protocol and the worklet's engine, run natively: what the browser's two halves say
//! to each other, and what the worklet's mixer makes of it.
use audiobox::mixer::{
    Distance, Fade, Loop, MasterLimiter, MixerConfig, OverLimit, Space2d, Variation,
};
use audiobox::synth::{Aliasing, RenderOptions};
use audiobox::{Audio, FadeCurve, TimePosition};

use crate::player::{Bus, Command, PlayOptions, PlayerEvent, SoundId, VoiceId};
use crate::protocol::{self, Message, Report};
use crate::worklet::Engine;
use crate::{Error, ErrorCode};

const SCORE: &str = "
tempo 150
instrument lead wave=pulse duty=25 adsr=4,80,60,60
channel lead = @lead o5 l8 c e g e
";

fn config() -> MixerConfig {
    MixerConfig {
        sample_rate: 48_000,
        channels: 2,
        buses: vec!["music".into(), "effects".into()],
        seed: 0xfeed_beef,
        space: Space2d {
            width: 8.0,
            min_distance: 2.0,
            max_distance: 30.0,
            model: Distance::Inverse,
        },
        limiter: Some(MasterLimiter {
            ceiling_db: -3.0,
            lookahead_ms: 2.5,
            release_ms: 80.0,
        }),
        ..MixerConfig::default()
    }
}

fn commands() -> Vec<Command> {
    let fade = Fade {
        seconds: 0.25,
        curve: FadeCurve::EqualPower,
    };
    vec![
        Command::Load {
            sound: SoundId(3),
            audio: Audio::new(vec![vec![0.5, -0.25, 0.125], vec![0.0, 1.0, -1.0]], 44_100).unwrap(),
        },
        Command::LoadScore {
            sound: SoundId(4),
            score: SCORE.into(),
            options: RenderOptions {
                aliasing: Aliasing::Naive,
                loops: None,
                gain: 0.5,
            },
        },
        Command::Unload { sound: SoundId(9) },
        Command::Play {
            voice: VoiceId(7),
            sound: SoundId(4),
            options: PlayOptions {
                bus: Bus(2),
                gain: 0.75,
                pan: -0.5,
                looping: Loop::Region { start: 10, end: 20 },
                fade_in: fade,
                priority: 200,
                rate: 1.5,
                pitch: -2.25,
                variation: Variation {
                    pitch: 1.0,
                    gain_db: 3.0,
                },
                start: TimePosition::Seconds(0.5),
                instances: Some(
                    SoundId(4)
                        .instances()
                        .max(3)
                        .min_gap(0.1)
                        .policy(OverLimit::StopQuietest),
                ),
                position: Some([-4.0, 1.5]),
            },
            at: Some(96_000),
        },
        Command::Play {
            voice: VoiceId(8),
            sound: SoundId(3),
            options: PlayOptions {
                start: TimePosition::Frames(-2),
                ..PlayOptions::default()
            },
            at: None,
        },
        Command::Stop {
            voice: VoiceId(7),
            fade,
            at: None,
        },
        Command::StopAll { fade, at: Some(5) },
        Command::Gain {
            voice: VoiceId(1),
            gain: 0.5,
            fade,
            at: None,
        },
        Command::Pan {
            voice: VoiceId(1),
            pan: 1.0,
            fade,
            at: Some(1),
        },
        Command::BusGain {
            bus: Bus(1),
            gain: 0.0,
            fade,
            at: None,
        },
        Command::Rate {
            voice: VoiceId(1),
            rate: 0.5,
            fade,
            at: Some(2),
        },
        Command::Pitch {
            voice: VoiceId(1),
            semitones: 7.0,
            fade,
            at: None,
        },
        Command::Position {
            voice: VoiceId(1),
            position: [3.0, -4.0],
            fade,
            at: Some(3),
        },
        Command::Pause {
            voice: VoiceId(1),
            fade,
            at: None,
        },
        Command::Resume {
            voice: VoiceId(1),
            fade,
            at: Some(4),
        },
        Command::PauseBus {
            bus: Bus(2),
            fade,
            at: None,
        },
        Command::ResumeBus {
            bus: Bus(2),
            fade,
            at: Some(5),
        },
        Command::Reseed { seed: 42 },
    ]
}

#[test]
fn every_command_survives_the_trip() {
    for command in commands() {
        let bytes = protocol::encode_command(&command);
        let Ok(Message::Command(decoded)) = protocol::decode_message(&bytes) else {
            panic!("{command:?}");
        };
        // The commands have no equality, so compare what they encode to.
        assert_eq!(protocol::encode_command(&decoded), bytes, "{command:?}");
    }
    let bytes = protocol::encode_init(&config());
    let Ok(Message::Init(decoded)) = protocol::decode_message(&bytes) else {
        panic!("init");
    };
    assert_eq!(
        (decoded.sample_rate, decoded.channels, decoded.buses),
        (48_000, 2, config().buses)
    );
    assert_eq!(
        (decoded.seed, decoded.space, decoded.limiter),
        (config().seed, config().space, config().limiter)
    );
    let plain = MixerConfig::default();
    let Ok(Message::Init(decoded)) = protocol::decode_message(&protocol::encode_init(&plain))
    else {
        panic!("init");
    };
    assert_eq!(decoded.limiter, None);
}

#[test]
fn reports_survive_the_trip() {
    let reports = vec![
        Report::Frame(123_456),
        Report::Event(PlayerEvent::Ended(VoiceId(1))),
        Report::Event(PlayerEvent::Stolen(VoiceId(2))),
        Report::Event(PlayerEvent::Rejected(VoiceId(3))),
        Report::Event(PlayerEvent::Limited(VoiceId(6))),
        Report::Event(PlayerEvent::Failed(
            VoiceId(4),
            audiobox::ErrorCode::DecodeError,
        )),
        Report::Event(PlayerEvent::CommandFailed(Error::new(
            ErrorCode::InvalidArgument,
            "No sound 5.",
        ))),
    ];
    let mut bytes = Vec::new();
    for report in &reports {
        protocol::encode_report(&mut bytes, report);
    }
    assert_eq!(protocol::decode_reports(&bytes).unwrap(), reports);
}

#[test]
fn malformed_messages_are_errors_not_panics() {
    let good = protocol::encode_command(&commands()[3]);
    // Every prefix of a message is cut short; another version is refused.
    for end in 0..good.len() {
        assert!(protocol::decode_message(&good[..end]).is_err(), "{end}");
    }
    let mut other = good.clone();
    other[0] = 99;
    let Err(error) = protocol::decode_message(&other) else {
        panic!()
    };
    assert!(error.message.contains("version 99"), "{}", error.message);
    let mut longer = good;
    longer.push(0);
    assert!(protocol::decode_message(&longer).is_err());
    assert!(protocol::decode_reports(&[protocol::VERSION, 42]).is_err());
}

/// Sends a command to the engine as the page would.
fn send(engine: &mut Engine, command: &Command) {
    engine.apply(&protocol::encode_command(command));
}

fn take_reports(engine: &mut Engine) -> Vec<Report> {
    let reports = protocol::decode_reports(engine.reports()).unwrap_or_default();
    engine.clear_reports();
    reports
}

#[test]
fn the_worklet_engine_plays_what_the_page_sends() {
    let mut engine = Engine::new();
    // Before the mixer is made: silence, no channels.
    assert_eq!(engine.channels(), 0);
    assert!(engine.render(128).iter().all(|s| *s == 0.0));

    // Without the limiter, whose look-ahead would delay the output.
    engine.apply(&protocol::encode_init(&MixerConfig {
        limiter: None,
        ..config()
    }));
    assert_eq!(engine.channels(), 2);
    let tone = Audio::new(vec![vec![0.5; 480]], 48_000).unwrap();
    send(
        &mut engine,
        &Command::Load {
            sound: SoundId(0),
            audio: tone,
        },
    );
    send(
        &mut engine,
        &Command::Play {
            voice: VoiceId(10),
            sound: SoundId(0),
            options: PlayOptions::default(),
            at: None,
        },
    );
    let first = engine.render(128).to_vec();
    assert!(
        first
            .iter()
            .all(|s| (s - 0.5 * std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-4),
        "{first:?}"
    );

    // The 480-frame tone ends, and the page hears of it by its own voice id.
    for _ in 0..4 {
        engine.render(128);
    }
    let reports = take_reports(&mut engine);
    assert!(
        reports.contains(&Report::Event(PlayerEvent::Ended(VoiceId(10)))),
        "{reports:?}"
    );

    // A score plays as it is synthesized, on a named bus.
    send(
        &mut engine,
        &Command::LoadScore {
            sound: SoundId(1),
            score: SCORE.into(),
            options: RenderOptions {
                loops: None,
                ..RenderOptions::default()
            },
        },
    );
    send(
        &mut engine,
        &Command::Play {
            voice: VoiceId(11),
            sound: SoundId(1),
            options: PlayOptions {
                bus: Bus(1),
                ..PlayOptions::default()
            },
            at: None,
        },
    );
    let mut energy = 0.0;
    for _ in 0..40 {
        energy += engine.render(128).iter().map(|s| s * s).sum::<f32>();
    }
    assert!(energy > 1.0, "{energy}");
    // The frame count is reported a few times a second.
    let reports = take_reports(&mut engine);
    assert!(
        reports
            .iter()
            .any(|report| matches!(report, Report::Frame(frame) if *frame >= 4_800)),
        "{reports:?}"
    );
}

#[test]
fn a_failing_command_is_reported_to_the_page() {
    let mut engine = Engine::new();
    send(&mut engine, &Command::Unload { sound: SoundId(0) });
    engine.apply(&protocol::encode_init(&config()));
    send(
        &mut engine,
        &Command::Play {
            voice: VoiceId(1),
            sound: SoundId(42),
            options: PlayOptions::default(),
            at: None,
        },
    );
    engine.apply(&[protocol::VERSION, 77]);
    let failed: Vec<String> = take_reports(&mut engine)
        .into_iter()
        .filter_map(|report| match report {
            Report::Event(PlayerEvent::CommandFailed(error)) => Some(error.message),
            _ => None,
        })
        .collect();
    assert_eq!(failed.len(), 3, "{failed:?}");
    assert!(failed[0].contains("before the mixer"), "{failed:?}");
    assert!(failed[1].contains("No sound 42"), "{failed:?}");
}

#[test]
fn the_controls_of_audiobox_0_3_reach_the_worklet_mixer() {
    let mut engine = Engine::new();
    engine.apply(&protocol::encode_init(&MixerConfig {
        limiter: None,
        ..config()
    }));
    let tone = Audio::new(vec![vec![0.5; 48_000]], 48_000).unwrap();
    send(
        &mut engine,
        &Command::Load {
            sound: SoundId(0),
            audio: tone,
        },
    );
    let play = |voice, options| Command::Play {
        voice: VoiceId(voice),
        sound: SoundId(0),
        options,
        at: None,
    };
    // Fully left, since the space is 8 units wide, and quieter with the distance, as the
    // space places it.
    let limited = PlayOptions {
        instances: Some(SoundId(0).instances().max(1)),
        position: Some([-8.0, 0.0]),
        ..PlayOptions::default()
    };
    send(&mut engine, &play(1, limited));
    // A second voice of the sound is over its limit of one.
    send(&mut engine, &play(2, limited));
    let frames = engine.render(128).to_vec();
    let (left, right): (Vec<f32>, Vec<f32>) =
        frames.chunks(2).map(|frame| (frame[0], frame[1])).unzip();
    let (pan, gain) = config().space.place([-8.0, 0.0]);
    assert_eq!(pan, -1.0);
    assert!(gain > 0.0 && gain < 1.0);
    assert!(
        left.iter().all(|s| (s - 0.5 * gain).abs() < 1e-3),
        "{left:?}"
    );
    assert!(right.iter().all(|s| s.abs() < 1e-3), "{right:?}");
    let reports = take_reports(&mut engine);
    assert!(
        reports.contains(&Report::Event(PlayerEvent::Limited(VoiceId(2)))),
        "{reports:?}"
    );

    // A paused bus is silent and keeps its voices; resumed, they play on.
    let now = Fade::NONE;
    send(
        &mut engine,
        &Command::PauseBus {
            bus: Bus::MASTER,
            fade: now,
            at: None,
        },
    );
    engine.render(128);
    assert!(engine.render(128).iter().all(|s| s.abs() < 1e-6));
    send(
        &mut engine,
        &Command::ResumeBus {
            bus: Bus::MASTER,
            fade: now,
            at: None,
        },
    );
    engine.render(128);
    assert!(engine.render(128).iter().any(|s| s.abs() > 0.05));

    // Rate, pitch, a pause of the voice and a new seed are applied without a failure.
    for command in [
        Command::Rate {
            voice: VoiceId(1),
            rate: 2.0,
            fade: now,
            at: None,
        },
        Command::Pitch {
            voice: VoiceId(1),
            semitones: -12.0,
            fade: now,
            at: None,
        },
        Command::Pause {
            voice: VoiceId(1),
            fade: now,
            at: None,
        },
        Command::Reseed { seed: 7 },
    ] {
        send(&mut engine, &command);
    }
    engine.render(128);
    assert!(engine.render(128).iter().all(|s| s.abs() < 1e-6));
    let failed: Vec<Report> = take_reports(&mut engine)
        .into_iter()
        .filter(|report| matches!(report, Report::Event(PlayerEvent::CommandFailed(_))))
        .collect();
    assert!(failed.is_empty(), "{failed:?}");
}
