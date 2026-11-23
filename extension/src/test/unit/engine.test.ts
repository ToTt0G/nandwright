// The engine as the extension loads it: the real .wasm file through the real
// loader, against real files. This runs on every platform in CI, which is
// where a path or line-ending mistake on Windows would show up.

import assert from 'node:assert/strict';
import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';
import { test } from 'node:test';
import { WasmEngine } from '../../engine/wasm';
import { ignoresCase, pathKey, samePath } from '../../paths';
import { formatValue, typedValue } from '../../values';

const root = path.resolve(__dirname, '..', '..');
const wasm = path.join(root, 'dist', 'engine.wasm');
const fixtures = path.join(root, '..', 'tests', 'fixtures');

/** A scratch copy of the fixture chips in a folder with a space in its name. */
function scratch(): string {
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'nandwright unit '));
    for (const name of fs.readdirSync(path.join(fixtures, 'chips'))) {
        fs.copyFileSync(path.join(fixtures, 'chips', name), path.join(dir, name));
    }
    for (const name of ['Stub.hdl', 'Stub.tst', 'Stub.cmp']) {
        fs.copyFileSync(path.join(fixtures, 'runtime', name), path.join(dir, name));
    }
    return dir;
}

function runToEnd(engine: WasmEngine, session: number): any {
    for (let i = 0; i < 100000; i++) {
        const status = engine.call({ op: 'test.run', session, budget: 1000 });
        assert.equal(status.error, undefined, JSON.stringify(status));
        if (status.state === 'finished') {
            return status;
        }
    }
    assert.fail('the script did not finish');
}

test('runs every fixture script from native paths', () => {
    const dir = scratch();
    const engine = new WasmEngine(wasm);
    const expected: Record<string, string> = {
        'Accumulator.tst': 'passed',
        'KeyLatch.tst': 'passed',
        'Majority.tst': 'passed',
        'Parity4.tst': 'passed',
        'Scratchpad.tst': 'passed',
        'SwapBytes.tst': 'passed',
        'Stub.tst': 'failed',
    };
    for (const [script, outcome] of Object.entries(expected)) {
        // The native path, exactly as VS Code's `fsPath` gives it.
        const opened = engine.call({ op: 'test.open', path: path.join(dir, script), crlf: os.EOL === '\r\n' });
        assert.equal(opened.error, undefined, `${script}: ${JSON.stringify(opened)}`);
        const status = runToEnd(engine, opened.session);
        assert.equal(status.outcome.kind, outcome, `${script}: ${JSON.stringify(status.outcome)}`);
        engine.call({ op: 'close', session: opened.session });
    }
    // The output file was written next to the script, with the platform's
    // line endings, and matches the compare file line for line.
    const out = fs.readFileSync(path.join(dir, 'Majority.out'), 'utf8');
    const cmp = fs.readFileSync(path.join(dir, 'Majority.cmp'), 'utf8');
    assert.equal(out.includes('\r\n'), os.EOL === '\r\n');
    assert.deepEqual(out.split(/\r?\n/), cmp.split(/\r?\n/));
    fs.rmSync(dir, { recursive: true, force: true });
});

test('reads files that are not UTF-8 and files with CRLF and a BOM', () => {
    const dir = scratch();
    // A Windows-1252 dash in a comment, as in the course's own built-in ALU.hdl.
    const hdl = fs.readFileSync(path.join(dir, 'Majority.hdl'));
    const crlf = Buffer.from(hdl.toString('latin1').replace(/\r?\n/g, '\r\n'), 'latin1');
    fs.writeFileSync(path.join(dir, 'Majority.hdl'), Buffer.concat([Buffer.from('// a \x96 b\r\n', 'latin1'), crlf]));
    const tst = fs.readFileSync(path.join(dir, 'Majority.tst'), 'utf8').replace(/\r?\n/g, '\r\n');
    fs.writeFileSync(path.join(dir, 'Majority.tst'), `﻿${tst}`);
    const engine = new WasmEngine(wasm);
    const opened = engine.call({ op: 'test.open', path: path.join(dir, 'Majority.tst') });
    assert.equal(opened.error, undefined, JSON.stringify(opened));
    assert.equal(runToEnd(engine, opened.session).outcome.kind, 'passed');
    fs.rmSync(dir, { recursive: true, force: true });
});

