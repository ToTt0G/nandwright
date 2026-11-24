// The chip simulator panel (runs inside a VS Code webview).
//
// It shows one chip: its inputs, which can be changed, and its outputs and
// internal pins, which follow. The extension owns the simulation; this file
// only draws the state it is sent and reports what the student does.

import type { Build } from '../chipSessions';
import type { ChipState, ChipTest, ClockStep, Expected, Pin, TruthTable } from '../runtime/protocol';
import { Radix, cellValue, formatValue, toBigInt, typedValue } from '../values';

declare function acquireVsCodeApi(): {
    postMessage(message: unknown): void;
    getState(): any;
    setState(state: any): void;
};

export type ToPanel =
    | {
          type: 'state';
          state: ChipState | null;
          /** The build `state` is of (`{session, revision}`), echoed back on `set` and `setAll`. */
          build: Build | null;
          /** Names the chip file currently targeted; echoed back on inputs and actions. */
          target: number;
          table: TruthTable | null;
          problem: string | null;
          expected: Expected | null;
          test: ChipTest | null;
          history: ClockStep[];
          historyCapped: boolean;
          /** The simulator stopped for good: offer Retry next to the problem. */
          retry?: boolean;
      }
    | { type: 'rejected'; pin: string; message: string }
    | { type: 'note'; message: string };

export type FromPanel =
    | { type: 'ready' }
    | { type: 'openExpected' }
    | { type: 'runTest' }
    | { type: 'clearHistory' }
    | { type: 'retry' }
    | { type: 'set'; pin: string; value: string | number; build: Build; target: number }
    | { type: 'setAll'; values: Record<string, number>; build: Build; target: number }
    | { type: 'action'; action: 'eval' | 'tick' | 'tock' | 'step' | 'reset'; target: number };

const vscode = acquireVsCodeApi();
const send = (message: FromPanel) => vscode.postMessage(message);

/** How long per-pin rejected-input feedback stays visible if the student does not edit that pin again. */
const REJECTED_MS = 5000;
/** How long a short non-modal note stays visible. */
const NOTE_MS = 4000;

let radix: Radix = vscode.getState()?.radix ?? 'dec';
let current: ChipState | null = null;
let currentBuild: Build | null = null;
let currentTarget = 0;
let table: TruthTable | null = null;
let expected: Expected | null = null;
let test: ChipTest | null = null;
let history: ClockStep[] = [];
let historyCapped = false;
let previous = new Map<string, string>();
/** The problem shown, kept for a redraw (a radix change). */
let shownProblem: { problem: string | null; retry: boolean } = { problem: null, retry: false };
/** Active rejected-input messages per pin, preserved across redraws until next edit or timeout. */
const rejectedPins = new Map<string, { message: string; timer: ReturnType<typeof setTimeout> }>();
/** Active non-modal note, preserved across redraws until timeout. */
let activeNote: { message: string; timer: ReturnType<typeof setTimeout> } | null = null;

function setRejection(pin: string, message: string): void {
    const prior = rejectedPins.get(pin);
    if (prior) {
        clearTimeout(prior.timer);
    }
    const timer = setTimeout(() => clearRejection(pin), REJECTED_MS);
    rejectedPins.set(pin, { message, timer });
    const slot = root.querySelector(`.rejected[data-for="${CSS.escape(pin)}"]`);
    if (slot) {
        slot.textContent = message;
    }
}

function clearRejection(pin: string): void {
    const prior = rejectedPins.get(pin);
    if (!prior) {
        return;
    }
    clearTimeout(prior.timer);
    rejectedPins.delete(pin);
    const slot = root.querySelector(`.rejected[data-for="${CSS.escape(pin)}"]`);
    if (slot) {
        slot.textContent = '';
    }
}

function clearAllFeedback(): void {
    for (const entry of rejectedPins.values()) {
        clearTimeout(entry.timer);
    }
    rejectedPins.clear();
    if (activeNote) {
        clearTimeout(activeNote.timer);
        activeNote = null;
    }
}

