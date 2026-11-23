// Nandwright: Nand2Tetris in VS Code.

import * as path from 'node:path';
import * as vscode from 'vscode';
import {
    LanguageClient,
    LanguageClientOptions,
    ServerOptions,
    TransportKind,
} from 'vscode-languageclient/node';
import { ChipPanels } from './chipPanel';
import { ComputerPanels } from './computerPanel';
import { Outputs } from './outputs';
import { debugScript, registerDebugger } from './debug';
import { Runtime } from './runtime/client';
import { ScriptResult, TestExplorer } from './testing';

const LANGUAGES = ['hdl', 'tst', 'cmp', 'hack-asm', 'hack', 'vm', 'jack'];
const BUILTIN_SCHEME = 'n2t-builtin';

/** What the extension exposes to its own integration tests. */
export interface NandwrightApi {
    tests: TestExplorer;
    chips: ChipPanels;
    computers: ComputerPanels;
    outputs: Outputs;
    runtime: Runtime;
    client: LanguageClient;
}

function startLanguageClient(context: vscode.ExtensionContext, output: vscode.LogOutputChannel): LanguageClient {
    const module = context.asAbsolutePath(path.join('dist', 'server.js'));
    const serverOptions: ServerOptions = {
        run: { module, transport: TransportKind.ipc },
        debug: { module, transport: TransportKind.ipc },
    };
    const clientOptions: LanguageClientOptions = {
        documentSelector: LANGUAGES.flatMap((language) => [
            { scheme: 'file', language },
            { scheme: 'untitled', language },
        ]),
        synchronize: {
            fileEvents: vscode.workspace.createFileSystemWatcher('**/*.{hdl,tst,cmp,out,asm,hack,vm,jack}'),
        },
        outputChannel: output,
    };
    const client = new LanguageClient('nandwright', 'Nandwright', serverOptions, clientOptions);
    client.start().catch((error) => {
        output.appendLine(`The language server did not start: ${error}`);
        void vscode.window.showErrorMessage(
            'Nand2Tetris: the language server did not start. Testing and the simulator still work. See the Nandwright output for details.',
        );
    });
    context.subscriptions.push({ dispose: () => void client.stop() });
    return client;
}

/** Built-in chips open as read-only documents that show their interface. */
function registerBuiltinDocuments(context: vscode.ExtensionContext, client: LanguageClient): void {
    context.subscriptions.push(
        vscode.workspace.registerTextDocumentContentProvider(BUILTIN_SCHEME, {
            async provideTextDocumentContent(uri) {
                const name = path.basename(uri.path, '.hdl');
                try {
                    const reply = await client.sendRequest<{ text: string } | null>('n2t/builtinSource', { name });
                    return reply?.text ?? `// ${name} is not a built-in chip.\n`;
                } catch {
                    return `// The interface of ${name} is not available right now.\n`;
                }
            },
        }),
    );
}

function chipNameOf(document: vscode.TextDocument): string | undefined {
    return /^\s*CHIP\s+([A-Za-z_][A-Za-z0-9_]*)/m.exec(document.getText())?.[1];
}

/** The `.tst` scripts in a chip's folder that load it. */
async function scriptsForChip(chip: vscode.Uri): Promise<vscode.Uri[]> {
    const dir = vscode.Uri.joinPath(chip, '..');
    const name = path.basename(chip.fsPath, path.extname(chip.fsPath));
    const loads = new RegExp(`\\bload\\s+${name.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')}\\.hdl\\b`, 'i');
    const found: vscode.Uri[] = [];
    let entries: [string, vscode.FileType][] = [];
    try {
        entries = await vscode.workspace.fs.readDirectory(dir);
    } catch {
        return found;
    }
    for (const [entry] of entries.sort(([a], [b]) => a.localeCompare(b))) {
        if (!entry.toLowerCase().endsWith('.tst')) {
            continue;
        }
        const uri = vscode.Uri.joinPath(dir, entry);
        const open = vscode.workspace.textDocuments.find((d) => d.uri.toString() === uri.toString());
        const text = open ? open.getText() : new TextDecoder('latin1').decode(await vscode.workspace.fs.readFile(uri));
        if (loads.test(text.replace(/\/\/.*$/gm, ''))) {
            found.push(uri);
        }
    }
    return found;
}

