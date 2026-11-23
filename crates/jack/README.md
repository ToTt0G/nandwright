# n2t-jack

The Jack compiler of Nandwright: Jack source to VM code, plus tokens, an
error-tolerant syntax tree, symbol resolution and the Jack OS API for the
language server. The library has no I/O and builds for `wasm32-unknown-unknown`.

```rust
pub fn compile(file_stem: &str, source: &str) -> Compiled;            // one class
pub fn compile_with(file_stem: &str, source: &str, options: &Options) -> Compiled;
pub fn compile_program(files: &[(&str, &str)]) -> ProgramOutput;       // a folder
pub fn compile_program_with(files: &[(&str, &str)], options: &Options) -> ProgramOutput;
pub fn parse(source: &str) -> ParsedJack;      // tokens (with comments) + tree + diagnostics
pub fn resolve(parsed: &ParsedJack) -> Symbols; // what every identifier refers to
pub fn analyze(file_stem: &str, source: &str) -> FileAnalysis; // all three at once
pub mod os;                                     // the Jack OS API table
pub fn tokens_xml(source: &str) -> Result<String, Vec<Diagnostic>>; // project-10 *T.xml
pub fn parse_xml(source: &str) -> Result<String, Vec<Diagnostic>>;  // project-10 *.xml
```

`examples/jackc.rs` compiles a file or directory from the command line
(`cargo run -p n2t-jack --example jackc -- path/to/Dir`).
`conformance/jack/run.py` diffs it against the official `JackCompiler`.

## Agreement with the official compiler

For every program the official `JackCompiler` accepts, the VM code is
byte-identical. It rejects the same programs and reports the first error on the
same line. Diagnostics carry exact spans and their own wording.
`Compiled::official_lines` gives the line the official tool prints, which is
sometimes different (see below).

What follows is what the official compiler actually does, found by experiment.
A reader of the book would not expect most of it. This crate reproduces each
behaviour unless the entry says otherwise.

### Tokens

* The tokenizer behaves like Java's `StreamTokenizer`. Whitespace is U+0000 to
  U+0020. Words may contain `_`, letters, digits, U+00A0 to U+00FF and anything
  above U+00FF. U+0080 to U+009F, `$`, `@`, `?` and `` ` `` are single stray
  characters.
* Numbers may have a fraction, which is truncated: `1.5` is `1`, and `32767.9`
  is accepted. `1.2.3` is `1.2` followed by `.3`. `3x` is `3` followed by `x`.
  Values above 32767 give "Integer constant too big". *We add a warning for
  fractions.*
* Strings interpret backslash escapes: `\n`, `\t`, `\\`, `\"`, octal `\101`, and
  `\q` becomes `q`. A string with no closing quote ends at the line break
  without an error: `"abc` followed by `;` on the next line is valid. Characters
  are emitted as signed 16-bit UTF-16 units, so an emoji becomes
  `push constant -10179` and `push constant -8704`. *We add a warning for an
  unclosed string and a hint for escapes.*
* A `/*` comment with no `*/` silently swallows the rest of the file. Comments
  do not nest. *We add a warning.*
* A UTF-8 byte order mark makes the first word `﻿class`, so the file is
  rejected ("Expected 'class'"). *We reject it too, with a message that names
  the BOM.*
* Line numbers: inside a block comment, a line break directly after another
  line break is not counted. A line break escaped inside a string is not
  counted either.

### Declarations and names

* Duplicate declarations are accepted: `var int x, x;`, a field and a static of
  the same name, two parameters of the same name. The latest declaration wins,
  and every name still gets its own slot (`var int x, x; var int y;` makes
  `y` local 2).
* Names are looked up in this order: arguments, locals, fields, statics.
  Arguments win over locals of the same name.
* Fields are invisible inside functions ("x is not defined"). Before a `.`,
  an unknown name is taken as a class name, so `fld.m()` in a function quietly
  compiles to `call fld.m`. *We add a warning.*
* A call on a variable of primitive type compiles: `do i.foo()` with `int i`
  emits `call int.foo 1`.