test('reports a missing file and a missing folder as such', () => {
    const dir = scratch();
    const engine = new WasmEngine(wasm);
    const missing = engine.call({ op: 'test.open', path: path.join(dir, 'Nope.tst') });
    assert.match(missing.error.message, /cannot read `Nope\.tst`: the file does not exist/);
    const nowhere = engine.call({ op: 'chip.open', path: path.join(dir, 'no such folder', 'X.hdl') });
    assert.match(nowhere.error.message, /cannot load `X\.hdl`/);
    fs.rmSync(dir, { recursive: true, force: true });
});

test('an interactive chip works through the loader', () => {
    const dir = scratch();
    const engine = new WasmEngine(wasm);
    const chip = engine.call({ op: 'chip.open', path: path.join(dir, 'Majority.hdl') });
    assert.equal(chip.chip, 'Majority');
    engine.call({ op: 'chip.set', session: chip.session, pin: 'a', value: 1 });
    const state = engine.call({ op: 'chip.set', session: chip.session, pin: 'b', value: 1, eval: true });
    assert.equal(state.pins.find((p: any) => p.name === 'out').bits, 1);
    const table = engine.call({ op: 'chip.table', session: chip.session });
    assert.equal(table.rows.length, 8);
    fs.rmSync(dir, { recursive: true, force: true });
});

test('unsaved text shadows the file, whatever the case of the drive letter', () => {
    const dir = scratch();
    const engine = new WasmEngine(wasm);
    const file = path.join(dir, 'Majority.hdl');
    const broken = fs.readFileSync(file, 'utf8').replace('Or(a=abOrAc, b=bc, out=out);', 'And(a=abOrAc, b=bc, out=out);');
    // VS Code lower-cases the drive letter in `fsPath`; Node keeps it upper-case.
    const spelled = file.replace(/^([A-Za-z]):/, (_, drive: string) => `${drive.toLowerCase()}:`);
    engine.call({ op: 'overlay.set', path: spelled, text: broken });
    const opened = engine.call({ op: 'test.open', path: path.join(dir, 'Majority.tst') });
    assert.equal(runToEnd(engine, opened.session).outcome.kind, 'failed');
    engine.call({ op: 'overlay.clear' });
    const again = engine.call({ op: 'test.open', path: path.join(dir, 'Majority.tst') });
    assert.equal(runToEnd(engine, again.session).outcome.kind, 'passed');
    fs.rmSync(dir, { recursive: true, force: true });
});

test('the language server answers through the same engine', () => {
    const engine = new WasmEngine(wasm);
    const reply = engine.call({
        op: 'lsp',
        message: JSON.stringify({ jsonrpc: '2.0', id: 1, method: 'initialize', params: { capabilities: {} } }),
    });
    const message = JSON.parse(reply.messages[0]);
    assert.equal(message.result.serverInfo.name, 'nandwright');
});

test('many calls do not leak engine memory', () => {
    const dir = scratch();
    const engine = new WasmEngine(wasm);
    const memory = () => (engine as any).exports.memory.buffer.byteLength as number;
    const cycle = () => {
        const opened = engine.call({ op: 'test.open', path: path.join(dir, 'Majority.tst') });
        runToEnd(engine, opened.session);
        engine.call({ op: 'close', session: opened.session });
    };
    for (let i = 0; i < 50; i++) {
        cycle();
    }
    const before = memory();
    for (let i = 0; i < 500; i++) {
        cycle();
    }
    assert.ok(memory() <= before * 1.5, `engine memory grew from ${before} to ${memory()} bytes`);
    fs.rmSync(dir, { recursive: true, force: true });
});

