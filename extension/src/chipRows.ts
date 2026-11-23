// The marks of the expected-output rows in the simulator, from a result.

import { StoredResult, rowMarks } from './resultStore';
import { rawCells } from './values';

/**
 * One mark per row shown, and for a differing row the cells the chip gave.
 * Rows are matched to failures by their line in the compare file, which is
 * how the engine counts (a blank line is a line, but not a row).
 */
export function resultRows(
    lines: number[],
    result: Pick<StoredResult, 'comparedLines' | 'failures' | 'failureCount'>,
): { rows: ('match' | 'differ' | 'unreached')[]; actual: Record<number, string[]> } {
    const rows = rowMarks(lines, result);
    const byLine = new Map(result.failures.map((failure) => [failure.line, failure.actual]));
    const actual: Record<number, string[]> = {};
    rows.forEach((mark, i) => {
        const line = byLine.get(lines[i]);
        if (mark === 'differ' && line !== null && line !== undefined) {
            actual[i] = rawCells(line);
        }
    });
    return { rows, actual };
}
