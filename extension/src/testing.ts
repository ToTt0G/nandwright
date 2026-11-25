// Test scripts in VS Code's Testing view.
//
// Every `.tst` file in the workspace is a test. Running one executes the
// script in the engine and reports what the official tools would: pass, or
// the first compare line that differs. On top of that a failure shows every
// failing row and the value of every pin at that step.

import * as fs from 'node:fs';
import * as path from 'node:path';
import * as vscode from 'vscode';
import { Runtime, RuntimeError } from './runtime/client';
import {
    MAX_REPORTED_FAILURES,
    Pin,
    Range,
    TestEvent,
    TestFailure,
    TestInspection,
    TestOpened,
    TestOutcome,
    TestStatus,
    isError,
} from './runtime/protocol';
import { ignoresCase, pathKey, samePath } from './paths';
import { FolderListing, RunInput, StoredResult, countRows, replaces, restore, resultMatchesFile, toSave } from './resultStore';
import {
    compareFilesForScript,
    findByStem,
    isForeverScript,
    orderScriptCandidates,
    parseScriptLinks,
    scriptTestsFile,
    scriptUsesSavedFile,
} from './testLinks';
import { describePin, keyName } from './values';

export { commandFiles, namedFiles, withoutComments } from './testLinks';

/** Maximum `.out` snapshot retained in memory for exact run diffs. */
const MAX_OUTPUT_SNAPSHOT_BYTES = 512 * 1024;

/** A script is stopped after this many commands. The longest official test
 * runs a few million; a script still going after this is stuck in a loop. */
const MAX_COMMANDS = 200_000_000;

/** Failing rows reported individually; the diff still shows all of them. */
const MAX_FAILURE_MESSAGES = 25;

const EXCLUDE = '**/{node_modules,.git,target,dist}/**';

/** Files the engine reads: a change to one can change a test's result. */
export const ENGINE_FILE = /\.(hdl|tst|cmp|asm|hack|vm|jack)$/i;

/** Saves in one folder within this long are run together. */
const SAVE_DELAY_MS = 200;

const STORE_KEY = 'nandwright.results';

export type { StoredResult } from './resultStore';

export type ScriptStatus =
    | { status: 'none' | 'running'; result?: StoredResult }
    | { status: 'current' | 'stale'; result: StoredResult };

export function toRange(range: Range): vscode.Range {
    return new vscode.Range(range.startLine, range.startCharacter, range.endLine, range.endCharacter);
}

/** The result of one script, as the rest of the extension sees it. */
export interface ScriptResult {
    uri: vscode.Uri;
    outcome: TestOutcome | { kind: 'error'; message: string; range: null };
    failures: TestFailure[];
    keysPressed: number[];
}

export class TestExplorer implements vscode.Disposable {
    readonly controller: vscode.TestController;
    private readonly disposables: vscode.Disposable[] = [];
    private readonly resultEmitter = new vscode.EventEmitter<ScriptResult>();
    /** Fires after each script finishes. Used by commands and by tests. */
    readonly onDidFinishScript = this.resultEmitter.event;
    private ready: Promise<void> | undefined;
    private readonly results = new Map<string, { uri: vscode.Uri; result: StoredResult }>();
    /** Per-(script, loaded-input) results so a source build never masquerades as an artifact run. */
    private readonly resultsByTarget = new Map<string, { uri: vscode.Uri; result: StoredResult }>();
    private readonly running = new Map<string, number>();
    /** When an engine file in a folder last changed, by folder key. */
    private readonly changedAt = new Map<string, number>();
    /** Recent in-editor save times of source/artifact files to break coarse-FS mtime ties. */
    private readonly savedFileAt = new Map<string, number>();
    private readonly statusEmitter = new vscode.EventEmitter<vscode.Uri | undefined>();
    /** Fires when a script starts or finishes, or results go stale (with no script). */
    readonly onDidChangeStatus = this.statusEmitter.event;
    private readonly saveTimers = new Map<string, { timer: NodeJS.Timeout; saved: vscode.Uri[] }>();
    private readonly autoRuns = new Map<string, { source: vscode.CancellationTokenSource; pending: Map<string, vscode.Uri>; done: Promise<unknown> }>();
    private readonly foreverCache = new Map<string, boolean>();
    private readonly requestSources = new WeakMap<vscode.TestRunRequest, vscode.Uri[]>();
    private persistTimer: NodeJS.Timeout | undefined;

    constructor(
        private readonly runtime: Runtime,
        private readonly startDebugging: (uri: vscode.Uri) => Thenable<boolean>,
        private readonly state?: vscode.Memento,
    ) {
        this.controller = vscode.tests.createTestController('nandwright', 'Nand2Tetris');
        this.controller.resolveHandler = async (item) => {
            if (!item) {
                await this.discover();
            }
        };
        this.controller.refreshHandler = () => this.rediscover();
        this.controller.createRunProfile(
            'Run',
            vscode.TestRunProfileKind.Run,
            async (request, token) => {
                await this.run(request, token);
            },
            true,
        );
        this.controller.createRunProfile(
            'Step through the script',
            vscode.TestRunProfileKind.Debug,
            (request) => this.debug(request),
            true,
        );

        const watcher = vscode.workspace.createFileSystemWatcher('**/*.tst');
        watcher.onDidCreate((uri) => this.add(uri));
        watcher.onDidDelete((uri) => this.remove(uri));
        this.disposables.push(
            watcher,
            this.resultEmitter,
            vscode.workspace.onDidChangeWorkspaceFolders(() => this.rediscover()),
            vscode.workspace.onDidOpenTextDocument((document) => {
                if (document.uri.scheme === 'file' && document.fileName.toLowerCase().endsWith('.tst')) {
                    this.add(document.uri);
                }
            }),
            this.statusEmitter,
        );
        this.watchChanges();
        this.load();
    }

