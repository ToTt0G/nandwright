// One hub per chip file: the only way to the runtime session that the chip
// simulator panel and the circuit diagram panel of that file share.
//
// Every runtime call for the session, from either panel, goes through the
// hub's one queue and runs alone, in the order asked. After each one the hub
// publishes one event to every member panel, numbered in that order; the
// panels update only from these events. So what a panel shows is always in
// the engine's order, whichever panel asked: there is nothing for the panels
// to tell each other afterwards, and nothing to race.
//
// This file does not import `vscode`: the unit tests drive it with a fake
// runtime (docs/CIRCUIT_DIAGRAM.md, "Session sharing").

import type { ChipDiagram, ChipState, ChipValues, EngineError, TruthTable } from './runtime/protocol';
import { isError } from './runtime/protocol';
import { pathKey } from './paths';

/**
 * Why an op failed. The engine's own codes (`stale-revision`, `invalid-path`,
 * `build-failed`, `session-ended`, `invalid-input`, `other`), and the hub's:
 * `stopped` (the worker stopped, or its engine was replaced, while the op
 * ran), `halted` (automatic recovery gave up; the student must retry or
 * edit), `busy` (too many user actions are already queued) and `closed`
 * (every panel left).
 */
export type HubErrorCode = 'stale-revision' | 'invalid-path' | 'build-failed' | 'session-ended' | 'invalid-input' | 'other' | 'stopped' | 'halted' | 'busy' | 'closed';

export interface HubError {
    message: string;
    code: HubErrorCode;
    /** The build the engine was at (for `stale-revision`). */
    revision?: number;
}

/** The session is gone (or never came): the op says nothing about the chip itself. */
export const isTransport = (error: HubError): boolean => error.code === 'session-ended' || error.code === 'stopped' || error.code === 'closed';

export type Failure = { ok: false; error: HubError };
export type Outcome<T> = { ok: true; value: T } | Failure;

/** A build of the chip: a session, and its revision in that session. */
export interface Build {
    session: number;
    revision: number;
}

/** What a panel can ask of the hub. */
export type HubRequest =
    | { op: 'state' }
    | { op: 'table' }
    /** Build the chip again after an edit: done once per edit, however many panels ask. */
    | { op: 'reload' }
    /** The student's Reset: always builds again. */
    | { op: 'reset' }
    /** An input. `expect` is required: the build the panel rendered; if the chip was built again since, nothing is done (`stale-revision`). */
    | { op: 'set'; pin: string; value: string | number; eval: boolean; expect: Build }
    | { op: 'setAll'; values: Record<string, string | number>; expect: Build }
    | { op: 'eval' }
    /** Clock ops are intents on the session, not on a build: they run on whatever build is there when their turn comes. */
    | { op: 'tick' }
    | { op: 'tock' }
    | { op: 'step' }
    /** A diagram of `path`. Explicit drill/breadcrumb clicks carry `expect`; automatic refreshes omit it. */
    | { op: 'diagram'; path: number[]; expect?: Build }
    | { op: 'values'; path: number[] };

/** Ops whose outcome is the chip's state. `open` is the hub opening (or reopening) the chip. */
export type StateOp = 'open' | 'reload' | 'reset' | 'set' | 'setAll' | 'eval' | 'tick' | 'tock' | 'step' | 'state';

/** Ops that build the chip (again). */
export const BUILD_OPS: ReadonlySet<string> = new Set(['open', 'reload', 'reset']);
/** Ops that change the simulation. */
export const CHANGE_OPS: ReadonlySet<string> = new Set(['set', 'setAll', 'eval', 'tick', 'tock', 'step']);
export const CLOCK_OPS: ReadonlySet<string> = new Set(['tick', 'tock', 'step']);

