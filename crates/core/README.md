# n2t-core

The front-ends shared by the rest of the engine: lexers, parsers and semantic analysis for
HDL chips and for test scripts (all three dialects), the interfaces of the built-in chips,
the line-preserving formatter, diagnostics, and `host`, the file-system abstraction that
lets the engine compile to WebAssembly.

It simulates nothing and has no dependencies.

- `hdl`: `parse`, `Elaborator` (resolves a chip and everything it is built from), `format_source`
- `tst`: `parse`, `analyze`, `Dialect`, `format_source`
- `host`: `Host`, `StdHost`, `MemoryHost`, `path`, text decoding that survives Windows-1252
- `builtins`: the pins, clocking and script-visible state of every built-in chip
