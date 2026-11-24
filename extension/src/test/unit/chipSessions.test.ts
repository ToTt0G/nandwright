// The shared chip session hub against a fake simulator whose replies the
// tests hold and give in a chosen order (docs/CIRCUIT_DIAGRAM.md, "Session
// sharing"). Each review finding the hub closes has a test named after it.

import assert from 'node:assert/strict';
import { test } from 'node:test';
import { ChipSessions, HubEvent, HubMember, MAX_RESTARTS, classify } from '../../chipSessions';
import { FakeRuntime } from '../fakeRuntime';
import { sourceHash, sourceHashReference } from '../../sourceHash';
import { endsSessions } from '../../runtime/protocol';

const FILE = '/work/chips/Fake.hdl';

class Member implements HubMember {
    readonly events: HubEvent[] = [];
    constructor(readonly name: string) {}
    hubEvent(event: HubEvent): void {
        this.events.push(event);
    }
    ops(): string[] {
        return this.events.map((e) => `${e.op}${e.outcome.ok ? '' : `!${e.outcome.error.code}`}`);
    }
}

const flush = () => new Promise((resolve) => setImmediate(resolve));

function setup(options: { now?: () => number; generation?: () => number } = {}) {
    const fake = new FakeRuntime();
    const sessions = new ChipSessions(fake, options);
    const a = new Member('a');
    const b = new Member('b');
    const hub = sessions.join(FILE, a);
    sessions.join(FILE, b);
    return { fake, sessions, hub, a, b };
}

test('every op of either member runs alone, in the order asked, and both see every event in that order', async () => {
    const { fake, hub, a, b } = setup();
    await hub.request(a, { op: 'state' });
    fake.hold('chip.step');
    const steps = [hub.request(a, { op: 'step' }), hub.request(b, { op: 'step' }), hub.request(a, { op: 'step' })];
    await flush();
    // One at a time: the second step is not sent before the first replies.
    assert.equal(fake.waiting('chip.step'), 1);
    fake.answer('chip.step');
    await flush();
    assert.equal(fake.waiting('chip.step'), 1);
    fake.release('chip.step');
    await Promise.all(steps);
    assert.deepEqual(
        a.events.map((e) => e.seq),
        b.events.map((e) => e.seq),
    );
    const times = a.events.filter((e) => e.op === 'step').map((e) => (e.outcome.ok ? (e.outcome.value as { time: string }).time : '?'));
    assert.deepEqual(times, ['1', '2', '3']);
    assert.deepEqual(
        a.events.filter((e) => e.op === 'step').map((e) => e.origin),
        [a, b, a],
    );
    for (let i = 1; i < a.events.length; i++) {
        assert.equal(a.events[i].seq, a.events[i - 1].seq + 1);
    }
});

test('R1: an input meant for an older build is not set on the new one (stale-revision)', async () => {
    const { fake, hub, a, b } = setup();
    const opened = await hub.request(a, { op: 'state' });
    const shown = { session: opened!.session!, revision: opened!.revision! };
    assert.equal(shown.revision, 1);
    // The other panel resets: build 2.
    await hub.request(b, { op: 'reset' });
    const sets = fake.count('chip.set');
    const event = await hub.request(a, { op: 'set', pin: 'a', value: 1, eval: false, expect: shown });
    assert.equal(event?.op, 'set');
    assert.equal(event!.outcome.ok, false);
    assert.equal(!event!.outcome.ok && event!.outcome.error.code, 'stale-revision');
    assert.equal(fake.count('chip.set'), sets, 'nothing was sent to the engine');
    assert.equal(fake.sessions.get(shown.session)!.pins.get('a'), undefined, 'the new build was not changed');
    // With the build shown now, it is set.
    const now = { session: event!.session!, revision: event!.revision! };
    const set = await hub.request(a, { op: 'set', pin: 'a', value: 1, eval: false, expect: now });
    assert.ok(set?.outcome.ok);
    assert.equal(fake.calls.find((c) => c.op === 'chip.set')?.expectRevision, 2, 'the engine is told the build too');
});