interface EventBase {
    /** 1, 2, 3, … per hub: the order the ops ran in. */
    seq: number;
    /** `pathKey` of the chip file. */
    key: string;
    /** The member that asked; `undefined` for the hub's own (recovery, an edit made while opening). */
    origin: unknown;
    /** The request, as the member passed it (members can match their own by identity). */
    request: HubRequest | undefined;
    /** The session and revision after the op. */
    session: number | undefined;
    revision: number | undefined;
    /** Why the chip last failed to build, until a build succeeds. Never cleared by any other op. */
    buildError: string | null;
    /** Set while automatic recovery has given up: what to show next to Retry. */
    halted: string | null;
}

export type HubEvent = EventBase &
    (
        | { op: StateOp; outcome: Outcome<ChipState> }
        | { op: 'table'; outcome: Outcome<TruthTable> }
        | { op: 'diagram'; path: number[]; outcome: Outcome<ChipDiagram> }
        | { op: 'values'; path: number[]; outcome: Outcome<ChipValues> }
        /** The session is gone (the worker stopped, or its engine was replaced). Recovery follows unless `halted`. */
        | { op: 'lost'; outcome: Failure }
        /** Automatic recovery gave up. */
        | { op: 'halted'; outcome: Failure }
    );

/** A panel on a hub. Called synchronously, in `seq` order. */
export interface HubMember {
    hubEvent(event: HubEvent): void;
}

/** The parts of the runtime the hubs use (a fake one in tests). */
export interface HubRuntime {
    tryCall<T>(request: Record<string, unknown>): Promise<T | EngineError>;
    syncUnsaved(): Promise<void>;
    close(session: number): void;
    /** The worker stopped or its engine was replaced: every session is gone. */
    onDidStop(listener: () => void): { dispose(): void };
}

export interface HubOptions {
    /** Counts edits and saves of chip files: a reload after the same edit is done once. */
    generation?: () => number;
    now?: () => number;
    /** Automatic restarts allowed within `restartWindowMs`, per runtime. */
    maxRestarts?: number;
    restartWindowMs?: number;
    /** Maximum pending user actions in a hub's queue before extra ones fail with `busy`. */
    maxQueuedActions?: number;
    log?: (line: string) => void;
}

export const MAX_RESTARTS = 3;
export const RESTART_WINDOW_MS = 60_000;
export const MAX_QUEUED_ACTIONS = 16;

const HALTED_MESSAGE =
    'The simulator keeps stopping, so it is not restarted automatically any more. Click Retry, or edit the chip, to try again. The Nandwright output channel may say why.';

const BUSY_MESSAGE = 'The simulator is busy; try again in a moment.';

/** The engine's error as a typed one, also for an engine that does not send codes yet. */
export function classify(reply: EngineError): HubError {
    const error = reply.error as EngineError['error'] & { code?: string; revision?: number };
    const message = error.message;
    const known: HubErrorCode[] = ['stale-revision', 'invalid-path', 'build-failed', 'session-ended', 'invalid-input', 'other'];
    if (error.crashed) {
        return { message, code: 'stopped' };
    }
    if (typeof error.code === 'string' && (known as string[]).includes(error.code)) {
        return { message, code: error.code as HubErrorCode, ...(typeof error.revision === 'number' ? { revision: error.revision } : {}) };
    }
    if (/simulator stopped unexpectedly/i.test(message)) {
        return { message, code: 'stopped' };
    }
    if (/session has ended/.test(message)) {
        return { message, code: 'session-ended' };
    }
    if (/is built-in and has no parts|is out of range for/.test(message)) {
        return { message, code: 'invalid-path' };
    }
    return { message, code: 'other' };
}

const failure = (code: HubErrorCode, message: string): Failure => ({ ok: false, error: { code, message } });

const samePathList = (a: number[], b: number[]): boolean => a.length === b.length && a.every((v, i) => v === b[i]);

