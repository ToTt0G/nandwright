// The rules for keeping test results: which result wins, what is saved
// between sessions and what comes back, and how a run's failures map onto
// the rows of its compare file. No `vscode` here: the unit tests run it in
// plain Node.

/** One source or artifact file a test run actually loaded. */
export interface RunInput {
    path: string;
    builtFromSource: boolean;
}

/** The last result of a script, kept for the simulator, the status bar and the editor. */
export interface StoredResult {
    kind: 'passed' | 'unchecked' | 'failed' | 'error' | 'limit' | 'skipped' | 'cancelled';
    /** Lines of the compare file compared, the header included. */
    comparedLines: number;
    failureCount: number;
    /** Data rows of the compare file the run reached (blank lines and the header are not rows). */
    rows?: number;
    /** Of those, the rows that differ. */
    differingRows?: number;
    /** The compare-file line and what the chip gave there, for the failures kept. */
    failures: { line: number; actual: string | null }[];
    notes: string[];
    /** The error message, or why there is no result (limit, skipped, cancelled, unchecked). */
    message?: string;
    /** The exact files loaded by the engine during this run (for file provenance). */
    inputs?: RunInput[];
    /** Compare and output file paths used by this run. */
    comparePath?: string;
    outputPath?: string;
    /** In-memory snapshot of the `.out` produced by this run (not persisted). */
    outputText?: string;
    /** When the engine was given the files the run used. */
    started: number;
    finished: number;
}

function normalizePathForMatch(file: string, ignoreCase: boolean): string {
    const normalized = file.replace(/\\/g, '/').replace(/\/+/g, '/');
    return ignoreCase ? normalized.toLowerCase() : normalized;
}

/**
 * Whether `result` actually tested `filePath` (so a source build of `double.asm`
 * never reports a green pass on a broken `Double.hack`, and vice versa).
 */
export function resultMatchesFile(
    result: Pick<StoredResult, 'inputs'>,
    filePath: string,
    ignoreCase: boolean,
): boolean {
    const dot = filePath.lastIndexOf('.');
    const ext = dot > 0 ? filePath.slice(dot + 1).toLowerCase() : '';
    if (ext === 'hdl' || ext === 'tst') {
        return true;
    }
    const inputs = result.inputs;
    if (!inputs || inputs.length === 0) {
        return false;
    }
    const target = normalizePathForMatch(filePath, ignoreCase);
    const same = (candidate: string) => normalizePathForMatch(candidate, ignoreCase) === target;
    switch (ext) {
        case 'hack':
        case 'vm':
            return inputs.some((input) => !input.builtFromSource && same(input.path));
        case 'asm':
            return inputs.some((input) => same(input.path));
        case 'jack':
            return inputs.some((input) => input.builtFromSource && same(input.path));
        default:
            return true;
    }
}

/**
 * Whether a result replaces the one stored. A cancelled run says nothing
 * about the chip, and a run that saw older files than the stored result
 * did (an overlapping run that finished later) must not hide it.
 */
export function replaces(stored: StoredResult | undefined, incoming: StoredResult): boolean {
    return incoming.kind !== 'cancelled' && (!stored || incoming.started >= stored.started);
}

/**
 * The rows of a compare file a run reached, and how many differ. `expected`
 * is the compare file as the engine read it, one entry per physical line.
 * A failure past the end of the file (the script wrote more lines) or on a
 * blank line differs, but is not a row.
 */
export function countRows(expected: string[], comparedLines: number, failureLines: number[]): { rows: number; differingRows: number } {
    const isRow = (line: number) => line >= 2 && line <= expected.length && expected[line - 1].trim() !== '';
    let rows = 0;
    for (let line = 2; line <= Math.min(comparedLines, expected.length); line++) {
        rows += isRow(line) ? 1 : 0;
    }
    const differingRows = new Set(failureLines.filter(isRow)).size;
    return { rows, differingRows };
}

/** One saved result, with what is needed to tell later whether it still holds. */
export interface SavedResult extends StoredResult {
    path: string;
    /** The engine files of the script's folder when it was saved, by name. */
    files: string[];
}

/** A folder's engine files as they are now: name and modification time. */
export type FolderListing = { name: string; mtimeMs: number }[] | null;

/**
 * What to save of the results: only those still current. A result that went
 * stale is as good as no result, and saving it would let the files' times
 * on disk (older than the run) make it look current again after a reload.
 */
export function toSave(
    entries: { path: string; result: StoredResult; current: boolean }[],
    listFolder: (file: string) => FolderListing,
): Record<string, SavedResult> {
    const saved: Record<string, SavedResult> = {};
    for (const { path, result, current } of entries) {
        const listing = current ? listFolder(path) : null;
        if (!listing) {
            continue;
        }
        const { outputText: _outputText, ...persisted } = result;
        saved[path] = {
            ...persisted,
            path,
            notes: result.notes.slice(0, 5),
            files: listing.map((file) => file.name).sort(),
        };
    }
    return saved;
}

/**
 * The saved results that still hold: the script is there, no engine file
 * of its folder was added, removed or changed after the run started.
 */
export function restore(saved: Record<string, SavedResult> | undefined, listFolder: (file: string) => FolderListing): { path: string; result: StoredResult }[] {
    const restored: { path: string; result: StoredResult }[] = [];
    for (const entry of Object.values(saved ?? {})) {
        if (!entry || typeof entry.path !== 'string' || !Array.isArray(entry.files)) {
            continue;
        }
        const listing = listFolder(entry.path);
        if (!listing) {
            continue;
        }
        const names = listing.map((file) => file.name).sort();
        const same = names.length === entry.files.length && names.every((name, i) => name === entry.files[i]);
        if (!same || listing.some((file) => file.mtimeMs > entry.started)) {
            continue;
        }
        const { path, files: _files, ...result } = entry;
        restored.push({ path, result });
    }
    return restored;
}

/**
 * The marks of the rows shown, one per data row, from a result. `lines[i]`
 * is the compare-file line of row `i`. Rows after the last failure kept are
 * not known when failures were left out, so they are not called matching.
 */
export function rowMarks(
    lines: number[],
    result: Pick<StoredResult, 'comparedLines' | 'failures' | 'failureCount'>,
): ('match' | 'differ' | 'unreached')[] {
    const failed = new Set(result.failures.map((failure) => failure.line));
    const complete = result.failures.length >= result.failureCount;
    const lastKnown = complete ? Infinity : Math.max(0, ...result.failures.map((failure) => failure.line));
    return lines.map((line) => {
        if (failed.has(line)) {
            return 'differ';
        }
        return line <= result.comparedLines && line <= lastKnown ? 'match' : 'unreached';
    });
}
