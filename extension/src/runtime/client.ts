// The extension's handle on the runtime worker.

import * as path from 'node:path';
import { Worker } from 'node:worker_threads';
import * as vscode from 'vscode';
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
        const failed = (reason: string) => {
            if (this.worker === worker) {
                this.worker = undefined;
            }
            this.output.appendLine(`Nandwright: the simulator stopped (${reason}) and will restart on next use.`);
            for (const [, pending] of this.pending) {
                pending.resolve({ error: { message: `The simulator stopped unexpectedly (${reason}). Run it again.` } });
            }
            this.pending.clear();
        };
        worker.on('error', (error) => failed(error.message));
        worker.on('exit', (code) => {
            if (code !== 0) {
                failed(`exit code ${code}`);
            }
        });
        this.worker = worker;
        return worker;
    }

    private send(
        message: ToWorker,
        onEvents?: (events: TestEvent[]) => void,
        onFrame?: (state: MachineState) => void,
    ): Promise<any> {
        return new Promise((resolve) => {
            this.pending.set(message.id, { resolve, onEvents, onFrame });
            this.ensure().postMessage(message);
        });
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
     * Runs a test session until it finishes or pauses. `token` cancels it.
     * `breakLines` are zero-based lines of the script to stop at.
     */
    async runTest(
        session: number,
        options: { breakLines?: number[] },
        onEvents: (events: TestEvent[]) => void,
        token?: vscode.CancellationToken,
    ): Promise<TestStatus> {
        const id = this.nextId++;
        const subscription = token?.onCancellationRequested(() => {
            this.worker?.postMessage({ type: 'cancel', id } satisfies ToWorker);
        });
        try {
            const reply = await this.send(
                { type: 'runTest', id, request: { session, breakLines: options.breakLines ?? [] } },
                onEvents,
            );
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
        return {
            done,
            stop: () => this.worker?.postMessage({ type: 'cancel', id } satisfies ToWorker),
        };
    }

    /**
     * Gives the engine the text of every unsaved document, so a run uses what
     * the student sees in the editor and not what was last saved.
     */
    async syncUnsaved(): Promise<void> {
        await this.call({ op: 'overlay.clear' });
        for (const document of vscode.workspace.textDocuments) {
            if (document.isDirty && document.uri.scheme === 'file' && ENGINE_FILE.test(document.fileName)) {
                await this.call({ op: 'overlay.set', path: document.uri.fsPath, text: document.getText() });
            }
        }
    }

    close(session: number): void {
        if (this.worker) {
            void this.tryCall({ op: 'close', session });
        }
    }

    dispose(): void {
        void this.worker?.terminate();
        this.worker = undefined;
    }
}
