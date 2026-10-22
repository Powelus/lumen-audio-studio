// SPDX-License-Identifier: GPL-3.0-or-later
//! The project model, as the web version saves it (project version 2).
//!
//! Timing is in ticks: a quarter note is `PPQ` ticks. Every struct keeps the
//! fields it does not know in `extra`, so a project saved by a newer build of
//! either version round-trips without losing anything.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

pub const PPQ: u32 = 96;
pub const STEPS_PER_BEAT: u32 = 4;
pub const TICKS_PER_STEP: u32 = PPQ / STEPS_PER_BEAT;
/// Mixer inserts besides the master.
pub const MIXER_INSERTS: usize = 24;
/// Effect slots per insert.
pub const MIXER_SLOTS: usize = 10;
/// Bump together with a new step in `migrate`.
pub const PROJECT_VERSION: u32 = 2;

pub type Id = String;
pub type Extra = Map<String, Value>;

fn yes() -> bool {
    true
}
fn is_false(b: &bool) -> bool {
    !*b
}

/// Gains (dB) of an insert's built-in EQ bands.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct InsertEq {
    pub low: f64,
    pub mid: f64,
    pub high: f64,
}

impl InsertEq {
    /// Frequencies of the bands, Hz.
    pub const FREQS: [f64; 3] = [100.0, 1000.0, 8000.0];
    /// The most a band boosts or cuts, dB.
    pub const RANGE: f64 = 18.0;

    pub fn is_flat(&self) -> bool {
        self.low == 0.0 && self.mid == 0.0 && self.high == 0.0
    }

    pub fn gains(&self) -> [f64; 3] {
        [self.low, self.mid, self.high]
    }
}

