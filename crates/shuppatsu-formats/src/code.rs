//! Tables the original builds on the stack from code immediates. Some per-stage tables are not
//! in the data sections: a function's prologue stores them into a local array with `mov`
//! instructions. [`stack_table`] reads such an array back out of the code, so it still comes
//! from the user's exe.

use crate::error::{FormatError, Result};

const WHAT: &str = "stack table code";

fn bad(detail: &str) -> FormatError {
    FormatError::invalid(WHAT, detail.to_string())
}

/// Length of a memory operand whose ModRM is at `code[i]`: absolute `[disp32]` (mod 0,
/// r/m 5) or a SIB without a base register (`[index·s + disp32]`). Other forms end the run.
fn absolute_operand_len(code: &[u8], i: usize) -> Option<usize> {
    let modrm = *code.get(i)?;
    match (modrm >> 6, modrm & 7) {
        (0, 5) => Some(5),
        (0, 4) if code.get(i + 1)? & 7 == 5 => Some(6),
        _ => None,
    }
}

/// The `len` dwords a function stores to its local array at the bottom of its frame, from its
/// first instructions.
///
/// The code must start with `sub esp, imm8` or `sub esp, imm32`. The run that follows may hold
/// register pushes and immediate pushes (arguments for a later call, which move the array
/// further from esp), `mov r32, imm32`, `xor r32, r32`, loads of other values into registers
/// (`mov r32, [abs]`, `movsx r32, word [abs]`; the register's value is then unknown), and the
/// stores `mov [esp + disp], r32` / `mov [esp + disp], imm32` with 8- or 32-bit displacements.
/// The array starts at `[esp]` after the `sub`. The run ends at the first other instruction;
/// every entry must have been written by then.
pub fn stack_table(code: &[u8], len: usize) -> Result<Vec<i32>> {
    let byte = |i: usize| code.get(i).copied().ok_or_else(|| bad("truncated"));
    let imm = |i: usize| -> Result<i32> {
        let b = code.get(i..i + 4).ok_or_else(|| bad("truncated"))?;
        Ok(i32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    };
    let mut i = match code.get(..2) {
        Some([0x83, 0xec]) => 3,
        Some([0x81, 0xec]) => 6,
        _ => return Err(bad("no sub esp prologue")),
    };
    let mut base = 0usize;
    let mut table: Vec<Option<i32>> = vec![None; len];
    let mut regs: [Option<i32>; 8] = [None; 8];
    loop {
        let op = byte(i)?;
        match op {
            0x50..=0x57 => {
                base += 4;
                i += 1;
            }
            0x68 => {
                base += 4;
                i += 5;
            }
            0x6a => {
                base += 4;
                i += 2;
            }
            0xb8..=0xbf => {
                regs[usize::from(op - 0xb8)] = Some(imm(i + 1)?);
                i += 5;
            }
            // xor r32, r/m32 with two registers.
            0x33 if byte(i + 1)? >> 6 == 3 => {
                let m = byte(i + 1)?;
                let (r, rm) = (usize::from((m >> 3) & 7), usize::from(m & 7));
                regs[r] = if r == rm {
                    Some(0)
                } else {
                    regs[r].zip(regs[rm]).map(|(a, b)| a ^ b)
                };
                i += 2;
            }
            // mov r32, [abs] / movsx r32, word [abs]: the register now holds data.
            0x8b | 0x0f => {
                let at = if op == 0x0f {
                    if byte(i + 1)? != 0xbf {
                        break;
                    }
                    i + 2
                } else {
                    i + 1
                };
                let Some(n) = absolute_operand_len(code, at) else {
                    break;
                };
                regs[usize::from((byte(at)? >> 3) & 7)] = None;
                i = at + n;
            }
            0x89 | 0xc7 => {
                let modrm = byte(i + 1)?;
                if modrm & 7 != 4 || byte(i + 2)? != 0x24 {
                    break;
                }
                let (disp, n) = match modrm >> 6 {
                    1 => (i64::from(byte(i + 3)? as i8), 4),
                    2 => (i64::from(imm(i + 3)?), 7),
                    _ => break,
                };
                let value = if op == 0x89 {
                    regs[usize::from((modrm >> 3) & 7)].ok_or_else(|| bad("unknown register"))?
                } else {
                    if (modrm >> 3) & 7 != 0 {
                        break;
                    }
                    imm(i + n)?
                };
                let off = disp - base as i64;
                if off < 0 || off % 4 != 0 || off as usize / 4 >= len {
                    return Err(bad("store outside the table"));
                }
                table[off as usize / 4] = Some(value);
                i += n + if op == 0xc7 { 4 } else { 0 };
            }
            _ => break,
        }
    }
    table
        .into_iter()
        .map(|v| v.ok_or_else(|| bad("entry not written")))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shape of 0x408f40's prologue: `sub esp, imm8`, register stores before and after
    /// pushes, a `movsx` and a load that clobber registers, `xor esi, esi`, imm32 stores.
    #[test]
    fn follows_pushes_and_clobbered_registers() {
        let mut code = vec![0x83, 0xec, 0x0c];
        code.extend([0xb8, 0x55, 0, 0, 0]); // mov eax, 0x55
        code.extend([0x89, 0x44, 0x24, 0x00]); // mov [esp], eax
        code.extend([0x0f, 0xbf, 0x05, 0xa4, 0x70, 0x85, 0x00]); // movsx eax, word [abs]
        code.push(0x56); // push esi
        code.extend([0x8b, 0x04, 0x85, 0x18, 0x3c, 0x46, 0x00]); // mov eax, [eax·4 + abs]
        code.extend([0x33, 0xf6]); // xor esi, esi
        code.extend([0x89, 0x74, 0x24, 0x08]); // mov [esp + 8], esi (entry 1)
        code.extend([0xc7, 0x44, 0x24, 0x0c, 0x90, 0, 0, 0]); // entry 2 = 0x90
        code.extend([0xe8, 0, 0, 0, 0]); // call
        assert_eq!(stack_table(&code, 3).unwrap(), [0x55, 0, 0x90]);
        // A store of a clobbered register is refused.
        let mut bad = code[..20].to_vec();
        bad.extend([0x89, 0x44, 0x24, 0x08, 0xe8]);
        assert!(stack_table(&bad, 3).is_err());
    }

    #[test]
    fn needs_every_entry() {
        let code = [0x81, 0xec, 0x40, 0x05, 0, 0, 0xc3];
        assert!(stack_table(&code, 1).is_err());
        assert!(stack_table(&[0xc3], 1).is_err());
    }
}
