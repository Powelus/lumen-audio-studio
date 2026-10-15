// SPDX-License-Identifier: GPL-3.0-or-later
//! `las`: command-line tools for Lumen Audio Studio.
//!
//! `las info` and `las convert` read and write `.las` project files.
//! Also: `las render-facet`, `las render-mist` and `las render-synth`, which render a patch and
//! notes to a WAV file, so the sound can be checked and shared without an
//! interface.

use las_dsp::engine::{Cc, Event, Playable};
use las_dsp::fm::fm_engine;
use las_dsp::mist::mist_engine;
use las_dsp::synth::synth_engine;
use serde_json::Value;
use std::process::ExitCode;

mod project;

const USAGE: &str = "las: command-line tools for Lumen Audio Studio

USAGE
    las render-facet <case.json> [-o <out.wav>] [--seconds <n>] [--rate <hz>]
    las render-mist  <case.json> [-o <out.wav>] [--seconds <n>] [--rate <hz>]
    las render-synth <case.json> [-o <out.wav>] [--seconds <n>] [--rate <hz>]
    las info <project.las>
    las convert <in.las> <out.las> [--plain]
    las --version

info lists what a project holds. convert reads a project (and repairs or updates
it) and writes it again; --plain leaves the file unencrypted.

A case file is JSON with a \"patch\" (only what differs from the defaults), a list
of \"events\" and an optional \"seed\". See examples/facet-bell.json and
examples/mist-pad.json and examples/synth-supersaw.json. Mist plays its built-in
texture. render-synth is the wavetable synth (Polaris, Vortex and the others).

