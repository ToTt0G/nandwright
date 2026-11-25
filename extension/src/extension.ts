// Nandwright: Nand2Tetris in VS Code.

import * as fs from 'node:fs';
import * as path from 'node:path';
import * as vscode from 'vscode';
import {
    LanguageClient,
    LanguageClientOptions,
    ServerOptions,
    TransportKind,
} from 'vscode-languageclient/node';
import { ChipPanels } from './chipPanel';
import { ChipSessions } from './chipSessions';
import { CircuitPanels } from './circuitPanel';
import { ComputerPanels, isProgram } from './computerPanel';
import { Outputs } from './outputs';
import { ignoresCase } from './paths';
import { Progress } from './progress';
import { debugScript, registerDebugger } from './debug';
import { Runtime, crashWorkerForTests } from './runtime/client';
import { ScriptResult, TestExplorer, rankScriptsForFile, scriptsForFile, textOf } from './testing';
import { compareFilesForScript, hasReferenceSource, primaryScriptTarget } from './testLinks';
import { addedFiles, handInFolder, planZip, projectFiles, writeProjectZip, zipPlaces, zipProject, zipSettings } from './zipProject';
import { SetupUi, offerSetup, runSetup, setupPlaces, vscodeSetupUi } from './setup';

const LANGUAGES = ['hdl', 'tst', 'cmp', 'hack-asm', 'hack', 'vm', 'jack'];
const BUILTIN_SCHEME = 'n2t-builtin';
const OUTPUT_SNAPSHOT_SCHEME = 'n2t-output';

/** What the extension exposes to its own integration tests. */
export interface TestHooks {
    /** Stops the runtime worker as if it had crashed. */
    crashWorker(): Promise<void>;
    /** The shared chip sessions (hubs) of the panels. */
    sessions: ChipSessions;
}

