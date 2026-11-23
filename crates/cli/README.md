# n2t-cli

The engine as a native command, `n2t`. The test suite and the conformance harness use it;
the VS Code extension does not (it ships the engine as WebAssembly).

```sh
n2t test <paths>…     # run .tst scripts; --json for one JSON object per script
n2t check <paths>…    # diagnostics for .hdl, .tst, .asm, .hack, .vm and .jack files
n2t eval Chip.hdl a=1 b=0 [--cycles N]
n2t fmt [--check] <paths>…
n2t builtins          # list the built-in chips
n2t lsp               # the language server on stdin/stdout
```

`n2t test` exits with 0 if every script passed, 1 if one failed, 2 on a usage error.