/** The chip a script loads, if it is a chip script and the file exists. */
async function chipForScript(script: vscode.Uri): Promise<vscode.Uri | undefined> {
    const document = await vscode.workspace.openTextDocument(script);
    const match = /\bload\s+([A-Za-z_][A-Za-z0-9_]*\.hdl)\b/i.exec(document.getText().replace(/\/\/.*$/gm, ''));
    if (!match) {
        return undefined;
    }
    const chip = vscode.Uri.joinPath(script, '..', match[1]);
    try {
        await vscode.workspace.fs.stat(chip);
        return chip;
    } catch {
        return undefined;
    }
}

function targetUri(argument: unknown): vscode.Uri | undefined {
    if (argument instanceof vscode.Uri) {
        return argument;
    }
    return vscode.window.activeTextEditor?.document.uri;
}

const isHdl = (uri: vscode.Uri) => uri.fsPath.toLowerCase().endsWith('.hdl');
const isTst = (uri: vscode.Uri) => uri.fsPath.toLowerCase().endsWith('.tst');

async function scriptsFor(uri: vscode.Uri | undefined): Promise<vscode.Uri[] | undefined> {
    if (!uri) {
        void vscode.window.showInformationMessage('Open a chip (.hdl) or a test script (.tst) to run its test.');
        return undefined;
    }
    if (isTst(uri)) {
        return [uri];
    }
    if (isHdl(uri)) {
        const scripts = await scriptsForChip(uri);
        if (scripts.length === 0) {
            const name = path.basename(uri.fsPath);
            const choice = await vscode.window.showInformationMessage(
                `No test script in this folder loads ${name}. You can still try the chip in the simulator.`,
                'Open in Simulator',
            );
            if (choice) {
                await vscode.commands.executeCommand('nandwright.openChip', uri);
            }
            return undefined;
        }
        return scripts;
    }
    void vscode.window.showInformationMessage('Open a chip (.hdl) or a test script (.tst) to run its test.');
    return undefined;
}

function summarize(results: ScriptResult[]): void {
    if (results.length === 0) {
        return;
    }
    const quiet = ['passed', 'unchecked', 'cancelled', 'skipped'];
    const skipped = results.find((r) => r.outcome.kind === 'skipped');
    if (skipped && skipped.outcome.kind === 'skipped' && results.length === 1) {
        void vscode.window.showInformationMessage(`${path.basename(skipped.uri.fsPath)} was not run. ${skipped.outcome.reason}`);
        return;
    }
    const failed = results.filter((r) => !quiet.includes(r.outcome.kind));
    if (failed.length === 0) {
        const names = results.map((r) => path.basename(r.uri.fsPath)).join(', ');
        vscode.window.setStatusBarMessage(`$(pass) ${names} passed`, 6000);
        return;
    }
    const first = failed[0];
    const name = path.basename(first.uri.fsPath);
    let text: string;
    switch (first.outcome.kind) {
        case 'failed':
            text = `${name} failed: line ${first.outcome.line} differs from the compare file.`;
            break;
        case 'error':
            text = `${name}: ${first.outcome.message.split('\n')[0]}`;
            break;
        default:
            text = `${name} did not finish.`;
    }
    if (failed.length > 1) {
        text += ` (${failed.length} of ${results.length} scripts failed.)`;
    }
    vscode.window.setStatusBarMessage(`$(error) ${text}`, 10000);
    void vscode.window.showWarningMessage(text, 'Show Details', 'Open in Simulator').then(async (choice) => {
        if (choice === 'Show Details') {
            const range =
                first.outcome.kind === 'failed' && first.outcome.range
                    ? new vscode.Range(
                          first.outcome.range.startLine,
                          first.outcome.range.startCharacter,
                          first.outcome.range.endLine,
                          first.outcome.range.endCharacter,
                      )
                    : undefined;
            await vscode.window.showTextDocument(first.uri, { selection: range, preview: true });
            await vscode.commands.executeCommand('workbench.panel.testResults.view.focus');
        } else if (choice === 'Open in Simulator') {
            const chip = await chipForScript(first.uri);
            if (chip) {
                await vscode.commands.executeCommand('nandwright.openChip', chip);
            } else {
                void vscode.window.showInformationMessage(`${name} does not load a chip from this folder.`);
            }
        }
    });
}

