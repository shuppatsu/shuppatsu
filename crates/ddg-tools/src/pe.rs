//! PE (32-bit) analysis: headers, sections and entropy, imports, exports, Rich header,
//! resources.

use anyhow::{Context, Result};
use pelite::pe32::{Pe, PeFile};
use pelite::resources::{Directory, Entry, Name};
use std::path::Path;

pub fn entropy(data: &[u8]) -> f64 {
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
        .sum::<f64>()
        // Normalise -0.0 (single-symbol data) to 0.0.
        + 0.0
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
    let ne = u32_at(&data, 0x3c).map(|o| o as usize);
    if let Some(ne) = ne.filter(|&o| data.get(o..o + 2) == Some(b"NE")) {
        return print_ne_report(path, &data, ne);
    }
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
                println!("{dll}: {}", import_names(pe, &desc).join(", "));
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

/// Import names for one descriptor. Borland linkers leave the import name table
/// (OriginalFirstThunk) empty, so fall back to the on-disk IAT, which then still
/// holds the hint/name RVAs.
fn import_names<'a>(
    pe: PeFile<'a>,
    desc: &pelite::pe32::imports::Desc<'a, PeFile<'a>>,
) -> Vec<String> {
    use pelite::pe32::imports::Import;
    let from_int: Vec<String> = desc
        .int()
        .map(|int| {
            int.flatten()
                .map(|imp| match imp {
                    Import::ByName { name, .. } => name.to_string(),
                    Import::ByOrdinal { ord } => format!("#{ord}"),
                })
                .collect()
        })
        .unwrap_or_default();
    if !from_int.is_empty() {
        return from_int;
    }
    let Ok(iat) = desc.iat() else {
        return Vec::new();
    };
    iat.take_while(|&&v| v != 0)
        .map(|&v| {
            if v & 0x8000_0000 != 0 {
                format!("#{}", v & 0xffff)
            } else {
                pe.derva_c_str(v + 2)
                    .map(|s| s.to_string())
                    .unwrap_or_else(|_| format!("<rva {v:#x}>"))
            }
        })
        .collect()
}

fn u16_at(d: &[u8], off: usize) -> Option<u16> {
    d.get(off..off + 2)
        .map(|b| u16::from_le_bytes([b[0], b[1]]))
}

/// Pascal-style (length-prefixed) string at `off`.
fn pstr(d: &[u8], off: usize) -> Option<String> {
    let len = *d.get(off)? as usize;
    d.get(off + 1..off + 1 + len)
        .map(|s| String::from_utf8_lossy(s).into_owned())
}

/// Minimal report for 16-bit NE modules: module name, linker, imported modules,
/// resident (exported) names.
fn print_ne_report(path: &Path, data: &[u8], ne: usize) -> Result<()> {
    let field = |off: usize| u16_at(data, ne + off).context("truncated NE header");
    println!("# {}", path.display());
    println!(
        "16-bit NE module, linker {}.{}, flags {:#06x}, target OS {}",
        data[ne + 2],
        data[ne + 3],
        field(0x0c)?,
        data.get(ne + 0x36).copied().unwrap_or(0)
    );
    let modref_count = field(0x1e)? as usize;
    let resident = ne + field(0x26)? as usize;
    let modref = ne + field(0x28)? as usize;
    let imported = ne + field(0x2a)? as usize;

    let mut names = Vec::new();
    let mut off = resident;
    while let Some(name) = pstr(data, off).filter(|s| !s.is_empty()) {
        let ordinal = u16_at(data, off + 1 + name.len()).unwrap_or(0);
        names.push(format!("{name}@{ordinal}"));
        off += name.len() + 3;
    }
    println!("resident names: {}", names.join(", "));

    let modules: Vec<String> = (0..modref_count)
        .filter_map(|i| u16_at(data, modref + 2 * i))
        .filter_map(|o| pstr(data, imported + o as usize))
        .collect();
    println!("imported modules: {}", modules.join(", "));
    Ok(())
}
