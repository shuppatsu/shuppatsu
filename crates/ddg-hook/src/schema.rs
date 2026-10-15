//! Trace schema: which globals of the original exe are logged per tick.
//!
//! One field per line: `name address type`, e.g. `speed 0x857234 f32`. `#` starts a comment.
//! Types: u8 i8 u16 i16 u32 i32 f32 f64.

use std::fmt::Write as _;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldType {
    U8,
    I8,
    U16,
    I16,
    U32,
    I32,
    F32,
    F64,
}

impl FieldType {
    pub fn size(self) -> usize {
        match self {
            Self::U8 | Self::I8 => 1,
            Self::U16 | Self::I16 => 2,
            Self::U32 | Self::I32 | Self::F32 => 4,
            Self::F64 => 8,
        }
    }

    fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "u8" => Self::U8,
            "i8" => Self::I8,
            "u16" => Self::U16,
            "i16" => Self::I16,
            "u32" => Self::U32,
            "i32" => Self::I32,
            "f32" => Self::F32,
            "f64" => Self::F64,
            _ => return None,
        })
    }

    /// Formats little-endian bytes of this type. Floats use their exact bit pattern in
    /// hex after the decimal value, so traces can be compared bit for bit.
    pub fn format(self, b: &[u8], out: &mut String) {
        let _ = match self {
            Self::U8 => write!(out, "{}", b[0]),
            Self::I8 => write!(out, "{}", b[0] as i8),
            Self::U16 => write!(out, "{}", u16::from_le_bytes([b[0], b[1]])),
            Self::I16 => write!(out, "{}", i16::from_le_bytes([b[0], b[1]])),
            Self::U32 => write!(out, "{}", u32::from_le_bytes([b[0], b[1], b[2], b[3]])),
            Self::I32 => write!(out, "{}", i32::from_le_bytes([b[0], b[1], b[2], b[3]])),
            Self::F32 => {
                let bits = u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
                write!(out, "{}:{bits:08x}", f32::from_bits(bits))
            }
            Self::F64 => {
                let bits = u64::from_le_bytes(b[..8].try_into().expect("8 bytes"));
                write!(out, "{}:{bits:016x}", f64::from_bits(bits))
            }
        };
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Field {
    pub name: String,
    pub address: u32,
    pub ty: FieldType,
}

/// Parses a schema file. Errors name the offending line.
pub fn parse(text: &str) -> Result<Vec<Field>, String> {
    let mut fields = Vec::new();
    for (n, line) in text.lines().enumerate() {
        let line = line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let parts: Vec<&str> = line.split_whitespace().collect();
        let [name, addr, ty] = parts[..] else {
            return Err(format!("line {}: expected `name address type`", n + 1));
        };
        let address = u32::from_str_radix(addr.trim_start_matches("0x"), 16)
            .map_err(|e| format!("line {}: address {addr}: {e}", n + 1))?;
        let ty =
            FieldType::parse(ty).ok_or_else(|| format!("line {}: unknown type {ty}", n + 1))?;
        fields.push(Field {
            name: name.to_owned(),
            address,
            ty,
        });
    }
    Ok(fields)
}

/// CSV header for a schema: `tick,` then the field names.
pub fn header(fields: &[Field]) -> String {
    let mut h = String::from("hook_tick");
    for f in fields {
        h.push(',');
        h.push_str(&f.name);
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_formats() {
        let f = parse("# comment\nspeed 0x857234 f32\nnotch 829d00 i32 # trailing\n").unwrap();
        assert_eq!(f.len(), 2);
        assert_eq!(f[0].address, 0x857234);
        assert_eq!(f[1].ty, FieldType::I32);
        assert_eq!(header(&f), "hook_tick,speed,notch");
        let mut s = String::new();
        FieldType::F32.format(&1.5f32.to_le_bytes(), &mut s);
        assert_eq!(s, "1.5:3fc00000");
        assert!(parse("x 0x1").is_err());
        assert!(parse("x zz u8").is_err());
    }
}
