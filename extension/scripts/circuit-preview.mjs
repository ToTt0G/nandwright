// A preview of the circuit diagram webview without VS Code: static pages that
// load dist/webview/circuit.js with a fake extension host (a fixture chip,
// simulated in the page) and VS Code-like theme variables, and screenshots of
// them taken with Playwright.
//
//   npm run build && node scripts/circuit-preview.mjs [--out dir] [--no-shots] [--only name]
//
// Open <out>/index.html in a browser to click around; the pages need no server.
// Only made-up chips are used (src/test/circuitFixtures.ts).

import * as esbuild from 'esbuild';
import { chromium } from 'playwright-core';
import { copyFileSync, existsSync, mkdirSync, readdirSync, writeFileSync } from 'node:fs';
import { homedir, tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const extension = join(dirname(fileURLToPath(import.meta.url)), '..');
const args = process.argv.slice(2);
const option = (name, fallback) => {
    const index = args.indexOf(`--${name}`);
    return index >= 0 ? args[index + 1] : fallback;
};
const out = resolve(option('out', join(tmpdir(), 'nandwright-circuit-preview')));
const only = option('only', '');
mkdirSync(out, { recursive: true });

const script = join(extension, 'dist', 'webview', 'circuit.js');
if (!existsSync(script)) {
    console.error('dist/webview/circuit.js is missing: run `npm run build` first.');
    process.exit(1);
}
copyFileSync(script, join(out, 'circuit.js'));
await esbuild.build({
    absWorkingDir: extension,
    entryPoints: ['src/test/circuitPreviewHost.ts'],
    bundle: true,
    format: 'iife',
    platform: 'browser',
    target: 'es2022',
    outfile: join(out, 'host.js'),
    logLevel: 'warning',
});

// Close to VS Code's Default Dark Modern, Light Modern and High Contrast.
const common = {
    'font-family': 'system-ui, "Segoe UI", Ubuntu, sans-serif',
    'font-size': '13px',
    'editor-font-family': '"DejaVu Sans Mono", Menlo, Consolas, monospace',
    'editor-font-size': '13px',
};
const THEMES = {
    dark: {
        body: 'vscode-dark',
        vars: {
            foreground: '#cccccc', 'editor-foreground': '#cccccc', 'editor-background': '#1f1f1f', descriptionForeground: '#9d9d9d',
            'widget-border': '#313131', 'panel-border': '#2b2b2b', focusBorder: '#0078d4', 'textLink-foreground': '#4daafc', 'textLink-activeForeground': '#4daafc',
            'button-background': '#0078d4', 'button-foreground': '#ffffff', 'toolbar-hoverBackground': 'rgba(90,93,94,0.31)', 'list-activeSelectionBackground': '#04395e',
            'list-activeSelectionForeground': '#ffffff', 'badge-background': '#616161', 'badge-foreground': '#f8f8f8', 'charts-green': '#89d185', 'charts-blue': '#3794ff',
            'charts-yellow': '#cca700', 'charts-red': '#f14c4c', 'editorHoverWidget-background': '#202020', 'editorHoverWidget-border': '#454545',
            'editorWidget-background': '#202020', 'menu-background': '#1f1f1f', 'menu-selectionBackground': '#0078d4', 'menu-border': '#454545',
            'input-background': '#313131', 'input-foreground': '#cccccc', 'inputOption-activeBackground': 'rgba(36,137,219,0.51)', 'inputOption-activeBorder': '#2488db',
            'errorForeground': '#f85149', 'inputValidation-errorBackground': '#5a1d1d', 'inputValidation-errorBorder': '#be1100', 'widget-shadow': 'rgba(0,0,0,0.36)',
        },
    },
    light: {
        body: 'vscode-light',
        vars: {
            foreground: '#3b3b3b', 'editor-foreground': '#3b3b3b', 'editor-background': '#ffffff', descriptionForeground: '#3b3b3b',
            'widget-border': '#e5e5e5', 'panel-border': '#e5e5e5', focusBorder: '#005fb8', 'textLink-foreground': '#005fb8', 'textLink-activeForeground': '#005fb8',
            'button-background': '#005fb8', 'button-foreground': '#ffffff', 'toolbar-hoverBackground': 'rgba(184,184,184,0.31)', 'list-activeSelectionBackground': '#e8e8e8',
            'list-activeSelectionForeground': '#000000', 'badge-background': '#cccccc', 'badge-foreground': '#3b3b3b', 'charts-green': '#388a34', 'charts-blue': '#1a85ff',
            'charts-yellow': '#bf8803', 'charts-red': '#e51400', 'editorHoverWidget-background': '#f8f8f8', 'editorHoverWidget-border': '#c8c8c8',
            'editorWidget-background': '#f8f8f8', 'menu-background': '#ffffff', 'menu-selectionBackground': '#005fb8', 'menu-selectionForeground': '#ffffff', 'menu-border': '#cecece',
            'input-background': '#ffffff', 'input-foreground': '#3b3b3b', 'inputOption-activeBackground': 'rgba(190,221,255,0.2)', 'inputOption-activeBorder': '#005fb8',
            'errorForeground': '#e51400', 'inputValidation-errorBackground': '#f2dede', 'inputValidation-errorBorder': '#be1100', 'widget-shadow': 'rgba(0,0,0,0.16)',
        },
    },
    hc: {
        body: 'vscode-dark vscode-high-contrast',
        vars: {
            foreground: '#ffffff', 'editor-foreground': '#ffffff', 'editor-background': '#000000', descriptionForeground: '#ffffff', contrastBorder: '#6fc3df',
            contrastActiveBorder: '#f38518', 'widget-border': '#6fc3df', focusBorder: '#f38518', 'textLink-foreground': '#21a6ff', 'textLink-activeForeground': '#21a6ff',
            'button-background': '#000000', 'button-foreground': '#ffffff', 'button-border': '#6fc3df', 'toolbar-hoverBackground': 'transparent', 'list-activeSelectionBackground': '#000000',
            'badge-background': '#000000', 'badge-foreground': '#ffffff', 'charts-green': '#89d185', 'charts-blue': '#3794ff', 'charts-yellow': '#ffd700', 'charts-red': '#f48771',
            'editorHoverWidget-background': '#0c141f', 'editorHoverWidget-border': '#6fc3df', 'editorWidget-background': '#0c141f', 'menu-background': '#000000',
            'menu-border': '#6fc3df', 'input-background': '#000000', 'input-foreground': '#ffffff', 'inputOption-activeBorder': '#6fc3df', 'errorForeground': '#f48771',
            'inputValidation-errorBackground': '#000000', 'inputValidation-errorBorder': '#f48771',
        },
    },
};

function page(name, theme, query) {
    const t = THEMES[theme];
    const vars = Object.entries({ ...common, ...t.vars })
        .map(([k, v]) => `--vscode-${k}: ${v};`)
        .join(' ');
    const file = `${name}-${theme}.html`;
    writeFileSync(
        join(out, file),
        `<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>${name} (${theme})</title>
<style>:root { ${vars} }</style>
</head>
<body class="${t.body}">
<main id="root"><p class="hint">Loading…</p></main>
<script src="host.js"></script>
<script src="circuit.js"></script>
</body>
</html>`,
    );
    return `${pathToFileURL(join(out, file)).href}?${new URLSearchParams(query)}`;
}

const SCENES = [
    { name: 'votecheck', chip: 'VoteCheck', set: 'a=1&b=1&d=1&word=1234&pick=1' },
    { name: 'accumulator', chip: 'Accumulator', set: 'in=5&add=1', steps: 3 },
    { name: 'setreset', chip: 'SetReset', set: 'set=1' },
    { name: 'parity4', chip: 'Parity4', set: 'in=11' },
    { name: 'swapbytes', chip: 'SwapBytes', set: 'in=4660', radix: 'hex' },
    { name: 'toggle', chip: 'Toggle', set: 'enable=1', steps: 1 },
    { name: 'wide60', chip: 'Wide60', set: 'x=5&s=1' },
    { name: 'majority-drilled', chip: 'VoteCheck', set: 'a=1&b=1&d=1&word=1234&pick=1', path: '0' },
];

const links = [];
for (const scene of SCENES) {
    for (const theme of Object.keys(THEMES)) {
        const query = { chip: scene.chip, set: scene.set ?? '', steps: String(scene.steps ?? 0), path: scene.path ?? '', radix: scene.radix ?? 'dec' };
        links.push({ scene, theme, url: page(scene.name, theme, query) });
    }
}
writeFileSync(
    join(out, 'index.html'),
    `<!DOCTYPE html><meta charset="UTF-8"><title>Circuit preview</title><body style="font: 14px system-ui; padding: 20px"><h1>Circuit diagram preview</h1><ul>${links
        .map((l) => `<li><a href="${l.url}">${l.scene.name} — ${l.theme}</a></li>`)
        .join('')}</ul></body>`,
);
console.log(`Preview pages: ${join(out, 'index.html')}`);

if (args.includes('--no-shots')) {
    process.exit(0);
}

function findChromium() {
    try {
        const path = chromium.executablePath();
        if (path && existsSync(path)) {
            return path;
        }
    } catch {
        // not installed where playwright-core expects it
    }
    const cache = join(homedir(), '.cache', 'ms-playwright');
    if (existsSync(cache)) {
        for (const dir of readdirSync(cache).filter((d) => d.startsWith('chromium-')).sort().reverse()) {
            for (const rel of ['chrome-linux64/chrome', 'chrome-linux/chrome', 'chrome-mac/Chromium.app/Contents/MacOS/Chromium', 'chrome-win/chrome.exe']) {
                if (existsSync(join(cache, dir, rel))) {
                    return join(cache, dir, rel);
                }
            }
        }
    }
    for (const path of ['/usr/bin/chromium', '/usr/bin/chromium-browser', '/usr/bin/google-chrome']) {
        if (existsSync(path)) {
            return path;
        }
    }
    throw new Error('No Chromium found: install one with `npx playwright-core install chromium`.');
}

const shots = join(out, 'shots');
mkdirSync(shots, { recursive: true });
const browser = await chromium.launch({ executablePath: findChromium() });
const context = await browser.newContext({ viewport: { width: 1180, height: 720 }, deviceScaleFactor: 2 });
const errors = [];
const taken = [];

async function open(url) {
    const p = await context.newPage();
    p.on('console', (m) => m.type() === 'error' && errors.push(`${url}: ${m.text()}`));
    p.on('pageerror', (e) => errors.push(`${url}: ${e.message}`));
    await p.goto(url);
    await p.waitForSelector('.c-svg .part, .c-svg .port');
    await p.waitForTimeout(150);
    return p;
}

async function shot(p, name) {
    const file = join(shots, `${name}.png`);
    await p.screenshot({ path: file });
    taken.push(file);
}

for (const link of links) {
    if (only && !link.scene.name.includes(only)) {
        continue;
    }
    const p = await open(link.url);
    await shot(p, `${link.scene.name}-${link.theme}`);
    await p.close();
}

if (!only) {
    // Interaction: hover a wire, flip an input mid-animation, the error overlay, export.
    const url = links.find((l) => l.scene.name === 'votecheck' && l.theme === 'dark').url;
    const p = await open(url);
    const hit = await p.$('.hit[data-net="9"]');
    if (hit) {
        const box = await hit.boundingBox();
        await p.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
        await p.waitForTimeout(100);
        await shot(p, 'interact-hover-wire');
    }
    const part = await p.$('.part[data-part="0"]');
    const pb = await part.boundingBox();
    await p.mouse.move(pb.x + pb.width / 2, pb.y + pb.height / 2);
    await p.waitForTimeout(80);
    await shot(p, 'interact-hover-part');
    await p.mouse.move(5, 300);
    const port = await p.$('.port[data-port="in:2"]');
    await port.click();
    await p.waitForTimeout(140);
    await shot(p, 'interact-animation-140ms');
    await p.waitForTimeout(260);
    await shot(p, 'interact-animation-400ms');
    await p.waitForTimeout(800);
    const word = await p.$('.port[data-port="in:4"]');
    await word.click();
    await p.waitForTimeout(80);
    await p.keyboard.type('0x00FF');
    await shot(p, 'interact-edit-bus');
    await p.keyboard.press('Enter');
    await p.waitForTimeout(1200);
    await p.evaluate(() => window.preview.post({ type: 'highlight', part: 5 }));
    await p.waitForTimeout(50);
    await shot(p, 'interact-highlight-part');
    await p.evaluate(() =>
        window.preview.post({ type: 'error', message: "VoteCheck.hdl:12: Mux4Way16 has no pin named 'sell'" }),
    );
    await p.waitForTimeout(100);
    await shot(p, 'interact-error');
    await p.evaluate(() => window.preview.post({ type: 'diagram', diagram: window.preview.session.diagram([]), settings: { animate: true, radix: 'dec' } }));
    await p.waitForTimeout(100);
    // Export: the SVG must stand alone (no CSS variables, no classes).
    await p.click('button[aria-label^="Export"]');
    await p.click('text=SVG (vector)…');
    await p.waitForTimeout(200);
    const svg = await p.evaluate(() => [...window.preview.log].reverse().find((m) => m.type === 'export' && m.format === 'svg')?.data);
    if (!svg) {
        errors.push('no SVG exported');
    } else {
        if (/var\(|class=|color-mix|color\(/.test(svg)) {
            errors.push('the exported SVG still depends on CSS');
        }
        writeFileSync(join(shots, 'export.svg'), svg);
        taken.push(join(shots, 'export.svg'));
    }
    await p.click('button[aria-label^="Export"]');
    await p.click('text=PNG (2× image)…');
    await p.waitForTimeout(800);
    const png = await p.evaluate(() => [...window.preview.log].reverse().find((m) => m.type === 'export' && m.format === 'png')?.data);
    if (!png) {
        errors.push('no PNG exported');
    } else {
        writeFileSync(join(shots, 'export.png'), Buffer.from(png, 'base64'));
        taken.push(join(shots, 'export.png'));
    }
    // Drill by double-click, then back up by the breadcrumb.
    const majority = await p.$('.part[data-part="0"]');
    await majority.dblclick();
    await p.waitForTimeout(300);
    await shot(p, 'interact-drilled');
    await p.close();

    const acc = links.find((l) => l.scene.name === 'accumulator' && l.theme === 'dark').url;
    const q = await open(acc);
    await q.click('button[aria-label^="Step"]');
    await q.waitForTimeout(160);
    await shot(q, 'interact-clock-step-160ms');
    await q.close();
}

await browser.close();
console.log(taken.map((f) => `  ${f}`).join('\n'));
if (errors.length) {
    console.error(`Problems:\n${errors.map((e) => `  ${e}`).join('\n')}`);
    process.exit(1);
}
