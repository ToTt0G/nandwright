//! The Hack instruction set: mnemonic tables, decoding and the ALU.
//!
//! Shared by the assembler, the disassembler and the CPU emulator so that all
//! three agree on every bit.
//!
//! A C-instruction is `111a cccc ccdd djjj`. In this module a *comp code* is
//! the 7-bit value `acccccc` (bits 12..6 of the word), a *dest code* the
//! 3-bit value `ddd` and a *jump code* the 3-bit value `jjj`.

/// A word the official CPU emulator treats as "no instruction here": unloaded
/// ROM cells hold it, and executing it does nothing but advance the PC. Every
/// other word of the form `100x…`, `101x…` or `110x…` is an illegal instruction.
pub const EMPTY_WORD: u16 = 0x8000;

/// The 28 documented computations: canonical mnemonic and comp code.
pub const COMPS: [(&str, u8); 28] = [
    ("0", 0b0101010),
    ("1", 0b0111111),
    ("-1", 0b0111010),
    ("D", 0b0001100),
    ("A", 0b0110000),
    ("!D", 0b0001101),
    ("!A", 0b0110001),
    ("-D", 0b0001111),
    ("-A", 0b0110011),
    ("D+1", 0b0011111),
    ("A+1", 0b0110111),
    ("D-1", 0b0001110),
    ("A-1", 0b0110010),
    ("D+A", 0b0000010),
    ("D-A", 0b0010011),
    ("A-D", 0b0000111),
    ("D&A", 0b0000000),
    ("D|A", 0b0010101),
    ("M", 0b1110000),
    ("!M", 0b1110001),
    ("-M", 0b1110011),
    ("M+1", 0b1110111),
    ("M-1", 0b1110010),
    ("D+M", 0b1000010),
    ("D-M", 0b1010011),
    ("M-D", 0b1000111),
    ("D&M", 0b1000000),
    ("D|M", 0b1010101),
];

/// Operand-swapped spellings the official assembler also accepts. They
/// assemble to the same code as their canonical form.
pub const COMP_ALIASES: [(&str, u8); 6] = [
    ("A+D", 0b0000010),
    ("A&D", 0b0000000),
    ("A|D", 0b0010101),
    ("M+D", 0b1000010),
    ("M&D", 0b1000000),
    ("M|D", 0b1010101),
];

/// Destination mnemonics, indexed by dest code. Only this letter order is
/// accepted by the official assembler (`DM=` and `MA=` are errors).
pub const DESTS: [&str; 8] = ["", "M", "D", "MD", "A", "AM", "AD", "AMD"];

/// Jump mnemonics, indexed by jump code.
pub const JUMPS: [&str; 8] = ["", "JGT", "JEQ", "JGE", "JLT", "JNE", "JLE", "JMP"];

/// RAM address of the first screen word.
pub const SCREEN: u16 = 16384;
/// Number of 16-bit words in the screen memory map.
pub const SCREEN_WORDS: usize = 8192;
/// RAM address of the keyboard register, the highest valid RAM address.
pub const KBD: u16 = 24576;

/// The predefined symbols and their values.
pub const PREDEFINED: [(&str, u16); 23] = [
    ("SP", 0),
    ("LCL", 1),
    ("ARG", 2),
    ("THIS", 3),
    ("THAT", 4),
    ("R0", 0),
    ("R1", 1),
    ("R2", 2),
    ("R3", 3),
    ("R4", 4),
    ("R5", 5),
    ("R6", 6),
    ("R7", 7),
    ("R8", 8),
    ("R9", 9),
    ("R10", 10),
    ("R11", 11),
    ("R12", 12),
    ("R13", 13),
    ("R14", 14),
    ("R15", 15),
    ("SCREEN", SCREEN),
    ("KBD", KBD),
];

/// Value of a predefined symbol.
pub fn predefined(name: &str) -> Option<u16> {
    PREDEFINED.iter().find(|(n, _)| *n == name).map(|&(_, v)| v)
}

/// The comp code for a mnemonic the official assembler accepts (canonical or alias).
pub fn comp_code(mnemonic: &str) -> Option<u8> {
    COMPS
        .iter()
        .chain(COMP_ALIASES.iter())
        .find(|(m, _)| *m == mnemonic)
        .map(|&(_, c)| c)
}

/// The canonical mnemonic of a documented comp code.
pub fn comp_mnemonic(code: u8) -> Option<&'static str> {
    COMPS.iter().find(|&&(_, c)| c == code).map(|&(m, _)| m)
}

/// The dest code for a mnemonic (`"AMD"` → 7). The empty string is not
/// accepted here; an absent destination is simply code 0.
pub fn dest_code(mnemonic: &str) -> Option<u8> {
    DESTS
        .iter()
        .skip(1)
        .position(|&m| m == mnemonic)
        .map(|i| i as u8 + 1)
}

