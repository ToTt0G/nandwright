// The Hack computer panel (runs inside a VS Code webview): the screen, the
// keyboard, the registers and the memory of a running program. The extension
// owns the emulator; this file draws what it is sent and reports what the
// student does.

import { memoryStartForAddress } from '../inspectRow';
import type { ChipTest, Expected, ExpectedImage, InspectedMismatch, InspectedRow, MachineState, ScriptChoice } from '../runtime/protocol';
import { Radix, cellMatchesExpected, formatValue, hackKeyCode, keyName, typedValue } from '../values';

declare function acquireVsCodeApi(): {
    postMessage(message: unknown): void;
    getState(): any;
    setState(state: any): void;
};

export type ToComputer =
    | {
          type: 'state';
          target: number;
          state: MachineState | null;
          running: boolean;
          problem: string | null;
          title: string;
          expected?: Expected | null;
          expectedVersion?: number;
          expectedImage?: ExpectedImage | null;
          test?: ChipTest | null;
          /** Every script that tests the program; a picker shows when there are several. */
          scripts?: ScriptChoice[];
          /** The compare row the machine was jumped to, if any. */
          inspected?: InspectedRow | null;
      }
    | { type: 'rejected'; target: number; message: string };

type Ready = { type: 'ready'; memoryStart?: number; speed?: number };
type ComputerAction =
    | { type: 'run' }
    | { type: 'pause' }
    | { type: 'step'; unit?: 'command' | 'line' }
    | { type: 'reset' }
    | { type: 'speed'; speed: number }
    | { type: 'key'; code: number }
    | { type: 'memory'; start: number }
    | { type: 'set'; target: string; address?: number; value: string }
    | { type: 'runTest'; script?: string }
    | { type: 'openExpected' }
    | { type: 'openExpectedImage' }
    | { type: 'compareOutput'; script?: string }
    | { type: 'selectScript'; script: string }
    | { type: 'inspectRow'; script?: string; line: number; rowIndex: number; expectedVersion?: number }
    | { type: 'openScriptAt'; script: string; line: number }
    | { type: 'debugScript'; script: string };

export type ComputerCommand = Ready | ComputerAction;
export type FromComputer = Ready | (ComputerAction & { program: number });

const vscode = acquireVsCodeApi();
let target: number | undefined;
function send(message: ComputerAction): void {
    if (target !== undefined) {
        vscode.postMessage({ ...message, program: target });
    }
}

const WIDTH = 512;
const HEIGHT = 256;
const ROWS = 16;

const saved = vscode.getState() ?? {};
let radix: Radix = saved.radix ?? 'dec';
let speed: number = saved.speed ?? 0;
let memoryStart: number = saved.memoryStart ?? 0;
let state: MachineState | null = null;
let running = false;
let problem: string | null = null;
let expected: Expected | null = null;
let expectedVersion: number | undefined;
let expectedImage: ExpectedImage | null = null;
let test: ChipTest | null = null;
let scripts: ScriptChoice[] = [];
let inspected: InspectedRow | null = null;
let expectedDirty = true;
let inspectDirty = true;
let imageDirty = true;
let heldKey = 0;

function remember(): void {
    vscode.setState({ radix, speed, memoryStart });
}

function element<K extends keyof HTMLElementTagNameMap>(
    tag: K,
    attributes: Record<string, string> = {},
    ...children: (Node | string)[]
): HTMLElementTagNameMap[K] {
    const node = document.createElement(tag);
    for (const [name, value] of Object.entries(attributes)) {
        if (name === 'class') {
            node.className = value;
        } else {
            node.setAttribute(name, value);
        }
    }
    node.append(...children);
    return node;
}

// ---- The fixed skeleton: built once, then updated in place, so that focus
// (the keyboard!) and the canvas survive every frame. ----

const root = document.getElementById('root')!;
const title = element('h1', {}, '');
const status = element('p', { class: 'status', role: 'status', 'aria-live': 'polite' });
const banner = element('div', { class: 'problem', role: 'alert', hidden: '' });
const inspectBanner = element('div', { class: 'inspect-banner', role: 'status', 'aria-live': 'polite', 'aria-label': 'Inspected test row', hidden: '' });

