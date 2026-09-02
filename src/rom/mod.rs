//! ROM containers, platform detection and header inspection.

use anyhow::{anyhow, bail, Context, Result};
use serde::Serialize;
use sha1::{Digest, Sha1};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

pub mod genesis;
pub mod nes;
pub mod snes;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    Nes,
    Snes,
    Genesis,
}

impl Platform {
    pub fn label(self) -> &'static str {
        match self {
            Platform::Nes => "NES",
            Platform::Snes => "Super NES",
            Platform::Genesis => "Mega Drive",
        }
    }

    pub fn from_ext(ext: &str) -> Option<Platform> {
        match ext.to_ascii_lowercase().as_str() {
            "nes" | "unf" | "unif" => Some(Platform::Nes),
            "sfc" | "smc" | "fig" | "swc" => Some(Platform::Snes),
            "md" | "gen" | "smd" | "32x" => Some(Platform::Genesis),
            _ => None,
        }
    }

    fn order(self) -> u8 {
        match self {
            Platform::Nes => 0,
            Platform::Snes => 1,
            Platform::Genesis => 2,
        }
    }
}

pub const ROM_EXTS: &[&str] = &[
    "nes", "unf", "unif", "sfc", "smc", "fig", "swc", "md", "gen", "smd", "32x", "bin",
];

pub fn ext_of(path: &Path) -> String {
    path.extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
}

fn stem_of(path: &Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default()
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Decide the platform from the extension, falling back to sniffing the first bytes.
pub fn sniff(ext: &str, head: &[u8]) -> Option<Platform> {
    if let Some(p) = Platform::from_ext(ext) {
        return Some(p);
    }
    if head.len() >= 4 && &head[..4] == b"NES\x1a" {
        return Some(Platform::Nes);
    }
    if head.len() >= 0x104 && &head[0x100..0x104] == b"SEGA" {
        return Some(Platform::Genesis);
    }
    None
}

#[derive(Clone, Debug, Serialize)]
pub struct LibraryEntry {
    pub id: String,
    pub name: String,
    pub platform: Platform,
    pub path: String,
    pub entry: Option<String>,
    pub bytes: u64,
}

pub fn entry_id(path: &Path) -> String {
    let mut h = Sha1::new();
    h.update(path.to_string_lossy().to_lowercase().as_bytes());
    hex(&h.finalize()[..8])
}

fn read_head(path: &Path) -> Option<Vec<u8>> {
    let mut f = fs::File::open(path).ok()?;
    let mut buf = vec![0u8; 0x200];
    let n = f.read(&mut buf).ok()?;
    buf.truncate(n);
    Some(buf)
}

fn zip_probe(path: &Path) -> Option<(String, Platform)> {
    let f = fs::File::open(path).ok()?;
    let mut za = zip::ZipArchive::new(f).ok()?;
    for i in 0..za.len() {
        let mut e = za.by_index(i).ok()?;
        if e.is_dir() {
            continue;
        }
        let name = e.name().to_string();
        let ext = ext_of(Path::new(&name));
        if !ROM_EXTS.contains(&ext.as_str()) {
            continue;
        }
        let platform = match Platform::from_ext(&ext) {
            Some(p) => Some(p),
            None => {
                let mut head = vec![0u8; 0x200];
                let n = e.read(&mut head).ok()?;
                head.truncate(n);
                sniff(&ext, &head)
            }
        };
        if let Some(p) = platform {
            return Some((name, p));
        }
    }
    None
}

/// Walk the library roots and list every ROM we can identify without fully reading it.
pub fn scan(roots: &[PathBuf]) -> Vec<LibraryEntry> {
    let mut out = Vec::new();
    for root in roots {
        for e in walkdir::WalkDir::new(root)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            if !e.file_type().is_file() {
                continue;
            }
            let p = e.path();
            let ext = ext_of(p);
            let bytes = e.metadata().map(|m| m.len()).unwrap_or(0);
            if ext == "zip" {
                if let Some((entry, platform)) = zip_probe(p) {
                    out.push(LibraryEntry {
                        id: entry_id(p),
                        name: stem_of(p),
                        platform,
                        path: p.display().to_string(),
                        entry: Some(entry),
                        bytes,
                    });
                }
            } else if ROM_EXTS.contains(&ext.as_str()) {
                let platform =
                    Platform::from_ext(&ext).or_else(|| read_head(p).and_then(|h| sniff(&ext, &h)));
                if let Some(platform) = platform {
                    out.push(LibraryEntry {
                        id: entry_id(p),
                        name: stem_of(p),
                        platform,
                        path: p.display().to_string(),
                        entry: None,
                        bytes,
                    });
                }
            }
        }
    }
    out.sort_by(|a, b| {
        (a.platform.order(), a.name.to_lowercase())
            .cmp(&(b.platform.order(), b.name.to_lowercase()))
    });
    out.dedup_by(|a, b| a.id == b.id);
    out
}

