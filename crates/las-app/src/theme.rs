// SPDX-License-Identifier: GPL-3.0-or-later
//! Colour themes, ported from the web version (`ui/themes.ts`): the same eight
//! themes, each derived from a handful of seed colours with the same formulas.
//! The palette in effect lives in a global so that every widget can ask for it.

use eframe::egui::{self, Color32};
use std::sync::RwLock;

type Rgb = [f64; 3];

fn rgb(c: Color32) -> Rgb {
    [f64::from(c.r()), f64::from(c.g()), f64::from(c.b())]
}

fn hex(s: &str) -> Color32 {
    let h = s.trim_start_matches('#');
    let v = u32::from_str_radix(h, 16).unwrap_or(0x888888);
    Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
}

/// `a` mixed with `b`: `t` 0 is `a`, 1 is `b`.
pub fn mix(a: Color32, b: Color32, t: f64) -> Color32 {
    let (x, y) = (rgb(a), rgb(b));
    let c = |i: usize| (x[i] + (y[i] - x[i]) * t).round().clamp(0.0, 255.0) as u8;
    Color32::from_rgb(c(0), c(1), c(2))
}

/// WCAG relative luminance.
fn luma(c: Color32) -> f64 {
    let f = |v: u8| {
        let c = f64::from(v) / 255.0;
        if c <= 0.03928 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * f(c.r()) + 0.7152 * f(c.g()) + 0.0722 * f(c.b())
}

fn contrast(a: Color32, b: Color32) -> f64 {
    let (x, y) = (luma(a), luma(b));
    (x.max(y) + 0.05) / (x.min(y) + 0.05)
}

/// `fg` nudged toward `toward` until it reads at `min`:1 on every colour in `bgs`.
fn ensure_contrast(fg: Color32, bgs: &[Color32], min: f64, toward: Color32) -> Color32 {
    let mut c = fg;
    for _ in 0..24 {
        if bgs.iter().all(|&b| contrast(c, b) >= min) {
            break;
        }
        c = mix(c, toward, 0.06);
    }
    c
}

/// White or near black, whichever reads best on `bg`.
fn ink_for(bg: Color32) -> Color32 {
    let dark = hex("#14161d");
    if 1.05 / (luma(bg) + 0.05) >= (luma(bg) + 0.05) / (luma(dark) + 0.05) {
        Color32::WHITE
    } else {
        dark
    }
}

/// Every colour the interface uses.
#[derive(Clone, Copy, Debug)]
pub struct Palette {
    pub dark: bool,
    pub bg: Color32,
    pub bg2: Color32,
    pub panel: Color32,
    pub panel2: Color32,
    pub raised: Color32,
    pub raised2: Color32,
    pub border: Color32,
    pub border2: Color32,
    pub text: Color32,
    pub text2: Color32,
    pub muted: Color32,
    pub accent: Color32,
    pub accent2: Color32,
    pub accent_strong: Color32,
    pub accent_text: Color32,
    pub on_accent: Color32,
    pub hot: Color32,
    pub ok: Color32,
    pub warn: Color32,
    pub grid: Color32,
    pub grid_bar: Color32,
    pub step_off: Color32,
    pub step_off_alt: Color32,
    pub well_deep: Color32,
    pub well: Color32,
    pub btn: Color32,
    pub led_off: Color32,
    pub lcd: Color32,
    pub roll_bg: Color32,
    pub roll_row_a: Color32,
    pub roll_row_b: Color32,
    pub key_white: Color32,
    pub key_black: Color32,
}

struct Seed {
    dark: bool,
    bg: &'static str,
    panel: &'static str,
    raised: &'static str,
    border: &'static str,
    text: &'static str,
    muted: &'static str,
    accent: &'static str,
    accent2: &'static str,
    hot: &'static str,
    ok: &'static str,
    warn: &'static str,
    /// Overrides of derived colours, by the web version's variable names.
    over: &'static [(&'static str, &'static str)],
}

fn derive(s: &Seed) -> Palette {
    let (bg, panel, raised, border, text) = (
        hex(s.bg),
        hex(s.panel),
        hex(s.raised),
        hex(s.border),
        hex(s.text),
    );
    let (black, white) = (Color32::BLACK, Color32::WHITE);
    let accent = hex(s.accent);
    let accent2 = hex(s.accent2);
    let dark = s.dark;
    let on_accent = ink_for(accent);
    let well_deep = mix(bg, black, if dark { 0.4 } else { 0.07 });
    let roll_bg = mix(bg, panel, 0.5);
    let panel2 = mix(panel, raised, 0.5);
    let well = mix(well_deep, panel, 0.3);
    let surfaces = [bg, panel, panel2, raised, well_deep, well];
    let step_off = mix(raised, bg, 0.12);
    let mut p = Palette {
        dark,
        bg,
        bg2: mix(bg, panel, 0.5),
        panel,
        panel2,
        raised,
        raised2: mix(raised, text, 0.08),
        border,
        border2: mix(border, text, 0.14),
        text,
        text2: ensure_contrast(mix(text, bg, 0.22), &surfaces, 7.0, text),
        muted: ensure_contrast(hex(s.muted), &surfaces, 4.6, text),
        accent,
        accent2,
        accent_strong: if on_accent == white {
            mix(accent, black, 0.14)
        } else {
            accent
        },
        accent_text: if dark {
            mix(accent, white, 0.45)
        } else {
            mix(accent, black, 0.2)
        },
        on_accent,
        hot: hex(s.hot),
        ok: hex(s.ok),
        warn: hex(s.warn),
        grid: mix(bg, panel, 0.75),
        grid_bar: mix(border, text, 0.16),
        step_off,
        step_off_alt: mix(step_off, accent, 0.14),
        well_deep,
        well,
        btn: mix(raised, panel, 0.55),
        led_off: mix(raised, text, 0.14),
        lcd: if dark {
            mix(accent2, white, 0.35)
        } else {
            mix(accent2, black, 0.3)
        },
        roll_bg,
        roll_row_a: mix(roll_bg, raised, 0.35),
        roll_row_b: mix(roll_bg, raised, 0.1),
        key_white: if dark { mix(text, bg, 0.12) } else { white },
        key_black: if dark {
            mix(bg, black, 0.5)
        } else {
            mix(text, bg, 0.2)
        },
    };
    for (k, v) in s.over {
        let c = hex(v);
        match *k {
            "--well-deep" => p.well_deep = c,
            "--well" => p.well = c,
            "--step-off" => p.step_off = c,
            "--step-off-alt" => p.step_off_alt = c,
            "--roll-bg" => p.roll_bg = c,
            "--roll-row-a" => p.roll_row_a = c,
            "--roll-row-b" => p.roll_row_b = c,
            "--lcd-text" => p.lcd = c,
            "--key-white" => p.key_white = c,
            "--key-black" => p.key_black = c,
            "--grid" => p.grid = c,
            "--grid-bar" => p.grid_bar = c,
            "--border-2" => p.border2 = c,
            "--btn-a" | "--btn-b" => p.btn = c,
            _ => {}
        }
    }
    p
}

/// One of the themes of the web version.
pub struct Theme {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    seed: Option<Seed>,
}

impl Theme {
    pub fn palette(&self) -> Palette {
        match &self.seed {
            Some(s) => derive(s),
            // the built-in look: the web version's stylesheet defaults
            None => Palette {
                dark: true,
                bg: hex("#0a0c12"),
                bg2: hex("#0e1119"),
                panel: hex("#141823"),
                panel2: hex("#191e2c"),
                raised: hex("#20263a"),
                raised2: hex("#283050"),
                border: hex("#262d42"),
                border2: hex("#333c58"),
                text: hex("#e7eaf3"),
                text2: hex("#b8bfd3"),
                muted: hex("#8b94ad"),
                accent: hex("#7c5cff"),
                accent2: hex("#22d3ee"),
                accent_strong: hex("#6a47f5"),
                accent_text: hex("#b7a6ff"),
                on_accent: Color32::WHITE,
                hot: hex("#ff5c8a"),
                ok: hex("#3ddc97"),
                warn: hex("#ffb547"),
                grid: hex("#1a1f2e"),
                grid_bar: hex("#2e3650"),
                step_off: hex("#232a3d"),
                step_off_alt: hex("#2c2440"),
                well_deep: hex("#06080c"),
                well: hex("#0b0e15"),
                btn: hex("#222940"),
                led_off: hex("#3a4255"),
                lcd: hex("#9fe9f7"),
                roll_bg: hex("#0e1119"),
                roll_row_a: hex("#151a28"),
                roll_row_b: hex("#10141f"),
                key_white: hex("#d9dde8"),
                key_black: hex("#06080b"),
            },
        }
    }
}

/// The themes, in the web version's order.
pub fn themes() -> Vec<Theme> {
    vec![
        Theme {
            id: "lumen",
            name: "Lumen Dark",
            description: "The default: deep blue-black with violet and cyan.",
            seed: None,
        },
        Theme {
            id: "fl",
            name: "Orange Steel",
            description: "Gunmetal grey with orange highlights, in the spirit of classic step-sequencer DAWs.",
            seed: Some(Seed {
                dark: true,
                bg: "#2a3036",
                panel: "#363e45",
                raised: "#4a555e",
                border: "#232a2f",
                text: "#e9ecef",
                muted: "#a0abb4",
                accent: "#ff9a1f",
                accent2: "#9ccc4a",
                hot: "#ff5d52",
                ok: "#93d44f",
                warn: "#ffc233",
                over: &[
                    ("--lcd-text", "#ffb84d"),
                    ("--well-deep", "#1e2328"),
                    ("--well", "#262c32"),
                    ("--step-off", "#4a555e"),
                    ("--step-off-alt", "#56626c"),
                    ("--roll-bg", "#2f363c"),
                    ("--roll-row-a", "#3a434a"),
                    ("--roll-row-b", "#323a41"),
                ],
            }),
        },
        Theme {
            id: "daylight",
            name: "Daylight",
            description: "A light theme for bright rooms.",
            seed: Some(Seed {
                dark: false,
                bg: "#e3e7ef",
                panel: "#f3f5f9",
                raised: "#d6dbe8",
                border: "#bcc4d6",
                text: "#1b2133",
                muted: "#5a6580",
                accent: "#5b3ff0",
                accent2: "#0b86a3",
                hot: "#d02a5e",
                ok: "#168f5d",
                warn: "#b86e00",
                over: &[
                    ("--well-deep", "#cfd5e3"),
                    ("--well", "#dde2ed"),
                    ("--step-off", "#cdd3e2"),
                    ("--step-off-alt", "#d6cdee"),
                    ("--roll-bg", "#eef1f7"),
                    ("--roll-row-a", "#f8f9fc"),
                    ("--roll-row-b", "#e5e9f2"),
                    ("--key-white", "#ffffff"),
                    ("--key-black", "#2b3042"),
                    ("--lcd-text", "#075e73"),
                    ("--btn-a", "#e6eaf3"),
                ],
            }),
        },
        Theme {
            id: "synthwave",
            name: "Synthwave",
            description: "Neon pink and cyan on night purple.",
            seed: Some(Seed {
                dark: true,
                bg: "#130b22",
                panel: "#1e1235",
                raised: "#2e1c4e",
                border: "#3f2768",
                text: "#f4e9ff",
                muted: "#a98fd4",
                accent: "#ff2fa5",
                accent2: "#19e6ff",
                hot: "#ff5277",
                ok: "#3dffb2",
                warn: "#ffc93d",
                over: &[],
            }),
        },
        Theme {
            id: "forest",
            name: "Forest",
            description: "Calm greens, easy on the eyes at night.",
            seed: Some(Seed {
                dark: true,
                bg: "#0b130f",
                panel: "#122019",
                raised: "#1c3025",
                border: "#254035",
                text: "#e3efe7",
                muted: "#86a493",
                accent: "#34d399",
                accent2: "#a3e635",
                hot: "#fb7185",
                ok: "#4ade80",
                warn: "#fbbf24",
                over: &[],
            }),
        },
        Theme {
            id: "ocean",
            name: "Ocean",
            description: "Deep sea blues with a bright aqua accent.",
            seed: Some(Seed {
                dark: true,
                bg: "#07121b",
                panel: "#0d2131",
                raised: "#153449",
                border: "#1b4560",
                text: "#e2f2fa",
                muted: "#7ca6bd",
                accent: "#22b8f0",
                accent2: "#5eead4",
                hot: "#ff6b8b",
                ok: "#4ade9a",
                warn: "#fbbf24",
                over: &[],
            }),
        },
        Theme {
            id: "graphite",
            name: "Graphite",
            description: "Neutral greys that keep the colour for your music.",
            seed: Some(Seed {
                dark: true,
                bg: "#111111",
                panel: "#1b1b1b",
                raised: "#2b2b2b",
                border: "#353535",
                text: "#e8e8e8",
                muted: "#9b9b9b",
                accent: "#4c9aff",
                accent2: "#4fd1c5",
                hot: "#ff6b6b",
                ok: "#5fd38d",
                warn: "#f0b429",
                over: &[],
            }),
        },
        Theme {
            id: "contrast",
            name: "High Contrast",
            description: "Maximum contrast for low vision: black, white and yellow.",
            seed: Some(Seed {
                dark: true,
                bg: "#000000",
                panel: "#0b0b0b",
                raised: "#1f1f1f",
                border: "#8c8c8c",
                text: "#ffffff",
                muted: "#d0d0d0",
                accent: "#ffd400",
                accent2: "#4de3ff",
                hot: "#ff6b6b",
                ok: "#57f287",
                warn: "#ffb347",
                over: &[
                    ("--border-2", "#bdbdbd"),
                    ("--grid", "#1a1a1a"),
                    ("--grid-bar", "#6a6a6a"),
                ],
            }),
        },
    ]
}

pub const SYSTEM: &str = "system";
/// The look the app starts in: gunmetal grey with orange, like FL Studio.
pub const DEFAULT: &str = "fl";

static CURRENT: RwLock<Option<Palette>> = RwLock::new(None);

/// Tests that switch the theme take this, so that they do not run into each other.
#[cfg(test)]
pub static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// The palette in effect.
pub fn pal() -> Palette {
    CURRENT.read().ok().and_then(|g| *g).unwrap_or_else(|| {
        themes()
            .into_iter()
            .find(|t| t.id == DEFAULT)
            .map_or_else(|| themes()[0].palette(), |t| t.palette())
    })
}

/// The accent colour of the theme in effect.
pub fn accent() -> Color32 {
    pal().accent
}

/// The theme id for a choice: "system" follows the operating system.
pub fn resolve(choice: &str, system_dark: bool) -> &'static str {
    if choice == SYSTEM {
        return if system_dark { DEFAULT } else { "daylight" };
    }
    themes()
        .into_iter()
        .find(|t| t.id == choice)
        .map_or(DEFAULT, |t| t.id)
}