/** Whether `a` and `b` are reads of the same target, so `b` supersedes a still-queued `a`. */
function sameReadTarget(a: HubRequest, b: HubRequest): boolean {
    if (a.op !== b.op) {
        return false;
    }
    switch (a.op) {
        case 'state':
        case 'table':
            return true;
        case 'values':
            return samePathList(a.path, (b as { path: number[] }).path);
        case 'diagram': {
            const other = b as Extract<HubRequest, { op: 'diagram' }>;
            return a.expect === undefined && other.expect === undefined && samePathList(a.path, other.path);
        }
        default:
            return false;
    }
}

/** Whether `request` is a student-triggered action that counts toward the backlog bound. */
function isUserAction(request: HubRequest): boolean {
    switch (request.op) {
        case 'set':
        case 'setAll':
        case 'eval':
        case 'tick':
        case 'tock':
        case 'step':
        case 'reset':
            return true;
        case 'diagram':
            return request.expect !== undefined;
        default:
            return false;
    }
}

type JobKind =
    | { kind: 'request'; request: HubRequest }
    | { kind: 'recover' }
    | { kind: 'retry' };

interface Job {
    origin: unknown;
    work: JobKind;
    resolve(event: HubEvent | undefined): void;
    extraResolvers?: (() => void)[];
}

/**
 * The shared session of one chip file. Members join through `ChipSessions`.
 */
export class ChipHub {
    readonly key: string;
    private readonly members = new Set<HubMember>();
    private session: number | undefined;
    private revision: number | undefined;
    private buildError: string | null = null;
    private halted: string | null = null;
    /** The edit generation when `halted` was entered: only a later edit lifts it via `reload`. */
    private haltedAtGeneration: number | undefined;
    private seq = 0;
    /** The edit generation the running circuit, or the last attempt to build it, read. */
    private builtGeneration: number | undefined;
    /** The last open failed to build at this generation: do not try again until an edit. */
    private openFailedAt: number | undefined;
    /** Non-zero top-level inputs last observed on the running chip. */
    private lastInputs: Record<string, number | string> | undefined;
    /** Inputs captured when a worker crash ended the session, to restore after reopen. */
    private restoreInputs: Record<string, number | string> | undefined;
    private readonly queue: Job[] = [];
    private running = false;
    private recoveryQueued = false;
    private closed = false;
    private idleWaiters: (() => void)[] = [];

    constructor(
        readonly file: string,
        private readonly runtime: HubRuntime,
        private readonly generation: () => number,
        private readonly log: (line: string) => void,
        private readonly maxQueuedActions: number = MAX_QUEUED_ACTIONS,
    ) {
        this.key = pathKey(file);
    }

    /** What the hub knows now (for a panel that joins, and for tests). */
    status(): { session: number | undefined; revision: number | undefined; buildError: string | null; halted: string | null; members: number; pending: number } {
        return {
            session: this.session,
            revision: this.revision,
            buildError: this.buildError,
            halted: this.halted,
            members: this.members.size,
            pending: this.queue.length + (this.running ? 1 : 0),
        };
    }

    /** @internal Through `ChipSessions.join`. */
    add(member: HubMember): void {
        this.members.add(member);
    }

    /** @internal Tears down the hub: drains queued work, closes any live session, and marks closed. */
    close(): void {
        if (this.closed) {
            return;
        }
        this.closed = true;
        this.members.clear();
        this.restoreInputs = undefined;
        for (const job of this.queue.splice(0)) {
            job.resolve(undefined);
            job.extraResolvers?.forEach((resolve) => resolve());
        }
        if (this.session !== undefined) {
            this.runtime.close(this.session);
            this.session = undefined;
        }
        this.settleIdle();
    }

    /** @internal Through `ChipSessions.leave`. Whether no one is left. */
    remove(member: HubMember): boolean {
        this.members.delete(member);
        if (this.members.size > 0) {
            return false;
        }
        this.close();
        return true;
    }

    has(member: HubMember): boolean {
        return this.members.has(member);
    }

