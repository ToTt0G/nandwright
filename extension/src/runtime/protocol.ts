// Messages between the extension and its runtime worker, and the shapes the
// engine returns. These mirror `crates/wasm`.

export type ToWorker =
    | { type: 'call'; id: number; request: Record<string, unknown> }
    | { type: 'runTest'; id: number; request: Record<string, unknown> }
    | { type: 'runMachine'; id: number; request: MachineRunRequest }
    | { type: 'cancel'; id: number };

export type FromWorker =
    | { type: 'reply'; id: number; reply: any }
    | { type: 'progress'; id: number; events: TestEvent[] }
    | { type: 'frame'; id: number; state: MachineState }
    | { type: 'log'; line: string };

export interface EngineError {
    error: { message: string; crashed?: boolean };
}

export interface Range {
    startLine: number;
    startCharacter: number;
    endLine: number;
    endCharacter: number;
}

export type Role = 'input' | 'output' | 'internal' | 'register' | 'memory';

export interface Pin {
    name: string;
    role: Role;
    width: number;
    /** A number, or a decimal string for values wider than 32 bits. */
    bits: number | string;
}

export type TestEvent =
    | { type: 'echo'; text: string }
    | { type: 'clearEcho' }
    | { type: 'output'; line: string }
    | { type: 'key'; code: number };

export interface TestFailure {
    /** The 1-based line of the compare file. */
    line: number;
    column: number | null;
    expected: string | null;
    actual: string | null;
    range: Range;
    state: Pin[];
}

export type TestOutcome =
    | { kind: 'passed'; comparedLines: number; unreachedLines: number }
    | { kind: 'unchecked' }
    | {
          kind: 'failed';
          line: number;
          column: number | null;
          expected: string | null;
          actual: string | null;
          message: string;
          range: Range | null;
          failureCount: number;
      }
    | { kind: 'error'; message: string; range: Range | null }
    | { kind: 'limit'; echo: string | null; commands: number }
    | { kind: 'skipped'; reason: string }
    | { kind: 'cancelled' };

export interface TestStatus {
    state: 'suspended' | 'finished';
    reason?: 'budget' | 'pause' | 'breakpoint' | 'step';
    at?: Range;
    next?: Range | null;
    outcome?: TestOutcome;
    events: TestEvent[];
    failures: TestFailure[];
    commands: number;
    echo: string | null;
}

export interface TestOpened {
    session: number;
    dialect: 'hardware' | 'cpu' | 'vm';
    next: Range | null;
    /** The script has a `repeat` without a count: it never ends by itself. */
    runsForever: boolean;
}

export interface TestInspection {
    dialect: string;
    state: Pin[];
    outputPath: string | null;
    output: string | null;
    comparePath: string | null;
    expected: string[] | null;
    echo: string | null;
    next: Range | null;
    commands: number;
}

export interface ChipState {
    session?: number;
    chip: string;
    clocked: boolean;
    time: string;
    pins: Pin[];
}

export interface TruthTable {
    inputs: { name: string; width: number }[];
    outputs: { name: string; width: number }[];
    rows: { inputs: number[]; outputs: number[] }[];
}

export function isError(reply: unknown): reply is EngineError {
    // A machine state has an `error` string of its own; an engine error is
    // an object with a message.
    const error = (reply as { error?: unknown } | null)?.error;
    return typeof error === 'object' && error !== null && 'message' in error;
}

/** A running Hack computer (CPU emulator) or VM, as the panel shows it. */
export interface MachineState {
    session?: number;
    kind: 'cpu' | 'vm';
    /** `running` only while a run is in progress. */
    status: 'running' | 'paused' | 'breakpoint' | 'error';
    error: string | null;
    registers: { name: string; value: number | null }[];
    /** The instruction about to execute, disassembled. */
    instruction: string;
    steps: number;
    programLength: number;
    file: string | null;
    /** Zero-based source line of the instruction about to execute. */
    line: number | null;
    key: number;
    ram: { start: number; values: number[] } | null;
    /** The 8192 screen words as hexadecimal, when the screen has changed. */
    screen: string | null;
}

export interface MachineRunRequest {
    kind: 'cpu' | 'vm';
    session: number;
    /** Instructions per second; 0 runs as fast as possible. */
    speed: number;
    breakLines: number[];
    ram: [number, number];
}
