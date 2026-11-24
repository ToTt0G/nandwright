// Bundles the extension: the extension host entry, the language-server
// process, the runtime worker and the webview scripts.

import * as esbuild from 'esbuild';
import { existsSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const extension = join(dirname(fileURLToPath(import.meta.url)), '..');
const watch = process.argv.includes('--watch');
const production = process.argv.includes('--production');

const common = {
    bundle: true,
    sourcemap: !production,
    minify: production,
    logLevel: 'info',
    absWorkingDir: extension,
};

const node = {
    ...common,
    platform: 'node',
    target: 'node20',
    format: 'cjs',
    external: ['vscode'],
};

const builds = [
    { ...node, entryPoints: ['src/extension.ts'], outfile: 'dist/extension.js' },
    { ...node, entryPoints: ['src/server/main.ts'], outfile: 'dist/server.js' },
    { ...node, entryPoints: ['src/runtime/worker.ts'], outfile: 'dist/runtime.js' },
    {
        ...common,
        platform: 'browser',
        target: 'es2022',
        format: 'iife',
        entryPoints: ['src/webview/chip.ts'],
        outfile: 'dist/webview/chip.js',
    },
    {
        ...common,
        platform: 'browser',
        target: 'es2022',
        format: 'iife',
        entryPoints: ['src/webview/computer.ts'],
        outfile: 'dist/webview/computer.js',
    },
];

if (process.argv.includes('--tests')) {
    builds.length = 0;
    builds.push({
        ...node,
        entryPoints: ['src/test/unit/engine.test.ts', 'src/test/unit/zip.test.ts', 'src/test/unit/results.test.ts', 'src/test/unit/zipName.test.ts', 'src/test/unit/textCompare.test.ts'],
        outdir: 'dist-test/unit',
        outExtension: { '.js': '.cjs' },
        sourcemap: 'inline',
    });
}

if (!existsSync(join(extension, 'dist', 'engine.wasm'))) {
    console.warn('dist/engine.wasm is missing: run `npm run build:wasm` first.');
}

if (watch) {
    for (const options of builds) {
        const context = await esbuild.context(options);
        await context.watch();
    }
} else {
    await Promise.all(builds.map((options) => esbuild.build(options)));
}
