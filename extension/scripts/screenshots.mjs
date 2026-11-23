// Takes the README screenshots: real captures of the real extension running
// in a real VS Code, driven with Playwright. Nothing is mocked or composed.
//
//   xvfb-run -a -s "-screen 0 2800x1800x24" node scripts/screenshots.mjs [--theme dark|light|hc] [--only name] [--out dir]
//
// The extension must be built first (npm run build:wasm && npm run build).
// The workspace is a scratch folder of practice chips from tests/fixtures;
// no course file appears in any screenshot.

import { downloadAndUnzipVSCode } from '@vscode/test-electron';
import { _electron as electron } from 'playwright-core';
import { cpSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const extension = join(dirname(fileURLToPath(import.meta.url)), '..');
const repo = join(extension, '..');
const args = process.argv.slice(2);
const option = (name, fallback) => {
    const index = args.indexOf(`--${name}`);
    return index >= 0 ? args[index + 1] : fallback;
};
const themeKey = option('theme', 'dark');
const only = option('only', '');
const outDir = resolve(option('out', join(repo, 'docs', 'images')));
const scale = Number(option('scale', '2'));
const [WIDTH, HEIGHT] = option('size', '1280x800').split('x').map(Number);

const THEMES = {
    dark: 'Default Dark Modern',
    light: 'Default Light Modern',
    hc: 'Default High Contrast',
    hclight: 'Default High Contrast Light',
};

const scratch = mkdtempSync(join(tmpdir(), 'nandwright-shots-'));
// The repository's examples folder, plus the unwritten practice chip that
// "Try the example chip" opens (for the animated capture).
const workspace = join(scratch, 'nandwright-examples');
cpSync(join(repo, 'examples'), workspace, { recursive: true });
mkdirSync(join(workspace, 'practice'), { recursive: true });
cpSync(join(extension, 'media', 'example'), join(workspace, 'practice'), { recursive: true });
const chips = join(workspace, 'chips');

const userData = join(scratch, 'user');
mkdirSync(join(userData, 'User'), { recursive: true });
writeFileSync(
    join(userData, 'User', 'settings.json'),
    JSON.stringify(
        {
            'workbench.colorTheme': THEMES[themeKey] ?? THEMES.dark,
            'workbench.startupEditor': 'none',
            'workbench.tips.enabled': false,
            'workbench.layoutControl.enabled': false,
            'workbench.editor.empty.hint': 'hidden',
            'workbench.statusBar.visible': true,
            'workbench.secondarySideBar.defaultVisibility': 'hidden',
            'window.commandCenter': false,
            // Without a window manager a native title bar is no title bar: the
            // capture is the workbench alone.
            'window.titleBarStyle': 'native',
            'chat.disableAIFeatures': true,
            'window.menuBarVisibility': 'hidden',
            'chat.commandCenter.enabled': false,
            'chat.agent.enabled': false,
            'editor.minimap.enabled': false,
            'editor.fontSize': 14,
            'editor.renderWhitespace': 'none',
            'editor.stickyScroll.enabled': false,
            'editor.inlineSuggest.enabled': false,
            'editor.codeLens': true,
            'breadcrumbs.enabled': false,
            'telemetry.telemetryLevel': 'off',
            'update.mode': 'none',
            'extensions.autoUpdate': false,
            'extensions.ignoreRecommendations': true,
            'security.workspace.trust.enabled': false,
            'git.enabled': false,
            'git.openRepositoryInParentFolders': 'never',
            'testing.automaticallyOpenPeekView': 'never',
            'testing.openTesting': 'neverOpen',
            'explorer.compactFolders': false,
            'files.autoSave': 'off',
            // Scripted typing must not be completed or closed for it.
            ...(args.includes('--demo')
                ? {
                      'editor.quickSuggestions': { other: 'off', comments: 'off', strings: 'off' },
                      'editor.suggestOnTriggerCharacters': false,
                      'editor.acceptSuggestionOnEnter': 'off',
                      'editor.autoClosingBrackets': 'never',
                      'editor.parameterHints.enabled': false,
                      'editor.codeLens': false,
                      'editor.fontSize': 15,
                  }
                : {}),
        },
        null,
        2,
    ),
);

// Commands are run by id through key bindings made for this script, which is
// more reliable than typing their titles into the command palette.
const KEYS = {
    closePanel: ['ctrl+alt+shift+1', 'workbench.action.closePanel'],
    closeEditors: ['ctrl+alt+shift+2', 'workbench.action.closeAllEditors'],
    clearNotifications: ['ctrl+alt+shift+3', 'notifications.clearAll'],
    showTests: ['ctrl+alt+shift+4', 'workbench.view.testing.focus'],
    runAllTests: ['ctrl+alt+shift+5', 'testing.runAll'],
    runTest: ['ctrl+alt+shift+6', 'nandwright.runTest'],
    openChip: ['ctrl+alt+shift+7', 'nandwright.openChip'],
    debugTest: ['ctrl+alt+shift+8', 'nandwright.debugTest'],
    peekFailure: ['ctrl+alt+shift+9', 'testing.openOutputPeek'],
    showProblems: ['ctrl+alt+shift+0', 'workbench.actions.view.problems'],
    showHover: ['ctrl+alt+shift+h', 'editor.action.showHover'],
    stopDebugging: ['ctrl+alt+shift+s', 'workbench.action.debug.stop'],
    showExplorer: ['ctrl+alt+shift+e', 'workbench.view.explorer'],
    toggleSidebar: ['ctrl+alt+shift+b', 'workbench.action.toggleSidebarVisibility'],
    showDebug: ['ctrl+alt+shift+d', 'workbench.view.debug'],
    runProgram: ['ctrl+alt+shift+r', 'nandwright.runProgram'],
    focusEditor: ['ctrl+alt+shift+f', 'workbench.action.focusFirstEditorGroup'],
    expandTests: ['ctrl+alt+shift+x', 'testing.expandAll'],
    suggest: ['ctrl+alt+shift+c', 'editor.action.triggerSuggest'],
    nextFailure: ['ctrl+alt+shift+n', 'testing.goToNextMessage'],
};
writeFileSync(
    join(userData, 'User', 'keybindings.json'),
    JSON.stringify(Object.values(KEYS).map(([key, command]) => ({ key, command }))),
);

const executablePath = await downloadAndUnzipVSCode('stable');
const app = await electron.launch({
    executablePath,
    args: [
        workspace,
        `--extensionDevelopmentPath=${extension}`,
        `--user-data-dir=${userData}`,
        `--extensions-dir=${join(scratch, 'extensions')}`,
        '--disable-workspace-trust',
        '--skip-welcome',
        '--skip-release-notes',
        '--disable-updates',
        '--no-sandbox',
        '--disable-gpu',
        `--force-device-scale-factor=${scale}`,
        `--window-size=${WIDTH},${HEIGHT}`,
    ],
    env: { ...process.env, NANDWRIGHT_SCREENSHOTS: '1' },
    timeout: 120_000,
});

const page = await app.firstWindow();
await app.evaluate(
    ({ BrowserWindow }, size) => {
        const [window] = BrowserWindow.getAllWindows();
        window.setBounds({ x: 0, y: 0, width: size.width, height: size.height });
    },
    { width: WIDTH, height: HEIGHT },
);
await page.waitForSelector('.monaco-workbench', { timeout: 120_000 });
const pause = (ms) => new Promise((r) => setTimeout(r, ms));
await pause(4000);

/** Runs a command through its key binding. */
async function command(name) {
    const [key] = KEYS[name];
    await page.keyboard.press(
        key
            .split('+')
            .map((part) => ({ ctrl: 'Control', alt: 'Alt', shift: 'Shift' })[part] ?? part.toUpperCase())
            .join('+'),
    );
    await pause(700);
}

async function openFile(name) {
    await page.keyboard.press('Control+P');
    await page.waitForSelector('.quick-input-widget:not(.hidden) input');
    await page.keyboard.type(name, { delay: 8 });
    await pause(600);
    await page.keyboard.press('Enter');
    await pause(1200);
}

async function closeEverything() {
    await page.keyboard.press('Escape');
    await command('closeEditors');
    await command('closePanel');
    await command('clearNotifications');
    await command('showExplorer');
    await pause(300);
}

/** Moves the cursor to a 1-based line and column of the active editor. */
async function goTo(line, column = 1) {
    await page.keyboard.press('Control+G');
    await page.waitForSelector('.quick-input-widget:not(.hidden) input');
    await page.keyboard.type(`${line}:${column}`);
    await page.keyboard.press('Enter');
    await pause(300);
}

async function shot(name) {
    mkdirSync(outDir, { recursive: true });
    const suffix = themeKey === 'dark' ? '' : `-${themeKey}`;
    const file = join(outDir, `${name}${suffix}.png`);
    await page.mouse.move(WIDTH - 5, HEIGHT / 2);
    await pause(400);
    await page.screenshot({ path: file });
    console.log(file);
}

const scenes = {
    // The Testing view with a project's scripts, all passing.
    async tests() {
        await openFile('chips/Majority.hdl');
        await command('showTests');
        await pause(1500);
        await command('runAllTests');
        await pause(5000);
        await command('expandTests');
        await command('clearNotifications');
        await command('focusEditor');
        await shot('testing-view');
    },

    // A failing test: the diff of expected and actual, and the pins.
    async failure() {
        await openFile('chips/Majority.hdl');
        const file = join(chips, 'Majority.hdl');
        const original = readFileSync(file, 'utf8');
        writeFileSync(file, original.replace('And(a=a, b=c, out=ac);', 'Or(a=a, b=c, out=ac);'));
        await pause(1500);
        await openFile('chips/Majority.tst');
        await command('runTest');
        await pause(4000);
        await command('clearNotifications');
        await goTo(9, 1);
        await command('peekFailure');
        await pause(2500);
        await shot('failing-test');
        await page.keyboard.press('Escape');
        writeFileSync(file, original);
        await pause(1000);
    },

    // The chip simulator beside the source, with the truth table.
    async simulator() {
        await openFile('chips/Majority.hdl');
        await command('openChip');
        await pause(3500);
        const frame = await chipFrame();
        await frame.locator('button.bit').nth(0).click();
        await pause(400);
        await frame.locator('button.bit').nth(2).click();
        await pause(1200);
        await shot('chip-simulator');
    },

    // A clocked chip: tick, tock and time.
    async clocked() {
        await openFile('Accumulator.hdl');
        await command('openChip');
        await pause(3500);
        const frame = await chipFrame();
        const input = frame.locator('input.bus').first();
        await input.fill('25');
        await input.press('Enter');
        await pause(400);
        await frame.locator('button.bit').first().click();
        await pause(300);
        await frame.getByRole('button', { name: 'Full cycle' }).click();
        await pause(300);
        await frame.getByRole('button', { name: 'Full cycle' }).click();
        await pause(300);
        await frame.getByRole('button', { name: 'Tick' }).click();
        await pause(1200);
        await shot('clocked-chip');
    },

    // Stepping through a test script with the debugger.
    async debug() {
        await openFile('Accumulator.tst');
        await command('debugTest');
        await pause(5000);
        for (let i = 0; i < 6; i++) {
            await page.keyboard.press('F10');
            await pause(700);
        }
        await command('showDebug');
        await command('focusEditor');
        await pause(1500);
        await shot('step-through');
        await command('stopDebugging');
        await pause(1000);
    },

    // A program on the Hack computer: screen, registers, memory.
    async computer() {
        await openFile('Diagonal.asm');
        await command('runProgram');
        await pause(3500);
        const frame = await panelFrame('.screen canvas');
        await frame.getByRole('button', { name: 'Run' }).click();
        await pause(1500);
        await frame.getByRole('button', { name: 'Pause' }).click();
        await pause(1200);
        await shot('hack-computer');
    },

    // A Jack program on the VM emulator, compiled in memory.
    async jack() {
        await openFile('Sunrise/Main.jack');
        await command('runProgram');
        await pause(3500);
        const frame = await panelFrame('.screen canvas');
        await frame.getByRole('button', { name: 'Run' }).click();
        await pause(2500);
        await frame.locator('.screen').click();
        for (let i = 0; i < 25; i++) {
            await page.keyboard.down('ArrowRight');
            await pause(60);
        }
        await page.keyboard.up('ArrowRight');
        await pause(600);
        await shot('jack-program');
        await frame.getByRole('button', { name: 'Pause' }).click();
        await pause(500);
    },

    // Diagnostics and hover while editing.
    async editing() {
        await openFile('Parity4.hdl');
        const file = join(chips, 'Parity4.hdl');
        const original = readFileSync(file, 'utf8');
        writeFileSync(file, original.replace('Xor(a=low, b=high, out=out);', 'Xor(a=low, b=hihg, out=out[1]);'));
        await pause(2500);
        await command('showProblems');
        await pause(1500);
        await command('focusEditor');
        await goTo(11, 5);
        await command('showHover');
        await pause(1500);
        await shot('editing');
        await page.keyboard.press('Escape');
        writeFileSync(file, original);
        await pause(1000);
    },
};

/**
 * The animated capture: write a chip, run its test, see it fail, fix it, see
 * it pass. Frames are real screenshots taken while the editor is driven.
 */
async function demo() {
    const framesDir = join(outDir, 'demo-frames');
    rmSync(framesDir, { recursive: true, force: true });
    mkdirSync(framesDir, { recursive: true });
    let count = 0;
    let capturing = true;
    const loop = (async () => {
        while (capturing) {
            const started = Date.now();
            await page.screenshot({ path: join(framesDir, `frame-${String(count++).padStart(4, '0')}.png`) }).catch(() => {});
            await pause(Math.max(0, 125 - (Date.now() - started)));
        }
    })();
    const type = (text) => page.keyboard.type(text, { delay: 28 });

    await command('toggleSidebar');
    await openFile('practice/Majority.hdl');
    await pause(1200);
    await goTo(16, 1);
    await page.keyboard.press('End');
    await page.keyboard.down('Shift');
    await page.keyboard.press('Home');
    await page.keyboard.press('Home');
    await page.keyboard.up('Shift');
    // The first attempt has a mistake: Or where an And belongs.
    await type('    And(a=a, b=b, out=ab);');
    await page.keyboard.press('Enter');
    await type('Or(a=a, b=c, out=ac);');
    await page.keyboard.press('Enter');
    await type('And(a=b, b=c, out=bc);');
    await page.keyboard.press('Enter');
    await type('Or(a=ab, b=ac, out=either);');
    await page.keyboard.press('Enter');
    await type('Or(a=either, b=bc, out=out);');
    await page.keyboard.press('Escape');
    await pause(1200);
    await command('runTest');
    await pause(2200);
    // The failure: the notification leads to the failing row and its details.
    await page.getByRole('button', { name: 'Show Details' }).click();
    await pause(1500);
    await command('peekFailure');
    await pause(4500);
    await page.keyboard.press('Escape');
    await command('closePanel');
    await openFile('practice/Majority.hdl');
    await pause(700);
    // Fix line 17: Or -> And.
    await goTo(17, 5);
    await page.keyboard.down('Shift');
    await page.keyboard.press('ArrowRight');
    await page.keyboard.press('ArrowRight');
    await page.keyboard.up('Shift');
    await pause(500);
    await type('And');
    await pause(1000);
    await command('runTest');
    await pause(3500);
    capturing = false;
    await loop;
    console.log(`${count} frames in ${framesDir}`);
}

/** The frame of the webview panel that contains `selector`. */
async function panelFrame(selector) {
    for (let attempt = 0; attempt < 40; attempt++) {
        for (const frame of page.frames()) {
            if ((await frame.locator(selector).count().catch(() => 0)) > 0) {
                return frame;
            }
        }
        await pause(250);
    }
    throw new Error(`no panel with ${selector} appeared`);
}

const chipFrame = () => panelFrame('#root .pins');

let failed = false;
try {
    if (args.includes('--demo')) {
        await closeEverything();
        await demo();
    }
    for (const [name, scene] of Object.entries(args.includes('--demo') ? {} : scenes)) {
        if (only && only !== name) {
            continue;
        }
        try {
            await closeEverything();
            await scene();
        } catch (error) {
            failed = true;
            console.error(`scene ${name} failed: ${error.message}`);
            await page.screenshot({ path: join(outDir, `FAILED-${name}.png`) }).catch(() => {});
        }
    }
} finally {
    await app.close().catch(() => {});
    rmSync(scratch, { recursive: true, force: true });
}
process.exit(failed ? 1 : 0);
