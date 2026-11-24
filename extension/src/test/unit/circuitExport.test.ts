// Sizing a PNG export of the circuit diagram, and checking what the canvas gave.

import assert from 'node:assert/strict';
import { test } from 'node:test';
import { MAX_CANVAS_AREA, MAX_CANVAS_SIDE, isPngBase64, pngSize } from '../../webview/circuitExport';

test('a diagram of usual size is exported at 2×', () => {
    assert.deepEqual(pngSize(800, 600), { width: 1600, height: 1200, scale: 2 });
});

test('a very wide diagram is scaled down to fit the canvas limits', () => {
    // 73,143 units wide would be a 146,286-pixel canvas at 2×.
    const size = pngSize(73_143, 900)!;
    assert.ok(size.width <= MAX_CANVAS_SIDE && size.height <= MAX_CANVAS_SIDE, JSON.stringify(size));
    assert.ok(size.width * size.height <= MAX_CANVAS_AREA);
    assert.ok(size.scale < 2);
    assert.ok(Math.abs(size.width / size.height - 73_143 / 900) / (73_143 / 900) < 0.02, 'the aspect ratio is kept');
});

test('a large square diagram stays under the area limit', () => {
    const size = pngSize(15_000, 15_000)!;
    assert.ok(size.width * size.height <= MAX_CANVAS_AREA, JSON.stringify(size));
    assert.ok(size.width <= MAX_CANVAS_SIDE);
});

test('an empty drawing has no PNG size', () => {
    assert.equal(pngSize(0, 100), null);
    assert.equal(pngSize(Number.NaN, 100), null);
});

test('only real PNG data passes the check', () => {
    const png = Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 13, 73, 72, 68, 82]).toString('base64');
    assert.ok(isPngBase64(png));
    // What `toDataURL` leaves after removing the prefix when the canvas was too large: `data:,`.
    assert.equal(isPngBase64('data:,'), false);
    assert.equal(isPngBase64(''), false);
    assert.equal(isPngBase64(Buffer.from('GIF89a and some more bytes').toString('base64')), false);
});
