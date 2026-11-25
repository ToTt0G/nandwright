// Runs the real bundled computer webview (`dist-test/webview/computer.js`) in a
// minimal DOM harness to verify Expected output, Expected screen, test row
// marks/diffs, action messages, and frame-update preservation.

import assert from 'node:assert/strict';
import * as fs from 'node:fs';
import * as path from 'node:path';
import { test } from 'node:test';
import * as vm from 'node:vm';
import type { FromComputer, ToComputer } from '../../webview/computer';

class FakeClassList {
    constructor(private readonly el: FakeElement) {}
    contains(cls: string): boolean {
        return this.el.className.split(/\s+/).filter(Boolean).includes(cls);
    }
    toggle(cls: string, force?: boolean): void {
        const set = new Set(this.el.className.split(/\s+/).filter(Boolean));
        if (force === undefined ? !set.has(cls) : force) {
            set.add(cls);
        } else {
            set.delete(cls);
        }
        this.el.className = [...set].join(' ');
    }
}

class FakeElement {
    className = '';
    value = '';
    title = '';
    hidden = false;
    disabled = false;
    selected = false;
    scrollTop = 0;
    scrollLeft = 0;
    readonly classList = new FakeClassList(this);
    readonly attributes = new Map<string, string>();
    readonly children: (FakeElement | string)[] = [];
    readonly listeners = new Map<string, ((event: any) => void)[]>();
    private text = '';

    constructor(readonly tagName: string) {}

    get textContent(): string {
        if (this.children.length === 0) {
            return this.text;
        }
        return this.children.map((c) => (typeof c === 'string' ? c : c.textContent)).join('');
    }

    set textContent(value: string) {
        this.children.length = 0;
        this.text = value;
    }

    setAttribute(name: string, value: string): void {
        this.attributes.set(name, value);
        if (name === 'hidden') {
            this.hidden = true;
        }
        if (name === 'disabled') {
            this.disabled = true;
        }
        if (name === 'title') {
            this.title = value;
        }
    }

    getAttribute(name: string): string | null {
        return this.attributes.get(name) ?? null;
    }

    append(...nodes: (FakeElement | string)[]): void {
        for (const node of nodes) {
            this.children.push(node);
        }
    }

    replaceChildren(...nodes: (FakeElement | string)[]): void {
        this.children.length = 0;
        this.text = '';
        this.append(...nodes);
    }

    addEventListener(type: string, handler: (event: any) => void): void {
        const list = this.listeners.get(type) ?? [];
        list.push(handler);
        this.listeners.set(type, list);
    }

    dispatch(type: string, event: Record<string, unknown> = {}): void {
        for (const handler of this.listeners.get(type) ?? []) {
            handler({ preventDefault: () => undefined, stopPropagation: () => undefined, ...event });
        }
    }

    focus(): void {
        // no-op
    }

    getContext(): { createImageData(w: number, h: number): { data: Uint8ClampedArray }; putImageData(): void } {
        return {
            createImageData: (w: number, h: number) => ({ data: new Uint8ClampedArray(w * h * 4) }),
            putImageData: () => undefined,
        };
    }

    querySelectorAll(selector: string): FakeElement[] {
        const parts = selector.trim().split(/\s+/);
        const pool = this.descendants();
        if (parts.length === 1) {
            return pool.filter((el) => matchesSimple(el, parts[0]));
        }
        const results: FakeElement[] = [];
        const walk = (el: FakeElement, ancestors: FakeElement[]) => {
            if (matchesSimple(el, parts[parts.length - 1])) {
                let idx = parts.length - 2;
                for (let i = ancestors.length - 1; i >= 0 && idx >= 0; i--) {
                    if (matchesSimple(ancestors[i], parts[idx])) {
                        idx--;
                    }
                }
                if (idx < 0) {
                    results.push(el);
                }
            }
            for (const child of el.children) {
                if (child instanceof FakeElement) {
                    walk(child, [...ancestors, el]);
                }
            }
        };
        for (const child of this.children) {
            if (child instanceof FakeElement) {
                walk(child, [this]);
            }
        }
        return results;
    }

    querySelector(selector: string): FakeElement | null {
        return this.querySelectorAll(selector)[0] ?? null;
    }

    private descendants(): FakeElement[] {
        const out: FakeElement[] = [];
        const visit = (el: FakeElement) => {
            for (const child of el.children) {
                if (child instanceof FakeElement) {
                    out.push(child);
                    visit(child);
                }
            }
        };
        visit(this);
        return out;
    }
}

