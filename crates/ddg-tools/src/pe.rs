//! PE (32-bit) analysis: headers, sections and entropy, imports, exports, Rich header,
//! resources.

use anyhow::{Context, Result};
use pelite::pe32::{Pe, PeFile};
use pelite::resources::{Directory, Entry, Name};
use std::path::Path;

fn entropy(data: &[u8]) -> f64 {
    if data.is_empty() {
        return 0.0;
    }
    let mut counts = [0u64; 256];
    for &b in data {
        counts[b as usize] += 1;
    }
    let n = data.len() as f64;
    counts
        .iter()
        .filter(|&&c| c > 0)
        .map(|&c| {
            let p = c as f64 / n;
            -p * p.log2()
        })
        .sum()
}

fn u32_at(d: &[u8], off: usize) -> Option<u32> {
    d.get(off..off + 4)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

/// Decodes the Rich header into (product id, build, count) triples.
fn rich_header(data: &[u8]) -> Option<Vec<(u16, u16, u32)>> {
    let pe_off = u32_at(data, 0x3c)? as usize;
    let stub = data.get(..pe_off)?;
    let rich = stub.windows(4).rposition(|w| w == b"Rich")?;
    let key = u32_at(stub, rich + 4)?;
    // Walk backwards from "Rich" until the XOR-ed "DanS" marker.
    let mut pos = rich;
    let mut entries = Vec::new();
    while pos >= 8 {
        pos -= 8;
        let a = u32_at(stub, pos)? ^ key;
        let b = u32_at(stub, pos + 4)? ^ key;
        if a == 0x536e_6144 {
            // "DanS"; the first three dword pairs after it are padding.
            entries.reverse();
            return Some(entries);
        }
        if a != 0 || b != 0 {
            entries.push(((a >> 16) as u16, a as u16, b));
        }
    }
    None
}

fn name_str(name: Name<'_>) -> String {
    match name {
        Name::Id(id) => format!("#{id}"),
        Name::Str(s) => s.to_string(),
        Name::Wide(w) => String::from_utf16_lossy(w),
    }
}

fn walk_resources(dir: Directory<'_>, depth: usize, path: &mut Vec<String>) {
    for e in dir.entries() {
        let name = e.name().map(name_str).unwrap_or_else(|_| "?".into());
        path.push(name);
        match e.entry() {
            Ok(Entry::Directory(sub)) if depth < 4 => walk_resources(sub, depth + 1, path),
            Ok(Entry::DataEntry(de)) => {
                println!("  {}  ({} bytes)", path.join("/"), de.size());
            }
            _ => {}
        }
        path.pop();
    }
}

pub fn print_report(path: &Path) -> Result<()> {
    let data = std::fs::read(path)?;
    let pe = PeFile::from_bytes(&data).context("not a 32-bit PE")?;
    let fh = pe.file_header();
    let oh = pe.optional_header();

    println!("# {}", path.display());
    println!(
        "machine {:#06x}  timestamp {:#010x}  characteristics {:#06x}",
        fh.Machine, fh.TimeDateStamp, fh.Characteristics
    );
    println!(
        "linker {}.{}  subsystem {}  image base {:#010x}  entry {:#010x}  OS {}.{}",
        oh.LinkerVersion.Major,
        oh.LinkerVersion.Minor,
        oh.Subsystem,
        oh.ImageBase,
        oh.AddressOfEntryPoint,
        oh.OperatingSystemVersion.Major,
        oh.OperatingSystemVersion.Minor
    );

    println!("\n## Sections\nname      vaddr      vsize      rawoff     rawsize    entropy  flags");
    for s in pe.section_headers() {
        let start = s.PointerToRawData as usize;
        let end = (start + s.SizeOfRawData as usize).min(data.len());
        let raw = data.get(start..end).unwrap_or(&[]);
        println!(
            "{:<9} {:#010x} {:#010x} {:#010x} {:#010x} {:7.3}  {:#010x}",
            String::from_utf8_lossy(&s.Name).trim_end_matches('\0'),
            s.VirtualAddress,
            s.VirtualSize,
            s.PointerToRawData,
            s.SizeOfRawData,
            entropy(raw),
            s.Characteristics
        );
    }
    let last_end = pe
        .section_headers()
        .iter()
        .map(|s| s.PointerToRawData as usize + s.SizeOfRawData as usize)
        .max()
        .unwrap_or(0);
    if last_end < data.len() {
        println!(
            "overlay: {} bytes at {:#x} (entropy {:.3})",
            data.len() - last_end,
            last_end,
            entropy(&data[last_end..])
        );
    }

    println!("\n## Rich header");
    match rich_header(&data) {
        Some(entries) => {
            println!("prodid  build   count");
            for (id, build, count) in entries {
                println!("{id:<7} {build:<7} {count}");
            }
        }
        None => println!("(none)"),
    }

    println!("\n## Imports");
    match pe.imports() {
        Ok(imports) => {
            for desc in imports {
                let dll = desc.dll_name().map(|n| n.to_string()).unwrap_or_default();
                let mut names: Vec<String> = Vec::new();
                if let Ok(int) = desc.int() {
                    for imp in int.flatten() {
                        names.push(match imp {
                            pelite::pe32::imports::Import::ByName { name, .. } => name.to_string(),
                            pelite::pe32::imports::Import::ByOrdinal { ord } => format!("#{ord}"),
                        });
                    }
                }
                println!("{dll} ({}): {}", names.len(), names.join(", "));
            }
        }
        Err(e) => println!("(none: {e})"),
    }

    println!("\n## Exports");
    match pe.exports().and_then(|e| e.by()) {
        Ok(by) => {
            for name in by.iter_names().filter_map(|(n, _)| n.ok()) {
                println!("  {name}");
            }
        }
        Err(e) => println!("(none: {e})"),
    }

    println!("\n## Resources");
    match pe.resources() {
        Ok(res) => {
            if let Ok(root) = res.root() {
                walk_resources(root, 0, &mut Vec::new());
            }
            if let Ok(vi) = res.version_info() {
                for lang in vi.translation() {
                    vi.strings(*lang, |k, v| println!("  version {k} = {v}"));
                }
            }
        }
        Err(e) => println!("(none: {e})"),
    }
    Ok(())
}
