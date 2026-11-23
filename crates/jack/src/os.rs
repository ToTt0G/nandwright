//! The Jack OS API: the eight standard classes and their subroutines, for
//! completion, signature help and call checks. The signatures follow the Jack
//! OS specification; the descriptions are this project's own.

use crate::ast::SubroutineKind;

/// One subroutine of the Jack OS.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OsSubroutine {
    pub class: &'static str,
    pub name: &'static str,
    pub kind: SubroutineKind,
    pub return_type: &'static str,
    /// `(type, name)` pairs, not counting the implicit `this` of methods.
    pub params: &'static [(&'static str, &'static str)],
    pub description: &'static str,
    /// Initialisation routines that `Sys.init` calls; programs should not call them.
    pub internal: bool,
}

impl OsSubroutine {
    /// `function int Math.max(int a, int b)`.
    pub fn signature(&self) -> String {
        let params = self
            .params
            .iter()
            .map(|(t, n)| format!("{t} {n}"))
            .collect::<Vec<_>>()
            .join(", ");
        format!(
            "{} {} {}.{}({})",
            self.kind.as_str(),
            self.return_type,
            self.class,
            self.name,
            params
        )
    }
}

/// The OS classes, in the order the book presents them.
pub const OS_CLASSES: [&str; 8] = [
    "Math", "String", "Array", "Output", "Screen", "Keyboard", "Memory", "Sys",
];

/// A one-line description of each OS class.
pub fn class_description(class: &str) -> Option<&'static str> {
    Some(match class {
        "Math" => "Integer arithmetic that the hardware does not provide.",
        "String" => "Mutable character strings with a fixed capacity.",
        "Array" => "Blocks of memory indexed with `a[i]`.",
        "Output" => "Text output on the screen with a character cursor.",
        "Screen" => "Drawing on the 512 x 256 black-and-white screen.",
        "Keyboard" => "Reading keys, characters, lines and numbers from the keyboard.",
        "Memory" => "Direct RAM access and heap allocation.",
        "Sys" => "Program start-up, halting, errors and timing.",
        _ => return None,
    })
}

use SubroutineKind::{Constructor as C, Function as F, Method as M};

macro_rules! os {
    ($class:literal, $kind:expr, $ret:literal, $name:literal, [$(($t:literal, $n:literal)),*], $desc:literal) => {
        OsSubroutine {
            class: $class,
            name: $name,
            kind: $kind,
            return_type: $ret,
            params: &[$(($t, $n)),*],
            description: $desc,
            internal: false,
        }
    };
    ($class:literal, init) => {
        OsSubroutine {
            class: $class,
            name: "init",
            kind: F,
            return_type: "void",
            params: &[],
            description: "Sets up this class's internal state. `Sys.init` calls it once at start-up; programs should not.",
            internal: true,
        }
    };
}

