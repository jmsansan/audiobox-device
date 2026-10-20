//! The protocol and the worklet's engine, run natively: what the browser's two halves say
//! to each other, and what the worklet's mixer makes of it.
use audiobox::mixer::{Fade, Loop, MixerConfig};
use audiobox::synth::{Aliasing, RenderOptions};
use audiobox::{Audio, FadeCurve};

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
            },
            at: Some(96_000),
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
}

#[test]
fn reports_survive_the_trip() {
    let reports = vec![
        Report::Frame(123_456),
        Report::Event(PlayerEvent::Ended(VoiceId(1))),
        Report::Event(PlayerEvent::Stolen(VoiceId(2))),
        Report::Event(PlayerEvent::Rejected(VoiceId(3))),
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

    engine.apply(&protocol::encode_init(&config()));
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
