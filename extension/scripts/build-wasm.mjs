// Builds the engine for WebAssembly and puts it where the extension loads it.
// Needs a Rust toolchain with the wasm32-unknown-unknown target; users of the
// packaged extension never run this.

import { execFileSync } from 'node:child_process';
import { copyFileSync, mkdirSync, statSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const extension = join(dirname(fileURLToPath(import.meta.url)), '..');
const repo = join(extension, '..');

execFileSync(
    'cargo',
    ['build', '-p', 'n2t-wasm', '--target', 'wasm32-unknown-unknown', '--release'],
    { cwd: repo, stdio: 'inherit' },
);
const built = join(repo, 'target', 'wasm32-unknown-unknown', 'release', 'n2t_wasm.wasm');
mkdirSync(join(extension, 'dist'), { recursive: true });
copyFileSync(built, join(extension, 'dist', 'engine.wasm'));
console.log(`dist/engine.wasm (${(statSync(built).size / 1024).toFixed(0)} KiB)`);
