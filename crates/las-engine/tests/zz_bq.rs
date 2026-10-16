use las_dsp::fx::native::WebBiquad;
use las_dsp::param::Param;
#[test]
fn bq() {
    for file in ["bq0", "bq1"] {
        let path = format!("{}/{file}.json", std::env::var("S").unwrap());
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let g: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(format!("{}/glide.json", std::env::var("S").unwrap())).unwrap()).unwrap();
        let arr = |k: &str| -> Vec<f32> { v[k].as_array().unwrap().iter().map(|x| x.as_f64().unwrap() as f32).collect() };
        let (input, out) = (arr("input"), arr("out"));
        let sr = 24000.0;
        let mut by: std::collections::BTreeMap<usize, Vec<(i64, f64)>> = Default::default();
        for e in g["ev"].as_array().unwrap() {
            let t = e[2].as_f64().unwrap();
            by.entry(((t * sr / 128.0) + 0.5).floor() as usize * 128).or_default().push((e[0].as_i64().unwrap(), e[1].as_f64().unwrap()));
        }
        let mut p = Param::new(0.0);
        let mut bq = WebBiquad::default();
        let total = input.len();
        let mut got = vec![0f32; total];
        let mut fr = vec![0f32; 128];
        let mut f = 0;
        while f < total {
            if let Some(list) = by.get(&f) {
                for &(k, val) in list {
                    if k == 1 && f > 0 { p.set_target_at_time(val, f as f64 / sr, 0.015); } else { p = Param::new(val); }
                }
            }
            let n = 128.min(total - f);
            p.fill(f, &mut fr[..n], sr);
            for i in 0..n {
                bq.set("lowpass", f64::from(fr[i]), 0.5, 0.0, sr);
                got[f + i] = bq.run(f64::from(input[f + i])) as f32;
            }
            f += n;
        }
        let mut worst = (0usize, 0f64);
        let mut sig = 0f64;
        for i in 0..total { let d = f64::from((got[i] - out[i]).abs()); if d > worst.1 { worst = (i, d); } sig = sig.max(f64::from(out[i].abs())); }
        println!("{file}: worst at {} err {:e} (signal peak {sig})", worst.0, worst.1);
    }
}

#[test]
fn impulse() {
    let s = std::env::var("S").unwrap();
    let g: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(format!("{s}/glide.json")).unwrap()).unwrap();
    let imp: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(format!("{s}/imp2.json")).unwrap()).unwrap();
    let sr = 24000.0;
    let mut by: std::collections::BTreeMap<usize, Vec<(i64, f64)>> = Default::default();
    for e in g["ev"].as_array().unwrap() {
        let t = e[2].as_f64().unwrap();
        by.entry(((t * sr / 128.0) + 0.5).floor() as usize * 128).or_default().push((e[0].as_i64().unwrap(), e[1].as_f64().unwrap()));
    }
    let total = 24576;
    let mut p = Param::new(0.0);
    let mut fr = vec![0f32; total];
    let mut f = 0;
    while f < total {
        if let Some(list) = by.get(&f) {
            for &(k, val) in list { if k == 1 && f > 0 { p.set_target_at_time(val, f as f64 / sr, 0.015); } else { p = Param::new(val); } }
        }
        let n = 128.min(total - f);
        p.fill(f, &mut fr[f..f + n], sr);
        f += n;
    }
    for (k, w) in imp.as_object().unwrap() {
        let n: usize = k.parse().unwrap();
        let mut bq = WebBiquad::default();
        let w = w.as_array().unwrap();
        let mut worst = 0f64; let mut peak = 0f64;
        for i in 0..400 {
            bq.set("lowpass", f64::from(fr[n + i]), 0.5, 0.0, sr);
            let y = bq.run(if i == 0 { 1.0 } else { 0.0 });
            let wv = w[i].as_f64().unwrap();
            worst = worst.max((y - wv).abs()); peak = peak.max(wv.abs());
        }
        println!("{n}: worst {worst:e} of peak {peak:e}");
    }
}

