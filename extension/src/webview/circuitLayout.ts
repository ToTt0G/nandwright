// Layout of a circuit diagram: where every part, port and wire goes. Pure (no
// DOM), deterministic, unit-tested.
//
// A layered ("Sugiyama") drawing:
//   1. a graph of the chip's input ports, parts and output ports, with one
//      edge per driver → reader connection;
//   2. cycles are broken: first at the inputs of sequential parts (a DFF's
//      input does not feed its output in the same step), then at the back
//      edges of a depth-first search;
//   3. layers by longest path from the inputs: input ports in layer 0,
//      output ports in the last;
//   4. wires that skip layers (or go back) get a placeholder ("dummy") in
//      every layer they pass, so they run between parts, never through them;
//   5. the order within each layer by barycenter sweeps, keeping the best;
//   6. vertical positions by repeated averaging over the neighbours, with the
//      order and the gaps kept (isotonic regression), snapped to a grid so
//      that pins meant to line up give straight wires;
//   7. wires: in each channel between two layers, every net that turns gets
//      a vertical track of its own; tracks are assigned so that no two
//      vertical runs and no two horizontal stubs overlap. A net that fans out
//      is one trunk with branches and junction dots. A pin that drives several
//      nets (`out=a, out=b`) is one pin; their stubs leave it together.

import type { BitRange, Diagram, Part, PinDecl, Values } from './circuitTypes';
import type { Radix } from '../values';
import { gateWidth, Shape, symbolFor } from './circuitSymbols';

/** The grid: every pin and node top is a multiple of this. */
export const GRID = 9;
/** Distance between pins on a side. */
export const ROW = 18;
/** Distance between two vertical tracks in a channel. */
export const PITCH = 10;
/** Average width of a character of the diagram's (monospace) font. */
export const CHAR = 6.6;
const MARGIN = 36;
const HEADER = 27;

export interface Point {
    x: number;
    y: number;
}

export interface PinGeom {
    name: string;
    width: number;
    index: number;
    /** One anchor per connection on the pin (at least one), on the box edge. */
    anchors: Point[];
    /**
     * Per anchor: the bits of the pin that row carries (`a[0..7]=lo` →
     * `[0, 8]`), or null for the whole pin. A lead shows only its own bits.
     */
    slices: (BitRange | null)[];
}

export interface PartGeom {
    index: number;
    chip: string;
    shape: Shape;
    variant?: string;
    x: number;
    y: number;
    w: number;
    h: number;
    /** Height of the gate shape (without the variant label under it); `h` for boxes. */
    bodyH: number;
    layer: number;
    sequential: boolean;
    drill: boolean;
    inputs: PinGeom[];
    outputs: PinGeom[];
}

export interface PortGeom {
    side: 'in' | 'out';
    /** Index into `Diagram.inputs` or `Diagram.outputs`. */
    index: number;
    name: string;
    width: number;
    net: number;
    x: number;
    y: number;
    w: number;
    h: number;
    anchors: Point[];
}

export type WireKind = 'stub' | 'trunk' | 'branch' | 'through' | 'straight' | 'path';

export interface WireGeom {
    net: number;
    /** The bits of the net this piece carries, when only some of them (a branch to `x[3]`). */
    bits: BitRange | null;
    /** Width in bits of what the piece carries. */
    width: number;
    points: Point[];
    kind: WireKind;
    /** When the propagation animation reaches this piece, in half-layers. */
    t0: number;
    t1: number;
}

export interface JunctionGeom {
    x: number;
    y: number;
    net: number;
    bits: BitRange | null;
    width: number;
    t: number;
}

export interface LabelGeom {
    x: number;
    y: number;
    /** Fixed text, or the prefix of a value label. */
    text: string;
    kind: 'slice' | 'value';
    anchor: 'start' | 'end';
    net: number;
    bits: BitRange | null;
    width: number;
}

/** The `/16` mark across a bus. */
export interface MarkGeom {
    x: number;
    y: number;
    width: number;
    net: number;
}

/** A pin tied to `true` or `false`. */
export interface TieGeom {
    part: number;
    pin: number;
    x: number;
    y: number;
    value: boolean;
    width: number;
}

export interface Layout {
    width: number;
    height: number;
    layers: number;
    parts: PartGeom[];
    inputs: PortGeom[];
    outputs: PortGeom[];
    wires: WireGeom[];
    junctions: JunctionGeom[];
    labels: LabelGeom[];
    marks: MarkGeom[];
    ties: TieGeom[];
    /** Last animation time (half-layers). */
    tEnd: number;
    /** Parts in reading order (by layer, then top to bottom), for Tab. */
    tabOrder: number[];
}

export function textWidth(text: string, char = CHAR): number {
    return text.length * char;
}

const snapUp = (n: number) => Math.ceil(n / GRID) * GRID;
const whole = (bits: BitRange | undefined, width: number) => !bits || (bits[0] === 0 && bits[1] === width);

export function range(bits: BitRange): string {
    return bits[1] === 1 ? `[${bits[0]}]` : `[${bits[0]}..${bits[0] + bits[1] - 1}]`;
}

// ---- the graph ----------------------------------------------------------

type NodeKind = 'in' | 'part' | 'out';

interface Driver {
    id: number;
    node: Node;
    /** Pin on the node (part output index; 0 for an input port). */
    pin: number;
    /** Offset of the anchor from the node's top, and its x side. */
    off: number;
    net: number;
    netBits: BitRange;
    pinBits: BitRange;
    pinWidth: number;
    sinks: Sink[];
    conn: number;
}

interface Sink {
    node: Node;
    pin: number;
    off: number;
    driver: Driver;
    netBits: BitRange;
    pinBits: BitRange;
    pinWidth: number;
}

interface Node {
    id: number;
    kind: NodeKind;
    index: number;
    w: number;
    h: number;
    layer: number;
    item?: Item;
    // Filled for parts:
    part?: Part;
    shape?: Shape;
    variant?: string;
    bodyH?: number;
    /** Per input pin: anchor offsets (rows). */
    inRows: number[][];
    outRows: number[][];
    /** Per pin, per row: the pin bits of that row (null: the whole pin). */
    inSlices?: (BitRange | null)[][];
    outSlices?: (BitRange | null)[][];
}

interface Item {
    id: number;
    layer: number;
    node?: Node;
    h: number;
    w: number;
    top: number;
    order: number;
    fixed: boolean;
}

interface Link {
    a: Item;
    aOff: number;
    b: Item;
    bOff: number;
}

/** Lays out a chip's diagram. */
/**
 * Whether `values` are of `diagram`: the same build of the chip (revision)
 * and the same level (path), with a value for every net and part. Values of
 * another build or level would put their numbers on the wrong wires.
 */
