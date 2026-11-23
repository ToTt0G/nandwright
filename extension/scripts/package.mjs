// Builds the VSIX: the one file a user installs.
//
//   node scripts/package.mjs            -> ../dist/nandwright-<version>.vsix
//
// The README, changelog, license and notices live at the repository root and
// are copied in for packaging, so the Marketplace page and the repository
// page are the same text.

import { execFileSync } from 'node:child_process';
import { copyFileSync, mkdirSync, readFileSync, rmSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const extension = join(dirname(fileURLToPath(import.meta.url)), '..');
const repo = join(extension, '..');
const manifest = JSON.parse(readFileSync(join(extension, 'package.json'), 'utf8'));
const copied = ['README.md', 'CHANGELOG.md', 'LICENSE', 'NOTICE.md'];
const node = process.execPath;
const run = (script, args = []) => execFileSync(node, [join(extension, script), ...args], { cwd: extension, stdio: 'inherit' });

if (!process.argv.includes('--skip-wasm')) {
    run('scripts/build-wasm.mjs');
}
run('scripts/build.mjs', ['--production']);

for (const name of copied) {
    copyFileSync(join(repo, name), join(extension, name));
}
const out = join(repo, 'dist');
mkdirSync(out, { recursive: true });
const vsix = join(out, `${manifest.name}-${manifest.version}.vsix`);
try {
    const repository = manifest.repository.url.replace(/\.git$/, '');
    execFileSync(
        node,
        [
            join(extension, 'node_modules', '@vscode', 'vsce', 'vsce'),
            'package',
            '--no-dependencies',
            '--out',
            vsix,
            '--baseContentUrl',
            `${repository}/blob/main`,
            '--baseImagesUrl',
            `${repository}/raw/main`,
        ],
        { cwd: extension, stdio: 'inherit' },
    );
} finally {
    for (const name of copied) {
        rmSync(join(extension, name), { force: true });
    }
}
console.log(vsix);