#[test]
fn saw() {
    let s = std::env::var("S").unwrap();
    let g: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(format!("{s}/{}.json", std::env::var("EV").unwrap_or("glide".into()))).unwrap()).unwrap();
    let d: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(format!("{s}/{}.json", std::env::var("DRY").unwrap_or("dry".into()))).unwrap()).unwrap();
    let ch = std::env::var("CH").unwrap_or("l".into());
    let il: Vec<f32> = d[ch.as_str()].as_array().unwrap().iter().map(|x| x.as_f64().unwrap() as f32).collect();
    let bytes = std::fs::read(format!("{s}/{}", std::env::var("WEB").unwrap_or("standalone.f32".into()))).unwrap();
    let web_all: Vec<f32> = bytes.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
    let nfr = d["l"].as_array().unwrap().len();
    let web: Vec<f32> = if ch == "r" { web_all[nfr..].to_vec() } else { web_all };
    let sr = 24000.0;
    let mut by: std::collections::BTreeMap<usize, Vec<(i64, f64)>> = Default::default();
    for e in g["ev"].as_array().unwrap() {
        let t = e[2].as_f64().unwrap();
        by.entry(((t * sr / 128.0) + 0.5).floor() as usize * 128).or_default().push((e[0].as_i64().unwrap(), e[1].as_f64().unwrap()));
    }
    let total = 48000;
    let mut p = Param::new(0.0);
    let mut bq = WebBiquad::default();
    let mut fr = vec![0f32; 128];
    let mut f = 0;
    let mut worst = (0usize, 0f64);
    let mut first_dev: Option<usize> = None;
    while f < total {
        if let Some(list) = by.get(&f) {
            for &(k, val) in list { if k == 1 && f > 0 { p.set_target_at_time(val, f as f64 / sr, 0.015); } else { p = Param::new(val); } }
        }
        p.fill(f, &mut fr, sr);
        for i in 0..128 {
            let kfrom: usize = std::env::var("KFROM").ok().and_then(|x| x.parse().ok()).unwrap_or(usize::MAX);
            let fv = if f >= kfrom { fr[0] } else { fr[i] };
            bq.set("lowpass", f64::from(fv), 0.5, 0.0, sr);
            let y = (bq.run(f64::from(il[f + i])) as f32) * 0.9;
            let dd = f64::from((y - web[f + i]).abs());
            if dd > worst.1 { worst = (f + i, dd); }
            if dd > 2e-5 && first_dev.is_none() { first_dev = Some(f + i); }
            if false { println!("{} diff {:e} in {:e} web {:e} rust {:e}", f + i, f64::from(web[f + i] - y), il[f + i], web[f + i], y); }
            if false { println!("{} rust {} web {} in {} freq {}", f + i, y, web[f + i], il[f + i], fr[i]); }
        }
        f += 128;
    }
    println!("saw worst at {} err {:e} first dev {first_dev:?}", worst.0, worst.1);
}

#[test]
fn synth() {
    let s = std::env::var("S").unwrap();
    let g: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(format!("{s}/glide.json")).unwrap()).unwrap();
    for file in ["sine", "saw2"] {
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(format!("{s}/{file}.json")).unwrap()).unwrap();
        let arr = |k: &str| -> Vec<f32> { v[k].as_array().unwrap().iter().map(|x| x.as_f64().unwrap() as f32).collect() };
        let (input, web) = (arr("input"), arr("out"));
        let sr = 24000.0;
        let mut by: std::collections::BTreeMap<usize, Vec<(i64, f64)>> = Default::default();
        for e in g["ev"].as_array().unwrap() {
            let t = e[2].as_f64().unwrap();
            by.entry(((t * sr / 128.0) + 0.5).floor() as usize * 128).or_default().push((e[0].as_i64().unwrap(), e[1].as_f64().unwrap()));
        }
        let total = input.len();
        let mut p = Param::new(0.0);
        let mut bq = WebBiquad::default();
        let mut fr = vec![0f32; 128];
        let mut f = 0;
        let mut first = None;
        let mut worst = 0f64;
        while f < total {
            if let Some(list) = by.get(&f) {
                for &(k, val) in list { if k == 1 && f > 0 { p.set_target_at_time(val, f as f64 / sr, 0.015); } else { p = Param::new(val); } }
            }
            p.fill(f, &mut fr, sr);
            for i in 0..128 {
                bq.set("lowpass", f64::from(fr[i]), 0.5, 0.0, sr);
                let y = bq.run(f64::from(input[f + i])) as f32;
                let dd = f64::from((y - web[f + i]).abs());
                worst = worst.max(dd);
                if dd > 1e-6 && first.is_none() { first = Some(f + i); }
            }
            f += 128;
        }
        println!("{file}: first diff > 1e-6 at {first:?}, worst {worst:e}");
    }
}