function matchesSimple(el: FakeElement, selector: string): boolean {
    let rest = selector;
    const tag = /^[a-z][a-z0-9]*/i.exec(rest);
    if (tag) {
        if (el.tagName.toLowerCase() !== tag[0].toLowerCase()) {
            return false;
        }
        rest = rest.slice(tag[0].length);
    }
    const classes = [...rest.matchAll(/\.([a-zA-Z0-9_-]+)/g)].map((m) => m[1]);
    const elClasses = el.className.split(/\s+/).filter(Boolean);
    for (const cls of classes) {
        if (!elClasses.includes(cls)) {
            return false;
        }
    }
    const attrs = [...rest.matchAll(/\[([^=\]]+)="([^"]*)"\]/g)];
    for (const [, name, expected] of attrs) {
        if (el.getAttribute(name) !== expected) {
            return false;
        }
    }
    return true;
}

function mountComputerWebview() {
    const bundle = fs.readFileSync(path.join(__dirname, '..', 'webview', 'computer.js'), 'utf8');
    const root = new FakeElement('main');
    const sent: FromComputer[] = [];
    const windowListeners = new Map<string, ((event: any) => void)[]>();

    const context = vm.createContext({
        HTMLInputElement: class {},
        Uint32Array,
        document: {
            activeElement: null,
            getElementById: (id: string) => (id === 'root' ? root : null),
            createElement: (tag: string) => new FakeElement(tag),
        },
        window: {
            addEventListener: (type: string, fn: (event: any) => void) => {
                const list = windowListeners.get(type) ?? [];
                list.push(fn);
                windowListeners.set(type, list);
            },
        },
        acquireVsCodeApi: () => ({
            postMessage: (m: FromComputer) => sent.push(JSON.parse(JSON.stringify(m))),
            getState: () => undefined,
            setState: () => undefined,
        }),
    });
    vm.runInContext(bundle, context);

    const deliver = (message: ToComputer) => {
        for (const fn of windowListeners.get('message') ?? []) {
            fn({ data: message });
        }
    };
    return { root, sent, deliver };
}

function baseState(overrides: Partial<Extract<ToComputer, { type: 'state' }>> = {}): Extract<ToComputer, { type: 'state' }> {
    return {
        type: 'state',
        target: 4,
        title: 'double.asm',
        running: false,
        problem: null,
        state: {
            kind: 'cpu',
            status: 'paused',
            steps: 0,
            programLength: 10,
            registers: [
                { name: 'A', value: 0 },
                { name: 'D', value: 0 },
                { name: 'PC', value: 0 },
                { name: 'M', value: 0 },
            ],
            ram: { start: 0, values: new Array(16).fill(0) },
            screen: null,
            key: 0,
            file: '/work/double.asm',
            line: 0,
            instruction: '@0',
            error: null,
        },
        ...overrides,
    };
}

test('computer webview renders expected output marks, diffs, actions and preserves them across animation frames', () => {
    const { root, sent, deliver } = mountComputerWebview();
    assert.equal(sent[0]?.type, 'ready');
    sent.length = 0;

    deliver(
        baseState({
            expected: {
                path: '/work/Double.cmp',
                name: 'Double.cmp',
                header: ['RAM[0]', 'RAM[1]'],
                rows: [
                    ['3', '6'],
                    ['5', '10'],
                    ['7', '14'],
                ],
                rawRows: [
                    ['       3  ', '       6  '],
                    ['       5  ', '      10  '],
                    ['       7  ', '      14  '],
                ],
                lines: [2, 3, 4],
                more: 0,
            },
            test: {
                script: 'Double.tst',
                scriptPath: '/work/Double.tst',
                sourceLabel: 'assembled double.asm',
                status: 'current',
                kind: 'failed',
                comparedLines: 3,
                comparedRows: 2,
                differingRows: 1,
                failureCount: 1,
                rows: ['match', 'differ', 'unreached'],
                actual: { 1: ['       5  ', '       9  '] },
            },
        }),
    );

    const summary = root.querySelector('.test-run .summary');
    assert.equal(summary?.textContent, 'Double.tst (assembled double.asm) fails: 1 of 2 rows differ.');
    assert.ok(summary?.classList.contains('fails'));

    const marks = root.querySelectorAll('table.expected tbody td.mark').map((td) => td.textContent);
    assert.deepEqual(marks, ['✓', '✗', '–']);

    const differs = root.querySelector('table.expected td.differs');
    assert.ok(differs);
    assert.equal(differs.querySelector('.got')?.textContent, 'got 9');

    // Clicking the `.cmp` link, Diff button, and Run test button sends tagged messages with script identity.
    root.querySelector('button.link-inline')!.dispatch('click');
    assert.deepEqual(sent.pop(), { type: 'openExpected', program: 4 });

    const diffButton = root.querySelectorAll('.test-run button').find((b) => b.textContent.includes('Diff'));
    assert.ok(diffButton);
    diffButton.dispatch('click');
    assert.deepEqual(sent.pop(), { type: 'compareOutput', script: '/work/Double.tst', program: 4 });

    const runButton = root.querySelectorAll('.test-run button').find((b) => b.textContent === 'Run test');
    assert.ok(runButton);
    runButton.dispatch('click');
    assert.deepEqual(sent.pop(), { type: 'runTest', script: '/work/Double.tst', program: 4 });

    // A high-frequency frame that omits `expected`/`test` keeps the rendered section untouched.
    const scrollBox = root.querySelector('.expected-scroll')!;
    scrollBox.scrollTop = 120;
    const tableBefore = root.querySelector('table.expected');
    deliver(baseState({ running: true }));
    assert.strictEqual(root.querySelector('table.expected'), tableBefore, 'animation frame does not rebuild expected table');

    // A test-status update for the same compare file keeps `.expected-scroll` mounted and preserves `scrollTop`.
    deliver(
        baseState({
            test: {
                script: 'Double.tst',
                scriptPath: '/work/Double.tst',
                status: 'running',
            },
        }),
    );
    assert.strictEqual(root.querySelector('.expected-scroll'), scrollBox);
    assert.equal(scrollBox.scrollTop, 120);
});

