# Notices and attribution

Nandwright is licensed under the Mozilla Public License 2.0 (see `LICENSE`).

It is an unofficial project. It is not affiliated with, sponsored by or endorsed by
the authors of Nand to Tetris (Noam Nisan and Shimon Schocken) or by MIT Press.
"Nand to Tetris" and "The Elements of Computing Systems" belong to their owners.
This repository contains no course material: no official tools, project files,
test scripts, compare files or OS sources, and no solutions to course projects.

The official Java tools were used only as a black box to compare behaviour with.
Their class files were not decompiled and their sources were not copied or translated.

## Code this project is derived from

| Source | What was taken | License |
|---|---|---|
| [isXander/nand2tetris](https://github.com/isXander/nand2tetris) at commit `5236f078dcfc4ca44a17067daa36f390c41e2e14` | The Rust crates `n2t-core`, `n2t-simulator`, `n2t-lsp` and `n2t-cli` (HDL and test-script front-ends, hardware simulator, language server, command line), the HDL, TST and CMP TextMate grammars and language configurations, the HDL and TST snippets, and the fixtures under `tests/fixtures/{errors,features,runtime}`. All modified since. | MPL-2.0 (declared in upstream's `Cargo.toml`; upstream ships no license file) |
| [nand2tetris/web-ide](https://github.com/nand2tetris/web-ide) at commit `3d39c945b38284788145d88ce61dd9582187cc52` | The 8×11 Hack bitmap font table in `crates/vm/src/os/font.rs`, derived from `simulator/src/vm/os/font.ts` and adjusted to match the Java VM Emulator. Nothing else was taken from the web IDE. | MIT; notice below |

Everything else was written for this project and is MPL-2.0: the crates `n2t-hack`
(assembler, CPU emulator), `n2t-vm` (VM emulator and built-in OS), `n2t-jack` (Jack
compiler) and `n2t-wasm`, the formatter, the VS Code extension, the conformance harness,
the examples and the documentation.

### MIT notice for the font table (nand2tetris/web-ide)

```
MIT License

Copyright 2022 David Souther et al

This software is based on Stefano Volpe's 'Nand2Tetris Tools'. Please check
https://github.com/foxyseta/nand-ide/blob/master/LICENSE for further information.
This software is based on Aviv Yaish's 'NAND IDE'. Please check
https://github.com/AvivYaish/nand-ide/blob/master/LICENSE for further information.

Permission is hereby granted, free of charge, to any person obtaining a copy of this
software and associated documentation files (the "Software"), to deal in the Software
without restriction, including without limitation the rights to use, copy, modify, merge,
publish, distribute, sublicense, and/or sell copies of the Software, and to permit persons
to whom the Software is furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all copies or
substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED,
INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR
PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE
FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR
OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
DEALINGS IN THE SOFTWARE.
```

## Packages bundled in the extension

The VSIX bundles `vscode-languageclient`, `vscode-languageserver-protocol`,
`vscode-jsonrpc` and their dependencies `minimatch` and `semver` (MIT and ISC, Microsoft
and contributors). The engine links the Rust crates `lsp-server`, `lsp-types`, `serde`,
`serde_json`, `url`, `percent-encoding` and their dependencies (MIT or Apache-2.0).
