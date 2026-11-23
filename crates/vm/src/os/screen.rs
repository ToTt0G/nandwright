//! Built-in `Screen` Jack OS class.
//!
//! Line and circle rasterization match the official Java VM Emulator
//! pixel-for-pixel:
//! - `drawLine`: 8-connected integer Bresenham normalized along the major axis
//!   (`x1 <= x2` when `|dx| >= |dy|`, `y1 <= y2` when `|dy| > |dx|`), stepping
//!   the minor axis when `d >= 0`.
//! - `drawCircle`: filled midpoint circle algorithm (`d = 1 - r`) drawing
//!   symmetric horizontal spans. A negative radius still draws the initial
//!   endpoints and reversed center span. A positive radius `> 181` or extending
//!   outside the 512x256 screen raises `ERR13`.

use crate::error::VmError;
use crate::os::memory::{HEAP_START, KEYBOARD_ADDR, SCREEN_END, SCREEN_START};

pub const SCREEN_WIDTH: i16 = 512;
pub const SCREEN_HEIGHT: i16 = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScreenState {
    /// Current drawing color: `true` = black (`1`), `false` = white (`0`).
    pub color: bool,
}

impl Default for ScreenState {
    fn default() -> Self {
        Self { color: true }
    }
}

impl ScreenState {
    pub fn init(&mut self) {
        self.color = true;
    }

    pub fn set_color(&mut self, b: i16) {
        self.color = b != 0;
    }

    pub fn clear_screen(&self, ram: &mut [i16]) {
        ram[SCREEN_START as usize..=SCREEN_END as usize].fill(0);
    }

    #[inline]
    fn draw_pixel_unchecked(&self, ram: &mut [i16], x: i16, y: i16) {
        let addr = SCREEN_START as usize + (y as usize) * 32 + ((x as usize) >> 4);
        let mask = 1u16 << (x & 15);
        self.draw_word_mask(&mut ram[addr], mask);
    }

    #[inline]
    fn draw_word_mask(&self, word: &mut i16, mask: u16) {
        let current = *word as u16;
        *word = if self.color {
            (current | mask) as i16
        } else {
            (current & !mask) as i16
        };
    }

    fn draw_circle_word(&self, ram: &mut [i16], addr: i32, mask: u16) -> Result<(), VmError> {
        // The built-in passes the linear address as a Hack word to memory.
        let address = addr as i16;
        if !(HEAP_START as i16..=KEYBOARD_ADDR as i16).contains(&address) {
            return Err(VmError::MemoryOutOfRange {
                function: "Screen.drawCircle",
                address,
            });
        }
        self.draw_word_mask(&mut ram[address as usize], mask);
        Ok(())
    }

    pub fn draw_pixel(&self, ram: &mut [i16], x: i16, y: i16) -> Result<(), VmError> {
        if !(0..SCREEN_WIDTH).contains(&x) || !(0..SCREEN_HEIGHT).contains(&y) {
            return Err(VmError::BuiltinError {
                function: "Screen.drawPixel",
                message: "illegal pixel coordinates".to_string(),
            });
        }
        self.draw_pixel_unchecked(ram, x, y);
        Ok(())
    }

    pub fn draw_line(
        &self,
        ram: &mut [i16],
        x1: i16,
        y1: i16,
        x2: i16,
        y2: i16,
    ) -> Result<(), VmError> {
        if !(0..SCREEN_WIDTH).contains(&x1)
            || !(0..SCREEN_HEIGHT).contains(&y1)
            || !(0..SCREEN_WIDTH).contains(&x2)
            || !(0..SCREEN_HEIGHT).contains(&y2)
        {
            return Err(VmError::BuiltinError {
                function: "Screen.drawLine",
                message: "illegal line coordinates".to_string(),
            });
        }

        let dx = (i32::from(x2) - i32::from(x1)).abs();
        let dy = (i32::from(y2) - i32::from(y1)).abs();

        if dy == 0 {
            self.draw_horizontal(ram, x1.min(x2), x1.max(x2), y1);
            return Ok(());
        }
        if dx == 0 {
            let y_lo = y1.min(y2);
            let y_hi = y1.max(y2);
            for y in y_lo..=y_hi {
                self.draw_pixel_unchecked(ram, x1, y);
            }
            return Ok(());
        }

        if dx >= dy {
            let (sx, sy, ex, ey) = if x1 <= x2 {
                (x1, y1, x2, y2)
            } else {
                (x2, y2, x1, y1)
            };
            let step_y: i16 = if ey >= sy { 1 } else { -1 };
            let mut d = 2 * dy - dx;
            let inc_e = 2 * dy;
            let inc_ne = 2 * (dy - dx);
            let mut y = sy;
            for x in sx..=ex {
                self.draw_pixel_unchecked(ram, x, y);
                if d >= 0 {
                    y += step_y;
                    d += inc_ne;
                } else {
                    d += inc_e;
                }
            }
        } else {
            let (sx, sy, ex, ey) = if y1 <= y2 {
                (x1, y1, x2, y2)
            } else {
                (x2, y2, x1, y1)
            };
            let step_x: i16 = if ex >= sx { 1 } else { -1 };
            let mut d = 2 * dx - dy;
            let inc_e = 2 * dx;
            let inc_ne = 2 * (dx - dy);
            let mut x = sx;
            for y in sy..=ey {
                self.draw_pixel_unchecked(ram, x, y);
                if d >= 0 {
                    x += step_x;
                    d += inc_ne;
                } else {
                    d += inc_e;
                }
            }
        }
        Ok(())
    }