    /**
     * Queues an op. The promise resolves, after the op's event has reached
     * every member, with that event; with `undefined` if nothing was done (a
     * reload after an edit that was already built, a superseded read, or the
     * hub closed).
     */
    request(origin: unknown, request: HubRequest): Promise<HubEvent | undefined> {
        return this.enqueue(origin, { kind: 'request', request });
    }

    /**
     * The student's Retry after recovery gave up: queued behind earlier work,
     * and lifts the halt only when it reaches the queue head to try once.
     */
    retry(origin: unknown): Promise<HubEvent | undefined> {
        return this.enqueue(origin, { kind: 'retry' });
    }

    /** Resolves when nothing is queued or running. */
    whenIdle(): Promise<void> {
        if (!this.running && this.queue.length === 0) {
            return Promise.resolve();
        }
        return new Promise((resolve) => this.idleWaiters.push(resolve));
    }

    private enqueue(origin: unknown, work: JobKind): Promise<HubEvent | undefined> {
        if (this.closed) {
            return Promise.resolve(undefined);
        }
        if (work.kind === 'request') {
            const req = work.request;
            // Coalesce duplicate pending reloads: one reload in the queue serves all callers.
            if (req.op === 'reload') {
                const existing = this.queue.find((job) => job.work.kind === 'request' && job.work.request.op === 'reload');
                if (existing) {
                    return new Promise((resolve) => {
                        (existing.extraResolvers ??= []).push(() => resolve(undefined));
                    });
                }
            }
            // Coalesce superseded reads per origin+path: drop the older queued read so only the latest runs.
            if (req.op === 'state' || req.op === 'table' || req.op === 'values' || (req.op === 'diagram' && req.expect === undefined)) {
                for (let i = this.queue.length - 1; i >= 0; i--) {
                    const job = this.queue[i];
                    if (job.origin === origin && job.work.kind === 'request' && sameReadTarget(job.work.request, req)) {
                        this.queue.splice(i, 1);
                        job.resolve(undefined);
                        job.extraResolvers?.forEach((resolve) => resolve());
                    }
                }
            }
            // Bound user-action backlog while preserving accepted FIFO order.
            if (origin !== undefined && isUserAction(req)) {
                let queuedActions = 0;
                for (const job of this.queue) {
                    if (job.origin !== undefined && job.work.kind === 'request' && isUserAction(job.work.request)) {
                        queuedActions++;
                    }
                }
                if (queuedActions >= this.maxQueuedActions) {
                    return Promise.resolve(this.publishFailure(origin, req, { code: 'busy', message: BUSY_MESSAGE }));
                }
            }
        } else if (work.kind === 'retry') {
            const existing = this.queue.find((job) => job.work.kind === 'retry');
            if (existing) {
                return new Promise((resolve) => {
                    (existing.extraResolvers ??= []).push(() => resolve(undefined));
                });
            }
        }
        return new Promise((resolve) => {
            this.queue.push({ origin, work, resolve });
            void this.pump();
        });
    }

    private async pump(): Promise<void> {
        if (this.running) {
            return;
        }
        this.running = true;
        try {
            while (this.queue.length > 0 && !this.closed) {
                const job = this.queue.shift()!;
                let event: HubEvent | undefined;
                try {
                    event = await this.execute(job.origin, job.work);
                } catch (error) {
                    this.log(`Nandwright: chip session op failed: ${error instanceof Error ? error.stack ?? error.message : String(error)}`);
                    const req = job.work.kind === 'request' ? job.work.request : undefined;
                    event = req ? this.publishFailure(job.origin, req, { code: 'other', message: error instanceof Error ? error.message : String(error) }) : undefined;
                }
                // Nobody is left to hear it.
                job.resolve(this.closed ? undefined : event);
                job.extraResolvers?.forEach((resolve) => resolve());
            }
        } finally {
            this.running = false;
            this.settleIdle();
        }
    }

    private settleIdle(): void {
        if (this.running || (this.queue.length > 0 && !this.closed)) {
            return;
        }
        const waiters = this.idleWaiters;
        this.idleWaiters = [];
        waiters.forEach((resolve) => resolve());
    }

