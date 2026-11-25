//! InstallShield 5 cabinets (`data1.cab`, signature `ISc(`), which hold the game's installed
//! files on the disc (docs/recon.md §6, docs/re/disc-import.md).
//!
//! Only what the game's disc uses: a single-volume cabinet whose header is inside the cabinet
//! itself (no `data1.hdr`), version word `0x01000004`. Layout (offsets little-endian):
//!
//! * common header at 0: signature, version, volume info, cabinet descriptor offset and size;
//! * cabinet descriptor: at +0x0c the file table offset (relative to the descriptor), at +0x14
//!   its size, +0x1c the directory count, +0x28 the file count;
//! * file table: `directories + files` u32 offsets relative to the table. Directory entries
//!   point at NUL-terminated names; file entries at 42-byte descriptors: name offset (u32,
//!   relative to the table), directory index (u32), flags (u16: 1 split, 2 obfuscated,
//!   4 compressed, 8 invalid), expanded size (u32), compressed size (u32), 20 unknown bytes,
//!   data offset (u32, from the start of the cabinet);
//! * compressed file data: chunks of a u16 length followed by that many bytes of an
//!   independent raw DEFLATE stream ([`crate::inflate`]).
//!
//! Structure confirmed against the disc and the open-source `unshield` 1.6.2 (MIT) as an
//! oracle: all 57 files extract to the same bytes (`shuppatsu-tools import` test).

use crate::disc::ReadAt;
use crate::error::{FormatError, Result};
use crate::inflate::inflate;
use crate::reader::Bytes;
use encoding_rs::SHIFT_JIS;

const WHAT: &str = "InstallShield cabinet";
const SIGNATURE: &[u8; 4] = b"ISc(";

const FLAG_SPLIT: u16 = 1;
const FLAG_OBFUSCATED: u16 = 2;
const FLAG_COMPRESSED: u16 = 4;
const FLAG_INVALID: u16 = 8;

/// One file of a cabinet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CabFile {
    /// `DIR/NAME`, or just `NAME` for files in the root directory.
    pub path: String,
    pub expanded_size: u32,
    pub compressed_size: u32,
    pub data_offset: u32,
    flags: u16,
}

impl CabFile {
    fn compressed(&self) -> bool {
        self.flags & FLAG_COMPRESSED != 0
    }
}

/// A cabinet's file list.
#[derive(Debug, Clone)]
pub struct Cabinet {
    pub files: Vec<CabFile>,
}

impl Cabinet {
    /// Reads the header and file table from the start of the cabinet.
    pub fn read(src: &(impl ReadAt + ?Sized)) -> Result<Self> {
        let mut common = [0u8; 20];
        src.read_at(0, &mut common)?;
        if &common[..4] != SIGNATURE {
            return Err(FormatError::BadMagic {
                what: WHAT,
                found: common[..4].to_vec(),
            });
        }
        let c = Bytes::new(&common, WHAT);
        let version = c.u32(4)?;
        if version >> 24 != 1 {
            return Err(FormatError::invalid(
                WHAT,
                format!("version {version:#010x}: only InstallShield 5 (0x01xxxxxx) is supported"),
            ));
        }
        let desc_offset = u64::from(c.u32(12)?);

        let mut desc = [0u8; 0x30];
        src.read_at(desc_offset, &mut desc)?;
        let d = Bytes::new(&desc, WHAT);
        let table_offset = d.u32(0x0c)?;
        let table_size = d.u32(0x14)?;
        let dir_count = d.u32(0x1c)? as usize;
        let file_count = d.u32(0x28)? as usize;
        if dir_count + file_count > 100_000 || table_size > 16 << 20 {
            return Err(FormatError::invalid(WHAT, "implausible file table"));
        }

        let mut table = vec![0u8; table_size as usize];
        src.read_at(desc_offset + u64::from(table_offset), &mut table)?;
        let t = Bytes::new(&table, "InstallShield file table");
        let string = |offset: u32| -> Result<String> {
            let rest = t.slice(offset as usize, table.len().saturating_sub(offset as usize))?;
            let end = rest.iter().position(|&b| b == 0).ok_or_else(|| {
                FormatError::invalid(WHAT, format!("unterminated name at {offset:#x}"))
            })?;
            Ok(SHIFT_JIS.decode(&rest[..end]).0.into_owned())
        };

        let dirs = (0..dir_count)
            .map(|i| string(t.u32(i * 4)?))
            .collect::<Result<Vec<_>>>()?;
        let mut files = Vec::with_capacity(file_count);
        for i in 0..file_count {
            let at = t.u32((dir_count + i) * 4)? as usize;
            let flags = t.u16(at + 8)?;
            if flags & FLAG_INVALID != 0 {
                continue;
            }
            let name = string(t.u32(at)?)?;
            let dir_index = t.u32(at + 4)? as usize;
            let dir = dirs.get(dir_index).ok_or_else(|| {
                FormatError::invalid(WHAT, format!("{name}: directory {dir_index} out of range"))
            })?;
            let path = if dir.is_empty() {
                name
            } else {
                format!("{}/{name}", dir.replace('\\', "/"))
            };
            files.push(CabFile {
                path,
                flags,
                expanded_size: t.u32(at + 10)?,
                compressed_size: t.u32(at + 14)?,
                data_offset: t.u32(at + 38)?,
            });
        }
        Ok(Self { files })
    }

