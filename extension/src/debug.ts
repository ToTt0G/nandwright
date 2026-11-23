// A debug adapter that steps through a test script.
//
// It uses VS Code's own debugger UI: F5 to start, F10 to run the script up
// to its next `;`, F11 for one command, breakpoints in the gutter of the
// .tst file, and the Variables view for the pins (or registers and memory)
// of whatever the script is testing.

import * as path from 'node:path';
import * as vscode from 'vscode';
import { Runtime, RuntimeError } from './runtime/client';
import { Pin, Range, TestEvent, TestInspection, TestOpened, TestOutcome, TestStatus } from './runtime/protocol';
import { formatValue, keyName } from './values';

const THREAD = 1;
const FRAME = 1;

/**
 * One scope, so that VS Code opens it by itself: a student should see the
 * pins without hunting for them. Values are listed inputs first, then
 * outputs, then internal pins (or registers, then memory).
 */
const SCOPE = 1;
const ROLE_ORDER: Pin['role'][] = ['input', 'output', 'internal', 'register', 'memory'];

interface Request {
    seq: number;
    type: 'request';
    command: string;
    arguments?: any;
}

class ScriptDebugSession implements vscode.DebugAdapter {
    private readonly emitter = new vscode.EventEmitter<vscode.DebugProtocolMessage>();
    readonly onDidSendMessage = this.emitter.event;
    private seq = 1;
    private session: number | undefined;
    private script = '';
    private breakLines: number[] = [];
    private next: Range | null = null;
    private pins: Pin[] = [];
    private finished = false;
    private running: vscode.CancellationTokenSource | undefined;
    private launched = false;
    private configured = false;
    private stopOnEntry = true;

    constructor(private readonly runtime: Runtime) {}

    private send(message: Record<string, unknown>): void {
        this.emitter.fire({ seq: this.seq++, ...message } as vscode.DebugProtocolMessage);
    }

    private respond(request: Request, body: unknown = {}): void {
        this.send({ type: 'response', request_seq: request.seq, command: request.command, success: true, body });
    }

    private fail(request: Request, message: string): void {
        this.send({
            type: 'response',
            request_seq: request.seq,
            command: request.command,
            success: false,
            message,
            body: { error: { id: 1, format: message, showUser: true } },
        });
    }

    private event(event: string, body: unknown = {}): void {
        this.send({ type: 'event', event, body });
    }

    private say(text: string, category: 'console' | 'stdout' | 'stderr' | 'important' = 'console'): void {
        this.event('output', { category, output: `${text}\n` });
    }

    handleMessage(message: vscode.DebugProtocolMessage): void {
        const request = message as unknown as Request;
        if (request.type !== 'request') {
            return;
        }
        this.dispatch(request).catch((error) => {
            this.fail(request, error instanceof Error ? error.message : String(error));
        });
    }