test('R1: an input meant for a build of a session that was replaced is stale too, even at the same revision', async () => {
    const { fake, hub, a } = setup();
    const opened = await hub.request(a, { op: 'state' });
    const shown = { session: opened!.session!, revision: opened!.revision! };
    fake.stop();
    await hub.whenIdle();
    assert.notEqual(hub.status().session, shown.session);
    assert.equal(hub.status().revision, 1);
    const event = await hub.request(a, { op: 'set', pin: 'a', value: 1, eval: false, expect: shown });
    assert.equal(!event!.outcome.ok && event!.outcome.error.code, 'stale-revision');
});

test('R3: Reset then Step run in click order: reset, then a step of the new build; neither is dropped', async () => {
    const { fake, hub, a } = setup();
    await hub.request(a, { op: 'step' });
    fake.hold('chip.reset');
    const reset = hub.request(a, { op: 'reset' });
    const step = hub.request(a, { op: 'step' });
    await flush();
    fake.release('chip.reset');
    const [r, s] = await Promise.all([reset, step]);
    assert.equal(r?.op, 'reset');
    assert.equal(s?.op, 'step');
    assert.ok(s!.seq > r!.seq);
    assert.equal(s!.revision, 2, 'the step ran on the new build');
    assert.equal(s!.outcome.ok && (s!.outcome.value as { time: string }).time, '1');
    assert.deepEqual(
        fake.ops().filter((op) => op === 'chip.reset' || op === 'chip.step'),
        ['chip.step', 'chip.reset', 'chip.step'],
    );
});

test('R4: a clock step never clears the build error; only a build that succeeds does', async () => {
    const { fake, hub, a, b } = setup();
    await hub.request(a, { op: 'state' });
    fake.buildError = 'unknown chip `Nott`';
    const reset = await hub.request(a, { op: 'reset' });
    assert.equal(!reset!.outcome.ok && reset!.outcome.error.code, 'build-failed');
    assert.equal(reset!.buildError, 'unknown chip `Nott`');
    assert.equal(reset!.revision, 1, 'the old circuit runs on');
    const step = await hub.request(b, { op: 'step' });
    assert.ok(step!.outcome.ok, 'the old circuit still steps');
    assert.equal(step!.buildError, 'unknown chip `Nott`', 'and the error stays');
    assert.equal(b.events.at(-1)!.buildError, 'unknown chip `Nott`');
    fake.buildError = undefined;
    const fixed = await hub.request(b, { op: 'reset' });
    assert.ok(fixed!.outcome.ok);
    assert.equal(fixed!.buildError, null);
    assert.equal(fixed!.revision, 2);
});

test('R4: clock states reach every member in the engine order, however the replies are delayed', async () => {
    const { fake, hub, a, b } = setup();
    await hub.request(a, { op: 'state' });
    fake.hold('chip.values');
    fake.hold('chip.step');
    // The circuit panel steps and asks for values; the simulator steps too.
    const work = [hub.request(a, { op: 'step' }), hub.request(a, { op: 'values', path: [] }), hub.request(b, { op: 'step' })];
    await flush();
    fake.answer('chip.step');
    await flush();
    // The values reply is slow; nothing behind it overtakes it.
    assert.equal(fake.waiting('chip.step'), 0);
    fake.release('chip.values');
    fake.release('chip.step');
    await Promise.all(work);
    const recorded = b.events.filter((e) => e.op === 'step').map((e) => (e.outcome.ok ? (e.outcome.value as { time: string }).time : '?'));
    assert.deepEqual(recorded, ['1', '2']);
});

test('a reload after an edit is done once for all members; Reset always builds', async () => {
    let generation = 0;
    const { fake, hub, a, b } = setup({ generation: () => generation });
    await hub.request(a, { op: 'state' });
    generation++;
    await Promise.all([hub.request(a, { op: 'reload' }), hub.request(b, { op: 'reload' })]);
    assert.equal(fake.count('chip.reset'), 1);
    assert.equal(await hub.request(a, { op: 'reload' }), undefined, 'nothing to do');
    await hub.request(b, { op: 'reset' });
    assert.equal(fake.count('chip.reset'), 2);
});

test('an edit made while the chip opens is built at once', async () => {
    let generation = 0;
    const { fake, hub, a } = setup({ generation: () => generation });
    fake.hold('chip.open');
    const opening = hub.request(a, { op: 'state' });
    await flush();
    generation++;
    fake.release('chip.open');
    await opening;
    await hub.whenIdle();
    assert.equal(fake.count('chip.reset'), 1, 'built again from the edited text');
    assert.equal(hub.status().revision, 2);
});

