// SPDX-License-Identifier: GPL-3.0-or-later
//! Export: a project as a standard MIDI file, as the web version's `exportMidi`.

use las_project::{ClipKind, Project, PPQ};
use std::collections::BTreeMap;

/// Which part of a project to export.
pub enum MidiSource<'a> {
    Song,
    /// A pattern by id (the first pattern if the id is unknown).
    Pattern(Option<&'a str>),
}

fn vlq(mut n: u32) -> Vec<u8> {
    let mut bytes = vec![(n & 0x7f) as u8];
    n >>= 7;
    while n > 0 {
        bytes.insert(0, ((n & 0x7f) | 0x80) as u8);
        n >>= 7;
    }
    bytes
}

fn chunk(kind: &str, data: &[u8]) -> Vec<u8> {
    let mut out = kind.as_bytes().to_vec();
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(data);
    out
}

struct Ev {
    tick: f64,
    data: Vec<u8>,
}

/// JavaScript's `Math.round`.
fn round(x: f64) -> f64 {
    (x + 0.5).floor()
}

fn track_data(events: Vec<Ev>, name: Option<&str>) -> Vec<u8> {
    let mut sorted = events;
    sorted.sort_by(|a, b| {
        a.tick
            .total_cmp(&b.tick)
            .then((a.data[0] & 0xf0).cmp(&(b.data[0] & 0xf0)))
    });
    let mut out = Vec::new();
    if let Some(name) = name.filter(|n| !n.is_empty()) {
        out.extend_from_slice(&[0, 0xff, 0x03]);
        out.extend(vlq(name.len() as u32));
        out.extend_from_slice(name.as_bytes());
    }
    let mut last = 0.0;
    for e in &sorted {
        out.extend(vlq(round(e.tick - last).max(0.0) as u32));
        out.extend_from_slice(&e.data);
        last = round(e.tick);
    }
    out.extend_from_slice(&[0, 0xff, 0x2f, 0]);
    out
}

struct Played {
    key: i32,
    tick: f64,
    len: f64,
    vel: f64,
}

/// A type 1 standard MIDI file with one track per channel that has notes.
pub fn export_midi(project: &Project, source: MidiSource) -> Vec<u8> {
    let mut per_channel: BTreeMap<String, Vec<Played>> = BTreeMap::new();
    let mut add = |ch: &str, key: i32, tick: f64, len: f64, vel: f64| {
        per_channel.entry(ch.to_string()).or_default().push(Played {
            key,
            tick,
            len: len.max(1.0),
            vel,
        });
    };
    match source {
        MidiSource::Pattern(id) => {
            let pat = id
                .and_then(|i| project.pattern(i))
                .or_else(|| project.patterns.first());
            if let Some(pat) = pat {
                for (ch, notes) in &pat.notes {
                    for n in notes {
                        add(ch, n.key, n.pos, n.len, n.vel);
                    }
                }
            }
        }
        MidiSource::Song => {
            for c in &project.playlist.clips {
                if c.kind != ClipKind::Pattern || c.muted == Some(true) {
                    continue;
                }
                let Some(pat) = project.pattern(&c.ref_id) else {
                    continue;
                };
                for (ch, notes) in &pat.notes {
                    for n in notes {
                        if n.pos >= c.offset && n.pos < c.offset + c.length {
                            add(ch, n.key, c.start + n.pos - c.offset, n.len, n.vel);
                        }
                    }
                }
            }
        }
    }
    let tempo = round(60_000_000.0 / project.bpm) as u32;
    let (num, den) = (project.time_sig.0, project.time_sig.1);
    let conductor = track_data(
        vec![
            Ev {
                tick: 0.0,
                data: vec![
                    0xff,
                    0x51,
                    0x03,
                    (tempo >> 16) as u8,
                    (tempo >> 8) as u8,
                    tempo as u8,
                ],
            },
            Ev {
                tick: 0.0,
                data: vec![
                    0xff,
                    0x58,
                    0x04,
                    num as u8,
                    f64::from(den).log2() as u8,
                    24,
                    8,
                ],
            },
        ],
        Some(&project.name),
    );
    let mut tracks = vec![conductor];
    let mut midi_ch = 0usize;
    for ch in &project.channels {
        let Some(notes) = per_channel.get(&ch.id).filter(|n| !n.is_empty()) else {
            continue;
        };
        // channel 10 is left to drums
        if midi_ch == 9 {
            midi_ch += 1;
        }
        let c = midi_ch as u8;
        midi_ch = (midi_ch + 1) % 16;
        let mut evs = Vec::new();
        for n in notes {
            let v = round(n.vel * 127.0).clamp(1.0, 127.0) as u8;
            evs.push(Ev {
                tick: n.tick,
                data: vec![0x90 | c, (n.key & 127) as u8, v],
            });
            evs.push(Ev {
                tick: n.tick + n.len,
                data: vec![0x80 | c, (n.key & 127) as u8, 0],
            });
        }
        tracks.push(track_data(evs, Some(&ch.name)));
    }
    let n = tracks.len() as u16;
    let mut header = vec![0, 1];
    header.extend_from_slice(&n.to_be_bytes());
    header.extend_from_slice(&(PPQ as u16).to_be_bytes());
    let mut out = chunk("MThd", &header);
    for t in &tracks {
        out.extend(chunk("MTrk", t));
    }
    out
}