async function openExample(context: vscode.ExtensionContext): Promise<void> {
    const source = vscode.Uri.joinPath(context.extensionUri, 'media', 'example');
    const target = vscode.Uri.joinPath(context.globalStorageUri, 'example');
    await vscode.workspace.fs.createDirectory(target);
    for (const [name] of await vscode.workspace.fs.readDirectory(source)) {
        const to = vscode.Uri.joinPath(target, name);
        try {
            // Keep the student's work if they have been here before.
            await vscode.workspace.fs.stat(to);
        } catch {
            await vscode.workspace.fs.copy(vscode.Uri.joinPath(source, name), to);
        }
    }
    await vscode.window.showTextDocument(vscode.Uri.joinPath(target, 'Majority.hdl'));
    await vscode.commands.executeCommand('setContext', 'nandwright.hasChips', true);
}

export async function activate(context: vscode.ExtensionContext): Promise<NandwrightApi> {
    const output = vscode.window.createOutputChannel('Nandwright', { log: true });
    context.subscriptions.push(output);

    const runtime = new Runtime(context.extensionPath, output);
    const client = startLanguageClient(context, output);
    registerBuiltinDocuments(context, client);
    const tests = new TestExplorer(runtime, debugScript);
    const chips = new ChipPanels(context.extensionUri, runtime);
    const computers = new ComputerPanels(context.extensionUri, runtime);
    const outputs = new Outputs(runtime);
    registerDebugger(context, runtime);
    context.subscriptions.push(runtime, tests, chips, computers, outputs);

    context.subscriptions.push(
        vscode.commands.registerCommand('nandwright.runTest', async (argument?: unknown) => {
            const scripts = await scriptsFor(targetUri(argument));
            if (scripts) {
                const results = await tests.runScripts(scripts);
                summarize(results);
                return results;
            }
            return [];
        }),
        vscode.commands.registerCommand('nandwright.debugTest', async (argument?: unknown) => {
            const scripts = await scriptsFor(targetUri(argument));
            if (scripts?.length) {
                await debugScript(scripts[0]);
            }
        }),
        vscode.commands.registerCommand('nandwright.openChip', async (argument?: unknown) => {
            let uri = targetUri(argument);
            if (uri && isTst(uri)) {
                uri = await chipForScript(uri);
            }
            if (!uri || !isHdl(uri)) {
                void vscode.window.showInformationMessage('Open a chip (.hdl) to see it in the simulator.');
                return undefined;
            }
            return chips.open(uri);
        }),
        vscode.commands.registerCommand('nandwright.runProgram', async (argument?: unknown) => {
            const uri = targetUri(argument);
            if (!uri || !/\.(asm|hack|vm|jack)$/i.test(uri.fsPath)) {
                void vscode.window.showInformationMessage(
                    'Open a program to run it: Hack assembly (.asm), machine code (.hack), VM code (.vm) or Jack (.jack).',
                );
                return undefined;
            }
            return computers.open(uri);
        }),
        vscode.commands.registerCommand('nandwright.compileJack', async (argument?: unknown) => {
            const uri = targetUri(argument);
            if (!uri || !/\.jack$/i.test(uri.fsPath)) {
                void vscode.window.showInformationMessage('Open a Jack file to compile the classes of its folder.');
                return undefined;
            }
            return outputs.produce('jack.compile', uri);
        }),
        vscode.commands.registerCommand('nandwright.assemble', async (argument?: unknown) => {
            const uri = targetUri(argument);
            if (!uri || !/\.asm$/i.test(uri.fsPath)) {
                void vscode.window.showInformationMessage('Open a Hack assembly file (.asm) to assemble it.');
                return undefined;
            }
            return outputs.produce('asm.assemble', uri);
        }),
        vscode.commands.registerCommand('nandwright.compareWithReference', async (argument?: unknown) => {
            const uri = targetUri(argument);
            if (!uri || !/\.(hack|vm|xml)$/i.test(uri.fsPath)) {
                void vscode.window.showInformationMessage(
                    'Open a .hack, .vm or .xml file that your own assembler, compiler or analyzer wrote to compare it with the reference.',
                );
                return undefined;
            }
            return outputs.compareWithReference(uri);
        }),
        vscode.commands.registerCommand('nandwright.compareOutput', async (argument?: unknown) => {
            const uri = targetUri(argument);
            if (!uri) {
                return;
            }
            const base = uri.fsPath.replace(/\.[^.\\/]+$/, '');
            const expected = vscode.Uri.file(`${base}.cmp`);
            const actual = vscode.Uri.file(`${base}.out`);
            for (const [file, hint] of [
                [expected, 'This test has no compare file.'],
                [actual, 'Run the test first: it writes the output file.'],
            ] as const) {
                try {
                    await vscode.workspace.fs.stat(file);
                } catch {
                    void vscode.window.showInformationMessage(`${path.basename(file.fsPath)} does not exist. ${hint}`);
                    return;
                }
            }
            const name = path.basename(base);
            await vscode.commands.executeCommand('vscode.diff', expected, actual, `${name}: expected (.cmp) ↔ actual (.out)`);
        }),
        vscode.commands.registerCommand('nandwright.getStarted', () =>
            vscode.commands.executeCommand(
                'workbench.action.openWalkthrough',
                `${context.extension.id}#nandwright.start`,
                false,
            ),
        ),
        vscode.commands.registerCommand('nandwright.openExample', () => openExample(context)),
        vscode.languages.registerCodeLensProvider(
            { language: 'hdl', scheme: 'file' },
            {
                async provideCodeLenses(document) {
                    const match = /^[ \t]*CHIP\b/m.exec(document.getText());
                    if (!match || !chipNameOf(document)) {
                        return [];
                    }
                    const position = document.positionAt(match.index);
                    const range = new vscode.Range(position, position);
                    const lenses: vscode.CodeLens[] = [];
                    const scripts = await scriptsForChip(document.uri);
                    if (scripts.length > 0) {
                        lenses.push(
                            new vscode.CodeLens(range, {
                                title: scripts.length === 1 ? '$(play) Run test' : `$(play) Run ${scripts.length} tests`,
                                command: 'nandwright.runTest',
                                arguments: [document.uri],
                                tooltip: scripts.map((s) => path.basename(s.fsPath)).join(', '),
                            }),
                            new vscode.CodeLens(range, {
                                title: '$(debug-alt) Step through',
                                command: 'nandwright.debugTest',
                                arguments: [document.uri],
                                tooltip: 'Run the test script one step at a time and watch the pins',
                            }),
                        );
                    }
                    lenses.push(
                        new vscode.CodeLens(range, {
                            title: '$(circuit-board) Open in simulator',
                            command: 'nandwright.openChip',
                            arguments: [document.uri],
                            tooltip: 'Set the inputs yourself and watch every pin',
                        }),
                    );
                    return lenses;
                },
            },
        ),
    );

    void vscode.workspace.findFiles('**/*.{hdl,tst,jack,asm,vm}', '**/node_modules/**', 1).then((found) => {
        void vscode.commands.executeCommand('setContext', 'nandwright.hasChips', found.length > 0);
    });
    void tests.discover();

    // Greet a new user once with the walkthrough.
    if (!context.globalState.get('welcomed') && context.extensionMode === vscode.ExtensionMode.Production) {
        void context.globalState.update('welcomed', true);
        void vscode.commands.executeCommand('nandwright.getStarted');
    }

    return { tests, chips, computers, outputs, runtime, client };
}

export function deactivate(): void {}
