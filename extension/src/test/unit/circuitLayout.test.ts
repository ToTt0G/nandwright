// The circuit diagram's layout: deterministic, nothing overlapping, every
// connection drawn to the right pin, cycles and slices handled, fast enough.

import assert from 'node:assert/strict';
import { test } from 'node:test';
import { FIXTURES, fixtureDiagram, wideChip } from '../circuitFixtures';
import { Layout, layoutDiagram, MAX_DRAWN_PARTS, maxOf, minOf, Point, valuesFit } from '../../webview/circuitLayout';
import { symbolFor } from '../../webview/circuitSymbols';
import type { Diagram } from '../../webview/circuitTypes';

type Rect = { x: number; y: number; w: number; h: number; what: string };

function boxes(layout: Layout): Rect[] {
    return [
        ...layout.parts.map((p) => ({ x: p.x, y: p.y, w: p.w, h: p.h, what: `part ${p.index} ${p.chip}` })),
        ...layout.inputs.map((p) => ({ x: p.x, y: p.y, w: p.w, h: p.h, what: `input ${p.name}` })),
        ...layout.outputs.map((p) => ({ x: p.x, y: p.y, w: p.w, h: p.h, what: `output ${p.name}` })),
    ];
}

type Segment = { a: Point; b: Point; net: number };

function segments(layout: Layout): Segment[] {
    const out: Segment[] = [];
    for (const wire of layout.wires) {
        for (let i = 1; i < wire.points.length; i++) {
            out.push({ a: wire.points[i - 1], b: wire.points[i], net: wire.net });
        }
    }
    return out;
}

const near = (a: number, b: number) => Math.abs(a - b) < 1e-6;
const same = (p: Point, q: Point) => near(p.x, q.x) && near(p.y, q.y);

function onSegment(p: Point, s: Segment): boolean {
    const minX = Math.min(s.a.x, s.b.x);
    const maxX = Math.max(s.a.x, s.b.x);
    const minY = Math.min(s.a.y, s.b.y);
    const maxY = Math.max(s.a.y, s.b.y);
    if (near(s.a.x, s.b.x)) {
        return near(p.x, s.a.x) && p.y >= minY - 1e-6 && p.y <= maxY + 1e-6;
    }
    if (near(s.a.y, s.b.y)) {
        return near(p.y, s.a.y) && p.x >= minX - 1e-6 && p.x <= maxX + 1e-6;
    }
    return false;
}

/** Whether two points are joined by the wires of one net. */
function connected(layout: Layout, net: number, from: Point, to: Point): boolean {
    const segs = segments(layout).filter((s) => s.net === net);
    const touches = (a: Segment, b: Segment) => onSegment(a.a, b) || onSegment(a.b, b) || onSegment(b.a, a) || onSegment(b.b, a);
    const reached = new Set<number>();
    const queue: number[] = [];
    segs.forEach((s, i) => {
        if (onSegment(from, s)) {
            reached.add(i);
            queue.push(i);
        }
    });
    while (queue.length) {
        const s = segs[queue.pop()!];
        if (onSegment(to, s)) {
            return true;
        }
        segs.forEach((other, j) => {
            if (!reached.has(j) && touches(s, other)) {
                reached.add(j);
                queue.push(j);
            }
        });
    }
    return false;
}