test('computer webview uses engine wildcard/spacing rules and explains non-row failures', () => {
    const { root, deliver } = mountComputerWebview();

    deliver(
        baseState({
            expected: {
                path: '/work/Wild.cmp',
                name: 'Wild.cmp',
                header: ['A', 'B'],
                rows: [['1*', '6']],
                rawRows: [['  1*  ', '   6  ']],
                lines: [2],
                more: 0,
            },
            test: {
                script: 'Wild.tst',
                status: 'current',
                kind: 'failed',
                comparedLines: 2,
                comparedRows: 1,
                differingRows: 1,
                failureCount: 1,
                rows: ['differ'],
                // Cell 0 (`  10  `) matches partial wildcard `  1*  `; cell 1 has wrong spacing!
                actual: { 0: ['  10  ', '  6   '] },
            },
        }),
    );
    const differingCells = root.querySelectorAll('table.expected td.differs');
    assert.equal(differingCells.length, 1, 'partial wildcard cell is not falsely blamed');
    assert.equal(differingCells[0].querySelector('.got')?.textContent, 'spacing differs');

    // Header/extra-output failure (0 data rows differ) shows the line-level reason, never "0 of N rows differ".
    deliver(
        baseState({
            test: {
                script: 'Wild.tst',
                status: 'current',
                kind: 'failed',
                comparedLines: 1,
                comparedRows: 0,
                differingRows: 0,
                failureCount: 1,
                firstFailureLine: 1,
                firstFailureReason: 'header line 1 differs',
                rows: ['unreached'],
                actual: {},
            },
        }),
    );
    assert.equal(root.querySelector('.test-run .summary')?.textContent, 'Wild.tst fails: header line 1 differs.');
});

test('computer webview renders expected screen image and clears sections on target switch', () => {
    const { root, sent, deliver } = mountComputerWebview();
    sent.length = 0;

    deliver(
        baseState({
            target: 2,
            title: 'Display.jack',
            expectedImage: {
                path: '/work/DisplayTestOutput.gif',
                name: 'DisplayTestOutput.gif',
                dataUrl: 'data:image/gif;base64,R0lGODlhAQABAAAAACw=',
            },
        }),
    );

    const img = root.querySelector('.expected-screen img');
    assert.ok(img);
    assert.equal(img.getAttribute('src'), 'data:image/gif;base64,R0lGODlhAQABAAAAACw=');

    root.querySelector('button.link-inline')!.dispatch('click');
    assert.deepEqual(sent.pop(), { type: 'openExpectedImage', program: 2 });

    // Switching to another program clears the previous program's expected image.
    deliver(baseState({ target: 3, title: 'Other.asm' }));
    assert.equal(root.querySelector('.expected-screen img'), null);
});

const multExpected = {
    path: '/work/Mult.cmp',
    name: 'Mult.cmp',
    header: ['RAM[0]', 'RAM[1]', 'RAM[2]'],
    rows: [
        ['3', '1', '3'],
        ['2', '4', '8'],
        ['0', '0', '0'],
    ],
    rawRows: [
        ['     3 ', '     1 ', '     3 '],
        ['     2 ', '     4 ', '     8 '],
        ['     0 ', '     0 ', '     0 '],
    ],
    lines: [2, 3, 4],
    more: 0,
};