    /// Reads and decompresses one file.
    pub fn extract(&self, src: &(impl ReadAt + ?Sized), file: &CabFile) -> Result<Vec<u8>> {
        if file.flags & (FLAG_SPLIT | FLAG_OBFUSCATED) != 0 {
            return Err(FormatError::invalid(
                WHAT,
                format!("{}: split or obfuscated files are not supported", file.path),
            ));
        }
        let stored = if file.compressed() {
            file.compressed_size
        } else {
            file.expanded_size
        };
        let mut raw = vec![0u8; stored as usize];
        src.read_at(u64::from(file.data_offset), &mut raw)?;
        if !file.compressed() {
            return Ok(raw);
        }
        let mut out = Vec::with_capacity(file.expanded_size as usize);
        let mut pos = 0usize;
        let r = Bytes::new(&raw, WHAT);
        while pos < raw.len() {
            let len = usize::from(r.u16(pos)?);
            if len == 0 {
                return Err(FormatError::invalid(
                    WHAT,
                    format!("{}: empty chunk", file.path),
                ));
            }
            inflate(r.slice(pos + 2, len)?, &mut out)?;
            pos += 2 + len;
        }
        if out.len() != file.expanded_size as usize {
            return Err(FormatError::invalid(
                WHAT,
                format!(
                    "{}: expanded to {} bytes, expected {}",
                    file.path,
                    out.len(),
                    file.expanded_size
                ),
            ));
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A cabinet with directories "" and "DATA", one stored file and one compressed file
    /// (two chunks), plus an invalid entry that must be skipped.
    fn cabinet() -> Vec<u8> {
        let mut cab = vec![0u8; 0x400];
        cab[..4].copy_from_slice(SIGNATURE);
        cab[4..8].copy_from_slice(&0x0100_0004u32.to_le_bytes());
        cab[12..16].copy_from_slice(&0x40u32.to_le_bytes()); // descriptor at 0x40
        let desc = 0x40;
        let table_off = 0x40u32; // table at 0x80
        let table = 0x80;
        cab[desc + 0x0c..desc + 0x10].copy_from_slice(&table_off.to_le_bytes());
        cab[desc + 0x14..desc + 0x18].copy_from_slice(&0x180u32.to_le_bytes());
        cab[desc + 0x1c..desc + 0x20].copy_from_slice(&2u32.to_le_bytes());
        cab[desc + 0x28..desc + 0x2c].copy_from_slice(&3u32.to_le_bytes());
        let put32 = |cab: &mut Vec<u8>, at: usize, v: u32| {
            cab[at..at + 4].copy_from_slice(&v.to_le_bytes())
        };
        // Entries: dir 0, dir 1, file 0, 1, 2 (offsets relative to the table).
        for (i, off) in [0x20u32, 0x21, 0x30, 0x60, 0x90].into_iter().enumerate() {
            put32(&mut cab, table + i * 4, off);
        }
        cab[table + 0x21..table + 0x26].copy_from_slice(b"DATA\0");
        cab[table + 0x100..table + 0x106].copy_from_slice(b"a.txt\0");
        cab[table + 0x110..table + 0x116].copy_from_slice(b"b.bin\0");
        // file 0: invalid
        cab[table + 0x30 + 8] = FLAG_INVALID as u8;
        // file 1: a.txt in the root, stored, 5 bytes at 0x300
        let f = table + 0x60;
        put32(&mut cab, f, 0x100);
        put32(&mut cab, f + 4, 0);
        put32(&mut cab, f + 10, 5);
        put32(&mut cab, f + 14, 5);
        put32(&mut cab, f + 38, 0x300);
        cab[0x300..0x305].copy_from_slice(b"hello");
        // file 2: DATA/b.bin, compressed: two stored-block chunks "ab" + "cd"
        let f = table + 0x90;
        put32(&mut cab, f, 0x110);
        put32(&mut cab, f + 4, 1);
        cab[f + 8] = FLAG_COMPRESSED as u8;
        put32(&mut cab, f + 10, 4);
        let chunks: [&[u8]; 2] = [
            &[7, 0, 0x01, 0x02, 0x00, 0xfd, 0xff, b'a', b'b'],
            &[7, 0, 0x01, 0x02, 0x00, 0xfd, 0xff, b'c', b'd'],
        ];
        let data: Vec<u8> = chunks.concat();
        put32(&mut cab, f + 14, data.len() as u32);
        put32(&mut cab, f + 38, 0x320);
        cab[0x320..0x320 + data.len()].copy_from_slice(&data);
        cab
    }

    #[test]
    fn lists_and_extracts() {
        let cab = cabinet();
        let c = Cabinet::read(cab.as_slice()).unwrap();
        let paths: Vec<_> = c.files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, ["a.txt", "DATA/b.bin"]);
        assert_eq!(c.extract(cab.as_slice(), &c.files[0]).unwrap(), b"hello");
        assert_eq!(c.extract(cab.as_slice(), &c.files[1]).unwrap(), b"abcd");
    }

    #[test]
    fn rejects_other_formats() {
        let mut cab = cabinet();
        cab[0] = b'M';
        assert!(Cabinet::read(cab.as_slice()).is_err());
        let mut cab = cabinet();
        cab[7] = 2; // a later InstallShield version
        assert!(Cabinet::read(cab.as_slice()).is_err());
    }

    #[test]
    fn checks_the_expanded_size() {
        let cab = cabinet();
        let c = Cabinet::read(cab.as_slice()).unwrap();
        let mut f = c.files[1].clone();
        f.expanded_size = 5;
        assert!(c.extract(cab.as_slice(), &f).is_err());
    }
}
