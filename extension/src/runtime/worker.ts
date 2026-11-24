// The runtime worker thread: test runs and simulators.
//
// It runs in a worker so that a chip with thousands of gates, or a program
// in a tight loop, never freezes the editor. Long work is done in slices; the
// worker returns to its event loop between slices, which is when a `cancel`
// message gets through. No single engine request is unbounded.

import * as path from 'node:path';
import { parentPort, workerData } from 'node:worker_threads';
import { EngineCrash, WasmEngine } from '../engine/wasm';
import { FromWorker, MAX_REPORTED_FAILURES, MachineRunRequest, TestRunRequest, ToWorker, WorkerStart, firstSession } from './protocol';

const wasmPath = path.join(__dirname, 'engine.wasm');
const port = parentPort!;

// Each engine numbers its sessions from its own range, disjoint from those
// of the other engines of this worker and of every other worker (see
// `firstSession`).
const start: WorkerStart = { worker: 0, ...(workerData as Partial<WorkerStart> | undefined) };
let engines = 0;
let engine = newEngine();

/** Runs in progress, and those of them that have been asked to stop. */
const active = new Set<number>();
const cancelled = new Set<number>();

function newEngine(): WasmEngine {
    const fresh = new WasmEngine(wasmPath, (line) => post({ type: 'log', line }));
    const first = firstSession(start.worker, engines);
    if (first !== 1) {
        fresh.call({ op: 'start', firstSession: first });
    }
    engines += 1;
    return fresh;
}

function post(message: FromWorker): void {
    port.postMessage(message);
}

/** Runs a request, restarting the engine if it traps. */
function call(request: object): any {
    try {
        return engine.call(request);
    } catch (error) {
        if (!(error instanceof EngineCrash)) {
            throw error;
        }
        engine = newEngine();
        return {
            error: {
                message: `The simulator hit an internal error and was restarted (${error.message}). Run it again; if it happens again, please report it.`,
                crashed: true,
            },
        };
    }
}

const yieldToEvents = () => new Promise<void>((resolve) => setImmediate(resolve));

/** How long one slice of a run may take before the worker checks for cancel. */
const SLICE_MS = 25;

/**
 * Runs a test session until it finishes or pauses for a reason other than
 * the slice budget, sizing slices so each takes about `SLICE_MS`. With
 * `step` it runs to the end of the current `;` step instead, which can
 * equally be a loop that never ends.
 *
 * Events are posted as they happen and not kept: a script that echoes in an
 * endless loop must not grow this thread's memory until it is stopped.
 */
async function runTest(id: number, request: TestRunRequest): Promise<any> {
    const op = request.step ? 'test.step' : 'test.run';
    let budget = 2000;
    const failures: unknown[] = [];
    let last: any = { commands: 0, echo: null };
    for (;;) {
        if (cancelled.has(id)) {
            return { state: 'finished', outcome: { kind: 'cancelled' }, events: [], failures, commands: last.commands, echo: last.echo };
        }
        const started = performance.now();
        const status = call({ op, session: request.session, breakLines: request.breakLines, budget });
        if (status.error) {
            return status;
        }
        last = status;
        if (status.events?.length) {
            post({ type: 'progress', id, events: status.events });
        }
        for (const failure of status.failures ?? []) {
            if (failures.length < MAX_REPORTED_FAILURES) {
                failures.push(failure);
            }
        }
        if (status.state !== 'suspended' || status.reason !== 'budget') {
            return { ...status, events: [], failures };
        }
        const elapsed = Math.max(performance.now() - started, 0.05);
        budget = Math.round(Math.min(Math.max((budget * SLICE_MS) / elapsed, 200), 20_000_000));
        await yieldToEvents();
    }
}

const sleep = (ms: number) => new Promise<void>((resolve) => setTimeout(resolve, ms));

/** The panel is redrawn at most this often while a program runs. */
const FRAME_MS = 33;

/**
 * At a chosen speed, the most time one slice makes up for. After the laptop
 * slept, or the thread was held up, the program does not race through the
 * time it missed (and Pause does not wait for it to).
 */
const MAX_CATCH_UP_MS = 100;

/**
 * Runs a program until it is paused (cancelled), reaches a breakpoint or
 * fails, posting a frame for the panel about thirty times a second.
 */
async function runMachine(id: number, request: MachineRunRequest): Promise<any> {
    const base = {
        session: request.session,
        breakLines: request.breakLines,
        breakpoints: request.breakpoints,
        ram: request.ram,
    };
    // A program that is waiting for a key or sleeping is still running.
    const going = new Set(['running', 'waiting', 'sleeping']);
    let budget = 50_000;
    let owed = 0;
    let last = performance.now();
    let lastFrame = 0;
    // The instruction the program is paused on is executed even if it has a
    // breakpoint: only the first slice says so.
    let resume = true;
    for (;;) {
        if (cancelled.has(id)) {
            return call({ ...base, op: `${request.kind}.state`, screen: true });
        }
        const now = performance.now();
        if (request.speed > 0) {
            // Paced: run the instructions that are due since the last slice.
            owed += (request.speed * Math.min(now - last, MAX_CATCH_UP_MS)) / 1000;
            budget = Math.floor(owed);
            owed -= budget;
        }
        last = now;
        const frameDue = now - lastFrame >= FRAME_MS;
        if (budget > 0) {
            const started = performance.now();
            const state = call({
                ...base,
                op: `${request.kind}.run`,
                budget,
                resume,
                screen: frameDue,
                now: Math.floor(performance.now()),
            });
            resume = false;
            if (state.error?.message) {
                return state;
            }
            if (!going.has(state.status)) {
                // Stopped: send the final picture with it.
                const final = call({ ...base, op: `${request.kind}.state`, screen: true });
                if (final.error?.message) {
                    return final;
                }
                return { ...final, status: state.status, screen: final.screen ?? state.screen };
            }
            if (frameDue) {
                lastFrame = now;
                post({ type: 'frame', id, state });
            }
            if (state.status !== 'running') {
                // Nothing to compute until a key or the clock: do not spin.
                await sleep(5);
            } else if (request.speed === 0) {
                const elapsed = Math.max(performance.now() - started, 0.05);
                budget = Math.round(Math.min(Math.max((budget * 12) / elapsed, 1000), 50_000_000));
            }
        }
        if (request.speed === 0) {
            await yieldToEvents();
        } else {
            await sleep(request.speed < 100 ? 15 : 4);
        }
    }
}

/** Runs one of the long operations and replies when it ends, however it ends. */
function run(id: number, work: Promise<any>): void {
    active.add(id);
    work.catch((error) => ({ error: { message: String(error?.message ?? error) } })).then((reply) => {
        active.delete(id);
        cancelled.delete(id);
        post({ type: 'reply', id, reply });
    });
}

port.on('message', (message: ToWorker) => {
    switch (message.type) {
        case 'cancel':
            // A cancel for a run that has already ended is not remembered.
            if (active.has(message.id)) {
                cancelled.add(message.id);
            }
            break;
        case 'call': {
            let reply: any;
            try {
                reply = call(message.request);
            } catch (error) {
                reply = { error: { message: String((error as Error)?.message ?? error) } };
            }
            post({ type: 'reply', id: message.id, reply });
            break;
        }
        case 'runMachine':
            run(message.id, runMachine(message.id, message.request));
            break;
        case 'runTest':
            run(message.id, runTest(message.id, message.request));
            break;
    }
});
