// Runs the integration tests in a real VS Code.
//
//   node scripts/run-integration.mjs                 the extension from this folder
//   node scripts/run-integration.mjs --vsix X.vsix   an installed VSIX (what users get)
//
// On Linux without a display, run it under xvfb:  xvfb-run -a node scripts/run-integration.mjs
// VS Code is downloaded on first use into .vscode-test/.

import { downloadAndUnzipVSCode, resolveCliArgsFromVSCodeExecutablePath, runTests } from '@vscode/test-electron';
import { spawnSync } from 'node:child_process';
import { cpSync, mkdirSync, mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import * as esbuild from 'esbuild';

const extension = join(dirname(fileURLToPath(import.meta.url)), '..');
const repo = join(extension, '..');
const args = process.argv.slice(2);
const vsixIndex = args.indexOf('--vsix');
const vsix = vsixIndex >= 0 ? resolve(args[vsixIndex + 1]) : undefined;
const versionIndex = args.indexOf('--vscode');
const version = versionIndex >= 0 ? args[versionIndex + 1] : 'stable';

await esbuild.build({
    absWorkingDir: extension,
    entryPoints: ['src/test/integration/index.ts'],
    outfile: 'dist-test/integration/index.cjs',
    bundle: true,
    platform: 'node',
    target: 'node20',
    format: 'cjs',
    external: ['vscode', 'mocha'],
    sourcemap: 'inline',
    logLevel: 'warning',
});

// A fresh workspace with a space in its path, holding copies of the fixtures:
// the tests write `.out` files and edit chips.
const scratch = mkdtempSync(join(tmpdir(), 'nandwright it '));
const workspace = join(scratch, 'My Projects', 'nand2tetris');
mkdirSync(workspace, { recursive: true });
cpSync(join(repo, 'tests', 'fixtures', 'chips'), join(workspace, 'chips'), { recursive: true });
cpSync(join(repo, 'tests', 'fixtures', 'runtime'), join(workspace, 'runtime'), { recursive: true });
cpSync(join(repo, 'tests', 'fixtures', 'programs'), join(workspace, 'programs'), { recursive: true });
cpSync(join(repo, 'crates', 'jack', 'tests', 'fixtures', 'Counter'), join(workspace, 'jack'), { recursive: true });

const userData = join(scratch, 'user-data');
const extensionsDir = join(scratch, 'extensions');
const launchArgs = [
    workspace,
    `--user-data-dir=${userData}`,
    `--extensions-dir=${extensionsDir}`,
    '--disable-workspace-trust',
    '--skip-welcome',
    '--skip-release-notes',
    '--disable-gpu',
    '--no-sandbox',
];

let code = 0;
try {
    const vscodeExecutablePath = await downloadAndUnzipVSCode(version);
    let extensionDevelopmentPath = extension;
    if (vsix) {
        const [cli, ...cliArgs] = resolveCliArgsFromVSCodeExecutablePath(vscodeExecutablePath);
        const installed = spawnSync(
            cli,
            [...cliArgs, `--extensions-dir=${extensionsDir}`, `--user-data-dir=${userData}`, '--install-extension', vsix, '--force'],
            { stdio: 'inherit', shell: process.platform === 'win32' },
        );
        if (installed.status !== 0) {
            throw new Error(`could not install ${vsix}`);
        }
        extensionDevelopmentPath = join(extension, 'test-harness');
        console.log(`Testing the installed VSIX: ${vsix}`);
    } else {
        console.log('Testing the extension from the source folder.');
    }
    await runTests({
        vscodeExecutablePath,
        extensionDevelopmentPath,
        extensionTestsPath: join(extension, 'dist-test', 'integration', 'index.cjs'),
        launchArgs,
        extensionTestsEnv: {
            NANDWRIGHT_TEST_WORKSPACE: workspace,
            NANDWRIGHT_TEST_VSIX: vsix ? '1' : '',
            NANDWRIGHT_SCREENSHOTS: process.env.NANDWRIGHT_SCREENSHOTS ?? '',
        },
    });
} catch (error) {
    console.error(String(error?.message ?? error));
    code = 1;
} finally {
    try {
        rmSync(scratch, { recursive: true, force: true });
    } catch {
        // Windows may still hold a file for a moment; the folder is in the temp dir.
    }
}
process.exit(code);
