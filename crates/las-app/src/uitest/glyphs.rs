// SPDX-License-Identifier: GPL-3.0-or-later
//! Every symbol the app's text uses has a glyph in the fonts it ships, so none shows as
//! an empty box (as the effect slots' on/off lights once did).

use super::Harness;
use eframe::egui::FontId;

/// The characters outside ASCII written in the string literals of a source file, with
/// their line numbers: `\u{..}` escapes and the characters themselves.
fn symbols(src: &str) -> Vec<(usize, char)> {
    let mut out = Vec::new();
    for (n, line) in src.lines().enumerate() {
        let code = line.trim_start();
        if code.starts_with("//") {
            continue;
        }
        // only what is inside double quotes
        let mut inside = false;
        let mut chars = line.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '\\' if inside => {
                    if chars.peek() == Some(&'u') {
                        chars.next();
                        if chars.peek() == Some(&'{') {
                            chars.next();
                            let hex: String = chars.by_ref().take_while(|c| *c != '}').collect();
                            if let Some(ch) =
                                u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32)
                            {
                                out.push((n + 1, ch));
                            }
                        }
                    } else {
                        chars.next();
                    }
                }
                '"' => inside = !inside,
                '/' if !inside && chars.peek() == Some(&'/') => break,
                c if inside && !c.is_ascii() => out.push((n + 1, c)),
                _ => {}
            }
        }
    }
    out
}

#[test]
fn every_symbol_in_the_interface_has_a_glyph() {
    let h = Harness::new();
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    let mut stack = vec![dir];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "rs")
                && !p.to_string_lossy().contains("uitest")
            {
                files.push(p);
            }
        }
    }
    let mut missing = Vec::new();
    for f in files {
        let src = std::fs::read_to_string(&f).unwrap();
        for (line, c) in symbols(&src) {
            // control and joiner characters draw nothing
            if c.is_control() || matches!(c, '\u{200b}'..='\u{200f}' | '\u{fe0e}' | '\u{fe0f}') {
                continue;
            }
            let ok = h
                .ctx
                .fonts_mut(|fo| fo.has_glyph(&FontId::proportional(14.0), c));
            if !ok {
                missing.push(format!(
                    "{}:{line}: {c:?} (U+{:04X})",
                    f.file_name().unwrap().to_string_lossy(),
                    c as u32
                ));
            }
        }
    }
    assert!(missing.is_empty(), "no glyph for: {missing:#?}");
}

#[test]
fn every_preset_and_sample_name_has_its_glyphs() {
    let h = Harness::new();
    let mut names: Vec<String> = Vec::new();
    for id in [
        "polaris", "vortex", "monolith", "zenith", "triosc", "facet", "mist", "nebula",
    ] {
        if let Some(b) = las_dsp::presets::bank(id) {
            names.push(b.name.clone());
            for p in &b.presets {
                names.push(p.name.clone());
                names.push(p.category.clone());
            }
        }
    }
    for s in las_engine::factory::FACTORY_SAMPLES {
        names.push(s.name.to_string());
        names.push(s.folder.to_string());
    }
    let missing: Vec<_> = names
        .iter()
        .filter(|n| {
            !h.ctx
                .fonts_mut(|fo| fo.has_glyphs(&FontId::proportional(14.0), n))
        })
        .collect();
    assert!(
        missing.is_empty(),
        "names with characters the fonts lack: {missing:?}"
    );
}
