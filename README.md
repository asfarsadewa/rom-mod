# rom-mod

A cheat-to-patch workbench for NES, Super NES and Mega Drive ROMs. One binary, no runtime, a local UI.

Point it at a ROM folder. It identifies each file from its header, pulls the matching cheat list from the [libretro cheat database](https://github.com/libretro/libretro-database), and decodes every code against the exact bytes of your file before anything is written. Codes that patch the cartridge become a patched ROM plus an IPS. Codes that poke work RAM become a RetroArch cheat file.

## Install

```
cargo install --path .
```

Or grab a build from the CI artifacts. There are no dependencies to install.

## Use

```
rom-mod --library D:\ROM
```

Opens the workbench in your browser. Scan any folder; zipped ROMs are fine. Pick a ROM, tick cheats, build.

The same engine is available from the terminal:

```
rom-mod info  "Contra (USA).zip"
rom-mod decode "Mortal Kombat II (World).zip" ALAA-AA9C
rom-mod patch  "Mortal Kombat II (World).zip" --code ALAA-AA9C --label "Infinite health"
```

## What it understands

| Platform | Containers | Codes | Patch output |
| --- | --- | --- | --- |
| NES | iNES, NES 2.0, zip | Game Genie (6 and 8 letter), FCEUX raw `AAAA:VV` | PRG bytes, bank-aware |
| Super NES | LoROM, HiROM, ExHiROM, copier headers, zip | Game Genie, Pro Action Replay | ROM bytes, checksum repaired |
| Mega Drive | plain binary, interleaved `.smd`, zip | Game Genie, Pro Action Replay | ROM bytes, checksum repaired |

Every decoded code shows the file offset, the byte that is there now, and the byte it becomes, so a code that targets another revision is obvious before you build. NES codes without a compare byte are applied to every bank that can appear at that address, exactly as the cartridge device would. Codes that write to a fixed bank resolve to a single offset.

Runtime codes are written to `<RetroArch>/cheats/<core>/<content name>.cht`, which RetroArch picks up from Quick Menu → Cheats → Load Cheat File.

## Build

```
cargo build --release
```

The UI is three hand-written files in `ui/`, embedded at compile time. There is no bundler and no framework.

## License

MIT