function setNote(message: string): void {
    if (activeNote) {
        clearTimeout(activeNote.timer);
    }
    const timer = setTimeout(() => {
        activeNote = null;
        const box = root.querySelector('.note');
        if (box) {
            box.textContent = '';
        }
    }, NOTE_MS);
    activeNote = { message, timer };
    const box = root.querySelector('.note');
    if (box) {
        box.textContent = message;
    } else {
        render(shownProblem.problem, shownProblem.retry);
    }
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

const root = document.getElementById('root')!;

function widthLabel(pin: { width: number }): string {
    return pin.width > 1 ? `[${pin.width}]` : '';
}

function pinRow(pin: Pin, editable: boolean, build: Build | null, target: number): HTMLElement {
    const value = String(pin.bits);
    const changed = previous.has(pin.name) && previous.get(pin.name) !== value;
    const row = element('div', { class: `pin${changed ? ' changed' : ''}`, role: 'row' });
    const name = element(
        'span',
        { class: 'pin-name', role: 'rowheader' },
        pin.name,
        element('span', { class: 'pin-width' }, widthLabel(pin)),
    );
    row.append(name);

    if (editable && pin.width === 1) {
        const on = value === '1';
        const toggle = element(
            'button',
            {
                class: `bit${on ? ' on' : ''}`,
                type: 'button',
                role: 'switch',
                'aria-checked': String(on),
                'aria-label': `${pin.name}: ${on ? 1 : 0}. Press to change.`,
                title: 'Click to flip this input',
            },
            on ? '1' : '0',
        );
        toggle.addEventListener('click', () => {
            clearRejection(pin.name);
            if (build) {
                send({ type: 'set', pin: pin.name, value: on ? 0 : 1, build, target });
            }
        });
        const cell = element('span', { role: 'cell', class: 'bit-cell' }, toggle);
        cell.append(element('span', { class: 'rejected', 'aria-live': 'polite', 'data-for': pin.name }, rejectedPins.get(pin.name)?.message ?? ''));
        row.append(cell);
    } else if (editable) {
        const input = element('input', {
            class: 'bus',
            type: 'text',
            spellcheck: 'false',
            autocomplete: 'off',
            'aria-label': `${pin.name}, ${pin.width} bits`,
            'data-pin': pin.name,
        });
        input.value = formatValue(pin.bits, pin.width, radix);
        const shown = input.value;
        const commit = () => {
            const text = input.value.trim();
            if (text !== shown && build) {
                clearRejection(pin.name);
                send({ type: 'set', pin: pin.name, value: typedValue(text, radix), build, target });
            }
        };
        input.addEventListener('input', () => clearRejection(pin.name));
        input.addEventListener('keydown', (event) => {
            if (event.key === 'Enter') {
                commit();
            } else if (event.key === 'Escape') {
                clearRejection(pin.name);
                input.value = shown;
            }
        });
        input.addEventListener('blur', commit);
        const cell = element('span', { role: 'cell', class: 'bus-cell' }, input);
        cell.append(element('span', { class: 'rejected', 'aria-live': 'polite', 'data-for': pin.name }, rejectedPins.get(pin.name)?.message ?? ''));
        row.append(cell);
    } else {
        const text = formatValue(pin.bits, pin.width, radix);
        const cell = element(
            'span',
            { role: 'cell', class: pin.width === 1 ? `bit readonly${value === '1' ? ' on' : ''}` : 'value' },
            text,
        );
        row.append(cell);
    }
    return row;
}

function section(title: string, pins: Pin[], editable: boolean, build: Build | null, target: number, emptyText?: string): HTMLElement | null {
    if (pins.length === 0 && !emptyText) {
        return null;
    }
    const block = element('section', {});
    block.append(element('h2', {}, title));
    if (pins.length === 0) {
        block.append(element('p', { class: 'hint' }, emptyText!));
        return block;
    }
    const grid = element('div', { class: 'pins', role: 'table', 'aria-label': title });
    pins.forEach((pin) => grid.append(pinRow(pin, editable, build, target)));
    block.append(grid);
    return block;
}

function radixPicker(): HTMLElement {
    const group = element('div', { class: 'segmented', role: 'radiogroup', 'aria-label': 'Number format' });
    const options: [Radix, string][] = [
        ['dec', 'Decimal'],
        ['bin', 'Binary'],
        ['hex', 'Hex'],
    ];
    for (const [value, label] of options) {
        const button = element(
            'button',
            { type: 'button', role: 'radio', 'aria-checked': String(radix === value), class: radix === value ? 'selected' : '' },
            label,
        );
        button.addEventListener('click', () => {
            radix = value;
            vscode.setState({ radix });
            render(shownProblem.problem, shownProblem.retry);
        });
        group.append(button);
    }
    return group;
}

function actionButton(label: string, action: 'eval' | 'tick' | 'tock' | 'step' | 'reset', title: string, target: number, primary = false): HTMLElement {
    const button = element('button', { type: 'button', class: primary ? 'primary' : 'secondary', title }, label);
    button.addEventListener('click', () => send({ type: 'action', action, target }));
    return button;
}

/**
 * The compare file as a table. For a chip without a clock whose inputs all
 * have a column, the row for the current inputs is marked, a click sets the
 * inputs to a row, and an output that differs in the current row is pointed
 * out. Otherwise the table is only shown.
 */
/** One line on the last run of the chip's test script. */
function testSummary(run: ChipTest): { text: string; fails: boolean } {
    const compared = run.comparedRows ?? Math.max(0, (run.comparedLines ?? 0) - 1);
    const differing = run.differingRows ?? run.failureCount ?? 0;
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
            return { text: `${run.script} passes: all ${compared} rows match.`, fails: false };
        case 'failed':
            return { text: `${run.script} fails: ${differing} of ${compared} rows differ.`, fails: true };
        case 'error':
            return { text: `${run.script} stopped with an error: ${run.message ?? ''}`, fails: true };
        default:
            return { text: `${run.script} did not give a result: ${run.message ?? ''}`, fails: false };
    }
}

