//! Built-in `Math` Jack OS class.

use crate::error::VmError;

#[inline]
pub fn abs(x: i16) -> i16 {
    if x < 0 { x.wrapping_neg() } else { x }
}

#[inline]
pub fn multiply(x: i16, y: i16) -> i16 {
    x.wrapping_mul(y)
}

#[inline]
pub fn divide(x: i16, y: i16) -> Result<i16, VmError> {
    if y == 0 {
        return Err(VmError::BuiltinError {
            function: "Math.divide",
            message: "division by zero".to_string(),
        });
    }
    Ok(x.wrapping_div(y))
}

#[inline]
pub fn min(x: i16, y: i16) -> i16 {
    x.min(y)
}

#[inline]
pub fn max(x: i16, y: i16) -> i16 {
    x.max(y)
}

#[inline]
pub fn sqrt(x: i16) -> Result<i16, VmError> {
    if x < 0 {
        return Err(VmError::BuiltinError {
            function: "Math.sqrt",
            message: "cannot compute square root of a negative number".to_string(),
        });
    }
    let n = x as u32;
    let mut y: u32 = 0;
    for bit in (0..8).rev() {
        let candidate = y | (1u32 << bit);
        if candidate * candidate <= n {
            y = candidate;
        }
    }
    Ok(y as i16)
}
