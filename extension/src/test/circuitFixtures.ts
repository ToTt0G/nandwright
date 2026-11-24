// Made-up chips for the circuit diagram's tests and preview, as the engine's
// `chip.diagram` and `chip.values` would describe them, with a tiny simulator
// so the preview can be clicked. Every chip here is an original example (from
// the repository's examples folder or written for this file), never a course
// project chip.

import type { BitRange, Conn, Diagram, NetDecl, Part, PinDecl, Values } from '../webview/circuitTypes';

type Eval = (inputs: bigint[], state: bigint[]) => bigint[];

interface Builtin {
    kind: 'builtin';
    name: string;
    inputs: PinDecl[];
    outputs: PinDecl[];
    sequential: boolean;
    /** Outputs from inputs (and, for sequential chips, the stored state). */
    eval: Eval;
    /** Sequential chips: the next state from the inputs, taken at the tick. */
    next?: (inputs: bigint[], state: bigint[]) => bigint[];
    state?: number;
}

interface PartSpec {
    chip: string;
    conns: string;
}

interface Composite {
    kind: 'composite';
    name: string;
    inputs: PinDecl[];
    outputs: PinDecl[];
    parts: PartSpec[];
}

type ChipDef = Builtin | Composite;

const mask = (width: number) => (1n << BigInt(width)) - 1n;
const pins = (spec: string): PinDecl[] =>
    spec
        .split(',')
        .map((p) => p.trim())
        .filter(Boolean)
        .map((p) => {
            const m = /^(\w+)(?:\[(\d+)\])?$/.exec(p)!;
            return { name: m[1], width: m[2] ? Number(m[2]) : 1 };
        });

function builtin(name: string, inputs: string, outputs: string, evaluate: Eval, seq?: { next: Builtin['next']; state: number }): Builtin {
    return { kind: 'builtin', name, inputs: pins(inputs), outputs: pins(outputs), sequential: !!seq, eval: evaluate, next: seq?.next, state: seq?.state };
}

const W16 = mask(16);
const LIB: Record<string, ChipDef> = {};
for (const def of [
    builtin('Nand', 'a, b', 'out', ([a, b]) => [(a & b) ^ 1n]),
    builtin('Not', 'in', 'out', ([a]) => [a ^ 1n]),
    builtin('And', 'a, b', 'out', ([a, b]) => [a & b]),
    builtin('Or', 'a, b', 'out', ([a, b]) => [a | b]),
    builtin('Xor', 'a, b', 'out', ([a, b]) => [a ^ b]),
    builtin('Not16', 'in[16]', 'out[16]', ([a]) => [a ^ W16]),
    builtin('And16', 'a[16], b[16]', 'out[16]', ([a, b]) => [a & b]),
    builtin('Or16', 'a[16], b[16]', 'out[16]', ([a, b]) => [a | b]),
    builtin('Or8Way', 'in[8]', 'out', ([a]) => [a !== 0n ? 1n : 0n]),
    builtin('Mux', 'a, b, sel', 'out', ([a, b, s]) => [s ? b : a]),
    builtin('Mux16', 'a[16], b[16], sel', 'out[16]', ([a, b, s]) => [s ? b : a]),
    builtin('Mux4Way16', 'a[16], b[16], c[16], d[16], sel[2]', 'out[16]', ([a, b, c, d, s]) => [[a, b, c, d][Number(s)]]),
    builtin('DMux', 'in, sel', 'a, b', ([i, s]) => (s ? [0n, i] : [i, 0n])),
    builtin('DMux4Way', 'in, sel[2]', 'a, b, c, d', ([i, s]) => [0n, 1n, 2n, 3n].map((k) => (k === s ? i : 0n))),
    builtin('Add16', 'a[16], b[16]', 'out[16]', ([a, b]) => [(a + b) & W16]),
    builtin('Inc16', 'in[16]', 'out[16]', ([a]) => [(a + 1n) & W16]),
    builtin('DFF', 'in', 'out', (_, [q]) => [q], { next: ([i]) => [i], state: 1 }),
    builtin('Bit', 'in, load', 'out', (_, [q]) => [q], { next: ([i, l], [q]) => [l ? i : q], state: 1 }),
    builtin('Register', 'in[16], load', 'out[16]', (_, [q]) => [q], { next: ([i, l], [q]) => [l ? i : q], state: 1 }),
]) {
    LIB[def.name] = def;
}