const runButton = element('button', { type: 'button', class: 'primary' }, 'Run');
const stepButton = element('button', { type: 'button', class: 'secondary', title: 'Execute one instruction' }, 'Step');
const stepLineButton = element(
    'button',
    { type: 'button', class: 'secondary', title: 'Run until the next line of the Jack program (calls are stepped over)', hidden: '' },
    'Step line',
);
const resetButton = element(
    'button',
    { type: 'button', class: 'secondary', title: 'Load the program again and clear memory' },
    'Reset',
);
const speedSelect = element('select', { 'aria-label': 'Speed', title: 'How fast the program runs' });
for (const [value, label] of [
    [0, 'Full speed'],
    [20000, 'Fast (20,000 steps/s)'],
    [200, 'Medium (200 steps/s)'],
    [5, 'Slow (5 steps/s)'],
] as const) {
    const option = element('option', { value: String(value) }, label);
    option.selected = value === speed;
    speedSelect.append(option);
}

const canvas = element('canvas', { width: String(WIDTH), height: String(HEIGHT) });
const screen = element(
    'div',
    {
        class: 'screen',
        tabindex: '0',
        role: 'application',
        'aria-label': 'Hack screen. While it has focus, the keys you press go to the Hack keyboard.',
    },
    canvas,
);
const keyboardHint = element('p', { class: 'hint keyboard' });
const expectedScreenSection = element('section', { hidden: '' });
const expectedSection = element('section', { hidden: '' });
const expectedHeader = element('div', {});
const expectedScroll = element('div', { class: 'expected-scroll', tabindex: '0', role: 'region', 'aria-label': 'Expected output', hidden: '' });
const expectedTable = element('table', { class: 'expected' });
const expectedMore = element('p', { class: 'hint', hidden: '' });
expectedScroll.append(expectedTable);
expectedSection.append(element('h2', {}, 'Expected output'), expectedHeader, expectedScroll, expectedMore);
let mountedExpectedPath: string | null = null;

const registers = element('div', { class: 'registers', role: 'table', 'aria-label': 'Registers' });
const badge = element('span', { class: 'badge' }, '');
const framesTitle = element('h2', { hidden: '' }, 'Call stack');
const frames = element('ol', { class: 'frames', hidden: '' });
const stackTitle = element('h2', { hidden: '' }, 'Stack (top first)');
const stack = element('ol', { class: 'stack', hidden: '' });
const memoryBody = element('tbody', {});
const addressInput = element('input', {
    class: 'address',
    type: 'text',
    spellcheck: 'false',
    autocomplete: 'off',
    'aria-label': 'Show memory from this address',
});
const rejected = element('p', { class: 'rejected', 'aria-live': 'polite' });

function radixPicker(): HTMLElement {
    const group = element('div', { class: 'segmented', role: 'radiogroup', 'aria-label': 'Number format' });
    const options: [Radix, string][] = [
        ['dec', 'Decimal'],
        ['bin', 'Binary'],
        ['hex', 'Hex'],
    ];
    for (const [value, label] of options) {
        const button = element('button', { type: 'button', role: 'radio', 'data-radix': value }, label);
        button.addEventListener('click', () => {
            radix = value;
            remember();
            update();
        });
        group.append(button);
    }
    return group;
}
const picker = radixPicker();

function jump(label: string, address: number, hint: string): HTMLElement {
    const button = element('button', { type: 'button', class: 'link', title: hint }, label);
    button.addEventListener('click', () => showMemory(address));
    return button;
}

/** The RAM cells SP, LCL, ARG, THIS and THAT live in. */
const POINTER_ADDRESSES: Record<string, number> = { SP: 0, LCL: 1, ARG: 2, THIS: 3, THAT: 4 };

/** The mismatches of the inspected row while the machine is still at that row. */
function liveMismatches(): InspectedMismatch[] {
    return inspected && !inspected.stepped ? inspected.mismatches : [];
}

function inspectRow(index: number): void {
    if (!expected || expected.lines[index] === undefined) {
        return;
    }
    send({
        type: 'inspectRow',
        ...(test?.scriptPath ? { script: test.scriptPath } : {}),
        line: expected.lines[index],
        rowIndex: index,
        ...(expectedVersion !== undefined ? { expectedVersion } : {}),
    });
}