    private async dispatch(request: Request): Promise<void> {
        const args = request.arguments ?? {};
        switch (request.command) {
            case 'initialize':
                this.respond(request, {
                    supportsConfigurationDoneRequest: true,
                    supportsEvaluateForHovers: true,
                    supportsTerminateRequest: true,
                    supportsRestartRequest: false,
                });
                this.event('initialized');
                return;
            case 'launch':
                await this.launch(request, args);
                return;
            case 'setBreakpoints': {
                const sameFile = !this.script || samePath(args.source?.path ?? '', this.script);
                const lines: number[] = (args.breakpoints ?? []).map((b: { line: number }) => b.line - 1);
                if (sameFile) {
                    this.breakLines = lines;
                }
                this.respond(request, {
                    breakpoints: lines.map((line) => ({ verified: sameFile, line: line + 1 })),
                });
                return;
            }
            case 'configurationDone':
                this.configured = true;
                this.respond(request);
                await this.begin();
                return;
            case 'threads':
                this.respond(request, { threads: [{ id: THREAD, name: 'Test script' }] });
                return;
            case 'stackTrace': {
                const frames = this.next
                    ? [
                          {
                              id: FRAME,
                              name: path.basename(this.script),
                              source: { name: path.basename(this.script), path: this.script },
                              line: this.next.startLine + 1,
                              column: this.next.startCharacter + 1,
                              endLine: this.next.endLine + 1,
                              endColumn: this.next.endCharacter + 1,
                          },
                      ]
                    : [];
                this.respond(request, { stackFrames: frames, totalFrames: frames.length });
                return;
            }
            case 'scopes': {
                const chip = this.pins.some((pin) => pin.role === 'input' || pin.role === 'output' || pin.role === 'internal');
                this.respond(request, {
                    scopes: [{ name: chip ? 'Pins' : 'Registers and memory', variablesReference: SCOPE, expensive: false }],
                });
                return;
            }
            case 'variables': {
                const pins = args.variablesReference === SCOPE ? this.pins : [];
                const variables = [...pins]
                    .sort((a, b) => ROLE_ORDER.indexOf(a.role) - ROLE_ORDER.indexOf(b.role))
                    .map((pin) => ({
                        name: pin.name,
                        value: `${describe(pin)}  · ${pin.role}`,
                        type: pin.width === 1 ? `${pin.role}, 1 bit` : `${pin.role}, ${pin.width} bits`,
                        variablesReference: 0,
                    }));
                this.respond(request, { variables });
                return;
            }
            case 'evaluate': {
                const name = String(args.expression ?? '').trim();
                const pin = this.pins.find((p) => p.name === name) ?? this.pins.find((p) => p.name.toLowerCase() === name.toLowerCase());
                if (!pin) {
                    this.fail(request, `No pin or register named ${name}`);
                    return;
                }
                this.respond(request, { result: describe(pin), variablesReference: 0 });
                return;
            }
            case 'continue':
                this.respond(request, { allThreadsContinued: true });
                await this.advance('continue');
                return;
            case 'next':
                this.respond(request);
                await this.advance('statement');
                return;
            case 'stepIn':
            case 'stepOut':
                this.respond(request);
                await this.advance('command');
                return;
            case 'pause':
                this.running?.cancel();
                this.respond(request);
                return;
            case 'terminate':
            case 'disconnect':
                this.running?.cancel();
                this.close();
                this.respond(request);
                if (request.command === 'terminate') {
                    this.event('terminated');
                }
                return;
            default:
                this.respond(request);
        }
    }

    private async launch(request: Request, args: any): Promise<void> {
        const script: string | undefined = args.script;
        if (!script || !script.toLowerCase().endsWith('.tst')) {
            this.fail(request, 'Open a .tst test script to step through it.');
            this.event('terminated');
            return;
        }
        this.script = script;
        this.stopOnEntry = args.stopOnEntry !== false;
        try {
            await this.runtime.syncUnsaved();
            const opened = await this.runtime.call<TestOpened>({
                op: 'test.open',
                path: script,
                crlf: process.platform === 'win32',
                collectAll: false,
                autoKeyboard: vscode.workspace.getConfiguration('nandwright').get('test.pressKeys', true),
            });
            this.session = opened.session;
            this.next = opened.next;
        } catch (error) {
            this.fail(request, error instanceof RuntimeError ? error.message : String(error));
            this.event('terminated');
            return;
        }
        this.launched = true;
        this.respond(request);
        this.say(`Stepping through ${path.basename(script)}. Step Over runs to the next ";", Step Into runs one command.`);
        await this.begin();
    }

    /** Starts once both `launch` and `configurationDone` have arrived. */
    private async begin(): Promise<void> {
        if (!this.launched || !this.configured || this.finished) {
            return;
        }
        this.launched = false;
        if (this.stopOnEntry) {
            await this.refresh();
            this.event('stopped', { reason: 'entry', threadId: THREAD, allThreadsStopped: true });
        } else {
            await this.advance('continue');
        }
    }

    private async refresh(): Promise<void> {
        if (this.session === undefined) {
            return;
        }
        const inspection = await this.runtime.call<TestInspection>({ op: 'test.inspect', session: this.session });
        this.pins = inspection.state;
        this.next = inspection.next;
    }

    private report(events: TestEvent[]): void {
        for (const event of events) {
            switch (event.type) {
                case 'output':
                    this.say(event.line, 'stdout');
                    break;
                case 'echo':
                    this.say(`The script says: ${event.text}`, 'important');
                    break;
                case 'key':
                    this.say(`Pressed ${keyName(event.code)} for the script, which was waiting for it.`);
                    break;
                case 'note':
                    this.say(`Note: ${event.text}`, 'important');
                    break;
            }
        }
    }

