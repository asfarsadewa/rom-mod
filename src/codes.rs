//! Cheat code parsers: Game Genie and Action Replay formats for NES, Super NES and Mega Drive.

use crate::rom::Platform;
use serde::Serialize;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Op {
    /// A patch to the cartridge address space.
    Rom {
        cpu_addr: u32,
        value: u32,
        width: u8,
        compare: Option<u8>,
    },
    /// A runtime write to work RAM; needs an emulator cheat engine.
    Ram { addr: u32, value: u32, width: u8 },
}

#[derive(Clone, Debug, Serialize)]
pub struct Parsed {
    pub raw: String,
    pub format: String,
    pub op: Option<Op>,
    pub error: Option<String>,
}

fn parsed(raw: &str, format: &str, r: Result<Op, String>) -> Parsed {
    match r {
        Ok(op) => Parsed {
            raw: raw.to_string(),
            format: format.to_string(),
            op: Some(op),
            error: None,
        },
        Err(e) => Parsed {
            raw: raw.to_string(),
            format: format.to_string(),
            op: None,
            error: Some(e),
        },
    }
}

/// Split a cheat string into its parts. Multi-part codes join with `+`.
pub fn parse(platform: Platform, code: &str) -> Vec<Parsed> {
    code.split(|c: char| c == '+' || c.is_whitespace() || c == ',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|part| parse_one(platform, part))
        .collect()
}

pub fn parse_one(platform: Platform, part: &str) -> Parsed {
    match platform {
        Platform::Nes => nes(part),
        Platform::Snes => snes(part),
        Platform::Genesis => genesis(part),
    }
}

fn is_hex(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_hexdigit())
}

fn hex(s: &str) -> Result<u32, String> {
    u32::from_str_radix(s, 16).map_err(|_| format!("'{s}' is not hexadecimal"))
}

// ---------------------------------------------------------------- NES

const NES_GG: &str = "APZLGITYEOXUKSVN";

fn nes(part: &str) -> Parsed {
    let up = part.to_ascii_uppercase();
    if let Some((a, rest)) = up.split_once(':') {
        // FCEUX raw form: AAAA:VV or AAAA?CC:VV
        let (addr, compare) = match a.split_once('?') {
            Some((addr, cmp)) => (addr, Some(cmp)),
            None => (a, None),
        };
        let r = (|| {
            let cpu_addr = hex(addr)?;
            let value = hex(rest)?;
            if cpu_addr > 0xFFFF || value > 0xFF {
                return Err("raw NES codes are AAAA:VV".to_string());
            }
            let compare = match compare {
                Some(c) => Some(hex(c)? as u8),
                None => None,
            };
            if cpu_addr < 0x8000 {
                Ok(Op::Ram {
                    addr: cpu_addr,
                    value,
                    width: 1,
                })
            } else {
                Ok(Op::Rom {
                    cpu_addr,
                    value,
                    width: 1,
                    compare,
                })
            }
        })();
        return parsed(part, "Raw", r);
    }
    parsed(part, "Game Genie", nes_game_genie(&up))
}

pub fn nes_game_genie(code: &str) -> Result<Op, String> {
    let n: Vec<u8> = code
        .chars()
        .map(|c| NES_GG.find(c).map(|i| i as u8))
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| "NES Game Genie codes use only the letters APZLGITYEOXUKSVN".to_string())?;
    if n.len() != 6 && n.len() != 8 {
        return Err("NES Game Genie codes are 6 or 8 letters".to_string());
    }
    let cpu_addr = 0x8000
        | ((n[3] & 7) as u32) << 12
        | ((n[5] & 7) as u32) << 8
        | ((n[4] & 8) as u32) << 8
        | ((n[2] & 7) as u32) << 4
        | ((n[1] & 8) as u32) << 4
        | (n[4] & 7) as u32
        | (n[3] & 8) as u32;
    let (value, compare) = if n.len() == 6 {
        (
            ((n[1] & 7) << 4) | ((n[0] & 8) << 4) | (n[0] & 7) | (n[5] & 8),
            None,
        )
    } else {
        (
            ((n[1] & 7) << 4) | ((n[0] & 8) << 4) | (n[0] & 7) | (n[7] & 8),
            Some(((n[7] & 7) << 4) | ((n[6] & 8) << 4) | (n[6] & 7) | (n[5] & 8)),
        )
    };
    Ok(Op::Rom {
        cpu_addr,
        value: value as u32,
        width: 1,
        compare,
    })
}

