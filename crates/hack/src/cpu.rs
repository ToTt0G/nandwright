//! The Hack CPU emulator.
//!
//! # Compatibility with the official CPU emulator
//!
//! [`Cpu::step`] matches one `ticktock` of the official Java CPU emulator
//! (`tools/CPUEmulator.sh`), including its departures from the hardware of
//! chapter 5. Established by experiment:
//!
//! - **Undocumented comp codes** compute what the nearest lower documented
//!   code computes ([`crate::isa::EFFECTIVE_COMP`]), not what the chip-level ALU would.
//! - **`dest` with both A and M** (`AM=M+1`): M is written at the *old* A.
//! - **Jumps go to the *new* A**: `A=D;JMP` jumps to D, not to the previous A
//!   (the hardware CPU would jump to the previous A).
//! - **Memory checks**: when the computation reads M (a-bit set) or `dest`
//!   includes M, A must be a RAM address, 0..=24576; otherwise the emulator
//!   stops with an error. Instructions that do not touch M never check A.
//!   The keyboard register (24576) can be written by the program.
//! - **Jump target check**: a taken jump to an A that is negative as a signed
//!   16-bit value (≥ 32768) is an error. Jumps into empty ROM are fine.
//! - **End of ROM**: falling through from address 32767 is an error; a jump
//!   at 32767 is fine. There is no wrap-around.
//! - **Empty ROM** holds [`EMPTY_WORD`] (`1000000000000000`), which
//!   executes as a no-op, so a program that runs off its end keeps going
//!   until address 32767. Any other `100…`/`101…`/`110…` word is an illegal
//!   instruction error when executed (not when loaded).
//! - **Loading a program** resets A, D and PC to 0 and clears the screen;
//!   other RAM, including the keyboard register, is kept.
//!
//! When `step` returns an error, nothing has changed: the instruction is not
//! partially executed. (The Java emulator's state after such an error cannot
//! be observed from a test script, which stops.)
//!
//! Registers are plain 16-bit words here. A test script shows A, D and RAM
//! as signed decimals (`A=-1` for 0xFFFF); formatting is the caller's job.

use std::fmt;

use crate::isa::{ALU_OPS, EMPTY_WORD, KBD, SCREEN, SCREEN_WORDS};

/// Words of ROM.
pub const ROM_SIZE: usize = 32768;
/// Words of addressable RAM: 0..=24576 (the keyboard register is the last).
pub const RAM_SIZE: usize = KBD as usize + 1;

/// Why the CPU stopped. Each variant records the PC of the offending
/// instruction; the CPU state is unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CpuError {
    /// A word of the form `100…`, `101…` or `110…` (other than the empty word).
    IllegalInstruction { pc: u16, word: u16 },
    /// The computation reads M but A is not a RAM address.
    IllegalRead { pc: u16, address: u16 },
    /// The destination includes M but A is not a RAM address.
    IllegalWrite { pc: u16, address: u16 },
    /// A taken jump to an address ≥ 32768 (negative A).
    IllegalJump { pc: u16, target: u16 },
    /// The instruction at 32767 does not jump, so there is no next instruction.
    PcOverflow { pc: u16 },
}

impl CpuError {
    /// The address of the instruction that failed.
    pub fn pc(&self) -> u16 {
        match *self {
            CpuError::IllegalInstruction { pc, .. }
            | CpuError::IllegalRead { pc, .. }
            | CpuError::IllegalWrite { pc, .. }
            | CpuError::IllegalJump { pc, .. }
            | CpuError::PcOverflow { pc } => pc,
        }
    }
}

fn signed(v: u16) -> String {
    if v >= 0x8000 {
        format!("{} ({v})", v as i16)
    } else {
        v.to_string()
    }
}

impl fmt::Display for CpuError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            CpuError::IllegalInstruction { pc, word } => write!(
                f,
                "ROM[{pc}] holds {word:016b}, which is not an instruction (a C-instruction must \
                 start with 111)"
            ),
            CpuError::IllegalRead { pc, address } => write!(
                f,
                "ROM[{pc}] reads M, but A = {} is not a RAM address (0..24576)",
                signed(address)
            ),
            CpuError::IllegalWrite { pc, address } => write!(
                f,
                "ROM[{pc}] writes M, but A = {} is not a RAM address (0..24576)",
                signed(address)
            ),
            CpuError::IllegalJump { pc, target } => write!(
                f,
                "ROM[{pc}] jumps to A = {}, which is not a ROM address (0..32767)",
                signed(target)
            ),
            CpuError::PcOverflow { pc } => write!(
                f,
                "ROM[{pc}] is the last ROM address and does not jump, so there is no next \
                 instruction"
            ),
        }
    }
}

impl std::error::Error for CpuError {}

/// The outcome of [`Cpu::run`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RunStatus {
    /// The budget was used up; call `run` again to continue.
    Budget,
    /// Execution stopped because the PC reached this breakpoint.
    Breakpoint(u16),
    /// An instruction failed; the CPU is left before it.
    Error(CpuError),
}