// ---------------------------------------------------------------------------
// Audio files
// ---------------------------------------------------------------------------

/// Bit depth of an exported WAV file (32 is floating point).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BitDepth {
    B16,
    B24,
    B32,
}

impl BitDepth {
    pub fn from_bits(bits: u32) -> Option<Self> {
        match bits {
            16 => Some(Self::B16),
            24 => Some(Self::B24),
            32 => Some(Self::B32),
            _ => None,
        }
    }
}

/// The loudest sample of all channels.
pub fn peak_of(channels: &[&[f32]]) -> f32 {
    channels
        .iter()
        .flat_map(|c| c.iter())
        .fold(0f32, |m, v| m.max(v.abs()))
}

/// Scale so the peak is `target_db` (the web version's default is -0.3).
pub fn normalize(channels: &[&[f32]], target_db: f64) -> Vec<Vec<f32>> {
    let peak = f64::from(peak_of(channels));
    let gain = if peak > 0.0 {
        (10f64.powf(target_db / 20.0) / peak) as f32
    } else {
        1.0
    };
    channels
        .iter()
        .map(|c| c.iter().map(|x| x * gain).collect())
        .collect()
}

/// A small generator for dither (triangular noise of one 16-bit step).
struct Dither(u32);

impl Dither {
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        self.0 as f32 / u32::MAX as f32
    }
}

/// A RIFF/WAVE file: 16 or 24 bit integer or 32 bit float samples, interleaved. At
/// 16 bit a triangular dither of one step is added, as the web version does.
pub fn encode_wav(channels: &[&[f32]], sample_rate: u32, depth: BitDepth, dither: bool) -> Vec<u8> {
    let n_ch = channels.len();
    let len = channels.first().map_or(0, |c| c.len());
    let bytes = match depth {
        BitDepth::B16 => 2,
        BitDepth::B24 => 3,
        BitDepth::B32 => 4,
    };
    let data_size = len * n_ch * bytes;
    let mut out = Vec::with_capacity(44 + data_size);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_size as u32).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&(if depth == BitDepth::B32 { 3u16 } else { 1u16 }).to_le_bytes());
    out.extend_from_slice(&(n_ch as u16).to_le_bytes());
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&(sample_rate * (n_ch * bytes) as u32).to_le_bytes());
    out.extend_from_slice(&((n_ch * bytes) as u16).to_le_bytes());
    out.extend_from_slice(&((bytes * 8) as u16).to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(data_size as u32).to_le_bytes());
    let mut noise = Dither(0x9e37_79b9);
    let step = if dither && depth == BitDepth::B16 {
        1.0 / 32768.0
    } else {
        0.0
    };
    for i in 0..len {
        for c in channels {
            let mut x = c[i];
            match depth {
                BitDepth::B32 => out.extend_from_slice(&x.to_le_bytes()),
                BitDepth::B16 => {
                    if step != 0.0 {
                        x += (noise.next() - noise.next()) * step;
                    }
                    let x = x.clamp(-1.0, 1.0);
                    let s = if x < 0.0 { x * 32768.0 } else { x * 32767.0 } as i16;
                    out.extend_from_slice(&s.to_le_bytes());
                }
                BitDepth::B24 => {
                    let x = f64::from(x.clamp(-1.0, 1.0));
                    let s = round(if x < 0.0 {
                        x * 8_388_608.0
                    } else {
                        x * 8_388_607.0
                    }) as i32;
                    out.extend_from_slice(&s.to_le_bytes()[..3]);
                }
            }
        }
    }
    out
}

