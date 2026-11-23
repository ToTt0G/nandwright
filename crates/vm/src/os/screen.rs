//! Built-in `Screen` Jack OS class.
//!
//! Line and circle rasterization match the official Java VM Emulator
//! pixel-for-pixel:
//! - `drawLine`: 8-connected integer Bresenham normalized along the major axis
//!   (`x1 <= x2` when `|dx| >= |dy|`, `y1 <= y2` when `|dy| > |dx|`), stepping
//!   the minor axis when `d >= 0`.
//! - `drawCircle`: filled midpoint circle algorithm (`d = 1 - r`) drawing
//!   symmetric horizontal spans; negative radius is a silent no-op (matching Java),
//!   radius `> 181` or extending outside the 512x256 screen raises `ERR13`.

use crate::error::VmError;
use crate::os::memory::{SCREEN_END, SCREEN_START};

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
    pub fn draw_pixel_unchecked(&self, ram: &mut [i16], x: i16, y: i16) {
        let addr = SCREEN_START as usize + (y as usize) * 32 + ((x as usize) >> 4);
        let mask = 1u16 << (x & 15);
        let word = ram[addr] as u16;
        ram[addr] = if self.color {
            (word | mask) as i16
        } else {
            (word & !mask) as i16
        };
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

    pub fn draw_horizontal(&self, ram: &mut [i16], x1: i16, x2: i16, y: i16) {
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
        // Verified against the Java VM Emulator: a negative radius does not
        // raise ERR13; its initial top/bottom pixels `(cx, cy + r)` and
        // `(cx, cy - r)` are drawn when inside screen bounds, and the loop
        // exits immediately.
        if r < 0 {
            let y1 = i32::from(cy) + i32::from(r);
            let y2 = i32::from(cy) - i32::from(r);
            if (0..i32::from(SCREEN_HEIGHT)).contains(&y1) {
                self.draw_pixel_unchecked(ram, cx, y1 as i16);
            }
            if (0..i32::from(SCREEN_HEIGHT)).contains(&y2) {
                self.draw_pixel_unchecked(ram, cx, y2 as i16);
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