/// A Hack computer: CPU registers, 32K ROM and the RAM with its memory-mapped
/// screen and keyboard.
#[derive(Clone)]
pub struct Cpu {
    a: u16,
    d: u16,
    pc: u16,
    /// 32K so that any 15-bit index is in bounds; only 0..=24576 is reachable.
    ram: Box<[u16; ROM_SIZE]>,
    rom: Box<[u16; ROM_SIZE]>,
    rom_len: usize,
    /// Scratch bitset for `run`, one bit per ROM address; always all zero
    /// between calls.
    breakpoints: Box<[u64; ROM_SIZE / 64]>,
    screen_generation: u64,
    screen_seen: u64,
    cycles: u64,
}

impl fmt::Debug for Cpu {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Cpu")
            .field("a", &self.a)
            .field("d", &self.d)
            .field("pc", &self.pc)
            .field("rom_len", &self.rom_len)
            .field("cycles", &self.cycles)
            .finish_non_exhaustive()
    }
}

impl Default for Cpu {
    fn default() -> Self {
        Self::new()
    }
}

fn boxed_array<T: Copy, const N: usize>(value: T) -> Box<[T; N]> {
    match vec![value; N].into_boxed_slice().try_into() {
        Ok(b) => b,
        Err(_) => unreachable!("vec![_; N] has length N"),
    }
}

#[inline(always)]
fn in_screen(address: u16) -> bool {
    (address.wrapping_sub(SCREEN) as usize) < SCREEN_WORDS
}

impl Cpu {
    /// A computer with zeroed registers and RAM and an empty ROM.
    pub fn new() -> Self {
        Cpu {
            a: 0,
            d: 0,
            pc: 0,
            ram: boxed_array(0),
            rom: boxed_array(EMPTY_WORD),
            rom_len: 0,
            breakpoints: boxed_array(0),
            screen_generation: 0,
            screen_seen: 0,
            cycles: 0,
        }
    }

    /// Loads a program the way the official emulator's `load` does: ROM gets
    /// `words` followed by empty words, A, D and PC become 0 and the screen is
    /// cleared; the rest of RAM is kept. Words beyond [`ROM_SIZE`] are ignored
    /// ([`crate::asm::parse_hack`] rejects such programs).
    pub fn load_rom(&mut self, words: &[u16]) {
        let n = words.len().min(ROM_SIZE);
        self.rom[..n].copy_from_slice(&words[..n]);
        self.rom[n..].fill(EMPTY_WORD);
        self.rom_len = n;
        self.a = 0;
        self.d = 0;
        self.pc = 0;
        let screen = SCREEN as usize;
        self.ram[screen..screen + SCREEN_WORDS].fill(0);
        self.screen_generation += 1;
    }

    /// Sets the PC to 0, like the emulator's reset button. Registers, RAM and
    /// ROM are kept.
    pub fn reset(&mut self) {
        self.pc = 0;
    }

    /// Executes one instruction (one `ticktock`).
    pub fn step(&mut self) -> Result<(), CpuError> {
        self.execute()?;
        self.cycles += 1;
        Ok(())
    }

    /// Executes at most `budget` instructions. Stops early at an error, or
    /// when the PC *reaches* one of `breakpoints` after an instruction (so a
    /// run that starts on a breakpoint executes it and moves on). An endless
    /// loop simply uses up the budget.
    pub fn run(&mut self, budget: u64, breakpoints: &[u16]) -> RunStatus {
        if breakpoints.is_empty() {
            for done in 0..budget {
                if let Err(e) = self.execute() {
                    self.cycles += done;
                    return RunStatus::Error(e);
                }
            }
            self.cycles += budget;
            return RunStatus::Budget;
        }
        for &b in breakpoints {
            let b = (b & 0x7FFF) as usize;
            self.breakpoints[b / 64] |= 1 << (b % 64);
        }
        let mut status = RunStatus::Budget;
        let mut done = 0;
        while done < budget {
            if let Err(e) = self.execute() {
                status = RunStatus::Error(e);
                break;
            }
            done += 1;
            let pc = (self.pc & 0x7FFF) as usize;
            if self.breakpoints[pc / 64] >> (pc % 64) & 1 != 0 {
                status = RunStatus::Breakpoint(self.pc);
                break;
            }
        }
        self.cycles += done;
        for &b in breakpoints {
            let b = (b & 0x7FFF) as usize;
            self.breakpoints[b / 64] = 0;
        }
        status
    }

