// The line rule of "Compare with Reference Output".

import assert from 'node:assert/strict';
import { test } from 'node:test';
import { comparedLines } from '../../textCompare';

test('spaces and tabs inside a line do not count, as in the TextComparer', () => {
    const indented = '<tokens>\n  <keyword> class </keyword>\n</tokens>\n';
    const flat = '<tokens>\r\n<keyword>class</keyword>\r\n\t</tokens>';
    assert.deepEqual(comparedLines(indented), comparedLines(flat));
});

test('a different, missing or moved blank line still counts', () => {
    assert.notDeepEqual(comparedLines('push constant 1\n'), comparedLines('push constant 2\n'));
    assert.notDeepEqual(comparedLines('a\nb\n'), comparedLines('a\n'));
    assert.notDeepEqual(comparedLines('a\n\nb\n'), comparedLines('a\nb\n\n'));
});

test('trailing blank lines and line endings do not count', () => {
    assert.deepEqual(comparedLines('a\r\nb\r\n\r\n'), ['a', 'b']);
    assert.deepEqual(comparedLines('a\rb'), ['a', 'b']);
});