/// The jump code for a mnemonic (`"JMP"` → 7).
pub fn jump_code(mnemonic: &str) -> Option<u8> {
    JUMPS
        .iter()
        .skip(1)
        .position(|&m| m == mnemonic)
        .map(|i| i as u8 + 1)
}

/// The ALU operation a C-instruction performs, after the official CPU
/// emulator's treatment of undocumented comp codes (see [`effective_comp`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum AluOp {
    Zero,
    One,
    MinusOne,
    D,
    Y,
    NotD,
    NotY,
    NegD,
    NegY,
    DPlus1,
    YPlus1,
    DMinus1,
    YMinus1,
    DPlusY,
    DMinusY,
    YMinusD,
    DAndY,
    DOrY,
}

impl AluOp {
    /// The operation of a documented `cccccc` code (the a-bit selects Y).
    const fn from_c_bits(c: u8) -> Option<AluOp> {
        Some(match c {
            0b101010 => AluOp::Zero,
            0b111111 => AluOp::One,
            0b111010 => AluOp::MinusOne,
            0b001100 => AluOp::D,
            0b110000 => AluOp::Y,
            0b001101 => AluOp::NotD,
            0b110001 => AluOp::NotY,
            0b001111 => AluOp::NegD,
            0b110011 => AluOp::NegY,
            0b011111 => AluOp::DPlus1,
            0b110111 => AluOp::YPlus1,
            0b001110 => AluOp::DMinus1,
            0b110010 => AluOp::YMinus1,
            0b000010 => AluOp::DPlusY,
            0b010011 => AluOp::DMinusY,
            0b000111 => AluOp::YMinusD,
            0b000000 => AluOp::DAndY,
            0b010101 => AluOp::DOrY,
            _ => return None,
        })
    }

    /// Computes the operation. `y` is A or M depending on the a-bit.
    #[inline(always)]
    pub fn apply(self, d: u16, y: u16) -> u16 {
        match self {
            AluOp::Zero => 0,
            AluOp::One => 1,
            AluOp::MinusOne => 0xFFFF,
            AluOp::D => d,
            AluOp::Y => y,
            AluOp::NotD => !d,
            AluOp::NotY => !y,
            AluOp::NegD => d.wrapping_neg(),
            AluOp::NegY => y.wrapping_neg(),
            AluOp::DPlus1 => d.wrapping_add(1),
            AluOp::YPlus1 => y.wrapping_add(1),
            AluOp::DMinus1 => d.wrapping_sub(1),
            AluOp::YMinus1 => y.wrapping_sub(1),
            AluOp::DPlusY => d.wrapping_add(y),
            AluOp::DMinusY => d.wrapping_sub(y),
            AluOp::YMinusD => y.wrapping_sub(d),
            AluOp::DAndY => d & y,
            AluOp::DOrY => d | y,
        }
    }
}

/// Whether a comp code is one of the 28 documented computations.
pub fn is_documented_comp(code: u8) -> bool {
    comp_mnemonic(code & 0x7F).is_some()
}

const fn documented(code: u8) -> bool {
    let c = code & 0x3F;
    let op = AluOp::from_c_bits(c);
    if op.is_none() {
        return false;
    }
    if code & 0x40 == 0 {
        return true;
    }
    // With a = 1 only the ten computations that read Y (= M) are documented.
    !matches!(
        op,
        Some(
            AluOp::Zero
                | AluOp::One
                | AluOp::MinusOne
                | AluOp::D
                | AluOp::NotD
                | AluOp::NegD
                | AluOp::DPlus1
                | AluOp::DMinus1
        )
    )
}

const fn build_effective() -> [u8; 128] {
    let mut table = [0u8; 128];
    let mut code = 0usize;
    let mut last = 0u8; // code 0 (D&A) is documented
    while code < 128 {
        if documented(code as u8) {
            last = code as u8;
        }
        table[code] = last;
        code += 1;
    }
    table
}

/// For each of the 128 comp codes, the documented code the official CPU
/// emulator actually executes.
///
/// Observed behaviour: an undocumented comp code computes exactly what the
/// nearest *lower* documented code computes (for example `1001101`, which no
/// mnemonic produces, behaves like `1000111`, `M-D`). This was verified for
/// all 128 codes against the Java emulator; it is not what the hardware ALU
/// of chapter 2 would compute.
pub const EFFECTIVE_COMP: [u8; 128] = build_effective();

/// The documented comp code the official CPU emulator executes for `code`.
pub fn effective_comp(code: u8) -> u8 {
    EFFECTIVE_COMP[(code & 0x7F) as usize]
}

const fn build_ops() -> [AluOp; 128] {
    let mut ops = [AluOp::Zero; 128];
    let mut code = 0usize;
    while code < 128 {
        let eff = EFFECTIVE_COMP[code];
        ops[code] = match AluOp::from_c_bits(eff & 0x3F) {
            Some(op) => op,
            None => AluOp::Zero, // unreachable: EFFECTIVE_COMP holds documented codes
        };
        code += 1;
    }
    ops
}

