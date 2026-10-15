// SPDX-License-Identifier: GPL-3.0-or-later
//! The `.las` project file, Lumen Audio Studio's file format.
//!
//! ```text
//! offset size
//! 0      4    magic "LAS\x1A"
//! 4      2    format version (u16 LE)
//! 6      2    flags: bit 0 = payload encrypted
//! 8      4    payload length (u32 LE)
//! 12     12   AES-GCM nonce
//! 24     4    CRC-32 of the plain container
//! 28     4    reserved (0)
//! 32     ...  payload = AES-256-GCM(container), header bytes 0..31 as AAD
//! ```
//!
//! The container is a list of chunks, each a 4 character tag, a u32 LE length
//! and the data: `HEAD` (JSON about the file), `PRJZ` (the deflated project
//! JSON), `SMPL` (an embedded sample: id, name and mime type, each with a u16
//! length, then the original file bytes) and `END_`. Unknown chunks are skipped.
//!
//! The key is built into the app. The encryption stops casual reading and
//! editing, and the GCM tag detects damage, but it is not DRM.

use crate::create::now_ms;
use crate::migrate::{migrate_project, ProjectError};
use crate::model::Project;
use aes_gcm::aead::{Aead, OsRng, Payload};
use aes_gcm::{AeadCore, Aes256Gcm, Key, KeyInit, Nonce};
use flate2::read::DeflateDecoder;
use flate2::write::DeflateEncoder;
use flate2::Compression;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::io::{Read, Write};

pub const EXTENSION: &str = ".las";
pub const MIME: &str = "application/vnd.lumen.las";
pub const VERSION: u16 = 1;
const MAGIC: [u8; 4] = [0x4c, 0x41, 0x53, 0x1a];
const HEADER: usize = 32;
const FLAG_ENCRYPTED: u16 = 1;
const KEY_SEED: &str = "lumen-audio-studio/las/v1/7f3c9a1e-5b2d-4c8e-9a6f-1d2e3c4b5a69";

#[derive(Debug)]
pub struct LasError(pub String);

impl std::fmt::Display for LasError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for LasError {}

fn err<T>(m: &str) -> Result<T, LasError> {
    Err(LasError(m.into()))
}

/// An embedded sample: the original file's bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sample {
    pub name: String,
    pub mime: String,
    pub data: Vec<u8>,
}

/// What a `.las` file holds.
pub struct Bundle {
    pub project: Project,
    pub samples: BTreeMap<String, Sample>,
    /// Repairs and notes from loading.
    pub warnings: Vec<String>,
}

fn key() -> Key<Aes256Gcm> {
    let digest = Sha256::digest(KEY_SEED.as_bytes());
    *Key::<Aes256Gcm>::from_slice(&digest)
}

/// Is this a `.las` file?
pub fn is_las(bytes: &[u8]) -> bool {
    bytes.len() >= HEADER && bytes[..4] == MAGIC
}

fn chunk(out: &mut Vec<u8>, tag: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(tag);
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out.extend_from_slice(data);
}

fn lp16(out: &mut Vec<u8>, s: &str) {
    out.extend_from_slice(&(s.len() as u16).to_le_bytes());
    out.extend_from_slice(s.as_bytes());
}

/// Whole numbers as integers, like `JSON.stringify` writes them.
fn compact(v: &mut Value) {
    match v {
        Value::Number(n) => {
            if let Some(f) = n.as_f64() {
                if n.is_f64() && f.fract() == 0.0 && f.abs() < 9.0e15 {
                    *v = json!(f as i64);
                }
            }
        }
        Value::Array(a) => a.iter_mut().for_each(compact),
        Value::Object(o) => o.values_mut().for_each(compact),
        _ => {}
    }
}

fn container(project: &Project, samples: &BTreeMap<String, Sample>) -> Result<Vec<u8>, LasError> {
    let mut out = Vec::new();
    let head = json!({
        "app": "Lumen Audio Studio",
        "format": VERSION,
        "savedAt": now_ms(),
        "id": project.id,
        "name": project.name,
    });
    chunk(&mut out, b"HEAD", head.to_string().as_bytes());
    let mut json = serde_json::to_value(project).map_err(|e| LasError(e.to_string()))?;
    compact(&mut json);
    let mut z = DeflateEncoder::new(Vec::new(), Compression::new(6));
    z.write_all(json.to_string().as_bytes())
        .map_err(|e| LasError(e.to_string()))?;
    let z = z.finish().map_err(|e| LasError(e.to_string()))?;
    chunk(&mut out, b"PRJZ", &z);
    for r in &project.samples {
        if r.factory == Some(true) {
            continue;
        }
        let Some(s) = samples.get(&r.id) else {
            continue;
        };
        let mut body = Vec::new();
        lp16(&mut body, &r.id);
        lp16(&mut body, &s.name);
        lp16(&mut body, &s.mime);
        body.extend_from_slice(&s.data);
        chunk(&mut out, b"SMPL", &body);
    }
    chunk(&mut out, b"END_", &[]);
    Ok(out)
}

