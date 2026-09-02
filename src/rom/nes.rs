//! iNES / NES 2.0 header handling and CPU address to PRG offset resolution.

use super::{blank_info, field, human_size, RomInfo};
use anyhow::{bail, Result};
use serde::Serialize;

#[derive(Clone, Debug, Serialize)]
pub struct NesHeader {
    pub prg_size: usize,
    pub chr_size: usize,
    pub mapper: u16,
    pub submapper: u8,
    pub mirroring: String,
    pub battery: bool,
    pub trainer: bool,
    pub nes2: bool,
    pub prg_offset: usize,
}

pub fn parse(d: &[u8]) -> Result<NesHeader> {
    if d.len() < 16 || &d[..4] != b"NES\x1a" {
        bail!("not an iNES file");
    }
    let nes2 = d[7] & 0x0C == 0x08;
    let mut prg = d[4] as usize;
    let mut chr = d[5] as usize;
    let mut mapper = ((d[6] >> 4) as u16) | ((d[7] & 0xF0) as u16);
    let mut submapper = 0;
    if nes2 {
        mapper |= ((d[8] & 0x0F) as u16) << 8;
        submapper = d[8] >> 4;
        prg |= ((d[9] & 0x0F) as usize) << 8;
        chr |= ((d[9] >> 4) as usize) << 8;
    }
    let trainer = d[6] & 4 != 0;
    let battery = d[6] & 2 != 0;
    let mirroring = if d[6] & 8 != 0 {
        "four-screen"
    } else if d[6] & 1 != 0 {
        "vertical"
    } else {
        "horizontal"
    }
    .to_string();
    let prg_offset = 16 + if trainer { 512 } else { 0 };
    Ok(NesHeader {
        prg_size: prg * 16384,
        chr_size: chr * 8192,
        mapper,
        submapper,
        mirroring,
        battery,
        trainer,
        nes2,
        prg_offset,
    })
}

/// Size of the switchable PRG window for common mappers. `None` means unknown.
pub fn bank_window(mapper: u16) -> Option<usize> {
    Some(match mapper {
        0 | 3 | 7 | 11 | 13 | 34 | 38 | 66 | 79 | 87 | 101 | 113 | 140 | 185 => 0x8000,
        1 | 2 | 10 | 71 | 94 | 152 | 180 | 232 => 0x4000,
        4 | 5 | 9 | 16 | 18 | 19 | 21 | 22 | 23 | 24 | 25 | 26 | 33 | 48 | 64 | 65 | 67 | 68
        | 69 | 74 | 76 | 80 | 82 | 85 | 88 | 95 | 118 | 119 | 154 | 206 | 210 => 0x2000,
        _ => return None,
    })
}

/// How a mapper hard-wires part of the CPU window to PRG.
#[derive(Clone, Copy)]
enum Fixed {
    /// CPU addresses from `start` up always show the last `size` bytes of PRG.
    Tail { start: u32, size: usize },
    /// CPU addresses below `end` always show the first `size` bytes of PRG.
    Head { end: u32, size: usize },
}

fn fixed_region(mapper: u16) -> Option<Fixed> {
    match mapper {
        1 | 2 | 10 | 71 | 94 | 152 | 232 => Some(Fixed::Tail {
            start: 0xC000,
            size: 0x4000,
        }),
        180 => Some(Fixed::Head {
            end: 0xC000,
            size: 0x4000,
        }),
        9 => Some(Fixed::Tail {
            start: 0xA000,
            size: 0x6000,
        }),
        4 | 19 | 24 | 26 | 33 | 48 | 64 | 65 | 67 | 68 | 69 | 74 | 76 | 88 | 95 | 118 | 119
        | 154 | 206 | 210 => Some(Fixed::Tail {
            start: 0xE000,
            size: 0x2000,
        }),
        _ => None,
    }
}