test('a chip that does not build is not opened again until an edit or Reset', async () => {
    let generation = 0;
    const { fake, hub, a } = setup({ generation: () => generation });
    fake.buildError = 'unknown chip `Andd`';
    const first = await hub.request(a, { op: 'diagram', path: [] });
    assert.equal(!first!.outcome.ok && first!.outcome.error.code, 'build-failed');
    assert.equal(first!.buildError, 'unknown chip `Andd`');
    await hub.request(a, { op: 'values', path: [] });
    assert.equal(fake.count('chip.open'), 1);
    fake.buildError = undefined;
    generation++;
    const reload = await hub.request(a, { op: 'reload' });
    assert.equal(reload?.op, 'open');
    assert.ok(reload!.outcome.ok);
    assert.equal(reload!.buildError, null);
});

test('a failed overlay sync is not taken for a broken chip: the next op tries again', async () => {
    const { fake, hub, a } = setup();
    fake.failSync = true;
    const failed = await hub.request(a, { op: 'state' });
    assert.equal(!failed!.outcome.ok && failed!.outcome.error.code, 'other');
    assert.equal(failed!.buildError, null);
    fake.failSync = false;
    const state = await hub.request(a, { op: 'state' });
    assert.ok(state!.outcome.ok);
});

test('the last member to leave closes the session; ops still queued do nothing', async () => {
    const { fake, sessions, hub, a, b } = setup();
    await hub.request(a, { op: 'state' });
    sessions.leave(FILE, a);
    assert.deepEqual(fake.closed, []);
    fake.hold('chip.step');
    const step = hub.request(b, { op: 'step' });
    const queued = hub.request(b, { op: 'step' });
    await flush();
    sessions.leave(FILE, b);
    assert.deepEqual(fake.closed, [1]);
    assert.equal(await queued, undefined);
    fake.release('chip.step');
    await step;
    assert.equal(sessions.hub(FILE), undefined);
    // A new member gets a new hub and a new session.
    const c = new Member('c');
    const fresh = sessions.join(FILE, c);
    assert.notEqual(fresh, hub);
    await fresh.request(c, { op: 'state' });
    assert.equal(fresh.status().session, 2);
});

test('a member that leaves while the chip opens: the session opened for nobody is closed', async () => {
    const { fake, sessions, hub, a, b } = setup();
    fake.hold('chip.open');
    const opening = hub.request(a, { op: 'state' });
    await flush();
    sessions.leave(FILE, a);
    sessions.leave(FILE, b);
    fake.release('chip.open');
    assert.equal(await opening, undefined);
    await flush();
    assert.deepEqual(fake.closed, [1]);
});

test('R6: a worker that stops during chip.diagram: the op fails as transport, the chip is opened again, the path is the asker\'s to keep', async () => {
    const { fake, hub, a } = setup();
    await hub.request(a, { op: 'diagram', path: [0] });
    fake.hold('chip.diagram');
    const drill = hub.request(a, { op: 'diagram', path: [0] });
    await flush();
    fake.stop();
    const event = await drill;
    assert.equal(event?.op, 'diagram');
    assert.equal(!event!.outcome.ok && event!.outcome.error.code, 'stopped');
    await hub.whenIdle();
    assert.deepEqual(a.ops().slice(-3), ['lost!stopped', 'diagram!stopped', 'open']);
    fake.release('chip.diagram');
    const again = await hub.request(a, { op: 'diagram', path: [0] });
    assert.ok(again!.outcome.ok, 'the same level, on the new session');
});

test('R6: a path that is not composite in the current build fails as invalid-path', async () => {
    const { fake, hub, a } = setup();
    await hub.request(a, { op: 'diagram', path: [0] });
    fake.paths = new Set();
    await hub.request(a, { op: 'reset' });
    const event = await hub.request(a, { op: 'diagram', path: [0] });
    assert.equal(!event!.outcome.ok && event!.outcome.error.code, 'invalid-path');
});

test('a session that ended without a stop is dropped, and the next op opens the chip again', async () => {
    const { fake, hub, a } = setup();
    await hub.request(a, { op: 'state' });
    fake.sessions.clear();
    const ended = await hub.request(a, { op: 'step' });
    assert.equal(!ended!.outcome.ok && ended!.outcome.error.code, 'session-ended');
    assert.equal(hub.status().session, undefined);
    const next = await hub.request(a, { op: 'step' });
    assert.ok(next!.outcome.ok);
    assert.equal(hub.status().session, 2);
});