// ---------------------------------------------------------------- Super NES

const SNES_GG: &str = "DF4709156BC8A23E";

fn snes(part: &str) -> Parsed {
    let up = part.to_ascii_uppercase();
    if up.len() == 9 && up.as_bytes()[4] == b'-' {
        return parsed(part, "Game Genie", snes_game_genie(&up));
    }
    let compact: String = up.chars().filter(|c| *c != ':').collect();
    if is_hex(&compact) && compact.len() == 8 {
        let addr = hex(&compact[..6]).unwrap_or(0);
        let value = hex(&compact[6..]).unwrap_or(0);
        let bank = addr >> 16;
        let low_ram = (bank & 0x7F) < 0x40 && (addr & 0xFFFF) < 0x2000;
        let op = if (0x7E..=0x7F).contains(&bank) || low_ram {
            Op::Ram {
                addr,
                value,
                width: 1,
            }
        } else {
            Op::Rom {
                cpu_addr: addr,
                value,
                width: 1,
                compare: None,
            }
        };
        return parsed(part, "Pro Action Replay", Ok(op));
    }
    parsed(
        part,
        "Unknown",
        Err("expected Game Genie XXXX-XXXX or Action Replay AAAAAA:VV".to_string()),
    )
}

pub fn snes_game_genie(code: &str) -> Result<Op, String> {
    let s: String = code.chars().filter(|c| *c != '-').collect();
    if s.len() != 8 {
        return Err("SNES Game Genie codes are XXXX-XXXX".to_string());
    }
    let mut data: u32 = 0;
    for c in s.chars() {
        let v = SNES_GG
            .find(c)
            .ok_or_else(|| "SNES Game Genie codes use the digits DF4709156BC8A23E".to_string())?;
        data = (data << 4) | v as u32;
    }
    let value = data >> 24;
    let a = data & 0xFFFFFF;
    let cpu_addr = ((a & 0x003C00) << 10)
        | ((a & 0x00003C) << 14)
        | ((a & 0xF00000) >> 8)
        | ((a & 0x000003) << 10)
        | ((a & 0x00C000) >> 6)
        | ((a & 0x0F0000) >> 12)
        | ((a & 0x0003C0) >> 6);
    Ok(Op::Rom {
        cpu_addr,
        value,
        width: 1,
        compare: None,
    })
}

// ---------------------------------------------------------------- Mega Drive

const GEN_GG: &str = "ABCDEFGHJKLMNPRSTVWXYZ0123456789";
const GEN_LAYOUT: &str = "ijklmnopIJKLMNOPABCDEFGHdefghabcQRSTUVWX";

fn genesis(part: &str) -> Parsed {
    let up = part.to_ascii_uppercase();
    if let Some((a, v)) = up.split_once(':') {
        let r = (|| {
            if !is_hex(a) || !is_hex(v) || a.len() != 6 || !(v.len() == 2 || v.len() == 4) {
                return Err("Action Replay codes are AAAAAA:VVVV or AAAAAA:VV".to_string());
            }
            let addr = hex(a)?;
            let value = hex(v)?;
            let width = (v.len() / 2) as u8;
            if addr >= 0xE00000 {
                Ok(Op::Ram {
                    addr: 0xFF0000 | (addr & 0xFFFF),
                    value,
                    width,
                })
            } else if addr < 0x400000 {
                Ok(Op::Rom {
                    cpu_addr: addr,
                    value,
                    width,
                    compare: None,
                })
            } else {
                Err(format!("${addr:06X} is neither ROM nor work RAM"))
            }
        })();
        return parsed(part, "Pro Action Replay", r);
    }
    parsed(part, "Game Genie", genesis_game_genie(&up))
}