function updateInspection(): void {
    if (!inspectDirty) {
        return;
    }
    inspectDirty = false;
    inspectBanner.hidden = !inspected;
    if (!inspected) {
        inspectBanner.replaceChildren();
        return;
    }
    const row = inspected;
    const heading = row.stepped
        ? `Stepped from ${row.script} · row ${row.rowIndex + 1} (line ${row.compareLine})`
        : `Inspecting ${row.script} · row ${row.rowIndex + 1} (line ${row.compareLine})`;
    const nodes: Node[] = [element('strong', { class: 'inspect-title' }, heading)];
    if (row.stepped) {
        nodes.push(element('p', { class: 'hint' }, 'The machine has moved on since that row: Reset to start over, or inspect a row again.'));
    } else if (row.mismatches.length === 0 && row.differs) {
        nodes.push(element('p', { class: 'hint' }, 'This row differs (line format or column count does not match).'));
    } else if (row.mismatches.length === 0) {
        nodes.push(element('p', { class: 'hint' }, 'This row matches. The machine is shown as the test left it at this row.'));
    } else {
        const chips = element('ul', { class: 'mismatches', 'aria-label': 'Values that differ on this row' });
        for (const mismatch of row.mismatches) {
            const text = `${mismatch.column}: expected ${mismatch.expected}, got ${mismatch.actual}`;
            const address = mismatch.target === 'RAM' ? mismatch.address : undefined;
            if (address !== undefined) {
                const chip = element('button', { type: 'button', class: 'chip', title: `Show RAM[${address}] in Memory` }, text);
                chip.addEventListener('click', () => showMemory(memoryStartForAddress(address, ROWS)));
                chips.append(element('li', {}, chip));
            } else {
                chips.append(element('li', {}, element('span', { class: 'chip' }, text)));
            }
        }
        nodes.push(chips);
    }
    const actions = element('div', { class: 'toolbar' });
    if (row.scriptLine !== null) {
        const open = element(
            'button',
            { type: 'button', class: 'secondary', title: 'Show where the script stopped' },
            `Open ${row.script}:${row.scriptLine}`,
        );
        const line = row.scriptLine;
        open.addEventListener('click', () => send({ type: 'openScriptAt', script: row.scriptPath, line }));
        actions.append(open);
    }
    const debug = element('button', { type: 'button', class: 'secondary', title: `Step through ${row.script} in the debugger` }, 'Debug script');
    debug.addEventListener('click', () => send({ type: 'debugScript', script: row.scriptPath }));
    const reset = element('button', { type: 'button', class: 'secondary', title: 'Leave the inspection and load the program again' }, 'Reset');
    reset.addEventListener('click', () => send({ type: 'reset' }));
    actions.append(debug, reset);
    nodes.push(actions);
    inspectBanner.replaceChildren(...nodes);
}

function showMemory(address: number): void {
    memoryStart = Math.max(0, Math.min(24576 - ROWS + 1, address));
    remember();
    send({ type: 'memory', start: memoryStart });
}

function build(): void {
    const header = element('header', {}, title, badge);
    const toolbar = element(
        'div',
        { class: 'toolbar', role: 'toolbar', 'aria-label': 'Program' },
        runButton,
        stepLineButton,
        stepButton,
        resetButton,
        speedSelect,
    );
    const memoryTools = element(
        'div',
        { class: 'toolbar memory-tools' },
        element('label', { class: 'address-label' }, 'From address ', addressInput),
        jump('R0', 0, 'RAM[0], where R0 to R15 and SP, LCL, ARG, THIS, THAT live'),
        jump('Variables', 16, 'RAM[16], where the assembler puts variables'),
        jump('Stack', 256, 'RAM[256], where the VM stack starts'),
        jump('Screen', 16384, 'RAM[16384], the first word of the screen'),
        jump('Keyboard', 24576 - ROWS + 1, 'RAM[24576], the keyboard'),
        element('span', { class: 'spacer' }),
        picker,
    );
    const memory = element(
        'table',
        { class: 'memory' },
        element(
            'thead',
            {},
            element('tr', {}, element('th', { scope: 'col' }, 'Address'), element('th', { scope: 'col' }, 'Value')),
        ),
        memoryBody,
    );
    root.replaceChildren(
        header,
        banner,
        toolbar,
        status,
        inspectBanner,
        screen,
        keyboardHint,
        expectedScreenSection,
        expectedSection,
        framesTitle,
        frames,
        element('h2', {}, 'Registers'),
        registers,
        stackTitle,
        stack,
        element('h2', {}, 'Memory'),
        memoryTools,
        rejected,
        memory,
    );
}

// ---- Screen ----

const context = canvas.getContext('2d')!;
const image = context.createImageData(WIDTH, HEIGHT);
const pixels = new Uint32Array(image.data.buffer);
const WHITE = 0xffffffff;
const BLACK = 0xff000000;
pixels.fill(WHITE);
context.putImageData(image, 0, 0);