test('R2: a worker that cannot start is restarted at most three times a minute, then the panels wait for Retry', async () => {
    let now = 0;
    const { fake, sessions, hub, a } = setup({ now: () => now });
    fake.broken = true;
    await hub.request(a, { op: 'state' });
    for (let i = 0; i < 20; i++) {
        await flush();
        await hub.whenIdle();
    }
    assert.equal(fake.spawns, 1 + MAX_RESTARTS, `spawned ${fake.spawns} times`);
    assert.ok(hub.status().halted, 'gave up');
    assert.equal(a.ops().filter((op) => op === 'halted!halted').length, 1);
    // Nothing automatic any more: an op fails at once, without a worker.
    const spawns = fake.spawns;
    const calls = fake.calls.length;
    const blocked = await hub.request(a, { op: 'diagram', path: [] });
    assert.equal(!blocked!.outcome.ok && blocked!.outcome.error.code, 'halted');
    assert.equal(fake.spawns, spawns);
    assert.equal(fake.calls.length, calls);
    // Retry is one attempt; it works once the worker can start.
    fake.broken = false;
    now += 1000;
    await sessions.retry(a);
    assert.equal(hub.status().halted, null);
    assert.ok(hub.status().session !== undefined);
    const state = await hub.request(a, { op: 'state' });
    assert.ok(state!.outcome.ok);
});

test('R2: Retry while the worker is still broken is one attempt, not another round', async () => {
    let now = 0;
    const { fake, sessions, hub, a } = setup({ now: () => now });
    fake.broken = true;
    await hub.request(a, { op: 'state' });
    for (let i = 0; i < 20; i++) {
        await flush();
        await hub.whenIdle();
    }
    const spawns = fake.spawns;
    await sessions.retry(a);
    for (let i = 0; i < 20; i++) {
        await flush();
        await hub.whenIdle();
    }
    assert.equal(fake.spawns, spawns + 1);
    assert.ok(hub.status().halted);
    // A minute later the budget is back.
    now += 61_000;
    fake.broken = false;
    fake.stop();
    await hub.whenIdle();
    assert.ok(hub.status().halted, 'still waiting for the student after a halt');
});

test('stops spread over more than a minute keep recovering', async () => {
    let now = 0;
    const { fake, hub, a } = setup({ now: () => now });
    await hub.request(a, { op: 'state' });
    for (let i = 0; i < 6; i++) {
        now += 30_000;
        fake.stop();
        await hub.whenIdle();
        assert.equal(hub.status().halted, null, `stop ${i + 1}`);
        assert.ok(hub.status().session !== undefined);
    }
});

test('an engine replaced in the worker (`crashed`) invalidates every hub through the same path as a worker exit', async () => {
    const fake = new FakeRuntime();
    const sessions = new ChipSessions(fake);
    const a = new Member('a');
    const c = new Member('c');
    const one = sessions.join(FILE, a);
    const other = sessions.join('/work/chips/Other.hdl', c);
    await one.request(a, { op: 'state' });
    await other.request(c, { op: 'state' });
    // What the client does with a `crashed` reply: fire onDidStop, then deliver the reply.
    fake.hold('chip.step');
    const step = one.request(a, { op: 'step' });
    await flush();
    const crashed = { error: { message: 'The simulator hit an internal error and was restarted (unreachable).', crashed: true } };
    assert.ok(endsSessions(crashed));
    fake.stop();
    const event = await step;
    assert.equal(!event!.outcome.ok && event!.outcome.error.code, 'stopped');
    assert.equal(classify(crashed).code, 'stopped');
    await one.whenIdle();
    await other.whenIdle();
    assert.ok(c.ops().includes('lost!stopped'), 'the other chip lost its session too');
    assert.equal(c.ops().at(-1), 'open', 'and was opened again');
    assert.equal(a.ops().at(-1), 'open');
});