/// Make a theme the one in effect, and give egui its colours.
pub fn apply(ctx: &egui::Context, id: &str) {
    let t = themes()
        .into_iter()
        .find(|t| t.id == id)
        .unwrap_or_else(|| themes().remove(0));
    let p = t.palette();
    if let Ok(mut g) = CURRENT.write() {
        *g = Some(p);
    }
    let mut v = if p.dark {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    };
    v.panel_fill = p.panel;
    v.window_fill = p.panel2;
    v.extreme_bg_color = p.well_deep;
    v.faint_bg_color = p.bg2;
    v.window_stroke = egui::Stroke::new(1.0, p.border2);
    v.window_corner_radius = egui::CornerRadius::same(8);
    v.menu_corner_radius = egui::CornerRadius::same(6);
    v.selection.bg_fill = p.accent;
    v.selection.stroke = egui::Stroke::new(1.0, p.on_accent);
    v.hyperlink_color = p.accent_text;
    let r = egui::CornerRadius::same(5);
    let w = &mut v.widgets;
    w.noninteractive.bg_fill = p.panel;
    w.noninteractive.bg_stroke = egui::Stroke::new(1.0, p.border);
    w.noninteractive.fg_stroke = egui::Stroke::new(1.0, p.text2);
    w.noninteractive.corner_radius = r;
    w.inactive.bg_fill = p.btn;
    w.inactive.weak_bg_fill = p.btn;
    w.inactive.bg_stroke = egui::Stroke::new(1.0, p.border);
    w.inactive.fg_stroke = egui::Stroke::new(1.0, p.text);
    w.inactive.corner_radius = r;
    w.hovered.bg_fill = p.raised2;
    w.hovered.weak_bg_fill = p.raised2;
    w.hovered.bg_stroke = egui::Stroke::new(1.0, p.border2);
    w.hovered.fg_stroke = egui::Stroke::new(1.5, p.text);
    w.hovered.corner_radius = r;
    w.active.bg_fill = p.accent_strong;
    w.active.weak_bg_fill = p.accent_strong;
    w.active.bg_stroke = egui::Stroke::new(1.0, p.accent);
    w.active.fg_stroke = egui::Stroke::new(1.5, p.text);
    w.active.corner_radius = r;
    w.open.bg_fill = p.raised;
    w.open.weak_bg_fill = p.raised;
    w.open.corner_radius = r;
    ctx.set_visuals(v);
    ctx.global_style_mut(|s| {
        s.spacing.item_spacing = egui::vec2(8.0, 6.0);
    });
}

