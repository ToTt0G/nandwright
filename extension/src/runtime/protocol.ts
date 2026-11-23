// Messages between the extension and its runtime worker, and the shapes the
// engine returns. These mirror `crates/wasm`.

export type ToWorker =
    | { type: 'call'; id: number; request: Record<string, unknown> }
    | { type: 'runTest'; id: number; request: Record<string, unknown> }
    | { type: 'cancel'; id: number };

export type FromWorker =
    | { type: 'reply'; id: number; reply: any }
    | { type: 'progress'; id: number; events: TestEvent[] }
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
    return typeof reply === 'object' && reply !== null && 'error' in reply;
}
