//! Developer CLI for reconnaissance and asset inspection.

mod inventory;
mod pe;

use anyhow::{Context, Result, bail};
use std::path::PathBuf;

const USAGE: &str = "\
usage: ddg-tools <command> [args]

commands:
  inventory <dir>                 list files with size, magic, hash; extension histogram
  manifest <dir> <variant>        print a data-manifest TOML (hashes only) for <dir>
  pe <file>                       PE sections/entropy, imports, Rich header, resources
  strings <file> [--min N] [--jp] ASCII and Shift-JIS strings with offsets";

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(cmd) = args.first() else {
        println!("{USAGE}");
        return Ok(());
    };
    let path = |i: usize| -> Result<PathBuf> {
        args.get(i)
            .map(PathBuf::from)
            .with_context(|| format!("missing argument\n\n{USAGE}"))
    };

    match cmd.as_str() {
        "inventory" => inventory::print_inventory(&path(1)?),
        "manifest" => {
            let variant = args.get(2).context("missing variant name")?;
            inventory::print_manifest(&path(1)?, variant)
        }
        "pe" => pe::print_report(&path(1)?),
        "strings" => {
            let mut min = 6;
            let mut jp_only = false;
            let mut rest = args[2..].iter();
            while let Some(a) = rest.next() {
                match a.as_str() {
                    "--min" => min = rest.next().context("--min needs a value")?.parse()?,
                    "--jp" => jp_only = true,
                    other => bail!("unknown option {other}"),
                }
            }
            let data = std::fs::read(path(1)?)?;
            for s in ddg_formats::strings::scan(&data, min) {
                if !jp_only || s.japanese {
                    println!("{:08x}  {}", s.offset, s.text.replace('\n', "\\n"));
                }
            }
            Ok(())
        }
        "-h" | "--help" | "help" => {
            println!("{USAGE}");
            Ok(())
        }
        other => bail!("unknown command {other}\n\n{USAGE}"),
    }
}
