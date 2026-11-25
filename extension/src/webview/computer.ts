// The Hack computer panel (runs inside a VS Code webview): the screen, the
// keyboard, the registers and the memory of a running program. The extension
// owns the emulator; this file draws what it is sent and reports what the
// student does.

import type { MachineState } from '../runtime/protocol';
import { Radix, formatValue, hackKeyCode, keyName, typedValue } from '../values';

declare function acquireVsCodeApi(): {
    postMessage(message: unknown): void;
    getState(): any;
    setState(state: any): void;
};

export type ToComputer =
    | { type: 'state'; target: number; state: MachineState | null; running: boolean; problem: string | null; title: string }
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
    | { type: 'set'; target: string; address?: number; value: string };

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
        screen,
        keyboardHint,
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

function update(): void {
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
    registers.replaceChildren();
    for (const register of state.registers) {
        const row = element('div', { class: 'register', role: 'row', title: REGISTER_HINTS[register.name] ?? '' });
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
        registers.append(row);
    }

    if (state.ram) {
        addressInput.value = String(state.ram.start);
        memoryBody.replaceChildren();
        const a = state.registers.find((r) => r.name === 'A')?.value;
        state.ram.values.forEach((value, i) => {
            const address = state!.ram!.start + i;
            const name = symbolFor(address);
            const row = element('tr', { class: address === a ? 'pointed' : '' });
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
    target = message.target;
    rejected.textContent = '';
    title.textContent = message.title;
    running = message.running;
    problem = message.problem;
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
