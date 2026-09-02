//! Resolving codes against a concrete ROM, building patched images, IPS and RetroArch files.

use crate::codes::{self, Op};
use crate::rom::{genesis, nes, snes, Header, Rom};
use anyhow::{bail, Context, Result};
use serde::Serialize;
use sha1::{Digest, Sha1};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct RomOp {
    pub offset: usize,
    pub old: Vec<u8>,
    pub new: Vec<u8>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Decoded {
    pub raw: String,
    pub format: String,
    pub op: Option<Op>,
    pub rom_ops: Vec<RomOp>,
    /// True when this part can be written into the ROM image.
    pub patchable: bool,
    /// True when the ROM already holds the target bytes.
    pub noop: bool,
    pub notes: Vec<String>,
    pub error: Option<String>,
}

pub fn decode(rom: &Rom, code: &str) -> Vec<Decoded> {
    codes::parse(rom.platform, code)
        .into_iter()
        .map(|p| {
            let (rom_ops, notes) = match &p.op {
                Some(op) => resolve(rom, op),
                None => (Vec::new(), Vec::new()),
            };
            let patchable = matches!(p.op, Some(Op::Rom { .. })) && !rom_ops.is_empty();
            let noop = patchable && rom_ops.iter().all(|o| o.old == o.new);
            Decoded {
                raw: p.raw,
                format: p.format,
                op: p.op,
                rom_ops,
                patchable,
                noop,
                notes,
                error: p.error,
            }
        })
        .collect()
}

pub fn resolve(rom: &Rom, op: &Op) -> (Vec<RomOp>, Vec<String>) {
    let d = &rom.data;
    match op {
        Op::Ram { .. } => (
            Vec::new(),
            vec!["Runtime write to work RAM; delivered as an emulator cheat file".to_string()],
        ),
        Op::Rom {
            cpu_addr,
            value,
            width,
            compare,
        } => {
            let width = *width as usize;
            let new: Vec<u8> = if width == 2 {
                (*value as u16).to_be_bytes().to_vec()
            } else {
                vec![*value as u8]
            };
            let (offsets, mut notes): (Vec<usize>, Vec<String>) = match &rom.header {
                Header::Nes(h) => nes::resolve(h, d, *cpu_addr, *compare),
                Header::Snes(h) => match snes::map_to_offset(h, *cpu_addr, d.len()) {
                    Some(o) => (vec![o], Vec::new()),
                    None => (
                        Vec::new(),
                        vec![format!(
                            "${cpu_addr:06X} does not map to ROM under {}",
                            h.map.label()
                        )],
                    ),
                },
                Header::Genesis(_) => {
                    let o = *cpu_addr as usize;
                    if o + width <= d.len() {
                        (vec![o], Vec::new())
                    } else {
                        (
                            Vec::new(),
                            vec![format!("${cpu_addr:06X} is beyond the end of this ROM")],
                        )
                    }
                }
            };
            let ops: Vec<RomOp> = offsets
                .into_iter()
                .filter(|o| o + width <= d.len())
                .map(|o| RomOp {
                    offset: o,
                    old: d[o..o + width].to_vec(),
                    new: new.clone(),
                })
                .collect();
            if !ops.is_empty() && ops.iter().all(|o| o.old == o.new) {
                notes.push("ROM already contains these bytes".to_string());
            }
            (ops, notes)
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct BuildResult {
    pub rom_path: String,
    pub ips_path: String,
    pub changed_bytes: usize,
    pub ops: Vec<RomOp>,
    pub checksum: Option<(String, String)>,
    pub sha1: String,
}

fn safe_label(label: &str) -> String {
    let cleaned: String = label
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' | '[' | ']' => ' ',
            c => c,
        })
        .collect();
    let cleaned = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    if cleaned.is_empty() {
        "Modded".to_string()
    } else {
        cleaned
    }
}

/// A single file-name component: separators become spaces, control characters are
/// dropped, and leading or trailing dots and spaces are trimmed so `..` cannot survive.
fn safe_component(s: &str) -> Option<String> {
    let cleaned: String = s
        .chars()
        .filter(|c| !c.is_control())
        .map(|c| match c {
            '/' | '\\' | ':' | '<' | '>' | '"' | '|' | '?' | '*' => ' ',
            c => c,
        })
        .collect();
    let cleaned = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    let cleaned = cleaned.trim_matches(|c| c == '.' || c == ' ').to_string();
    if cleaned.is_empty() {
        None
    } else {
        Some(cleaned)
    }
}

/// Collect the ROM operations for a list of codes, failing on anything that cannot be patched.
pub fn collect_ops(rom: &Rom, code_list: &[String]) -> Result<Vec<RomOp>> {
    let mut ops = Vec::new();
    for code in code_list {
        for part in decode(rom, code) {
            if let Some(e) = part.error {
                bail!("{}: {e}", part.raw);
            }
            if !part.patchable {
                let why = part
                    .notes
                    .first()
                    .cloned()
                    .unwrap_or_else(|| "not a ROM patch".to_string());
                bail!("{}: {why}", part.raw);
            }
            ops.extend(part.rom_ops);
        }
    }
    ops.sort_by_key(|o| o.offset);
    ops.dedup();
    let mut last_end = 0usize;
    for o in &ops {
        if o.offset < last_end {
            bail!(
                "two codes write overlapping bytes at ${:06X}; pick one",
                o.offset
            );
        }
        last_end = o.offset + o.new.len();
    }
    Ok(ops)
}

/// IPS records covering every byte that differs between `orig` and `patched`.
pub fn ips(orig: &[u8], patched: &[u8]) -> Result<Vec<u8>> {
    let mut out = b"PATCH".to_vec();
    let mut i = 0;
    let n = orig.len().min(patched.len());
    while i < n {
        if orig[i] == patched[i] {
            i += 1;
            continue;
        }
        let start = i;
        while i < n && orig[i] != patched[i] && i - start < 0xFFFF {
            i += 1;
        }
        if start >= 0x1000000 {
            bail!("IPS cannot address offset ${start:X}");
        }
        out.extend_from_slice(&(start as u32).to_be_bytes()[1..]);
        out.extend_from_slice(&((i - start) as u16).to_be_bytes());
        out.extend_from_slice(&patched[start..i]);
    }
    out.extend_from_slice(b"EOF");
    Ok(out)
}

pub fn build(
    rom: &Rom,
    ops: &[RomOp],
    label: &str,
    out_dir: Option<&Path>,
    overwrite: bool,
) -> Result<BuildResult> {
    if ops.is_empty() {
        bail!("nothing to patch");
    }
    let mut data = rom.data.clone();
    for o in ops {
        if o.offset + o.new.len() > data.len() {
            bail!("patch at ${:06X} runs past the end of the ROM", o.offset);
        }
        data[o.offset..o.offset + o.new.len()].copy_from_slice(&o.new);
    }
    let checksum = match &rom.header {
        Header::Genesis(_) => {
            let (a, b) = genesis::fix_checksum(&mut data);
            Some((format!("{a:04X}"), format!("{b:04X}")))
        }
        Header::Snes(h) => {
            let (a, b) = snes::fix_checksum(&mut data, h);
            Some((format!("{a:04X}"), format!("{b:04X}")))
        }
        Header::Nes(_) => None,
    };
    let ips_bytes = ips(&rom.data, &data)?;
    let changed_bytes = rom.data.iter().zip(&data).filter(|(a, b)| a != b).count();

    let dir: PathBuf = match out_dir {
        Some(d) => d.to_path_buf(),
        None => rom
            .path
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from(".")),
    };
    fs::create_dir_all(&dir)?;
    let base = format!("{} [{}]", rom.name, safe_label(label));
    let rom_path = if rom.entry.is_some() {
        dir.join(format!("{base}.zip"))
    } else {
        dir.join(format!("{base}.{}", rom.ext))
    };
    let ips_path = dir.join(format!("{base}.ips"));
    if !overwrite && (rom_path.exists() || ips_path.exists()) {
        bail!("EXISTS:{}", rom_path.display());
    }
    if rom.entry.is_some() {
        let f = fs::File::create(&rom_path)?;
        let mut zw = zip::ZipWriter::new(f);
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        zw.start_file(format!("{base}.{}", rom.ext), opts)?;
        zw.write_all(&data)?;
        zw.finish()?;
    } else {
        fs::write(&rom_path, &data).with_context(|| format!("writing {}", rom_path.display()))?;
    }
    fs::write(&ips_path, &ips_bytes)?;

    let mut h = Sha1::new();
    h.update(&data);
    let sha1 = crate::rom::hex(&h.finalize());
    Ok(BuildResult {
        rom_path: rom_path.display().to_string(),
        ips_path: ips_path.display().to_string(),
        changed_bytes,
        ops: ops.to_vec(),
        checksum,
        sha1,
    })
}

/// Write a RetroArch cheat file to `<retroarch>/cheats/<core>/<content>.cht`.
pub fn write_retroarch_cht(
    retroarch_dir: &Path,
    core: &str,
    content_name: &str,
    cheats: &[(String, String)],
) -> Result<PathBuf> {
    if cheats.is_empty() {
        bail!("no cheats selected");
    }
    let core = safe_component(core).ok_or_else(|| anyhow::anyhow!("invalid core name"))?;
    let content_name =
        safe_component(content_name).ok_or_else(|| anyhow::anyhow!("invalid content name"))?;
    let dir = retroarch_dir.join("cheats").join(core);
    fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{content_name}.cht"));
    let mut text = format!("cheats = {}\n\n", cheats.len());
    for (i, (desc, code)) in cheats.iter().enumerate() {
        let desc = desc.replace('"', "'");
        text.push_str(&format!(
            "cheat{i}_desc = \"{desc}\"\ncheat{i}_code = \"{code}\"\ncheat{i}_enable = true\n\n"
        ));
    }
    fs::write(&path, text)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ips_records_cover_changes() {
        let orig = vec![0u8; 16];
        let mut patched = orig.clone();
        patched[3] = 1;
        patched[4] = 2;
        patched[10] = 9;
        let p = ips(&orig, &patched).unwrap();
        assert_eq!(&p[..5], b"PATCH");
        assert_eq!(&p[p.len() - 3..], b"EOF");
        assert_eq!(&p[5..12], &[0, 0, 3, 0, 2, 1, 2]);
        assert_eq!(&p[12..18], &[0, 0, 10, 0, 1, 9]);
    }

    #[test]
    fn labels_are_filename_safe() {
        assert_eq!(safe_label("Infinite lives: P1/P2?"), "Infinite lives P1 P2");
        assert_eq!(safe_label("   "), "Modded");
    }

    #[test]
    fn path_components_reject_traversal() {
        assert_eq!(
            safe_component("Genesis Plus GX").as_deref(),
            Some("Genesis Plus GX")
        );
        assert_eq!(
            safe_component("Contra (USA) [Rev 1]").as_deref(),
            Some("Contra (USA) [Rev 1]")
        );
        assert_eq!(safe_component("..").as_deref(), None);
        assert_eq!(safe_component("../x").as_deref(), Some("x"));
        assert_eq!(safe_component("a/b\\c").as_deref(), Some("a b c"));
        assert_eq!(safe_component("   ").as_deref(), None);
    }
}