function checkGeometry(diagram: Diagram, layout: Layout): void {
    // No two boxes overlap.
    const rects = boxes(layout);
    for (let i = 0; i < rects.length; i++) {
        for (let j = i + 1; j < rects.length; j++) {
            const a = rects[i];
            const b = rects[j];
            const overlap = a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h;
            assert.ok(!overlap, `${diagram.chip}: ${a.what} overlaps ${b.what}`);
        }
    }
    // Wires are horizontal or vertical, and never cross the inside of a box.
    for (const s of segments(layout)) {
        assert.ok(near(s.a.x, s.b.x) || near(s.a.y, s.b.y), `${diagram.chip}: a slanted wire`);
        for (const r of rects) {
            const x0 = Math.min(s.a.x, s.b.x);
            const x1 = Math.max(s.a.x, s.b.x);
            const y0 = Math.min(s.a.y, s.b.y);
            const y1 = Math.max(s.a.y, s.b.y);
            const inside = x0 < r.x + r.w - 0.5 && x1 > r.x + 0.5 && y0 < r.y + r.h - 0.5 && y1 > r.y + 0.5;
            assert.ok(!inside, `${diagram.chip}: a wire of ${diagram.nets[s.net].name} runs through ${r.what} (${x0},${y0})-(${x1},${y1})`);
        }
    }
    // Wires of different nets never run on top of each other, except where
    // one pin drives both (`out=a, out=b`): they leave it as one wire.
    const shared = new Set<string>();
    for (const part of diagram.parts) {
        for (const a of part.conns) {
            for (const b of part.conns) {
                if (a.dir === 'out' && b.dir === 'out' && a.pin === b.pin && a.pinBits.join() === b.pinBits.join()) {
                    shared.add(`${a.net}:${b.net}`);
                }
            }
        }
    }
    const segs = segments(layout);
    for (let i = 0; i < segs.length; i++) {
        for (let j = i + 1; j < segs.length; j++) {
            const a = segs[i];
            const b = segs[j];
            if (a.net === b.net || shared.has(`${a.net}:${b.net}`)) {
                continue;
            }
            const horizontal = near(a.a.y, a.b.y) && near(b.a.y, b.b.y) && near(a.a.y, b.a.y) && !near(a.a.x, a.b.x) && !near(b.a.x, b.b.x);
            const vertical = near(a.a.x, a.b.x) && near(b.a.x, b.b.x) && near(a.a.x, b.a.x) && !near(a.a.y, a.b.y) && !near(b.a.y, b.b.y);
            if (horizontal) {
                const overlap = Math.min(Math.max(a.a.x, a.b.x), Math.max(b.a.x, b.b.x)) - Math.max(Math.min(a.a.x, a.b.x), Math.min(b.a.x, b.b.x));
                assert.ok(overlap <= 1e-6, `${diagram.chip}: ${diagram.nets[a.net].name} and ${diagram.nets[b.net].name} overlap at y=${a.a.y}`);
            }
            if (vertical) {
                const overlap = Math.min(Math.max(a.a.y, a.b.y), Math.max(b.a.y, b.b.y)) - Math.max(Math.min(a.a.y, a.b.y), Math.min(b.a.y, b.b.y));
                assert.ok(overlap <= 1e-6, `${diagram.chip}: ${diagram.nets[a.net].name} and ${diagram.nets[b.net].name} overlap at x=${a.a.x}`);
            }
        }
    }
    // Every connection to a net is drawn from a pin of its driver to the right pin.
    const driverAnchors = (net: number): Point[] => {
        const anchors: Point[] = [];
        layout.inputs.forEach((port) => port.net === net && anchors.push(...port.anchors));
        diagram.parts.forEach((part, p) => {
            const geom = layout.parts.find((g) => g.index === p)!;
            // One row per distinct slice of an output pin: `out=a, out=b` share one.
            const rows = new Map<number, string[]>();
            for (const conn of part.conns) {
                if (conn.dir === 'out') {
                    const keys = rows.get(conn.pin) ?? [];
                    rows.set(conn.pin, keys);
                    const key = conn.pinBits.join(',');
                    let row = keys.indexOf(key);
                    if (row < 0) {
                        row = keys.push(key) - 1;
                    }
                    if (conn.net === net) {
                        anchors.push(geom.outputs[conn.pin].anchors[row]);
                    }
                }
            }
        });
        return anchors;
    };
    diagram.parts.forEach((part, p) => {
        const geom = layout.parts.find((g) => g.index === p)!;
        const rows = new Map<number, number>();
        for (const conn of part.conns) {
            if (conn.dir !== 'in') {
                continue;
            }
            const row = rows.get(conn.pin) ?? 0;
            rows.set(conn.pin, row + 1);
            const anchor = geom.inputs[conn.pin].anchors[row];
            assert.ok(anchor, `${diagram.chip}: ${part.chip}.${part.inputs[conn.pin].name} has a row for each connection`);
            if (conn.net === undefined) {
                assert.ok(
                    layout.ties.some((t) => t.part === p && t.pin === conn.pin && same({ x: t.x, y: t.y }, anchor)),
                    `${diagram.chip}: ${part.chip}.${part.inputs[conn.pin].name} has its tie-off`,
                );
                continue;
            }
            const ends = layout.wires.filter((w) => w.net === conn.net && same(w.points[w.points.length - 1], anchor));
            assert.equal(ends.length, 1, `${diagram.chip}: one wire ends on ${part.chip}(part ${p}).${part.inputs[conn.pin].name}`);
            assert.ok(
                driverAnchors(conn.net).some((d) => connected(layout, conn.net!, d, anchor)),
                `${diagram.chip}: ${diagram.nets[conn.net].name} reaches ${part.chip}(part ${p}).${part.inputs[conn.pin].name}`,
            );
        }
    });
    layout.outputs.forEach((port) => {
        if (driverAnchors(port.net).length) {
            assert.ok(
                port.anchors.every((a) => driverAnchors(port.net).some((d) => connected(layout, port.net, d, a))),
                `${diagram.chip}: output ${port.name} is wired`,
            );
        }
    });
    // Inputs on the left edge, outputs on the right.
    const left = Math.min(...rects.map((r) => r.x));
    const right = Math.max(...rects.map((r) => r.x + r.w));
    for (const port of layout.inputs) {
        assert.ok(port.x + port.w <= Math.min(...layout.parts.map((p) => p.x), Infinity) + 1e-6, `${diagram.chip}: input ${port.name} is left of every part`);
        assert.ok(port.x >= left - 1e-6);
    }
    for (const port of layout.outputs) {
        assert.ok(port.x >= Math.max(...layout.parts.map((p) => p.x + p.w), -Infinity) - 1e-6, `${diagram.chip}: output ${port.name} is right of every part`);
        assert.ok(port.x + port.w <= right + 1e-6);
    }
    assert.ok(layout.width >= right && layout.height >= Math.max(...rects.map((r) => r.y + r.h)), `${diagram.chip}: the bounds hold everything`);
}