export function valuesFit(diagram: Diagram, values: Values): boolean {
    if (values.revision !== undefined && diagram.revision !== undefined && values.revision !== diagram.revision) {
        return false;
    }
    if (values.path && (values.path.length !== diagram.path.length || values.path.some((p, i) => p !== diagram.path[i]))) {
        return false;
    }
    return values.nets.length === diagram.nets.length && values.parts.length === diagram.parts.length;
}

/**
 * The most parts one level may have and still be drawn. A student's CPU has
 * a few dozen; far beyond this the layout takes seconds and the drawing is
 * unreadable, so the panel shows a message instead.
 */
export const MAX_DRAWN_PARTS = 2000;

export function layoutDiagram(diagram: Diagram, options: { char?: number; radix?: Radix } = {}): Layout {
    const char = options.char ?? CHAR;
    const radix = options.radix ?? 'dec';
    const tw = (text: string) => textWidth(text, char);
    const portWidth = (pin: PinDecl): number => {
        const value = pin.width > 1 ? valueChars(pin.width, radix) * char + 12 : 18;
        return snapUp(tw(pin.name) + value + 26);
    };
    const nets = diagram.nets;

    // Nodes.
    const nodes: Node[] = [];
    const netOfInput = diagram.inputs.map((pin, i) => netIndex(diagram, pin, 'input', i));
    const netOfOutput = diagram.outputs.map((pin, i) => netIndex(diagram, pin, 'output', diagram.inputs.length + i));
    const inputNodes = diagram.inputs.map((pin, i) => {
        const node: Node = { id: nodes.length, kind: 'in', index: i, w: portWidth(pin), h: ROW, layer: 0, inRows: [], outRows: [[GRID]] };
        nodes.push(node);
        return node;
    });
    const partNodes = diagram.parts.map((part, i) => {
        const node: Node = { id: nodes.length, kind: 'part', index: i, w: 0, h: 0, layer: 1, part, inRows: [], outRows: [] };
        nodes.push(node);
        return node;
    });
    const outputNodes = diagram.outputs.map((pin, i) => {
        const node: Node = { id: nodes.length, kind: 'out', index: i, w: portWidth(pin), h: ROW, layer: 0, inRows: [[GRID]], outRows: [] };
        nodes.push(node);
        return node;
    });

    // Drivers of each net, in a fixed order: the input port, then part outputs.
    const drivers: Driver[] = [];
    const driversOf: Driver[][] = nets.map(() => []);
    inputNodes.forEach((node, i) => {
        const net = netOfInput[i];
        if (net < 0) {
            return;
        }
        const width = diagram.inputs[i].width;
        const driver: Driver = { id: drivers.length, node, pin: 0, off: 0, net, netBits: [0, width], pinBits: [0, width], pinWidth: width, sinks: [], conn: -1 };
        drivers.push(driver);
        driversOf[net].push(driver);
    });
    const driverRows: Driver[][][] = diagram.parts.map((part) => part.outputs.map(() => []));
    partNodes.forEach((node, p) => {
        const part = node.part!;
        part.conns.forEach((conn, c) => {
            if (conn.dir !== 'out' || conn.net === undefined || !nets[conn.net] || !part.outputs[conn.pin]) {
                return;
            }
            const width = nets[conn.net].width;
            const driver: Driver = {
                id: drivers.length,
                node,
                pin: conn.pin,
                off: 0,
                net: conn.net,
                netBits: conn.netBits ?? [0, width],
                pinBits: conn.pinBits,
                pinWidth: part.outputs[conn.pin].width,
                sinks: [],
                conn: c,
            };
            drivers.push(driver);
            driversOf[conn.net].push(driver);
            driverRows[p][conn.pin].push(driver);
        });
    });

    // Rows on each pin: one per (connection, driver) pair, so that no two
    // wires end on the same point; constants get a row for their tie-off.
    const overlaps = (a: BitRange, b: BitRange) => a[0] < b[0] + b[1] && b[0] < a[0] + a[1];
    type Row = { sink?: Sink; tie?: boolean; bits?: BitRange };
    const inPinRows: Row[][][] = diagram.parts.map((part) => part.inputs.map(() => []));
    partNodes.forEach((node, p) => {
        const part = node.part!;
        for (const conn of part.conns) {
            if (conn.dir !== 'in' || !part.inputs[conn.pin]) {
                continue;
            }
            const rows = inPinRows[p][conn.pin];
            const pinWidth = part.inputs[conn.pin].width;
            if (conn.net === undefined || !nets[conn.net]) {
                rows.push({ tie: conn.constant ?? false, bits: conn.pinBits });
                continue;
            }
            const netBits = conn.netBits ?? [0, nets[conn.net].width];
            const from = driversOf[conn.net].filter((d) => overlaps(d.netBits, netBits));
            if (from.length === 0) {
                rows.push({ bits: conn.pinBits });
            }
            for (const driver of from) {
                const sink: Sink = { node, pin: conn.pin, off: 0, driver, netBits, pinBits: conn.pinBits, pinWidth };
                driver.sinks.push(sink);
                rows.push({ sink, bits: conn.pinBits });
            }
        }
    });
    const outPortRows: Row[][] = outputNodes.map((node, i) => {
        const net = netOfOutput[i];
        if (net < 0) {
            return [{}];
        }
        const width = diagram.outputs[i].width;
        const rows = driversOf[net].map((driver) => {
            const sink: Sink = { node, pin: 0, off: 0, driver, netBits: driver.netBits, pinBits: driver.netBits, pinWidth: width };
            driver.sinks.push(sink);
            return { sink };
        });
        return rows.length ? rows : [{}];
    });

    // Sizes and anchor offsets.
    const ties: { node: Node; pin: number; row: number; value: boolean; width: number }[] = [];
    partNodes.forEach((node, p) => {
        const part = node.part!;
        const symbol = symbolFor(part);
        node.shape = symbol.shape;
        node.variant = symbol.variant;
        const inCounts = part.inputs.map((_, i) => Math.max(1, inPinRows[p][i].length));
        // Drivers of one pin that take the same bits (`out=a, out=b`) share
        // a row: one pin, and the wires fan out from it. Different slices
        // of the pin (`out[0..3]=x, out[4..7]=y`) each get their own row.
        const outSlots = part.outputs.map((_, o) => {
            const keys: string[] = [];
            const slot = driverRows[p][o].map((driver) => {
                const key = driver.pinBits.join(',');
                let at = keys.indexOf(key);
                if (at < 0) {
                    at = keys.push(key) - 1;
                }
                return at;
            });
            return { count: keys.length, slot };
        });
        const outCounts = outSlots.map((slots) => Math.max(1, slots.count));
        const nIn = sum(inCounts);
        const nOut = sum(outCounts);
        const n = Math.max(nIn, nOut, 1);
        let first: number;
        if (symbol.shape === 'box') {
            const inName = maxOf(part.inputs, (pin) => tw(pinLabel(pin)), 0);
            const outName = maxOf(part.outputs, (pin) => tw(pinLabel(pin)), 0);
            node.w = snapUp(Math.max(72, tw(part.chip) * 1.1 + (part.drill ? 34 : 20), inName + outName + 30));
            first = HEADER + GRID;
            node.h = first + (n - 1) * ROW + ROW + (part.sequential ? GRID : 0);
            node.bodyH = node.h;
        } else {
            node.w = gateWidth(symbol.shape);
            const bodyH = n === 1 ? 36 : (n + 1) * ROW;
            first = (bodyH - (n - 1) * ROW) / 2;
            node.bodyH = bodyH;
            node.h = bodyH + (symbol.variant ? ROW : 0);
            if (symbol.variant) {
                node.w = Math.max(node.w, snapUp(tw(symbol.variant) * 0.85 + 6));
            }
        }
        const place = (counts: number[], total: number): number[][] => {
            let at = first + ((n - total) * ROW) / 2;
            return counts.map((count) => {
                const offs: number[] = [];
                for (let r = 0; r < count; r++) {
                    offs.push(at);
                    at += ROW;
                }
                return offs;
            });
        };
        node.inRows = place(inCounts, nIn);
        node.outRows = place(outCounts, nOut);
        const sliceOf = (bits: BitRange | undefined, width: number): BitRange | null =>
            !bits || (bits[0] === 0 && bits[1] >= width) ? null : bits;
        node.inSlices = part.inputs.map((pin, i) => {
            const rows = inPinRows[p][i];
            return rows.length ? rows.map((row) => sliceOf(row.bits, pin.width)) : [null];
        });
        node.outSlices = part.outputs.map((pin, o) => {
            const slices: (BitRange | null)[] = new Array(outCounts[o]).fill(null);
            driverRows[p][o].forEach((driver, r) => {
                slices[outSlots[o].slot[r]] = sliceOf(driver.pinBits, pin.width);
            });
            return slices;
        });
        part.inputs.forEach((pin, i) => {
            inPinRows[p][i].forEach((row, r) => {
                if (row.sink) {
                    row.sink.off = node.inRows[i][r];
                }
                if (row.tie !== undefined) {
                    const conn = part.conns.filter((c) => c.dir === 'in' && c.pin === i)[r];
                    ties.push({ node, pin: i, row: r, value: row.tie, width: conn ? conn.pinBits[1] : pin.width });
                }
            });
        });
        part.outputs.forEach((_, o) => {
            driverRows[p][o].forEach((driver, r) => {
                driver.off = node.outRows[o][outSlots[o].slot[r]];
            });
        });
    });
    inputNodes.forEach((node) => {
        for (const driver of drivers) {
            if (driver.node === node) {
                driver.off = GRID;
            }
        }
    });
    outputNodes.forEach((node, i) => {
        const rows = outPortRows[i];
        node.h = rows.length * ROW;
        node.inRows = [rows.map((_, r) => GRID + r * ROW)];
        rows.forEach((row, r) => {
            if (row.sink) {
                row.sink.off = GRID + r * ROW;
            }
        });
    });

    // ---- cycles and layers ------------------------------------------------

    type Edge = { from: Node; to: Node; sink: Sink; cut: boolean };
    const edges: Edge[] = [];
    for (const driver of drivers) {
        for (const sink of driver.sinks) {
            edges.push({ from: driver.node, to: sink.node, sink, cut: false });
        }
    }
    const outgoing = (cutOk: boolean) => {
        const out: Edge[][] = nodes.map(() => []);
        for (const edge of edges) {
            if (cutOk || !edge.cut) {
                out[edge.from.id].push(edge);
            }
        }
        return out;
    };
    // Edges inside a cycle into an input that no output depends on at once
    // (a register's `in`, a DFF's `in`) are cut first.
    const comp = components(nodes.length, outgoing(false));
    for (const edge of edges) {
        if (edge.from === edge.to) {
            edge.cut = true;
        } else if (comp[edge.from.id] === comp[edge.to.id] && edge.to.part && !feedsThrough(edge.to.part, edge.sink.pin)) {
            edge.cut = true;
        }
    }
    // Then the back edges of a depth-first search, from the inputs.
    {
        const out = outgoing(false);
        const state = new Uint8Array(nodes.length); // 0 new, 1 on stack, 2 done
        for (const start of nodes) {
            if (state[start.id]) {
                continue;
            }
            const stack: { node: Node; next: number }[] = [{ node: start, next: 0 }];
            state[start.id] = 1;
            while (stack.length) {
                const top = stack[stack.length - 1];
                const list = out[top.node.id];
                if (top.next >= list.length) {
                    state[top.node.id] = 2;
                    stack.pop();
                    continue;
                }
                const edge = list[top.next++];
                const to = edge.to.id;
                if (state[to] === 1) {
                    edge.cut = true;
                } else if (state[to] === 0) {
                    state[to] = 1;
                    stack.push({ node: edge.to, next: 0 });
                }
            }
        }
    }
    // Longest path from the inputs, over the edges kept.
    {
        const out = outgoing(false);
        const indegree = new Int32Array(nodes.length);
        for (const edge of edges) {
            if (!edge.cut) {
                indegree[edge.to.id]++;
            }
        }
        for (const node of nodes) {
            node.layer = node.kind === 'in' ? 0 : 1;
        }
        const queue = nodes.filter((node) => indegree[node.id] === 0);
        for (let q = 0; q < queue.length; q++) {
            const node = queue[q];
            for (const edge of out[node.id]) {
                edge.to.layer = Math.max(edge.to.layer, node.layer + 1);
                if (--indegree[edge.to.id] === 0) {
                    queue.push(edge.to);
                }
            }
        }
    }
    let lastPartLayer = 0;
    for (const node of partNodes) {
        lastPartLayer = Math.max(lastPartLayer, node.layer);
    }
    const outLayer = lastPartLayer + 1;
    for (const node of outputNodes) {
        node.layer = outLayer;
    }
    const layerCount = outLayer + 1;

    // ---- items: nodes and the dummies of long and backward wires ----------

    const layers: Item[][] = Array.from({ length: layerCount }, () => []);
    const items: Item[] = [];
    const addItem = (layer: number, node?: Node): Item => {
        const item: Item = { id: items.length, layer, node, h: node ? node.h : 0, w: node ? node.w : 0, top: 0, order: 0, fixed: !!node && node.kind !== 'part' };
        items.push(item);
        layers[layer].push(item);
        if (node) {
            node.item = item;
        }
        return item;
    };
    for (const node of nodes) {
        addItem(node.layer, node);
    }

    interface Chain {
        driver: Driver;
        forward: Sink[];
        back: Sink[];
        fMax: number;
        bMin: number;
        F: Map<number, Item>;
        B: Map<number, Item>;
    }
    const links: Link[] = [];
    const chains: Chain[] = [];
    for (const driver of drivers) {
        if (driver.sinks.length === 0) {
            continue;
        }
        const ld = driver.node.layer;
        const forward = driver.sinks.filter((s) => s.node.layer > ld);
        const back = driver.sinks.filter((s) => s.node.layer <= ld);
        const chain: Chain = {
            driver,
            forward,
            back,
            fMax: maxOf(forward, (s) => s.node.layer, ld),
            bMin: minOf(back, (s) => s.node.layer, ld + 1),
            F: new Map(),
            B: new Map(),
        };
        const dItem = driver.node.item!;
        let prev: Item = dItem;
        let prevOff = driver.off;
        for (let k = ld + 1; k < chain.fMax; k++) {
            const dummy = addItem(k);
            chain.F.set(k, dummy);
            links.push({ a: prev, aOff: prevOff, b: dummy, bOff: 0 });
            prev = dummy;
            prevOff = 0;
        }
        for (const sink of forward) {
            const k = sink.node.layer;
            const from = k - 1 === ld ? dItem : chain.F.get(k - 1)!;
            links.push({ a: from, aOff: from === dItem ? driver.off : 0, b: sink.node.item!, bOff: sink.off });
        }
        let after: Item | undefined;
        for (let k = ld; k >= chain.bMin; k--) {
            const dummy = addItem(k);
            chain.B.set(k, dummy);
            if (after) {
                links.push({ a: dummy, aOff: 0, b: after, bOff: 0 });
            }
            after = dummy;
        }
        for (const sink of back) {
            // Into the sink from the channel on its left: pull it toward the
            // dummy one layer up the chain, if there is one.
            const k = sink.node.layer;
            const upstream = chain.B.get(k + 1);
            if (upstream && k + 1 <= ld) {
                links.push({ a: sink.node.item!, aOff: sink.off, b: upstream, bOff: 0 });
            }
        }
        chains.push(chain);
    }

    // ---- order within layers ------------------------------------------------

    const linksBetween: Link[][] = Array.from({ length: layerCount }, () => []); // between layer k and k+1, at k
    const neighbours: { left: { item: Item; off: number; myOff: number }[]; right: { item: Item; off: number; myOff: number }[] }[] = items.map(() => ({ left: [], right: [] }));
    for (const link of links) {
        if (link.b.layer !== link.a.layer + 1) {
            continue;
        }
        linksBetween[link.a.layer].push(link);
        neighbours[link.b.id].left.push({ item: link.a, off: link.aOff, myOff: link.bOff });
        neighbours[link.a.id].right.push({ item: link.b, off: link.bOff, myOff: link.aOff });
    }
    const setOrder = (layer: Item[]) => layer.forEach((item, i) => (item.order = i));
    layers.forEach(setOrder);
    const fineOrder = (item: Item, off: number) => item.order + (item.h ? off / item.h : 0.5) * 0.5;
    const sweep = (k: number, side: 'left' | 'right') => {
        const layer = layers[k];
        if (layer.every((item) => item.fixed)) {
            return;
        }
        const key = new Map<Item, number>();
        for (const item of layer) {
            const list = neighbours[item.id][side];
            key.set(item, list.length ? list.reduce((s, n) => s + fineOrder(n.item, n.off), 0) / list.length : item.order);
        }
        layer.sort((a, b) => key.get(a)! - key.get(b)! || a.order - b.order);
        setOrder(layer);
    };
    const crossings = (): number => {
        let total = 0;
        for (const list of linksBetween) {
            for (let i = 0; i < list.length; i++) {
                const a = list[i];
                const a1 = fineOrder(a.a, a.aOff);
                const a2 = fineOrder(a.b, a.bOff);
                for (let j = i + 1; j < list.length; j++) {
                    const b = list[j];
                    const d1 = a1 - fineOrder(b.a, b.aOff);
                    const d2 = a2 - fineOrder(b.b, b.bOff);
                    if (d1 * d2 < 0) {
                        total++;
                    }
                }
            }
        }
        return total;
    };
    // Initial order: parts by index, dummies after; then sweeps.
    for (const layer of layers) {
        layer.sort((a, b) => rank(a) - rank(b));
        setOrder(layer);
    }
    let best = crossings();
    let bestOrder = layers.map((layer) => [...layer]);
    for (let round = 0; round < 8 && best > 0; round++) {
        for (let k = 1; k < layerCount; k++) {
            sweep(k, 'left');
        }
        for (let k = layerCount - 2; k >= 0; k--) {
            sweep(k, 'right');
        }
        const now = crossings();
        if (now < best) {
            best = now;
            bestOrder = layers.map((layer) => [...layer]);
        }
    }
    bestOrder.forEach((layer, k) => {
        layers[k] = layer;
        setOrder(layer);
    });

    // ---- vertical positions ---------------------------------------------------

    const gapAfter = (a: Item, b: Item): number => {
        if (!a.node || !b.node) {
            return a.node || b.node ? ROW : ROW;
        }
        if (a.node.kind !== 'part' && b.node.kind !== 'part') {
            return ROW;
        }
        return 2 * ROW;
    };
    for (const layer of layers) {
        let y = 0;
        layer.forEach((item, i) => {
            item.top = y;
            y += item.h + (i + 1 < layer.length ? gapAfter(item, layer[i + 1]) : 0);
        });
    }
    // Centre the layers on each other to start with.
    {
        const heights = layers.map((layer) => (layer.length ? layer[layer.length - 1].top + layer[layer.length - 1].h : 0));
        const tallest = maxOf(heights, (h) => h, -Infinity);
        layers.forEach((layer, k) => {
            const shift = snapDown((tallest - heights[k]) / 2);
            for (const item of layer) {
                item.top += shift;
            }
        });
    }
    const place = (layer: Item[], desired: number[], weights: number[]) => {
        // Isotonic regression of the desired tops, with the gaps folded in.
        const offsets: number[] = [];
        let acc = 0;
        layer.forEach((item, i) => {
            offsets.push(acc);
            acc += item.h + (i + 1 < layer.length ? gapAfter(item, layer[i + 1]) : 0);
        });
        const target = desired.map((d, i) => d - offsets[i]);
        const q = isotonic(target, weights);
        layer.forEach((item, i) => {
            item.top = Math.round(q[i] / GRID) * GRID + offsets[i];
        });
    };
    const relax = (k: number, sides: ('left' | 'right')[]) => {
        const layer = layers[k];
        const desired: number[] = [];
        const weights: number[] = [];
        for (const item of layer) {
            let total = 0;
            let count = 0;
            for (const side of sides) {
                for (const n of neighbours[item.id][side]) {
                    total += n.item.top + n.off - n.myOff;
                    count++;
                }
            }
            desired.push(count ? total / count : item.top);
            weights.push(count ? count + (item.node ? 0 : 0.5) : 0.05);
        }
        place(layer, desired, weights);
    };
    for (let round = 0; round < 6; round++) {
        for (let k = 1; k < layerCount; k++) {
            relax(k, ['left']);
        }
        for (let k = layerCount - 2; k >= 0; k--) {
            relax(k, ['right']);
        }
    }
    for (let round = 0; round < 3; round++) {
        for (let k = 0; k < layerCount; k++) {
            relax(k, ['left', 'right']);
        }
    }

    // ---- channels and tracks ----------------------------------------------------

    interface End {
        side: 'L' | 'R';
        y: number;
        /** x on the item (anchor), filled once columns are placed. */
        item: Item;
        /** Which end of the item: a node anchor, or a dummy's left or right end. */
        at: 'anchor' | 'left' | 'right';
        sink?: Sink;
    }
    interface Group {
        driver: Driver;
        channel: number;
        source: End;
        dests: End[];
        t0: number;
        track: number;
        lo: number;
        hi: number;
    }
    const groups: Group[] = [];
    const channels: Group[][] = Array.from({ length: Math.max(0, layerCount - 1) }, () => []);
    const groupAt = (chain: Chain, c: number, source: End, t0: number): Group => {
        const existing = channels[c].find((g) => g.driver === chain.driver);
        if (existing) {
            return existing;
        }
        const group: Group = { driver: chain.driver, channel: c, source, dests: [], t0, track: -1, lo: 0, hi: 0 };
        channels[c].push(group);
        groups.push(group);
        return group;
    };
    const anchorY = (item: Item, off: number) => item.top + off;
    interface Through {
        driver: Driver;
        item: Item;
        dir: 'right' | 'left';
        t0: number;
    }
    const throughs: Through[] = [];
    for (const chain of chains) {
        const driver = chain.driver;
        const ld = driver.node.layer;
        const dItem = driver.node.item!;
        const dEnd: End = { side: 'L', y: anchorY(dItem, driver.off), item: dItem, at: 'anchor' };
        for (let c = ld; c < chain.fMax; c++) {
            const src: End = c === ld ? dEnd : { side: 'L', y: chain.F.get(c)!.top, item: chain.F.get(c)!, at: 'right' };
            const group = groupAt(chain, c, src, 2 * c + 1);
            if (c + 1 < chain.fMax) {
                const f = chain.F.get(c + 1)!;
                group.dests.push({ side: 'R', y: f.top, item: f, at: 'left' });
            }
            for (const sink of chain.forward) {
                if (sink.node.layer === c + 1) {
                    group.dests.push({ side: 'R', y: anchorY(sink.node.item!, sink.off), item: sink.node.item!, at: 'anchor', sink });
                }
            }
        }
        for (let k = ld + 1; k < chain.fMax; k++) {
            throughs.push({ driver, item: chain.F.get(k)!, dir: 'right', t0: 2 * k });
        }
        if (chain.back.length) {
            const bl = chain.B.get(ld)!;
            const group = groupAt(chain, ld, dEnd, 2 * ld + 1);
            group.dests.push({ side: 'L', y: bl.top, item: bl, at: 'right' });
            for (let k = ld; k >= chain.bMin; k--) {
                throughs.push({ driver, item: chain.B.get(k)!, dir: 'left', t0: 2 * ld + 2 + 2 * (ld - k) });
            }
            for (let c = ld - 1; c >= chain.bMin - 1; c--) {
                const from = chain.B.get(c + 1)!;
                const group = groupAt(chain, c, { side: 'R', y: from.top, item: from, at: 'left' }, 2 * ld + 3 + 2 * (ld - 1 - c));
                if (c >= chain.bMin) {
                    const to = chain.B.get(c)!;
                    group.dests.push({ side: 'L', y: to.top, item: to, at: 'right' });
                }
                for (const sink of chain.back) {
                    if (sink.node.layer === c + 1) {
                        group.dests.push({ side: 'R', y: anchorY(sink.node.item!, sink.off), item: sink.node.item!, at: 'anchor', sink });
                    }
                }
            }
        }
    }

    // Tracks, channel by channel.
    const trackCount: number[] = [];
    channels.forEach((list) => {
        const turning: Group[] = [];
        for (const group of list) {
            group.lo = minOf(group.dests, (d) => d.y, group.source.y);
            group.hi = maxOf(group.dests, (d) => d.y, group.source.y);
            if (group.lo === group.hi && group.dests.length > 0) {
                group.track = -1; // straight across
            } else if (group.dests.length > 0) {
                turning.push(group);
            }
        }
        const leftAt = new Map<number, Group>();
        const rightAt = new Map<number, Group>();
        for (const group of turning) {
            for (const end of [group.source, ...group.dests]) {
                (end.side === 'L' ? leftAt : rightAt).set(end.y, group);
            }
        }
        // A group with a left stub at y must sit left of a group with a right stub at y.
        const before = new Map<Group, Set<Group>>();
        const after = new Map<Group, Set<Group>>();
        for (const group of turning) {
            before.set(group, new Set());
            after.set(group, new Set());
        }
        for (const [y, a] of leftAt) {
            const b = rightAt.get(y);
            if (b && b !== a) {
                after.get(a)!.add(b);
                before.get(b)!.add(a);
            }
        }
        const remaining = new Set(turning);
        const tracks: Group[][] = [];
        const key = (g: Group) => {
            // Wires going down take the outer tracks on the left, so that
            // they cross fewer of the wires going up.
            const down = g.source.y <= g.lo;
            return (down ? -1 : 1) * 1e6 + (down ? -g.hi : g.lo) + g.driver.id * 1e-3;
        };
        while (remaining.size) {
            let pick: Group | undefined;
            for (const group of remaining) {
                const ready = [...before.get(group)!].every((p) => !remaining.has(p));
                if (ready && (!pick || key(group) < key(pick))) {
                    pick = group;
                }
            }
            if (!pick) {
                for (const group of remaining) {
                    if (!pick || key(group) < key(pick)) {
                        pick = group;
                    }
                }
            }
            const group = pick!;
            remaining.delete(group);
            let min = 0;
            for (const p of before.get(group)!) {
                if (p.track >= 0) {
                    min = Math.max(min, p.track + 1);
                }
            }
            let t = min;
            while (tracks[t] && tracks[t].some((o) => o.lo <= group.hi + GRID / 2 && group.lo <= o.hi + GRID / 2)) {
                t++;
            }
            (tracks[t] ??= []).push(group);
            group.track = t;
        }
        trackCount.push(tracks.length);
    });

    // ---- columns (x) -------------------------------------------------------------

    const layerW = layers.map((layer) => maxOf(layer, (i) => i.w, layer.some((i) => i.node) ? 0 : 18));
    const padL: number[] = [];
    const padR: number[] = [];
    channels.forEach((list) => {
        let left = 18;
        let right = 18;
        for (const group of list) {
            const d = group.driver;
            if (group.source.at === 'anchor' && group.source.item.node?.kind === 'part') {
                const net = nets[d.net].width;
                const prefix = (whole(d.pinBits, d.pinWidth) ? '' : range(d.pinBits)) + (whole(d.netBits, net) ? '' : `→${range(d.netBits)}`);
                if (net > 1 || prefix) {
                    const value = net > 1 ? Math.min(valueChars(net, radix), 8) * char : 0;
                    left = Math.max(left, tw(prefix) + (prefix && net > 1 ? char : 0) + value + (net > 1 ? 30 : 14));
                }
            }
            for (const end of group.dests) {
                if (end.sink && end.sink.node.kind === 'part') {
                    const text = sliceText(end.sink, nets[d.net].width);
                    if (text) {
                        right = Math.max(right, tw(text) + 14);
                    }
                }
            }
        }
        padL.push(left);
        padR.push(right);
    });
    ties.forEach((tie) => {
        const c = tie.node.layer - 1;
        if (c >= 0 && c < padR.length) {
            padR[c] = Math.max(padR[c], tie.width > 1 ? 54 : 36);
        }
    });
    const layerX: number[] = [];
    const channelX: number[] = [];
    {
        let x = MARGIN;
        for (let k = 0; k < layerCount; k++) {
            layerX.push(x);
            x += layerW[k];
            if (k < layerCount - 1) {
                channelX.push(x);
                const tracks = trackCount[k];
                x += padL[k] + Math.max(0, tracks - 1) * PITCH + padR[k];
            }
        }
    }
    const trackX = (c: number, t: number) => channelX[c] + padL[c] + t * PITCH;
    const itemX = (item: Item): number => {
        if (!item.node) {
            return layerX[item.layer];
        }
        const w = layerW[item.layer];
        if (item.node.kind === 'in') {
            return layerX[item.layer] + w - item.w;
        }
        if (item.node.kind === 'out') {
            return layerX[item.layer];
        }
        return layerX[item.layer] + Math.round((w - item.w) / 2);
    };
    const endX = (end: End): number => {
        if (end.at === 'left') {
            return layerX[end.item.layer];
        }
        if (end.at === 'right') {
            return layerX[end.item.layer] + layerW[end.item.layer];
        }
        const x = itemX(end.item);
        return end.side === 'L' ? x + end.item.w : x;
    };

    // ---- geometry ----------------------------------------------------------------

    let minY = Infinity;
    let maxY = -Infinity;
    for (const item of items) {
        minY = Math.min(minY, item.top);
        maxY = Math.max(maxY, item.top + item.h);
    }
    const dy = MARGIN - (Number.isFinite(minY) ? minY : 0);
    const Y = (y: number) => y + dy;

    const parts: PartGeom[] = partNodes.map((node) => {
        const part = node.part!;
        const x = itemX(node.item!);
        const y = Y(node.item!.top);
        return {
            index: node.index,
            chip: part.chip,
            shape: node.shape!,
            variant: node.variant,
            x,
            y,
            w: node.w,
            h: node.h,
            bodyH: node.bodyH!,
            layer: node.layer,
            sequential: part.sequential,
            drill: part.drill,
            inputs: part.inputs.map((pin, i) => ({
                name: pin.name,
                width: pin.width,
                index: i,
                anchors: node.inRows[i].map((off) => ({ x, y: y + off })),
                slices: node.inRows[i].map((_, r) => node.inSlices?.[i]?.[r] ?? null),
            })),
            outputs: part.outputs.map((pin, o) => ({
                name: pin.name,
                width: pin.width,
                index: o,
                anchors: node.outRows[o].map((off) => ({ x: x + node.w, y: y + off })),
                slices: node.outRows[o].map((_, r) => node.outSlices?.[o]?.[r] ?? null),
            })),
        };
    });
    const ports = (list: Node[], side: 'in' | 'out', pins: PinDecl[], netOf: number[]): PortGeom[] =>
        list.map((node, i) => {
            const x = itemX(node.item!);
            const y = Y(node.item!.top);
            const offs = side === 'in' ? node.outRows[0] : node.inRows[0];
            return {
                side,
                index: i,
                name: pins[i].name,
                width: pins[i].width,
                net: netOf[i],
                x,
                y,
                w: node.w,
                h: node.h,
                anchors: offs.map((off) => ({ x: side === 'in' ? x + node.w : x, y: y + off })),
            };
        });

    const wires: WireGeom[] = [];
    const junctions: JunctionGeom[] = [];
    const labels: LabelGeom[] = [];
    const marks: MarkGeom[] = [];
    const pt = (x: number, y: number): Point => ({ x, y: Y(y) });
    /** What a driver puts on its net: all of it, or a slice (`out[0]=x` of a wider `x`). */
    const own = (d: Driver): { bits: BitRange | null; width: number } => {
        const netWidth = nets[d.net].width;
        return whole(d.netBits, netWidth) ? { bits: null, width: netWidth } : { bits: d.netBits, width: d.netBits[1] };
    };
    const carried = (d: Driver, end: End): { bits: BitRange | null; width: number } => {
        const netWidth = nets[d.net].width;
        if (end.sink && end.sink.node.kind === 'part' && !whole(end.sink.netBits, netWidth)) {
            return { bits: end.sink.netBits, width: end.sink.netBits[1] };
        }
        return own(d);
    };
    const labelled = new Set<string>();
    let tEnd = 0;
    for (const group of groups) {
        const d = group.driver;
        const netWidth = nets[d.net].width;
        const mine = own(d);
        const sx = endX(group.source);
        const t0 = group.t0;
        tEnd = Math.max(tEnd, t0 + 1);
        if (group.track < 0) {
            for (const dest of group.dests) {
                const c = carried(d, dest);
                wires.push({ net: d.net, ...c, points: [pt(sx, group.source.y), pt(endX(dest), dest.y)], kind: 'straight', t0, t1: t0 + 1 });
            }
        } else {
            const tx = trackX(group.channel, group.track);
            const sy = group.source.y;
            wires.push({ net: d.net, ...mine, points: [pt(sx, sy), pt(tx, sy)], kind: 'stub', t0, t1: t0 + 0.4 });
            if (group.lo < sy) {
                wires.push({ net: d.net, ...mine, points: [pt(tx, sy), pt(tx, group.lo)], kind: 'trunk', t0: t0 + 0.4, t1: t0 + 0.7 });
            }
            if (group.hi > sy) {
                wires.push({ net: d.net, ...mine, points: [pt(tx, sy), pt(tx, group.hi)], kind: 'trunk', t0: t0 + 0.4, t1: t0 + 0.7 });
            }
            for (const dest of group.dests) {
                const c = carried(d, dest);
                wires.push({ net: d.net, ...c, points: [pt(tx, dest.y), pt(endX(dest), dest.y)], kind: 'branch', t0: t0 + 0.7, t1: t0 + 1 });
            }
            // Junction dots where three or more pieces meet on the track.
            const ends = [group.source, ...group.dests];
            const ys = [...new Set(ends.map((e) => e.y))];
            for (const y of ys) {
                const stubs = ends.filter((e) => e.y === y).length;
                const degree = stubs + (y > group.lo ? 1 : 0) + (y < group.hi ? 1 : 0);
                if (degree >= 3) {
                    junctions.push({ x: tx, y: Y(y), net: d.net, ...mine, t: y === sy ? t0 + 0.4 : t0 + 0.7 });
                }
            }
        }
        // Labels at the driver: bus width, value, slice of the pin.
        if (group.source.at === 'anchor') {
            const sy = Y(group.source.y);
            const fromPart = group.source.item.node?.kind === 'part';
            // A pin that drives several nets (`out=a, out=b`) is labelled once.
            const key = `${group.source.item.id}:${d.pin}:${d.pinBits.join(',')}:${d.netBits.join(',')}`;
            const first = !labelled.has(key);
            labelled.add(key);
            let labelX = sx + 4;
            if (mine.width > 1 && first) {
                marks.push({ x: sx + 9, y: sy, width: mine.width, net: d.net });
                labelX = sx + 18 + tw(String(mine.width)) * 0.8;
            }
            if (fromPart && first) {
                const prefix = whole(d.pinBits, d.pinWidth) ? '' : range(d.pinBits);
                const netPart = whole(d.netBits, netWidth) ? '' : `→${range(d.netBits)}`;
                if (mine.width > 1) {
                    labels.push({ x: labelX, y: sy - 5, text: prefix + netPart, kind: 'value', anchor: 'start', net: d.net, ...mine });
                } else if (prefix || netPart) {
                    labels.push({ x: labelX, y: sy - 4, text: prefix + netPart, kind: 'slice', anchor: 'start', net: d.net, ...mine });
                }
            }
        }
        for (const dest of group.dests) {
            if (dest.sink && dest.sink.node.kind === 'part') {
                const text = sliceText(dest.sink, netWidth);
                if (text) {
                    const c = carried(d, dest);
                    labels.push({ x: endX(dest) - 5, y: Y(dest.y) - 4, text, kind: 'slice', anchor: 'end', net: d.net, ...c });
                }
            }
        }
    }
    // A pin that drives several nets (`out=a, out=b`): their stubs leave the
    // pin as one wire and part where the nearest track turns off; a dot there.
    const fanned = new Map<string, Group[]>();
    for (const group of groups) {
        if (group.source.at === 'anchor') {
            const key = `${group.channel}:${group.source.item.id}:${group.source.y}`;
            fanned.set(key, [...(fanned.get(key) ?? []), group]);
        }
    }
    for (const list of fanned.values()) {
        const turning = list.filter((g) => g.track >= 0).sort((a, b) => a.track - b.track);
        if (list.length < 2 || turning.length === 0) {
            continue;
        }
        const near = turning[0];
        const tx = trackX(near.channel, near.track);
        if (junctions.some((j) => j.x === tx && j.y === Y(near.source.y))) {
            continue;
        }
        junctions.push({ x: tx, y: Y(near.source.y), net: near.driver.net, ...own(near.driver), t: near.t0 + 0.4 });
    }
    for (const through of throughs) {
        const x0 = layerX[through.item.layer];
        const x1 = x0 + layerW[through.item.layer];
        const y = through.item.top;
        const points = through.dir === 'right' ? [pt(x0, y), pt(x1, y)] : [pt(x1, y), pt(x0, y)];
        wires.push({ net: through.driver.net, ...own(through.driver), points, kind: 'through', t0: through.t0, t1: through.t0 + 1 });
        tEnd = Math.max(tEnd, through.t0 + 1);
    }

    const tieGeoms: TieGeom[] = ties.map((tie) => ({
        part: tie.node.index,
        pin: tie.pin,
        x: itemX(tie.node.item!),
        y: Y(tie.node.item!.top + tie.node.inRows[tie.pin][tie.row]),
        value: tie.value,
        width: tie.width,
    }));

    const lastX = layerX[layerCount - 1] + layerW[layerCount - 1];
    const tabOrder = [...parts].sort((a, b) => a.layer - b.layer || a.y - b.y || a.index - b.index).map((p) => p.index);
    return {
        width: Math.ceil(lastX + MARGIN),
        height: Math.ceil(Y(Number.isFinite(maxY) ? maxY : 0) + MARGIN),
        layers: layerCount,
        parts,
        inputs: ports(inputNodes, 'in', diagram.inputs, netOfInput),
        outputs: ports(outputNodes, 'out', diagram.outputs, netOfOutput),
        wires: mergeWires(wires),
        junctions,
        labels,
        marks,
        ties: tieGeoms,
        tEnd,
        tabOrder,
    };

    function rank(item: Item): number {
        if (!item.node) {
            return 1e6 + item.id;
        }
        return item.node.id;
    }
}

