# Notices and attribution

Nandwright is licensed under the Mozilla Public License 2.0 (see `LICENSE`).

It is an unofficial project. It is not affiliated with, sponsored by or endorsed by
the authors of Nand to Tetris (Noam Nisan and Shimon Schocken) or by MIT Press.
"Nand to Tetris" and "The Elements of Computing Systems" belong to their owners.
This repository contains no course material: no official tools, project files,
test scripts, compare files or OS sources, and no solutions to course projects.

## Code this project is derived from

| Source | What was taken | License |
|---|---|---|
| [isXander/nand2tetris](https://github.com/isXander/nand2tetris) at commit `5236f078dcfc4ca44a17067daa36f390c41e2e14` | The Rust crates `n2t-core`, `n2t-simulator`, `n2t-lsp` and `n2t-cli` (HDL and test-script front-ends, hardware simulator, language server, command line), the HDL/TST/CMP TextMate grammars, the fixtures under `tests/fixtures/{errors,features,runtime}` and the specifications under `docs/spec`. Modified since; see the git history of this repository. | MPL-2.0 (declared in upstream's `Cargo.toml`) |
| [nand2tetris/web-ide](https://github.com/nand2tetris/web-ide) at commit `3d39c945b38284788145d88ce61dd9582187cc52` | The 8×11 Hack bitmap font table in `crates/vm/src/os/font.rs`, derived from `simulator/src/vm/os/font.ts`. | MIT (Copyright 2022 David Souther et al) |