    private publish(event: Omit<EventBase, 'seq' | 'key' | 'session' | 'revision' | 'buildError' | 'halted'> & Record<string, unknown>): HubEvent {
        const full = {
            ...event,
            seq: ++this.seq,
            key: this.key,
            session: this.session,
            revision: this.revision,
            buildError: this.buildError,
            halted: this.halted,
        } as unknown as HubEvent;
        if (this.closed) {
            return full;
        }
        for (const member of [...this.members]) {
            try {
                member.hubEvent(full);
            } catch (error) {
                this.log(`Nandwright: a panel failed to take a chip session event: ${error instanceof Error ? error.stack ?? error.message : String(error)}`);
            }
        }
        return full;
    }

    private publishFailure(origin: unknown, request: HubRequest, error: HubError): HubEvent {
        const path = request.op === 'diagram' || request.op === 'values' ? { path: request.path } : {};
        return this.publish({ op: request.op, origin, request, ...path, outcome: { ok: false, error } });
    }

    /** A reply about `session`: an error that means the session is gone drops it (the next op opens the chip again). */
    private check<T>(session: number, reply: T | EngineError): Outcome<T> {
        if (!isError(reply)) {
            const revision = (reply as { revision?: unknown }).revision;
            if (typeof revision === 'number' && this.session === session) {
                this.revision = revision;
            }
            return { ok: true, value: reply };
        }
        const error = classify(reply);
        if (isTransport(error) && this.session === session) {
            this.session = undefined;
        }
        return { ok: false, error };
    }

    private rememberInputs(state: ChipState): void {
        const inputs: Record<string, number | string> = {};
        for (const pin of state.pins) {
            if (pin.role === 'input' && pin.bits !== 0 && pin.bits !== '0') {
                inputs[pin.name] = pin.bits;
            }
        }
        this.lastInputs = Object.keys(inputs).length > 0 ? inputs : undefined;
    }