/** Draws the 8192 screen words (four hex digits each; bit 0 is the leftmost pixel). */
function draw(hex: string): void {
    for (let word = 0; word < 8192; word++) {
        const value = parseInt(hex.substr(word * 4, 4), 16);
        const base = word * 16;
        for (let bit = 0; bit < 16; bit++) {
            pixels[base + bit] = (value >> bit) & 1 ? BLACK : WHITE;
        }
    }
    context.putImageData(image, 0, 0);
}

// ---- Keyboard ----

function press(code: number): void {
    if (code !== heldKey) {
        heldKey = code;
        send({ type: 'key', code });
        updateKeyboard();
    }
}

screen.addEventListener('keydown', (event) => {
    if (event.key === 'Tab') {
        return; // Tab leaves the screen, so the panel stays usable from the keyboard.
    }
    const code = hackKeyCode(event);
    if (code !== undefined) {
        event.preventDefault();
        press(code);
    }
});
screen.addEventListener('keyup', () => press(0));
screen.addEventListener('blur', () => {
    press(0);
    updateKeyboard();
});
screen.addEventListener('focus', updateKeyboard);

function updateKeyboard(): void {
    const focused = document.activeElement === screen;
    keyboardHint.textContent = !focused
        ? 'Click the screen to type on the Hack keyboard.'
        : heldKey
          ? `Keyboard: ${keyName(heldKey)} is down (code ${heldKey}).`
          : 'Keyboard is active: the keys you press go to the program. Tab leaves.';
}

// ---- Controls ----

runButton.addEventListener('click', () => send({ type: running ? 'pause' : 'run' }));
stepButton.addEventListener('click', () => send({ type: 'step', unit: 'command' }));
stepLineButton.addEventListener('click', () => send({ type: 'step', unit: 'line' }));
resetButton.addEventListener('click', () => send({ type: 'reset' }));
speedSelect.addEventListener('change', () => {
    speed = Number(speedSelect.value);
    remember();
    send({ type: 'speed', speed });
});
addressInput.addEventListener('keydown', (event) => {
    if (event.key === 'Enter') {
        const address = Number(addressInput.value.trim());
        if (Number.isInteger(address) && address >= 0 && address <= 24576) {
            showMemory(address);
        } else {
            rejected.textContent = 'An address is a number from 0 to 24576.';
        }
    }
});

function cellInput(label: string, value: number, onCommit: (text: string) => void): HTMLInputElement {
    const input = element('input', { class: 'cell', type: 'text', spellcheck: 'false', autocomplete: 'off', 'aria-label': label });
    const shown = formatValue(value, 16, radix);
    const program = target;
    input.value = shown;
    input.disabled = running;
    const commit = () => {
        const text = input.value.trim();
        if (program === target && text !== shown) {
            onCommit(typedValue(text, radix));
        }
    };
    input.addEventListener('keydown', (event) => {
        if (event.key === 'Enter') {
            commit();
        } else if (event.key === 'Escape') {
            input.value = shown;
        }
    });
    input.addEventListener('blur', commit);
    return input;
}

const REGISTER_HINTS: Record<string, string> = {
    A: 'The address register',
    D: 'The data register',
    PC: 'The program counter: the address of the next instruction',
    M: 'RAM[A]: the memory cell A points at',
    SP: 'The stack pointer, RAM[0]: the address just above the top of the stack',
    LCL: 'RAM[1]: where the local variables of the current function start',
    ARG: 'RAM[2]: where the arguments of the current function start',
    THIS: 'RAM[3]: the base of the this segment (the current object)',
    THAT: 'RAM[4]: the base of the that segment (the current array)',
};

function fileName(file: string | null): string {
    return file ? file.split(/[\\/]/).pop()! : '';
}

const MARKS = { match: '✓', differ: '✗', unreached: '–' } as const;

function testSummary(run: ChipTest): { text: string; fails: boolean } {
    const compared = run.comparedRows ?? Math.max(0, (run.comparedLines ?? 0) - 1);
    const differing = run.differingRows ?? run.failureCount ?? 0;
    const tag = run.sourceLabel ? `${run.script} (${run.sourceLabel})` : run.script;
    switch (run.status) {
        case 'none':
            return { text: 'Not run yet.', fails: false };
        case 'running':
            return { text: `Running ${run.script}…`, fails: false };
        case 'stale':
            return { text: 'Edited since the last run. Save or press Run test to check again.', fails: false };
    }
    switch (run.kind) {
        case 'passed':
            return { text: `${tag} passes: all ${compared} rows match.`, fails: false };
        case 'failed':
            if (differing === 0 && run.firstFailureReason) {
                return { text: `${tag} fails: ${run.firstFailureReason}.`, fails: true };
            }
            return { text: `${tag} fails: ${differing} of ${compared} rows differ.`, fails: true };
        case 'error':
            return { text: `${tag} stopped with an error: ${run.message ?? ''}`, fails: true };
        default:
            return { text: `${tag} did not give a result: ${run.message ?? ''}`, fails: false };
    }
}