test('a crashed reply that comes without a stop still drops the session', async () => {
    const { fake, hub, a } = setup();
    await hub.request(a, { op: 'state' });
    fake.hold('chip.step');
    const step = hub.request(a, { op: 'step' });
    await flush();
    fake.answer('chip.step', { error: { message: 'internal error', crashed: true } });
    await step;
    assert.equal(hub.status().session, undefined);
    const next = await hub.request(a, { op: 'state' });
    assert.ok(next!.outcome.ok);
});

test('engine errors without a code are classified by their message', () => {
    assert.equal(classify({ error: { message: 'this session has ended; start it again' } }).code, 'session-ended');
    assert.equal(classify({ error: { message: 'The simulator stopped unexpectedly (exit code 1). Run it again.' } }).code, 'stopped');
    assert.equal(classify({ error: { message: 'chip `Not` is built-in and has no parts to inspect' } }).code, 'invalid-path');
    assert.equal(classify({ error: { message: 'part index 9 is out of range for `X` (which has 2 parts)' } }).code, 'invalid-path');
    assert.equal(classify({ error: { message: 'x', code: 'stale-revision', revision: 4 } as never }).revision, 4);
    assert.equal(classify({ error: { message: 'x', code: 'made-up' } as never }).code, 'other');
});

test('R5: sourceHash is 64-bit FNV-1a of the UTF-8 text', () => {
    // The vector in docs/CIRCUIT_DIAGRAM.md, and FNV-1a's own.
    assert.equal(sourceHash('CHIP X {}\n'), 'f3235dc7995af525');
    assert.equal(sourceHash(''), 'cbf29ce484222325');
    assert.equal(sourceHash('a'), 'af63dc4c8601ec8c');
    assert.equal(sourceHash('foobar'), '85944171f73967e8');
    // UTF-8, not UTF-16: é is two bytes, 😀 four.
    assert.equal(sourceHash('é😀'), sourceHashReference('é😀'));
    let seed = 7;
    for (let i = 0; i < 200; i++) {
        seed = (seed * 1103515245 + 12345) & 0x7fffffff;
        const text = Array.from({ length: seed % 300 }, (_, j) => String.fromCharCode(((seed >> (j % 16)) + j * 31) % 0x2fff)).join('');
        assert.equal(sourceHash(text), sourceHashReference(text));
    }
});

test('F1: set and setAll require a build identity (missing expect is rejected as stale-revision)', async () => {
    const { fake, hub, a } = setup();
    await hub.request(a, { op: 'state' });
    const setEvent = await hub.request(a, { op: 'set', pin: 'a', value: 1, eval: false, expect: undefined as never });
    assert.equal(!setEvent!.outcome.ok && setEvent!.outcome.error.code, 'stale-revision');
    const setAllEvent = await hub.request(a, { op: 'setAll', values: { a: 1 }, expect: undefined as never });
    assert.equal(!setAllEvent!.outcome.ok && setAllEvent!.outcome.error.code, 'stale-revision');
    assert.equal(fake.count('chip.set'), 0);
});

test('F2: superseded pending reads per origin+path and duplicate reloads coalesce before enqueueing', async () => {
    let generation = 0;
    const { fake, hub, a, b } = setup({ generation: () => generation });
    await hub.request(a, { op: 'state' });
    fake.hold('chip.step');
    const running = hub.request(a, { op: 'step' });
    await flush();

    // Queue duplicate reads for `a` and distinct reads for `b` / another path, plus duplicate reloads.
    generation++;
    const s1 = hub.request(a, { op: 'state' });
    const s2 = hub.request(a, { op: 'state' });
    const sb = hub.request(b, { op: 'state' });
    const v1 = hub.request(a, { op: 'values', path: [0] });
    const v2 = hub.request(a, { op: 'values', path: [0] });
    const vRoot = hub.request(a, { op: 'values', path: [] });
    const d1 = hub.request(a, { op: 'diagram', path: [0] });
    const d2 = hub.request(a, { op: 'diagram', path: [0] });
    const r1 = hub.request(a, { op: 'reload' });
    const r2 = hub.request(b, { op: 'reload' });
    const r3 = hub.request(a, { op: 'reload' });

    // Only 1 running + 6 queued (s2, sb, v2, vRoot, d2, r1) — not 12!
    assert.equal(hub.status().pending, 1 + 6);
    fake.release('chip.step');
    const results = await Promise.all([running, s1, s2, sb, v1, v2, vRoot, d1, d2, r1, r2, r3]);
    // Superseded reads and duplicate reloads resolve cleanly with undefined.
    assert.equal(results[1], undefined, 's1 superseded');
    assert.ok(results[2]?.outcome.ok, 's2 ran');
    assert.ok(results[3]?.outcome.ok, 'sb ran');
    assert.equal(results[4], undefined, 'v1 superseded');
    assert.ok(results[5]?.outcome.ok, 'v2 ran');
    assert.ok(results[6]?.outcome.ok, 'vRoot ran');
    assert.equal(results[7], undefined, 'd1 superseded');
    assert.ok(results[8]?.outcome.ok, 'd2 ran');
    assert.ok(results[9]?.outcome.ok, 'r1 ran');
    assert.equal(results[10], undefined, 'r2 coalesced');
    assert.equal(results[11], undefined, 'r3 coalesced');
    assert.equal(fake.count('chip.state'), 3, 'initial open state + s2 + sb');
    assert.equal(fake.count('chip.values'), 2, 'v2 + vRoot');
    assert.equal(fake.count('chip.diagram'), 1, 'd2 only');
    assert.equal(fake.count('chip.reset'), 1, 'one reload for all');
});