    private async advance(how: 'continue' | 'statement' | 'command'): Promise<void> {
        if (this.session === undefined || this.finished) {
            return;
        }
        let status: TestStatus;
        if (how === 'continue') {
            this.running = new vscode.CancellationTokenSource();
            status = await this.runtime.runTest(
                this.session,
                { breakLines: this.breakLines },
                (events) => this.report(events),
                this.running.token,
            );
            this.running = undefined;
        } else {
            status = await this.runtime.call<TestStatus>({ op: 'test.step', session: this.session, unit: how });
            this.report(status.events);
        }
        if (status.state === 'finished') {
            const outcome = status.outcome!;
            if (outcome.kind === 'cancelled') {
                // Paused by the user.
                await this.refresh();
                this.event('stopped', { reason: 'pause', threadId: THREAD, allThreadsStopped: true });
                return;
            }
            this.finish(outcome);
            return;
        }
        await this.refresh();
        const reason = status.reason === 'breakpoint' ? 'breakpoint' : status.reason === 'pause' ? 'pause' : 'step';
        this.event('stopped', { reason, threadId: THREAD, allThreadsStopped: true });
    }

    private finish(outcome: TestOutcome): void {
        this.finished = true;
        switch (outcome.kind) {
            case 'passed':
                this.say(`End of script: comparison ended successfully (${outcome.comparedLines} lines compared).`, 'important');
                break;
            case 'unchecked':
                this.say('End of script. It has no compare-to, so nothing was checked.', 'important');
                break;
            case 'failed':
                this.say(
                    `Comparison failure at line ${outcome.line}.\n  expected: ${outcome.expected ?? '(end of the compare file)'}\n  actual:   ${outcome.actual ?? '(nothing)'}`,
                    'stderr',
                );
                break;
            case 'error':
                this.say(outcome.message, 'stderr');
                break;
            case 'limit':
                this.say('The script was still running after the command limit and was stopped.', 'stderr');
                break;
            case 'cancelled':
            case 'skipped':
                break;
        }
        this.close();
        this.event('terminated');
    }

    private close(): void {
        if (this.session !== undefined) {
            this.runtime.close(this.session);
            this.session = undefined;
        }
    }

    dispose(): void {
        this.running?.cancel();
        this.close();
        this.emitter.dispose();
    }
}

function samePath(a: string, b: string): boolean {
    const normalize = (p: string) => path.normalize(p).replace(/\\/g, '/').toLowerCase();
    return normalize(a) === normalize(b);
}

/** A value with its other spellings, as the Variables view shows it. */
function describe(pin: Pin): string {
    const decimal = formatValue(pin.bits, pin.width, 'dec');
    if (pin.width === 1) {
        return decimal;
    }
    return `${decimal}  (${formatValue(pin.bits, pin.width, 'bin')})`;
}

export function registerDebugger(context: vscode.ExtensionContext, runtime: Runtime): void {
    context.subscriptions.push(
        vscode.debug.registerDebugAdapterDescriptorFactory('nandwright', {
            createDebugAdapterDescriptor: () =>
                new vscode.DebugAdapterInlineImplementation(new ScriptDebugSession(runtime)),
        }),
        vscode.debug.registerDebugConfigurationProvider('nandwright', {
            // F5 with no launch.json: step through the open script.
            resolveDebugConfiguration(_folder, config) {
                if (!config.type && !config.request && !config.name) {
                    const editor = vscode.window.activeTextEditor;
                    if (editor?.document.fileName.toLowerCase().endsWith('.tst')) {
                        return {
                            type: 'nandwright',
                            request: 'launch',
                            name: 'Step through the test script',
                            script: editor.document.uri.fsPath,
                        };
                    }
                    void vscode.window.showInformationMessage('Open a .tst test script to step through it.');
                    return undefined;
                }
                return config;
            },
        }),
    );
}

/** Starts stepping through a script. */
export function debugScript(uri: vscode.Uri): Thenable<boolean> {
    return vscode.debug.startDebugging(vscode.workspace.getWorkspaceFolder(uri), {
        type: 'nandwright',
        request: 'launch',
        name: `Step through ${path.basename(uri.fsPath)}`,
        script: uri.fsPath,
    });
}