Events: {\"t\":\"on\",\"time\":0,\"key\":60,\"vel\":0.8}   (also \"fine\" and \"pan\")
        {\"t\":\"off\",\"time\":0.5,\"key\":60}
        {\"t\":\"allOff\",\"time\":1}
        {\"t\":\"cc\",\"time\":0.2,\"cc\":\"mw\",\"value\":0.5}   (\"mw\", \"at\" or \"pb\")
";

fn num(v: &Value, key: &str, default: f64) -> f64 {
    v.get(key).and_then(Value::as_f64).unwrap_or(default)
}

fn event(v: &Value) -> Result<Event, String> {
    let time = num(v, "time", 0.0);
    Ok(match v.get("t").and_then(Value::as_str) {
        Some("on") => Event::On {
            time,
            key: num(v, "key", 60.0),
            vel: num(v, "vel", 1.0),
            fine: num(v, "fine", 0.0),
            pan: num(v, "pan", 0.0),
        },
        Some("off") => Event::Off {
            time,
            key: num(v, "key", 60.0),
        },
        Some("allOff") => Event::AllOff { time },
        Some("cc") => Event::Cc {
            time,
            cc: match v.get("cc").and_then(Value::as_str) {
                Some("mw") => Cc::Mw,
                Some("at") => Cc::At,
                Some("pb") => Cc::Pb,
                other => return Err(format!("unknown controller {other:?}")),
            },
            value: num(v, "value", 0.0),
        },
        other => return Err(format!("unknown event type {other:?}")),
    })
}

struct Options {
    input: String,
    output: String,
    seconds: Option<f64>,
    rate: u32,
}

fn parse(args: &[String]) -> Result<Options, String> {
    let mut input = None;
    let mut output = "out.wav".to_string();
    let mut seconds = None;
    let mut rate = 48000;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value = |name: &str| it.next().cloned().ok_or(format!("{name} needs a value"));
        match a.as_str() {
            "-o" | "--out" => output = value("-o")?,
            "--seconds" => {
                seconds = Some(
                    value("--seconds")?
                        .parse::<f64>()
                        .map_err(|e| format!("--seconds: {e}"))?,
                )
            }
            "--rate" => {
                rate = value("--rate")?
                    .parse::<u32>()
                    .map_err(|e| format!("--rate: {e}"))?
            }
            s if s.starts_with('-') => return Err(format!("unknown option {s}")),
            s => input = Some(s.to_string()),
        }
    }
    Ok(Options {
        input: input.ok_or("missing the case file")?,
        output,
        seconds,
        rate,
    })
}

#[derive(Clone, Copy)]
enum Kind {
    Facet,
    Mist,
    Synth,
}

/// Play the events through an engine in small blocks, as the live app does.
fn run<E: Playable>(
    mut engine: E,
    patch: Option<&Value>,
    events: &[Event],
    frames: usize,
    opts: &Options,
) -> Result<(Vec<f32>, Vec<f32>), String> {
    if let Some(p) = patch {
        engine
            .set_patch(p)
            .map_err(|e| format!("{}: {e}", opts.input))?;
    }
    engine.queue(events);
    let sr = engine.sample_rate();
    let (mut l, mut r) = (vec![0f32; frames], vec![0f32; frames]);
    let mut i = 0;
    while i < frames {
        let end = (i + 128).min(frames);
        engine.process(&mut l[i..end], &mut r[i..end], i as f64 / sr);
        i = end;
    }
    Ok((l, r))
}

fn render(kind: Kind, opts: &Options) -> Result<(), String> {
    let text = std::fs::read_to_string(&opts.input).map_err(|e| format!("{}: {e}", opts.input))?;
    let case: Value = serde_json::from_str(&text).map_err(|e| format!("{}: {e}", opts.input))?;
    let events: Vec<Event> = case
        .get("events")
        .and_then(Value::as_array)
        .map_or(Ok(Vec::new()), |a| {
            a.iter().map(event).collect::<Result<_, _>>()
        })?;
    let last = events.iter().map(Event::time).fold(0.0, f64::max);
    // the notes, then a tail for the release
    let seconds = opts.seconds.unwrap_or(last + 2.0);
    if !(0.0..=600.0).contains(&seconds) || seconds == 0.0 {
        return Err("--seconds must be between 0 and 600".into());
    }
    if !(8000..=384_000).contains(&opts.rate) {
        return Err("--rate must be between 8000 and 384000".into());
    }
    let sr = f64::from(opts.rate);
    let seed = num(&case, "seed", 1.0) as u32;
    let frames = (seconds * sr) as usize;
    let patch = case.get("patch");
    let (l, r) = match kind {
        Kind::Facet => run(fm_engine(sr, seed), patch, &events, frames, opts)?,
        Kind::Mist => run(mist_engine(sr, seed), patch, &events, frames, opts)?,
        Kind::Synth => run(synth_engine(sr, seed), patch, &events, frames, opts)?,
    };
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: opts.rate,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut w = hound::WavWriter::create(&opts.output, spec)
        .map_err(|e| format!("{}: {e}", opts.output))?;
    for (a, b) in l.iter().zip(&r) {
        w.write_sample(*a).map_err(|e| e.to_string())?;
        w.write_sample(*b).map_err(|e| e.to_string())?;
    }
    w.finalize().map_err(|e| e.to_string())?;
    let peak = l.iter().chain(&r).fold(0f32, |m, x| m.max(x.abs()));
    println!(
        "wrote {} ({seconds:.2} s, {} Hz, peak {peak:.3})",
        opts.output, opts.rate
    );
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("--version" | "-V") => {
            println!("las {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Some("render-facet") => parse(&args[1..]).and_then(|o| render(Kind::Facet, &o)),
        Some("render-mist") => parse(&args[1..]).and_then(|o| render(Kind::Mist, &o)),
        Some("render-synth") => parse(&args[1..]).and_then(|o| render(Kind::Synth, &o)),
        Some("info") => project::info(&args[1..]),
        Some("convert") => project::convert(&args[1..]),
        Some("--help" | "-h") | None => {
            print!("{USAGE}");
            Ok(())
        }
        Some(other) => Err(format!("unknown command {other}\n\n{USAGE}")),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("las: {e}");
            ExitCode::FAILURE
        }
    }
}
