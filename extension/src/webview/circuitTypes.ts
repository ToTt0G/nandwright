// The circuit diagram's data and messages, as docs/CIRCUIT_DIAGRAM.md defines
// them. Shared by the extension (circuitPanel.ts) and the webview
// (circuit.ts), so it must not import `vscode` or Node modules.

import type { Radix } from '../values';

/** A value as the engine sends it: a number when it fits in 53 bits, else a decimal string. */
export type Bits = number | string;

export interface PinDecl {
    name: string;
    width: number;
}

export interface NetDecl {
    name: string;
    width: number;
    kind: 'input' | 'output' | 'internal';
}

/** Where a part statement is in `Diagram.file`: offsets, and 0-based lines. */
export interface Span {
    start: number;
    end: number;
    line: number;
    endLine: number;
}

/** `[start, width]`, least-significant bit first. */
export type BitRange = [number, number];

/**
 * One `pin=net` of a part statement. `net` is an index into `Diagram.nets`;
 * a connection to `true` or `false` has `constant` instead.
 */
export interface Conn {
    dir: 'in' | 'out';
    pin: number;
    pinBits: BitRange;
    net?: number;
    netBits?: BitRange;
    constant?: boolean;
}

export interface Part {
    chip: string;
    drill: boolean;
    builtin: boolean;
    sequential: boolean;
    span: Span | null;
    inputs: PinDecl[];
    outputs: PinDecl[];
    /** Per output: the input indices it depends on combinationally. */
    deps: number[][];
    conns: Conn[];
}

export interface Crumb {
    chip: string;
    path: number[];
}

export interface PartValues {
    inputs: Bits[];
    outputs: Bits[];
}

export interface Values {
    /** The circuit these values are of: +1 each time the chip is built again (see `Diagram.revision`). */
    revision?: number;
    /** The level these values are of. */
    path?: number[];
    /** As chip.state; "" when not clocked. */
    time: string;
    clocked: boolean;
    /** One per net, in `Diagram.nets` order. */
    nets: Bits[];
    /** Each part's own pin values, in `Diagram.parts` order. */
    parts: PartValues[];
}

export interface Diagram {
    /**
     * 64-bit FNV-1a (hex) of the exact source text of `file` the build used:
     * span offsets are exact only in text with the same hash. Null for a built-in.
     */
    sourceHash?: string | null;
    /**
     * Which build of the chip this is: 1 when opened, +1 on each reset that
     * loads. Values of another revision number their nets differently.
     */
    revision?: number;
    path: number[];
    chip: string;
    file: string | null;
    breadcrumb: Crumb[];
    inputs: PinDecl[];
    outputs: PinDecl[];
    nets: NetDecl[];
    parts: Part[];
    values: Values;
}

export interface Settings {
    animate: boolean;
    radix: Radix;
    reducedMotion?: boolean;
}

export type ValuesCause = 'input' | 'clock' | 'reset' | 'external';

/** Extension → webview. */
export type ToCircuit =
    /** `token` names this diagram (for pin/part/net commands); `target` names the chip file (for clock/reset). */
    | { type: 'diagram'; diagram: Diagram; settings: Settings; token: number; target: number }
    | { type: 'values'; values: Values; cause: ValuesCause }
    | { type: 'highlight'; part?: number; net?: number }
    /** Keep the last diagram, greyed, under the message; `retry`: the simulator stopped for good, offer Retry. */
    | { type: 'error'; message: string; retry?: boolean }
    /** A short note that goes away by itself (an input dropped because the chip was rebuilt, or busy). */
    | { type: 'note'; message: string }
    | { type: 'settings'; settings: Settings };

/**
 * Webview → extension: a command about the diagram shown. `diagram` is the
 * `token` of the diagram it was given on; the extension drops a command
 * whose diagram has since been replaced (another chip, a reload), since its
 * pin, part and net numbers may mean something else now. (Clock buttons carry
 * `target` instead: they survive rebuilds of the same chip, in click order,
 * but are dropped after the panel switches to another file.)
 */
export type DiagramCommand =
    | { type: 'set'; pin: string; value: string | number; diagram: number }
    | { type: 'drill'; path: number[]; diagram: number }
    | { type: 'reveal'; part?: number; net?: number; diagram: number };

/** A command without its diagram or file-target token (filled in by whoever sends it). */
export type Unbound<T> = T extends { diagram: number }
    ? Omit<T, 'diagram'> & { diagram?: number }
    : T extends { target: number }
      ? Omit<T, 'target'> & { target?: number }
      : T;

/** Webview → extension. */
export type FromCircuit =
    | { type: 'ready' }
    | DiagramCommand
    /** Tick, Tock, Step, Reset: on the chip's session across rebuilds, dropped if the panel switched files. */
    | { type: 'clock'; action: 'tick' | 'tock' | 'step' | 'reset'; target: number }
    /** Retry after the simulator stopped for good. */
    | { type: 'retry' }
    /** The webview has drawn the diagram with this token. */
    | { type: 'drawn'; diagram: number }
    | { type: 'export'; format: 'svg' | 'png'; data: string }
    | { type: 'settings'; animate?: boolean; radix?: Radix }
    /** A webview-side failure, for the log; `notify` also shows it to the student (a failed export). */
    | { type: 'error'; message: string; notify?: boolean };