function composite(name: string, inputs: string, outputs: string, parts: [string, string][]): Composite {
    const def: Composite = { kind: 'composite', name, inputs: pins(inputs), outputs: pins(outputs), parts: parts.map(([chip, conns]) => ({ chip, conns })) };
    LIB[name] = def;
    return def;
}

// ---- the example chips ----------------------------------------------------------

/** Majority vote of three bits (examples/chips/Majority.hdl). */
composite('Majority', 'a, b, c', 'out', [
    ['And', 'a=a, b=b, out=ab'],
    ['And', 'a=a, b=c, out=ac'],
    ['And', 'a=b, b=c, out=bc'],
    ['Or', 'a=ab, b=ac, out=abOrAc'],
    ['Or', 'a=abOrAc, b=bc, out=out'],
]);

/** Odd parity of a 4-bit bus (examples/chips/Parity4.hdl). */
composite('Parity4', 'in[4]', 'out', [
    ['Xor', 'a=in[0], b=in[1], out=low'],
    ['Xor', 'a=in[2], b=in[3], out=high'],
    ['Xor', 'a=low, b=high, out=out'],
]);

/** A running total with add and reset (examples/chips/Accumulator.hdl). */
composite('Accumulator', 'in[16], add, reset', 'out[16]', [
    ['Add16', 'a=total, b=in, out=sum'],
    ['Mux16', 'a=total, b=sum, sel=add, out=next'],
    ['Mux16', 'a=next, b=false, sel=reset, out=stored'],
    ['Register', 'in=stored, load=true, out=total, out=out'],
]);

/** The two bytes of a word swapped (examples/chips/SwapBytes.hdl). */
composite('SwapBytes', 'in[16]', 'out[16]', [['Or16', 'a[0..7]=in[8..15], a[8..15]=in[0..7], b=false, out=out']]);

/** A set/reset latch of two cross-coupled Nands: a combinational loop. */
composite('SetReset', 'set, reset', 'q, qBar', [
    ['Not', 'in=set, out=setLow'],
    ['Not', 'in=reset, out=resetLow'],
    ['Nand', 'a=setLow, b=qBar, out=q, out=qLoop'],
    ['Nand', 'a=resetLow, b=qLoop, out=qBar'],
]);

/** A flip-flop that changes every clock cycle while `enable` is 1. */
composite('Toggle', 'enable', 'out', [
    ['Xor', 'a=enable, b=state, out=flipped'],
    ['DFF', 'in=flipped, out=state, out=out'],
]);

/**
 * Two levels: a vote and a parity check of the same four bits, a choice
 * between two words, and a small counter.
 */
composite('VoteCheck', 'a, b, c, d, word[16], pick[2]', 'vote, odd, agree, chosen[16], low[4]', [
    ['Majority', 'a=a, b=b, c=c, out=vote, out=v'],
    ['Parity4', 'in[0]=a, in[1]=b, in[2]=c, in[3]=d, out=odd, out=p'],
    ['Xor', 'a=v, b=p, out=differ'],
    ['Not', 'in=differ, out=agree'],
    ['Not16', 'in=word, out=inverted'],
    ['Mux4Way16', 'a=word, b=inverted, c=false, d=true, sel=pick, out=chosen, out[0..3]=low'],
]);

