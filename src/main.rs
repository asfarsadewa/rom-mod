mod cheatdb;
mod codes;
mod patch;
mod rom;
mod server;

use anyhow::Result;
use clap::{Parser, Subcommand};
use codes::Op;
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "rom-mod",
    version,
    about = "Cheat-to-patch workbench for NES, Super NES and Mega Drive ROMs"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Option<Cmd>,
    /// Library folder to scan (repeatable)
    #[arg(short, long, global = true, value_name = "DIR")]
    library: Vec<PathBuf>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Start the workbench in your browser (default)
    Serve {
        #[arg(short, long, default_value_t = 4310)]
        port: u16,
        /// Do not open a browser window
        #[arg(long)]
        no_open: bool,
    },
    /// Print what the header says about a ROM
    Info { rom: PathBuf },
    /// Decode cheat codes against a ROM and show the bytes they would change
    Decode { rom: PathBuf, codes: Vec<String> },
    /// Apply cheat codes and write a patched ROM plus an IPS next to it
    Patch {
        rom: PathBuf,
        /// A code to apply (repeatable)
        #[arg(short, long = "code", required = true)]
        codes: Vec<String>,
        /// Label appended to the output file name
        #[arg(short, long, default_value = "Modded")]
        label: String,
        /// Output folder (defaults to the ROM's folder)
        #[arg(short, long)]
        out: Option<PathBuf>,
        #[arg(long)]
        overwrite: bool,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd.unwrap_or(Cmd::Serve {
        port: 4310,
        no_open: false,
    }) {
        Cmd::Serve { port, no_open } => server::serve(cli.library, port, !no_open).await,
        Cmd::Info { rom } => {
            let r = rom::load(&rom)?;
            print_info(&r);
            Ok(())
        }
        Cmd::Decode { rom, codes } => {
            let r = rom::load(&rom)?;
            for code in codes {
                for part in patch::decode(&r, &code) {
                    print_part(&part);
                }
            }
            Ok(())
        }
        Cmd::Patch {
            rom,
            codes,
            label,
            out,
            overwrite,
        } => {
            let r = rom::load(&rom)?;
            let ops = patch::collect_ops(&r, &codes)?;
            let res = patch::build(&r, &ops, &label, out.as_deref(), overwrite)?;
            println!("wrote {}", res.rom_path);
            println!("wrote {}", res.ips_path);
            println!("{} byte(s) changed", res.changed_bytes);
            if let Some((a, b)) = res.checksum {
                println!("checksum {a} -> {b}");
            }
            println!("sha1 {}", res.sha1);
            Ok(())
        }
    }
}

fn hexs(b: &[u8]) -> String {
    b.iter()
        .map(|x| format!("{x:02X}"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn print_info(r: &rom::Rom) {
    println!("{}", r.name);
    println!("  platform   {}", r.platform.label());
    if !r.info.title.is_empty() {
        println!("  title      {}", r.info.title);
    }
    if !r.info.region.is_empty() {
        println!("  region     {}", r.info.region);
    }
    println!("  size       {}", rom::human_size(r.info.size));
    println!("  sha1       {}", r.info.sha1);
    println!("  crc32      {}", r.info.crc32);
    if let Some(c) = &r.info.checksum {
        let status = if c.valid {
            "valid".to_string()
        } else {
            format!("mismatch, computed {}", c.computed)
        };
        println!("  checksum   {} ({status})", c.stored);
    }
    for f in &r.info.fields {
        println!("  {:<10} {}", f.label.to_lowercase(), f.value);
    }
    for n in &r.info.notes {
        println!("  note       {n}");
    }
}

fn print_part(p: &patch::Decoded) {
    if let Some(e) = &p.error {
        println!("{:<12} {:<18} error: {e}", p.raw, p.format);
        return;
    }
    match &p.op {
        Some(Op::Ram { addr, value, width }) => {
            let w = *width as usize * 2;
            println!(
                "{:<12} {:<18} RAM ${addr:06X} = ${value:0w$X}  (runtime cheat)",
                p.raw, p.format
            );
        }
        Some(Op::Rom {
            cpu_addr,
            value,
            width,
            compare,
        }) => {
            let w = *width as usize * 2;
            let cmp = compare.map(|c| format!(" if ${c:02X}")).unwrap_or_default();
            println!(
                "{:<12} {:<18} ${cpu_addr:06X} = ${value:0w$X}{cmp}",
                p.raw, p.format
            );
            for o in &p.rom_ops {
                println!(
                    "{:>12} file ${:06X}: {} -> {}",
                    "",
                    o.offset,
                    hexs(&o.old),
                    hexs(&o.new)
                );
            }
        }
        None => {}
    }
    for n in &p.notes {
        println!("{:>12} {n}", "");
    }
}