/// Write a project and its samples. `encrypt` is what the app does by default.
pub fn encode(
    project: &Project,
    samples: &BTreeMap<String, Sample>,
    encrypt: bool,
) -> Result<Vec<u8>, LasError> {
    let plain = container(project, samples)?;
    let mut header = [0u8; HEADER];
    header[..4].copy_from_slice(&MAGIC);
    header[4..6].copy_from_slice(&VERSION.to_le_bytes());
    header[6..8].copy_from_slice(&(if encrypt { FLAG_ENCRYPTED } else { 0 }).to_le_bytes());
    header[24..28].copy_from_slice(&crc32fast::hash(&plain).to_le_bytes());
    let payload = if encrypt {
        let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
        header[12..24].copy_from_slice(&nonce);
        // the length is covered by the AAD, so it goes in first (GCM adds a 16 byte tag)
        header[8..12].copy_from_slice(&((plain.len() + 16) as u32).to_le_bytes());
        Aes256Gcm::new(&key())
            .encrypt(
                &nonce,
                Payload {
                    msg: &plain,
                    aad: &header,
                },
            )
            .map_err(|_| LasError("Could not encrypt the project".into()))?
    } else {
        header[8..12].copy_from_slice(&(plain.len() as u32).to_le_bytes());
        plain
    };
    let mut out = header.to_vec();
    out.extend_from_slice(&payload);
    Ok(out)
}

fn read_str(data: &[u8], at: &mut usize) -> Option<String> {
    let n = u16::from_le_bytes(data.get(*at..*at + 2)?.try_into().ok()?) as usize;
    let s = data.get(*at + 2..*at + 2 + n)?;
    *at += 2 + n;
    Some(String::from_utf8_lossy(s).into_owned())
}

fn parse_container(c: &[u8]) -> Result<Bundle, LasError> {
    let mut at = 0;
    let mut project = None;
    let mut samples = BTreeMap::new();
    let mut warnings = Vec::new();
    while at + 8 <= c.len() {
        let tag = &c[at..at + 4];
        let len = u32::from_le_bytes([c[at + 4], c[at + 5], c[at + 6], c[at + 7]]) as usize;
        at += 8;
        if at + len > c.len() {
            return err("Project file is truncated");
        }
        let data = &c[at..at + len];
        at += len;
        match tag {
            b"PRJZ" => {
                let damaged =
                    || LasError("The project data is damaged and could not be read".into());
                let mut text = String::new();
                DeflateDecoder::new(data)
                    .read_to_string(&mut text)
                    .map_err(|_| damaged())?;
                let json: Value = serde_json::from_str(&text).map_err(|_| damaged())?;
                let migrated = migrate_project(&json).map_err(|e: ProjectError| LasError(e.0))?;
                if let Some(from) = migrated.migrated_from {
                    warnings.push(format!("Updated from project version {from}"));
                }
                warnings.extend(migrated.warnings);
                project = Some(migrated.project);
            }
            b"SMPL" => {
                let mut q = 0;
                let parsed = (|| {
                    let id = read_str(data, &mut q)?;
                    let name = read_str(data, &mut q)?;
                    let mime = read_str(data, &mut q)?;
                    Some((id, name, mime))
                })();
                match parsed {
                    Some((id, name, mime)) => {
                        samples.insert(
                            id,
                            Sample {
                                name,
                                mime,
                                data: data[q..].to_vec(),
                            },
                        );
                    }
                    None => warnings
                        .push("A sample stored in the file is damaged and was skipped".into()),
                }
            }
            b"END_" => break,
            // unknown chunks are skipped, for newer files
            _ => {}
        }
    }
    let Some(project) = project else {
        return err("Project data is missing from the file");
    };
    Ok(Bundle {
        project,
        samples,
        warnings,
    })
}

/// Read a `.las` file.
pub fn decode(bytes: &[u8]) -> Result<Bundle, LasError> {
    if !is_las(bytes) {
        return err("Not a Lumen Audio Studio project (.las)");
    }
    let version = u16::from_le_bytes([bytes[4], bytes[5]]);
    if version > VERSION {
        return Err(LasError(format!(
            "This project was saved by a newer version of Lumen Audio Studio (format {version})"
        )));
    }
    let flags = u16::from_le_bytes([bytes[6], bytes[7]]);
    let len = u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]) as usize;
    if HEADER + len > bytes.len() {
        return err("Project file is truncated");
    }
    let header = &bytes[..HEADER];
    let payload = &bytes[HEADER..HEADER + len];
    let plain = if flags & FLAG_ENCRYPTED != 0 {
        Aes256Gcm::new(&key())
            .decrypt(
                Nonce::from_slice(&header[12..24]),
                Payload {
                    msg: payload,
                    aad: header,
                },
            )
            .map_err(|_| {
                LasError(
                    "Project file is damaged or was modified outside Lumen Audio Studio".into(),
                )
            })?
    } else {
        payload.to_vec()
    };
    let crc = u32::from_le_bytes([bytes[24], bytes[25], bytes[26], bytes[27]]);
    if crc32fast::hash(&plain) != crc {
        return err("Project file failed its integrity check");
    }
    parse_container(&plain)
}
