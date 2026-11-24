// A fake simulator for the chip session hub: a little engine whose replies
// a test can hold and give when it chooses, so races are reproduced in a
// fixed order. No `vscode` import: used by the unit tests and, with panels,
// by the integration tests.

import type { HubRuntime } from '../chipSessions';

type Call = Record<string, unknown>;

interface FakeSession {
    revision: number;
    time: number;
    pins: Map<string, number>;
    /** Composite paths (`a/b`) this build has, besides the root. */
    paths: Set<string>;
}

const key = (path: unknown) => (Array.isArray(path) ? path.join('/') : '');

/**
 * Sessions are numbered from 1; each build of a session has a revision. The
 * chip is clocked; its one output `out` is the clock time. Inputs are kept
 * across builds, as the engine keeps them.
 */
export class FakeRuntime implements HubRuntime {
    readonly calls: Call[] = [];
    readonly closed: number[] = [];
    readonly sessions = new Map<number, FakeSession>();
    private readonly held = new Map<string, ((reply: unknown) => void)[]>();
    private readonly holding = new Set<string>();
    private listeners: (() => void)[] = [];
    private nextSession = 1;
    /** Set: builds (open, reset) fail with this message, as an HDL error. */
    buildError: string | undefined;
    /** Set: the worker cannot start; every call stops it again. */
    broken = false;
    /** Set: overlay sync fails. */
    failSync = false;
    /** Composite paths the next build has (`'0'`, `'0/1'`, …). */
    paths = new Set<string>(['0']);
    /** Times the worker was started (the first call after a stop starts one). */
    spawns = 0;
    private running = false;

    onDidStop(listener: () => void): { dispose(): void } {
        this.listeners.push(listener);
        return { dispose: () => (this.listeners = this.listeners.filter((l) => l !== listener)) };
    }

    /** Replies to `op` wait for `answer` (or `release`). */
    hold(op: string): void {
        this.holding.add(op);
    }

    /** Lets `op` reply at once again, and gives the waiting ones their normal reply. */
    release(op: string): void {
        this.holding.delete(op);
        for (const waiting of this.held.get(op)?.splice(0) ?? []) {
            waiting(undefined);
        }
    }

    /** Gives the oldest waiting `op` its normal reply, or `reply`. */
    answer(op: string, reply?: unknown): void {
        const waiting = this.held.get(op)?.shift();
        if (!waiting) {
            throw new Error(`no ${op} is waiting`);
        }
        waiting(reply);
    }

    waiting(op: string): number {
        return this.held.get(op)?.length ?? 0;
    }

    /** The worker stops: every session is gone, and calls in flight fail. */
    stop(): void {
        this.running = false;
        this.sessions.clear();
        this.listeners.forEach((listener) => listener());
        for (const [op, list] of this.held) {
            for (const waiting of list.splice(0)) {
                waiting(STOPPED);
            }
            this.held.delete(op);
        }
    }

    ops(): string[] {
        return this.calls.map((c) => String(c.op));
    }

    count(op: string): number {
        return this.ops().filter((o) => o === op).length;
    }

    private start(): boolean {
        if (!this.running) {
            this.spawns++;
            if (this.broken) {
                // As a worker whose engine does not load: it stops at once.
                queueMicrotask(() => this.stop());
                return false;
            }
            this.running = true;
        }
        return true;
    }

    /** As the real one: a call, which throws on an error. */
    async syncUnsaved(): Promise<void> {
        if (this.failSync) {
            throw new Error('overlay sync failed');
        }
        const reply = (await this.tryCall({ op: 'overlay.replace' })) as { error?: { message: string } };
        if (reply?.error) {
            throw new Error(reply.error.message);
        }
    }

    tryCall<T>(request: Call): Promise<T> {
        this.calls.push(request);
        const op = String(request.op);
        if (!this.start()) {
            return new Promise((resolve) => queueMicrotask(() => resolve(STOPPED as T)));
        }
        if (this.holding.has(op)) {
            return new Promise((resolve) => {
                const list = this.held.get(op) ?? [];
                // Answered later: the reply is computed then, from the engine as it is then.
                list.push((reply) => resolve((reply === undefined ? this.reply(request) : reply) as T));
                this.held.set(op, list);
            });
        }
        return Promise.resolve(this.reply(request) as T);
    }

    close(session: number): void {
        this.closed.push(session);
        this.sessions.delete(session);
    }

    /** What the engine answers now. */
    reply(request: Call): unknown {
        const op = String(request.op);
        if (op === 'chip.open') {
            if (this.buildError) {
                return { error: { message: this.buildError, code: 'build-failed' } };
            }
            const session = this.nextSession++;
            this.sessions.set(session, { revision: 1, time: 0, pins: new Map(), paths: new Set(this.paths) });
            return { ...this.state(session), session };
        }
        if (op === 'overlay.replace') {
            return {};
        }
        const session = Number(request.session);
        const s = this.sessions.get(session);
        if (!s) {
            return { error: { message: 'this session has ended; start it again', code: 'session-ended' } };
        }
        if (typeof request.expectRevision === 'number' && request.expectRevision !== s.revision) {
            return { error: { message: 'stale revision', code: 'stale-revision', revision: s.revision } };
        }
        switch (op) {
            case 'chip.reset':
                if (this.buildError) {
                    return { error: { message: this.buildError, code: 'build-failed' } };
                }
                s.revision++;
                s.time = 0;
                s.paths = new Set(this.paths);
                return this.state(session);
            case 'chip.set':
                if (request.pin === 'nope') {
                    return { error: { message: 'no pin `nope`', code: 'invalid-input' } };
                }
                s.pins.set(String(request.pin), Number(request.value));
                return this.state(session);
            case 'chip.tick':
            case 'chip.tock':
            case 'chip.step':
                s.time++;
                return this.state(session);
            case 'chip.eval':
            case 'chip.state':
                return this.state(session);
            case 'chip.table':
                return { inputs: [], outputs: [], rows: [] };
            case 'chip.diagram':
            case 'chip.values': {
                const path = (request.path as number[]) ?? [];
                if (path.length > 0 && !s.paths.has(key(path))) {
                    return { error: { message: 'chip `Not` is built-in and has no parts to inspect', code: 'invalid-path', revision: s.revision } };
                }
                const values = { revision: s.revision, path, time: String(s.time), clocked: true, nets: [s.pins.get('a') ?? 0, s.time], parts: [] };
                if (op === 'chip.values') {
                    return values;
                }
                return {
                    revision: s.revision,
                    path,
                    chip: path.length ? `Part${key(path)}` : 'Fake',
                    file: null,
                    breadcrumb: [],
                    inputs: [{ name: 'a', width: 1 }],
                    outputs: [{ name: 'out', width: 16 }],
                    nets: [
                        { name: 'a', width: 1, kind: 'input' },
                        { name: 'out', width: 16, kind: 'output' },
                    ],
                    parts: [],
                    values,
                };
            }
            default:
                return { error: { message: `unknown op ${op}`, code: 'other' } };
        }
    }

    private state(session: number) {
        const s = this.sessions.get(session)!;
        return {
            chip: 'Fake',
            clocked: true,
            time: String(s.time),
            revision: s.revision,
            pins: [
                { name: 'a', role: 'input', width: 1, bits: s.pins.get('a') ?? 0 },
                { name: 'out', role: 'output', width: 16, bits: s.time },
            ],
        };
    }
}

export const STOPPED = { error: { message: 'The simulator stopped unexpectedly (exit code 1). Run it again.' } };
