use las_dsp::fx::registry::{full_state, parse_params, EffectUnit};
use serde_json::json;
#[test]
fn af() {
    let s = std::env::var("S").unwrap();
    let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(format!("{s}/bq1.json")).unwrap()).unwrap();
    let g: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(format!("{s}/glide.json")).unwrap()).unwrap();
    let arr = |k: &str| -> Vec<f32> { v[k].as_array().unwrap().iter().map(|x| x.as_f64().unwrap() as f32).collect() };
    let (input, out) = (arr("input"), arr("out"));
    let sr = 24000.0;
    let mut by: std::collections::BTreeMap<usize, f64> = Default::default();
    for e in g["ev"].as_array().unwrap() {
        let t = e[2].as_f64().unwrap();
        if e[0].as_i64().unwrap() == 0 && e[1].as_f64().unwrap() == 1500.0 { continue; }
        by.insert(((t * sr / 128.0) + 0.5).floor() as usize * 128, e[1].as_f64().unwrap());
    }
    let mut unit = EffectUnit::new("las-filter", sr).unwrap();
    let mk = |c: f64| parse_params("las-filter", &full_state("las-filter", &json!({"type":"lowpass","cutoff":c,"res":0.5,"lfoRate":0,"lfoDepth":0,"mix":1})), sr).unwrap();
    unit.apply(mk(1500.0));
    let total = input.len();
    let mut got = vec![0f32; total];
    let mut f = 0;
    while f < total {
        unit.set_time(f);
        if let Some(&c) = by.get(&f) { unit.apply(mk(c)); }
        let n = 128.min(total - f);
        let (mut ol, mut or) = (vec![0f32; n], vec![0f32; n]);
        unit.process(&input[f..f + n], &input[f..f + n], &mut ol, &mut or, None);
        got[f..f + n].copy_from_slice(&ol);
        f += n;
    }
    let mut worst = (0usize, 0f64);
    for i in 0..total { let d = f64::from((got[i] - out[i]).abs()); if d > worst.1 { worst = (i, d); } }
    println!("worst at {} err {:e}", worst.0, worst.1);
}

#[test]
fn af_stereo() {
    let s = std::env::var("S").unwrap();
    let d: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(format!("{s}/dry.json")).unwrap()).unwrap();
    let g: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(format!("{s}/glide.json")).unwrap()).unwrap();
    let ld = |k: &str| -> Vec<f32> { d[k].as_array().unwrap().iter().map(|x| x.as_f64().unwrap() as f32).collect() };
    let (il, ir) = (ld("l"), ld("r"));
    let sr = 24000.0;
    let mut by: std::collections::BTreeMap<usize, f64> = Default::default();
    for e in g["ev"].as_array().unwrap() {
        let t = e[2].as_f64().unwrap();
        if e[0].as_i64().unwrap() == 0 && e[1].as_f64().unwrap() == 1500.0 { continue; }
        let off: i64 = std::env::var("OFF").ok().and_then(|x| x.parse().ok()).unwrap_or(0);
        by.insert((((t * sr / 128.0) + 0.5).floor() as i64 * 128 + off).max(0) as usize, e[1].as_f64().unwrap());
    }
    let mut unit = EffectUnit::new("las-filter", sr).unwrap();
    let mk = |c: f64| parse_params("las-filter", &full_state("las-filter", &json!({"type":"lowpass","cutoff":c,"res":0.5,"lfoRate":0,"lfoDepth":0,"mix":1})), sr).unwrap();
    unit.apply(mk(1500.0));
    let total = il.len();
    let mut got = vec![0f32; total];
    let mut f = 0;
    while f < total {
        unit.set_time(f);
        if let Some(&c) = by.get(&f) { unit.apply(mk(c)); }
        let n = (by.range(f + 1..).next().map_or(usize::MAX, |(k, _)| *k - f)).min(128).min(total - f);
        let (mut ol, mut or) = (vec![0f32; n], vec![0f32; n]);
        unit.process(&il[f..f + n], &ir[f..f + n], &mut ol, &mut or, None);
        got[f..f + n].copy_from_slice(&ol);
        f += n;
    }
    let bytes = std::fs::read(format!("{s}/standalone.f32")).unwrap();
    let full: Vec<f32> = bytes.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
    let mut worst = (0usize, 0f64);
    for i in 0..48000 { let d = f64::from((got[i] * 0.9 - full[i]).abs()); if d > worst.1 { worst = (i, d); } }
    println!("stereo worst at {} err {:e}", worst.0, worst.1);
    let mut shown = 0;
    for i in 0..48000 { let d = f64::from((got[i] * 0.9 - full[i]).abs()); if d > 2e-8 && shown < 10 { println!("{i}: rust {} web {} diff {:e}", got[i]*0.9, full[i], d); shown += 1; } }
}

#[test]
fn af_static() {
    let s = std::env::var("S").unwrap();
    let d: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(format!("{s}/dry.json")).unwrap()).unwrap();
    let ld = |k: &str| -> Vec<f32> { d[k].as_array().unwrap().iter().map(|x| x.as_f64().unwrap() as f32).collect() };
    let (il, ir) = (ld("l"), ld("r"));
    let sr = 24000.0;
    let mut unit = EffectUnit::new("las-filter", sr).unwrap();
    let st = json!({"type":"lowpass","cutoff":80.0,"res":0.5,"lfoRate":0,"lfoDepth":0,"mix":1});
    unit.apply(parse_params("las-filter", &full_state("las-filter", &st), sr).unwrap());
    let total = il.len();
    let mut got = vec![0f32; total];
    let mut f = 0;
    while f < total {
        let n = 128.min(total - f);
        unit.set_time(f);
        let (mut ol, mut or) = (vec![0f32; n], vec![0f32; n]);
        unit.process(&il[f..f + n], &ir[f..f + n], &mut ol, &mut or, None);
        got[f..f + n].copy_from_slice(&ol);
        f += n;
    }
    let bytes = std::fs::read(format!("{s}/static80.f32")).unwrap();
    let web: Vec<f32> = bytes.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
    let mut worst = (0usize, 0f64);
    for i in 0..total { let d = f64::from((got[i] * 0.9 - web[i]).abs()); if d > worst.1 { worst = (i, d); } }
    println!("static worst at {} err {:e} web {} rust {}", worst.0, worst.1, web[worst.0], got[worst.0]*0.9);
}