/** A wide chip for the size budget: about `n` parts in a few rippling rows. */
export function wideChip(n: number): string {
    const name = `Wide${n}`;
    if (LIB[name]) {
        return name;
    }
    const parts: [string, string][] = [];
    const rows = 4;
    const cols = Math.ceil(n / rows);
    const kinds = ['And', 'Or', 'Xor', 'Nand', 'Mux', 'Not'];
    for (let c = 0; c < cols; c++) {
        for (let r = 0; r < rows && parts.length < n; r++) {
            const kind = kinds[(c * 7 + r * 3) % kinds.length];
            const left = c === 0 ? `x[${r}]` : `n${c - 1}r${r}`;
            const other = c === 0 ? `x[${(r + 1) % rows}]` : `n${c - 1}r${(r + 1) % rows}`;
            const sel = c >= 2 ? `n${c - 2}r${(r + 2) % rows}` : `s`;
            const out = `n${c}r${r}`;
            if (kind === 'Not') {
                parts.push([kind, `in=${left}, out=${out}`]);
            } else if (kind === 'Mux') {
                parts.push([kind, `a=${left}, b=${other}, sel=${sel}, out=${out}`]);
            } else {
                parts.push([kind, `a=${left}, b=${other}, out=${out}`]);
            }
        }
    }
    const lastCol = Math.ceil(parts.length / rows) - 1;
    const outs: string[] = [];
    for (let r = 0; r < rows; r++) {
        const exists = lastCol * rows + r < parts.length;
        parts.push(['Or', `a=n${exists ? lastCol : lastCol - 1}r${r}, b=s, out=out[${r}]`]);
        outs.push('');
    }
    composite(name, 'x[4], s', 'out[4]', parts);
    return name;
}

// ---- building diagrams ----------------------------------------------------------

interface ParsedConn {
    pin: string;
    pinRange?: [number, number];
    net: string;
    netRange?: [number, number];
}

function parseConns(text: string): ParsedConn[] {
    return text.split(',').map((piece) => {
        const m = /^\s*(\w+)(?:\[(\d+)(?:\.\.(\d+))?\])?\s*=\s*(\w+)(?:\[(\d+)(?:\.\.(\d+))?\])?\s*$/.exec(piece);
        if (!m) {
            throw new Error(`bad connection ${piece}`);
        }
        const r = (a?: string, b?: string): [number, number] | undefined => (a === undefined ? undefined : [Number(a), Number(b ?? a)]);
        return { pin: m[1], pinRange: r(m[2], m[3]), net: m[4], netRange: r(m[5], m[6]) };
    });
}

const toBits = (r: [number, number]): BitRange => [r[0], r[1] - r[0] + 1];

interface Built {
    def: Composite;
    nets: NetDecl[];
    parts: (Part & { def: ChipDef })[];
}

const builtCache = new Map<string, Built>();

function build(def: Composite): Built {
    const cached = builtCache.get(def.name);
    if (cached) {
        return cached;
    }
    const nets: NetDecl[] = [
        ...def.inputs.map((p) => ({ name: p.name, width: p.width, kind: 'input' as const })),
        ...def.outputs.map((p) => ({ name: p.name, width: p.width, kind: 'output' as const })),
    ];
    const index = (name: string) => nets.findIndex((n) => n.name === name);
    const specs = def.parts.map((spec) => ({ spec, chip: LIB[spec.chip], conns: parseConns(spec.conns) }));
    // Internal nets: widths from the outputs that drive them.
    for (const { chip, conns } of specs) {
        for (const c of conns) {
            const pin = chip.outputs.find((p) => p.name === c.pin);
            if (pin && index(c.net) < 0) {
                nets.push({ name: c.net, width: c.pinRange ? c.pinRange[1] - c.pinRange[0] + 1 : pin.width, kind: 'internal' });
            }
        }
    }
    let line = 4;
    const parts = specs.map(({ spec, chip, conns }) => {
        const out: Conn[] = conns.map((c) => {
            const inPin = chip.inputs.findIndex((p) => p.name === c.pin);
            const dir = inPin >= 0 ? 'in' : 'out';
            const pinIndex = inPin >= 0 ? inPin : chip.outputs.findIndex((p) => p.name === c.pin);
            if (pinIndex < 0) {
                throw new Error(`${spec.chip} has no pin ${c.pin}`);
            }
            const pin = (dir === 'in' ? chip.inputs : chip.outputs)[pinIndex];
            const pinBits: BitRange = c.pinRange ? toBits(c.pinRange) : [0, pin.width];
            if (c.net === 'true' || c.net === 'false') {
                return { dir, pin: pinIndex, pinBits, constant: c.net === 'true' };
            }
            const net = index(c.net);
            if (net < 0) {
                throw new Error(`${def.name}: ${c.net} is not driven`);
            }
            const netBits: BitRange = c.netRange ? toBits(c.netRange) : [0, Math.min(nets[net].width, pinBits[1])];
            return { dir, pin: pinIndex, pinBits, net, netBits };
        });
        const sequential = chip.kind === 'builtin' ? chip.sequential : isSequential(chip);
        const text = `    ${spec.chip}(${spec.conns});`;
        const part: Part & { def: ChipDef } = {
            def: chip,
            chip: spec.chip,
            drill: chip.kind === 'composite',
            builtin: chip.kind === 'builtin',
            sequential,
            span: { start: line * 40, end: line * 40 + text.length, line, endLine: line },
            inputs: chip.inputs,
            outputs: chip.outputs,
            deps: chip.outputs.map(() => (sequential ? [] : chip.inputs.map((_, i) => i))),
            conns: out,
        };
        line++;
        return part;
    });
    const built = { def, nets, parts };
    builtCache.set(def.name, built);
    return built;
}