    private async execute(origin: unknown, work: JobKind): Promise<HubEvent | undefined> {
        if (work.kind === 'recover') {
            this.recoveryQueued = false;
            // Re-check halt and build failure at execution time: an earlier queued
            // op may have already exhausted the restart budget or reopened the chip.
            if (this.session !== undefined || this.blocked() !== undefined) {
                return undefined;
            }
            return this.open(origin, undefined);
        }
        if (work.kind === 'retry') {
            // Only the Retry job lifts the halt, and only when it reaches the queue head.
            this.halted = null;
            this.haltedAtGeneration = undefined;
            this.openFailedAt = undefined;
            return this.session === undefined ? this.open(origin, undefined) : undefined;
        }
        const request = work.request;
        if (request.op === 'reload' || request.op === 'reset') {
            return this.rebuild(origin, request);
        }
        if (this.session === undefined) {
            const blocked = this.blocked();
            if (blocked) {
                return this.publishFailure(origin, request, blocked.error);
            }
            const opened = await this.open(origin, undefined);
            if (this.session === undefined) {
                const error: HubError = opened && !opened.outcome.ok ? opened.outcome.error : { code: 'session-ended', message: 'The chip is not open.' };
                return this.publishFailure(origin, request, error);
            }
        }
        const session = this.session!;
        const op = request.op;
        switch (op) {
            case 'state':
            case 'eval':
            case 'tick':
            case 'tock':
            case 'step': {
                const outcome = this.check(session, await this.runtime.tryCall<ChipState>({ op: `chip.${op}`, session }));
                if (outcome.ok) {
                    this.rememberInputs(outcome.value);
                }
                return this.publish({ op, origin, request, outcome });
            }
            case 'table': {
                const outcome = this.check(session, await this.runtime.tryCall<TruthTable>({ op: 'chip.table', session }));
                return this.publish({ op, origin, request, outcome });
            }
            case 'set': {
                const stale = this.stale(request.expect);
                if (stale) {
                    return this.publishFailure(origin, request, stale);
                }
                const call: Record<string, unknown> = {
                    op: 'chip.set',
                    session,
                    pin: request.pin,
                    value: request.value,
                    eval: request.eval,
                    expectRevision: request.expect.revision,
                };
                const outcome = this.check(session, await this.runtime.tryCall<ChipState>(call));
                if (outcome.ok) {
                    this.rememberInputs(outcome.value);
                }
                return this.publish({ op, origin, request, outcome });
            }
            case 'setAll': {
                const stale = this.stale(request.expect);
                if (stale) {
                    return this.publishFailure(origin, request, stale);
                }
                // A row of the truth table (or restoring inputs after a crash): a
                // pin the chip does not have is skipped; a session that is gone ends it.
                let last: Outcome<ChipState> | undefined;
                let first: Failure | undefined;
                for (const [pin, value] of Object.entries(request.values)) {
                    const call: Record<string, unknown> = {
                        op: 'chip.set',
                        session,
                        pin,
                        value,
                        eval: true,
                        expectRevision: request.expect.revision,
                    };
                    const one = this.check(session, await this.runtime.tryCall<ChipState>(call));
                    if (one.ok) {
                        last = one;
                    } else if (isTransport(one.error) || one.error.code === 'stale-revision') {
                        last = one;
                        break;
                    } else {
                        first ??= one;
                    }
                }
                let outcome: Outcome<ChipState> | undefined = last ?? first;
                outcome ??= this.check(session, await this.runtime.tryCall<ChipState>({ op: 'chip.state', session }));
                if (outcome.ok) {
                    this.rememberInputs(outcome.value);
                }
                return this.publish({ op, origin, request, outcome });
            }
            case 'diagram': {
                if (request.expect !== undefined) {
                    const stale = this.stale(request.expect);
                    if (stale) {
                        return this.publishFailure(origin, request, stale);
                    }
                }
                const call: Record<string, unknown> = { op: 'chip.diagram', session, path: request.path };
                if (request.expect !== undefined) {
                    call.expectRevision = request.expect.revision;
                }
                const outcome = this.check(session, await this.runtime.tryCall<ChipDiagram>(call));
                return this.publish({ op, origin, request, path: request.path, outcome });
            }
            case 'values': {
                const outcome = this.check(session, await this.runtime.tryCall<ChipValues>({ op: 'chip.values', session, path: request.path }));
                return this.publish({ op, origin, request, path: request.path, outcome });
            }
        }
    }

    /** Why the chip cannot be opened without the student doing something, if so. */
    private blocked(): Failure | undefined {
        if (this.halted !== null) {
            return failure('halted', this.halted);
        }
        if (this.openFailedAt !== undefined && this.openFailedAt === this.generation() && this.buildError !== null) {
            return failure('build-failed', this.buildError);
        }
        return undefined;
    }

    /** `expect` is missing or names a build that is no longer the one running. */
    private stale(expect: Build | undefined): HubError | undefined {
        if (expect && expect.session === this.session && expect.revision === this.revision) {
            return undefined;
        }
        return {
            code: 'stale-revision',
            message: 'The chip was built again since; the input was not set.',
            ...(this.revision !== undefined ? { revision: this.revision } : {}),
        };
    }

    /** Gives the engine the editor's unsaved text; a failure is returned as the engine's error would be. */
    private async sync(): Promise<EngineError | undefined> {
        try {
            await this.runtime.syncUnsaved();
            return undefined;
        } catch (error) {
            return { error: { message: error instanceof Error ? error.message : String(error) } };
        }
    }

