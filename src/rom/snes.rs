//! Super NES header detection, checksum repair and SNES CPU address mapping.

use super::{blank_info, field, human_size, ChecksumInfo, RomInfo};
use anyhow::{anyhow, bail, Result};
use serde::Serialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
// LoROM / HiROM / ExHiROM are the established names for these layouts.
#[allow(clippy::enum_variant_names)]
pub enum MapMode {
    LoRom,
    HiRom,
    ExHiRom,
}

impl MapMode {
    pub fn label(self) -> &'static str {
        match self {
            MapMode::LoRom => "LoROM",
            MapMode::HiRom => "HiROM",
            MapMode::ExHiRom => "ExHiROM",
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct SnesHeader {
    /// 0 or 512 bytes of copier header before the ROM image.
    pub copier_header: usize,
    pub map: MapMode,
    /// Absolute file offset of the 32-byte internal header.
    pub header_at: usize,
    pub title: String,
    pub map_byte: u8,
    pub rom_type: u8,
    pub rom_size_code: u8,
    pub sram_code: u8,
    pub country: u8,
    pub version: u8,
    pub complement: u16,
    pub checksum: u16,
}

pub fn parse(d: &[u8]) -> Result<SnesHeader> {
    let copier = if d.len() % 1024 == 512 { 512 } else { 0 };
    let body = &d[copier..];
    let mut best: Option<(i32, usize, MapMode)> = None;
    for (off, mode) in [
        (0x7FC0usize, MapMode::LoRom),
        (0xFFC0, MapMode::HiRom),
        (0x40FFC0, MapMode::ExHiRom),
    ] {
        if off + 0x40 > body.len() {
            continue;
        }
        let h = &body[off..off + 0x40];
        let mut score = 0;
        let comp = u16::from_le_bytes([h[0x1C], h[0x1D]]);
        let sum = u16::from_le_bytes([h[0x1E], h[0x1F]]);
        if comp ^ sum == 0xFFFF {
            score += 4;
        }
        let m = h[0x15];
        let mode_ok = match mode {
            MapMode::LoRom => m & 0x25 == 0x20,
            MapMode::HiRom => m & 0x25 == 0x21,
            MapMode::ExHiRom => m & 0x25 == 0x25,
        };
        if mode_ok {
            score += 2;
        }
        if h[..21].iter().all(|&b| (0x20..0x7F).contains(&b)) {
            score += 1;
        }
        let reset = u16::from_le_bytes([h[0x3C], h[0x3D]]);
        if reset >= 0x8000 {
            score += 1;
        }
        if best.is_none_or(|(s, _, _)| score > s) {
            best = Some((score, off, mode));
        }
    }
    let (score, off, map) = best.ok_or_else(|| anyhow!("file is too small to be a SNES image"))?;
    if score < 3 {
        bail!("no plausible SNES internal header found");
    }
    let h = &body[off..off + 0x40];
    Ok(SnesHeader {
        copier_header: copier,
        map,
        header_at: copier + off,
        title: String::from_utf8_lossy(&h[..21]).trim().to_string(),
        map_byte: h[0x15],
        rom_type: h[0x16],
        rom_size_code: h[0x17],
        sram_code: h[0x18],
        country: h[0x19],
        version: h[0x1B],
        complement: u16::from_le_bytes([h[0x1C], h[0x1D]]),
        checksum: u16::from_le_bytes([h[0x1E], h[0x1F]]),
    })
}

/// Checksum over the image with the checksum fields normalised, mirroring odd tails.
pub fn compute_checksum(body: &[u8], header_in_body: usize) -> u16 {
    let len = body.len();
    if len == 0 {
        return 0;
    }
    let p = if len.is_power_of_two() {
        len
    } else {
        1usize << (usize::BITS - 1 - len.leading_zeros())
    };
    let sum_a: u32 = body[..p].iter().map(|&b| b as u32).sum();
    let rem = len - p;
    let sum_b: u32 = if rem > 0 {
        let s: u32 = body[p..].iter().map(|&b| b as u32).sum();
        let mult = if rem.is_power_of_two() && p % rem == 0 {
            (p / rem) as u32
        } else {
            1
        };
        s.wrapping_mul(mult)
    } else {
        0
    };
    let mut total = sum_a.wrapping_add(sum_b);
    if header_in_body + 0x20 <= len {
        let f = &body[header_in_body + 0x1C..header_in_body + 0x20];
        let actual: u32 = f.iter().map(|&b| b as u32).sum();
        total = total.wrapping_sub(actual).wrapping_add(0x1FE);
    }
    (total & 0xFFFF) as u16
}

/// Rewrite the complement/checksum pair. Returns (stored before, computed).
pub fn fix_checksum(d: &mut [u8], h: &SnesHeader) -> (u16, u16) {
    let sum = compute_checksum(&d[h.copier_header..], h.header_at - h.copier_header);
    let comp = sum ^ 0xFFFF;
    let at = h.header_at;
    d[at + 0x1C..at + 0x1E].copy_from_slice(&comp.to_le_bytes());
    d[at + 0x1E..at + 0x20].copy_from_slice(&sum.to_le_bytes());
    (h.checksum, sum)
}

/// Map a 24-bit SNES CPU address to a file offset, or `None` when it is not ROM.
pub fn map_to_offset(h: &SnesHeader, addr: u32, file_len: usize) -> Option<usize> {
    let bank = (addr >> 16) & 0xFF;
    let lo = (addr & 0xFFFF) as usize;
    let rom_len = file_len.checked_sub(h.copier_header)?;
    if rom_len == 0 {
        return None;
    }
    let off = match h.map {
        MapMode::LoRom => {
            if lo < 0x8000 {
                return None;
            }
            let b = bank & 0x7F;
            if b >= 0x7E {
                return None;
            }
            (b as usize) * 0x8000 + (lo - 0x8000)
        }
        MapMode::HiRom => {
            let b = bank & 0x7F;
            if (b < 0x40 && lo < 0x8000) || (b >= 0x7E && bank < 0x80) {
                return None;
            }
            ((b & 0x3F) as usize) * 0x10000 + lo
        }
        MapMode::ExHiRom => {
            let b = bank & 0x7F;
            if b < 0x40 && lo < 0x8000 {
                return None;
            }
            let base = if bank & 0x80 != 0 { 0 } else { 0x400000 };
            base + ((b & 0x3F) as usize) * 0x10000 + lo
        }
    };
    let off = if off >= rom_len {
        if rom_len.is_power_of_two() {
            off % rom_len
        } else {
            return None;
        }
    } else {
        off
    };
    Some(off + h.copier_header)
}

fn country_label(c: u8) -> &'static str {
    match c {
        0 => "Japan",
        1 => "USA",
        2..=5 | 8..=17 => "Europe",
        6 => "France",
        7 => "Netherlands",
        _ => "",
    }
}

pub fn inspect(d: &[u8]) -> Result<(RomInfo, SnesHeader)> {
    let h = parse(d)?;
    let mut info = blank_info();
    info.title = h.title.clone();
    info.region = country_label(h.country).to_string();
    let computed = compute_checksum(&d[h.copier_header..], h.header_at - h.copier_header);
    info.checksum = Some(ChecksumInfo {
        stored: format!("{:04X}", h.checksum),
        computed: format!("{computed:04X}"),
        valid: computed == h.checksum && h.checksum ^ h.complement == 0xFFFF,
    });
    info.fields.push(field("Mapping", h.map.label()));
    if h.copier_header > 0 {
        info.fields.push(field("Copier header", "512 bytes"));
    }
    info.fields.push(field(
        "Speed",
        if h.map_byte & 0x10 != 0 {
            "FastROM"
        } else {
            "SlowROM"
        },
    ));
    info.fields
        .push(field("ROM size", human_size(d.len() - h.copier_header)));
    if h.sram_code > 0 {
        info.fields
            .push(field("SRAM", format!("{} KB", 1usize << h.sram_code)));
    }
    info.fields
        .push(field("Version", format!("1.{}", h.version)));
    Ok((info, h))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checksum_pair_is_self_consistent() {
        // Build a fake 64 KB LoROM image with a header block and verify fix_checksum
        // produces a pair that validates when recomputed.
        let mut d = vec![0x37u8; 0x10000];
        let h = SnesHeader {
            copier_header: 0,
            map: MapMode::LoRom,
            header_at: 0x7FC0,
            title: String::new(),
            map_byte: 0x20,
            rom_type: 0,
            rom_size_code: 0,
            sram_code: 0,
            country: 1,
            version: 0,
            complement: 0,
            checksum: 0,
        };
        let (_, sum) = fix_checksum(&mut d, &h);
        let stored = u16::from_le_bytes([d[0x7FDE], d[0x7FDF]]);
        let comp = u16::from_le_bytes([d[0x7FDC], d[0x7FDD]]);
        assert_eq!(stored, sum);
        assert_eq!(stored ^ comp, 0xFFFF);
        assert_eq!(compute_checksum(&d, 0x7FC0), sum);
    }

    #[test]
    fn lorom_and_hirom_mapping() {
        let lo = SnesHeader {
            copier_header: 0,
            map: MapMode::LoRom,
            header_at: 0x7FC0,
            title: String::new(),
            map_byte: 0x20,
            rom_type: 0,
            rom_size_code: 0,
            sram_code: 0,
            country: 1,
            version: 0,
            complement: 0,
            checksum: 0,
        };
        assert_eq!(map_to_offset(&lo, 0x008000, 0x100000), Some(0));
        assert_eq!(map_to_offset(&lo, 0x01FFFF, 0x100000), Some(0xFFFF));
        assert_eq!(map_to_offset(&lo, 0x818000, 0x100000), Some(0x8000));
        assert_eq!(map_to_offset(&lo, 0x7E0000, 0x100000), None);
        let hi = SnesHeader {
            map: MapMode::HiRom,
            header_at: 0xFFC0,
            ..lo.clone()
        };
        assert_eq!(map_to_offset(&hi, 0xC00000, 0x100000), Some(0));
        assert_eq!(map_to_offset(&hi, 0x408000, 0x100000), Some(0x8000));
        assert_eq!(map_to_offset(&hi, 0x7E1234, 0x100000), None);
    }
}