test('F2: user-action backlog is bounded while preserving accepted FIFO order', async () => {
    const fake = new FakeRuntime();
    const sessions = new ChipSessions(fake, { maxQueuedActions: 3 });
    const a = new Member('a');
    const hub = sessions.join(FILE, a);
    await hub.request(a, { op: 'state' });

    fake.hold('chip.step');
    const running = hub.request(a, { op: 'step' });
    await flush();
    // 3 actions fit in the queue; the 4th and 5th are rejected at once with `busy`.
    const q1 = hub.request(a, { op: 'step' });
    const q2 = hub.request(a, { op: 'step' });
    const q3 = hub.request(a, { op: 'step' });
    const overflow1 = hub.request(a, { op: 'step' });
    const overflow2 = hub.request(a, { op: 'reset' });
    assert.equal(hub.status().pending, 4, '1 running + 3 queued');
    const [e1, e2] = await Promise.all([overflow1, overflow2]);
    assert.equal(!e1!.outcome.ok && e1!.outcome.error.code, 'busy');
    assert.equal(!e2!.outcome.ok && e2!.outcome.error.code, 'busy');

    fake.release('chip.step');
    const done = await Promise.all([running, q1, q2, q3]);
    assert.deepEqual(
        done.map((e) => (e?.outcome.ok ? (e.outcome.value as { time: string }).time : '?')),
        ['1', '2', '3', '4'],
        'accepted actions ran in FIFO order',
    );
    assert.equal(fake.count('chip.step'), 4);
    assert.equal(fake.count('chip.reset'), 0);
    sessions.dispose();
});

test('F3: held Step + queued Steps + worker stop does not let an older queued recovery job exceed the restart budget', async () => {
    let now = 0;
    const { fake, hub, a } = setup({ now: () => now });
    await hub.request(a, { op: 'state' });
    assert.equal(fake.spawns, 1);

    fake.hold('chip.step');
    const held = hub.request(a, { op: 'step' });
    // Queued steps ahead of the recovery job that fake.stop() will enqueue.
    const queued = [
        hub.request(a, { op: 'step' }),
        hub.request(a, { op: 'step' }),
        hub.request(a, { op: 'step' }),
    ];
    await flush();
    fake.broken = true;
    fake.stop();
    await Promise.all([held, ...queued]);
    for (let i = 0; i < 20; i++) {
        await flush();
        await hub.whenIdle();
    }
    assert.equal(fake.spawns, 1 + MAX_RESTARTS, `must not exceed budget (${fake.spawns} spawns)`);
    assert.ok(hub.status().halted, 'hub halted once budget exhausted');
});

