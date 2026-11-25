// The pure helpers behind "inspect this test row" in the computer panel.

import assert from 'node:assert/strict';
import { test } from 'node:test';
import { inspectedAddress, memoryStartForAddress, parseTargetColumn, rowMismatches } from '../../inspectRow';
import type { Expected } from '../../runtime/protocol';

test('parseTargetColumn reads RAM cells and registers, case-insensitively', () => {
    assert.deepEqual(parseTargetColumn('RAM[0]'), { target: 'RAM', address: 0 });
    assert.deepEqual(parseTargetColumn(' RAM[261] '), { target: 'RAM', address: 261 });
    assert.deepEqual(parseTargetColumn('ram[16384]'), { target: 'RAM', address: 16384 });
    assert.deepEqual(parseTargetColumn('RAM[24576]'), { target: 'RAM', address: 24576 });
    assert.equal(parseTargetColumn('RAM[24577]'), null);
    for (const name of ['A', 'D', 'PC', 'SP', 'LCL', 'ARG', 'THIS', 'THAT']) {
        assert.deepEqual(parseTargetColumn(name), { target: 'register', name });
        assert.deepEqual(parseTargetColumn(name.toLowerCase()), { target: 'register', name });
    }
    assert.deepEqual(parseTargetColumn('local'), { target: 'register', name: 'LCL' });
    assert.deepEqual(parseTargetColumn('argument'), { target: 'register', name: 'ARG' });
    assert.deepEqual(parseTargetColumn('PC[]'), { target: 'register', name: 'PC' });
    assert.equal(parseTargetColumn('time'), null);
    assert.equal(parseTargetColumn('out'), null);
    assert.equal(parseTargetColumn('RAM'), null);
    assert.equal(parseTargetColumn('RAM[x]'), null);
});

const expected: Expected = {
    path: '/work/Mult.cmp',
    name: 'Mult.cmp',
    header: ['RAM[0]', 'RAM[1]', 'RAM[2]', 'time', 'SP'],
    rows: [
        ['3', '1', '3', '1*', '256'],
        ['2', '4', '8', '2', '256'],
    ],
    rawRows: [
        ['     3 ', '     1 ', '     3 ', ' 1*', '   256 '],
        ['     2 ', '     4 ', '     8 ', ' 2 ', '   256 '],
    ],
    lines: [2, 3],
    more: 0,
};

test('rowMismatches lists every differing column with its machine target', () => {
    assert.deepEqual(rowMismatches(expected, ['     3 ', '     1 ', '     3 ', ' 17', '   256 '], 0), []);
    assert.deepEqual(rowMismatches(expected, ['     2 ', '     4 ', '     6 ', ' 3 ', '   261 '], 1), [
        { column: 'RAM[2]', target: 'RAM', address: 2, expected: '8', actual: '6' },
        { column: 'time', target: 'other', expected: '2', actual: '3' },
        { column: 'SP', target: 'register', name: 'SP', expected: '256', actual: '261' },
    ]);
    // Spacing alone differs, as the engine sees it.
    assert.equal(rowMismatches(expected, ['    2  ', '     4 ', '     8 ', ' 2 ', '   256 '], 1)[0]?.column, 'RAM[0]');
    // Missing and extra cells are reported rather than silently treated as matching.
    assert.deepEqual(rowMismatches(expected, ['     2 ', '     4 ', '     8 ', ' 2 '], 1), [
        { column: 'SP', target: 'register', name: 'SP', expected: '256', actual: '(missing)' },
    ]);
    assert.deepEqual(rowMismatches(expected, ['     2 ', '     4 ', '     8 ', ' 2 ', '   256 ', '  99 '], 1), [
        { column: 'column 6', target: 'other', expected: '(none)', actual: '99' },
    ]);
    assert.deepEqual(rowMismatches(expected, undefined, 1), []);
    assert.deepEqual(rowMismatches(expected, ['1'], 9), []);
});

test('memoryStartForAddress keeps the address near the top of a clamped window', () => {
    assert.equal(memoryStartForAddress(0), 0);
    assert.equal(memoryStartForAddress(15), 0);
    assert.equal(memoryStartForAddress(16), 14);
    assert.equal(memoryStartForAddress(261), 259);
    assert.equal(memoryStartForAddress(24576), 24576 - 16 + 1);
    assert.equal(memoryStartForAddress(100, 8), 98);
});

test('inspectedAddress prefers a differing RAM cell, else the first RAM column', () => {
    assert.equal(inspectedAddress(expected, [{ column: 'RAM[2]', target: 'RAM', address: 2, expected: '8', actual: '6' }]), 2);
    assert.equal(inspectedAddress(expected, []), 0);
    assert.equal(inspectedAddress({ ...expected, header: ['time', 'out'] }, []), null);
});
