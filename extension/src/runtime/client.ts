// The extension's handle on the runtime worker.

import * as path from 'node:path';
import { Worker } from 'node:worker_threads';
import * as vscode from 'vscode';
import { ignoresCase } from '../paths';
import {
    EngineError,
    FromWorker,
    MachineRunRequest,
    MachineState,
    TestEvent,
    TestStatus,
    ToWorker,
    WorkerStart,
    endsSessions,
    isError,
} from './protocol';

/** A failure the user should read: an engine error message. */
export class RuntimeError extends Error {}

/** How long a single worker `call`/`tryCall` may run without replying before the worker is treated as hung. */
export const CALL_TIMEOUT_MS = 10_000;

export interface RuntimeOptions {
    /** Deadline for single `call`/`tryCall` requests in milliseconds (`0` disables). Defaults to `CALL_TIMEOUT_MS`. */
    callTimeoutMs?: number;
}

interface Pending {
    /** The worker that was asked: only it can answer, or fail, this request. */
    worker: Worker;
    resolve(reply: any): void;
    timer?: NodeJS.Timeout;
    onEvents?: (events: TestEvent[]) => void;
    onFrame?: (state: MachineState) => void;
}

interface WorkerEntry {
    handle: Worker;
    fail(reason: string): void;
}

/** File types whose unsaved text the engine should see. */
const ENGINE_FILE = /\.(hdl|tst|cmp|asm|hack|vm|jack)$/i;

export class Runtime implements vscode.Disposable {
    private worker: WorkerEntry | undefined;
    private nextId = 1;
    private readonly pending = new Map<number, Pending>();
    private disposed = false;
    private readonly callTimeoutMs: number;
    /** Workers started so far: each numbers its sessions from its own range. */
    private workers = 0;
    private readonly restarted = new vscode.EventEmitter<void>();
    /**
     * Every session is gone: the worker stopped (fired once per worker,
     * before any request is sent to the worker that replaces it), or it
     * replaced its engine after a trap (fired before the reply that says
     * `crashed` is delivered).
     */
    readonly onDidStop = this.restarted.event;

    constructor(
        private readonly extensionPath: string,
        private readonly output: vscode.OutputChannel,
        options: RuntimeOptions = {},
    ) {
        this.callTimeoutMs = options.callTimeoutMs ?? CALL_TIMEOUT_MS;
    }

    private ensure(): WorkerEntry {
        if (this.worker) {
            return this.worker;
        }
        const start: WorkerStart = { worker: this.workers++ };
        const worker = new Worker(path.join(this.extensionPath, 'dist', 'runtime.js'), { workerData: start });
        // A worker fails once (`error` is followed by `exit`, and a timeout
        // terminates the worker too), and takes only its own requests with it:
        // by the time `exit` arrives, a new worker may already be answering new ones.
        let failed = false;
        const fail = (reason: string, terminate = false) => {
            if (failed) {
                return;
            }
            failed = true;
            if (this.worker?.handle === worker) {
                this.worker = undefined;
            }
            if (terminate) {
                void worker.terminate();
            }
            if (!this.disposed) {
                this.output.appendLine(`Nandwright: the simulator stopped (${reason}) and will restart on next use.`);
                this.restarted.fire();
            }
            for (const [id, pending] of [...this.pending]) {
                if (pending.worker === worker) {
                    this.pending.delete(id);
                    if (pending.timer) {
                        clearTimeout(pending.timer);
                    }
                    pending.resolve({ error: { message: `The simulator stopped unexpectedly (${reason}). Run it again.` } });
                }
            }
        };
        worker.on('message', (message: FromWorker) => {
            if (failed) {
                return;
            }
            if (message.type === 'log') {
                this.output.appendLine(message.line);
            } else if (message.type === 'progress') {
                this.pending.get(message.id)?.onEvents?.(message.events);
            } else if (message.type === 'frame') {
                this.pending.get(message.id)?.onFrame?.(message.state);
            } else {
                const pending = this.pending.get(message.id);
                this.pending.delete(message.id);
                if (pending?.timer) {
                    clearTimeout(pending.timer);
                }
                if (endsSessions(message.reply) && !this.disposed) {
                    // The worker replaced its engine: as for a worker that stopped,
                    // every session is gone, and its users hear it first.
                    this.output.appendLine('Nandwright: the simulator hit an internal error and restarted its engine.');
                    this.restarted.fire();
                }
                pending?.resolve(message.reply);
            }
        });
        worker.on('error', (error) => fail(error.message));
        worker.on('exit', (code) => fail(`exit code ${code}`));
        const entry: WorkerEntry = { handle: worker, fail: (reason) => fail(reason, true) };
        this.worker = entry;
        return entry;
    }