function updateExpected(): void {
    if (imageDirty) {
        imageDirty = false;
        expectedScreenSection.hidden = !expectedImage;
        if (expectedImage) {
            const open = element('button', { type: 'button', class: 'link-inline' }, expectedImage.name);
            open.addEventListener('click', () => send({ type: 'openExpectedImage' }));
            const hint = element('p', { class: 'hint' }, 'From ');
            hint.append(open, ': compare the Hack screen above with this expected image.');
            const frame = element(
                'div',
                { class: 'expected-screen' },
                element('img', { src: expectedImage.dataUrl, alt: `Expected screen output from ${expectedImage.name}` }),
            );
            expectedScreenSection.replaceChildren(element('h2', {}, 'Expected screen'), hint, frame);
        } else {
            expectedScreenSection.replaceChildren();
        }
    }

    if (!expectedDirty) {
        return;
    }
    expectedDirty = false;
    if (!expected && !test) {
        expectedSection.hidden = true;
        expectedHeader.replaceChildren();
        expectedTable.replaceChildren();
        expectedScroll.hidden = true;
        expectedMore.hidden = true;
        mountedExpectedPath = null;
        return;
    }
    expectedSection.hidden = false;

    // Remember scroll position and button focus inside the expected section.
    const sameFile = expected !== null && expected.path === mountedExpectedPath;
    const savedScrollTop = sameFile ? expectedScroll.scrollTop : 0;
    const savedScrollLeft = sameFile ? expectedScroll.scrollLeft : 0;
    const activeAction = (document.activeElement as HTMLElement | null)?.getAttribute('data-expected-action');
    mountedExpectedPath = expected?.path ?? null;

    const activeRow = (document.activeElement as HTMLElement | null)?.getAttribute('data-row');
    const headerNodes: Node[] = [];
    if (scripts.length > 1) {
        const picker = element('select', { 'data-expected-action': 'script', 'aria-label': 'Test script', title: 'Which test script to show' });
        const shown = test?.scriptPath ?? inspected?.scriptPath;
        for (const choice of scripts) {
            const option = element('option', { value: choice.path }, choice.name);
            option.selected = choice.path === shown;
            picker.append(option);
        }
        if (shown) {
            picker.value = shown;
        }
        picker.addEventListener('change', () => send({ type: 'selectScript', script: picker.value }));
        headerNodes.push(element('label', { class: 'script-picker' }, 'Test script ', picker));
    }
    if (expected) {
        const open = element('button', { type: 'button', class: 'link-inline', 'data-expected-action': 'open' }, expected.name);
        open.addEventListener('click', () => send({ type: 'openExpected' }));
        const hint = element('p', { class: 'hint' }, 'From ');
        hint.append(open, ': what the test script compares your program with.');
        headerNodes.push(hint);
    }
    if (test) {
        const scriptPath = test.scriptPath;
        const bar = element('div', { class: 'test-run' });
        const { text, fails } = testSummary(test);
        bar.append(element('span', { class: `summary${fails ? ' fails' : ''}`, role: 'status' }, text));
        if (expected && test.status === 'current' && (test.kind === 'passed' || test.kind === 'failed')) {
            const diffButton = element(
                'button',
                { type: 'button', class: 'secondary', 'data-expected-action': 'diff', title: 'Compare actual .out with expected .cmp' },
                'Diff .out ↔ .cmp',
            );
            diffButton.addEventListener('click', () => send({ type: 'compareOutput', ...(scriptPath ? { script: scriptPath } : {}) }));
            bar.append(diffButton);
        }
        const firstDiffer = test.status === 'current' && test.kind === 'failed' && test.rows ? test.rows.indexOf('differ') : -1;
        if (expected && firstDiffer >= 0 && expected.lines[firstDiffer] !== undefined) {
            const inspectButton = element(
                'button',
                {
                    type: 'button',
                    class: 'secondary',
                    'data-expected-action': 'inspectFirst',
                    title: `Show the machine as it was at the first row that differs (line ${expected.lines[firstDiffer]})`,
                },
                'Inspect failure',
            );
            inspectButton.addEventListener('click', () => inspectRow(firstDiffer));
            bar.append(inspectButton);
        }
        const button = element(
            'button',
            { type: 'button', class: 'secondary', 'data-expected-action': 'run', title: `Run ${test.script}` },
            'Run test',
        );
        if (test.status === 'running') {
            button.setAttribute('disabled', '');
        }
        button.addEventListener('click', () => send({ type: 'runTest', ...(scriptPath ? { script: scriptPath } : {}) }));
        bar.append(button);
        headerNodes.push(bar);
    }
    expectedHeader.replaceChildren(...headerNodes);

    if (expected) {
        const marks = test?.status === 'current' && test.rows ? test.rows : null;
        const actualRows = (marks && test?.actual) || {};
        const head = element('tr', {});
        if (marks) {
            head.append(element('th', { scope: 'col', class: 'mark', 'aria-label': 'Last test run' }));
        }
        expected.header.forEach((cell) => head.append(element('th', { scope: 'col' }, cell)));
        const body = element('tbody', {});
        for (const [index, row] of expected.rows.entries()) {
            const mark = marks?.[index];
            const reached = mark === 'match' || mark === 'differ';
            const tr = element('tr', {});
            const current =
                reached
                && inspected !== null
                && inspected.compareLine === expected.lines[index]
                && inspected.scriptPath === test?.scriptPath;
            if (reached) {
                tr.className = current ? 'inspectable inspected' : 'inspectable';
                tr.setAttribute('data-row', String(index));
                tr.setAttribute('title', `Click to see the machine at this row (line ${expected.lines[index]})`);
                if (current) {
                    tr.setAttribute('aria-current', 'true');
                }
                tr.addEventListener('click', () => inspectRow(index));
            }
            const produced = mark === 'differ' ? actualRows[index] : undefined;
            const rawExpectedRow = expected.rawRows?.[index];
            let blamedCell = false;
            const dataCells: HTMLElement[] = [];
            row.forEach((cell, i) => {
                const rawActual = produced?.[i];
                const rawExpected = rawExpectedRow?.[i];
                const cellDiffers =
                    rawActual !== undefined
                    && (rawExpected !== undefined
                        ? !cellMatchesExpected(rawExpected, rawActual)
                        : !/^\*+$/.test(cell) && rawActual.trim() !== cell);
                if (cellDiffers && rawActual !== undefined) {
                    blamedCell = true;
                    const gave = rawActual.trim();
                    const detail = gave === cell ? 'spacing differs' : `got ${gave}`;
                    const text = `Expected ${cell}, your program gave ${gave} (${detail})`;
                    const td = element('td', { class: 'differs', title: text, 'aria-label': text }, `${cell} ✗`);
                    td.append(element('span', { class: 'got' }, detail));
                    dataCells.push(td);
                } else {
                    dataCells.push(element('td', {}, cell));
                }
            });
            if (marks) {
                const lineNote =
                    mark === 'differ' && !blamedCell
                        ? 'This row differs (line format or column count does not match).'
                        : mark === 'differ'
                          ? `This row differs. Your program gave: ${produced ? `|${produced.join('|')}|` : ''}`
                          : mark === 'match'
                            ? 'This row matches'
                            : 'The test did not reach this row';
                if (reached) {
                    const label = `Inspect row ${index + 1} (${mark === 'differ' ? 'differs' : 'matches'}, line ${expected.lines[index]})`;
                    const button = element(
                        'button',
                        { type: 'button', class: 'mark-button', 'data-inspect-row': String(index), title: lineNote, 'aria-label': label },
                        MARKS[mark],
                    );
                    button.addEventListener('click', (event: MouseEvent) => {
                        event.stopPropagation();
                        inspectRow(index);
                    });
                    tr.append(element('td', { class: `mark ${mark}`, title: lineNote }, button));
                } else {
                    tr.append(element('td', { class: `mark ${mark ?? 'unreached'}`, title: lineNote, 'aria-label': lineNote }, MARKS[mark ?? 'unreached']));
                }
            }
            tr.append(...dataCells);
            body.append(tr);
        }
        expectedTable.replaceChildren(element('thead', {}, head), body);
        expectedScroll.hidden = false;
        expectedScroll.scrollTop = savedScrollTop;
        expectedScroll.scrollLeft = savedScrollLeft;
        if (expected.more > 0) {
            expectedMore.hidden = false;
            expectedMore.textContent = `${expected.more} more rows are in ${expected.name}.`;
        } else {
            expectedMore.hidden = true;
            expectedMore.textContent = '';
        }
    } else {
        expectedTable.replaceChildren();
        expectedScroll.hidden = true;
        expectedMore.hidden = true;
        expectedMore.textContent = '';
    }

    if (activeAction) {
        const refocus = expectedHeader.querySelector(`[data-expected-action="${activeAction}"]`) as HTMLElement | null;
        refocus?.focus();
    } else if (activeRow) {
        const refocus = expectedTable.querySelector(`tr[data-row="${activeRow}"]`) as HTMLElement | null;
        refocus?.focus();
    }
}

