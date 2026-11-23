//! Built-in `String` Jack OS class.
//!
//! Heap object layout (matches the official Java VM Emulator):
//! - `RAM[this + 0]` = `max_length`
//! - `RAM[this + 1]` = `current_length`
//! - `RAM[this + 2 .. this + 2 + max_length]` = character buffer
//!
//! Allocated via `Memory.alloc(max_length + 2)`.

use crate::error::VmError;
use crate::os::memory;

fn read_word(ram: &[i16], addr: i32, function: &'static str) -> Result<i16, VmError> {
    if !(memory::HEAP_START as i32..=memory::HEAP_END as i32).contains(&addr) {
        return Err(VmError::MemoryOutOfRange {
            function,
            address: addr as i16,
        });
    }
    Ok(ram[addr as usize])
}

fn write_word(ram: &mut [i16], addr: i32, val: i16, function: &'static str) -> Result<(), VmError> {
    if !(memory::HEAP_START as i32..=memory::HEAP_END as i32).contains(&addr) {
        return Err(VmError::MemoryOutOfRange {
            function,
            address: addr as i16,
        });
    }
    ram[addr as usize] = val;
    Ok(())
}

pub fn new(ram: &mut [i16], max_length: i16) -> Result<i16, VmError> {
    if max_length < 0 {
        return Err(VmError::BuiltinError {
            function: "String.new",
            message: "maximum length must be non-negative".to_string(),
        });
    }
    let alloc_size = i32::from(max_length) + 2;
    let ptr = memory::alloc_i32(ram, alloc_size, "String.new")?;
    let base = i32::from(ptr);
    write_word(ram, base, max_length, "String.new")?;
    write_word(ram, base + 1, 0, "String.new")?;
    Ok(ptr)
}

pub fn dispose(ram: &mut [i16], this: i16) -> Result<i16, VmError> {
    memory::de_alloc(ram, this)?;
    Ok(0)
}

pub fn length(ram: &[i16], this: i16) -> Result<i16, VmError> {
    read_word(ram, i32::from(this) + 1, "String.length")
}

pub fn char_at(ram: &[i16], this: i16, index: i16) -> Result<i16, VmError> {
    let base = i32::from(this);
    let len = read_word(ram, base + 1, "String.charAt")?;
    if index < 0 || index >= len {
        return Err(VmError::BuiltinError {
            function: "String.charAt",
            message: "string index out of bounds".to_string(),
        });
    }
    read_word(ram, base + 2 + i32::from(index), "String.charAt")
}

pub fn set_char_at(ram: &mut [i16], this: i16, index: i16, ch: i16) -> Result<i16, VmError> {
    let base = i32::from(this);
    let len = read_word(ram, base + 1, "String.setCharAt")?;
    if index < 0 || index >= len {
        return Err(VmError::BuiltinError {
            function: "String.setCharAt",
            message: "string index out of bounds".to_string(),
        });
    }
    write_word(ram, base + 2 + i32::from(index), ch, "String.setCharAt")?;
    Ok(0)
}

pub fn append_char(ram: &mut [i16], this: i16, ch: i16) -> Result<i16, VmError> {
    let base = i32::from(this);
    let max_len = read_word(ram, base, "String.appendChar")?;
    let len = read_word(ram, base + 1, "String.appendChar")?;
    if len >= max_len {
        return Err(VmError::BuiltinError {
            function: "String.appendChar",
            message: "string is full".to_string(),
        });
    }
    write_word(ram, base + 2 + i32::from(len), ch, "String.appendChar")?;
    write_word(ram, base + 1, len + 1, "String.appendChar")?;
    Ok(this)
}

pub fn erase_last_char(ram: &mut [i16], this: i16) -> Result<i16, VmError> {
    let base = i32::from(this);
    let len = read_word(ram, base + 1, "String.eraseLastChar")?;
    if len <= 0 {
        return Err(VmError::BuiltinError {
            function: "String.eraseLastChar",
            message: "string is empty".to_string(),
        });
    }
    write_word(ram, base + 1, len - 1, "String.eraseLastChar")?;
    Ok(0)
}

pub fn int_value(ram: &[i16], this: i16) -> Result<i16, VmError> {
    let base = i32::from(this);
    let len = read_word(ram, base + 1, "String.intValue")?;
    if len <= 0 {
        return Ok(0);
    }
    let first = read_word(ram, base + 2, "String.intValue")?;
    let neg = first == i16::from(b'-');
    let mut i = if neg { 1i32 } else { 0i32 };
    let mut val: i16 = 0;
    while i < i32::from(len) {
        let ch = read_word(ram, base + 2 + i, "String.intValue")?;
        if !(i16::from(b'0')..=i16::from(b'9')).contains(&ch) {
            break;
        }
        let digit = ch - i16::from(b'0');
        val = val.wrapping_mul(10).wrapping_add(digit);
        i += 1;
    }
    Ok(if neg { val.wrapping_neg() } else { val })
}

pub fn set_int(ram: &mut [i16], this: i16, val: i16) -> Result<i16, VmError> {
    let base = i32::from(this);
    let max_len = read_word(ram, base, "String.setInt")?;

    let mut buf = [0u8; 6];
    let mut len = 0usize;
    let mut v = i32::from(val);
    if v < 0 {
        buf[0] = b'-';
        len = 1;
        v = -v;
    }
    let mut digits = [0u8; 5];
    let mut nd = 0usize;
    if v == 0 {
        digits[0] = b'0';
        nd = 1;
    } else {
        while v > 0 {
            digits[nd] = b'0' + (v % 10) as u8;
            nd += 1;
            v /= 10;
        }
    }
    for k in (0..nd).rev() {
        buf[len] = digits[k];
        len += 1;
    }

    if (len as i16) > max_len {
        return Err(VmError::BuiltinError {
            function: "String.setInt",
            message: "insufficient string capacity".to_string(),
        });
    }
    write_word(ram, base + 1, len as i16, "String.setInt")?;
    for (i, &b) in buf[..len].iter().enumerate() {
        write_word(ram, base + 2 + i as i32, i16::from(b), "String.setInt")?;
    }
    Ok(0)
}

#[inline]
pub const fn back_space() -> i16 {
    129
}

#[inline]
pub const fn double_quote() -> i16 {
    34
}

#[inline]
pub const fn new_line() -> i16 {
    128
}
