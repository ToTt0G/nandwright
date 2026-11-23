// The rules for keeping test results: which result wins, what is saved and
// restored between sessions, and how failures map onto compare-file rows.

import assert from 'node:assert/strict';
import { test } from 'node:test';
import { resultRows } from '../../chipRows';
import { FolderListing, SavedResult, StoredResult, countRows, replaces, restore, toSave } from '../../resultStore';
import { parseCompare } from '../../values';

const result = (overrides: Partial<StoredResult>): StoredResult => ({
    kind: 'passed',
    comparedLines: 9,
    failureCount: 0,
    failures: [],
    notes: [],
    started: 1000,
    finished: 2000,
    ...overrides,
});

test('a run that saw older files does not replace a newer result, and a cancelled run replaces nothing', () => {
    const newer = result({ started: 5000, kind: 'passed' });
    // A slow run that started first and finished last.
    assert.equal(replaces(newer, result({ started: 4000, finished: 9000, kind: 'failed' })), false);
    assert.equal(replaces(newer, result({ started: 6000 })), true);
    assert.equal(replaces(newer, result({ started: 6000, kind: 'cancelled' })), false);
    assert.equal(replaces(undefined, result({})), true);
});

test('only current results are saved, with every failure kept', () => {
    const listing: FolderListing = [{ name: 'A.tst', mtimeMs: 10 }];
    const failures = Array.from({ length: 60 }, (_, i) => ({ line: i + 2, actual: `|${i}|` }));
    const saved = toSave(
        [
            { path: '/p/A.tst', result: result({ kind: 'failed', failureCount: 60, failures, comparedLines: 61 }), current: true },
            { path: '/p/B.tst', result: result({}), current: false },
        ],
        () => listing,
    );
    assert.deepEqual(Object.keys(saved), ['/p/A.tst']);
    assert.equal(saved['/p/A.tst'].failures.length, 60);
});

test('a saved result comes back only while its folder is as it was', () => {
    const saved: Record<string, SavedResult> = {
        '/p/A.tst': { ...result({ started: 1000 }), path: '/p/A.tst', files: ['A.cmp', 'A.hdl', 'A.tst'] },
    };
    const same: FolderListing = [
        { name: 'A.tst', mtimeMs: 900 },
        { name: 'A.hdl', mtimeMs: 900 },
        { name: 'A.cmp', mtimeMs: 900 },
    ];
    assert.equal(restore(saved, () => same).length, 1);
    assert.equal(restore(saved, () => same)[0].result.started, 1000);
    // A file changed after the run.
    assert.equal(restore(saved, () => [...same.slice(0, 2), { name: 'A.cmp', mtimeMs: 1500 }]).length, 0);
    // A file deleted while VS Code was closed: what is left is older than the run.
    assert.equal(restore(saved, () => same.slice(0, 2)).length, 0);
    // A file added.
    assert.equal(restore(saved, () => [...same, { name: 'B.hdl', mtimeMs: 10 }]).length, 0);
    // The script is gone.
    assert.equal(restore(saved, () => null).length, 0);
    // Something that is not a saved result.
    assert.equal(restore({ x: 3 } as unknown as Record<string, SavedResult>, () => same).length, 0);
});

test('rows are counted as the compare file has them: no header, no blank lines, no extra output', () => {
    const expected = ['|h|', '|0|', '|1|', '', '|2|'];
    // Every line compared; the blank line and an extra line of output differ, but are not rows.
    assert.deepEqual(countRows(expected, 5, [4, 6]), { rows: 3, differingRows: 0 });
    assert.deepEqual(countRows(expected, 5, [3, 4, 5]), { rows: 3, differingRows: 2 });
    // The script stopped after the first row.
    assert.deepEqual(countRows(expected, 2, []), { rows: 1, differingRows: 0 });
});

test('row marks follow the line each row is on, across a blank line', () => {
    const cmp = parseCompare(['|a|out|', '|0|0|', '|1|1|', '', '|2|0|', '|3|1|'].join('\n') + '\n', 256)!;
    assert.deepEqual(cmp.lines, [2, 3, 5, 6]);
    // The blank line 4 was compared against output the chip wrote there; the
    // rows after it are compared with the chip's lines shifted by one.
    const { rows, actual } = resultRows(cmp.lines, {
        comparedLines: 5,
        failureCount: 2,
        failures: [
            { line: 4, actual: '|2|0|' },
            { line: 5, actual: '|3|1|' },
        ],
    });
    assert.deepEqual(rows, ['match', 'match', 'differ', 'unreached']);
    assert.deepEqual(actual, { 2: ['3', '1'] });
});

test('rows after the last failure kept are not called matching when failures were left out', () => {
    const lines = [2, 3, 4, 5];
    const { rows } = resultRows(lines, { comparedLines: 5, failureCount: 3, failures: [{ line: 2, actual: '|x|' }] });
    assert.deepEqual(rows, ['differ', 'unreached', 'unreached', 'unreached']);
});