const multFailed = {
    script: 'Mult.tst',
    scriptPath: '/work/Mult.tst',
    status: 'current' as const,
    kind: 'failed' as const,
    comparedLines: 3,
    comparedRows: 2,
    differingRows: 1,
    failureCount: 1,
    rows: ['match', 'differ', 'unreached'] as ('match' | 'differ' | 'unreached')[],
    actual: { 1: ['     2 ', '     4 ', '     6 '] },
};

const multInspected = {
    script: 'Mult.tst',
    scriptPath: '/work/Mult.tst',
    scriptLine: 12,
    rowIndex: 1,
    compareLine: 3,
    stepped: false,
    mismatches: [
        { column: 'RAM[2]', target: 'RAM' as const, address: 2, expected: '8', actual: '6' },
        { column: 'D', target: 'register' as const, name: 'D', expected: '8', actual: '6' },
        { column: 'time', target: 'other' as const, expected: '5', actual: '7' },
    ],
};

test('computer webview offers "Inspect failure" and inspects reached rows via row click or labeled mark button', () => {
    const { root, sent, deliver } = mountComputerWebview();
    sent.length = 0;
    deliver(baseState({ expected: multExpected, expectedVersion: 7, test: multFailed }));

    const inspect = root.querySelectorAll('.test-run button').find((b) => b.textContent === 'Inspect failure');
    assert.ok(inspect, 'a failing current run offers Inspect failure');
    assert.equal(inspect.getAttribute('data-expected-action'), 'inspectFirst');
    inspect.dispatch('click');
    assert.deepEqual(sent.pop(), { type: 'inspectRow', script: '/work/Mult.tst', line: 3, rowIndex: 1, expectedVersion: 7, program: 4 });

    const rows = root.querySelectorAll('table.expected tbody tr');
    assert.equal(rows.length, 3);
    assert.ok(rows[0].classList.contains('inspectable'));
    assert.ok(rows[0].title.includes('see the machine at this row'));
    assert.ok(rows[1].classList.contains('inspectable'));
    assert.ok(!rows[2].classList.contains('inspectable'), 'an unreached row is not inspectable');

    const markButtons = root.querySelectorAll('table.expected tbody button.mark-button');
    assert.equal(markButtons.length, 2, 'only reached rows have an accessible inspect button');
    assert.equal(markButtons[0].getAttribute('aria-label'), 'Inspect row 1 (matches, line 2)');
    assert.equal(markButtons[1].getAttribute('aria-label'), 'Inspect row 2 (differs, line 3)');

    rows[0].dispatch('click');
    assert.deepEqual(sent.pop(), { type: 'inspectRow', script: '/work/Mult.tst', line: 2, rowIndex: 0, expectedVersion: 7, program: 4 });
    markButtons[1].dispatch('click');
    assert.deepEqual(sent.pop(), { type: 'inspectRow', script: '/work/Mult.tst', line: 3, rowIndex: 1, expectedVersion: 7, program: 4 });
    rows[2].dispatch('click');
    assert.equal(sent.length, 0, 'unreached rows send nothing');

    // A passing run has no failure to inspect.
    deliver(baseState({ test: { ...multFailed, kind: 'passed', rows: ['match', 'match', 'match'], actual: {} } }));
    assert.equal(root.querySelectorAll('.test-run button').find((b) => b.textContent === 'Inspect failure'), undefined);
});