/// All PRG file offsets that can appear at `cpu_addr`, filtered by the compare byte if given.
pub fn resolve(
    h: &NesHeader,
    d: &[u8],
    cpu_addr: u32,
    compare: Option<u8>,
) -> (Vec<usize>, Vec<String>) {
    let mut notes = Vec::new();
    if !(0x8000..=0xFFFF).contains(&cpu_addr) {
        notes.push(format!(
            "${cpu_addr:04X} is outside the cartridge range $8000-$FFFF"
        ));
        return (Vec::new(), notes);
    }

    // A hard-wired bank gives one unambiguous offset.
    if let Some(f) = fixed_region(h.mapper) {
        let fixed = match f {
            Fixed::Tail { start, size } if cpu_addr >= start && h.prg_size >= size => {
                Some(h.prg_offset + h.prg_size - size + (cpu_addr - start) as usize)
            }
            Fixed::Head { end, size } if cpu_addr < end && h.prg_size >= size => {
                Some(h.prg_offset + (cpu_addr - 0x8000) as usize)
            }
            _ => None,
        };
        if let Some(o) = fixed.filter(|&o| o < d.len()) {
            match compare {
                Some(c) if d[o] != c => notes.push(format!(
                    "Fixed bank holds ${:02X} at ${cpu_addr:04X}, not the compare byte ${c:02X}; scanning every bank",
                    d[o]
                )),
                _ => return (vec![o], notes),
            }
        }
    }

    let (window, known) = match bank_window(h.mapper) {
        Some(w) => (w, true),
        None => (0x2000, false),
    };
    let window = window.min(h.prg_size.max(1));
    if !known {
        notes.push(format!(
            "Mapper {} has no bank table entry; assuming 8 KB banks",
            h.mapper
        ));
    }
    let rel = ((cpu_addr - 0x8000) as usize) % window;
    let banks = (h.prg_size / window).max(1);
    let mut offs: Vec<usize> = (0..banks)
        .map(|b| h.prg_offset + b * window + rel)
        .filter(|&o| o < d.len())
        .collect();
    if let Some(c) = compare {
        let before = offs.len();
        offs.retain(|&o| d[o] == c);
        if offs.is_empty() {
            notes.push(format!(
                "Compare byte ${c:02X} was not found at ${cpu_addr:04X} in any of {before} bank(s); the code may target another revision"
            ));
        }
    } else if offs.len() > 1 {
        notes.push(format!(
            "No compare byte; applied to all {} banks that can map ${cpu_addr:04X}",
            offs.len()
        ));
    }
    (offs, notes)
}

pub fn inspect(d: &[u8]) -> Result<(RomInfo, NesHeader)> {
    let h = parse(d)?;
    let mut info = blank_info();
    info.fields
        .push(field("Format", if h.nes2 { "NES 2.0" } else { "iNES" }));
    info.fields.push(field("PRG ROM", human_size(h.prg_size)));
    info.fields.push(field(
        "CHR",
        if h.chr_size == 0 {
            "RAM".to_string()
        } else {
            human_size(h.chr_size)
        },
    ));
    let mapper = if h.nes2 && h.submapper != 0 {
        format!("{}.{}", h.mapper, h.submapper)
    } else {
        h.mapper.to_string()
    };
    let bank = bank_window(h.mapper)
        .map(|w| format!(" · {} KB banks", w / 1024))
        .unwrap_or_default();
    info.fields.push(field("Mapper", format!("{mapper}{bank}")));
    info.fields.push(field("Mirroring", h.mirroring.clone()));
    if h.battery {
        info.fields.push(field("Battery", "yes"));
    }
    if h.trainer {
        info.fields.push(field("Trainer", "512 bytes"));
    }
    Ok((info, h))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake(mapper: u16, prg_banks_16k: u8) -> (NesHeader, Vec<u8>) {
        let mut d = vec![0u8; 16 + prg_banks_16k as usize * 0x4000];
        d[..4].copy_from_slice(b"NES\x1a");
        d[4] = prg_banks_16k;
        d[6] = ((mapper & 0x0F) << 4) as u8;
        d[7] = (mapper & 0xF0) as u8;
        for (i, b) in d.iter_mut().enumerate().skip(16) {
            *b = (i % 251) as u8;
        }
        (parse(&d).unwrap(), d)
    }

    #[test]
    fn nrom_resolves_to_one_offset() {
        let (h, d) = fake(0, 2);
        let (offs, _) = resolve(&h, &d, 0x9234, None);
        assert_eq!(offs, vec![16 + 0x1234]);
        let (h16, d16) = fake(0, 1);
        let (offs, _) = resolve(&h16, &d16, 0xD234, None);
        assert_eq!(offs, vec![16 + 0x1234]);
    }

    #[test]
    fn uxrom_fixed_bank_is_unique_and_switchable_is_not() {
        let (h, d) = fake(2, 8);
        let (offs, notes) = resolve(&h, &d, 0xDAD2, None);
        assert_eq!(offs, vec![16 + 7 * 0x4000 + 0x1AD2]);
        assert!(notes.is_empty());
        let (offs, _) = resolve(&h, &d, 0x9000, None);
        assert_eq!(offs.len(), 8);
    }

    #[test]
    fn compare_mismatch_in_fixed_bank_falls_back_to_scan() {
        let (h, d) = fake(2, 4);
        let fixed = 16 + 3 * 0x4000 + 0x0100;
        let (offs, notes) = resolve(&h, &d, 0xC100, Some(d[fixed]));
        assert_eq!(offs, vec![fixed]);
        assert!(notes.is_empty());
        let wrong = d[fixed].wrapping_add(1);
        let (offs, notes) = resolve(&h, &d, 0xC100, Some(wrong));
        assert!(offs.iter().all(|&o| d[o] == wrong));
        assert!(!notes.is_empty());
    }

    #[test]
    fn mmc3_tail_is_last_8k() {
        let (h, d) = fake(4, 8);
        let (offs, _) = resolve(&h, &d, 0xFFFC, None);
        assert_eq!(offs, vec![16 + 8 * 0x4000 - 0x2000 + 0x1FFC]);
    }
}