function testBar(run: ChipTest): HTMLElement {
    const bar = element('div', { class: 'test-run' });
    const { text, fails } = testSummary(run);
    bar.append(element('span', { class: `summary${fails ? ' fails' : ''}`, role: 'status' }, text));
    const button = element('button', { type: 'button', class: 'secondary', title: `Run ${run.script}` }, 'Run test');
    if (run.status === 'running') {
        button.setAttribute('disabled', '');
    }
    button.addEventListener('click', () => send({ type: 'runTest' }));
    bar.append(button);
    return bar;
}

const MARKS = { match: '✓', differ: '✗', unreached: '–' } as const;

function expectedTable(state: ChipState, file: Expected, build: Build | null, target: number): HTMLElement {
    const block = element('section', {});
    block.append(element('h2', {}, 'Expected output'));
    const open = element('button', { type: 'button', class: 'link' }, file.name);
    open.addEventListener('click', () => send({ type: 'openExpected' }));
    const hint = element('p', { class: 'hint' }, 'From ');
    hint.append(open, ': what the test script compares your chip with.');
    block.append(hint);
    if (test) {
        block.append(testBar(test));
    }
    // Marks only for a result that still holds.
    const marks = test?.status === 'current' && test.rows ? test.rows : null;
    const actualRows = (marks && test?.actual) || {};

    const inputs = state.pins.filter((pin) => pin.role === 'input');
    const outputs = state.pins.filter((pin) => pin.role === 'output');
    const column = (pin: Pin) => file.header.indexOf(pin.name);
    const interactive = !state.clocked && inputs.every((pin) => column(pin) >= 0);
    const matches = (row: string[]) =>
        inputs.every((pin) => {
            const value = cellValue(row[column(pin)] ?? '', pin.width);
            return value === 'any' || (value !== null && value === toBigInt(pin.bits));
        });
    const currentRow = interactive ? file.rows.find(matches) : undefined;

    const tableElement = element('table', { class: 'truth expected' });
    const head = element('tr', {});
    if (marks) {
        head.append(element('th', { scope: 'col', class: 'mark', 'aria-label': 'Last test run' }));
    }
    file.header.forEach((cell) => head.append(element('th', { scope: 'col' }, cell)));
    tableElement.append(element('thead', {}, head));
    const body = element('tbody', {});
    for (const [index, row] of file.rows.entries()) {
        const isCurrent = row === currentRow;
        const values: Record<string, number> = {};
        let settable = interactive;
        for (const pin of inputs) {
            const value = interactive ? cellValue(row[column(pin)] ?? '', pin.width) : null;
            if (typeof value === 'bigint') {
                values[pin.name] = Number(value);
            } else {
                settable = false;
            }
        }
        const attributes: Record<string, string> = {};
        if (isCurrent) {
            attributes.class = 'current';
        }
        if (settable) {
            attributes.tabindex = '0';
            attributes.title = 'Set the inputs to this row';
            attributes['aria-selected'] = String(isCurrent);
        }
        const tr = element('tr', attributes);
        const mark = marks?.[index];
        const produced = mark === 'differ' ? actualRows[index] : undefined;
        if (marks) {
            const label =
                mark === 'match'
                    ? 'This row matches'
                    : mark === 'differ'
                      ? `This row differs. Your chip gave: ${produced ? `|${produced.join('|')}|` : ''}`
                      : 'The test did not reach this row';
            tr.append(element('td', { class: `mark ${mark ?? 'unreached'}`, title: label, 'aria-label': label }, MARKS[mark ?? 'unreached']));
        }
        row.forEach((cell, i) => {
            const pin = isCurrent ? outputs.find((p) => column(p) === i) : undefined;
            const wanted = pin ? cellValue(cell, pin.width) : null;
            const gave = produced?.[i]?.trim();
            if (pin && typeof wanted === 'bigint' && wanted !== toBigInt(pin.bits)) {
                const actual = formatValue(pin.bits, pin.width, pin.width > 1 ? radix : 'dec');
                const text = `Expected ${cell}, your chip gives ${actual}`;
                tr.append(element('td', { class: 'differs', title: text, 'aria-label': text }, `${cell} ✗`));
            } else if (gave !== undefined && !/^\*+$/.test(cell) && gave !== cell) {
                const text = `Expected ${cell}, your chip gave ${gave}`;
                tr.append(element('td', { class: 'differs', title: text, 'aria-label': text }, `${cell} ✗`));
            } else {
                tr.append(element('td', {}, cell));
            }
        });
        if (settable) {
            const apply = () => {
                if (build) {
                    send({ type: 'setAll', values, build, target });
                }
            };
            tr.addEventListener('click', apply);
            tr.addEventListener('keydown', (event) => {
                if (event.key === 'Enter' || event.key === ' ') {
                    event.preventDefault();
                    apply();
                }
            });
        }
        body.append(tr);
    }
    tableElement.append(body);
    block.append(tableElement);
    if (file.more > 0) {
        block.append(element('p', { class: 'hint' }, `${file.more} more rows are in ${file.name}.`));
    }
    return block;
}