    // ----- Results, and when they stop being current -----

    private watchChanges(): void {
        const changed = (uri: vscode.Uri, at: number) => {
            if (uri.scheme !== 'file' || !ENGINE_FILE.test(uri.fsPath)) {
                return;
            }
            const folder = pathKey(path.dirname(uri.fsPath));
            if (at > (this.changedAt.get(folder) ?? 0)) {
                this.changedAt.set(folder, at);
                this.statusEmitter.fire(undefined);
                // A result that has gone stale must not be saved as one that holds.
                this.persist();
            }
        };
        // A file event can arrive well after the change (and after a run that
        // already saw it): the file's own time says when it changed, clamped to
        // `Date.now()` so a file with a future mtime (e.g. zip/clock skew) does
        // not mark every later run in the folder stale.
        const onDisk = (uri: vscode.Uri) => {
            fs.promises.stat(uri.fsPath).then(
                (stat) => changed(uri, Math.min(stat.mtimeMs, Date.now())),
                () => changed(uri, Date.now()),
            );
        };
        // `.out` files are written by the runs themselves and are not engine files.
        const watcher = vscode.workspace.createFileSystemWatcher('**/*.{hdl,tst,cmp,asm,hack,vm,jack,HDL,TST,CMP,ASM,HACK,VM,JACK}');
        watcher.onDidChange(onDisk);
        watcher.onDidCreate(onDisk);
        watcher.onDidDelete((uri) => changed(uri, Date.now()));
        this.disposables.push(
            watcher,
            vscode.workspace.onDidChangeTextDocument((event) => {
                if (event.contentChanges.length > 0) {
                    changed(event.document.uri, Date.now());
                }
            }),
            vscode.workspace.onDidSaveTextDocument((document) => {
                const now = Date.now();
                if (document.uri.scheme === 'file' && ENGINE_FILE.test(document.fileName)) {
                    this.savedFileAt.set(pathKey(document.uri.fsPath), now);
                }
                changed(document.uri, now);
                this.saved(document);
            }),
        );
    }

    /**
     * The last result of a script and whether it still holds. When `forFile` is
     * provided (e.g. a `.hack` editor or panel), only a run that actually tested
     * that file is returned, so a source build of `double.asm` never shows a
     * passing result for a broken `Double.hack`.
     */
    statusOf(script: vscode.Uri, forFile?: vscode.Uri): ScriptStatus {
        const key = pathKey(script.fsPath);
        let entry: StoredResult | undefined;
        if (forFile) {
            const targetKey = `${key}\n${pathKey(forFile.fsPath)}`;
            const specific = this.resultsByTarget.get(targetKey)?.result;
            const main = this.results.get(key)?.result;
            if (specific) {
                entry = specific;
            } else if (main && resultMatchesFile(main, forFile.fsPath, ignoresCase(forFile.fsPath))) {
                entry = main;
            }
        } else {
            entry = this.results.get(key)?.result;
        }
        if (this.running.has(key)) {
            return { status: 'running', result: entry };
        }
        if (!entry) {
            return { status: 'none' };
        }
        const scriptChanged = this.changedAt.get(pathKey(path.dirname(script.fsPath))) ?? 0;
        const fileChanged = forFile ? (this.changedAt.get(pathKey(path.dirname(forFile.fsPath))) ?? 0) : 0;
        const changed = Math.max(scriptChanged, fileChanged);
        // From the start of the run: an edit made while it ran is not in its result.
        return { status: changed > entry.started ? 'stale' : 'current', result: entry };
    }

    private indexTargets(uri: vscode.Uri, result: StoredResult): void {
        const scriptKey = pathKey(uri.fsPath);
        for (const input of result.inputs ?? []) {
            const targetKey = `${scriptKey}\n${pathKey(input.path)}`;
            if (replaces(this.resultsByTarget.get(targetKey)?.result, result)) {
                this.resultsByTarget.set(targetKey, { uri, result });
            }
        }
    }

    private store(uri: vscode.Uri, result: StoredResult): void {
        const key = pathKey(uri.fsPath);
        let updated = false;
        if (replaces(this.results.get(key)?.result, result)) {
            this.results.set(key, { uri, result });
            updated = true;
        }
        this.indexTargets(uri, result);
        if (updated) {
            this.persist();
        }
    }

    private setRunning(uri: vscode.Uri, delta: 1 | -1): void {
        const key = pathKey(uri.fsPath);
        const count = (this.running.get(key) ?? 0) + delta;
        if (count > 0) {
            this.running.set(key, count);
        } else {
            this.running.delete(key);
        }
        this.statusEmitter.fire(uri);
    }

    private persist(): void {
        if (!this.state) {
            return;
        }
        clearTimeout(this.persistTimer);
        this.persistTimer = setTimeout(() => {
            const entries = [...this.results.values()].map(({ uri, result }) => ({
                path: uri.fsPath,
                result,
                current: this.statusOf(uri).status === 'current',
            }));
            void this.state!.update(STORE_KEY, toSave(entries, listFolder));
        }, 1000);
    }

    /** Results of earlier sessions that still hold. */
    private load(): void {
        for (const { path: file, result } of restore(this.state?.get(STORE_KEY), listFolder)) {
            const uri = vscode.Uri.file(file);
            this.results.set(pathKey(file), { uri, result });
            this.indexTargets(uri, result);
        }
        this.persist();
    }