for (const name of FIXTURES) {
    test(`${name}: nothing overlaps and every connection is drawn`, () => {
        const diagram = fixtureDiagram(name);
        checkGeometry(diagram, layoutDiagram(diagram));
        checkGeometry(diagram, layoutDiagram(diagram, { radix: 'bin' }));
    });
}

test('the same chip always looks the same', () => {
    for (const name of [...FIXTURES, wideChip(60)]) {
        const a = JSON.stringify(layoutDiagram(fixtureDiagram(name)));
        const b = JSON.stringify(layoutDiagram(fixtureDiagram(name)));
        assert.equal(a, b, name);
    }
    // Values do not move anything.
    const quiet = layoutDiagram(fixtureDiagram('VoteCheck'));
    const busy = layoutDiagram(fixtureDiagram('VoteCheck', { a: 1, b: 1, word: 1234, pick: 2 }));
    assert.deepEqual(busy, quiet);
});

test('a latch of two Nands: the loop is cut and drawn back', () => {
    const diagram = fixtureDiagram('SetReset');
    const layout = layoutDiagram(diagram);
    checkGeometry(diagram, layout);
    const backwards = layout.wires.filter((w) => w.points.some((p, i) => i > 0 && p.y === w.points[i - 1].y && p.x < w.points[i - 1].x - 30));
    assert.ok(backwards.length > 0, 'a wire runs back to the left');
    const nands = layout.parts.filter((p) => p.chip === 'Nand');
    assert.equal(nands.length, 2);
    assert.notEqual(nands[0].layer, nands[1].layer, 'the two Nands are one after the other');
});

test('a DFF in a feedback loop: the loop is cut at its input', () => {
    const diagram = fixtureDiagram('Toggle');
    const layout = layoutDiagram(diagram);
    checkGeometry(diagram, layout);
    const dff = layout.parts.find((p) => p.chip === 'DFF')!;
    const xor = layout.parts.find((p) => p.chip === 'Xor')!;
    assert.ok(dff.layer < xor.layer, 'the DFF comes first; its input wire runs back');
    assert.ok(dff.sequential);

    const acc = fixtureDiagram('Accumulator');
    const accLayout = layoutDiagram(acc);
    checkGeometry(acc, accLayout);
    const register = accLayout.parts.find((p) => p.chip === 'Register')!;
    assert.ok(accLayout.parts.every((p) => p === register || p.layer > register.layer), 'the Register is first');
});

test('fan-out is one trunk with junction dots', () => {
    const diagram = fixtureDiagram('Majority');
    const layout = layoutDiagram(diagram);
    for (const name of ['a', 'b', 'c']) {
        const net = diagram.nets.findIndex((n) => n.name === name);
        assert.ok(layout.junctions.some((j) => j.net === net), `${name} has a junction`);
        assert.ok(layout.wires.filter((w) => w.net === net && w.kind === 'stub').length <= 1, `${name} leaves its port once`);
    }
});