/** The pins after each tick and tock, oldest first, in a box that scrolls. */
function clockHistory(state: ChipState): HTMLElement {
    const block = element('section', {});
    block.append(element('h2', {}, 'Clock history'));
    if (history.length === 0) {
        block.append(element('p', { class: 'hint' }, 'Press Tick or Tock and each step is listed here.'));
        return block;
    }
    const bar = element('div', { class: 'history-bar' });
    if (historyCapped) {
        bar.append(element('p', { class: 'hint' }, 'Showing the last 100 steps.'));
    }
    const clear = element('button', { type: 'button', class: 'link' }, 'Clear');
    clear.addEventListener('click', () => send({ type: 'clearHistory' }));
    bar.append(clear);
    block.append(bar);
    const columns = state.pins.filter((pin) => pin.role === 'input' || pin.role === 'output');
    const tableElement = element('table', { class: 'truth history' });
    const head = element('tr', {});
    head.append(element('th', { scope: 'col' }, 'time'));
    columns.forEach((pin) =>
        head.append(element('th', { scope: 'col', class: pin === columns.find((p) => p.role === 'output') ? 'out first-out' : '' }, pin.name)),
    );
    tableElement.append(element('thead', {}, head));
    const body = element('tbody', {});
    for (const step of history) {
        const tr = element('tr', {});
        tr.append(element('td', {}, step.time));
        for (const pin of columns) {
            const value = step.pins.find((p) => p.name === pin.name);
            const text = value ? formatValue(value.bits, value.width, value.width > 1 ? radix : 'dec') : '';
            tr.append(element('td', pin === columns.find((p) => p.role === 'output') ? { class: 'first-out' } : {}, text));
        }
        body.append(tr);
    }
    tableElement.append(body);
    const scroller = element('div', { class: 'history-scroll', tabindex: '0', role: 'region', 'aria-label': 'Clock history' }, tableElement);
    block.append(scroller);
    return block;
}