#[test]
fn stepresp() {
    let s = std::env::var("S").unwrap();
    let g: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(format!("{s}/evA.json")).unwrap()).unwrap();
    for file in ["stepA", "stepB"] {
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(format!("{s}/{file}.json")).unwrap()).unwrap();
        let arr = |k: &str| -> Vec<f32> { v[k].as_array().unwrap().iter().map(|x| x.as_f64().unwrap() as f32).collect() };
        let (input, web) = (arr("input"), arr("out"));
        let sr = 24000.0;
        let mut by: std::collections::BTreeMap<usize, Vec<(i64, f64)>> = Default::default();
        for e in g["ev"].as_array().unwrap() {
            let t = e[2].as_f64().unwrap();
            by.entry(((t * sr / 128.0) + 0.5).floor() as usize * 128).or_default().push((e[0].as_i64().unwrap(), e[1].as_f64().unwrap()));
        }
        let total = input.len();
        let mut p = Param::new(0.0);
        let mut bq = WebBiquad::default();
        let mut fr = vec![0f32; 128];
        let mut f = 0;
        let mut worst = (0usize, 0f64);
        while f < total {
            if let Some(list) = by.get(&f) {
                for &(k, val) in list { if k == 1 && f > 0 { p.set_target_at_time(val, f as f64 / sr, 0.015); } else { p = Param::new(val); } }
            }
            p.fill(f, &mut fr, sr);
            for i in 0..128 {
                bq.set("lowpass", f64::from(fr[i]), 0.5, 0.0, sr);
                let y = bq.run(f64::from(input[f + i])) as f32;
                let dd = f64::from((y - web[f + i]).abs());
                if dd > worst.1 { worst = (f + i, dd); }
                if (file == "stepA" && (f + i) % 16 == 0 && f + i >= 1024 && f + i < 1200) { println!("{} rust {:e} web {:e} diff {:e}", f + i, y, web[f + i], dd); }
            }
            f += 128;
        }
        println!("{file}: worst at {} err {:e}", worst.0, worst.1);
    }
}

#[test]
fn positions() {
    let s = std::env::var("S").unwrap();
    let imp: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(format!("{s}/imp3.json")).unwrap()).unwrap();
    let sr = 24000.0;
    let b0 = |f: f64| -> f64 { let mut bq = WebBiquad::default(); bq.set("lowpass", f, 0.5, 0.0, sr); bq.run(1.0) };
    let mut keys: Vec<usize> = imp.as_object().unwrap().keys().map(|k| k.parse().unwrap()).collect();
    keys.sort();
    for n in keys {
        let w = imp[n.to_string()].as_array().unwrap();
        for (side, name) in [(0, "L"), (1, "R")] {
            let target = w[side].as_f64().unwrap();
            let (mut lo, mut hi) = (50.0f64, 200.0f64);
            for _ in 0..60 { let mid = 0.5 * (lo + hi); if b0(mid) < target { lo = mid } else { hi = mid } }
            let f = 0.5 * (lo + hi);
            let (a, t) = (79.62143411069945f64, 92.05131471627121f64);
            let pos = if f > a { 768.0 - 360.0 * ((t - f) / (t - a)).ln() } else { f64::NAN };
            println!("n {n} {name}: f {f:.4} position {:.1} (frames after event {:.1}) vs n-768 = {}", pos, pos - 768.0, n as i64 - 768);
        }
    }
}