    /**
     * Whether a script repeats forever (a demonstration to watch, not a
     * test). The engine says; the answer is kept per version of the script.
     */
    async runsForever(script: vscode.Uri): Promise<boolean> {
        const open = vscode.workspace.textDocuments.find((d) => d.uri.scheme === 'file' && samePath(d.uri.fsPath, script.fsPath));
        let version: string;
        if (open?.isDirty) {
            version = `text:${open.getText()}`;
        } else {
            try {
                version = `mtime:${(await fs.promises.stat(script.fsPath)).mtimeMs}`;
            } catch {
                return false;
            }
        }
        const key = `${pathKey(script.fsPath)}\n${version}`;
        const known = this.foreverCache.get(key);
        if (known !== undefined) {
            return known;
        }
        let answer = false;
        try {
            // Always: the engine may still hold unsaved text from before a save or a revert.
            await this.runtime.syncUnsaved();
            const opened = await this.runtime.tryCall<TestOpened>({ op: 'test.open', path: script.fsPath });
            if (!isError(opened)) {
                this.runtime.close(opened.session);
                answer = opened.runsForever;
            }
        } catch {
            // Not known: treat it as a test, and its run will say what is wrong.
        }
        if (this.foreverCache.size > 1000) {
            this.foreverCache.clear();
        }
        this.foreverCache.set(key, answer);
        return answer;
    }

    // ----- Running the tests of a saved file -----

    private saved(document: vscode.TextDocument): void {
        if (document.uri.scheme !== 'file' || !ENGINE_FILE.test(document.fileName)) {
            return;
        }
        if (!vscode.workspace.getConfiguration('nandwright').get<boolean>('test.runOnSave', true)) {
            return;
        }
        const folder = pathKey(path.dirname(document.uri.fsPath));
        const pending = this.saveTimers.get(folder);
        clearTimeout(pending?.timer);
        const saved = [...(pending?.saved ?? []).filter((uri) => !samePath(uri.fsPath, document.uri.fsPath)), document.uri];
        const timer = setTimeout(() => {
            this.saveTimers.delete(folder);
            void this.runSaved(folder, saved);
        }, SAVE_DELAY_MS);
        this.saveTimers.set(folder, { timer, saved });
    }

    /**
     * Runs the scripts that use the files saved. A newer save in the folder
     * cancels the automatic run still going there; the new run takes over
     * the scripts that one had not finished.
     */
    private runSaved(folder: string, saved: vscode.Uri[]): Promise<unknown> {
        const previous = this.autoRuns.get(folder);
        previous?.source.cancel();
        const source = new vscode.CancellationTokenSource();
        const pending = new Map<string, vscode.Uri>();
        const entry = { source, pending, done: Promise.resolve() as Promise<unknown> };
        this.autoRuns.set(folder, entry);
        entry.done = (async () => {
            for (const file of saved) {
                for (const script of await scriptsUsing(file)) {
                    pending.set(pathKey(script.fsPath), script);
                }
            }
            if (previous) {
                await previous.done;
                previous.pending.forEach((uri, key) => pending.set(key, uri));
            }
            if (source.token.isCancellationRequested) {
                return;
            }
            const tests: vscode.Uri[] = [];
            for (const [key, script] of [...pending]) {
                if (await this.runsForever(script)) {
                    pending.delete(key);
                } else {
                    tests.push(script);
                }
            }
            if (tests.length === 0 || source.token.isCancellationRequested) {
                return;
            }
            // Only this run's own results count as done: a manual run of the
            // same script finishing meanwhile does not finish it here.
            const savedSources = saved.filter((uri) => /\.(asm|jack|hack|vm)$/i.test(uri.fsPath));
            const results = await this.runScripts(tests, {
                quiet: true,
                token: source.token,
                ...(savedSources.length > 0 ? { source: savedSources } : {}),
            });
            for (const result of results) {
                if (result.outcome.kind !== 'cancelled') {
                    pending.delete(pathKey(result.uri.fsPath));
                }
            }
        })()
            .catch(() => {
                // An automatic run that cannot start shows nothing; a manual run will say why.
            })
            .finally(() => {
                source.dispose();
                if (this.autoRuns.get(folder) === entry) {
                    this.autoRuns.delete(folder);
                }
            });
        return entry.done;
    }

    /** Settles when the automatic runs started by saves so far have finished, for tests. */
    async savedRunsSettled(): Promise<void> {
        for (;;) {
            await new Promise((resolve) => setTimeout(resolve, SAVE_DELAY_MS + 50));
            if (this.saveTimers.size === 0 && this.autoRuns.size === 0) {
                return;
            }
            await Promise.all([...this.autoRuns.values()].map((run) => run.done));
        }
    }

    /** Finds every script in the workspace. Safe to call repeatedly. */
    discover(): Promise<void> {
        this.ready ??= (async () => {
            const files = await vscode.workspace.findFiles('**/*.tst', EXCLUDE);
            for (const uri of files) {
                this.add(uri);
            }
        })();
        return this.ready;
    }

    private async rediscover(): Promise<void> {
        this.controller.items.replace([]);
        this.ready = undefined;
        await this.discover();
    }