function truthTable(state: ChipState, truth: TruthTable, build: Build | null, target: number): HTMLElement {
    const block = element('section', {});
    block.append(element('h2', {}, 'Truth table'));
    block.append(element('p', { class: 'hint' }, 'Every input combination. Select a row to apply it.'));
    const tableElement = element('table', { class: 'truth' });
    const head = element('tr', {});
    truth.inputs.forEach((pin) => head.append(element('th', { scope: 'col' }, pin.name)));
    truth.outputs.forEach((pin, i) =>
        head.append(element('th', { scope: 'col', class: i === 0 ? 'out first-out' : 'out' }, pin.name)),
    );
    tableElement.append(element('thead', {}, head));
    const body = element('tbody', {});
    const now = truth.inputs.map((pin) => Number(state.pins.find((p) => p.name === pin.name)?.bits ?? 0));
    for (const row of truth.rows) {
        const isCurrent = row.inputs.every((value, i) => value === now[i]);
        const tr = element('tr', {
            class: isCurrent ? 'current' : '',
            tabindex: '0',
            'aria-selected': String(isCurrent),
        });
        row.inputs.forEach((value, i) =>
            tr.append(element('td', {}, formatValue(value, truth.inputs[i].width, truth.inputs[i].width > 1 ? radix : 'dec'))),
        );
        row.outputs.forEach((value, i) =>
            tr.append(
                element(
                    'td',
                    { class: i === 0 ? 'out first-out' : 'out' },
                    formatValue(value, truth.outputs[i].width, truth.outputs[i].width > 1 ? radix : 'dec'),
                ),
            ),
        );
        const apply = () => {
            if (!build) {
                return;
            }
            const values: Record<string, number> = {};
            truth.inputs.forEach((pin, i) => (values[pin.name] = row.inputs[i]));
            send({ type: 'setAll', values, build, target });
        };
        tr.addEventListener('click', apply);
        tr.addEventListener('keydown', (event) => {
            if (event.key === 'Enter' || event.key === ' ') {
                event.preventDefault();
                apply();
            }
        });
        body.append(tr);
    }
    tableElement.append(body);
    block.append(tableElement);
    return block;
}