    fn draw_horizontal(&self, ram: &mut [i16], x1: i16, x2: i16, y: i16) {
        let mut x = x1;
        while x <= x2 && (x & 15) != 0 {
            self.draw_pixel_unchecked(ram, x, y);
            x += 1;
        }
        let fill: i16 = if self.color { -1 } else { 0 };
        let row_base = SCREEN_START as usize + (y as usize) * 32;
        while x + 15 <= x2 {
            ram[row_base + ((x as usize) >> 4)] = fill;
            x += 16;
        }
        while x <= x2 {
            self.draw_pixel_unchecked(ram, x, y);
            x += 1;
        }
    }

    pub fn draw_rectangle(
        &self,
        ram: &mut [i16],
        x1: i16,
        y1: i16,
        x2: i16,
        y2: i16,
    ) -> Result<(), VmError> {
        if !(0..SCREEN_WIDTH).contains(&x1)
            || !(0..SCREEN_HEIGHT).contains(&y1)
            || !(0..SCREEN_WIDTH).contains(&x2)
            || !(0..SCREEN_HEIGHT).contains(&y2)
            || x1 > x2
            || y1 > y2
        {
            return Err(VmError::BuiltinError {
                function: "Screen.drawRectangle",
                message: "illegal rectangle coordinates".to_string(),
            });
        }
        for y in y1..=y2 {
            self.draw_horizontal(ram, x1, x2, y);
        }
        Ok(())
    }

    pub fn draw_circle(&self, ram: &mut [i16], cx: i16, cy: i16, r: i16) -> Result<(), VmError> {
        if !(0..SCREEN_WIDTH).contains(&cx) || !(0..SCREEN_HEIGHT).contains(&cy) {
            return Err(VmError::BuiltinError {
                function: "Screen.drawCircle",
                message: "illegal center coordinates".to_string(),
            });
        }
        // A negative radius skips the circle loop, but Java draws its initial
        // endpoints and reversed horizontal span through linear RAM addresses.
        if r < 0 {
            let row = i32::from(SCREEN_START) + i32::from(cy) * 32;
            let x = i32::from(cx);
            let r = i32::from(r);
            let pixel = row + (x >> 4);
            let mask = 1 << (x & 15);
            self.draw_circle_word(ram, pixel + r * 32, mask)?;
            self.draw_circle_word(ram, pixel - r * 32, mask)?;
            let start = x - r;
            let end = x + r;
            let start_word = start >> 4;
            // In one word the two masks cancel. Across words Java writes the
            // right-hand mask into the word following the start, even when
            // the end coordinate lies to the left of the start.
            if start_word != end >> 4 {
                self.draw_circle_word(ram, row + start_word, u16::MAX << (start & 15))?;
                self.draw_circle_word(ram, row + start_word + 1, u16::MAX >> (15 - (end & 15)))?;
            }
            return Ok(());
        }
        if r > 181 {
            return Err(VmError::BuiltinError {
                function: "Screen.drawCircle",
                message: "illegal radius".to_string(),
            });
        }
        let r32 = i32::from(r);
        let cx32 = i32::from(cx);
        let cy32 = i32::from(cy);
        if cx32 - r32 < 0
            || cx32 + r32 >= i32::from(SCREEN_WIDTH)
            || cy32 - r32 < 0
            || cy32 + r32 >= i32::from(SCREEN_HEIGHT)
        {
            return Err(VmError::BuiltinError {
                function: "Screen.drawCircle",
                message: "illegal radius".to_string(),
            });
        }

        // Filled midpoint circle algorithm (matches Java VM Emulator pixel-for-pixel).
        let mut x: i32 = 0;
        let mut y: i32 = r32;
        let mut d: i32 = 1 - r32;
        while x <= y {
            self.draw_horizontal(ram, (cx32 - x) as i16, (cx32 + x) as i16, (cy32 + y) as i16);
            self.draw_horizontal(ram, (cx32 - x) as i16, (cx32 + x) as i16, (cy32 - y) as i16);
            self.draw_horizontal(ram, (cx32 - y) as i16, (cx32 + y) as i16, (cy32 + x) as i16);
            self.draw_horizontal(ram, (cx32 - y) as i16, (cx32 + y) as i16, (cy32 - x) as i16);
            if d < 0 {
                d += 2 * x + 3;
            } else {
                d += 2 * (x - y) + 5;
                y -= 1;
            }
            x += 1;
        }
        Ok(())
    }
}