/**
 * Joins pieces of one wire that continue each other with nothing else
 * meeting there, so a long wire is one path: no overlapping round caps, one
 * pulse along it.
 */
function mergeWires(wires: WireGeom[]): WireGeom[] {
    const key = (p: Point) => `${Math.round(p.x * 100)},${Math.round(p.y * 100)}`;
    const ends = new Map<string, number>();
    const count = (k: string) => ends.set(k, (ends.get(k) ?? 0) + 1);
    for (const w of wires) {
        count(`${w.net}@${key(w.points[0])}`);
        count(`${w.net}@${key(w.points[w.points.length - 1])}`);
    }
    const startsAt = new Map<string, WireGeom>();
    for (const w of wires) {
        startsAt.set(`${w.net}@${key(w.points[0])}`, w);
    }
    const sameBits = (a: WireGeom, b: WireGeom) => (a.bits?.join(',') ?? '') === (b.bits?.join(',') ?? '') && a.width === b.width;
    const absorbed = new Set<WireGeom>();
    const out: WireGeom[] = [];
    for (const w of wires) {
        if (absorbed.has(w)) {
            continue;
        }
        // Only start chains at a piece nothing continues into.
        const startKey = `${w.net}@${key(w.points[0])}`;
        const before = wires.find((o) => o !== w && !absorbed.has(o) && o.net === w.net && key(o.points[o.points.length - 1]) === key(w.points[0]));
        if (before && ends.get(startKey) === 2 && sameBits(before, w)) {
            continue;
        }
        const merged: WireGeom = { ...w, points: [...w.points] };
        for (;;) {
            const endKey = `${w.net}@${key(merged.points[merged.points.length - 1])}`;
            const next = startsAt.get(endKey);
            if (!next || next === w || absorbed.has(next) || ends.get(endKey) !== 2 || !sameBits(merged, next)) {
                break;
            }
            absorbed.add(next);
            for (let k = 1; k < next.points.length; k++) {
                merged.points.push(next.points[k]);
            }
            merged.t1 = next.t1;
            merged.kind = 'path';
        }
        absorbed.add(w);
        out.push(merged);
    }
    for (const w of wires) {
        if (!absorbed.has(w)) {
            out.push(w);
        }
    }
    for (const w of out) {
        // Drop points in the middle of a straight run.
        w.points = w.points.filter((p, i, list) => {
            if (i === 0 || i === list.length - 1) {
                return true;
            }
            const a = list[i - 1];
            const b = list[i + 1];
            return !((a.x === p.x && p.x === b.x) || (a.y === p.y && p.y === b.y));
        });
    }
    return out;
}