/// A FLAC file of 16 or 24 bit samples.
pub fn encode_flac(channels: &[&[f32]], sample_rate: u32, bits: u32) -> Result<Vec<u8>, String> {
    use flacenc::component::BitRepr;
    use flacenc::error::Verify;
    let bits = if bits == 16 { 16 } else { 24 };
    let scale = f64::from(1u32 << (bits - 1));
    let len = channels.first().map_or(0, |c| c.len());
    let mut samples = Vec::with_capacity(len * channels.len());
    for i in 0..len {
        for c in channels {
            let x = f64::from(c[i]).clamp(-1.0, 1.0);
            samples.push(round(x * (scale - 1.0)) as i32);
        }
    }
    let config = flacenc::config::Encoder::default()
        .into_verified()
        .map_err(|e| format!("{e:?}"))?;
    let source = flacenc::source::MemSource::from_samples(
        &samples,
        channels.len(),
        bits as usize,
        sample_rate as usize,
    );
    let stream = flacenc::encode_with_fixed_block_size(&config, source, config.block_size)
        .map_err(|e| format!("{e:?}"))?;
    let mut sink = flacenc::bitsink::ByteSink::new();
    stream.write(&mut sink).map_err(|e| format!("{e:?}"))?;
    Ok(sink.as_slice().to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wav_headers_and_samples() {
        let l = [0.0f32, 0.5, -0.5, 1.0];
        let r = [0.0f32, -1.0, 0.25, 0.0];
        let w = encode_wav(&[&l, &r], 44100, BitDepth::B24, false);
        assert_eq!(&w[0..4], b"RIFF");
        assert_eq!(u32::from_le_bytes(w[40..44].try_into().unwrap()), 4 * 2 * 3);
        // 0.5 at 24 bit
        let s = i32::from_le_bytes([w[44 + 6], w[44 + 7], w[44 + 8], 0]);
        assert_eq!(s, 4_194_304);
        let f = encode_wav(&[&l], 48000, BitDepth::B32, false);
        assert_eq!(&f[44 + 4..44 + 8], &0.5f32.to_le_bytes());
    }

    #[test]
    fn normalising_puts_the_peak_at_the_target() {
        let l = [0.1f32, -0.4];
        let out = normalize(&[&l], -6.0);
        assert!((f64::from(peak_of(&[&out[0]])) - 10f64.powf(-6.0 / 20.0)).abs() < 1e-6);
    }

    #[test]
    fn flac_round_trips() {
        let l: Vec<f32> = (0..9000).map(|i| (i as f32 * 0.05).sin() * 0.5).collect();
        let r: Vec<f32> = l.iter().map(|x| -x).collect();
        let bytes = encode_flac(&[&l, &r], 44100, 16).unwrap();
        assert_eq!(&bytes[0..4], b"fLaC");
        let mut reader = claxon::FlacReader::new(&bytes[..]).unwrap();
        assert_eq!(reader.streaminfo().sample_rate, 44100);
        let got: Vec<i32> = reader.samples().map(|s| s.unwrap()).collect();
        assert_eq!(got.len(), l.len() * 2);
        assert_eq!(got[2000], (f64::from(l[1000]) * 32767.0).round() as i32);
        assert_eq!(got[2001], -got[2000]);
    }
}

/// An MP3 file (constant bit rate, `kbps` 128 to 320) of a stereo or mono render.
pub fn encode_mp3(channels: &[&[f32]], sample_rate: u32, kbps: u32) -> Result<Vec<u8>, String> {
    use mp3lame_encoder::{Bitrate, Builder, DualPcm, FlushNoGap, Quality};
    let left = channels.first().ok_or("nothing to encode")?;
    let right = channels.get(1).unwrap_or(left);
    let mut b = Builder::new().ok_or("the MP3 encoder could not start")?;
    let rate = match kbps {
        0..=128 => Bitrate::Kbps128,
        129..=192 => Bitrate::Kbps192,
        193..=256 => Bitrate::Kbps256,
        _ => Bitrate::Kbps320,
    };
    b.set_num_channels(2).map_err(|e| format!("{e:?}"))?;
    b.set_sample_rate(sample_rate)
        .map_err(|e| format!("{e:?}"))?;
    b.set_brate(rate).map_err(|e| format!("{e:?}"))?;
    b.set_quality(Quality::Best).map_err(|e| format!("{e:?}"))?;
    let mut enc = b.build().map_err(|e| format!("{e:?}"))?;
    let mut out = Vec::new();
    // in pieces, so the output buffer stays small
    for start in (0..left.len()).step_by(1 << 16) {
        let end = (start + (1 << 16)).min(left.len());
        let pcm = DualPcm {
            left: &left[start..end],
            right: &right[start..end.min(right.len())],
        };
        out.reserve(mp3lame_encoder::max_required_buffer_size(end - start));
        enc.encode_to_vec(pcm, &mut out)
            .map_err(|e| format!("{e:?}"))?;
    }
    out.reserve(7200);
    enc.flush_to_vec::<FlushNoGap>(&mut out)
        .map_err(|e| format!("{e:?}"))?;
    Ok(out)
}

#[cfg(test)]
mod mp3_tests {
    use super::*;

    #[test]
    fn an_mp3_has_frames_and_is_about_the_size_its_bit_rate_says() {
        let sr = 44_100;
        let l: Vec<f32> = (0..sr * 2)
            .map(|i| (i as f32 * 440.0 * std::f32::consts::TAU / sr as f32).sin() * 0.5)
            .collect();
        let bytes = encode_mp3(&[&l, &l], sr as u32, 192).unwrap();
        // an MPEG audio frame starts with eleven set bits
        let sync = bytes
            .windows(2)
            .position(|w| w[0] == 0xFF && w[1] & 0xE0 == 0xE0);
        assert!(sync.is_some(), "no MPEG frame");
        // two seconds at 192 kbit/s is about 48 kB
        assert!((40_000..60_000).contains(&bytes.len()), "{}", bytes.len());
    }
}
