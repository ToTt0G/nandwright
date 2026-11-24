// Gate symbols for the circuit diagram: which shape a part gets, and the SVG
// path data that draws it. Pure (no DOM), so the layout and its tests can use
// it too.

import type { Part } from './circuitTypes';

export type Shape = 'not' | 'and' | 'nand' | 'or' | 'nor' | 'xor' | 'xnor' | 'mux' | 'dmux' | 'box';

export interface Symbol {
    shape: Shape;
    /** Shown under a gate shape when the chip is a variant (`And16`, `Or8Way`, `Mux4Way16`). */
    variant?: string;
}

const GATES: Record<string, Shape> = {
    Not: 'not',
    And: 'and',
    Nand: 'nand',
    Or: 'or',
    Nor: 'nor',
    Xor: 'xor',
    Xnor: 'xnor',
};

/**
 * The shape for a part, by chip name: `Not`, `And`, `Nand`, `Or`, `Nor`,
 * `Xor` (and `Xnor`) and their 16-bit and multi-way variants get gate
 * shapes, `Mux*` and `DMux*` trapezoids, everything else a box. A chip whose
 * pins do not fit the shape (a home-made `And` with three inputs) is a box.
 */
export function symbolFor(part: Pick<Part, 'chip' | 'inputs' | 'outputs'>): Symbol {
    const chip = part.chip;
    const gate = /^(Not|And|Nand|Or|Nor|Xor|Xnor)(\d+Way|\d+)?$/.exec(chip);
    if (gate) {
        const shape = GATES[gate[1]];
        const inputs = shape === 'not' ? [1] : [1, 2];
        if (part.outputs.length === 1 && inputs.includes(part.inputs.length)) {
            return gate[2] ? { shape, variant: chip } : { shape };
        }
        return { shape: 'box' };
    }
    const mux = /^(D?Mux)(\d+Way)?(\d+)?$/.exec(chip);
    if (mux) {
        const sel = part.inputs.length > 0 && part.inputs[part.inputs.length - 1].name === 'sel';
        if (mux[1] === 'Mux' && sel && part.inputs.length >= 3 && part.outputs.length === 1) {
            return mux[2] || mux[3] ? { shape: 'mux', variant: chip } : { shape: 'mux' };
        }
        if (mux[1] === 'DMux' && sel && part.inputs.length === 2 && part.outputs.length >= 2) {
            return mux[2] || mux[3] ? { shape: 'dmux', variant: chip } : { shape: 'dmux' };
        }
    }
    return { shape: 'box' };
}

/** Width of a gate shape's bounding box, including the pin leads. */
export function gateWidth(shape: Shape): number {
    switch (shape) {
        case 'not':
            return 45;
        case 'mux':
            return 45;
        case 'dmux':
            return 54;
        default:
            return 63;
    }
}

/** Length of the pin leads drawn on each side of a gate body. */
export const LEAD = 9;

const r1 = (n: number) => Math.round(n * 10) / 10;

/**
 * The outline of a gate body in a box `w` × `h` (the shape area, without the
 * variant label), as SVG path data, plus the inversion bubble and the extra
 * curve of `Xor`, which are drawn as separate strokes.
 */
export function gatePaths(shape: Shape, w: number, h: number): { body: string; bubble?: { cx: number; cy: number; r: number }; extra?: string } {
    const x0 = LEAD;
    const x1 = w - LEAD;
    const top = 5;
    const bottom = h - 5;
    const mid = h / 2;
    const inverted = shape === 'nand' || shape === 'nor' || shape === 'xnor' || shape === 'not';
    const bubbleR = 3.5;
    const end = inverted ? x1 - bubbleR * 2 : x1;
    const bubble = inverted ? { cx: r1(end + bubbleR), cy: mid, r: bubbleR } : undefined;
    switch (shape) {
        case 'not': {
            const t = Math.max(top, mid - 13);
            const b = Math.min(bottom, mid + 13);
            return { body: `M${x0},${t} L${r1(end)},${mid} L${x0},${b} Z`, bubble };
        }
        case 'and':
        case 'nand': {
            const r = (bottom - top) / 2;
            const flat = end - r;
            return {
                body: `M${x0},${top} H${r1(flat)} A${r1(r)},${r1(r)} 0 0 1 ${r1(flat)},${bottom} H${x0} Z`,
                bubble,
            };
        }
        case 'or':
        case 'nor':
        case 'xor':
        case 'xnor': {
            const back = x0 - 1;
            const len = end - back;
            const bulge = 8;
            const body =
                `M${back},${top} ` +
                `C${r1(back + len * 0.55)},${top} ${r1(back + len * 0.88)},${r1(mid - (mid - top) * 0.35)} ${r1(end)},${mid} ` +
                `C${r1(back + len * 0.88)},${r1(mid + (bottom - mid) * 0.35)} ${r1(back + len * 0.55)},${bottom} ${back},${bottom} ` +
                `Q${back + bulge},${mid} ${back},${top} Z`;
            const extra = shape === 'xor' || shape === 'xnor' ? `M${back - 5},${top} Q${back - 5 + bulge},${mid} ${back - 5},${bottom}` : undefined;
            return { body, bubble, extra };
        }
        case 'mux': {
            const inset = Math.min(12, h / 5);
            return { body: `M${x0},${top - 2} L${x1},${r1(top - 2 + inset)} L${x1},${r1(bottom + 2 - inset)} L${x0},${bottom + 2} Z` };
        }
        case 'dmux': {
            const inset = Math.min(12, h / 5);
            return { body: `M${x0},${r1(top - 2 + inset)} L${x1},${top - 2} L${x1},${bottom + 2} L${x0},${r1(bottom + 2 - inset)} Z` };
        }
        case 'box':
            return { body: `M0,0 H${w} V${h} H0 Z` };
    }
}

/**
 * Where an input lead of a gate meets its body: the leads are drawn from the
 * bounding box edge to here, under the body, so curved backs look right.
 */
export function leadEnd(shape: Shape): number {
    return shape === 'or' || shape === 'nor' || shape === 'xor' || shape === 'xnor' ? LEAD + 4 : LEAD;
}