export interface NandwrightApi {
    tests: TestExplorer;
    chips: ChipPanels;
    circuits: CircuitPanels;
    computers: ComputerPanels;
    outputs: Outputs;
    progress: Progress;
    /** Zip Project Files after its dialogs: what is offered, and writing the zip. */
    zipProject: {
        projectFiles: typeof projectFiles;
        writeProjectZip: typeof writeProjectZip;
        addedFiles: typeof addedFiles;
        planZip: typeof planZip;
        zipSettings: typeof zipSettings;
        zipPlaces: typeof zipPlaces;
        handInFolder: typeof handInFolder;
    };
    /** Nandwright: Set Up… with given answers, and whether the offer to run it was shown. */
    setup: { run: (ui: SetupUi) => Promise<string[]>; offerShown: boolean };
    runtime: Runtime;
    client: LanguageClient;
    /** Only when the extension runs under test (`ExtensionMode.Test`); never in a user's editor. */
    testHooks?: TestHooks;
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

/** The chip a script loads, if it is a chip script and the file exists. */
async function chipForScript(script: vscode.Uri): Promise<vscode.Uri | undefined> {
    const document = await vscode.workspace.openTextDocument(script);
    const match = /\bload\s+"?([A-Za-z_][A-Za-z0-9_]*\.hdl)\b/i.exec(document.getText().replace(/\/\/.*$/gm, ''));
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

/** The program file a CPU or VM script tests, if one exists in its folder. */
async function programForScript(script: vscode.Uri): Promise<vscode.Uri | undefined> {
    const text = await textOf(script.fsPath);
    if (text === null) {
        return undefined;
    }
    const dir = path.dirname(script.fsPath);
    let entries: string[];
    try {
        entries = await fs.promises.readdir(dir);
    } catch {
        return undefined;
    }
    const name = primaryScriptTarget(text, entries, ignoresCase(script.fsPath));
    if (!name) {
        return undefined;
    }
    const target = vscode.Uri.file(path.join(dir, name));
    return isProgram(target) ? target : undefined;
}

function targetUri(argument: unknown): vscode.Uri | undefined {
    if (argument instanceof vscode.Uri) {
        return argument;
    }
    return vscode.window.activeTextEditor?.document.uri;
}

const isHdl = (uri: vscode.Uri) => uri.fsPath.toLowerCase().endsWith('.hdl');
const isTst = (uri: vscode.Uri) => uri.fsPath.toLowerCase().endsWith('.tst');

async function scriptsFor(uri: vscode.Uri | undefined): Promise<{ scripts: vscode.Uri[]; source?: vscode.Uri } | undefined> {
    if (!uri) {
        void vscode.window.showInformationMessage('Open a chip (.hdl), program (.asm, .hack, .vm, .jack) or test script (.tst) to run its test.');
        return undefined;
    }
    if (isTst(uri)) {
        return { scripts: [uri] };
    }
    if (isHdl(uri)) {
        const scripts = await scriptsForFile(uri);
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
        return { scripts, source: uri };
    }
    if (isProgram(uri)) {
        const scripts = await scriptsForFile(uri);
        if (scripts.length === 0) {
            const name = path.basename(uri.fsPath);
            const panel = /\.(vm|jack)$/i.test(uri.fsPath) ? 'VM emulator' : 'Hack computer';
            const choice = await vscode.window.showInformationMessage(
                `No automated test script in this folder tests ${name}. You can still run it in the ${panel}.`,
                'Run Program',
            );
            if (choice) {
                await vscode.commands.executeCommand('nandwright.runProgram', uri);
            }
            return undefined;
        }
        return { scripts, source: uri };
    }
    void vscode.window.showInformationMessage('Open a chip (.hdl), program (.asm, .hack, .vm, .jack) or test script (.tst) to run its test.');
    return undefined;
}

function summarize(results: ScriptResult[]): void {
    if (results.length === 0) {
        return;
    }
    const quiet = ['passed', 'unchecked', 'cancelled', 'skipped'];
    const skipped = results.find((r) => r.outcome.kind === 'skipped');
    if (skipped && skipped.outcome.kind === 'skipped' && results.length === 1) {
        const reason = skipped.outcome.reason;
        void programForScript(skipped.uri).then((program) => {
            if (program) {
                void vscode.window
                    .showInformationMessage(`${path.basename(skipped.uri.fsPath)} was not run. ${reason}`, 'Run Program')
                    .then((choice) => choice && vscode.commands.executeCommand('nandwright.runProgram', program));
            } else {
                void vscode.window.showInformationMessage(`${path.basename(skipped.uri.fsPath)} was not run. ${reason}`);
            }
        });
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
                return;
            }
            const program = await programForScript(first.uri);
            if (program) {
                await vscode.commands.executeCommand('nandwright.runProgram', program);
                return;
            }
            void vscode.window.showInformationMessage(`${name} does not load a chip or program from this folder.`);
        }
    });
}

/**
 * A CodeLens with the last result of a chip's or program's test scripts:
 * whether they pass, and how many rows differ when one does not. None before
 * a run, when a stored run tested a different input (e.g. `.asm` vs `.hack`),
 * or once an edit has made the result out of date.
 */