fn one() -> f64 {
    1.0
}
fn rest_velocity() -> f64 {
    0.78
}
fn root_key() -> i32 {
    60
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Note {
    pub id: Id,
    /// MIDI key; 60 is C5 in the web version's naming.
    pub key: i32,
    /// Start in ticks, from the pattern start.
    pub pos: f64,
    pub len: f64,
    /// 0 to 1.
    #[serde(default = "rest_velocity")]
    pub vel: f64,
    /// -1 to 1.
    pub pan: f64,
    /// Fine pitch, cents.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fine: Option<f64>,
    /// Release velocity, 0 to 1 (FL's note release); 0.5 when absent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub release: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub slide: Option<bool>,
    /// Entered with the step sequencer.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step: Option<bool>,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct VariationOptions {
    pub seed: f64,
    pub timing: f64,
    pub velocity: f64,
    pub density: f64,
    pub pitch: f64,
    pub swing: f64,
    pub rotate: f64,
    pub channels: Vec<Id>,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Variation {
    pub of: Id,
    pub options: VariationOptions,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Pattern {
    pub id: Id,
    pub name: String,
    pub color: String,
    /// Notes by channel id.
    pub notes: std::collections::BTreeMap<Id, Vec<Note>>,
    /// Ticks; 0 takes the length from the content (rounded to a bar).
    pub length: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub variation: Option<Variation>,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Snapshot {
    pub id: Id,
    pub name: String,
    pub time: f64,
    pub state: Value,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Channel {
    pub id: Id,
    pub name: String,
    pub color: String,
    /// `instrument`, `sampler` or `audioclip`.
    pub kind: String,
    /// The instrument, for example `polaris` or `sampler`.
    #[serde(rename = "pluginId")]
    pub plugin_id: String,
    /// The instrument's patch or settings.
    pub state: Value,
    /// Linear gain, 1 is unity, at most 1.25.
    #[serde(default = "one")]
    pub volume: f64,
    pub pan: f64,
    /// Semitones.
    pub pitch: f64,
    /// Mixer insert (0 is the master).
    #[serde(rename = "mixerTrack")]
    pub mixer_track: usize,
    pub mute: bool,
    #[serde(rename = "sampleId", skip_serializing_if = "Option::is_none")]
    pub sample_id: Option<Id>,
    #[serde(rename = "rootKey", default = "root_key")]
    pub root_key: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    /// Swing of this channel's steps (0 to 1); none follows the project's swing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub swing: Option<f64>,
    /// Sounds pinned from the sound-design history.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snapshots: Option<Vec<Snapshot>>,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SampleRef {
    pub id: Id,
    pub name: String,
    /// Where the file came from.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Seconds, when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration: Option<f64>,
    /// Factory samples are generated and never stored.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub factory: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub missing: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PlaylistTrack {
    pub id: Id,
    pub name: String,
    pub color: String,
    pub mute: bool,
    /// Only soloed tracks play while any track is soloed.
    #[serde(skip_serializing_if = "is_false")]
    pub solo: bool,
    pub height: f64,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ClipKind {
    #[default]
    Pattern,
    Audio,
    Automation,
}

impl ClipKind {
    pub fn name(self) -> &'static str {
        match self {
            Self::Pattern => "pattern",
            Self::Audio => "audio",
            Self::Automation => "automation",
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Take {
    pub group: Id,
    pub index: f64,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StretchMode {
    Stretch,
    Resample,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Stretch {
    pub bpm: f64,
    pub mode: StretchMode,
}

/// Non-destructive edits of one audio clip.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioClipEdit {
    /// dB.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gain: Option<f64>,
    /// Ticks.
    #[serde(rename = "fadeIn", skip_serializing_if = "Option::is_none")]
    pub fade_in: Option<f64>,
    #[serde(rename = "fadeOut", skip_serializing_if = "Option::is_none")]
    pub fade_out: Option<f64>,
    /// -1 to 1: 0 linear, above 0 starts slowly, below 0 quickly.
    #[serde(rename = "fadeInCurve", skip_serializing_if = "Option::is_none")]
    pub fade_in_curve: Option<f64>,
    #[serde(rename = "fadeOutCurve", skip_serializing_if = "Option::is_none")]
    pub fade_out_curve: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reverse: Option<bool>,
    /// Semitones, timing unchanged.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pitch: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stretch: Option<Stretch>,
    /// Bring the loudest peak of the source to 0 dB (before the clip gain).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub normalize: Option<bool>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Clip {
    pub id: Id,
    pub kind: ClipKind,
    /// A pattern, an audio channel or an automation clip, by `kind`.
    #[serde(rename = "refId")]
    pub ref_id: Id,
    pub track: usize,
    pub start: f64,
    pub length: f64,
    /// Into the source, ticks.
    pub offset: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub muted: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub take: Option<Take>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio: Option<AudioClipEdit>,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AutomationPoint {
    /// Ticks from the clip start.
    pub t: f64,
    /// 0 to 1.
    pub v: f64,
    /// The shape of the curve from this point to the next, as FL's point menu.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub curve: Option<CurveKind>,
    /// How far the curve to the next point bends, -1 to 1 (0 is straight).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tension: Option<f64>,
    #[serde(flatten)]
    pub extra: Extra,
}

/// The shape of an automation curve between two points (FL's point modes).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CurveKind {
    /// One bend; straight without tension.
    #[default]
    Single,
    /// An S: bends one way, then the other.
    Double,
    /// Stays at this point's value until the next point.
    Hold,
    /// Steps up or down; tension sets how many.
    Stairs,
    /// Eases in and out.
    Smooth,
}

impl CurveKind {
    pub const ALL: [CurveKind; 5] = [
        Self::Single,
        Self::Double,
        Self::Hold,
        Self::Stairs,
        Self::Smooth,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Single => "Single curve",
            Self::Double => "Double curve",
            Self::Hold => "Hold",
            Self::Stairs => "Stairs",
            Self::Smooth => "Smooth",
        }
    }

    /// How far (0 to 1) the value has gone from one point to the next at `k` (0 to 1)
    /// of the way between them.
    pub fn shape(self, tension: f64, k: f64) -> f64 {
        let k = k.clamp(0.0, 1.0);
        let t = tension.clamp(-1.0, 1.0);
        let e = 4f64.powf(t);
        match self {
            Self::Single => k.powf(e),
            Self::Double => {
                if k < 0.5 {
                    0.5 * (2.0 * k).powf(e)
                } else {
                    1.0 - 0.5 * (2.0 * (1.0 - k)).powf(e)
                }
            }
            Self::Hold => {
                if k >= 1.0 {
                    1.0
                } else {
                    0.0
                }
            }
            Self::Stairs => {
                let n = Self::stairs(t) as f64;
                ((k * n).floor() / (n - 1.0)).min(1.0)
            }
            Self::Smooth => (1.0 - (std::f64::consts::PI * k).cos()) / 2.0,
        }
    }

    /// The number of steps of a stairs curve with this tension (2 to 16).
    pub fn stairs(tension: f64) -> u32 {
        (2.0 + (tension.clamp(-1.0, 1.0) + 1.0) * 7.0).round() as u32
    }

    /// Whether the curve is a straight line with this tension.
    pub fn is_straight(self, tension: f64) -> bool {
        self == Self::Single && tension.abs() < 1e-9
    }
}

/// What an automation clip drives.
#[derive(Clone, Debug, PartialEq)]
pub enum AutomationTarget {
    Mixer {
        track: usize,
        param: String,
    },
    Channel {
        channel_id: Id,
        param: String,
    },
    Plugin {
        channel_id: Id,
        param: String,
    },
    Effect {
        track: usize,
        slot_id: Id,
        param: String,
    },
    Master {
        param: String,
    },
}

impl AutomationTarget {
    /// Read a saved target, or `None` if it is not one the model knows.
    pub fn from_value(v: &Value) -> Option<Self> {
        let s = |k: &str| v.get(k)?.as_str().map(str::to_string);
        let t = || v.get("track")?.as_u64().map(|n| n as usize);
        let param = s("param")?;
        Some(match v.get("kind")?.as_str()? {
            "mixer" => Self::Mixer { track: t()?, param },
            "channel" => Self::Channel {
                channel_id: s("channelId")?,
                param,
            },
            "plugin" => Self::Plugin {
                channel_id: s("channelId")?,
                param,
            },
            "effect" => Self::Effect {
                track: t()?,
                slot_id: s("slotId")?,
                param,
            },
            "master" => Self::Master { param },
            _ => return None,
        })
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AutomationClip {
    pub id: Id,
    pub name: String,
    pub color: String,
    /// Kept as saved; see [`AutomationTarget::from_value`].
    pub target: Value,
    pub points: Vec<AutomationPoint>,
    pub length: f64,
    #[serde(flatten)]
    pub extra: Extra,
}

impl AutomationClip {
    pub fn target(&self) -> Option<AutomationTarget> {
        AutomationTarget::from_value(&self.target)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EffectSlot {
    pub id: Id,
    #[serde(rename = "pluginId")]
    pub plugin_id: String,
    #[serde(default = "yes")]
    pub enabled: bool,
    /// Dry and wet, 0 to 1.
    #[serde(default = "one")]
    pub mix: f64,
    pub state: Value,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct MixSend {
    /// Destination insert.
    pub target: usize,
    /// Linear gain, 0 to 1.25.
    pub amount: f64,
    /// Only feeds the destination's sidechain inputs.
    #[serde(rename = "sidechainOnly")]
    pub sidechain_only: bool,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct MixerTrack {
    pub id: Id,
    pub name: String,
    pub color: String,
    #[serde(default = "one")]
    pub volume: f64,
    pub pan: f64,
    /// 0 (mono) to 2 (extra wide); 1 leaves the stereo image alone.
    #[serde(default = "one")]
    pub width: f64,
    pub mute: bool,
    pub solo: bool,
    /// Insert effects, `MIXER_SLOTS` long, `None` for an empty slot.
    pub slots: Vec<Option<EffectSlot>>,
    pub sends: Vec<MixSend>,
    /// Where the insert plays to; `None` on the master.
    pub output: Option<usize>,
    #[serde(rename = "fxBypass")]
    pub fx_bypass: bool,
    /// The insert's built-in three-band EQ, like FL's: gains in dB of a low shelf
    /// (100 Hz), a peak (1 kHz) and a high shelf (8 kHz). Absent means flat.
    #[serde(skip_serializing_if = "InsertEq::is_flat")]
    pub eq: InsertEq,
    /// Invert the polarity (phase) of both sides.
    #[serde(skip_serializing_if = "is_false")]
    pub invert: bool,
    /// Swap the left and right sides.
    #[serde(rename = "swapLR", skip_serializing_if = "is_false")]
    pub swap: bool,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ProjectMeta {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub genre: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comments: Option<String>,
    #[serde(rename = "createdAt")]
    pub created_at: f64,
    #[serde(rename = "updatedAt")]
    pub updated_at: f64,
    #[serde(rename = "importedFrom", skip_serializing_if = "Option::is_none")]
    pub imported_from: Option<String>,
    /// Analysis of a reference track.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reference: Option<Value>,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Playlist {
    pub tracks: Vec<PlaylistTrack>,
    pub clips: Vec<Clip>,
}

impl Playlist {
    /// For each track (as many as the tracks listed or used by a clip), whether it is
    /// silent: muted, or not soloed while another track is.
    pub fn silent_tracks(&self) -> Vec<bool> {
        let used = self.clips.iter().map(|c| c.track + 1).max().unwrap_or(0);
        let n = self.tracks.len().max(used);
        let any_solo = self.tracks.iter().any(|t| t.solo);
        (0..n)
            .map(|i| match self.tracks.get(i) {
                Some(t) => t.mute || (any_solo && !t.solo),
                None => any_solo,
            })
            .collect()
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Marker {
    pub id: Id,
    /// Ticks.
    pub tick: f64,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct MidiMapping {
    pub id: Id,
    /// 0 to 15.
    pub channel: u8,
    /// 0 to 127.
    pub cc: u8,
    pub target: Value,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Project {
    pub version: u32,
    pub id: Id,
    pub name: String,
    pub bpm: f64,
    #[serde(rename = "timeSig")]
    pub time_sig: (u32, u32),
    /// Global swing, 0 to 1.
    pub swing: f64,
    #[serde(rename = "masterVolume")]
    pub master_volume: f64,
    /// Cents.
    #[serde(rename = "masterPitch")]
    pub master_pitch: f64,
    pub channels: Vec<Channel>,
    pub patterns: Vec<Pattern>,
    pub playlist: Playlist,
    pub automation: Vec<AutomationClip>,
    pub mixer: Vec<MixerTrack>,
    pub samples: Vec<SampleRef>,
    pub meta: ProjectMeta,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub markers: Option<Vec<Marker>>,
    #[serde(rename = "midiMap", skip_serializing_if = "Option::is_none")]
    pub midi_map: Option<Vec<MidiMapping>>,
    #[serde(flatten)]
    pub extra: Extra,
}

impl Project {
    pub fn channel(&self, id: &str) -> Option<&Channel> {
        self.channels.iter().find(|c| c.id == id)
    }

    pub fn pattern(&self, id: &str) -> Option<&Pattern> {
        self.patterns.iter().find(|p| p.id == id)
    }
}
