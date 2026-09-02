# rom-mod

Local utility for applying published cheat codes to NES, Super NES and Mega Drive ROM images.

## Overview

- Identifies ROM images by header. Zipped and raw files are accepted.
- Retrieves cheat lists from the libretro cheat database and caches them locally.
- Decodes Game Genie and Action Replay codes against the bytes of the selected file.
- Writes a patched image and an IPS file for codes that modify ROM.
- Writes a RetroArch cheat file for codes that modify RAM at runtime.
- Provides a browser UI served on the loopback interface and an equivalent command-line interface.

## Supported formats

| Platform | Images | Code formats | Notes |
| --- | --- | --- | --- |
| NES | iNES, NES 2.0 | Game Genie (6 and 8 letter), raw `AAAA:VV`, `AAAA?CC:VV` | PRG offsets resolved per mapper; compare bytes filter banks |
| Super NES | LoROM, HiROM, ExHiROM, optional 512-byte copier header | Game Genie, Pro Action Replay | Internal checksum and complement recalculated |
| Mega Drive | Binary, interleaved SMD | Game Genie, Pro Action Replay | Header checksum recalculated |

## Requirements

- Rust 1.82 or later to build. No runtime dependencies.
- Network access to github.com for database lookups. Manually entered codes work offline.

## Installation

```
cargo install --path .
```

Pre-built binaries are attached to tagged releases.

## Usage

```
rom-mod [--library <DIR>]... [serve [--port <PORT>] [--no-open]]
rom-mod info   <ROM>
rom-mod decode <ROM> <CODE>...
rom-mod patch  <ROM> --code <CODE>... [--label <TEXT>] [--out <DIR>] [--overwrite]
```

- `serve` is the default command. The server binds to `127.0.0.1` on port 4310.
- Output files are written next to the source image as `<name> [<label>].<ext>` and `<name> [<label>].ips`. Existing files are not overwritten unless requested.
- RetroArch cheat files are written to `<retroarch>/cheats/<core>/<name>.cht`. The RetroArch directory is detected from common install locations or set with the `ROM_MOD_RETROARCH` environment variable.
- Each decoded code reports the file offset, the current byte and the replacement byte before any write takes place.

## Security model

- The HTTP server listens on the loopback interface only. There is no option to bind elsewhere.
- Every API request must carry a per-session token that is issued with the page. Requests from other origins are rejected.
- The page is served with a Content Security Policy and loads no remote resources.
- ROM images and archive entries larger than 64 MB are refused before allocation.
- Outbound traffic is limited to `api.github.com` and `raw.githubusercontent.com` for cheat data. No telemetry.
- Database content is treated as untrusted input. Every code is parsed and bounds-checked against the file before use.
- Files are written only to the chosen output directory, the cache directory and the RetroArch cheats directory.

## Data

- Cheat data source: `cht/` tree of [libretro-database](https://github.com/libretro/libretro-database).
- Cache location: `%LOCALAPPDATA%\rom-mod\cache` on Windows, `$XDG_CACHE_HOME/rom-mod/cache` or `~/.cache/rom-mod/cache` elsewhere. The index is refreshed after seven days.

## Limitations

- NES bank resolution covers the mappers listed in `src/rom/nes.rs`. Others fall back to 8 KB banks with a warning.
- MMC1 images are assumed to run in fixed-last-bank mode. A compare byte corrects the remaining cases.
- Codes that target a different revision than the file on disk are reported and not applied.
- The selected ROM image is held in memory for the duration of the session.

## Development

- `cargo test`, `cargo clippy --all-targets -- -D warnings` and `cargo fmt --check` are enforced in CI on Windows and Linux.
- `cargo deny check advisories bans sources` runs in CI.
- UI sources are in `ui/` and embedded at compile time. There is no build step for the UI.
- Releases are produced by the `release` workflow on tags matching `v*`.

## License

MIT. See `LICENSE`.
