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
