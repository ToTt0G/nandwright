//! Built-in `Output` Jack OS class.

use crate::error::VmError;
use crate::os::font;
use crate::os::memory::SCREEN_START;
use crate::os::string;

pub const MAX_ROWS: u8 = 23;
pub const MAX_COLS: u8 = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct OutputState {
    pub row: u8,
    pub col: u8,
}

impl OutputState {
    pub fn init(&mut self) {
        self.row = 0;
        self.col = 0;
    }

    pub fn move_cursor(&mut self, row: i16, col: i16) -> Result<(), VmError> {
        if !(0..i16::from(MAX_ROWS)).contains(&row) || !(0..i16::from(MAX_COLS)).contains(&col) {
            return Err(VmError::BuiltinError {
                function: "Output.moveCursor",
                message: "illegal cursor location".to_string(),
            });
        }
        self.row = row as u8;
        self.col = col as u8;
        Ok(())
    }

    /// Draws an 8x11 glyph directly at `(self.row, self.col)` without advancing the cursor.
    /// Overwrites the 8-bit lane (clearing background bits to white).
    pub fn draw_glyph_at_cursor(&self, ram: &mut [i16], glyph: &[u8; 11]) {
        let base_y = usize::from(self.row) * 11;
        let word_col = usize::from(self.col) >> 1;
        let is_high = (self.col & 1) != 0;
        for (dy, &row_bits) in glyph.iter().enumerate() {
            let addr = SCREEN_START as usize + (base_y + dy) * 32 + word_col;
            let current = ram[addr] as u16;
            let updated = if is_high {
                (current & 0x00FF) | (u16::from(row_bits) << 8)
            } else {
                (current & 0xFF00) | u16::from(row_bits)
            };
            ram[addr] = updated as i16;
        }
    }

    pub fn draw_char_at_cursor(&self, ram: &mut [i16], c: i16) {
        self.draw_glyph_at_cursor(ram, font::glyph(c));
    }

    /// Draws the solid block cursor (used by `Keyboard.readChar`/`readLine`) at `(row, col)`.
    pub fn draw_cursor_box(&self, ram: &mut [i16]) {
        self.draw_glyph_at_cursor(ram, &font::FALLBACK_GLYPH);
    }

    pub fn advance_cursor(&mut self) {
        self.col += 1;
        if self.col >= MAX_COLS {
            self.col = 0;
            self.row += 1;
            if self.row >= MAX_ROWS {
                self.row = 0;
            }
        }
    }

    pub fn print_char(&mut self, ram: &mut [i16], c: i16) {
        if c == 128 {
            self.println();
            return;
        }
        if c == 129 {
            self.back_space(ram);
            return;
        }
        self.draw_char_at_cursor(ram, c);
        self.advance_cursor();
    }

    pub fn print_str_ascii(&mut self, ram: &mut [i16], s: &str) {
        for b in s.bytes() {
            self.print_char(ram, i16::from(b));
        }
    }

    pub fn print_string(&mut self, ram: &mut [i16], str_ptr: i16) -> Result<(), VmError> {
        let len = string::length(ram, str_ptr)?;
        for i in 0..len {
            let ch = string::char_at(ram, str_ptr, i)?;
            self.print_char(ram, ch);
        }
        Ok(())
    }

    pub fn print_int(&mut self, ram: &mut [i16], val: i16) {
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
        for &b in &buf[..len] {
            self.print_char(ram, i16::from(b));
        }
    }

    pub fn println(&mut self) {
        self.col = 0;
        self.row += 1;
        if self.row >= MAX_ROWS {
            self.row = 0;
        }
    }

    pub fn back_space(&mut self, ram: &mut [i16]) {
        if self.col == 0 {
            if self.row > 0 {
                self.row -= 1;
                self.col = MAX_COLS - 1;
            }
        } else {
            self.col -= 1;
        }
        self.draw_char_at_cursor(ram, 32);
    }
}