test('an output pin that feeds several nets is one pin, and its wire fans out', () => {
    const diagram = fixtureDiagram('VoteCheck');
    const layout = layoutDiagram(diagram);
    checkGeometry(diagram, layout);
    const index = diagram.parts.findIndex((p) => p.chip === 'Majority');
    const majority = layout.parts.find((p) => p.index === index)!;
    assert.equal(majority.outputs[0].anchors.length, 1, 'out=vote, out=v: one row on the pin');
    const from = majority.outputs[0].anchors[0];
    const vote = diagram.nets.findIndex((n) => n.name === 'vote');
    const v = diagram.nets.findIndex((n) => n.name === 'v');
    for (const net of [vote, v]) {
        assert.ok(layout.wires.some((w) => w.net === net && same(w.points[0], from)), `${diagram.nets[net].name} leaves the pin`);
    }
    assert.ok(
        layout.junctions.some((j) => near(j.y, from.y) && j.x > from.x && (j.net === vote || j.net === v)),
        'a junction dot where the two wires part',
    );
    // Different slices of one pin keep their own rows.
    const mux = layout.parts.find((p) => p.chip === 'Mux4Way16')!;
    assert.equal(mux.outputs[0].anchors.length, 2, 'out=chosen, out[0..3]=low: two rows');
});

test('bit slices are labelled and carry their own bits', () => {
    const parity = fixtureDiagram('Parity4');
    const layout = layoutDiagram(parity);
    for (const label of ['[0]', '[1]', '[2]', '[3]']) {
        assert.ok(layout.labels.some((l) => l.kind === 'slice' && l.text === label), label);
    }
    const one = layout.wires.filter((w) => w.bits && w.bits[1] === 1);
    assert.ok(one.length >= 4, 'the wires into the Xors are 1 bit wide');
    assert.ok(layout.marks.some((m) => m.width === 4), 'the bus has a /4 mark');

    const swap = fixtureDiagram('SwapBytes');
    const swapped = layoutDiagram(swap);
    assert.ok(swapped.labels.some((l) => l.text === '[8..15]→[0..7]'));
    assert.ok(swapped.labels.some((l) => l.text === '[0..7]→[8..15]'));
    const or16 = swapped.parts[0];
    assert.equal(or16.inputs[0].anchors.length, 2, 'two connections on one pin get two rows');
    assert.equal(swapped.ties.length, 1, 'b=false is a tie-off');
    assert.equal(swapped.ties[0].value, false);
});

test('two levels: composite parts can be opened', () => {
    const diagram = fixtureDiagram('VoteCheck');
    const layout = layoutDiagram(diagram);
    checkGeometry(diagram, layout);
    const majority = layout.parts.find((p) => p.chip === 'Majority')!;
    assert.ok(majority.drill && majority.shape === 'box');
    assert.equal(layout.parts.find((p) => p.chip === 'Mux4Way16')!.shape, 'mux');
    assert.equal(layout.parts.find((p) => p.chip === 'Not16')!.variant, 'Not16');
    assert.deepEqual(new Set(layout.tabOrder), new Set(diagram.parts.map((_, i) => i)));
});

test('gate shapes by chip name, boxes when the pins do not fit', () => {
    const pin = (name: string, width = 1) => ({ name, width });
    assert.equal(symbolFor({ chip: 'Nand', inputs: [pin('a'), pin('b')], outputs: [pin('out')] }).shape, 'nand');
    assert.equal(symbolFor({ chip: 'Or8Way', inputs: [pin('in', 8)], outputs: [pin('out')] }).variant, 'Or8Way');
    assert.equal(symbolFor({ chip: 'DMux4Way', inputs: [pin('in'), pin('sel', 2)], outputs: ['a', 'b', 'c', 'd'].map((n) => pin(n)) }).shape, 'dmux');
    assert.equal(symbolFor({ chip: 'And', inputs: [pin('a'), pin('b'), pin('c')], outputs: [pin('out')] }).shape, 'box');
    assert.equal(symbolFor({ chip: 'Register', inputs: [pin('in', 16), pin('load')], outputs: [pin('out', 16)] }).shape, 'box');
    assert.equal(symbolFor({ chip: 'Andromeda', inputs: [pin('a'), pin('b')], outputs: [pin('out')] }).shape, 'box');
});

test('a 60-part chip lays out in well under 100 ms', () => {
    const diagram = fixtureDiagram(wideChip(60));
    assert.ok(diagram.parts.length >= 60);
    const layout = layoutDiagram(diagram);
    checkGeometry(diagram, layout);
    const runs = 5;
    const started = performance.now();
    for (let i = 0; i < runs; i++) {
        layoutDiagram(diagram);
    }
    const each = (performance.now() - started) / runs;
    assert.ok(each < 60, `took ${each.toFixed(1)} ms`);
});

