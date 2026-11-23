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
    isError,
} from './protocol';

/** A failure the user should read: an engine error message. */
export class RuntimeError extends Error {}

interface Pending {
    /** The worker that was asked: only it can answer, or fail, this request. */
    worker: Worker;
    resolve(reply: any): void;
    onEvents?: (events: TestEvent[]) => void;
    onFrame?: (state: MachineState) => void;
}

/** File types whose unsaved text the engine should see. */
const ENGINE_FILE = /\.(hdl|tst|cmp|asm|hack|vm|jack)$/i;

export class Runtime implements vscode.Disposable {
    private worker: Worker | undefined;
    private nextId = 1;
    private readonly pending = new Map<number, Pending>();
    private disposed = false;

    constructor(
        private readonly extensionPath: string,
        private readonly output: vscode.OutputChannel,
    ) {}

    private ensure(): Worker {
        if (this.worker) {
            return this.worker;
        }
        const worker = new Worker(path.join(this.extensionPath, 'dist', 'runtime.js'));
        worker.on('message', (message: FromWorker) => {
            if (message.type === 'log') {
                this.output.appendLine(message.line);
            } else if (message.type === 'progress') {
                this.pending.get(message.id)?.onEvents?.(message.events);
            } else if (message.type === 'frame') {
                this.pending.get(message.id)?.onFrame?.(message.state);
            } else {
                const pending = this.pending.get(message.id);
                this.pending.delete(message.id);
                pending?.resolve(message.reply);
            }
        });
        // A worker fails once (`error` is followed by `exit`), and takes only
        // its own requests with it: by the time `exit` arrives, a new worker
        // may already be answering new ones.
        let failed = false;
        const fail = (reason: string) => {
            if (failed) {
                return;
            }
            failed = true;
            if (this.worker === worker) {
                this.worker = undefined;
            }
            if (!this.disposed) {
                this.output.appendLine(`Nandwright: the simulator stopped (${reason}) and will restart on next use.`);
            }
            for (const [id, pending] of [...this.pending]) {
                if (pending.worker === worker) {
                    this.pending.delete(id);
                    pending.resolve({ error: { message: `The simulator stopped unexpectedly (${reason}). Run it again.` } });
                }
            }
        };
        worker.on('error', (error) => fail(error.message));
        worker.on('exit', (code) => fail(`exit code ${code}`));
        this.worker = worker;
        return worker;
    }

    private send(
        message: ToWorker,
        onEvents?: (events: TestEvent[]) => void,
        onFrame?: (state: MachineState) => void,
    ): Promise<any> {
        return new Promise((resolve) => {
            const worker = this.ensure();
            this.pending.set(message.id, { worker, resolve, onEvents, onFrame });
            worker.postMessage(message);
        });
    }

    /** Asks the worker that is running `id` to stop it. */
    private cancel(id: number): void {
        this.pending.get(id)?.worker.postMessage({ type: 'cancel', id } satisfies ToWorker);
    }

    /** One engine request. Rejects with `RuntimeError` on an engine error. */
    async call<T>(request: Record<string, unknown>): Promise<T> {
        const reply = await this.tryCall<T>(request);
        if (isError(reply)) {
            throw new RuntimeError(reply.error.message);
        }
        return reply;
    }

    /** One engine request, returning the error instead of throwing. */
    tryCall<T>(request: Record<string, unknown>): Promise<T | EngineError> {
        return this.send({ type: 'call', id: this.nextId++, request });
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
                onEvents,
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
        const done = this.send({ type: 'runMachine', id, request }, undefined, onFrame);
        return { done, stop: () => this.cancel(id) };
    }

    /**
     * Gives the engine the text of every unsaved document, so a run uses what
     * the student sees in the editor and not what was last saved. The buffers
     * are replaced in one request: nothing can read between "the old ones
     * are gone" and "the new ones are there".
     */
    async syncUnsaved(): Promise<void> {
        const files = vscode.workspace.textDocuments
            .filter((document) => document.isDirty && document.uri.scheme === 'file' && ENGINE_FILE.test(document.fileName))
            .map((document) => ({
                path: document.uri.fsPath,
                text: document.getText(),
                ignoreCase: ignoresCase(document.uri.fsPath),
            }));
        await this.call({ op: 'overlay.replace', files });
    }

    close(session: number): void {
        if (this.worker) {
            void this.tryCall({ op: 'close', session });
        }
    }

    dispose(): void {
        this.disposed = true;
        void this.worker?.terminate();
        this.worker = undefined;
    }
}