    private send(
        message: ToWorker,
        options?: {
            timeoutMs?: number;
            onEvents?: (events: TestEvent[]) => void;
            onFrame?: (state: MachineState) => void;
        },
    ): Promise<any> {
        return new Promise((resolve) => {
            if (this.disposed) {
                resolve({ error: { message: 'The simulator stopped unexpectedly (disposed). Run it again.' } });
                return;
            }
            const entry = this.ensure();
            const pending: Pending = {
                worker: entry.handle,
                resolve,
                onEvents: options?.onEvents,
                onFrame: options?.onFrame,
            };
            const timeoutMs = options?.timeoutMs;
            if (timeoutMs && timeoutMs > 0) {
                pending.timer = setTimeout(() => {
                    if (this.pending.has(message.id)) {
                        entry.fail(`timed out after ${timeoutMs} ms`);
                    }
                }, timeoutMs);
            }
            this.pending.set(message.id, pending);
            entry.handle.postMessage(message);
        });
    }

    /** Asks the worker that is running `id` to stop it. */
    private cancel(id: number): void {
        this.pending.get(id)?.worker.postMessage({ type: 'cancel', id } satisfies ToWorker);
    }

    /** One engine request. Rejects with `RuntimeError` on an engine error. */
    async call<T>(request: Record<string, unknown>, options?: { timeoutMs?: number }): Promise<T> {
        const reply = await this.tryCall<T>(request, options);
        if (isError(reply)) {
            throw new RuntimeError(reply.error.message);
        }
        return reply;
    }

    /** One engine request, returning the error instead of throwing. */
    tryCall<T>(request: Record<string, unknown>, options?: { timeoutMs?: number }): Promise<T | EngineError> {
        const timeoutMs = options?.timeoutMs ?? this.callTimeoutMs;
        return this.send({ type: 'call', id: this.nextId++, request }, { timeoutMs });
    }

    /**
     * Runs a test session until it finishes or pauses, or with `step` to the
     * end of the current `;` step. `token` stops it, whatever the script is
     * doing. `breakLines` are zero-based lines of the script to stop at.
     * Events arrive through `onEvents` as they happen.
     */
    async runTest(
        session: number,
        options: { breakLines?: number[]; step?: boolean },
        onEvents: (events: TestEvent[]) => void,
        token?: vscode.CancellationToken,
    ): Promise<TestStatus> {
        const id = this.nextId++;
        const subscription = token?.onCancellationRequested(() => this.cancel(id));
        try {
            const done = this.send(
                { type: 'runTest', id, request: { session, breakLines: options.breakLines ?? [], step: options.step } },
                { onEvents },
            );
            if (token?.isCancellationRequested) {
                this.cancel(id);
            }
            const reply = await done;
            if (isError(reply)) {
                throw new RuntimeError(reply.error.message);
            }
            return reply as TestStatus;
        } finally {
            subscription?.dispose();
        }
    }

    /**
     * Runs a program until it stops by itself or `stop()` is called. Frames
     * arrive through `onFrame`; the promise resolves with the final state.
     */
    runMachine(
        request: MachineRunRequest,
        onFrame: (state: MachineState) => void,
    ): { done: Promise<MachineState | EngineError>; stop(): void } {
        const id = this.nextId++;
        const done = this.send({ type: 'runMachine', id, request }, { onFrame });
        return { done, stop: () => this.cancel(id) };
    }

    /**
     * Captures the text of every unsaved engine document synchronously, so a
     * request can snapshot editor buffers and preference decisions together.
     */
    captureUnsaved(): { path: string; text: string; ignoreCase: boolean }[] {
        return vscode.workspace.textDocuments
            .filter((document) => document.isDirty && document.uri.scheme === 'file' && ENGINE_FILE.test(document.fileName))
            .map((document) => ({
                path: document.uri.fsPath,
                text: document.getText(),
                ignoreCase: ignoresCase(document.uri.fsPath),
            }));
    }

    /**
     * Gives the engine the text of every unsaved document, so a run uses what
     * the student sees in the editor and not what was last saved. The buffers
     * are replaced in one request: nothing can read between "the old ones
     * are gone" and "the new ones are there".
     */
    async syncUnsaved(): Promise<void> {
        await this.call({ op: 'overlay.replace', files: this.captureUnsaved() });
    }

    close(session: number): void {
        if (this.worker) {
            void this.tryCall({ op: 'close', session });
        }
    }

    dispose(): void {
        this.disposed = true;
        this.restarted.dispose();
        const current = this.worker;
        this.worker = undefined;
        for (const [id, pending] of [...this.pending]) {
            this.pending.delete(id);
            if (pending.timer) {
                clearTimeout(pending.timer);
            }
            pending.resolve({ error: { message: 'The simulator stopped unexpectedly (disposed). Run it again.' } });
        }
        void current?.handle.terminate();
    }
}

/**
 * Stops the worker of `runtime` as if it had crashed. Only for the
 * extension's test hooks, which exist only when the extension runs under
 * test (`ExtensionMode.Test`): nothing else can reach it.
 */
export async function crashWorkerForTests(runtime: Runtime): Promise<void> {
    const worker = (runtime as unknown as { worker: WorkerEntry | undefined }).worker?.handle;
    if (!worker) {
        return;
    }
    // Listeners run in order: once this one runs, the worker's own `exit` handling has.
    await new Promise<void>((resolve) => {
        worker.once('exit', () => resolve());
        void worker.terminate();
    });
}
