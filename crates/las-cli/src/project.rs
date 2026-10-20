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

/// How `las render-project` writes audio.
struct Output {
    flac: bool,
    bits: u32,
    normalize: bool,
}

impl Output {
    fn extension(&self) -> &'static str {
        if self.flac {
            "flac"
        } else {
            "wav"
        }
    }

    /// Write one stereo rendering.
    fn write(&self, path: &str, r: &las_engine::Rendered) -> Result<(), String> {
        let (l, rr) = (&r.left[..], &r.right[..]);
        let scaled;
        let chans: [&[f32]; 2] = if self.normalize {
            scaled = las_engine::export::normalize(&[l, rr], -0.3);
            [&scaled[0], &scaled[1]]
        } else {
            [l, rr]
        };
        let rate = r.sample_rate as u32;
        let bytes = if self.flac {
            las_engine::export::encode_flac(&chans, rate, self.bits)?
        } else {
            let depth = las_engine::export::BitDepth::from_bits(self.bits)
                .ok_or("--bits wants 16, 24 or 32")?;
            las_engine::export::encode_wav(&chans, rate, depth, true)
        };
        std::fs::write(path, bytes).map_err(|e| format!("{path}: {e}"))
    }
}

/// `las render-project`: the mix of a project, offline, as a WAV or FLAC file.
pub fn render(args: &[String]) -> Result<(), String> {
    let mut file = None;
    let mut out = None;
    let mut opts = las_engine::RenderOptions::default();
    let mut pattern: Option<Option<String>> = None;
    let mut loops = 1usize;
    let mut output = Output {
        flac: false,
        bits: 32,
        normalize: false,
    };
    let mut stems: Option<String> = None;
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
            "--bits" => {
                output.bits = value("--bits")?
                    .parse()
                    .map_err(|_| "--bits wants 16, 24 or 32".to_string())?
            }
            "--format" => match value("--format")?.as_str() {
                "wav" => output.flac = false,
                "flac" => output.flac = true,
                other => return Err(format!("--format wants wav or flac, not {other}")),
            },
            "--normalize" => output.normalize = true,
            "--stems" => stems = Some(value("--stems")?),
            "--pattern" => {
                let id = it.next_if(|n| !n.starts_with('-')).cloned();
                pattern = Some(id);
            }
            f if !f.starts_with('-') && file.is_none() => file = Some(f.to_string()),
            other => return Err(format!("unexpected {other}")),
        }
    }
    let file = file.ok_or("usage: las render-project <project.las> [-o out.wav]")?;
    if output.flac && output.bits == 32 {
        output.bits = 24;
    }
    if let Some(id) = pattern {
        opts.mode = las_engine::Mode::Pattern { id, loops };
    }
    let b = load(&file)?;
    let started = std::time::Instant::now();
    if let Some(dir) = stems {
        std::fs::create_dir_all(&dir).map_err(|e| format!("{dir}: {e}"))?;
        let list = las_engine::render_stems(&b.project, &b.samples, &opts)?;
        for s in &list {
            let name: String = s
                .name
                .chars()
                .map(|c| {
                    if c.is_alphanumeric() || c == ' ' || c == '-' {
                        c
                    } else {
                        '_'
                    }
                })
                .collect();
            output.write(&format!("{dir}/{name}.{}", output.extension()), &s.audio)?;
        }
        println!(
            "{} stems written to {dir} in {:.1} s",
            list.len(),
            started.elapsed().as_secs_f64()
        );
        for n in &b.warnings {
            eprintln!("note: {n}");
        }
        return Ok(());
    }
    let out =
        out.unwrap_or_else(|| format!("{}.{}", file.trim_end_matches(".las"), output.extension()));
    let r = las_engine::render_mix_with(&b.project, &b.samples, &opts)?;
    output.write(&out, &r)?;
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

/// `las midi`: the notes of a project as a standard MIDI file.
pub fn midi(args: &[String]) -> Result<(), String> {
    let mut file = None;
    let mut out = None;
    let mut pattern: Option<Option<String>> = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "-o" | "--out" => out = Some(it.next().cloned().ok_or("-o needs a value")?),
            "--pattern" => {
                let id = it
                    .as_slice()
                    .first()
                    .filter(|n| !n.starts_with('-'))
                    .cloned();
                if id.is_some() {
                    it.next();
                }
                pattern = Some(id);
            }
            f if !f.starts_with('-') && file.is_none() => file = Some(f.to_string()),
            other => return Err(format!("unexpected {other}")),
        }
    }
    let file = file.ok_or("usage: las midi <project.las> [-o out.mid] [--pattern [id]]")?;
    let b = load(&file)?;
    let out = out.unwrap_or_else(|| file.trim_end_matches(".las").to_string() + ".mid");
    let source = match &pattern {
        Some(id) => las_engine::export::MidiSource::Pattern(id.as_deref()),
        None => las_engine::export::MidiSource::Song,
    };
    let bytes = las_engine::export::export_midi(&b.project, source);
    std::fs::write(&out, &bytes).map_err(|e| format!("{out}: {e}"))?;
    println!("{out}: {} bytes", bytes.len());
    Ok(())
}
