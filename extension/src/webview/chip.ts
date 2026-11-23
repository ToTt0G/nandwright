// The chip simulator panel (runs inside a VS Code webview).
//
// It shows one chip: its inputs, which can be changed, and its outputs and
// internal pins, which follow. The extension owns the simulation; this file
// only draws the state it is sent and reports what the student does.

import type { ChipState, Pin, TruthTable } from '../runtime/protocol';
import { Radix, formatValue, typedValue } from '../values';

declare function acquireVsCodeApi(): {
    postMessage(message: unknown): void;
    getState(): any;
    setState(state: any): void;
};

export type ToPanel =
    | { type: 'state'; state: ChipState | null; table: TruthTable | null; problem: string | null }
    | { type: 'rejected'; pin: string; message: string };

export type FromPanel =
    | { type: 'ready' }
    | { type: 'set'; pin: string; value: string | number }
    | { type: 'setAll'; values: Record<string, number> }
    | { type: 'action'; action: 'eval' | 'tick' | 'tock' | 'step' | 'reset' };

const vscode = acquireVsCodeApi();
const send = (message: FromPanel) => vscode.postMessage(message);

let radix: Radix = vscode.getState()?.radix ?? 'dec';
let current: ChipState | null = null;
let table: TruthTable | null = null;
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
        } else {
            // The panel moved to another chip that has not loaded: the old
            // chip's pins must not stay on screen under the new one's name.
            current = null;
            table = null;
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