* A second subroutine with the same name is an error ("Subroutine f
  redeclared"), and so is a class name that differs from the file name.

### Types

The book's Jack is untyped, but the official compiler has its own operand
checker:

* Types: *any* (object variables, calls, array elements and every parenthesised
  expression), integer constant, string constant, `true`/`false`, `null`,
  `this`, int, char and boolean variables. Unary `-` and `~` keep the type of
  their operand.
* An expression is checked left to right. Its type starts as *any* (or as
  "integer constant" inside `a[…]`) and is combined with each operand in turn.
  An operand is illegal unless:
  * it is an integer constant and the running type is an integer constant,
    int or char;
  * it is `true`/`false` and the running type is an integer constant;
  * it is int and the running type is an integer constant or int;
  * it is char and the running type is an integer constant or char;
  * it is boolean and the running type is an integer constant, boolean or
    `true`/`false`.

  Strings, `null` and `this` are never legal after an operator. The operator
  plays no part. The result is the running type, except that an integer
  constant takes the type of the next operand. So `true & false`,
  `flag | true`, `x + c` (int plus char) and `1 + null` are errors, while
  `(flag) + 1`, `1 + flag` and `vs + flag` are not.
* After a string constant, a variable, array element or call is also rejected
  ("Illegal casting into String constant"), even inside a unary operator.
* `let v = e` checks `e` against the type of `v`. An int rejects strings,
  `true`/`false`, `null`, `this`, char and boolean. A char rejects strings,
  `true`/`false`, `null`, `this`, int and boolean. A boolean rejects strings,
  `null`, `this` and char, but accepts int. Object variables accept anything.
  An integer constant fits int, char and boolean. The official line for this
  error is **one less than the line of the `;`**.
* Conditions, arguments, return values and `let a[i] = …` are not
  type-checked.

### Statements and flow

* `this` in a function is an error. Inside a constructor, `return` must be
  followed by an expression that starts with `this`, so `return this + 1;`
  passes. A void subroutine must not return a value, and a non-void one must.
* Every subroutine must end in a `return` on every path, void ones included.
  The analysis is unusual. A `while` counts as returning if its body does.
  An `if` without `else` never returns, even when it comes after a `return`.
  `let` and `do` keep the current state. A list entered after a `return`
  never warns again.
* "Warning: Unreachable code" is given at most once per statement list. It is
  a warning, so the `.vm` file is still written.

### Calls

* `f()` inside a function is an error at once ("called as a method from
  within a function"), whether or not `f` exists.
* The remaining call checks run after all files are compiled, and they only
  check classes that compiled without errors in the same run. A call to a
  subroutine that does not exist, a method called as a function or the
  reverse, or a wrong argument count is printed as an error, **but the `.vm`
  file is still written**. *By default we withhold the VM code. Set
  `Options::keep_vm_on_call_errors` to get it anyway.*
* The official compiler knows nothing about the OS, nor about classes outside
  the folder. *`compile_program` adds warnings for wrong OS calls and for
  unknown classes. A single-file `compile` only checks calls within its own
  class, as the official compiler does.*

### Output

* Labels: `IF_TRUEn`, `IF_FALSEn` and `IF_ENDn` (the last only when there is an
  `else`), and `WHILE_EXPn` and `WHILE_ENDn`. There are separate counters for
  `if` and `while`. Both reset in every subroutine and count in source order.
* `a[i]` pushes the index before the base address. `let a[i] = e` evaluates the
  index, then the base, then `e`, then
  `pop temp 0 / pop pointer 1 / push temp 0 / pop that 0`.
* `true` is `push constant 0` then `not`. `false` and `null` are
  `push constant 0`. `this` is `push pointer 0`. Every `do` ends with
  `pop temp 0`, and a void return pushes a 0.
* An empty class compiles to an empty file.

### Deliberate differences

* **More than 100 nested expressions** (through parentheses, array indexes or
  call arguments): the official compiler prints nothing and writes a `.vm`
  file that holds only the `function` line of that subroutine and nothing
  after it. We report an error.
* **More than 128 levels of nesting** of expressions, unary operators and
  blocks counted together: the official compiler accepts deeper unary chains
  and blocks. We stop with an error, so that the recursive passes never
  overflow the stack. That is 512 KiB in a release build, well inside
  wasm's default of 1 MiB.
* Call-check errors withhold the VM code by default (see above).
