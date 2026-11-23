// The chip simulator panel (runs inside a VS Code webview).
//
// It shows one chip: its inputs, which can be changed, and its outputs and
// internal pins, which follow. The extension owns the simulation; this file
// only draws the state it is sent and reports what the student does.

import type { ChipState, ChipTest, Expected, Pin, TruthTable } from '../runtime/protocol';
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
          table: TruthTable | null;
          problem: string | null;
          expected: Expected | null;
          test: ChipTest | null;
      }
    | { type: 'rejected'; pin: string; message: string };

export type FromPanel =
    | { type: 'ready' }
    | { type: 'openExpected' }
    | { type: 'runTest' }
    | { type: 'set'; pin: string; value: string | number }
    | { type: 'setAll'; values: Record<string, number> }
    | { type: 'action'; action: 'eval' | 'tick' | 'tock' | 'step' | 'reset' };

const vscode = acquireVsCodeApi();
const send = (message: FromPanel) => vscode.postMessage(message);

let radix: Radix = vscode.getState()?.radix ?? 'dec';
let current: ChipState | null = null;
let table: TruthTable | null = null;
let expected: Expected | null = null;
let test: ChipTest | null = null;
let previous = new Map<string, string>();

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

function pinRow(pin: Pin, editable: boolean): HTMLElement {
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
        toggle.addEventListener('click', () => send({ type: 'set', pin: pin.name, value: on ? 0 : 1 }));
        row.append(element('span', { role: 'cell' }, toggle));
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
            if (text !== shown) {
                send({ type: 'set', pin: pin.name, value: typedValue(text, radix) });
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
        const cell = element('span', { role: 'cell', class: 'bus-cell' }, input);
        cell.append(element('span', { class: 'rejected', 'aria-live': 'polite', 'data-for': pin.name }));
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

function section(title: string, pins: Pin[], editable: boolean, emptyText?: string): HTMLElement | null {
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
    pins.forEach((pin) => grid.append(pinRow(pin, editable)));
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
            render(null);
        });
        group.append(button);
    }
    return group;
}

function actionButton(label: string, action: 'eval' | 'tick' | 'tock' | 'step' | 'reset', title: string, primary = false): HTMLElement {
    const button = element('button', { type: 'button', class: primary ? 'primary' : 'secondary', title }, label);
    button.addEventListener('click', () => send({ type: 'action', action }));
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
    const compared = Math.max(0, (run.comparedLines ?? 0) - 1);
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
            return { text: `${run.script} fails: ${run.failureCount ?? 0} of ${compared} rows differ.`, fails: true };
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

function expectedTable(state: ChipState, file: Expected): HTMLElement {
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
            const apply = () => send({ type: 'setAll', values });
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

function truthTable(state: ChipState, truth: TruthTable): HTMLElement {
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
            const values: Record<string, number> = {};
            truth.inputs.forEach((pin, i) => (values[pin.name] = row.inputs[i]));
            send({ type: 'setAll', values });
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

function render(problem: string | null): void {
    const focused = (document.activeElement as HTMLElement | null)?.getAttribute('data-pin');
    root.replaceChildren();
    if (problem) {
        const banner = element('div', { class: 'problem', role: 'alert' });
        banner.append(element('strong', {}, current ? 'The chip has an error. Showing the last version that loaded.' : 'This chip cannot be simulated yet.'));
        banner.append(element('pre', {}, problem));
        root.append(banner);
    }
    if (!current) {
        if (!problem) {
            root.append(element('p', { class: 'hint' }, 'Loading the chip…'));
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
            actionButton('Tick', 'tick', 'Raise the clock: clocked chips read their inputs', !high),
            actionButton('Tock', 'tock', 'Lower the clock: clocked chips show their new outputs', high),
            actionButton('Full cycle', 'step', 'Tick, then tock'),
        );
    }
    toolbar.append(actionButton('Reset', 'reset', 'Reload the chip and clear its state. Inputs are kept.'));
    toolbar.append(element('span', { class: 'spacer' }));
    toolbar.append(radixPicker());
    root.append(toolbar);

    const inputs = state.pins.filter((pin) => pin.role === 'input');
    const outputs = state.pins.filter((pin) => pin.role === 'output');
    const internal = state.pins.filter((pin) => pin.role === 'internal');
    for (const block of [
        section('Inputs', inputs, true, 'This chip has no inputs.'),
        section('Outputs', outputs, false, 'This chip has no outputs.'),
        section(
            'Internal pins',
            internal,
            false,
            'None yet. Pins you create by connecting parts to each other appear here.',
        ),
    ]) {
        if (block) {
            root.append(block);
        }
    }
    if (table && !state.clocked) {
        root.append(truthTable(state, table));
    }
    if (expected) {
        root.append(expectedTable(state, expected));
    }
    previous = new Map(state.pins.map((pin) => [pin.name, String(pin.bits)]));
    if (focused) {
        (root.querySelector(`[data-pin="${CSS.escape(focused)}"]`) as HTMLElement | null)?.focus();
    }
}

window.addEventListener('message', (event: MessageEvent<ToPanel>) => {
    const message = event.data;
    if (message.type === 'state') {
        if (message.state) {
            if (current && current.chip !== message.state.chip) {
                previous = new Map();
            }
            current = message.state;
            table = message.table;
            expected = message.expected;
            test = message.test;
        } else {
            // The panel moved to another chip that has not loaded: the old
            // chip's pins must not stay on screen under the new one's name.
            current = null;
            table = null;
            expected = null;
            test = null;
            previous = new Map();
        }
        render(message.problem);
    } else if (message.type === 'rejected') {
        const slot = root.querySelector(`.rejected[data-for="${CSS.escape(message.pin)}"]`);
        if (slot) {
            slot.textContent = message.message;
        }
    }
});

send({ type: 'ready' });