#[derive(Clone, Debug, Serialize)]
pub struct Field {
    pub label: String,
    pub value: String,
}

pub fn field(label: &str, value: impl Into<String>) -> Field {
    Field {
        label: label.to_string(),
        value: value.into(),
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct ChecksumInfo {
    pub stored: String,
    pub computed: String,
    pub valid: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct RomInfo {
    pub title: String,
    pub region: String,
    pub size: usize,
    pub sha1: String,
    pub crc32: String,
    pub checksum: Option<ChecksumInfo>,
    pub fields: Vec<Field>,
    pub notes: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "platform", rename_all = "lowercase")]
pub enum Header {
    Nes(nes::NesHeader),
    Snes(snes::SnesHeader),
    Genesis(genesis::GenHeader),
}

pub struct Rom {
    pub id: String,
    pub name: String,
    pub platform: Platform,
    pub path: PathBuf,
    pub entry: Option<String>,
    pub ext: String,
    pub data: Vec<u8>,
    pub info: RomInfo,
    pub header: Header,
}

fn zip_read(path: &Path) -> Result<(String, Vec<u8>)> {
    let f = fs::File::open(path)?;
    let mut za = zip::ZipArchive::new(f).context("not a readable zip archive")?;
    for i in 0..za.len() {
        let mut e = za.by_index(i)?;
        if e.is_dir() {
            continue;
        }
        let name = e.name().to_string();
        let ext = ext_of(Path::new(&name));
        if !ROM_EXTS.contains(&ext.as_str()) {
            continue;
        }
        let mut buf = Vec::with_capacity(e.size() as usize);
        e.read_to_end(&mut buf)?;
        return Ok((name, buf));
    }
    bail!("the archive contains no ROM file")
}

/// Region guess from No-Intro style name tags, used when the header carries none.
pub fn region_from_name(name: &str) -> String {
    let lower = name.to_lowercase();
    let mut tags = Vec::new();
    for (tag, label) in [
        ("(usa", "USA"),
        ("(japan", "Japan"),
        ("(europe", "Europe"),
        ("(world", "World"),
        ("(korea", "Korea"),
        ("(brazil", "Brazil"),
        ("(australia", "Australia"),
        ("(asia", "Asia"),
    ] {
        if lower.contains(tag) {
            tags.push(label);
        }
    }
    tags.join(", ")
}

pub fn load(path: &Path) -> Result<Rom> {
    let ext = ext_of(path);
    let (data, entry) = if ext == "zip" {
        let (n, d) = zip_read(path)?;
        (d, Some(n))
    } else {
        (
            fs::read(path).with_context(|| format!("reading {}", path.display()))?,
            None,
        )
    };
    let inner_ext = entry
        .as_deref()
        .map(|n| ext_of(Path::new(n)))
        .unwrap_or_else(|| ext.clone());
    let platform = sniff(&inner_ext, &data).ok_or_else(|| anyhow!("unrecognised ROM format"))?;

    let mut data = data;
    let mut notes = Vec::new();
    let mut out_ext = inner_ext.clone();
    if platform == Platform::Genesis && genesis::is_smd(&inner_ext, &data) {
        data = genesis::deinterleave_smd(&data);
        out_ext = "md".to_string();
        notes.push("Interleaved .smd image converted to a plain binary".to_string());
    }

    let (mut info, header) = match platform {
        Platform::Nes => {
            let (i, h) = nes::inspect(&data)?;
            (i, Header::Nes(h))
        }
        Platform::Snes => {
            let (i, h) = snes::inspect(&data)?;
            (i, Header::Snes(h))
        }
        Platform::Genesis => {
            let (i, h) = genesis::inspect(&data)?;
            (i, Header::Genesis(h))
        }
    };
    let name = stem_of(path);
    if info.region.is_empty() {
        info.region = region_from_name(&name);
    }
    info.notes.extend(notes);
    let mut h = Sha1::new();
    h.update(&data);
    info.sha1 = hex(&h.finalize());
    info.crc32 = format!("{:08x}", crc32fast::hash(&data));
    info.size = data.len();

    Ok(Rom {
        id: entry_id(path),
        name,
        platform,
        path: path.to_path_buf(),
        entry,
        ext: out_ext,
        data,
        info,
        header,
    })
}

pub fn blank_info() -> RomInfo {
    RomInfo {
        title: String::new(),
        region: String::new(),
        size: 0,
        sha1: String::new(),
        crc32: String::new(),
        checksum: None,
        fields: Vec::new(),
        notes: Vec::new(),
    }
}

pub fn human_size(n: usize) -> String {
    if n >= 1 << 20 {
        let mb = n as f64 / (1024.0 * 1024.0);
        if (mb - mb.round()).abs() < 0.01 {
            format!("{} MB", mb.round() as usize)
        } else {
            format!("{mb:.2} MB")
        }
    } else {
        format!("{} KB", n / 1024)
    }
}