    #[inline(always)]
    fn execute(&mut self) -> Result<(), CpuError> {
        let pc = self.pc;
        let ins = self.rom[(pc & 0x7FFF) as usize];
        if ins & 0x8000 == 0 {
            if pc == 0x7FFF {
                return Err(CpuError::PcOverflow { pc });
            }
            self.a = ins;
            self.pc = pc + 1;
            return Ok(());
        }
        if ins & 0xE000 != 0xE000 {
            if ins != EMPTY_WORD {
                return Err(CpuError::IllegalInstruction { pc, word: ins });
            }
            if pc == 0x7FFF {
                return Err(CpuError::PcOverflow { pc });
            }
            self.pc = pc + 1;
            return Ok(());
        }
        let a = self.a;
        let y = if ins & 0x1000 != 0 {
            if a > KBD {
                return Err(CpuError::IllegalRead { pc, address: a });
            }
            self.ram[(a & 0x7FFF) as usize]
        } else {
            a
        };
        let out = ALU_OPS[((ins >> 6) & 0x7F) as usize].apply(self.d, y);
        let write_m = ins & 0x0008 != 0;
        if write_m && a > KBD {
            return Err(CpuError::IllegalWrite { pc, address: a });
        }
        let new_a = if ins & 0x0020 != 0 { out } else { a };
        let jump = ins & 7;
        let class = if out == 0 {
            2
        } else if out & 0x8000 != 0 {
            4
        } else {
            1
        };
        let next = if jump & class != 0 {
            if new_a & 0x8000 != 0 {
                return Err(CpuError::IllegalJump { pc, target: new_a });
            }
            new_a
        } else {
            if pc == 0x7FFF {
                return Err(CpuError::PcOverflow { pc });
            }
            pc + 1
        };
        if write_m {
            self.ram[(a & 0x7FFF) as usize] = out;
            if in_screen(a) {
                self.screen_generation += 1;
            }
        }
        self.a = new_a;
        if ins & 0x0010 != 0 {
            self.d = out;
        }
        self.pc = next;
        Ok(())
    }

    pub fn a(&self) -> u16 {
        self.a
    }
    pub fn d(&self) -> u16 {
        self.d
    }
    pub fn pc(&self) -> u16 {
        self.pc
    }
    pub fn set_a(&mut self, value: u16) {
        self.a = value;
    }
    pub fn set_d(&mut self, value: u16) {
        self.d = value;
    }
    /// Sets the PC. Only the low 15 bits are kept (the ROM has 32768 words);
    /// a test-script runner should reject out-of-range values itself, as the
    /// official one does.
    pub fn set_pc(&mut self, value: u16) {
        self.pc = value & 0x7FFF;
    }

    /// The RAM word at `address`, or 0 if `address` > 24576.
    pub fn ram(&self, address: u16) -> u16 {
        if address <= KBD {
            self.ram[(address & 0x7FFF) as usize]
        } else {
            0
        }
    }

    /// Writes a RAM word. Returns `false` (and writes nothing) if
    /// `address` > 24576.
    pub fn set_ram(&mut self, address: u16, value: u16) -> bool {
        if address > KBD {
            return false;
        }
        self.ram[(address & 0x7FFF) as usize] = value;
        if in_screen(address) {
            self.screen_generation += 1;
        }
        true
    }

    /// All of RAM, addresses 0..=24576.
    pub fn ram_slice(&self) -> &[u16] {
        &self.ram[..RAM_SIZE]
    }

    /// All 32768 ROM words; unloaded cells hold [`EMPTY_WORD`].
    pub fn rom_slice(&self) -> &[u16] {
        &self.rom[..]
    }

    /// The number of words loaded by [`Cpu::load_rom`] (or extended by
    /// [`Cpu::set_rom`]).
    pub fn rom_len(&self) -> usize {
        self.rom_len
    }

    /// The ROM word at `address` (only the low 15 bits are used).
    pub fn rom(&self, address: u16) -> u16 {
        self.rom[(address & 0x7FFF) as usize]
    }

    /// Writes one ROM word (only the low 15 bits of `address` are used).
    pub fn set_rom(&mut self, address: u16, word: u16) {
        let i = (address & 0x7FFF) as usize;
        self.rom[i] = word;
        self.rom_len = self.rom_len.max(i + 1);
    }

    /// Sets the keyboard register (RAM[24576]); 0 means no key is pressed.
    pub fn set_key(&mut self, code: u16) {
        self.ram[KBD as usize] = code;
    }

    /// The keyboard register.
    pub fn key(&self) -> u16 {
        self.ram[KBD as usize]
    }

    /// The 8192 screen words, RAM[16384..24576]. Row `r` (0..256) occupies
    /// words `32*r .. 32*r+32`; bit `b` of a word is pixel column `16*w + b`.
    pub fn screen(&self) -> &[u16] {
        let s = SCREEN as usize;
        &self.ram[s..s + SCREEN_WORDS]
    }

    /// A counter that changes whenever a screen word may have changed. Cheap
    /// to poll from any number of observers.
    pub fn screen_generation(&self) -> u64 {
        self.screen_generation
    }

    /// Whether the screen may have changed since the last call.
    pub fn take_screen_dirty(&mut self) -> bool {
        let dirty = self.screen_generation != self.screen_seen;
        self.screen_seen = self.screen_generation;
        dirty
    }

    /// Instructions executed since this `Cpu` was created.
    pub fn cycles(&self) -> u64 {
        self.cycles
    }
}

/// Convenience used by tests and tools: the effective operation for a comp code.
pub fn alu(comp: u8, d: u16, a_or_m: u16) -> u16 {
    ALU_OPS[(comp & 0x7F) as usize].apply(d, a_or_m)
}

#[cfg(test)]
mod tests;