    /** Opens the chip. Published as `open`, with `request` (a reload or Reset that found no session). */
    private async open(origin: unknown, request: HubRequest | undefined): Promise<HubEvent | undefined> {
        // The generation is read with the text sent: an edit made while the
        // chip loads is not taken as loaded.
        const generation = this.generation();
        const unsynced = await this.sync();
        if (this.closed) {
            return undefined;
        }
        if (unsynced) {
            // Not the chip's fault: the next op tries again.
            return this.publish({ op: 'open', origin, request, outcome: { ok: false, error: classify(unsynced) } });
        }
        const reply = await this.runtime.tryCall<ChipState>({ op: 'chip.open', path: this.file });
        if (this.closed) {
            if (!isError(reply) && reply.session !== undefined) {
                this.runtime.close(reply.session);
            }
            return undefined;
        }
        if (isError(reply) || reply.session === undefined) {
            const error = isError(reply) ? classify(reply) : { code: 'other' as const, message: 'The simulator did not open the chip.' };
            if (!isTransport(error)) {
                // The chip does not build: no session until an edit or Reset.
                this.buildError = error.message;
                this.openFailedAt = generation;
                this.builtGeneration = generation;
                error.code = 'build-failed';
            }
            return this.publish({ op: 'open', origin, request, outcome: { ok: false, error } });
        }
        const saved = this.restoreInputs;
        this.restoreInputs = undefined;
        this.session = reply.session;
        this.revision = typeof reply.revision === 'number' ? reply.revision : 1;
        this.buildError = null;
        this.openFailedAt = undefined;
        this.builtGeneration = generation;
        this.rememberInputs(reply);
        const event = this.publish({ op: 'open', origin, request, outcome: { ok: true, value: reply } });
        if (saved) {
            // Restore the last known top-level inputs through the hub, guarded by the new build.
            void this.enqueue(undefined, {
                kind: 'request',
                request: { op: 'setAll', values: saved, expect: { session: this.session, revision: this.revision } },
            });
        }
        if (generation !== this.generation()) {
            // Edited while it opened: build it again from the new text.
            void this.enqueue(undefined, { kind: 'request', request: { op: 'reload' } });
        }
        return event;
    }

    /** Builds the chip again from the editor's text, keeping its inputs. */
    private async rebuild(origin: unknown, request: HubRequest & { op: 'reload' | 'reset' }): Promise<HubEvent | undefined> {
        const force = request.op === 'reset';
        if (this.session === undefined) {
            const generation = this.generation();
            if (!force) {
                if (this.halted !== null && this.haltedAtGeneration === generation) {
                    return undefined;
                }
                if (this.halted === null && this.openFailedAt !== undefined && this.openFailedAt === generation) {
                    return undefined;
                }
            }
            // The student's intent (a new edit, or Reset): one attempt even after recovery gave up.
            this.halted = null;
            this.haltedAtGeneration = undefined;
            this.openFailedAt = undefined;
            return this.open(origin, request);
        }
        const generation = this.generation();
        if (!force && this.builtGeneration === generation) {
            return undefined;
        }
        const session = this.session;
        const unsynced = await this.sync();
        if (this.closed) {
            return undefined;
        }
        if (unsynced) {
            return this.publish({ op: request.op, origin, request, outcome: { ok: false, error: classify(unsynced) } });
        }
        const reply = await this.runtime.tryCall<ChipState>({ op: 'chip.reset', session });
        let outcome = this.check(session, reply);
        if (!outcome.ok && !isTransport(outcome.error)) {
            // The old circuit, and its revision, run on.
            this.buildError = outcome.error.message;
            this.builtGeneration = generation;
            outcome = { ok: false, error: { ...outcome.error, code: 'build-failed' } };
        } else if (outcome.ok) {
            this.buildError = null;
            this.builtGeneration = generation;
            this.rememberInputs(outcome.value);
        }
        return this.publish({ op: request.op, origin, request, outcome });
    }