static API: &[OsSubroutine] = &[
    os!("Math", init),
    os!("Math", F, "int", "abs", [("int", "x")], "The absolute value of x."),
    os!("Math", F, "int", "multiply", [("int", "x"), ("int", "y")], "x times y; the compiler calls this for the `*` operator."),
    os!("Math", F, "int", "divide", [("int", "x"), ("int", "y")], "x divided by y, rounded towards zero; the compiler calls this for the `/` operator."),
    os!("Math", F, "int", "min", [("int", "x"), ("int", "y")], "The smaller of x and y."),
    os!("Math", F, "int", "max", [("int", "x"), ("int", "y")], "The larger of x and y."),
    os!("Math", F, "int", "sqrt", [("int", "x")], "The integer part of the square root of x (x must not be negative)."),
    os!("String", C, "String", "new", [("int", "maxLength")], "A new empty string that can hold up to maxLength characters."),
    os!("String", M, "void", "dispose", [], "Frees the memory used by this string."),
    os!("String", M, "int", "length", [], "The number of characters currently in this string."),
    os!("String", M, "char", "charAt", [("int", "j")], "The character at position j (counting from 0)."),
    os!("String", M, "void", "setCharAt", [("int", "j"), ("char", "c")], "Replaces the character at position j with c."),
    os!("String", M, "String", "appendChar", [("char", "c")], "Adds c to the end of this string and returns this string, so calls can be chained."),
    os!("String", M, "void", "eraseLastChar", [], "Removes the last character of this string."),
    os!("String", M, "int", "intValue", [], "The integer written at the start of this string (an optional `-` followed by digits)."),
    os!("String", M, "void", "setInt", [("int", "val")], "Replaces the contents of this string with the decimal digits of val."),
    os!("String", F, "char", "backSpace", [], "The character code of the backspace key (129)."),
    os!("String", F, "char", "doubleQuote", [], "The character code of a double quote (34)."),
    os!("String", F, "char", "newLine", [], "The character code of the newline key (128)."),
    os!("Array", F, "Array", "new", [("int", "size")], "A new array with room for size elements."),
    os!("Array", M, "void", "dispose", [], "Frees the memory used by this array."),
    os!("Output", init),
    os!("Output", F, "void", "moveCursor", [("int", "i"), ("int", "j")], "Moves the cursor to row i (0..22) and column j (0..63)."),
    os!("Output", F, "void", "printChar", [("char", "c")], "Prints c at the cursor and moves the cursor on."),
    os!("Output", F, "void", "printString", [("String", "s")], "Prints the characters of s starting at the cursor."),
    os!("Output", F, "void", "printInt", [("int", "i")], "Prints i in decimal starting at the cursor."),
    os!("Output", F, "void", "println", [], "Moves the cursor to the start of the next line."),
    os!("Output", F, "void", "backSpace", [], "Moves the cursor one column back."),
    os!("Screen", init),
    os!("Screen", F, "void", "clearScreen", [], "Clears the whole screen."),
    os!("Screen", F, "void", "setColor", [("boolean", "b")], "Chooses the colour of later drawing: true for black, false for white."),
    os!("Screen", F, "void", "drawPixel", [("int", "x"), ("int", "y")], "Draws the pixel at column x and row y."),
    os!("Screen", F, "void", "drawLine", [("int", "x1"), ("int", "y1"), ("int", "x2"), ("int", "y2")], "Draws a straight line from (x1, y1) to (x2, y2)."),
    os!("Screen", F, "void", "drawRectangle", [("int", "x1"), ("int", "y1"), ("int", "x2"), ("int", "y2")], "Fills the rectangle whose top-left corner is (x1, y1) and bottom-right corner is (x2, y2)."),
    os!("Screen", F, "void", "drawCircle", [("int", "x"), ("int", "y"), ("int", "r")], "Fills a circle of radius r (at most 181) centred on (x, y)."),
    os!("Keyboard", init),
    os!("Keyboard", F, "char", "keyPressed", [], "The code of the key held down right now, or 0 if none is."),
    os!("Keyboard", F, "char", "readChar", [], "Waits for a key press and release, echoes the character and returns its code."),
    os!("Keyboard", F, "String", "readLine", [("String", "message")], "Prints message, then reads and echoes a line of text up to Enter and returns it."),
    os!("Keyboard", F, "int", "readInt", [("String", "message")], "Prints message, then reads and echoes a line and returns the number at its start."),
    os!("Memory", init),
    os!("Memory", F, "int", "peek", [("int", "address")], "The value stored in RAM at address."),
    os!("Memory", F, "void", "poke", [("int", "address"), ("int", "value")], "Stores value in RAM at address."),
    os!("Memory", F, "Array", "alloc", [("int", "size")], "Reserves a free block of size words on the heap and returns its address."),
    os!("Memory", F, "void", "deAlloc", [("Array", "o")], "Returns a block obtained from alloc to the heap."),
    os!("Sys", F, "void", "init", [], "Starts the computer: initialises the other OS classes, then calls Main.main and halts."),
    os!("Sys", F, "void", "halt", [], "Stops the program for good."),
    os!("Sys", F, "void", "error", [("int", "errorCode")], "Shows the error code on the screen and halts."),
    os!("Sys", F, "void", "wait", [("int", "duration")], "Pauses for about duration milliseconds."),
];

/// Every subroutine of the Jack OS.
pub fn api() -> &'static [OsSubroutine] {
    API
}

/// The subroutines of one OS class.
pub fn class_api(class: &str) -> impl Iterator<Item = &'static OsSubroutine> + '_ {
    API.iter().filter(move |s| s.class == class)
}

/// Looks up `class.name` in the OS API.
pub fn find(class: &str, name: &str) -> Option<&'static OsSubroutine> {
    API.iter().find(|s| s.class == class && s.name == name)
}

/// Whether `class` is one of the eight OS classes.
pub fn is_os_class(class: &str) -> bool {
    OS_CLASSES.contains(&class)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_is_complete_and_unique() {
        for class in OS_CLASSES {
            assert!(class_api(class).count() > 0, "{class}");
            assert!(class_description(class).is_some());
        }
        for (i, a) in API.iter().enumerate() {
            for b in &API[i + 1..] {
                assert!(!(a.class == b.class && a.name == b.name), "{}.{}", a.class, a.name);
            }
        }
        assert_eq!(
            find("Math", "max").map(|s| s.signature()),
            Some("function int Math.max(int x, int y)".to_string())
        );
        assert_eq!(find("String", "charAt").map(|s| s.kind), Some(SubroutineKind::Method));
    }
}
