// Runs the real bundled chip simulator webview (`dist-test/webview/chip.js`)
// in a minimal DOM harness to verify build/target tagging (F1) and persistent
// rejected-input feedback for both 1-bit and bus pins (F7).

import assert from 'node:assert/strict';
import * as fs from 'node:fs';
import * as path from 'node:path';
import { test } from 'node:test';
import * as vm from 'node:vm';
import type { FromPanel, ToPanel } from '../../webview/chip';

class FakeElement {
    className = '';
    value = '';
    scrollTop = 0;
    scrollHeight = 0;
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
            handler({ preventDefault: () => undefined, ...event });
        }
    }

    focus(): void {
        // no-op
    }

    /** Supports the selectors chip.ts and these tests query: `.cls`, `[attr="val"]`, `.cls[attr="val"]`, `tag.cls`, `tbody tr`. */
    querySelectorAll(selector: string): FakeElement[] {
        const parts = selector.trim().split(/\s+/);
        let pool: FakeElement[] = this.descendants();
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

function mountChipWebview() {
    const bundle = fs.readFileSync(path.join(__dirname, '..', 'webview', 'chip.js'), 'utf8');
    const root = new FakeElement('main');
    const sent: FromPanel[] = [];
    const windowListeners = new Map<string, ((event: any) => void)[]>();
    let now = 0;
    let nextTimer = 1;
    const timers = new Map<number, { at: number; fn: () => void }>();

    const context = vm.createContext({
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
        CSS: { escape: (s: string) => s },
        acquireVsCodeApi: () => ({
            postMessage: (m: FromPanel) => sent.push(JSON.parse(JSON.stringify(m))),
            getState: () => undefined,
            setState: () => undefined,
        }),
        setTimeout: (fn: () => void, ms: number) => {
            const id = nextTimer++;
            timers.set(id, { at: now + ms, fn });
            return id;
        },
        clearTimeout: (id: number) => {
            timers.delete(id);
        },
    });
    vm.runInContext(bundle, context);

    const deliver = (message: ToPanel) => {
        for (const fn of windowListeners.get('message') ?? []) {
            fn({ data: message });
        }
    };
    const advance = (ms: number) => {
        now += ms;
        for (const [id, entry] of [...timers].sort((a, b) => a[1].at - b[1].at)) {
            if (entry.at <= now && timers.has(id)) {
                timers.delete(id);
                entry.fn();
            }
        }
    };
    return { root, sent, deliver, advance };
}

function sampleState(overrides: Partial<Extract<ToPanel, { type: 'state' }>> = {}): Extract<ToPanel, { type: 'state' }> {
    return {
        type: 'state',
        state: {
            chip: 'Sample',
            clocked: false,
            time: '',
            revision: 1,
            pins: [
                { name: 'a', role: 'input', width: 1, bits: 0 },
                { name: 'bus', role: 'input', width: 16, bits: 0 },
                { name: 'out', role: 'output', width: 1, bits: 0 },
            ],
        },
        build: { session: 1, revision: 1 },
        target: 7,
        table: {
            inputs: [{ name: 'a', width: 1 }],
            outputs: [{ name: 'out', width: 1 }],
            rows: [
                { inputs: [0], outputs: [0] },
                { inputs: [1], outputs: [1] },
            ],
        },
        problem: null,
        expected: null,
        test: null,
        history: [],
        historyCapped: false,
        ...overrides,
    };
}

test('F1: chip webview tags set, setAll and actions with the rendered build and file-target token', () => {
    const { root, sent, deliver } = mountChipWebview();
    assert.deepEqual(sent, [{ type: 'ready' }]);
    sent.length = 0;

    deliver(sampleState({ build: { session: 3, revision: 2 }, target: 9 }));

    // 1-bit input click carries build + target.
    const toggle = root.querySelector('button.bit')!;
    assert.ok(toggle);
    toggle.dispatch('click');
    assert.deepEqual(sent.pop(), { type: 'set', pin: 'a', value: 1, build: { session: 3, revision: 2 }, target: 9 });

    // Bus input edit carries build + target.
    const bus = root.querySelector('input.bus')!;
    bus.value = '42';
    bus.dispatch('keydown', { key: 'Enter' });
    assert.deepEqual(sent.pop(), { type: 'set', pin: 'bus', value: '42', build: { session: 3, revision: 2 }, target: 9 });

    // Truth-table row click carries build + target.
    const row = root.querySelectorAll('tbody tr')[1]!;
    row.dispatch('click');
    assert.deepEqual(sent.pop(), { type: 'setAll', values: { a: 1 }, build: { session: 3, revision: 2 }, target: 9 });

    // Reset button carries target (survives rebuilds, dropped on chip switch).
    const reset = root.querySelectorAll('button.secondary').find((b) => b.textContent === 'Reset')!;
    assert.ok(reset);
    reset.dispatch('click');
    assert.deepEqual(sent.pop(), { type: 'action', action: 'reset', target: 9 });
});

test('F7: rejected-input feedback survives redraws for both 1-bit and bus pins, and clears on edit or timeout', () => {
    const { root, deliver, advance } = mountChipWebview();
    const state = sampleState();
    deliver(state);

    // Reject both a 1-bit pin and a 16-bit bus pin, then immediately redraw with `state`
    // (exactly what ChipPanel.hubEvent does on every failed `set`).
    deliver({ type: 'rejected', pin: 'a', message: 'The chip was built again since; set the input again.' });
    deliver({ type: 'rejected', pin: 'bus', message: 'value 99999 does not fit in 16 bits' });
    deliver(state);

    const bitSlot = () => root.querySelector('.rejected[data-for="a"]');
    const busSlot = () => root.querySelector('.rejected[data-for="bus"]');
    assert.equal(bitSlot()?.textContent, 'The chip was built again since; set the input again.', '1-bit pin has a slot and keeps rejection across redraw');
    assert.equal(busSlot()?.textContent, 'value 99999 does not fit in 16 bits', 'bus pin keeps rejection across redraw');

    // Editing the 1-bit pin clears its own rejection without clearing the bus pin's.
    root.querySelector('button.bit')!.dispatch('click');
    deliver(state);
    assert.equal(bitSlot()?.textContent, '');
    assert.equal(busSlot()?.textContent, 'value 99999 does not fit in 16 bits');

    // Typing in the bus pin clears its rejection too.
    root.querySelector('input.bus')!.dispatch('input');
    assert.equal(busSlot()?.textContent, '');

    // Reject again and verify timeout clears it after a few seconds.
    deliver({ type: 'rejected', pin: 'a', message: 'rebuilt' });
    deliver(state);
    assert.equal(bitSlot()?.textContent, 'rebuilt');
    advance(6000);
    assert.equal(bitSlot()?.textContent, '');

    // Switching to another chip clears active rejection feedback immediately.
    deliver({ type: 'rejected', pin: 'bus', message: 'bad' });
    deliver(sampleState({ target: 8 }));
    assert.equal(busSlot()?.textContent, '');
});
