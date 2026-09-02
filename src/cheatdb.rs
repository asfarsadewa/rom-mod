//! Lookup against the libretro cheat database on GitHub, with a local cache.

use crate::rom::Platform;
use anyhow::{anyhow, bail, Context, Result};
use serde::Serialize;
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const REPO: &str = "libretro/libretro-database";
const INDEX_TTL: Duration = Duration::from_secs(7 * 24 * 3600);

pub fn system_dir(p: Platform) -> &'static str {
    match p {
        Platform::Nes => "Nintendo - Nintendo Entertainment System",
        Platform::Snes => "Nintendo - Super Nintendo Entertainment System",
        Platform::Genesis => "Sega - Mega Drive - Genesis",
    }
}

fn cache_dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("XDG_CACHE_HOME").map(PathBuf::from))
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("rom-mod").join("cache")
}

fn percent_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn get(url: &str) -> Result<String> {
    let resp = ureq::get(url)
        .set(
            "User-Agent",
            "rom-mod (https://github.com/asfarsadewa/rom-mod)",
        )
        .set("Accept", "application/vnd.github+json, text/plain")
        .timeout(Duration::from_secs(30))
        .call();
    match resp {
        Ok(r) => Ok(r.into_string()?),
        Err(ureq::Error::Status(404, _)) => bail!("not found"),
        Err(ureq::Error::Status(code, r)) => {
            bail!(
                "GitHub returned {code}: {}",
                r.into_string().unwrap_or_default()
            )
        }
        Err(e) => bail!("network error: {e}"),
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn slug(p: Platform) -> &'static str {
    match p {
        Platform::Nes => "nes",
        Platform::Snes => "snes",
        Platform::Genesis => "genesis",
    }
}

/// All cheat file names (without extension) for a platform.
pub fn index(p: Platform) -> Result<Vec<String>> {
    let dir = cache_dir();
    fs::create_dir_all(&dir)?;
    let path = dir.join(format!("index-{}.json", slug(p)));
    if let Ok(text) = fs::read_to_string(&path) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
            let fetched = v["fetched"].as_u64().unwrap_or(0);
            if now().saturating_sub(fetched) < INDEX_TTL.as_secs() {
                if let Some(names) = v["names"].as_array() {
                    return Ok(names
                        .iter()
                        .filter_map(|n| n.as_str().map(|s| s.to_string()))
                        .collect());
                }
            }
        }
    }
    let listing = get(&format!("https://api.github.com/repos/{REPO}/contents/cht"))?;
    let listing: serde_json::Value = serde_json::from_str(&listing)?;
    let sha = listing
        .as_array()
        .and_then(|arr| {
            arr.iter()
                .find(|e| e["name"].as_str() == Some(system_dir(p)))
                .and_then(|e| e["sha"].as_str().map(|s| s.to_string()))
        })
        .ok_or_else(|| anyhow!("system folder missing from the database listing"))?;
    let tree = get(&format!(
        "https://api.github.com/repos/{REPO}/git/trees/{sha}"
    ))?;
    let tree: serde_json::Value = serde_json::from_str(&tree)?;
    let names: Vec<String> = tree["tree"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|e| e["path"].as_str())
                .filter_map(|p| p.strip_suffix(".cht").map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();
    if names.is_empty() {
        bail!("database index came back empty");
    }
    let cached = serde_json::json!({ "fetched": now(), "names": names });
    let _ = fs::write(&path, serde_json::to_string(&cached)?);
    Ok(names)
}

fn norm(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric())
        .collect()
}

fn title_of(s: &str) -> String {
    let t = s.split(" (").next().unwrap_or(s);
    norm(t)
}

#[derive(Clone, Debug, Serialize, Default)]
pub struct Match {
    pub exact: Option<String>,
    pub candidates: Vec<String>,
}

pub fn find(p: Platform, name: &str) -> Result<Match> {
    let names = index(p)?;
    let want = norm(name);
    let want_title = title_of(name);
    let exact = names.iter().find(|n| norm(n) == want).cloned();
    let mut candidates: Vec<String> = names
        .iter()
        .filter(|n| title_of(n) == want_title)
        .cloned()
        .collect();
    if candidates.is_empty() && want_title.len() >= 4 {
        candidates = names
            .iter()
            .filter(|n| {
                let t = title_of(n);
                t.starts_with(&want_title) || want_title.starts_with(&t)
            })
            .cloned()
            .collect();
    }
    candidates.sort();
    candidates.truncate(24);
    Ok(Match { exact, candidates })
}

#[derive(Clone, Debug, Serialize)]
pub struct DbCheat {
    pub desc: String,
    pub code: String,
}

pub fn fetch(p: Platform, name: &str) -> Result<Vec<DbCheat>> {
    let dir = cache_dir().join(slug(p));
    fs::create_dir_all(&dir)?;
    let safe: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == ' ' || c == '-' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let path = dir.join(format!("{safe}.cht"));
    let text = match fs::read_to_string(&path) {
        Ok(t) => t,
        Err(_) => {
            let url = format!(
                "https://raw.githubusercontent.com/{REPO}/master/cht/{}/{}.cht",
                percent_encode(system_dir(p)),
                percent_encode(name)
            );
            let t = get(&url).with_context(|| format!("fetching cheats for {name}"))?;
            let _ = fs::write(&path, &t);
            t
        }
    };
    Ok(parse_cht(&text))
}

pub fn parse_cht(text: &str) -> Vec<DbCheat> {
    let mut descs = BTreeMap::new();
    let mut codes = BTreeMap::new();
    for line in text.lines() {
        let line = line.trim();
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let k = k.trim();
        let v = v.trim().trim_matches('"').trim();
        let Some(rest) = k.strip_prefix("cheat") else {
            continue;
        };
        if let Some(n) = rest.strip_suffix("_desc") {
            if let Ok(i) = n.parse::<usize>() {
                descs.insert(i, v.to_string());
            }
        } else if let Some(n) = rest.strip_suffix("_code") {
            if let Ok(i) = n.parse::<usize>() {
                codes.insert(i, v.to_string());
            }
        }
    }
    codes
        .into_iter()
        .filter(|(_, c)| !c.is_empty())
        .map(|(i, code)| DbCheat {
            desc: descs
                .get(&i)
                .cloned()
                .unwrap_or_else(|| format!("Cheat {i}")),
            code,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_cht_pairs() {
        let t = "cheats = 2\n\ncheat0_desc = \"Infinite Energy P1\"\ncheat0_code = \"ALAA-AA9C\"\ncheat0_enable = false\n\ncheat1_desc = \"Two\"\ncheat1_code = \"A+B\"\n";
        let v = parse_cht(t);
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].desc, "Infinite Energy P1");
        assert_eq!(v[1].code, "A+B");
    }

    #[test]
    fn matching_ignores_punctuation() {
        assert_eq!(norm("Mortal Kombat II (World)"), "mortalkombatiiworld");
        assert_eq!(title_of("Contra (USA)"), "contra");
    }

    #[test]
    fn percent_encoding_keeps_unreserved() {
        assert_eq!(
            percent_encode("Sega - Mega Drive"),
            "Sega%20-%20Mega%20Drive"
        );
        assert_eq!(percent_encode("Contra (USA)"), "Contra%20%28USA%29");
    }
}