    /** The test item of a script, creating it (and its folders) if needed. */
    add(uri: vscode.Uri): vscode.TestItem {
        const folder = vscode.workspace.getWorkspaceFolder(uri);
        let collection = this.controller.items;
        if (folder) {
            const segments = path.relative(folder.uri.fsPath, path.dirname(uri.fsPath)).split(path.sep).filter(Boolean);
            const roots = vscode.workspace.workspaceFolders ?? [];
            let current = folder.uri;
            if (roots.length > 1) {
                collection = this.group(collection, folder.uri, folder.name);
            }
            for (const segment of segments) {
                current = vscode.Uri.joinPath(current, segment);
                collection = this.group(collection, current, segment);
            }
        }
        const id = uri.toString();
        let item = collection.get(id);
        if (!item) {
            item = this.controller.createTestItem(id, path.basename(uri.fsPath), uri);
            item.range = new vscode.Range(0, 0, 0, 0);
            collection.add(item);
        }
        return item;
    }

    private group(collection: vscode.TestItemCollection, uri: vscode.Uri, label: string): vscode.TestItemCollection {
        const id = `${uri.toString()}/`;
        let item = collection.get(id);
        if (!item) {
            item = this.controller.createTestItem(id, label, uri);
            collection.add(item);
        }
        return item.children;
    }

    private remove(uri: vscode.Uri): void {
        const id = uri.toString();
        const visit = (collection: vscode.TestItemCollection): boolean => {
            if (collection.get(id)) {
                collection.delete(id);
                return true;
            }
            let found = false;
            collection.forEach((item) => {
                if (!found && visit(item.children)) {
                    found = true;
                    if (item.children.size === 0 && item.id.endsWith('/')) {
                        collection.delete(item.id);
                    }
                }
            });
            return found;
        };
        visit(this.controller.items);
    }

    /** The scripts a request selects, in a stable order. */
    private scripts(request: vscode.TestRunRequest): vscode.TestItem[] {
        const excluded = new Set((request.exclude ?? []).map((item) => item.id));
        const scripts: vscode.TestItem[] = [];
        const visit = (item: vscode.TestItem) => {
            if (excluded.has(item.id)) {
                return;
            }
            if (item.id.endsWith('/')) {
                item.children.forEach(visit);
            } else {
                scripts.push(item);
            }
        };
        if (request.include) {
            request.include.forEach(visit);
        } else {
            this.controller.items.forEach(visit);
        }
        return scripts.sort((a, b) => a.id.localeCompare(b.id));
    }

    /**
     * Runs scripts and shows the results in the Testing view. A quiet run
     * opens no view and moves no focus: it is marked as an automatic
     * (continuous) run, for which VS Code does not open the failure peek, and
     * keeps focus, so the Test Results view does not open either. A run that
     * is not quiet behaves like one started in the Testing view.
     */
    async runScripts(
        uris: vscode.Uri[],
        options: { quiet?: boolean; token?: vscode.CancellationToken; source?: vscode.Uri | vscode.Uri[] } = {},
    ): Promise<ScriptResult[]> {
        const items = uris.map((uri) => this.add(uri));
        const request =
            options.quiet === undefined
                ? new vscode.TestRunRequest(items)
                : new vscode.TestRunRequest(items, undefined, undefined, options.quiet, options.quiet);
        if (options.source) {
            this.requestSources.set(request, Array.isArray(options.source) ? options.source : [options.source]);
        }
        return this.run(request, options.token ?? new vscode.CancellationTokenSource().token);
    }

    private async debug(request: vscode.TestRunRequest): Promise<void> {
        const [first] = this.scripts(request);
        if (first?.uri) {
            await this.startDebugging(first.uri);
        }
    }

    private async run(request: vscode.TestRunRequest, token: vscode.CancellationToken): Promise<ScriptResult[]> {
        const run = this.controller.createTestRun(request);
        const results: ScriptResult[] = [];
        const scripts = this.scripts(request);
        scripts.forEach((item) => run.enqueued(item));
        const explicitSource = this.requestSources.get(request);
        try {
            // Capture unsaved buffers synchronously once for this run; each
            // `test.open` passes this exact snapshot atomically with its
            // `preferSources` so concurrent edits or panel reloads cannot race.
            const synced = Date.now();
            const overlay = this.runtime.captureUnsaved();
            for (const item of scripts) {
                if (token.isCancellationRequested) {
                    run.skipped(item);
                    continue;
                }
                this.setRunning(item.uri!, 1);
                let result: ScriptResult;
                try {
                    result = await this.runOne(run, item, token, synced, overlay, explicitSource);
                } finally {
                    this.setRunning(item.uri!, -1);
                }
                results.push(result);
                this.resultEmitter.fire(result);
            }
        } catch (error) {
            const message = error instanceof Error ? error.message : String(error);
            run.appendOutput(`${message}\r\n`);
            void vscode.window.showErrorMessage(`Nand2Tetris: ${message}`);
        } finally {
            run.end();
        }
        return results;
    }

    /**
     * Computes the `test.open` execution and source-building options for
     * `script`, matching `runOne` so test replays (e.g. row inspection in
     * `ComputerPanel`) load the exact same code and respect the same settings.
     */
    async openOptionsFor(
        script: vscode.Uri,
        explicitSources: vscode.Uri[] | undefined,
        overlay: { path: string; text: string; ignoreCase: boolean }[],
    ): Promise<{
        buildSources: boolean;
        preferSources: string[];
        autoKeyboard: boolean;
        stopEndlessWaits: boolean;
        ignoreCase: boolean;
        maxCommands: number;
    }> {
        const explicitOnlyArtifacts =
            explicitSources !== undefined
            && explicitSources.length > 0
            && explicitSources.every((u) => /\.(hack|vm)$/i.test(u.fsPath));
        const config = vscode.workspace.getConfiguration('nandwright');
        const buildSources = !explicitOnlyArtifacts && config.get<boolean>('test.buildSources', true);
        const preferSources = buildSources
            ? await preferredSources(script, explicitSources, overlay, this.savedFileAt)
            : [];
        return {
            buildSources,
            preferSources,
            autoKeyboard: config.get('test.pressKeys', true),
            stopEndlessWaits: true,
            ignoreCase: ignoresCase(script.fsPath),
            maxCommands: MAX_COMMANDS,
        };
    }