test('computer webview shows the inspection banner, chips and actions', () => {
    const { root, sent, deliver } = mountComputerWebview();
    deliver(baseState({ expected: multExpected, test: multFailed }));
    const banner = root.querySelector('.inspect-banner')!;
    assert.ok(banner.hidden, 'no banner before an inspection');

    sent.length = 0;
    deliver(baseState({ inspected: multInspected }));
    assert.ok(!banner.hidden);
    assert.equal(banner.querySelector('.inspect-title')?.textContent, 'Inspecting Mult.tst · row 2 (line 3)');
    const chips = banner.querySelectorAll('.chip').map((c) => c.textContent);
    assert.deepEqual(chips, ['RAM[2]: expected 8, got 6', 'D: expected 8, got 6', 'time: expected 5, got 7']);

    const inspectedRow = root.querySelector('table.expected tr.inspected');
    assert.ok(inspectedRow, 'the inspected row is marked in the table');
    assert.equal(inspectedRow.getAttribute('data-row'), '1');

    banner.querySelector('button.chip')!.dispatch('click');
    assert.deepEqual(sent.pop(), { type: 'memory', start: 0, program: 4 });

    const buttons = banner.querySelectorAll('button').filter((b) => !b.classList.contains('chip'));
    assert.deepEqual(buttons.map((b) => b.textContent), ['Open Mult.tst:12', 'Debug script', 'Reset']);
    buttons[0].dispatch('click');
    assert.deepEqual(sent.pop(), { type: 'openScriptAt', script: '/work/Mult.tst', line: 12, program: 4 });
    buttons[1].dispatch('click');
    assert.deepEqual(sent.pop(), { type: 'debugScript', script: '/work/Mult.tst', program: 4 });
    buttons[2].dispatch('click');
    assert.deepEqual(sent.pop(), { type: 'reset', program: 4 });

    // A far RAM cell jumps the memory window near it.
    deliver(baseState({ inspected: { ...multInspected, mismatches: [{ column: 'RAM[261]', target: 'RAM', address: 261, expected: '1', actual: '2' }] } }));
    banner.querySelector('button.chip')!.dispatch('click');
    assert.deepEqual(sent.pop(), { type: 'memory', start: 259, program: 4 });

    // Without a script line there is no Open button; stepped hides the chips.
    deliver(baseState({ inspected: { ...multInspected, scriptLine: null, stepped: true } }));
    assert.equal(banner.querySelector('.inspect-title')?.textContent, 'Stepped from Mult.tst · row 2 (line 3)');
    assert.equal(banner.querySelectorAll('.chip').length, 0);
    assert.deepEqual(banner.querySelectorAll('button').map((b) => b.textContent), ['Debug script', 'Reset']);

    // Structural failure with no individual blamed cells must not say "This row matches".
    deliver(baseState({ inspected: { ...multInspected, differs: true, mismatches: [] } }));
    assert.equal(banner.getAttribute('role'), 'status');
    assert.equal(banner.getAttribute('aria-live'), 'polite');
    assert.ok(banner.querySelector('.hint')?.textContent.includes('line format or column count does not match'));

    deliver(baseState({ inspected: null }));
    assert.ok(banner.hidden);
    assert.equal(root.querySelector('table.expected tr.inspected'), null);
});

test('computer webview highlights mismatched registers and RAM until the student steps', () => {
    const { root, deliver } = mountComputerWebview();
    const ram = { start: 0, values: [2, 4, 6, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0] };
    const base = baseState();
    const machine = { ...base.state!, ram };
    deliver(baseState({ state: machine, inspected: multInspected }));

    const differingRegisters = root.querySelectorAll('.registers .register.differs');
    assert.equal(differingRegisters.length, 1);
    assert.equal(differingRegisters[0].querySelector('.register-name')?.textContent, 'D');
    assert.equal(differingRegisters[0].querySelector('.got')?.textContent, 'expected 8');

    const differingRam = root.querySelectorAll('table.memory tbody tr.differs');
    assert.equal(differingRam.length, 1);
    assert.equal(differingRam[0].querySelector('th')?.textContent.startsWith('2'), true);
    assert.equal(differingRam[0].querySelector('.got')?.textContent, 'expected 8');

    deliver(baseState({ state: { ...machine, steps: 1 }, inspected: { ...multInspected, stepped: true } }));
    assert.equal(root.querySelectorAll('.registers .register.differs').length, 0);
    assert.equal(root.querySelectorAll('table.memory tbody tr.differs').length, 0);
    assert.equal(root.querySelectorAll('.got').filter((g) => g.textContent.startsWith('expected')).length, 0);
});

test('computer webview offers a script picker when several scripts test the program', () => {
    const { root, sent, deliver } = mountComputerWebview();
    const one = [{ name: 'Mult.tst', path: '/work/Mult.tst' }];
    deliver(baseState({ expected: multExpected, test: multFailed, scripts: one }));
    assert.equal(root.querySelector('select[data-expected-action="script"]'), null, 'no picker for one script');

    sent.length = 0;
    deliver(baseState({ scripts: [...one, { name: 'MultBig.tst', path: '/work/MultBig.tst' }] }));
    const picker = root.querySelector('select[data-expected-action="script"]');
    assert.ok(picker);
    const options = picker.querySelectorAll('option');
    assert.deepEqual(options.map((o) => o.textContent), ['Mult.tst', 'MultBig.tst']);
    assert.ok(options[0].selected);
    picker.value = '/work/MultBig.tst';
    picker.dispatch('change');
    assert.deepEqual(sent.pop(), { type: 'selectScript', script: '/work/MultBig.tst', program: 4 });
});