/// The ALU operation executed for each comp code (after [`EFFECTIVE_COMP`]).
pub const ALU_OPS: [AluOp; 128] = build_ops();

/// A decoded instruction word.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Instruction {
    /// `0vvv vvvv vvvv vvvv`: load the 15-bit constant into A.
    A(u16),
    /// `111a cccc ccdd djjj`. `comp` may be undocumented; see [`effective_comp`].
    C { comp: u8, dest: u8, jump: u8 },
    /// [`EMPTY_WORD`]: an unloaded ROM cell, executed as a no-op.
    Empty,
    /// Any other word with the top bit set: the CPU emulator stops with an error.
    Illegal(u16),
}

/// Decodes one instruction word.
pub fn decode(word: u16) -> Instruction {
    if word & 0x8000 == 0 {
        Instruction::A(word)
    } else if word & 0xE000 == 0xE000 {
        Instruction::C {
            comp: ((word >> 6) & 0x7F) as u8,
            dest: ((word >> 3) & 7) as u8,
            jump: (word & 7) as u8,
        }
    } else if word == EMPTY_WORD {
        Instruction::Empty
    } else {
        Instruction::Illegal(word)
    }
}

/// Encodes a C-instruction.
pub fn encode_c(comp: u8, dest: u8, jump: u8) -> u16 {
    0xE000 | (u16::from(comp & 0x7F) << 6) | (u16::from(dest & 7) << 3) | u16::from(jump & 7)
}

/// Disassembles one word into canonical Hack assembly: `@2`, `D=A`, `0;JMP`,
/// `M=D+1;JGT`.
///
/// Words that no assembly instruction produces are rendered as `?` followed
/// by the 16 binary digits, e.g. `?1000110000010000`. That covers
/// [`EMPTY_WORD`] (unloaded ROM), illegal words (`100…`, `101…`, `110…`) and
/// C-instructions with an undocumented comp code. The `?` form is deliberately
/// not valid assembly, so a disassembly can never silently re-assemble to
/// different bits. Use [`decode`] and [`effective_comp`] for a structured view.
pub fn disassemble(word: u16) -> String {
    match decode(word) {
        Instruction::A(v) => format!("@{v}"),
        Instruction::C { comp, dest, jump } => match comp_mnemonic(comp) {
            Some(c) => {
                let mut s = String::with_capacity(12);
                if dest != 0 {
                    s.push_str(DESTS[dest as usize]);
                    s.push('=');
                }
                s.push_str(c);
                if jump != 0 {
                    s.push(';');
                    s.push_str(JUMPS[jump as usize]);
                }
                s
            }
            None => format!("?{word:016b}"),
        },
        Instruction::Empty | Instruction::Illegal(_) => format!("?{word:016b}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tables_are_consistent() {
        for &(m, c) in &COMPS {
            assert!(is_documented_comp(c), "{m}");
            assert_eq!(effective_comp(c), c);
            assert_eq!(comp_code(m), Some(c));
        }
        let documented_count = (0u8..128).filter(|&c| is_documented_comp(c)).count();
        assert_eq!(documented_count, 28);
        for c in 0u8..128 {
            assert!(effective_comp(c) <= c);
            assert!(is_documented_comp(effective_comp(c)));
        }
        // Examples observed in the Java emulator.
        assert_eq!(effective_comp(0b1001101), 0b1000111);
        assert_eq!(effective_comp(0b0000001), 0b0000000);
        assert_eq!(effective_comp(0b1111111), 0b1110111);
        assert_eq!(dest_code("AMD"), Some(7));
        assert_eq!(dest_code("DM"), None);
        assert_eq!(dest_code(""), None);
        assert_eq!(jump_code("JMP"), Some(7));
        assert_eq!(jump_code("jmp"), None);
    }

    #[test]
    fn disassembles_canonically() {
        assert_eq!(disassemble(2), "@2");
        assert_eq!(disassemble(0b1110110000010000), "D=A");
        assert_eq!(disassemble(0b1110101010000111), "0;JMP");
        assert_eq!(disassemble(0b1110011111001001), "M=D+1;JGT");
        assert_eq!(disassemble(0b1111000010001000), "M=D+M");
        assert_eq!(disassemble(0x8000), "?1000000000000000");
        assert_eq!(disassemble(0x8C10), "?1000110000010000");
        assert_eq!(disassemble(0xE000 | (0b1001101 << 6)), "?1111001101000000");
        for &(_, c) in &COMPS {
            for dest in 0..8 {
                for jump in 0..8 {
                    let w = encode_c(c, dest, jump);
                    let text = disassemble(w);
                    let parsed = crate::asm::assemble(&text);
                    assert_eq!(parsed.words, vec![w], "{text}");
                }
            }
        }
    }
}
