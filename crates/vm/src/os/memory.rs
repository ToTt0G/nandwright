//! Built-in `Memory` (and `Array`) Jack OS classes.
//!
//! Heap layout (matches the official Java VM Emulator):
//! - Heap range: `RAM[2048..=16383]` (`14336` words).
//! - Each block has a 2-word header at `block`:
//!   - `RAM[block + 0]` = free payload size (`0` when allocated, `> 0` when free)
//!   - `RAM[block + 1]` = address of next block (`16384` for the last block)
//!   - payload starts at `block + 2` and extends up to `RAM[block + 1]`.
//! - Initial state after `Memory.init`:
//!   - `RAM[2048] = 14334` (`16384 - 2048 - 2`)
//!   - `RAM[2049] = 16384`

use crate::error::VmError;

pub const HEAP_START: u16 = 2048;
pub const HEAP_END: u16 = 16383;
pub const HEAP_LIMIT: u16 = 16384;
pub const SCREEN_START: u16 = 16384;
pub const SCREEN_END: u16 = 24575;
pub const KEYBOARD_ADDR: u16 = 24576;
pub const RAM_LEN: usize = 24577;

/// Initializes the heap free list at `RAM[2048..16384]`.
pub fn init(ram: &mut [i16]) {
    ram[HEAP_START as usize] = (HEAP_LIMIT - HEAP_START - 2) as i16; // 14334
    ram[(HEAP_START + 1) as usize] = HEAP_LIMIT as i16; // 16384
}

/// Built-in `Memory.peek(address)`.
///
/// The official Java VM Emulator allows `0` or the Heap/Screen/Keyboard range
/// (`2048..=24576`), rejecting `1..=2047`, `< 0`, and `> 24576`.
pub fn peek(ram: &[i16], address: i16) -> Result<i16, VmError> {
    if !is_heap_or_screen_or_zero(address) {
        return Err(VmError::MemoryOutOfRange {
            function: "Memory.peek",
            address,
        });
    }
    Ok(ram[address as usize])
}

/// Built-in `Memory.poke(address, value)`. Returns `true` if the screen was touched.
pub fn poke(ram: &mut [i16], address: i16, value: i16) -> Result<bool, VmError> {
    if !is_heap_or_screen_or_zero(address) {
        return Err(VmError::MemoryOutOfRange {
            function: "Memory.poke",
            address,
        });
    }
    let addr = address as usize;
    ram[addr] = value;
    Ok((SCREEN_START as usize..=SCREEN_END as usize).contains(&addr))
}

#[inline]
fn is_heap_or_screen_or_zero(address: i16) -> bool {
    address == 0 || (HEAP_START as i16..=KEYBOARD_ADDR as i16).contains(&address)
}

/// Built-in `Memory.alloc(size)`.
pub fn alloc(ram: &mut [i16], size: i16) -> Result<i16, VmError> {
    if size <= 0 {
        return Err(VmError::BuiltinError {
            function: "Memory.alloc",
            message: "allocated memory size must be positive".to_string(),
        });
    }
    alloc_i32(ram, i32::from(size), "Memory.alloc")
}

/// Built-in `Array.new(size)`.
pub fn array_new(ram: &mut [i16], size: i16) -> Result<i16, VmError> {
    if size <= 0 {
        return Err(VmError::BuiltinError {
            function: "Array.new",
            message: "array size must be positive".to_string(),
        });
    }
    alloc_i32(ram, i32::from(size), "Array.new")
}

/// Internal allocator used by `Memory.alloc`, `Array.new`, and `String.new`.
pub(crate) fn alloc_i32(
    ram: &mut [i16],
    size: i32,
    function: &'static str,
) -> Result<i16, VmError> {
    if size <= 0 {
        return Err(VmError::BuiltinError {
            function,
            message: "allocated memory size must be positive".to_string(),
        });
    }

    // First-fit scan through the contiguous block chain starting at HEAP_START.
    // Verified against Java VM Emulator: `alloc` does NOT coalesce adjacent free
    // blocks during scanning or on OOM; only `deAlloc` coalesces forward once.
    // Guard against corrupted heap chains with an iteration cap.
    let mut block: i32 = i32::from(HEAP_START);
    let mut steps: u32 = 0;
    while block >= i32::from(HEAP_START) && block < i32::from(HEAP_LIMIT) && steps < 16384 {
        steps += 1;
        let avail = i32::from(ram[block as usize]);
        let next = i32::from(ram[(block + 1) as usize]);

        if avail >= size {
            let payload = block + 2;
            let remaining = avail - size;
            // Split if the leftover can hold at least a 2-word header + 1 word payload (remaining >= 3).
            if remaining >= 3 {
                let new_block = payload + size;
                ram[new_block as usize] = (remaining - 2) as i16;
                ram[(new_block + 1) as usize] = next as i16;
                ram[(block + 1) as usize] = new_block as i16;
            }
            // Mark block allocated (size = 0).
            ram[block as usize] = 0;
            return Ok(payload as i16);
        }

        if next <= block {
            break;
        }
        block = next;
    }

    Err(VmError::BuiltinError {
        function,
        message: "heap overflow".to_string(),
    })
}

/// Built-in `Memory.deAlloc(object)` (and `Array.dispose`).
pub fn de_alloc(ram: &mut [i16], object: i16) -> Result<(), VmError> {
    let header = i32::from(object) - 2;
    if header < i32::from(HEAP_START) || header > i32::from(HEAP_END) - 1 {
        return Err(VmError::MemoryOutOfRange {
            function: "Memory.deAlloc",
            address: object,
        });
    }
    let next = i32::from(ram[(header + 1) as usize]);
    let mut size = next - i32::from(object);
    if size < 0 {
        size = 0;
    }
    // Coalesce with immediately following block if it is free.
    if next >= i32::from(HEAP_START) && next < i32::from(HEAP_LIMIT) {
        let next_size = i32::from(ram[next as usize]);
        if next_size > 0 {
            let next_next = i32::from(ram[(next + 1) as usize]);
            size = size + 2 + next_size;
            ram[(header + 1) as usize] = next_next as i16;
        }
    }
    ram[header as usize] = size as i16;
    Ok(())
}