    private async runOne(
        run: vscode.TestRun,
        item: vscode.TestItem,
        token: vscode.CancellationToken,
        synced: number,
        overlay: { path: string; text: string; ignoreCase: boolean }[],
        explicitSources?: vscode.Uri[],
    ): Promise<ScriptResult> {
        const uri = item.uri!;
        const name = path.basename(uri.fsPath);
        const say = (text: string) => run.appendOutput(`${text.replace(/\r?\n/g, '\r\n')}\r\n`, undefined, item);
        const started = Date.now();
        run.started(item);
        const notes: string[] = [];
        const fallbackErrorInputs = (): RunInput[] | undefined => {
            if (!explicitSources || explicitSources.length === 0) {
                return undefined;
            }
            return explicitSources.map((u) => ({
                path: u.fsPath,
                builtFromSource: /\.(asm|jack)$/i.test(u.fsPath),
            }));
        };
        const remember = (kind: TestOutcome['kind'], details: Partial<StoredResult> = {}) =>
            this.store(uri, { kind, comparedLines: 0, failureCount: 0, failures: [], notes, started: synced, finished: Date.now(), ...details });

        const errored = (message: string, range: Range | null, kind: 'error' | 'limit' = 'error', inputs?: RunInput[]): ScriptResult => {
            const testMessage = new vscode.TestMessage(message);
            if (range) {
                testMessage.location = new vscode.Location(uri, toRange(range));
            }
            run.errored(item, testMessage, Date.now() - started);
            say(`✗ ${name}: ${message}`);
            remember(kind, { message, ...(inputs && inputs.length > 0 ? { inputs } : {}) });
            return { uri, outcome: { kind: 'error', message, range: null }, failures: [], keysPressed: [] };
        };

        let session: number | undefined;
        try {
            const openOptions = await this.openOptionsFor(uri, explicitSources, overlay);
            const opened = await this.runtime.call<TestOpened>({
                op: 'test.open',
                path: uri.fsPath,
                crlf: process.platform === 'win32',
                collectAll: true,
                // Nobody can press a key in the Testing view: a wait that nothing can end is not worth 200 million commands.
                ...openOptions,
                overlay,
            });
            session = opened.session;
            if (opened.runsForever) {
                const how =
                    opened.dialect === 'hardware'
                        ? 'use "Step Through the Script" to run it.'
                        : 'open the program with "Run Program" to interact with it, or use "Step Through the Script".';
                const reason = `This script repeats forever by design (it has a \`repeat\` without a count), so it has no result to report. It is meant to be watched: ${how}`;
                run.skipped(item);
                say(`- ${name}: skipped. ${reason}`);
                remember('skipped', { message: reason });
                return { uri, outcome: { kind: 'skipped', reason }, failures: [], keysPressed: [] };
            }
            const lines: string[] = [];
            const failures: TestFailure[] = [];
            const keysPressed: number[] = [];
            const onEvents = (events: TestEvent[]) => {
                for (const event of events) {
                    if (event.type === 'echo') {
                        say(`  ${name} says: ${event.text}`);
                    } else if (event.type === 'note') {
                        notes.push(event.text);
                        say(`  Note: ${event.text}`);
                    } else if (event.type === 'key') {
                        keysPressed.push(event.code);
                        say(`  Pressed ${keyName(event.code)} for the script, which was waiting for it.`);
                    } else if (event.type === 'output') {
                        lines.push(event.line);
                    }
                }
            };
            let status: TestStatus;
            for (;;) {
                status = await this.runtime.runTest(session, {}, onEvents, token);
                for (const failure of status.failures) {
                    if (failures.length < MAX_REPORTED_FAILURES) {
                        failures.push(failure);
                    }
                }
                // A `!` pause or a script breakpoint: carry on, as "run" means run.
                if (status.state === 'finished') {
                    break;
                }
            }
            const outcome = status.outcome!;
            const duration = Date.now() - started;
            const inspectMeta = async (): Promise<{
                inspection: TestInspection;
                meta: Pick<StoredResult, 'inputs' | 'comparePath' | 'outputPath' | 'outputText'>;
            }> => {
                const inspection = await this.runtime.call<TestInspection>({ op: 'test.inspect', session });
                const outputText =
                    inspection.output !== null && Buffer.byteLength(inspection.output, 'utf8') <= MAX_OUTPUT_SNAPSHOT_BYTES
                        ? inspection.output
                        : undefined;
                return {
                    inspection,
                    meta: {
                        ...(inspection.loaded && inspection.loaded.length > 0 ? { inputs: inspection.loaded } : {}),
                        ...(inspection.comparePath ? { comparePath: inspection.comparePath } : {}),
                        ...(inspection.outputPath ? { outputPath: inspection.outputPath } : {}),
                        ...(outputText !== undefined ? { outputText } : {}),
                    },
                };
            };
            switch (outcome.kind) {
                case 'passed': {
                    const { meta } = await inspectMeta();
                    run.passed(item, duration);
                    remember('passed', {
                        ...meta,
                        comparedLines: outcome.comparedLines,
                        rows: Math.max(0, outcome.comparedLines - 1),
                        differingRows: 0,
                    });
                    say(`✓ ${name}: comparison ended successfully (${outcome.comparedLines} lines compared)`);
                    if (outcome.unreachedLines > 0) {
                        say(
                            `  Note: the compare file has ${outcome.unreachedLines} more line(s) that the script never produced. The official tools pass this too.`,
                        );
                    }
                    break;
                }
                case 'unchecked': {
                    const { meta } = await inspectMeta();
                    run.passed(item, duration);
                    const reason = 'the script ran to its end. It has no compare-to, so nothing was checked.';
                    remember('unchecked', { ...meta, message: reason });
                    say(`✓ ${name}: ${reason}`);
                    break;
                }
                case 'failed': {
                    const { inspection, meta } = await inspectMeta();
                    run.failed(item, failureMessages(uri, failures, inspection, notes), duration);
                    // Every line the script wrote was compared, up to the end of the compare file.
                    const comparedLines = Math.min(lines.length, inspection.expected?.length ?? lines.length);
                    remember('failed', {
                        ...meta,
                        comparedLines,
                        ...countRows(inspection.expected ?? [], comparedLines, failures.map((failure) => failure.line)),
                        failureCount: outcome.failureCount,
                        failures: failures.map((failure) => ({ line: failure.line, actual: failure.actual })),
                    });
                    say(`✗ ${name}: comparison failure at line ${outcome.line}`);
                    say(`    expected: ${outcome.expected ?? '(the compare file ended here)'}`);
                    say(`    actual:   ${outcome.actual ?? '(the script produced no more lines)'}`);
                    if (outcome.failureCount > 1) {
                        say(`    ${outcome.failureCount} lines differ in all.`);
                    }
                    say(comparisonTable(inspection.expected ?? [], lines));
                    break;
                }
                case 'error': {
                    const { meta } = await inspectMeta();
                    return errored(outcome.message, outcome.range, 'error', meta.inputs ?? fallbackErrorInputs());
                }
                case 'limit': {
                    const { meta } = await inspectMeta();
                    return errored(
                        `The script was still running after ${outcome.commands.toLocaleString('en-US')} commands, so it was stopped.` +
                            (outcome.echo ? ` Its last message was: "${outcome.echo}".` : '') +
                            ' A script that loops forever or waits for input is meant to be used interactively.',
                        null,
                        'limit',
                        meta.inputs ?? fallbackErrorInputs(),
                    );
                }
                case 'cancelled':
                    run.skipped(item);
                    say(`- ${name}: cancelled`);
                    remember('cancelled', { message: 'cancelled' });
                    break;
                case 'skipped':
                    run.skipped(item);
                    remember('skipped', { message: outcome.reason });
                    break;
            }
            return { uri, outcome, failures, keysPressed };
        } catch (error) {
            if (error instanceof RuntimeError) {
                return errored(error.message, null);
            }
            throw error;
        } finally {
            if (session !== undefined) {
                this.runtime.close(session);
            }
        }
    }