function render(problem: string | null, retry = false): void {
    const build = currentBuild;
    const target = currentTarget;
    const focused = (document.activeElement as HTMLElement | null)?.getAttribute('data-pin');
    root.replaceChildren();
    if (problem) {
        const banner = element('div', { class: 'problem', role: 'alert' });
        const heading = retry
            ? 'The simulator stopped.'
            : current
              ? 'The chip has an error. Showing the last version that loaded.'
              : 'This chip cannot be simulated yet.';
        banner.append(element('strong', {}, heading));
        banner.append(element('pre', {}, problem));
        if (retry) {
            const again = element('button', { type: 'button', class: 'primary retry' }, 'Retry');
            again.addEventListener('click', () => send({ type: 'retry' }));
            banner.append(again);
        }
        root.append(banner);
    }
    if (!current) {
        if (!problem) {
            root.append(element('p', { class: 'hint' }, 'Loading the chip…'));
        }
        if (activeNote) {
            root.append(element('div', { class: 'note', role: 'status', 'aria-live': 'polite' }, activeNote.message));
        }
        return;
    }
    const state = current;
    const header = element('header', {});
    header.append(element('h1', {}, state.chip));
    header.append(
        element(
            'span',
            {
                class: 'badge',
                title: state.clocked
                    ? 'This chip has state: its outputs change on the clock.'
                    : 'This chip has no state: its outputs follow its inputs at once.',
            },
            state.clocked ? 'clocked' : 'combinational',
        ),
    );
    if (state.clocked) {
        header.append(element('span', { class: 'time', title: 'Clock time. A + means the clock is high (after tick).' }, `time ${state.time}`));
    }
    root.append(header);

    const toolbar = element('div', { class: 'toolbar', role: 'toolbar', 'aria-label': 'Simulation' });
    if (state.clocked) {
        const high = state.time.endsWith('+');
        toolbar.append(
            actionButton('Tick', 'tick', 'Raise the clock: clocked chips read their inputs', target, !high),
            actionButton('Tock', 'tock', 'Lower the clock: clocked chips show their new outputs', target, high),
            actionButton('Full cycle', 'step', 'Tick, then tock', target),
        );
    }
    toolbar.append(actionButton('Reset', 'reset', 'Reload the chip and clear its state. Inputs are kept.', target));
    toolbar.append(element('span', { class: 'spacer' }));
    toolbar.append(radixPicker());
    root.append(toolbar);
    root.append(element('div', { class: 'note', role: 'status', 'aria-live': 'polite' }, activeNote?.message ?? ''));

    const inputs = state.pins.filter((pin) => pin.role === 'input');
    const outputs = state.pins.filter((pin) => pin.role === 'output');
    const internal = state.pins.filter((pin) => pin.role === 'internal');
    for (const block of [
        section('Inputs', inputs, true, build, target, 'This chip has no inputs.'),
        section('Outputs', outputs, false, build, target, 'This chip has no outputs.'),
        section(
            'Internal pins',
            internal,
            false,
            build,
            target,
            'None yet. Pins you create by connecting parts to each other appear here.',
        ),
    ]) {
        if (block) {
            root.append(block);
        }
    }
    let scroller: HTMLElement | null = null;
    if (state.clocked) {
        const block = clockHistory(state);
        scroller = block.querySelector('.history-scroll');
        root.append(block);
    }
    if (table && !state.clocked) {
        root.append(truthTable(state, table, build, target));
    }
    if (expected) {
        root.append(expectedTable(state, expected, build, target));
    }
    if (scroller) {
        // The newest step is the one to see.
        scroller.scrollTop = scroller.scrollHeight;
    }
    previous = new Map(state.pins.map((pin) => [pin.name, String(pin.bits)]));
    if (focused) {
        (root.querySelector(`[data-pin="${CSS.escape(focused)}"]`) as HTMLElement | null)?.focus();
    }
}

window.addEventListener('message', (event: MessageEvent<ToPanel>) => {
    const message = event.data;
    if (message.type === 'state') {
        const switched = currentTarget !== 0 && currentTarget !== message.target;
        currentTarget = message.target;
        currentBuild = message.build;
        if (switched) {
            previous = new Map();
            clearAllFeedback();
        }
        if (message.state) {
            if (current && current.chip !== message.state.chip) {
                previous = new Map();
                clearAllFeedback();
            }
            current = message.state;
            table = message.table;
            expected = message.expected;
            test = message.test;
            history = message.history;
            historyCapped = message.historyCapped;
        } else {
            // The panel moved to another chip that has not loaded: the old
            // chip's pins must not stay on screen under the new one's name.
            current = null;
            table = null;
            expected = null;
            test = null;
            history = [];
            historyCapped = false;
            previous = new Map();
        }
        shownProblem = { problem: message.problem, retry: message.retry === true };
        render(shownProblem.problem, shownProblem.retry);
    } else if (message.type === 'rejected') {
        setRejection(message.pin, message.message);
    } else if (message.type === 'note') {
        setNote(message.message);
    }
});

send({ type: 'ready' });
