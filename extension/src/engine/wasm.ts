// Loads the Nandwright engine (WebAssembly) and gives it the file system.
//
// The engine is one .wasm file, the same bytes on Windows, macOS and Linux.
// It reaches files only through the four imports below, so this is the one
// place where the engine meets the host's paths.

import * as fs from 'node:fs';

/** The engine died (a trap). The instance is gone; make a new one. */
export class EngineCrash extends Error {
    constructor(detail: string) {
        super(detail);
        this.name = 'EngineCrash';
    }
}

interface Exports {
    memory: WebAssembly.Memory;
    nw_alloc(len: number): number;
    nw_free(ptr: number, len: number): void;
    nw_call(ptr: number, len: number): number;
}

const enum Status {
    Ok = 0,
    NotFound = 1,
    Error = 2,
}

const encoder = new TextEncoder();
const decoder = new TextDecoder('utf-8');

let compiled: { path: string; module: WebAssembly.Module } | undefined;

function compile(wasmPath: string): WebAssembly.Module {
    if (compiled?.path !== wasmPath) {
        compiled = { path: wasmPath, module: new WebAssembly.Module(fs.readFileSync(wasmPath)) };
    }
    return compiled.module;
}

/**
 * Engine-form paths use `/` separators on every platform (`C:/Users/me/x`).
 * Node's `fs` accepts them as they are, also on Windows, so no conversion is
 * needed on the way out.
 */
function isMissing(error: unknown): boolean {
    const code = (error as NodeJS.ErrnoException | undefined)?.code;
    return code === 'ENOENT' || code === 'ENOTDIR';
}

function describe(error: unknown): string {
    const e = error as NodeJS.ErrnoException | undefined;
    switch (e?.code) {
        case 'EACCES':
        case 'EPERM':
            return 'permission denied';
        case 'EISDIR':
            return 'it is a folder, not a file';
        case 'EBUSY':
            return 'the file is in use by another program';
        default:
            return e?.message ?? String(error);
    }
}

export class WasmEngine {
    private exports: Exports;
    private lastLog = '';
    private dead = false;

    constructor(wasmPath: string, private readonly log: (line: string) => void = () => {}) {
        const instance = new WebAssembly.Instance(compile(wasmPath), {
            host: {
                read: (path: number, pathLen: number, out: number) =>
                    this.guard(out, () => fs.readFileSync(this.text(path, pathLen))),
                list: (path: number, pathLen: number, out: number) =>
                    this.guard(out, () => {
                        const dir = this.text(path, pathLen);
                        const lines = fs.readdirSync(dir, { withFileTypes: true }).map((entry) => {
                            let isDir = entry.isDirectory();
                            if (entry.isSymbolicLink()) {
                                try {
                                    isDir = fs.statSync(`${dir}/${entry.name}`).isDirectory();
                                } catch {
                                    isDir = false;
                                }
                            }
                            return isDir ? `${entry.name}/` : entry.name;
                        });
                        return encoder.encode(lines.join('\n'));
                    }),
                write: (path: number, pathLen: number, data: number, dataLen: number, out: number) =>
                    this.guard(out, () => {
                        // Copy before writing: the view dies if memory grows.
                        const bytes = new Uint8Array(this.exports.memory.buffer, data, dataLen).slice();
                        fs.writeFileSync(this.text(path, pathLen), bytes);
                        return new Uint8Array(0);
                    }),
                log: (ptr: number, len: number) => {
                    this.lastLog = this.text(ptr, len);
                    this.log(this.lastLog);
                },
            },
        });
        this.exports = instance.exports as unknown as Exports;
    }

    private text(ptr: number, len: number): string {
        return decoder.decode(new Uint8Array(this.exports.memory.buffer, ptr, len));
    }

    /** Hands `bytes` to the engine in a buffer it will free. */
    private give(out: number, bytes: Uint8Array): void {
        const ptr = this.exports.nw_alloc(bytes.length);
        new Uint8Array(this.exports.memory.buffer, ptr, bytes.length).set(bytes);
        const view = new DataView(this.exports.memory.buffer);
        view.setUint32(out, ptr, true);
        view.setUint32(out + 4, bytes.length, true);
    }

    private guard(out: number, action: () => Uint8Array): number {
        try {
            this.give(out, action());
            return Status.Ok;
        } catch (error) {
            if (isMissing(error)) {
                this.give(out, new Uint8Array(0));
                return Status.NotFound;
            }
            this.give(out, encoder.encode(describe(error)));
            return Status.Error;
        }
    }

    /** Sends one request to the engine. Throws `EngineCrash` if it traps. */
    call<T = any>(request: object): T {
        return JSON.parse(this.callText(JSON.stringify(request)));
    }

    callText(request: string): string {
        if (this.dead) {
            throw new EngineCrash('the engine has stopped');
        }
        const bytes = encoder.encode(request);
        try {
            const ptr = this.exports.nw_alloc(bytes.length);
            new Uint8Array(this.exports.memory.buffer, ptr, bytes.length).set(bytes);
            const reply = this.exports.nw_call(ptr, bytes.length);
            const length = new DataView(this.exports.memory.buffer).getUint32(reply, true);
            const text = decoder.decode(new Uint8Array(this.exports.memory.buffer, reply + 4, length));
            this.exports.nw_free(reply, length + 4);
            return text;
        } catch (error) {
            this.dead = true;
            const detail = this.lastLog || (error instanceof Error ? error.message : String(error));
            throw new EngineCrash(detail);
        }
    }
}