test('each row of a pin knows its own bits, so its lead shows only those', () => {
    const values = { time: '', clocked: false, nets: [0, 255, 255], parts: [{ inputs: [65280, 65535], outputs: [65280] }] };
    const diagram: Diagram = {
        path: [],
        chip: 'HalfMask',
        file: null,
        breadcrumb: [],
        inputs: [
            { name: 'lo', width: 8 },
            { name: 'hi', width: 8 },
        ],
        outputs: [{ name: 'out', width: 16 }],
        nets: [
            { name: 'lo', width: 8, kind: 'input' },
            { name: 'hi', width: 8, kind: 'input' },
            { name: 'out', width: 16, kind: 'output' },
        ],
        parts: [
            {
                chip: 'And16',
                drill: false,
                builtin: true,
                sequential: false,
                span: null,
                inputs: [
                    { name: 'a', width: 16 },
                    { name: 'b', width: 16 },
                ],
                outputs: [{ name: 'out', width: 16 }],
                deps: [[0, 1]],
                conns: [
                    { dir: 'in', pin: 0, pinBits: [0, 8], net: 0, netBits: [0, 8] },
                    { dir: 'in', pin: 0, pinBits: [8, 8], net: 1, netBits: [0, 8] },
                    { dir: 'in', pin: 1, pinBits: [0, 16], constant: true },
                    { dir: 'out', pin: 0, pinBits: [0, 16], net: 2, netBits: [0, 16] },
                ],
            },
        ],
        values,
    };
    const layout = layoutDiagram(diagram);
    const and16 = layout.parts[0];
    assert.deepEqual(and16.inputs[0].slices, [
        [0, 8],
        [8, 8],
    ]);
    assert.deepEqual(and16.inputs[1].slices, [null]);
    assert.deepEqual(and16.outputs[0].slices, [null]);
    assert.equal(and16.inputs[0].slices.length, and16.inputs[0].anchors.length);
});

/** `n` Not gates side by side, all fed by one input: one very tall layer. */
function manyNots(n: number): Diagram {
    const parts: Diagram['parts'] = [];
    const nets: Diagram['nets'] = [{ name: 'in', width: 1, kind: 'input' }];
    for (let i = 0; i < n; i++) {
        nets.push({ name: `n${i}`, width: 1, kind: 'internal' });
        parts.push({
            chip: 'Not',
            drill: false,
            builtin: true,
            sequential: false,
            span: null,
            inputs: [{ name: 'in', width: 1 }],
            outputs: [{ name: 'out', width: 1 }],
            deps: [[0]],
            conns: [
                { dir: 'in', pin: 0, pinBits: [0, 1], net: 0, netBits: [0, 1] },
                { dir: 'out', pin: 0, pinBits: [0, 1], net: i + 1, netBits: [0, 1] },
            ],
        });
    }
    return {
        path: [],
        chip: 'Many',
        file: null,
        breadcrumb: [],
        inputs: [{ name: 'in', width: 1 }],
        outputs: [],
        nets,
        parts,
        values: { time: '', clocked: false, nets: nets.map(() => 0), parts: parts.map(() => ({ inputs: [0], outputs: [1] })) },
    };
}

test('the extremes of very long lists are found without spreading them', () => {
    const big = Array.from({ length: 300_000 }, (_, i) => i % 1000);
    // `Math.max(...big)` throws RangeError in V8 at this size.
    assert.equal(maxOf(big, (x) => x, 0), 999);
    assert.equal(minOf(big, (x) => x, Infinity), 0);
    assert.equal(maxOf([], (x: number) => x, 7), 7);
});

test('a level at the drawing limit still lays out', () => {
    const diagram = manyNots(MAX_DRAWN_PARTS);
    const layout = layoutDiagram(diagram);
    assert.equal(layout.parts.length, MAX_DRAWN_PARTS);
    assert.ok(Number.isFinite(layout.width) && Number.isFinite(layout.height));
});

test('values fit a diagram only when they are of the same build and level', () => {
    const diagram = { ...fixtureDiagram('Majority'), revision: 3 };
    const values = { ...diagram.values, revision: 3, path: [] as number[] };
    assert.ok(valuesFit(diagram, values));
    assert.equal(valuesFit(diagram, { ...values, revision: 4 }), false, 'another build of the chip');
    assert.equal(valuesFit(diagram, { ...values, path: [0] }), false, 'another level');
    assert.equal(valuesFit(diagram, { ...values, nets: values.nets.slice(1) }), false, 'other nets');
    // Values without a revision or path (the preview's fake simulator) are judged by their size.
    assert.ok(valuesFit(diagram, diagram.values));
});