    dispose(): void {
        this.saveTimers.forEach(({ timer }) => clearTimeout(timer));
        this.autoRuns.forEach(({ source }) => source.cancel());
        clearTimeout(this.persistTimer);
        this.controller.dispose();
        this.disposables.forEach((d) => d.dispose());
    }
}

function groupPins(pins: Pin[]): string {
    const groups: [string, Pin['role'][]][] = [
        ['in', ['input']],
        ['out', ['output']],
        ['internal', ['internal']],
        ['registers', ['register']],
        ['memory', ['memory']],
    ];
    return groups
        .map(([label, roles]) => {
            const members = pins.filter((pin) => roles.includes(pin.role));
            return members.length ? `${label}: ${members.map(describePin).join('  ')}` : '';
        })
        .filter(Boolean)
        .join('\n');
}

/**
 * The compare file and the produced output side by side, with a mark on each
 * line that differs: the whole picture, for the Test Results panel.
 */
export function comparisonTable(expected: string[], produced: string[]): string {
    const width = Math.max(8, ...expected.map((line) => line.length));
    const rows = [`      ${'expected'.padEnd(width)}   actual`];
    for (let i = 0; i < Math.max(expected.length, produced.length); i++) {
        const want = expected[i];
        const got = produced[i];
        if (got === undefined) {
            // The script stopped producing output here (or never got this far).
            break;
        }
        const same = want !== undefined && want.length === got.length && [...want].every((c, k) => c === '*' || c === got[k]);
        rows.push(`${String(i + 1).padStart(3)} ${same ? ' ' : '≠'} ${(want ?? '').padEnd(width)}   ${got}`);
    }
    return rows.join('\n');
}

/** The names of the columns in which two rows of an output table differ. */
export function differingColumns(header: string | undefined, expected: string | null, actual: string | null): string[] {
    if (!header || expected === null || actual === null) {
        return [];
    }
    const cells = (row: string) => row.split('|').slice(1, -1);
    const [names, want, got] = [cells(header), cells(expected), cells(actual)];
    if (want.length !== got.length || want.length !== names.length) {
        return [];
    }
    return names.filter((_, i) => want[i] !== got[i]).map((name) => name.trim());
}

/**
 * One message per failing row, at the script line that produced it, with the
 * pin values at that step.
 */