test('a binary field edited by hand reaches the engine as the number it shows', () => {
    const dir = scratch();
    const engine = new WasmEngine(wasm);
    const chip = engine.call({ op: 'chip.open', path: path.join(dir, 'SwapBytes.hdl') });
    assert.equal(chip.chip, 'SwapBytes');
    const pin = (state: any) => state.pins.find((p: any) => p.name === 'in');
    // The field shows the value in groups of four; the student changes the last digit.
    const shown = formatValue(0, 16, 'bin');
    assert.equal(shown, '0000 0000 0000 0000');
    const typed = typedValue(`${shown.slice(0, -1)}1`, 'bin');
    assert.equal(typed, '0b0000000000000001');
    const state = engine.call({ op: 'chip.set', session: chip.session, pin: 'in', value: typed, eval: true });
    assert.equal(state.error, undefined, JSON.stringify(state));
    assert.equal(pin(state).bits, 1);
    // The bytes are swapped on the way out.
    assert.equal(state.pins.find((p: any) => p.name === 'out').bits, 0x100);
    // A wider pattern, as typed with spaces in the middle.
    const again = engine.call({
        op: 'chip.set',
        session: chip.session,
        pin: 'in',
        value: typedValue('0000 0000 0000 0101', 'bin'),
        eval: true,
    });
    assert.equal(pin(again).bits, 5);
    fs.rmSync(dir, { recursive: true, force: true });
});

test('typed values that are not plain binary digits are left alone', () => {
    for (const text of ['5', '-1', '0x1F', '%B101', '0b11', '12 3']) {
        assert.equal(typedValue(text, 'bin'), text.trim());
    }
    // Spaces only mean something in the binary view.
    assert.equal(typedValue('0101 0101', 'dec'), '0101 0101');
    assert.equal(typedValue('  7  ', 'hex'), '7');
    assert.equal(typedValue('101', 'bin'), '0b101');
});

test('paths are compared as the file system holding them compares names', () => {
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'nandwright paths '));
    const lower = path.join(dir, 'chip.hdl');
    const upper = path.join(dir, 'CHIP.HDL');
    fs.writeFileSync(lower, '// one\n');
    // What this volume really does with a name in the other case.
    const sameFile = fs.existsSync(upper);
    assert.equal(ignoresCase(lower), sameFile);
    assert.equal(ignoresCase(upper), sameFile);
    // Keys fold case only where the platform's usual file systems do.
    const folds = process.platform === 'win32' || process.platform === 'darwin';
    assert.equal(pathKey(lower) === pathKey(upper), folds);
    assert.equal(samePath(lower, upper), folds);
    // Spelling the same file differently is always one path.
    assert.equal(samePath(lower, path.join(dir, '.', 'sub', '..', 'chip.hdl')), true);
    assert.equal(samePath(lower, path.join(dir, 'other.hdl')), false);
    if (!sameFile) {
        // Two files that differ only in case are two files.
        fs.writeFileSync(upper, '// two\n');
        assert.notEqual(pathKey(lower), pathKey(upper));
        assert.equal(fs.readFileSync(lower, 'utf8'), '// one\n');
    }
    fs.rmSync(dir, { recursive: true, force: true });
});

test('an unsaved buffer shadows only its own file unless the volume ignores case', () => {
    const dir = scratch();
    const engine = new WasmEngine(wasm);
    const broken = fs.readFileSync(path.join(dir, 'Majority.hdl'), 'utf8').replace('Or(a=abOrAc, b=bc, out=out);', 'And(a=abOrAc, b=bc, out=out);');
    const outcome = () => {
        const opened = engine.call({ op: 'test.open', path: path.join(dir, 'Majority.tst') });
        const kind = runToEnd(engine, opened.session).outcome.kind;
        engine.call({ op: 'close', session: opened.session });
        return kind;
    };
    const replace = (file: string, ignoreCase: boolean) => {
        const reply = engine.call({ op: 'overlay.replace', files: [{ path: path.join(dir, file), text: broken, ignoreCase }] });
        assert.equal(reply.error, undefined, JSON.stringify(reply));
    };
    assert.equal(outcome(), 'passed');
    // The exact path is shadowed whatever the setting.
    replace('Majority.hdl', false);
    assert.equal(outcome(), 'failed');
    // A buffer for the other-case name does not touch this file ...
    replace('MAJORITY.HDL', false);
    assert.equal(outcome(), 'passed');
    // ... unless the volume treats the two names as one file.
    replace('MAJORITY.HDL', true);
    assert.equal(outcome(), 'failed');
    // Replacing with nothing drops every buffer at once.
    assert.equal(engine.call({ op: 'overlay.replace', files: [] }).error, undefined);
    assert.equal(outcome(), 'passed');
    fs.rmSync(dir, { recursive: true, force: true });
});
