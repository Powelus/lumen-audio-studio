use las_engine::{render_mix, Mode, RenderOptions};
use las_project::decode;
#[test]
fn dry() {
    let path = format!("{}/tests/fixtures/fxa_timed.las", env!("CARGO_MANIFEST_DIR"));
    let mut b = decode(&std::fs::read(path).unwrap()).unwrap();
    for s in b.project.mixer[1].slots.iter_mut() { *s = None; }
    b.project.master_volume = 1.0;
    let r = render_mix(&b.project, &RenderOptions { sample_rate: 24000.0, tail: 0.5, mode: Mode::Song }).unwrap();
    let j = serde_json::json!({ "l": r.left, "r": r.right });
    std::fs::write(format!("{}/dry.json", std::env::var("S").unwrap()), j.to_string()).unwrap();
}
#[test]
fn full() {
    let path = format!("{}/tests/fixtures/fxa_timed.las", env!("CARGO_MANIFEST_DIR"));
    let b = decode(&std::fs::read(path).unwrap()).unwrap();
    let r = render_mix(&b.project, &RenderOptions { sample_rate: 24000.0, tail: 0.5, mode: Mode::Song }).unwrap();
    let mut bytes = Vec::new();
    for v in r.left.iter().chain(r.right.iter()) { bytes.extend_from_slice(&v.to_le_bytes()); }
    std::fs::write(format!("{}/rustfull.f32", std::env::var("S").unwrap()), bytes).unwrap();
}
