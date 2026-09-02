//! Mega Drive / Genesis header handling, checksum repair and .smd conversion.

use super::{blank_info, field, human_size, ChecksumInfo, RomInfo};
use anyhow::{bail, Result};
use serde::Serialize;

#[derive(Clone, Debug, Serialize)]
pub struct GenHeader {
    pub system: String,
    pub copyright: String,
    pub domestic_title: String,
    pub overseas_title: String,
    pub serial: String,
    pub checksum: u16,
    pub rom_end: u32,
    pub region: String,
    pub sram: bool,
}

pub fn is_smd(ext: &str, d: &[u8]) -> bool {
    if ext == "smd" {
        return true;
    }
    d.len() > 512 + 0x200 && d[8] == 0xAA && d[9] == 0xBB && &d[0x100..0x104] != b"SEGA"
}

pub fn deinterleave_smd(d: &[u8]) -> Vec<u8> {
    let body = &d[512..];
    let mut out = vec![0u8; body.len()];
    for (bi, blk) in body.chunks(16384).enumerate() {
        let half = blk.len() / 2;
        for i in 0..half {
            out[bi * 16384 + 2 * i + 1] = blk[i];
            out[bi * 16384 + 2 * i] = blk[half + i];
        }
    }
    out
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b)
        .trim_matches(|c: char| c == '\0' || c.is_whitespace())
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn region_label(raw: &[u8]) -> String {
    let letters = text(raw).to_ascii_uppercase();
    let mut tags = Vec::new();
    let old_style = !letters.is_empty()
        && letters
            .chars()
            .all(|c| matches!(c, 'J' | 'U' | 'E' | 'A' | 'B' | 'C' | ' '));
    if old_style {
        if letters.contains('J') {
            tags.push("Japan");
        }
        if letters.contains('U') {
            tags.push("USA");
        }
        if letters.contains('E') {
            tags.push("Europe");
        }
    } else if let Some(v) = letters.chars().next().and_then(|c| c.to_digit(16)) {
        if v & 1 != 0 {
            tags.push("Japan");
        }
        if v & 4 != 0 {
            tags.push("USA");
        }
        if v & 8 != 0 {
            tags.push("Europe");
        }
    }
    tags.join(", ")
}

pub fn parse(d: &[u8]) -> Result<GenHeader> {
    if d.len() < 0x200 || &d[0x100..0x104] != b"SEGA" {
        bail!("no SEGA header at $100");
    }
    Ok(GenHeader {
        system: text(&d[0x100..0x110]),
        copyright: text(&d[0x110..0x120]),
        domestic_title: text(&d[0x120..0x150]),
        overseas_title: text(&d[0x150..0x180]),
        serial: text(&d[0x180..0x18E]),
        checksum: u16::from_be_bytes([d[0x18E], d[0x18F]]),
        rom_end: u32::from_be_bytes([d[0x1A4], d[0x1A5], d[0x1A6], d[0x1A7]]),
        region: region_label(&d[0x1F0..0x1F3]),
        sram: &d[0x1B0..0x1B2] == b"RA",
    })
}

pub fn compute_checksum(d: &[u8]) -> u16 {
    d[0x200.min(d.len())..]
        .chunks(2)
        .map(|c| {
            if c.len() == 2 {
                u16::from_be_bytes([c[0], c[1]]) as u32
            } else {
                (c[0] as u32) << 8
            }
        })
        .fold(0u32, |a, b| (a + b) & 0xFFFF) as u16
}

/// Rewrite the header checksum. Returns (stored before, computed).
pub fn fix_checksum(d: &mut [u8]) -> (u16, u16) {
    let old = u16::from_be_bytes([d[0x18E], d[0x18F]]);
    let new = compute_checksum(d);
    d[0x18E..0x190].copy_from_slice(&new.to_be_bytes());
    (old, new)
}

pub fn inspect(d: &[u8]) -> Result<(RomInfo, GenHeader)> {
    let h = parse(d)?;
    let mut info = blank_info();
    info.title = if h.overseas_title.is_empty() {
        h.domestic_title.clone()
    } else {
        h.overseas_title.clone()
    };
    info.region = h.region.clone();
    let computed = compute_checksum(d);
    info.checksum = Some(ChecksumInfo {
        stored: format!("{:04X}", h.checksum),
        computed: format!("{computed:04X}"),
        valid: computed == h.checksum,
    });
    info.fields.push(field("System", h.system.clone()));
    info.fields.push(field("Serial", h.serial.clone()));
    info.fields.push(field("Copyright", h.copyright.clone()));
    info.fields.push(field("ROM size", human_size(d.len())));
    if h.sram {
        info.fields.push(field("SRAM", "yes"));
    }
    Ok((info, h))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smd_deinterleave_round_trip() {
        // Build a 2-block plain image, interleave it by hand, and check we get it back.
        let plain: Vec<u8> = (0..32768u32).map(|i| (i * 7 % 251) as u8).collect();
        let mut smd = vec![0u8; 512];
        smd[8] = 0xAA;
        smd[9] = 0xBB;
        for blk in plain.chunks(16384) {
            let mut odd = Vec::new();
            let mut even = Vec::new();
            for (i, b) in blk.iter().enumerate() {
                if i % 2 == 1 {
                    odd.push(*b)
                } else {
                    even.push(*b)
                }
            }
            smd.extend(odd);
            smd.extend(even);
        }
        assert_eq!(deinterleave_smd(&smd), plain);
    }

    #[test]
    fn checksum_sums_words_after_header() {
        let mut d = vec![0u8; 0x204];
        d[0x200] = 0x12;
        d[0x201] = 0x34;
        d[0x202] = 0x00;
        d[0x203] = 0x01;
        assert_eq!(compute_checksum(&d), 0x1235);
    }
}