function sliceText(sink: Sink, netWidth: number): string {
    const net = whole(sink.netBits, netWidth) ? '' : range(sink.netBits);
    const pin = whole(sink.pinBits, sink.pinWidth) ? '' : `→${range(sink.pinBits)}`;
    return net + pin;
}

function pinLabel(pin: PinDecl): string {
    return pin.width > 1 ? `${pin.name}[${pin.width}]` : pin.name;
}

/** Characters needed for the widest value of `width` bits in a radix. */
export function valueChars(width: number, radix: Radix): number {
    switch (radix) {
        case 'bin':
            return width > 4 ? width + Math.floor((width - 1) / 4) : width;
        case 'hex':
            return 2 + Math.ceil(width / 4);
        case 'dec':
            return width === 16 ? 6 : ((1n << BigInt(width)) - 1n).toString().length;
    }
}

function netIndex(diagram: Diagram, pin: PinDecl, kind: 'input' | 'output', fallback: number): number {
    const byName = diagram.nets.findIndex((net) => net.kind === kind && net.name === pin.name);
    if (byName >= 0) {
        return byName;
    }
    return diagram.nets[fallback] ? fallback : -1;
}

/** Whether some output of the part depends at once on its input `pin`. */
function feedsThrough(part: Part, pin: number): boolean {
    if (!Array.isArray(part.deps) || part.deps.length === 0) {
        return !part.sequential;
    }
    return part.deps.some((deps) => deps.includes(pin));
}