pub fn genesis_game_genie(code: &str) -> Result<Op, String> {
    let s: String = code.chars().filter(|c| *c != '-').collect();
    if s.len() != 8 {
        return Err("Mega Drive Game Genie codes are XXXX-XXXX".to_string());
    }
    let mut bits = Vec::with_capacity(40);
    for c in s.chars() {
        let v = GEN_GG
            .find(c)
            .ok_or_else(|| format!("'{c}' is not a Mega Drive Game Genie character"))?;
        for i in (0..5).rev() {
            bits.push(((v >> i) & 1) as u32);
        }
    }
    let mut addr_bits = [0u32; 24];
    let mut data_bits = [0u32; 16];
    for (bit, label) in bits.iter().zip(GEN_LAYOUT.chars()) {
        if label.is_ascii_uppercase() {
            addr_bits[(label as u8 - b'A') as usize] = *bit;
        } else {
            data_bits[(label as u8 - b'a') as usize] = *bit;
        }
    }
    let cpu_addr = addr_bits.iter().fold(0u32, |acc, b| (acc << 1) | b);
    let value = data_bits.iter().fold(0u32, |acc, b| (acc << 1) | b);
    Ok(Op::Rom {
        cpu_addr,
        value,
        width: 2,
        compare: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn genesis_game_genie_known_vectors() {
        // Mortal Kombat II (World): verified against the ROM bytes.
        assert_eq!(
            genesis_game_genie("ALAA-AA9C").unwrap(),
            Op::Rom {
                cpu_addr: 0x0080E2,
                value: 0x6002,
                width: 2,
                compare: None
            }
        );
        assert_eq!(
            genesis_game_genie("ABVT-BE64").unwrap(),
            Op::Rom {
                cpu_addr: 0x00639A,
                value: 0x7200,
                width: 2,
                compare: None
            }
        );
    }

    #[test]
    fn genesis_action_replay_kinds() {
        let ram = parse(Platform::Genesis, "FFB622:0078");
        assert_eq!(
            ram[0].op,
            Some(Op::Ram {
                addr: 0xFFB622,
                value: 0x78,
                width: 2
            })
        );
        let rom = parse(Platform::Genesis, "00639A:7200");
        assert_eq!(
            rom[0].op,
            Some(Op::Rom {
                cpu_addr: 0x639A,
                value: 0x7200,
                width: 2,
                compare: None
            })
        );
    }

    #[test]
    fn nes_game_genie_shapes() {
        let six = nes_game_genie("SXIOPO").unwrap();
        let eight = nes_game_genie("ZEXPYGLA").unwrap();
        assert!(matches!(
            six,
            Op::Rom {
                compare: None,
                width: 1,
                ..
            }
        ));
        assert!(matches!(
            eight,
            Op::Rom {
                compare: Some(_),
                width: 1,
                ..
            }
        ));
        assert!(nes_game_genie("SXIOP").is_err());
        assert!(nes_game_genie("BBBBBB").is_err());
    }

    #[test]
    fn nes_game_genie_bit_layout() {
        assert_eq!(
            nes_game_genie("AAAAAA").unwrap(),
            Op::Rom {
                cpu_addr: 0x8000,
                value: 0,
                width: 1,
                compare: None
            }
        );
        assert_eq!(
            nes_game_genie("NNNNNN").unwrap(),
            Op::Rom {
                cpu_addr: 0xFFFF,
                value: 0xFF,
                width: 1,
                compare: None
            }
        );
    }

    #[test]
    fn snes_game_genie_bit_layout() {
        assert_eq!(
            snes_game_genie("DDDD-DDDD").unwrap(),
            Op::Rom {
                cpu_addr: 0,
                value: 0,
                width: 1,
                compare: None
            }
        );
        assert_eq!(
            snes_game_genie("EEEE-EEEE").unwrap(),
            Op::Rom {
                cpu_addr: 0xFFFFFF,
                value: 0xFF,
                width: 1,
                compare: None
            }
        );
    }

    #[test]
    fn snes_action_replay_kinds() {
        assert_eq!(
            parse(Platform::Snes, "7E0DBF:63")[0].op,
            Some(Op::Ram {
                addr: 0x7E0DBF,
                value: 0x63,
                width: 1
            })
        );
        assert_eq!(
            parse(Platform::Snes, "C0FFEE:EA")[0].op,
            Some(Op::Rom {
                cpu_addr: 0xC0FFEE,
                value: 0xEA,
                width: 1,
                compare: None
            })
        );
    }

    #[test]
    fn multipart_codes_split() {
        let parts = parse(Platform::Genesis, "9VVT-BCRJ+SBVT-AAGL+EKVT-BPRN");
        assert_eq!(parts.len(), 3);
        assert!(parts.iter().all(|p| p.op.is_some()));
    }
}
