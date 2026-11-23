// Messages between the extension and its runtime worker, and the shapes the
// engine returns. These mirror `crates/wasm`.

export type ToWorker =
    | { type: 'call'; id: number; request: Record<string, unknown> }
    | { type: 'runTest'; id: number; request: TestRunRequest }
    | { type: 'runMachine'; id: number; request: MachineRunRequest }
    | { type: 'cancel'; id: number };

export type FromWorker =
    | { type: 'reply'; id: number; reply: any }
    | { type: 'progress'; id: number; events: TestEvent[] }
    | { type: 'frame'; id: number; state: MachineState }
    | { type: 'log'; line: string };

export interface TestRunRequest {
    session: number;
    /** Zero-based lines of the script to stop at. */
    breakLines: number[];
    /** Run only to the end of the current `;` step, instead of to the end. */
    step?: boolean;
}

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
    | { type: 'key'; code: number }
    | { type: 'note'; text: string };

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

/**
 * The status of a test session. From one engine request it carries the
 * events and failures of that request. From a whole run it carries no
 * events (they arrive as they happen, through the progress callback) and
 * at most `MAX_REPORTED_FAILURES` failures.
 */
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

/** A run keeps the details of this many failing rows; the count is kept in full. */
export const MAX_REPORTED_FAILURES = 500;

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

/** A chip's compare file, as the simulator panel shows it. */
export interface Expected {
    path: string;
    name: string;
    header: string[];
    rows: string[][];
    /** The line of the file each row is on, from 1; a blank line is a line but not a row. */
    lines: number[];
    /** Rows left out because the file is long. */
    more: number;
}

/** The pins after one clock step in the chip simulator. */
export interface ClockStep {
    time: string;
    pins: Pin[];
}

/** The last run of a chip's test script, as the simulator panel shows it. */
export interface ChipTest {
    /** The script's file name. */
    script: string;
    status: 'none' | 'running' | 'current' | 'stale';
    kind?: TestOutcome['kind'];
    /** Lines of the compare file compared, the header included. */
    comparedLines?: number;
    failureCount?: number;
    /** Data rows of the compare file the run reached, and how many of them differ. */
    comparedRows?: number;
    differingRows?: number;
    message?: string;
    /** One mark per data row of the compare file shown, matched to it by line. */
    rows?: ('match' | 'differ' | 'unreached')[];
    /** For a differing row, the cells of the line the chip produced, unpadded spacing kept. */
    actual?: Record<number, string[]>;
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
    /**
     * `running` only while a run is in progress. A VM program can also be
     * `halted` (it ended), `waiting` (for a key) or `sleeping` (in `Sys.wait`).
     */
    status: 'running' | 'paused' | 'breakpoint' | 'error' | 'halted' | 'waiting' | 'sleeping';
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
    /** VM only: the function being executed. */
    function?: string;
    /** VM only: whether the program was compiled from Jack sources. */
    fromJack?: boolean;
    /** VM only: the active calls, innermost first. */
    callStack?: { function: string; file: string | null; line: number | null; locals: number[]; arguments: number[] }[];
    /** VM only: the top of the stack, newest first. */
    stack?: number[];
}

export interface MachineRunRequest {
    kind: 'cpu' | 'vm';
    session: number;
    /** Instructions per second; 0 runs as fast as possible. */
    speed: number;
    /** CPU: zero-based lines of the program's file. */
    breakLines: number[];
    /** VM: zero-based lines per source file. */
    breakpoints: { path: string; lines: number[] }[];
    ram: [number, number];
}