/**
 * The largest of `pick` over `list`, at least `floor`. A loop, not
 * `Math.max(...list)`: spreading a very large list overflows the stack.
 */
export function maxOf<T>(list: readonly T[], pick: (item: T) => number, floor: number): number {
    let best = floor;
    for (const item of list) {
        const v = pick(item);
        if (v > best) {
            best = v;
        }
    }
    return best;
}

/** The smallest of `pick` over `list`, at most `ceiling` (see `maxOf`). */
export function minOf<T>(list: readonly T[], pick: (item: T) => number, ceiling: number): number {
    let best = ceiling;
    for (const item of list) {
        const v = pick(item);
        if (v < best) {
            best = v;
        }
    }
    return best;
}

function sum(list: number[]): number {
    return list.reduce((a, b) => a + b, 0);
}

function snapDown(n: number): number {
    return Math.floor(n / GRID) * GRID;
}

/** Strongly connected components (Tarjan, iterative); returns a component id per node. */
function components(count: number, out: { to: { id: number } }[][]): Int32Array {
    const index = new Int32Array(count).fill(-1);
    const low = new Int32Array(count);
    const onStack = new Uint8Array(count);
    const comp = new Int32Array(count).fill(-1);
    const stack: number[] = [];
    let counter = 0;
    let compCount = 0;
    for (let start = 0; start < count; start++) {
        if (index[start] >= 0) {
            continue;
        }
        const work: { v: number; i: number }[] = [{ v: start, i: 0 }];
        index[start] = low[start] = counter++;
        stack.push(start);
        onStack[start] = 1;
        while (work.length) {
            const frame = work[work.length - 1];
            const list = out[frame.v];
            if (frame.i < list.length) {
                const w = list[frame.i++].to.id;
                if (index[w] < 0) {
                    index[w] = low[w] = counter++;
                    stack.push(w);
                    onStack[w] = 1;
                    work.push({ v: w, i: 0 });
                } else if (onStack[w]) {
                    low[frame.v] = Math.min(low[frame.v], index[w]);
                }
            } else {
                if (low[frame.v] === index[frame.v]) {
                    let w: number;
                    do {
                        w = stack.pop()!;
                        onStack[w] = 0;
                        comp[w] = compCount;
                    } while (w !== frame.v);
                    compCount++;
                }
                work.pop();
                if (work.length) {
                    const parent = work[work.length - 1].v;
                    low[parent] = Math.min(low[parent], low[frame.v]);
                }
            }
        }
    }
    return comp;
}

/**
 * Weighted isotonic regression (pool adjacent violators): the nondecreasing
 * sequence closest to `target` in weighted least squares.
 */
export function isotonic(target: number[], weights: number[]): number[] {
    const blocks: { value: number; weight: number; count: number }[] = [];
    target.forEach((t, i) => {
        blocks.push({ value: t, weight: weights[i], count: 1 });
        while (blocks.length > 1 && blocks[blocks.length - 2].value > blocks[blocks.length - 1].value) {
            const b = blocks.pop()!;
            const a = blocks[blocks.length - 1];
            const weight = a.weight + b.weight;
            a.value = weight > 0 ? (a.value * a.weight + b.value * b.weight) / weight : (a.value + b.value) / 2;
            a.weight = weight;
            a.count += b.count;
        }
    });
    const out: number[] = [];
    for (const block of blocks) {
        for (let i = 0; i < block.count; i++) {
            out.push(block.value);
        }
    }
    return out;
}
