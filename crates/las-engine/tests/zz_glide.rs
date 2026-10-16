use las_dsp::param::Param;
#[test]
fn glide() {
    let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(std::env::var("G").unwrap()).unwrap()).unwrap();
    let out: Vec<f32> = v["out"].as_array().unwrap().iter().map(|x| x.as_f64().unwrap() as f32).collect();
    let mut p = Param::new(0.0);
    let sr = 24000.0;
    let mut by: std::collections::BTreeMap<usize, Vec<(i64, f64, f64)>> = Default::default();
    for e in v["ev"].as_array().unwrap() {
        let k = e[0].as_i64().unwrap();
        let t = e[2].as_f64().unwrap();
        let frame = ((t * sr / 128.0) + 0.5).floor() as usize * 128;
        by.entry(frame).or_default().push((k, e[1].as_f64().unwrap(), t));
    }
    let total = out.len();
    let mut got = vec![0f32; total];
    let mut f = 0;
    while f < total {
        if let Some(list) = by.get(&f) {
            for &(k, val, _t) in list {
                if k == 1 && f > 0 { p.set_target_at_time(val, f as f64 / sr, 0.015); } else { p = Param::new(val); }
            }
        }
        let n = 128.min(total - f);
        p.fill(f, &mut got[f..f + n], sr);
        f += n;
    }
    let mut worst = (0usize, 0f64);
    for i in 0..total {
        let d = ((got[i] - out[i]) as f64).abs();
        if d > worst.1 { worst = (i, d); }
    }
    println!("worst at frame {} err {} web {} rust {}", worst.0, worst.1, out[worst.0], got[worst.0]);
    let mut shown = 0;
    for i in 0..total {
        let rel = f64::from((got[i] - out[i]).abs()) / f64::from(out[i].abs()).max(1e-9);
        if rel > 1e-6 && shown < 12 { println!("dev {i}: web {} rust {} rel {:e}", out[i], got[i], rel); shown += 1; }
    }
    for i in [6016usize, 12032, 18048, 24064] { println!("{i}: web {} rust {}", out[i], got[i]); }
}