fn choice_file() -> Option<std::path::PathBuf> {
    Some(crate::presets::config_dir()?.join("theme"))
}

/// The theme choice that was saved ("system" or a theme id).
pub fn saved_choice() -> String {
    choice_file()
        .and_then(|f| std::fs::read_to_string(f).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| s == SYSTEM || themes().iter().any(|t| t.id == s))
        .unwrap_or_else(|| DEFAULT.to_string())
}

/// Remember the theme choice.
pub fn save_choice(choice: &str) {
    if let Some(f) = choice_file() {
        if let Some(d) = f.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let _ = std::fs::write(f, choice);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn there_are_the_eight_themes_of_the_web_version() {
        let ids: Vec<_> = themes().iter().map(|t| t.id).collect();
        assert_eq!(
            ids,
            [
                "lumen",
                "fl",
                "daylight",
                "synthwave",
                "forest",
                "ocean",
                "graphite",
                "contrast"
            ]
        );
    }

    #[test]
    fn every_theme_keeps_its_text_readable() {
        for t in themes() {
            let p = t.palette();
            for bg in [p.bg, p.panel, p.raised] {
                assert!(contrast(p.text, bg) >= 4.5, "{} text on {bg:?}", t.id);
                assert!(contrast(p.muted, bg) >= 3.0, "{} muted on {bg:?}", t.id);
            }
            assert!(contrast(p.on_accent, p.accent) >= 3.0, "{} accent", t.id);
        }
    }

    #[test]
    fn mixing_and_contrast_follow_the_web_formulas() {
        assert_eq!(
            mix(Color32::BLACK, Color32::WHITE, 0.5),
            Color32::from_rgb(128, 128, 128)
        );
        assert_eq!(ink_for(hex("#ffd400")), hex("#14161d"));
        assert_eq!(ink_for(hex("#5b3ff0")), Color32::WHITE);
        assert!(contrast(Color32::BLACK, Color32::WHITE) > 20.0);
    }

    #[test]
    fn system_follows_the_operating_system_and_unknown_ids_fall_back() {
        assert_eq!(resolve(SYSTEM, true), DEFAULT);
        assert_eq!(resolve(SYSTEM, false), "daylight");
        assert_eq!(resolve("synthwave", true), "synthwave");
        assert_eq!(resolve("nope", true), DEFAULT);
        assert_eq!(DEFAULT, "fl", "the default is not violet");
    }

    #[test]
    fn applying_a_theme_changes_the_palette() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let ctx = egui::Context::default();
        apply(&ctx, "synthwave");
        assert_eq!(accent(), hex("#ff2fa5"));
        apply(&ctx, "lumen");
        assert_eq!(accent(), hex("#7c5cff"));
    }
}