function failureMessages(
    uri: vscode.Uri,
    failures: TestFailure[],
    inspection: TestInspection,
    notes: string[],
): vscode.TestMessage[] {
    const compareName = inspection.comparePath ? path.basename(inspection.comparePath) : 'the compare file';
    const header = inspection.expected?.[0];
    const messages: vscode.TestMessage[] = [];
    for (const failure of failures.slice(0, MAX_FAILURE_MESSAGES)) {
        const columns = differingColumns(header, failure.expected, failure.actual);
        const where = columns.length > 0 ? `: ${columns.join(', ')} ${columns.length === 1 ? 'differs' : 'differ'}` : ' differs';
        const text = [
            ...notes.map((note) => `Note: ${note}\n`),
            `Line ${failure.line} of ${compareName}${where}`,
            '',
            `expected  ${failure.expected ?? '(the compare file ended here)'}`,
            `actual    ${failure.actual ?? '(nothing)'}`,
            '',
            'Values at this step',
            groupPins(failure.state),
        ].join('\n');
        const message = new vscode.TestMessage(text);
        message.location = new vscode.Location(uri, toRange(failure.range));
        messages.push(message);
    }
    return messages;
}

/** The text of a file as the student sees it: an open editor's, saved or not, else the disk's. */
export async function textOf(file: string): Promise<string | null> {
    const open = vscode.workspace.textDocuments.find((document) => document.uri.scheme === 'file' && samePath(document.uri.fsPath, file));
    if (open) {
        return open.getText();
    }
    try {
        return await fs.promises.readFile(file, 'utf8');
    } catch {
        return null;
    }
}

/**
 * Source files (`.asm` or `.jack`) loaded by `script` that should be built in
 * memory even when a `.hack` or `.vm` artifact already exists on disk:
 * - the explicit `.asm`/`.jack` source file(s) the student ran or saved;
 * - any source file present in the captured unsaved `overlay`;
 * - any source file whose modification time is newer than the actual loaded
 *   artifact (or tied on a coarse-timestamp filesystem after a recent source save).
 *
 * Launching from a single `.jack` file only prefers THAT `.jack` file (plus
 * dirty/newer siblings), never forcing unrelated untouched `.jack` files over
 * valid `.vm` artifacts. When the built `.hack` or `.vm` artifact is newer than
 * an untouched source (project 6/11 own tool outputs), the source is NOT listed.
 */
export async function preferredSources(
    script: vscode.Uri,
    explicitSource?: vscode.Uri | vscode.Uri[],
    overlay?: { path: string; text: string; ignoreCase: boolean }[],
    savedFileAt?: Map<string, number>,
): Promise<string[]> {
    const scriptDir = path.dirname(script.fsPath);
    const scriptText = (await textOf(script.fsPath)) ?? '';
    const parsed = parseScriptLinks(scriptText);

    const dirty = new Set(
        overlay
            ? overlay.map((buf) => pathKey(buf.path, buf.ignoreCase))
            : vscode.workspace.textDocuments
                  .filter((doc) => doc.isDirty && doc.uri.scheme === 'file')
                  .map((doc) => pathKey(doc.uri.fsPath)),
    );
    const explicitList = explicitSource ? (Array.isArray(explicitSource) ? explicitSource : [explicitSource]) : [];
    const explicitKeys = new Set(
        explicitList
            .filter((u) => /\.(asm|jack)$/i.test(u.fsPath))
            .map((u) => pathKey(u.fsPath)),
    );

    const preferred: string[] = [];
    const added = new Set<string>();
    const addPreferred = (filePath: string) => {
        const key = pathKey(filePath);
        if (!added.has(key)) {
            added.add(key);
            preferred.push(filePath);
        }
    };

    const isSourceFresher = async (sourcePath: string, artifactPath: string): Promise<boolean> => {
        try {
            const [sourceStat, artifactStat] = await Promise.all([
                fs.promises.stat(sourcePath),
                fs.promises.stat(artifactPath),
            ]);
            if (sourceStat.mtimeMs > artifactStat.mtimeMs) {
                return true;
            }
            if (sourceStat.mtimeMs === artifactStat.mtimeMs && savedFileAt) {
                const sourceSaved = savedFileAt.get(pathKey(sourcePath)) ?? 0;
                const artifactSaved = savedFileAt.get(pathKey(artifactPath)) ?? 0;
                if (sourceSaved > artifactSaved) {
                    return true;
                }
            }
        } catch {
            // Unreadable or missing artifact: leave missing-artifact fallback to the engine.
        }
        return false;
    };

    const checkDir = async (dir: string, explicitArtifactByStem?: Map<string, string>, allowAsm = true) => {
        let names: string[];
        try {
            names = await fs.promises.readdir(dir);
        } catch {
            return;
        }
        const ignoreCase = ignoresCase(path.join(dir, names[0] ?? 'x'));
        for (const name of names) {
            const isAsm = allowAsm && /\.asm$/i.test(name);
            const isJack = /\.jack$/i.test(name);
            if (!isAsm && !isJack) {
                continue;
            }
            const full = path.join(dir, name);
            const key = pathKey(full, ignoreCase);
            if (explicitKeys.has(key) || dirty.has(key)) {
                addPreferred(full);
                continue;
            }
            const stem = name.replace(/\.[^.]*$/, '');
            const artifactExt = isAsm ? 'hack' : 'vm';
            const mappedArtifact = explicitArtifactByStem?.get(
                (isAsm || ignoreCase) ? stem.toLowerCase() : stem,
            );
            const artifactName =
                mappedArtifact
                ?? findByStem(names, stem, artifactExt, ignoreCase, isAsm);
            if (!artifactName) {
                continue;
            }
            if (await isSourceFresher(full, path.join(dir, artifactName))) {
                addPreferred(full);
            }
        }
    };

    // Map explicit `load X.hack` / `load X.vm` targets so freshness is always
    // measured against the exact artifact the script actually loads.
    const explicitArtifacts = new Map<string, string>();
    const scriptIgnoreCase = ignoresCase(script.fsPath);
    for (const target of parsed.loads) {
        const dot = target.lastIndexOf('.');
        const ext = dot > 0 ? target.slice(dot + 1).toLowerCase() : '';
        if (ext === 'hack' || ext === 'vm') {
            const stem = target.slice(0, dot);
            const mapKey = (ext === 'hack' || scriptIgnoreCase) ? stem.toLowerCase() : stem;
            if (!explicitArtifacts.has(mapKey)) {
                explicitArtifacts.set(mapKey, target);
            }
        }
    }

    await checkDir(scriptDir, explicitArtifacts, true);
    for (const subFolder of parsed.loadFolders) {
        await checkDir(path.join(scriptDir, subFolder), undefined, false);
    }
    return preferred;
}