#[test]
fn stereo_params() {
    let s = std::env::var("S").unwrap();
    let g: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(format!("{s}/{}.json", std::env::var("EV").unwrap_or("evA".into()))).unwrap()).unwrap();
    let imp: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(format!("{s}/{}.json", std::env::var("IMP").unwrap_or("imp3".into()))).unwrap()).unwrap();
    let sr = 24000.0;
    let mut by: std::collections::BTreeMap<usize, Vec<(i64, f64)>> = Default::default();
    for e in g["ev"].as_array().unwrap() {
        let t = e[2].as_f64().unwrap();
        by.entry(((t * sr / 128.0) + 0.5).floor() as usize * 128).or_default().push((e[0].as_i64().unwrap(), e[1].as_f64().unwrap()));
    }
    let total = 4096;
    let mut p = Param::new(0.0);
    let (mut fl, mut fr) = (vec![0f32; total], vec![0f32; total]);
    let mut f = 0;
    while f < total {
        if let Some(list) = by.get(&f) {
            for &(k, val) in list { if k == 1 && f > 0 { p.set_target_at_time(val, f as f64 / sr, 0.015); } else { p = Param::new(val); } }
        }
        let (l, r) = (&mut fl[f..f + 128], &mut fr[f..f + 128]);
        p.fill_channels(f, &mut [l, r], sr);
        f += 128;
    }
    let b0 = |fq: f64| -> f64 { let mut bq = WebBiquad::default(); bq.set("lowpass", fq, 0.5, 0.0, sr); bq.run(1.0) };
    let mut keys: Vec<usize> = imp.as_object().unwrap().keys().map(|k| k.parse().unwrap()).collect();
    keys.sort();
    for n in keys {
        let w = imp[n.to_string()].as_array().unwrap();
        let (bl, br) = (b0(f64::from(fl[n])), b0(f64::from(fr[n])));
        println!("n {n}: L rust {bl:e} web {:e} | R rust {br:e} web {:e}", w[0].as_f64().unwrap(), w[1].as_f64().unwrap());
    }
}

#[test]
fn which_pos() {
    let sr = 24000.0;
    let b0 = |fq: f64| -> f64 { let mut bq = WebBiquad::default(); bq.set("lowpass", fq, 0.5, 0.0, sr); bq.run(1.0) };
    let t2 = 106.42165185335884f64;
    // find L's E1 value at 1408 by bisection on web's L b0 (1.419832551619038e-4)
    let solve = |target: f64| { let (mut lo, mut hi) = (50.0f64, 200.0f64); for _ in 0..60 { let m = 0.5*(lo+hi); if b0(m) < target { lo = m } else { hi = m } } 0.5*(lo+hi) };
    let f_l = solve(1.419832551619038e-4);
    let f_r = solve(1.5591274132020772e-4);
    println!("L f at 1408 = {f_l}, R f at 1408 = {f_r}, target2 {t2}");
    for pos in [120.0, 125.0, 126.0, 127.0, 128.0, 129.0, 130.0] {
        let f = t2 + (f_l - t2) * (-pos / 360.0f64).exp();
        println!("pos {pos}: f {f}");
    }
}

#[test]
fn fit_r() {
    let s = std::env::var("S").unwrap();
    let imp: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(format!("{s}/{}.json", std::env::var("IMP").unwrap_or("imp5".into()))).unwrap()).unwrap();
    let sr = 24000.0;
    let b0 = |fq: f64| -> f64 { let mut bq = WebBiquad::default(); bq.set("lowpass", fq, 0.5, 0.0, sr); bq.run(1.0) };
    let solve = |target: f64| { let (mut lo, mut hi) = (50.0f64, 200.0f64); for _ in 0..60 { let m = 0.5*(lo+hi); if b0(m) < target { lo = m } else { hi = m } } 0.5*(lo+hi) };
    let mut keys: Vec<usize> = imp.as_object().unwrap().keys().map(|k| k.parse().unwrap()).collect();
    keys.sort();
    let t2: f64 = std::env::var("T2").ok().and_then(|x| x.parse().ok()).unwrap_or(106.42165185335884);
    for n in keys {
        let w = imp[n.to_string()].as_array().unwrap();
        let (fl, fr) = (solve(w[0].as_f64().unwrap()), solve(w[1].as_f64().unwrap()));
        // implied frames of decay from start values a: (t2 - f) = (t2 - a) * exp(-p/360)
        println!("n {n}: L f {fl:.5}  R f {fr:.5}  ln-dist L {:.5} R {:.5}", (t2 - fl).ln(), (t2 - fr).ln());
    }
}