    /**
     * @internal From `ChipSessions`: every session is gone. With `recover`
     * the chip is opened again (if anyone is here); otherwise recovery has
     * given up until the student retries or edits.
     */
    invalidate(recover: boolean, reason: string): void {
        if (this.closed) {
            return;
        }
        const had = this.session !== undefined || this.openFailedAt === undefined;
        if (this.lastInputs && !this.restoreInputs) {
            this.restoreInputs = { ...this.lastInputs };
        }
        this.session = undefined;
        this.publish({ op: 'lost', origin: undefined, request: undefined, outcome: failure('stopped', reason) });
        if (!recover) {
            this.halted = HALTED_MESSAGE;
            this.haltedAtGeneration = this.generation();
            this.publish({ op: 'halted', origin: undefined, request: undefined, outcome: failure('halted', this.halted) });
            return;
        }
        if (had && this.halted === null && !this.recoveryQueued && this.members.size > 0) {
            this.recoveryQueued = true;
            void this.enqueue(undefined, { kind: 'recover' });
        }
    }
}

/**
 * The hubs, one per chip file, and the recovery budget of the runtime they
 * share. The first panel to join a file creates its hub, the last to leave
 * closes it (and its session).
 */
export class ChipSessions {
    private readonly hubs = new Map<string, ChipHub>();
    private readonly subscription: { dispose(): void };
    /** When the runtime stopped, within the window. */
    private stops: number[] = [];
    private readonly generation: () => number;
    private readonly now: () => number;
    private readonly maxRestarts: number;
    private readonly windowMs: number;
    private readonly maxQueuedActions: number;
    private readonly log: (line: string) => void;

    constructor(
        private readonly runtime: HubRuntime,
        options: HubOptions = {},
    ) {
        this.generation = options.generation ?? (() => 0);
        this.now = options.now ?? Date.now;
        this.maxRestarts = options.maxRestarts ?? MAX_RESTARTS;
        this.windowMs = options.restartWindowMs ?? RESTART_WINDOW_MS;
        this.maxQueuedActions = options.maxQueuedActions ?? MAX_QUEUED_ACTIONS;
        this.log = options.log ?? (() => undefined);
        this.subscription = runtime.onDidStop(() => this.stopped());
    }

    /**
     * Joins the hub of a chip file, creating it if no panel has. Synchronous:
     * the member gets every event from now on. A new hub opens the chip with
     * the first op that needs it.
     */
    join(file: string, member: HubMember): ChipHub {
        const key = pathKey(file);
        let hub = this.hubs.get(key);
        if (!hub) {
            hub = new ChipHub(file, this.runtime, this.generation, this.log, this.maxQueuedActions);
            this.hubs.set(key, hub);
        }
        hub.add(member);
        return hub;
    }

    /** Leaves the hub of a chip file; the last member to leave closes its session. */
    leave(file: string, member: HubMember): void {
        const key = pathKey(file);
        const hub = this.hubs.get(key);
        if (hub?.has(member) && hub.remove(member)) {
            this.hubs.delete(key);
        }
    }

    /** The hub of a chip file, if a panel is on it. */
    hub(file: string): ChipHub | undefined {
        return this.hubs.get(pathKey(file));
    }

    /** The student's Retry: every hub that gave up tries once more. */
    retry(origin: unknown): Promise<unknown> {
        return Promise.all([...this.hubs.values()].filter((hub) => hub.status().halted !== null).map((hub) => hub.retry(origin)));
    }

    /**
     * The worker stopped, or replaced its engine: every session is gone.
     * Bounded: at most `maxRestarts` automatic recoveries within the window;
     * after that the panels wait for the student.
     */
    private stopped(): void {
        const now = this.now();
        this.stops = this.stops.filter((at) => now - at < this.windowMs);
        const recover = this.stops.length < this.maxRestarts;
        if (recover) {
            this.stops.push(now);
        } else {
            this.log('Nandwright: the simulator stopped again; it will not restart until you retry or edit the chip.');
        }
        for (const hub of [...this.hubs.values()]) {
            hub.invalidate(recover, 'The simulator stopped; its sessions are gone.');
        }
    }

    dispose(): void {
        this.subscription.dispose();
        for (const hub of this.hubs.values()) {
            hub.close();
        }
        this.hubs.clear();
    }
}