test('F3: Retry lifts the halt only when its job reaches the queue head, not for earlier queued ops', async () => {
    let now = 0;
    const fake = new FakeRuntime();
    const sessions = new ChipSessions(fake, { now: () => now });
    const a = new Member('a');
    const hub = sessions.join(FILE, a);
    // Exhaust budget so the hub is halted.
    fake.broken = true;
    await hub.request(a, { op: 'state' });
    for (let i = 0; i < 20; i++) {
        await flush();
        await hub.whenIdle();
    }
    assert.ok(hub.status().halted);

    // Hold a sync in flight on a reset so we can queue [step, retry] behind it while halted.
    let releaseSync!: () => void;
    const origSync = fake.syncUnsaved.bind(fake);
    let blockNextSync = true;
    fake.syncUnsaved = async () => {
        if (blockNextSync) {
            blockNextSync = false;
            await new Promise<void>((resolve) => {
                releaseSync = resolve;
            });
            throw new Error('sync aborted');
        }
        return origSync();
    };

    const inFlight = hub.request(a, { op: 'reset' });
    await flush();
    // Worker stops again (budget already exhausted) -> hub is halted while `inFlight` is still running.
    fake.stop();
    assert.ok(hub.status().halted, 'halted while in-flight op is still awaiting sync');

    // Now the worker is healthy again; queue a Step, then Retry behind it.
    fake.broken = false;
    const earlierStep = hub.request(a, { op: 'step' });
    const retryPromise = hub.retry(a);
    // Before Retry runs, the hub must still report halted.
    assert.ok(hub.status().halted, 'Retry has not reached the queue head yet');

    releaseSync();
    const [, stepEvent, retryEvent] = await Promise.all([inFlight, earlierStep, retryPromise]);
    assert.equal(!stepEvent!.outcome.ok && stepEvent!.outcome.error.code, 'halted', 'earlier Step failed as halted');
    assert.equal(retryEvent?.op, 'open');
    assert.ok(retryEvent?.outcome.ok, 'Retry itself opened the chip');
    assert.equal(hub.status().halted, null);
    assert.equal(fake.count('chip.step'), 0, 'the earlier Step never reached the engine');
    sessions.dispose();
});

test('F4: explicit drill diagram carries expectRevision and fails as stale-revision after a rebuild', async () => {
    const { fake, hub, a, b } = setup();
    const opened = await hub.request(a, { op: 'state' });
    const build1 = { session: opened!.session!, revision: opened!.revision! };
    await hub.request(b, { op: 'reset' });
    const staleDrill = await hub.request(a, { op: 'diagram', path: [0], expect: build1 });
    assert.equal(!staleDrill!.outcome.ok && staleDrill!.outcome.error.code, 'stale-revision');
    assert.equal(fake.count('chip.diagram'), 0, 'old drill never reached the new revision');
    // Current build succeeds and passes expectRevision to the engine.
    const build2 = { session: staleDrill!.session!, revision: staleDrill!.revision! };
    const freshDrill = await hub.request(a, { op: 'diagram', path: [0], expect: build2 });
    assert.ok(freshDrill!.outcome.ok);
    assert.equal(fake.calls.find((c) => c.op === 'chip.diagram')?.expectRevision, 2);
});

test('F6: disposing the registry drains queued requests and closes late chip.open sessions', async () => {
    const { fake, sessions, hub, a } = setup();
    fake.hold('chip.open');
    const opening = hub.request(a, { op: 'state' });
    const queued = hub.request(a, { op: 'step' });
    await flush();
    assert.equal(fake.waiting('chip.open'), 1);

    sessions.dispose();
    fake.release('chip.open');
    assert.equal(await opening, undefined);
    assert.equal(await queued, undefined);
    await flush();
    assert.deepEqual(fake.closed, [1], 'late session closed');
    assert.equal(fake.count('chip.step'), 0, 'queued request did not run');
    assert.equal(a.events.length, 0, 'no events published after dispose');
});

test('worker crash restores last known non-zero top-level inputs on the reopened session via guarded setAll', async () => {
    const { fake, hub, a } = setup();
    const opened = await hub.request(a, { op: 'state' });
    await hub.request(a, { op: 'set', pin: 'a', value: 1, eval: false, expect: { session: opened!.session!, revision: opened!.revision! } });
    assert.equal(fake.sessions.get(1)!.pins.get('a'), 1);

    fake.stop();
    await hub.whenIdle();
    const newSession = hub.status().session!;
    assert.notEqual(newSession, 1);
    assert.equal(fake.sessions.get(newSession)!.pins.get('a'), 1, 'input restored on reopened session');
    const restoreCall = fake.calls.filter((c) => c.op === 'chip.set').at(-1);
    assert.equal(restoreCall?.session, newSession);
    assert.equal(restoreCall?.expectRevision, 1);
});