export function testResultLens(tests: TestExplorer, scripts: vscode.Uri[], forFile?: vscode.Uri): vscode.Command | undefined {
    const statuses = scripts.map((script) => ({ script, ...tests.statusOf(script, forFile) }));
    // Plain text: a lens with no command cannot be clicked.
    const text = (title: string, tooltip: string): vscode.Command => ({ title, tooltip, command: '' });
    if (statuses.some((status) => status.status === 'running')) {
        return text('$(sync~spin) Testing…', 'The test is running.');
    }
    const results = statuses.flatMap((status) => (status.status === 'current' ? [{ script: status.script, result: status.result }] : []));
    if (results.length !== scripts.length) {
        return undefined;
    }
    const passes = (kind: string) => kind === 'passed' || kind === 'unchecked';
    const failing = results.filter(({ result }) => !passes(result.kind));
    if (scripts.length > 1) {
        return failing.length === 0
            ? text(`$(pass) ${scripts.length} tests pass`, results.map(({ script }) => path.basename(script.fsPath)).join(', '))
            : text(`$(error) ${failing.length} of ${scripts.length} tests fail`, failing.map(({ script }) => path.basename(script.fsPath)).join(', '));
    }
    const [{ script, result }] = results;
    const name = path.basename(script.fsPath);
    switch (result.kind) {
        case 'passed':
            return text('$(pass) Test passes', `${name}: all ${result.rows ?? Math.max(0, result.comparedLines - 1)} rows match.`);
        case 'unchecked':
            return text('$(pass) Test passes', `${name}: ${result.message ?? ''}`);
        case 'failed': {
            const rows = result.rows ?? Math.max(0, result.comparedLines - 1);
            const differ = result.differingRows ?? result.failureCount;
            return text(`$(error) Test fails: ${differ} of ${rows} rows differ`, `${name} fails: ${differ} of ${rows} rows differ.`);
        }
        case 'error':
        case 'limit':
            return text('$(error) Test stopped with an error', `${name}: ${result.message ?? ''}`);
        default:
            return undefined;
    }
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
    const tests = new TestExplorer(runtime, debugScript, context.workspaceState);
    // Edits and saves of chip files: a reload after the same edit is done once for all panels.
    let generation = 0;
    const edited = (document: vscode.TextDocument) => {
        if (/\.hdl$/i.test(document.fileName)) {
            generation++;
        }
    };
    context.subscriptions.push(
        vscode.workspace.onDidChangeTextDocument((event) => event.contentChanges.length > 0 && edited(event.document)),
        vscode.workspace.onDidSaveTextDocument(edited),
    );
    const sessions = new ChipSessions(runtime, { generation: () => generation, log: (line) => output.appendLine(line) });
    const chips = new ChipPanels(context.extensionUri, sessions, tests);
    const circuits = new CircuitPanels(context.extensionUri, sessions, context.workspaceState, output);
    const computers = new ComputerPanels(context.extensionUri, runtime, tests);
    const outputs = new Outputs(runtime);
    const progress = new Progress(tests);
    registerDebugger(context, runtime);
    context.subscriptions.push(runtime, tests, chips, circuits, sessions, computers, outputs, progress);

    const outputSnapshots = new Map<string, string>();
    const outputSnapshotsChanged = new vscode.EventEmitter<vscode.Uri>();
    context.subscriptions.push(
        outputSnapshotsChanged,
        vscode.workspace.registerTextDocumentContentProvider(OUTPUT_SNAPSHOT_SCHEME, {
            onDidChange: outputSnapshotsChanged.event,
            provideTextDocumentContent: (uri) => outputSnapshots.get(uri.path) ?? '',
        }),
    );

    // The result of the last run is shown in the editor, beside Run test, so
    // nobody has to open the Test Results view to see whether a chip passes.
    const lensesChanged = new vscode.EventEmitter<void>();
    let lensTimer: NodeJS.Timeout | undefined;
    context.subscriptions.push(
        lensesChanged,
        tests.onDidChangeStatus(() => {
            clearTimeout(lensTimer);
            lensTimer = setTimeout(() => lensesChanged.fire(), 150);
        }),
        { dispose: () => clearTimeout(lensTimer) },
    );

    context.subscriptions.push(
        vscode.commands.registerCommand('nandwright.runTest', async (argument?: unknown) => {
            const found = await scriptsFor(targetUri(argument));
            if (found) {
                const results = await tests.runScripts(found.scripts, { source: found.source });
                summarize(results);
                return results;
            }
            return [];
        }),
        vscode.commands.registerCommand('nandwright.debugTest', async (argument?: unknown) => {
            const found = await scriptsFor(targetUri(argument));
            if (found?.scripts.length) {
                const ordered = found.source
                    ? await rankScriptsForFile(found.scripts, found.source, tests)
                    : found.scripts;
                await debugScript(ordered[0], found.source);
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
        vscode.commands.registerCommand('nandwright.showCircuit', async (argument?: unknown) => {
            let uri = targetUri(argument);
            if (uri && isTst(uri)) {
                uri = await chipForScript(uri);
            }
            if (!uri || !isHdl(uri)) {
                void vscode.window.showInformationMessage('Open a chip (.hdl) to see its circuit diagram.');
                return undefined;
            }
            return circuits.open(uri);
        }),
        vscode.commands.registerCommand('nandwright.runProgram', async (argument?: unknown) => {
            const uri = targetUri(argument);
            if (!uri || !isProgram(uri)) {
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
            let uri: vscode.Uri | undefined;
            let forFile: vscode.Uri | undefined;
            if (
                argument
                && typeof argument === 'object'
                && 'script' in argument
                && (argument as { script?: unknown }).script instanceof vscode.Uri
            ) {
                uri = (argument as { script: vscode.Uri }).script;
                const candidate = (argument as { forFile?: unknown }).forFile;
                if (candidate instanceof vscode.Uri) {
                    forFile = candidate;
                }
            } else {
                uri = targetUri(argument);
            }
            if (!uri) {
                return;
            }
            const base = uri.fsPath.replace(/\.[^.\\/]+$/, '');
            let selectedScript: vscode.Uri | undefined;
            let expectedPath: string | null = `${base}.cmp`;
            let actualPath = `${base}.out`;
            if (isTst(uri)) {
                selectedScript = uri;
                const text = await textOf(uri.fsPath);
                if (text !== null) {
                    const pair = compareFilesForScript(text, uri.fsPath);
                    expectedPath = pair.comparePath;
                    actualPath = pair.outputPath;
                }
            } else if (isHdl(uri) || isProgram(uri)) {
                forFile = uri;
                const scripts = await rankScriptsForFile(await scriptsForFile(uri), uri, tests);
                for (const script of scripts) {
                    const text = await textOf(script.fsPath);
                    if (text === null) {
                        continue;
                    }
                    const pair = compareFilesForScript(text, script.fsPath);
                    if (pair.comparePath && fs.existsSync(pair.comparePath)) {
                        selectedScript = script;
                        expectedPath = pair.comparePath;
                        actualPath = pair.outputPath;
                        break;
                    }
                }
                if (!selectedScript && scripts.length > 0) {
                    selectedScript = scripts[0];
                    const text = await textOf(selectedScript.fsPath);
                    if (text !== null) {
                        const pair = compareFilesForScript(text, selectedScript.fsPath);
                        expectedPath = pair.comparePath;
                        actualPath = pair.outputPath;
                    }
                }
            }
            if (!expectedPath) {
                void vscode.window.showInformationMessage('This test script does not name a valid .cmp compare file in its folder.');
                return;
            }
            const runStatus = selectedScript ? tests.statusOf(selectedScript, forFile) : undefined;
            const runResult = runStatus?.status === 'current' ? runStatus.result : undefined;
            if (runResult?.comparePath) {
                expectedPath = runResult.comparePath;
            }
            if (runResult?.outputPath) {
                actualPath = runResult.outputPath;
            }
            const expected = vscode.Uri.file(expectedPath);
            try {
                await vscode.workspace.fs.stat(expected);
            } catch {
                void vscode.window.showInformationMessage(`${path.basename(expected.fsPath)} does not exist. This test has no compare file.`);
                return;
            }
            let actual: vscode.Uri = vscode.Uri.file(actualPath);
            if (runResult?.outputText !== undefined) {
                let diskMatches = false;
                try {
                    const diskText = await fs.promises.readFile(actualPath, 'utf8');
                    diskMatches = diskText.replace(/\r\n/g, '\n') === runResult.outputText.replace(/\r\n/g, '\n');
                } catch {
                    diskMatches = false;
                }
                if (!diskMatches) {
                    const snapshotUri = vscode.Uri.from({ scheme: OUTPUT_SNAPSHOT_SCHEME, path: actual.path });
                    outputSnapshots.set(snapshotUri.path, runResult.outputText);
                    outputSnapshotsChanged.fire(snapshotUri);
                    actual = snapshotUri;
                }
            } else {
                try {
                    await vscode.workspace.fs.stat(actual);
                } catch {
                    void vscode.window.showInformationMessage(`${path.basename(actual.fsPath)} does not exist. Run the test first: it writes the output file.`);
                    return;
                }
            }
            const name = path.basename(expectedPath, path.extname(expectedPath));
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
        vscode.commands.registerCommand('nandwright.zipProject', (argument?: unknown) => zipProject(argument)),
        vscode.commands.registerCommand('nandwright.setup', () => runSetup(vscodeSetupUi, setupPlaces())),
        vscode.languages.registerCodeLensProvider(
            { language: 'hdl', scheme: 'file' },
            {
                onDidChangeCodeLenses: lensesChanged.event,
                async provideCodeLenses(document) {
                    const match = /^[ \t]*CHIP\b/m.exec(document.getText());
                    if (!match || !chipNameOf(document)) {
                        return [];
                    }
                    const position = document.positionAt(match.index);
                    const range = new vscode.Range(position, position);
                    const lenses: vscode.CodeLens[] = [];
                    const scripts = await scriptsForFile(document.uri);
                    if (scripts.length > 0) {
                        const result = testResultLens(tests, scripts);
                        if (result) {
                            lenses.push(new vscode.CodeLens(range, result));
                        }
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
        vscode.languages.registerCodeLensProvider(
            ['hack-asm', 'hack', 'vm', 'jack', 'xml'].map((language) => ({ language, scheme: 'file' })),
            {
                onDidChangeCodeLenses: lensesChanged.event,
                async provideCodeLenses(document) {
                    const uri = document.uri;
                    const range = new vscode.Range(0, 0, 0, 0);
                    const fileName = path.basename(uri.fsPath);
                    const dir = path.dirname(uri.fsPath);
                    let entries: string[] = [];
                    try {
                        entries = await fs.promises.readdir(dir);
                    } catch {
                        // Folder unreadable.
                    }
                    const ignoreCase = ignoresCase(uri.fsPath);
                    if (document.languageId === 'xml') {
                        if (!hasReferenceSource(fileName, entries, ignoreCase)) {
                            return [];
                        }
                        return [
                            new vscode.CodeLens(range, {
                                title: '$(diff) Compare with reference',
                                command: 'nandwright.compareWithReference',
                                arguments: [uri],
                                tooltip: 'Diff this XML file against the built-in syntax analyzer output',
                            }),
                        ];
                    }
                    const lenses: vscode.CodeLens[] = [];
                    const scripts = await scriptsForFile(uri);
                    if (scripts.length > 0) {
                        const result = testResultLens(tests, scripts, uri);
                        if (result) {
                            lenses.push(new vscode.CodeLens(range, result));
                        }
                        lenses.push(
                            new vscode.CodeLens(range, {
                                title: scripts.length === 1 ? '$(play) Run test' : `$(play) Run ${scripts.length} tests`,
                                command: 'nandwright.runTest',
                                arguments: [uri],
                                tooltip: scripts.map((s) => path.basename(s.fsPath)).join(', '),
                            }),
                            new vscode.CodeLens(range, {
                                title: '$(debug-alt) Step through',
                                command: 'nandwright.debugTest',
                                arguments: [uri],
                                tooltip: 'Run the test script one step at a time and watch registers and memory',
                            }),
                        );
                    }
                    if ((document.languageId === 'hack' || document.languageId === 'vm') && hasReferenceSource(fileName, entries, ignoreCase)) {
                        lenses.push(
                            new vscode.CodeLens(range, {
                                title: '$(diff) Compare with reference',
                                command: 'nandwright.compareWithReference',
                                arguments: [uri],
                                tooltip: 'Diff this file against the built-in reference tool output',
                            }),
                        );
                    }
                    const isVm = document.languageId === 'vm' || document.languageId === 'jack';
                    lenses.push(
                        new vscode.CodeLens(range, {
                            title: isVm ? '$(vm-running) Open in VM emulator' : '$(vm-running) Open in Hack computer',
                            command: 'nandwright.runProgram',
                            arguments: [uri],
                            tooltip: 'Run with the screen, keyboard, registers, memory and expected output',
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

    const offerShown = offerSetup(context);

    return {
        tests,
        chips,
        circuits,
        computers,
        outputs,
        progress,
        zipProject: { projectFiles, writeProjectZip, addedFiles, planZip, zipSettings, zipPlaces, handInFolder },
        setup: { run: (ui: SetupUi) => runSetup(ui, setupPlaces()), offerShown },
        runtime,
        client,
        ...(context.extensionMode === vscode.ExtensionMode.Test ? { testHooks: { crashWorker: () => crashWorkerForTests(runtime), sessions } } : {}),
    };
}

export function deactivate(): void {}