function update(): void {
    updateExpected();
    updateInspection();
    banner.hidden = !problem;
    banner.replaceChildren(element('strong', {}, 'The program stopped.'), element('pre', {}, problem ?? ''));
    picker.querySelectorAll('button').forEach((button) => {
        const selected = button.getAttribute('data-radix') === radix;
        button.classList.toggle('selected', selected);
        button.setAttribute('aria-checked', String(selected));
    });
    runButton.textContent = running ? 'Pause' : 'Run';
    runButton.title = running ? 'Pause the program' : 'Run the program';
    stepButton.disabled = running;
    if (!state) {
        badge.textContent = '';
        runButton.disabled = stepButton.disabled = true;
        stepLineButton.hidden = true;
        framesTitle.hidden = frames.hidden = stackTitle.hidden = stack.hidden = true;
        registers.replaceChildren();
        memoryBody.replaceChildren();
        status.textContent = problem ? '' : 'Loading the program…';
        return;
    }
    const vm = state.kind === 'vm';
    badge.textContent = vm ? 'VM emulator' : 'Hack computer';
    stepLineButton.hidden = !state.fromJack;
    stepButton.title = vm ? 'Execute one VM command' : 'Execute one instruction';
    const pc = state.registers.find((r) => r.name === 'PC')?.value ?? 0;
    const where =
        state.line !== null
            ? `${vm && state.file ? `${fileName(state.file)} ` : ''}line ${state.line + 1}`
            : vm
              ? (state.function ?? 'the program')
              : `ROM[${pc}]`;
    const steps = `${state.steps.toLocaleString('en-US')} steps`;
    const finished = state.status === 'error' || state.status === 'halted';
    status.textContent = running
        ? state.status === 'waiting'
            ? `Waiting for a key · click the screen and type · ${steps}`
            : state.status === 'sleeping'
              ? `Running (in Sys.wait) · ${steps}`
              : `Running · ${steps}`
        : state.status === 'error'
          ? `Stopped at ${where}`
          : state.status === 'halted'
            ? `The program ended · ${steps}`
            : `${state.status === 'breakpoint' ? 'Breakpoint' : 'Paused'} at ${where} · next: ${state.instruction} · ${steps}`;
    runButton.disabled = finished;
    stepButton.disabled = running || finished;
    stepLineButton.disabled = running || finished;

    const calls = state.callStack ?? [];
    framesTitle.hidden = frames.hidden = !vm || calls.length === 0;
    frames.replaceChildren(
        ...calls.map((frame) =>
            element(
                'li',
                {},
                frame.function,
                element('span', { class: 'where' }, frame.line !== null ? `${fileName(frame.file)}:${frame.line + 1}` : 'built-in'),
            ),
        ),
    );
    const values = state.stack ?? [];
    stackTitle.hidden = stack.hidden = !vm;
    stack.replaceChildren(
        ...(values.length > 0
            ? values.map((value) => element('li', {}, formatValue(value, 16, radix)))
            : [element('li', { class: 'muted' }, 'empty')]),
    );

    const focusedLabel = (document.activeElement as HTMLElement | null)?.getAttribute('aria-label');
    const differing = liveMismatches();
    registers.replaceChildren();
    for (const register of state.registers) {
        const mismatch = differing.find((m) => m.target === 'register' && m.name === register.name);
        const row = element('div', {
            class: mismatch ? 'register differs' : 'register',
            role: 'row',
            title: mismatch
                ? `${register.name}: the test expected ${mismatch.expected}, the program had ${mismatch.actual}`
                : (REGISTER_HINTS[register.name] ?? ''),
        });
        row.append(element('span', { class: 'register-name', role: 'rowheader' }, register.name));
        if (register.value === null) {
            row.append(element('span', { class: 'value muted', role: 'cell' }, 'not a RAM address'));
        } else if (register.name === 'M') {
            row.append(element('span', { class: 'value', role: 'cell' }, formatValue(register.value, 16, radix)));
        } else {
            row.append(
                cellInput(register.name, register.value, (text) => send({ type: 'set', target: register.name, value: text })),
            );
        }
        if (mismatch) {
            row.append(element('span', { class: 'got' }, `expected ${mismatch.expected}`));
        }
        registers.append(row);
    }

    if (state.ram) {
        addressInput.value = String(state.ram.start);
        memoryBody.replaceChildren();
        const a = state.registers.find((r) => r.name === 'A')?.value;
        state.ram.values.forEach((value, i) => {
            const address = state!.ram!.start + i;
            const name = symbolFor(address);
            const mismatch = differing.find(
                (m) =>
                    (m.target === 'RAM' && m.address === address)
                    || (m.target === 'register' && m.name !== undefined && POINTER_ADDRESSES[m.name] === address),
            );
            const classes = [address === a ? 'pointed' : '', mismatch ? 'differs' : ''].filter(Boolean).join(' ');
            const row = element('tr', { class: classes });
            if (mismatch) {
                row.setAttribute('title', `${mismatch.column}: the test expected ${mismatch.expected}, the program had ${mismatch.actual}`);
            }
            row.append(
                element(
                    'th',
                    { scope: 'row' },
                    String(address),
                    name ? element('span', { class: 'symbol' }, name) : '',
                    address === a ? element('span', { class: 'symbol', title: 'The A register points here' }, '← A') : '',
                ),
            );
            row.append(
                element(
                    'td',
                    {},
                    cellInput(`RAM[${address}]`, value, (text) => send({ type: 'set', target: 'RAM', address, value: text })),
                    mismatch ? element('span', { class: 'got' }, `expected ${mismatch.expected}`) : '',
                ),
            );
            memoryBody.append(row);
        });
    }
    if (focusedLabel) {
        const again = Array.from(root.querySelectorAll('input.cell')).find(
            (input) => input.getAttribute('aria-label') === focusedLabel,
        ) as HTMLElement | undefined;
        again?.focus();
    }
}

