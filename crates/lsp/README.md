# n2t-lsp

The language server for HDL, test scripts, Hack assembly, Hack machine code, VM code, Jack
and compare/output files.

It is a state machine, not a process: `JsonServer::handle` takes one JSON-RPC message and
returns the messages to send back. The VS Code extension runs it inside WebAssembly and owns
the transport; `run_stdio` runs it natively over stdin and stdout for other editors.

Files are reached through `n2t_core::host::Host`, and unsaved editor buffers shadow the
files on disk, so a chip is checked against its neighbours as they are in the editor.
