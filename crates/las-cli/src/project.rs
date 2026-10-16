// SPDX-License-Identifier: GPL-3.0-or-later
//! `las info` and `las convert`.

use las_project::{decode, encode, Bundle};

fn load(path: &str) -> Result<Bundle, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
    decode(&bytes).map_err(|e| format!("{path}: {e}"))
}

pub fn info(args: &[String]) -> Result<(), String> {
    let [path] = args else {
        return Err("usage: las info <project.las>".into());
    };
    let b = load(path)?;
    let p = &b.project;
    let notes: usize = p
        .patterns
        .iter()
        .map(|x| x.notes.values().map(Vec::len).sum::<usize>())
        .sum();
    let effects: usize = p
        .mixer
        .iter()
        .map(|t| t.slots.iter().flatten().count())
        .sum();
    println!("{}", p.name);
    println!(
        "  tempo      {} BPM, {}/{}",
        p.bpm, p.time_sig.0, p.time_sig.1
    );
    println!("  channels   {}", p.channels.len());
    for c in &p.channels {
        println!("    {:<16} {}", c.name, c.plugin_id);
    }
    println!("  patterns   {} ({notes} notes)", p.patterns.len());
    println!(
        "  playlist   {} clips on {} tracks",
        p.playlist.clips.len(),
        p.playlist.tracks.len()
    );
    println!("  automation {}", p.automation.len());
    println!("  mixer      {} inserts, {effects} effects", p.mixer.len());
    println!(
        "  samples    {} ({} stored in the file, {} bytes)",
        p.samples.len(),
        b.samples.len(),
        b.samples.values().map(|s| s.data.len()).sum::<usize>()
    );
    for w in &b.warnings {
        println!("  note: {w}");
    }
    Ok(())
}

pub fn convert(args: &[String]) -> Result<(), String> {
    let plain = args.iter().any(|a| a == "--plain");
    let files: Vec<&String> = args.iter().filter(|a| *a != "--plain").collect();
    let [from, to] = files[..] else {
        return Err("usage: las convert <in.las> <out.las> [--plain]".into());
    };
    let b = load(from)?;
    let bytes = encode(&b.project, &b.samples, !plain).map_err(|e| e.to_string())?;
    std::fs::write(to, bytes).map_err(|e| format!("{to}: {e}"))?;
    for w in &b.warnings {
        eprintln!("note: {w}");
    }
    Ok(())
}

/// `las render-project`: the mix of a project, offline, as a float WAV.
pub fn render(args: &[String]) -> Result<(), String> {
    let mut file = None;
    let mut out = None;
    let mut opts = las_engine::RenderOptions::default();
    let mut pattern: Option<Option<String>> = None;
    let mut loops = 1usize;
    let mut it = args.iter().peekable();
    while let Some(a) = it.next() {
        let mut value = |name: &str| -> Result<String, String> {
            it.next()
                .cloned()
                .ok_or_else(|| format!("{name} needs a value"))
        };
        match a.as_str() {
            "-o" | "--out" => out = Some(value("-o")?),
            "--rate" => {
                opts.sample_rate = value("--rate")?
                    .parse()
                    .map_err(|_| "--rate wants a number of hertz".to_string())?
            }
            "--tail" => {
                opts.tail = value("--tail")?
                    .parse()
                    .map_err(|_| "--tail wants seconds".to_string())?
            }
            "--loops" => {
                loops = value("--loops")?
                    .parse()
                    .map_err(|_| "--loops wants a whole number".to_string())?
            }
            "--pattern" => {
                let id = it.next_if(|n| !n.starts_with('-')).cloned();
                pattern = Some(id);
            }
            f if !f.starts_with('-') && file.is_none() => file = Some(f.to_string()),
            other => return Err(format!("unexpected {other}")),
        }
    }
    let file = file.ok_or("usage: las render-project <project.las> [-o out.wav]")?;
    if let Some(id) = pattern {
        opts.mode = las_engine::Mode::Pattern { id, loops };
    }
    let b = load(&file)?;
    let out = out.unwrap_or_else(|| file.trim_end_matches(".las").to_string() + ".wav");
    let started = std::time::Instant::now();
    let r = las_engine::render_mix(&b.project, &opts)?;
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: r.sample_rate as u32,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut w = hound::WavWriter::create(&out, spec).map_err(|e| format!("{out}: {e}"))?;
    for (l, rr) in r.left.iter().zip(&r.right) {
        w.write_sample(*l).map_err(|e| e.to_string())?;
        w.write_sample(*rr).map_err(|e| e.to_string())?;
    }
    w.finalize().map_err(|e| e.to_string())?;
    let secs = r.seconds();
    let took = started.elapsed().as_secs_f64();
    println!(
        "{out}: {secs:.1} s rendered in {took:.1} s ({:.0}x real time)",
        secs / took.max(1e-9)
    );
    for n in &b.warnings {
        eprintln!("note: {n}");
    }
    Ok(())
}
