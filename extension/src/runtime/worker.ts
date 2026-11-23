// The runtime worker thread: test runs and simulators.
//
// It runs in a worker so that a chip with thousands of gates, or a program
// in a tight loop, never freezes the editor. Long work is done in slices; the
// worker returns to its event loop between slices, which is when a `cancel`
// message gets through.

import * as path from 'node:path';
import { parentPort } from 'node:worker_threads';
import { EngineCrash, WasmEngine } from '../engine/wasm';
import type { FromWorker, ToWorker } from './protocol';

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

port.on('message', (message: ToWorker) => {
    switch (message.type) {
        case 'cancel':
            cancelled.add(message.id);
            break;
        case 'call':
            post({ type: 'reply', id: message.id, reply: call(message.request) });
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