function isSequential(def: ChipDef): boolean {
    if (def.kind === 'builtin') {
        return def.sequential;
    }
    return def.parts.some((p) => isSequential(LIB[p.chip]));
}

// ---- simulating ------------------------------------------------------------------

interface Instance {
    def: ChipDef;
    state: bigint[];
    nextState: bigint[];
    nets: bigint[];
    partIn: bigint[][];
    partOut: bigint[][];
    children: Instance[];
}

function instantiate(def: ChipDef): Instance {
    if (def.kind === 'builtin') {
        const n = def.state ?? 0;
        return { def, state: new Array(n).fill(0n), nextState: new Array(n).fill(0n), nets: [], partIn: [], partOut: [], children: [] };
    }
    const built = build(def);
    return {
        def,
        state: [],
        nextState: [],
        nets: built.nets.map(() => 0n),
        partIn: built.parts.map((p) => p.inputs.map(() => 0n)),
        partOut: built.parts.map((p) => p.outputs.map(() => 0n)),
        children: built.parts.map((p) => instantiate(p.def)),
    };
}

const getBits = (v: bigint, [start, width]: BitRange) => (v >> BigInt(start)) & mask(width);
const setBits = (v: bigint, [start, width]: BitRange, bits: bigint) => (v & ~(mask(width) << BigInt(start))) | ((bits & mask(width)) << BigInt(start));

/** Evaluates an instance's outputs from its inputs (combinational settle). */
function evaluate(inst: Instance, inputs: bigint[]): bigint[] {
    const def = inst.def;
    if (def.kind === 'builtin') {
        return def.eval(inputs, inst.state).map((v, i) => v & mask(def.outputs[i].width));
    }
    const built = build(def);
    def.inputs.forEach((_, i) => (inst.nets[i] = inputs[i] & mask(def.inputs[i].width)));
    for (let round = 0; round < 64; round++) {
        let changed = false;
        built.parts.forEach((part, p) => {
            const ins = part.inputs.map(() => 0n);
            for (const c of part.conns) {
                if (c.dir !== 'in') {
                    continue;
                }
                const v = c.constant !== undefined ? (c.constant ? mask(c.pinBits[1]) : 0n) : getBits(inst.nets[c.net!], c.netBits!);
                ins[c.pin] = setBits(ins[c.pin], c.pinBits, v);
            }
            inst.partIn[p] = ins;
            const outs = evaluate(inst.children[p], ins);
            inst.partOut[p] = outs;
            for (const c of part.conns) {
                if (c.dir !== 'out' || c.net === undefined) {
                    continue;
                }
                const next = setBits(inst.nets[c.net], c.netBits!, getBits(outs[c.pin], c.pinBits));
                if (next !== inst.nets[c.net]) {
                    inst.nets[c.net] = next;
                    changed = true;
                }
            }
        });
        if (!changed) {
            break;
        }
    }
    return def.outputs.map((_, i) => inst.nets[def.inputs.length + i]);
}