/**
 * The `.tst` scripts that test `file` (`.hdl`, `.asm`, `.hack`, `.vm` or
 * `.jack`): scripts in the same folder, plus (for `.vm`/`.jack`) scripts in
 * the parent folder that `load <thisFolder>`. Forever demonstration scripts
 * (such as `Fill.tst`) are omitted by default.
 */
export async function scriptsForFile(
    file: vscode.Uri,
    options: { includeForever?: boolean } = {},
): Promise<vscode.Uri[]> {
    const dir = path.dirname(file.fsPath);
    const fileName = path.basename(file.fsPath);
    const ignoreCase = ignoresCase(file.fsPath);
    const found: vscode.Uri[] = [];
    const seen = new Set<string>();
    const scanDir = async (scanPath: string, subfolderName?: string) => {
        let names: string[];
        try {
            names = await fs.promises.readdir(scanPath);
        } catch {
            return;
        }
        const entriesForCheck = subfolderName ? [] : names;
        for (const name of names.sort((a, b) => a.localeCompare(b))) {
            if (!/\.tst$/i.test(name)) {
                continue;
            }
            const script = path.join(scanPath, name);
            const key = pathKey(script);
            if (seen.has(key)) {
                continue;
            }
            const text = await textOf(script);
            if (text === null) {
                continue;
            }
            if (!options.includeForever && isForeverScript(text)) {
                continue;
            }
            if (scriptTestsFile(text, fileName, entriesForCheck, ignoreCase, subfolderName)) {
                seen.add(key);
                found.push(vscode.Uri.file(script));
            }
        }
    };
    await scanDir(dir);
    if (/\.(vm|jack)$/i.test(fileName)) {
        const parent = path.dirname(dir);
        const folderName = path.basename(dir);
        if (parent !== dir && folderName) {
            await scanDir(parent, folderName);
        }
    }
    return found;
}

/**
 * Ranks a file's test scripts consistently across ComputerPanel, Step Through
 * and Compare Output (current failure → readable `.cmp` → exact stem → name).
 */
export async function rankScriptsForFile(
    scripts: vscode.Uri[],
    file: vscode.Uri,
    tests: TestExplorer,
): Promise<vscode.Uri[]> {
    const candidates = await Promise.all(
        scripts.map(async (script) => {
            const st = tests.statusOf(script, file);
            const currentFailure = st.status === 'current' && st.result.kind === 'failed';
            const text = await textOf(script.fsPath);
            let hasCompare = false;
            if (text !== null) {
                const { comparePath } = compareFilesForScript(text, script.fsPath);
                if (comparePath) {
                    try {
                        const stat = await fs.promises.stat(comparePath);
                        hasCompare = stat.isFile();
                    } catch {
                        // Compare file absent.
                    }
                }
            }
            return {
                item: script,
                scriptName: path.basename(script.fsPath),
                currentFailure,
                hasCompare,
            };
        }),
    );
    return orderScriptCandidates(candidates, path.basename(file.fsPath));
}

/**
 * The scripts that use `file` when saved: the file itself if it is a script,
 * scripts in the same folder that test/compare it, plus (for `.vm`/`.jack`)
 * scripts in the parent folder that `load <thisFolder>`.
 */
export async function scriptsUsing(file: vscode.Uri): Promise<vscode.Uri[]> {
    const dir = path.dirname(file.fsPath);
    const savedName = path.basename(file.fsPath);
    const ignoreCase = ignoresCase(file.fsPath);
    const found: vscode.Uri[] = [];
    const seen = new Set<string>();
    const scanDir = async (scanPath: string, subfolderName?: string) => {
        let names: string[];
        try {
            names = await fs.promises.readdir(scanPath);
        } catch {
            return;
        }
        const entriesForCheck = subfolderName ? [] : names;
        for (const name of names.sort()) {
            if (!/\.tst$/i.test(name)) {
                continue;
            }
            const script = path.join(scanPath, name);
            const key = pathKey(script);
            if (seen.has(key)) {
                continue;
            }
            const text = await textOf(script);
            if (text !== null && scriptUsesSavedFile(text, name, savedName, entriesForCheck, ignoreCase, subfolderName)) {
                seen.add(key);
                found.push(vscode.Uri.file(script));
            }
        }
    };
    await scanDir(dir);
    if (/\.(vm|jack)$/i.test(savedName)) {
        const parent = path.dirname(dir);
        const folderName = path.basename(dir);
        if (parent !== dir && folderName) {
            await scanDir(parent, folderName);
        }
    }
    return found;
}

/** The engine files beside a file, for telling whether a saved result still holds. Null when the file is gone. */
function listFolder(file: string): FolderListing {
    try {
        if (!fs.statSync(file).isFile()) {
            return null;
        }
        const folder = path.dirname(file);
        return fs
            .readdirSync(folder)
            .filter((name) => ENGINE_FILE.test(name))
            .map((name) => ({ name, mtimeMs: fs.statSync(path.join(folder, name)).mtimeMs }));
    } catch {
        return null;
    }
}