/** The predefined symbol of a RAM address, if it has one. */
function symbolFor(address: number): string {
    const names: Record<number, string> = { 0: 'SP', 1: 'LCL', 2: 'ARG', 3: 'THIS', 4: 'THAT', 16384: 'SCREEN', 24576: 'KBD' };
    if (names[address]) {
        return names[address];
    }
    return address >= 5 && address <= 15 ? `R${address}` : '';
}

window.addEventListener('message', (event: MessageEvent<ToComputer>) => {
    const message = event.data;
    if (message.type === 'rejected') {
        if (message.target === target) {
            rejected.textContent = message.message;
        }
        return;
    }
    if (target !== undefined && message.target < target) {
        return;
    }
    if (target !== message.target || !message.state) {
        state = null;
        heldKey = 0;
        pixels.fill(WHITE);
        context.putImageData(image, 0, 0);
        updateKeyboard();
    }
    if (target !== message.target) {
        expected = null;
        expectedVersion = undefined;
        expectedImage = null;
        test = null;
        scripts = [];
        inspected = null;
        expectedDirty = true;
        imageDirty = true;
        inspectDirty = true;
    }
    target = message.target;
    rejected.textContent = '';
    title.textContent = message.title;
    running = message.running;
    problem = message.problem;
    if (message.expected !== undefined) {
        expected = message.expected;
        expectedVersion = message.expectedVersion;
        expectedDirty = true;
    }
    if (message.expectedImage !== undefined) {
        expectedImage = message.expectedImage;
        imageDirty = true;
    }
    if (message.test !== undefined) {
        test = message.test;
        expectedDirty = true;
    }
    if (message.scripts !== undefined && JSON.stringify(message.scripts) !== JSON.stringify(scripts)) {
        scripts = message.scripts;
        expectedDirty = true;
    }
    if (message.inspected !== undefined && JSON.stringify(message.inspected) !== JSON.stringify(inspected)) {
        inspected = message.inspected;
        expectedDirty = true;
        inspectDirty = true;
    }
    if (message.state) {
        const previous = state;
        state = message.state;
        if (state.screen) {
            draw(state.screen);
        }
        // A frame without a memory window keeps the one on display.
        if (!state.ram && previous?.ram) {
            state.ram = previous.ram;
        }
    }
    // While a cell is being edited, a frame must not take it away.
    const editing = !running && document.activeElement instanceof HTMLInputElement && document.activeElement.classList.contains('cell');
    if (!editing || !running) {
        update();
    }
});

build();
update();
updateKeyboard();
vscode.postMessage({ type: 'ready', memoryStart, speed } satisfies Ready);