function clock(inst: Instance, phase: 'tick' | 'tock'): void {
    const def = inst.def;
    if (def.kind === 'builtin') {
        if (def.next) {
            if (phase === 'tick') {
                inst.nextState = def.next(inst.partIn[0] ?? [], inst.state);
            } else {
                inst.state = inst.nextState;
            }
        }
        return;
    }
    inst.children.forEach((child, p) => {
        if (child.def.kind === 'builtin') {
            child.partIn[0] = inst.partIn[p];
        }
        clock(child, phase);
    });
}

/** A chip being simulated, answering the panel's messages as the extension would. */
export class FixtureSession {
    readonly top: Composite;
    private root: Instance;
    private inputs: bigint[];
    private time = 0;
    private half = false;

    constructor(name: string) {
        const def = LIB[name];
        if (!def || def.kind !== 'composite') {
            throw new Error(`no fixture chip ${name}`);
        }
        this.top = def;
        this.root = instantiate(def);
        this.inputs = def.inputs.map(() => 0n);
        this.settle();
    }

    get clocked(): boolean {
        return isSequential(this.top);
    }

    private settle(): void {
        evaluate(this.root, this.inputs);
    }

    set(pin: string, value: string | number): void {
        const i = this.top.inputs.findIndex((p) => p.name === pin);
        if (i < 0) {
            throw new Error(`no input ${pin}`);
        }
        let text = String(value).trim();
        if (/^%B/i.test(text)) {
            text = `0b${text.slice(2)}`;
        } else if (/^%X/i.test(text)) {
            text = `0x${text.slice(2)}`;
        }
        const n = text.startsWith('-') ? -BigInt(text.slice(1)) : BigInt(text);
        this.inputs[i] = BigInt.asUintN(this.top.inputs[i].width, n);
        this.settle();
    }

    step(action: 'tick' | 'tock' | 'step' | 'reset'): void {
        if (action === 'reset') {
            this.root = instantiate(this.top);
            this.time = 0;
            this.half = false;
            this.settle();
            return;
        }
        if (action === 'tick' || action === 'step') {
            if (!this.half) {
                clock(this.root, 'tick');
                this.half = true;
                this.settle();
            }
        }
        if (action === 'tock' || action === 'step') {
            if (this.half) {
                clock(this.root, 'tock');
                this.half = false;
                this.time++;
                this.settle();
            }
        }
    }

    private at(path: number[]): Instance {
        let inst = this.root;
        for (const i of path) {
            inst = inst.children[i];
            if (!inst || inst.def.kind !== 'composite') {
                throw new Error(`no composite part at ${path.join('.')}`);
            }
        }
        return inst;
    }

    values(path: number[]): Values {
        const inst = this.at(path);
        const num = (v: bigint) => (v <= BigInt(Number.MAX_SAFE_INTEGER) ? Number(v) : v.toString());
        return {
            time: this.clocked ? `${this.time}${this.half ? '+' : ''}` : '',
            clocked: this.clocked,
            nets: inst.nets.map(num),
            parts: inst.partIn.map((ins, p) => ({ inputs: ins.map(num), outputs: (inst.partOut[p] ?? []).map(num) })),
        };
    }

    diagram(path: number[]): Diagram {
        const crumbs = [{ chip: this.top.name, path: [] as number[] }];
        let def: Composite = this.top;
        path.forEach((index, i) => {
            const child = build(def).parts[index].def as Composite;
            crumbs.push({ chip: child.name, path: path.slice(0, i + 1) });
            def = child;
        });
        const built = build(def);
        return {
            path,
            chip: def.name,
            file: `/examples/${def.name}.hdl`,
            breadcrumb: crumbs,
            inputs: def.inputs,
            outputs: def.outputs,
            nets: built.nets,
            parts: built.parts.map(({ def: _def, ...part }) => part),
            values: this.values(path),
        };
    }
}

/** A chip's diagram at its root, with all inputs 0 except those given. */
export function fixtureDiagram(name: string, inputs: Record<string, number> = {}): Diagram {
    const session = new FixtureSession(name);
    for (const [pin, value] of Object.entries(inputs)) {
        session.set(pin, value);
    }
    return session.diagram([]);
}

export const FIXTURES = ['VoteCheck', 'Accumulator', 'SetReset', 'Majority', 'Parity4', 'SwapBytes', 'Toggle'];
