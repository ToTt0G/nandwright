// The runtime worker thread: test runs and simulators.
//
// It runs in a worker so that a chip with thousands of gates, or a program
// in a tight loop, never freezes the editor. Long work is done in slices; the
// worker returns to its event loop between slices, which is when a `cancel`
// message gets through.

import * as path from 'node:path';
import { parentPort } from 'node:worker_threads';
import { EngineCrash, WasmEngine } from '../engine/wasm';
import type { FromWorker, MachineRunRequest, ToWorker } from './protocol';

const wasmPath = path.join(__dirname, 'engine.wasm');
const port = parentPort!;

let engine = newEngine();
const cancelled = new Set<number>();

function newEngine(): WasmEngine {
    return new WasmEngine(wasmPath, (line) => post({ type: 'log', line }));
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
 * the slice budget, sizing slices so each takes about `SLICE_MS`.
 */
async function runTest(id: number, request: Record<string, unknown>): Promise<any> {
    let budget = 2000;
    const events: unknown[] = [];
    const failures: unknown[] = [];
    for (;;) {
        if (cancelled.delete(id)) {
            return { state: 'finished', outcome: { kind: 'cancelled' }, events, failures };
        }
        const started = performance.now();
        const status = call({ ...request, op: 'test.run', budget });
        if (status.error) {
            return status;
        }
        events.push(...(status.events ?? []));
        failures.push(...(status.failures ?? []));
        if (status.events?.length) {
            post({ type: 'progress', id, events: status.events });
        }
        if (status.state !== 'suspended' || status.reason !== 'budget') {
            return { ...status, events, failures };
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
 * Runs a program until it is paused (cancelled), reaches a breakpoint or
 * fails, posting a frame for the panel about thirty times a second.
 */
async function runMachine(id: number, request: MachineRunRequest): Promise<any> {
    const base = { session: request.session, breakLines: request.breakLines, ram: request.ram };
    let budget = 50_000;
    let owed = 0;
    let last = performance.now();
    let lastFrame = 0;
    for (;;) {
        if (cancelled.delete(id)) {
            return call({ ...base, op: `${request.kind}.state`, screen: true });
        }
        const now = performance.now();
        if (request.speed > 0) {
            // Paced: run the instructions that are due since the last slice.
            owed += (request.speed * (now - last)) / 1000;
            budget = Math.floor(owed);
            owed -= budget;
        }
        last = now;
        const frameDue = now - lastFrame >= FRAME_MS;
        if (budget > 0) {
            const started = performance.now();
            const state = call({ ...base, op: `${request.kind}.run`, budget, screen: frameDue });
            if (state.error?.message) {
                return state;
            }
            if (state.status !== 'running') {
                // Stopped: send the final picture with it.
                const final = call({ ...base, op: `${request.kind}.state`, screen: true });
                return { ...final, status: state.status, screen: final.screen ?? state.screen };
            }
            if (frameDue) {
                lastFrame = now;
                post({ type: 'frame', id, state });
            }
            if (request.speed === 0) {
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

port.on('message', (message: ToWorker) => {
    switch (message.type) {
        case 'cancel':
            cancelled.add(message.id);
            break;
        case 'call':
            post({ type: 'reply', id: message.id, reply: call(message.request) });
            break;
        case 'runMachine':
            runMachine(message.id, message.request).then(
                (reply) => post({ type: 'reply', id: message.id, reply }),
                (error) =>
                    post({
                        type: 'reply',
                        id: message.id,
                        reply: { error: { message: String(error?.message ?? error) } },
                    }),
            );
            break;
        case 'runTest':
            runTest(message.id, message.request).then(
                (reply) => post({ type: 'reply', id: message.id, reply }),
                (error) =>
                    post({
                        type: 'reply',
                        id: message.id,
                        reply: { error: { message: String(error?.message ?? error) } },
                    }),
            );
            break;
    }
});
