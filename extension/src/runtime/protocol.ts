// Messages between the extension and its runtime worker, and the shapes the
// engine returns. These mirror `crates/wasm`.

/**
 * Session numbers. Every engine numbers its sessions from a range of its
 * own, so a session number kept from an engine that crashed, or from a
 * worker that was replaced, can never name a session of the engine or
 * worker that took over: it gets "this session has ended".
 *
 * Engine `e` of worker `w` (both counted from 0) starts at
 * `(w * ENGINES_PER_WORKER + e) * SESSIONS_PER_ENGINE + 1`. The engine takes
 * first sessions up to 2^52, which these sizes fill exactly.
 */
export const SESSIONS_PER_ENGINE = 2 ** 24;
export const ENGINES_PER_WORKER = 2 ** 14;
export const MAX_WORKERS = 2 ** 14;

/** What the extension gives each worker it starts. */
export interface WorkerStart {
    /** Which worker this is, from 0. */
    worker: number;
}

/** The first session number of engine `engine` of worker `worker`. */
export function firstSession(worker: number, engine: number): number {
    const index = (worker % MAX_WORKERS) * ENGINES_PER_WORKER + (engine % ENGINES_PER_WORKER);
    return index * SESSIONS_PER_ENGINE + 1;
}

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

export interface LoadedInput {
    path: string;
    builtFromSource: boolean;
}

export interface TestInspection {
    dialect: string;
    state: Pin[];
    outputPath: string | null;
    output: string | null;
    comparePath: string | null;
    expected: string[] | null;
    loaded?: LoadedInput[];
    echo: string | null;
    next: Range | null;
    commands: number;
}

export interface ChipState {
    session?: number;
    /** Which build of the chip this is: 1 when opened, +1 on each reset that loads. */
    revision?: number;
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
    /** Untrimmed cells per row, for exact character/wildcard comparison. */
    rawRows?: string[][];
    /** The line of the file each row is on, from 1; a blank line is a line but not a row. */
    lines: number[];
    /** Rows left out because the file is long. */
    more: number;
}

/** A reference screen image next to a program (e.g. project 12's `*Output.gif`). */
export interface ExpectedImage {
    path: string;
    name: string;
    dataUrl: string;
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
    /** Full path of the script shown, echoed back in panel actions. */
    scriptPath?: string;
    status: 'none' | 'running' | 'current' | 'stale';
    kind?: TestOutcome['kind'];
    /** Short description of the inputs the run tested (e.g. `assembled double.asm` or `Double.hack`). */
    sourceLabel?: string;
    /** Lines of the compare file compared, the header included. */
    comparedLines?: number;
    failureCount?: number;
    /** Data rows of the compare file the run reached, and how many of them differ. */
    comparedRows?: number;
    differingRows?: number;
    /** First failing line and explanation when the failure falls outside data rows. */
    firstFailureLine?: number;
    firstFailureReason?: string;
    message?: string;
    /** One mark per data row of the compare file shown, matched to it by line. */
    rows?: ('match' | 'differ' | 'unreached')[];
    /** For a differing row, the cells of the line the chip produced, unpadded spacing kept. */
    actual?: Record<number, string[]>;
}

/**
 * The circuit diagram ops of a chip session (docs/CIRCUIT_DIAGRAM.md):
 * `chip.diagram` replies with a `ChipDiagram`, `chip.values` with
 * `ChipValues`, for the composite instance at `path` (`[]` = the chip).
 */
export interface ChipDiagramRequest {
    op: 'chip.diagram' | 'chip.values';
    session: number;
    path: number[];
}
export type { Diagram as ChipDiagram, Values as ChipValues } from '../webview/circuitTypes';

export interface TruthTable {
    inputs: { name: string; width: number }[];
    outputs: { name: string; width: number }[];
    rows: { inputs: number[]; outputs: number[] }[];
}

/**
 * A reply saying the worker replaced its engine after a trap: every session
 * of the old engine is gone, as when a worker stops.
 */
export function endsSessions(reply: unknown): boolean {
    return isError(reply) && reply.error.crashed === true;
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
