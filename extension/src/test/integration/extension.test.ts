// Integration tests: the real extension in a real VS Code.
//
// The workspace is a scratch copy of `tests/fixtures`, in a folder with a
// space in its path. On Windows CI this is the only automated check of the
// platform the extension's first user is on.

import assert from 'node:assert/strict';
import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';
import * as vscode from 'vscode';
import type { NandwrightApi } from '../../extension';
import { samePath } from '../../paths';
import type { ScriptResult } from '../../testing';
import { CircuitPanel, type CircuitPanels } from '../../circuitPanel';
import { ChipPanel } from '../../chipPanel';
import { Runtime } from '../../runtime/client';
import { ChipSessions, type HubEvent } from '../../chipSessions';
import { firstSession } from '../../runtime/protocol';
import type { TestExplorer } from '../../testing';
import { FakeRuntime } from '../fakeRuntime';

const EXTENSION_ID = 'nandwright.nandwright';
const workspace = process.env.NANDWRIGHT_TEST_WORKSPACE!;
const file = (...parts: string[]) => vscode.Uri.file(path.join(workspace, ...parts));

/** What a panel shows, for the message of a timeout. */
function shows(computer: { snapshot(): { state: { status: string; line: number | null; steps: number } | null; running: boolean; problem: string | null } }): () => string {
    return () => {
        const { state, running, problem } = computer.snapshot();
        return `the panel showed status=${state?.status}, line=${state?.line}, steps=${state?.steps}, running=${running}, problem=${problem}`;
    };
}

async function until<T>(what: string, probe: () => T | undefined | false | Promise<T | undefined | false>, timeoutMs = 20_000, describe?: () => string): Promise<T> {
    const deadline = Date.now() + timeoutMs;
    for (;;) {
        const value = await probe();
        if (value) {
            return value;
        }
        if (Date.now() > deadline) {
            throw new Error(`timed out waiting for ${what}${describe ? `; ${describe()}` : ''}`);
        }
        await new Promise((resolve) => setTimeout(resolve, 50));
    }
}

async function api(): Promise<NandwrightApi> {
    const extension = vscode.extensions.getExtension<NandwrightApi>(EXTENSION_ID);
    assert.ok(extension, `${EXTENSION_ID} is not installed`);
    return extension.isActive ? extension.exports : extension.activate();
}

async function open(...parts: string[]): Promise<vscode.TextEditor> {
    const document = await vscode.workspace.openTextDocument(file(...parts));
    return vscode.window.showTextDocument(document, { preview: false });
}

function diagnosticsOf(uri: vscode.Uri): vscode.Diagnostic[] {
    return vscode.languages.getDiagnostics(uri);
}

async function replaceAll(editor: vscode.TextEditor, text: string): Promise<void> {
    const whole = new vscode.Range(0, 0, editor.document.lineCount, 0);
    assert.ok(await editor.edit((edit) => edit.replace(whole, text)));
}

/** Removes a scratch folder; on Windows a folder just watched or edited in can stay locked for a moment, and a leftover is not a failure. */
async function removeFolder(folder: string): Promise<void> {
    try {
        fs.rmSync(folder, { recursive: true, force: true, maxRetries: 10, retryDelay: 200 });
    } catch {
        // Left for the temp folder's own cleanup.
    }
}

/** A script that takes a few seconds, so that something can happen while it runs. */
const BUSY_SCRIPT = 'load Majority.hdl,\nrepeat 6000000 {\n    set a 1, eval;\n}\n';

/** A scratch folder inside the workspace holding copies of fixture files, given as `folder/name`. */
function scratchCopy(prefix: string, files: string[]): string {
    const folder = fs.mkdtempSync(path.join(workspace, prefix));
    for (const name of files) {
        fs.copyFileSync(file(...name.split('/')).fsPath, path.join(folder, path.basename(name)));
    }
    return folder;
}

async function discardEdits(): Promise<void> {
    for (const document of vscode.workspace.textDocuments) {
        if (document.isDirty) {
            await vscode.window.showTextDocument(document);
            await vscode.commands.executeCommand('workbench.action.files.revert');
        }
    }
}

async function closeAll(): Promise<void> {
    await vscode.commands.executeCommand('workbench.action.closeAllEditors');
}

describe('Nandwright', function () {
    before(async function () {
        this.timeout(120_000);
        const nandwright = await api();
        // The language server is a separate process: wait until it answers.
        await until('the language server', () => nandwright.client.isRunning(), 60_000);
    });

    afterEach(async () => {
        // Discard edits so one test's broken chip does not leak into the next.
        await discardEdits();
        await closeAll();
    });

    it('is the build under test and contributes the languages', async () => {
        const extension = vscode.extensions.getExtension(EXTENSION_ID)!;
        if (process.env.NANDWRIGHT_TEST_VSIX) {
            // Installed from the VSIX, not loaded from the source folder.
            assert.ok(!extension.extensionPath.includes('test-harness'));
            assert.ok(fs.existsSync(path.join(extension.extensionPath, 'dist', 'engine.wasm')), 'the VSIX has no engine');
            assert.ok(!fs.existsSync(path.join(extension.extensionPath, 'src')), 'the VSIX ships its sources');
        }
        const languages = await vscode.languages.getLanguages();
        for (const id of ['hdl', 'tst', 'cmp', 'hack-asm', 'hack', 'vm', 'jack']) {
            assert.ok(languages.includes(id), `language ${id} is missing`);
        }
        assert.equal((await open('chips', 'Majority.hdl')).document.languageId, 'hdl');
        assert.equal((await open('chips', 'Majority.tst')).document.languageId, 'tst');
        assert.equal((await open('chips', 'Majority.cmp')).document.languageId, 'cmp');
        const commands = await vscode.commands.getCommands(true);
        for (const command of ['runTest', 'debugTest', 'openChip', 'showCircuit', 'compareOutput', 'getStarted']) {
            assert.ok(commands.includes(`nandwright.${command}`), `command ${command} is missing`);
        }
    });

    describe('editing', () => {
        it('a correct chip has no problems', async () => {
            const editor = await open('chips', 'Majority.hdl');
            // Give the server time to publish; then there must be nothing.
            await new Promise((resolve) => setTimeout(resolve, 1500));
            assert.deepEqual(diagnosticsOf(editor.document.uri), []);
        });

        it('an unimplemented chip is a hint, never an error or warning', async () => {
            const editor = await open('runtime', 'Stub.hdl');
            const diagnostics = await until('the stub hint', () => {
                const found = diagnosticsOf(editor.document.uri);
                return found.length > 0 && found;
            });
            assert.ok(
                diagnostics.every((d) => d.severity === vscode.DiagnosticSeverity.Hint),
                JSON.stringify(diagnostics.map((d) => [d.severity, d.message])),
            );
        });

        it('reports a mistake where it is, as you type', async () => {
            const editor = await open('chips', 'Majority.hdl');
            await replaceAll(editor, editor.document.getText().replace('Or(a=ab, b=ac', 'Orr(a=ab, b=ac'));
            const diagnostic = await until('the unknown-chip error', () =>
                diagnosticsOf(editor.document.uri).find((d) => d.code === 'hdl-unknown-chip'),
            );
            assert.equal(diagnostic.severity, vscode.DiagnosticSeverity.Error);
            assert.equal(editor.document.getText(diagnostic.range), 'Orr');
        });

        it('offers hover and completion', async () => {
            const editor = await open('chips', 'Majority.hdl');
            const text = editor.document.getText();
            const at = editor.document.positionAt(text.indexOf('And(a=a, b=b') + 1);
            const hovers = await until('a hover', async () => {
                const found = await vscode.commands.executeCommand<vscode.Hover[]>('vscode.executeHoverProvider', editor.document.uri, at);
                return found.length > 0 && found;
            });
            const hoverText = hovers.flatMap((h) => h.contents).map((c) => (typeof c === 'string' ? c : c.value)).join('\n');
            assert.match(hoverText, /And/);
            const parts = editor.document.positionAt(text.indexOf('And(a=a, b=b'));
            const completions = await vscode.commands.executeCommand<vscode.CompletionList>(
                'vscode.executeCompletionItemProvider',
                editor.document.uri,
                parts,
            );
            const labels = completions.items.map((item) => (typeof item.label === 'string' ? item.label : item.label.label));
            assert.ok(labels.includes('Nand'), `no Nand in ${labels.slice(0, 20).join(', ')}`);
            assert.ok(labels.includes('Mux16'));
        });

        it('opens the interface of a built-in chip', async () => {
            const editor = await open('chips', 'Majority.hdl');
            const text = editor.document.getText();
            const at = editor.document.positionAt(text.indexOf('Or(a=ab') + 1);
            const locations = await until('a definition', async () => {
                const found = await vscode.commands.executeCommand<(vscode.Location | vscode.LocationLink)[]>(
                    'vscode.executeDefinitionProvider',
                    editor.document.uri,
                    at,
                );
                return found.length > 0 && found;
            });
            const first = locations[0];
            const uri = 'targetUri' in first ? first.targetUri : first.uri;
            assert.equal(uri.scheme, 'n2t-builtin');
            const document = await vscode.workspace.openTextDocument(uri);
            assert.match(document.getText(), /CHIP Or\b[\s\S]*BUILTIN Or;/);
        });

        it('formats without moving anything to another line', async () => {
            const editor = await open('chips', 'Majority.hdl');
            const messy = [
                'CHIP Majority{',
                'IN a // first',
                '  , b,c ;',
                '      OUT out;',
                'PARTS:',
                'And( a = a ,b=b, out=ab ); // one',
                '    And(a=a, b=c, out=ac);',
                'And(a=b, b=c, out=bc);',
                '  Or(a=ab, b=ac, out=abOrAc);',
                'Or(a=abOrAc, b=bc, out=out);',
                '}',
                '',
            ].join('\n');
            await replaceAll(editor, messy);
            const edits = await until('formatting edits', async () => {
                const found = await vscode.commands.executeCommand<vscode.TextEdit[] | undefined>(
                    'vscode.executeFormatDocumentProvider',
                    editor.document.uri,
                    { tabSize: 4, insertSpaces: true },
                );
                return found && found.length > 0 && found;
            });
            const workspaceEdit = new vscode.WorkspaceEdit();
            workspaceEdit.set(editor.document.uri, edits);
            assert.ok(await vscode.workspace.applyEdit(workspaceEdit));
            const formatted = editor.document.getText().replace(/\r\n/g, '\n');
            assert.equal(
                formatted,
                [
                    'CHIP Majority {',
                    '    IN a // first',
                    // A continuation line keeps its indentation relative to the
                    // line that started the statement.
                    '      , b, c;',
                    '    OUT out;',
                    '    PARTS:',
                    '    And(a=a, b=b, out=ab); // one',
                    '    And(a=a, b=c, out=ac);',
                    '    And(a=b, b=c, out=bc);',
                    '    Or(a=ab, b=ac, out=abOrAc);',
                    '    Or(a=abOrAc, b=bc, out=out);',
                    '}',
                    '',
                ].join('\n'),
            );
        });
    });

    describe('assembly and Jack', () => {
        it('shows the machine code of an instruction and flags a mistake', async () => {
            const editor = await open('programs', 'Countdown.asm');
            const text = editor.document.getText();
            const at = editor.document.positionAt(text.indexOf('D=A') + 1);
            const hovers = await until('a hover', async () => {
                const found = await vscode.commands.executeCommand<vscode.Hover[]>('vscode.executeHoverProvider', editor.document.uri, at);
                return found.length > 0 && found;
            });
            const hoverText = hovers.flatMap((h) => h.contents).map((c) => (typeof c === 'string' ? c : c.value)).join('\n');
            assert.match(hoverText, /1110110000010000/);
            await replaceAll(editor, text.replace('D;JLE', 'D;JXX'));
            const diagnostic = await until('the assembly error', () =>
                diagnosticsOf(editor.document.uri).find((d) => d.severity === vscode.DiagnosticSeverity.Error),
            );
            assert.equal(diagnostic.range.start.line, editor.document.positionAt(text.indexOf('D;JLE')).line);
        });

        it('checks a Jack class against the other classes of its folder', async () => {
            const editor = await open('jack', 'Main.jack');
            await new Promise((resolve) => setTimeout(resolve, 1500));
            assert.deepEqual(
                diagnosticsOf(editor.document.uri).filter((d) => d.severity <= vscode.DiagnosticSeverity.Warning),
                [],
            );
            const text = editor.document.getText();
            const call = /Counter\.(\w+)\(/.exec(text);
            assert.ok(call, 'the fixture calls Counter');
            await replaceAll(editor, text.replace(`Counter.${call[1]}(`, 'Counter.noSuchThing('));
            const diagnostic = await until('the unknown-subroutine error', () =>
                diagnosticsOf(editor.document.uri).find((d) => d.code === 'jack-unknown-subroutine'),
            );
            assert.equal(diagnostic.severity, vscode.DiagnosticSeverity.Error);
        });
    });

    describe('testing', () => {
        const run = async (...scripts: vscode.Uri[]): Promise<ScriptResult[]> => (await api()).tests.runScripts(scripts);

        it('finds every script in the workspace', async () => {
            const nandwright = await api();
            await nandwright.tests.discover();
            const ids: string[] = [];
            const visit = (item: vscode.TestItem) => {
                ids.push(item.id);
                item.children.forEach(visit);
            };
            nandwright.tests.controller.items.forEach(visit);
            for (const script of [file('chips', 'Majority.tst'), file('runtime', 'Stub.tst'), file('programs', 'Countdown.tst')]) {
                assert.ok(ids.includes(script.toString()), `${script.fsPath} was not discovered`);
            }
        });

        it('passes the fixture chips and writes their output files', async () => {
            const names = ['Accumulator', 'KeyLatch', 'Majority', 'Parity4', 'Scratchpad', 'SwapBytes'];
            const results = await run(...names.map((name) => file('chips', `${name}.tst`)));
            assert.deepEqual(
                results.map((r) => [path.basename(r.uri.fsPath), r.outcome.kind]),
                names.map((name) => [`${name}.tst`, 'passed']),
            );
            const out = fs.readFileSync(file('chips', 'Majority.out').fsPath, 'utf8');
            const cmp = fs.readFileSync(file('chips', 'Majority.cmp').fsPath, 'utf8');
            assert.deepEqual(out.split(/\r?\n/), cmp.split(/\r?\n/));
            // The official tools write the platform's line endings; so do we.
            assert.equal(out.includes('\r\n'), os.EOL === '\r\n');
            // The keyboard test was run by pressing the key it waits for.
            assert.deepEqual(results.find((r) => r.uri.fsPath.endsWith('KeyLatch.tst'))!.keysPressed, [75]);
        });

        it('runs a CPU emulator script', async () => {
            const [result] = await run(file('programs', 'Countdown.tst'));
            assert.equal(result.outcome.kind, 'passed', JSON.stringify(result.outcome));
        });

        it('fails an unimplemented chip on comparison, like the official simulator', async () => {
            const [result] = await run(file('runtime', 'Stub.tst'));
            assert.equal(result.outcome.kind, 'failed');
            assert.equal(result.outcome.kind === 'failed' && result.outcome.line, 3);
            const out = fs.readFileSync(file('runtime', 'Stub.out').fsPath, 'utf8');
            assert.equal(out.trimEnd().split(/\r?\n/).length, 3, 'the output file ends at the failing line');
        });

        it('reports every failing row with the internal pins, from unsaved text', async () => {
            const editor = await open('chips', 'Majority.hdl');
            const original = editor.document.getText();
            await replaceAll(editor, original.replace('And(a=a, b=c, out=ac);', 'Or(a=a, b=c, out=ac);'));
            assert.ok(editor.document.isDirty);
            const [result] = await run(file('chips', 'Majority.tst'));
            assert.equal(result.outcome.kind, 'failed', JSON.stringify(result.outcome));
            assert.equal(result.failures.length, 2);
            const ac = result.failures[0].state.find((pin) => pin.name === 'ac');
            assert.deepEqual([ac?.role, ac?.bits], ['internal', 1]);
            // The file on disk was not touched.
            assert.ok(fs.readFileSync(editor.document.uri.fsPath, 'utf8').includes('And(a=a, b=c, out=ac);'));
            await replaceAll(editor, original);
            const [fixed] = await run(file('chips', 'Majority.tst'));
            assert.equal(fixed.outcome.kind, 'passed');
        });

        it('explains errors instead of failing silently', async () => {
            const results = await run(file('runtime', 'MissingChip.tst'), file('runtime', 'InfiniteLoop.tst'));
            // A script that repeats forever is skipped with an explanation, not run until a limit.
            assert.equal(results.find((r) => r.uri.fsPath.endsWith('InfiniteLoop.tst'))!.outcome.kind, 'skipped');
            const missing = results.find((r) => r.uri.fsPath.endsWith('MissingChip.tst'))!;
            assert.ok(missing.outcome.kind === 'error' && /cannot load/.test(missing.outcome.message), JSON.stringify(missing.outcome));
        });

        it('shows the result of the last run above the chip, beside Run test', async () => {
            const nandwright = await api();
            const folder = scratchCopy('lens-', ['chips/Majority.hdl', 'chips/Majority.tst', 'chips/Majority.cmp']);
            try {
                const chip = vscode.Uri.file(path.join(folder, 'Majority.hdl'));
                const editor = await vscode.window.showTextDocument(await vscode.workspace.openTextDocument(chip), { preview: false });
                const titles = async () =>
                    ((await vscode.commands.executeCommand<vscode.CodeLens[]>('vscode.executeCodeLensProvider', chip)) ?? []).map((lens) => lens.command?.title ?? '');
                assert.deepEqual(await titles(), ['$(play) Run test', '$(debug-alt) Step through', '$(circuit-board) Open in simulator']);
                await vscode.commands.executeCommand('nandwright.runTest', chip);
                assert.equal((await titles())[0], '$(pass) Test passes');
                await replaceAll(editor, editor.document.getText().replace('And(a=a, b=c, out=ac);', 'Or(a=a, b=c, out=ac);'));
                await until('the result to go out of date', () => nandwright.tests.statusOf(vscode.Uri.file(path.join(folder, 'Majority.tst'))).status === 'stale');
                assert.equal((await titles())[0], '$(play) Run test');
                await vscode.commands.executeCommand('nandwright.runTest', chip);
                assert.equal((await titles())[0], '$(error) Test fails: 2 of 8 rows differ');
            } finally {
                await discardEdits();
                await closeAll();
                await removeFolder(folder);
            }
        });

        it('runs the test of the chip in the editor', async () => {
            await open('chips', 'Parity4.hdl');
            const results = await vscode.commands.executeCommand<ScriptResult[]>('nandwright.runTest');
            assert.deepEqual(
                results.map((r) => [path.basename(r.uri.fsPath), r.outcome.kind]),
                [['Parity4.tst', 'passed']],
            );
        });
    });

    describe('tests on save', () => {
        const majority = ['chips/Majority.hdl', 'chips/Majority.tst', 'chips/Majority.cmp'];
        const statusOf = async (uri: vscode.Uri) => (await api()).tests.statusOf(uri);

        it('runs the tests of a saved chip quietly, and an edit makes the result stale', async () => {
            const folder = scratchCopy('on-save-', majority);
            try {
                const chip = vscode.Uri.file(path.join(folder, 'Majority.hdl'));
                const script = vscode.Uri.file(path.join(folder, 'Majority.tst'));
                const editor = await vscode.window.showTextDocument(await vscode.workspace.openTextDocument(chip), { preview: false });
                assert.equal((await statusOf(script)).status, 'none');
                await replaceAll(editor, `${editor.document.getText()}\n// saved\n`);
                assert.ok(await editor.document.save());
                const status = await until('a current result', async () => {
                    const found = await statusOf(script);
                    return found.status === 'current' && found;
                }, 30_000);
                assert.equal(status.result?.kind, 'passed');
                // Nine lines: the header and eight rows.
                assert.equal(status.result?.comparedLines, 9);
                assert.equal(vscode.window.activeTextEditor?.document.uri.fsPath, chip.fsPath, 'the run took focus');
                await replaceAll(editor, editor.document.getText().replace('// saved', '// edited'));
                await until('the result to go stale', async () => (await statusOf(script)).status === 'stale');
            } finally {
                await discardEdits();
                await closeAll();
                await removeFolder(folder);
            }
        });

        it('runs nothing when the setting is off', async () => {
            const nandwright = await api();
            const settings = vscode.workspace.getConfiguration('nandwright.test');
            await settings.update('runOnSave', false, vscode.ConfigurationTarget.Global);
            const folder = scratchCopy('on-save-off-', majority);
            try {
                const chip = vscode.Uri.file(path.join(folder, 'Majority.hdl'));
                const editor = await vscode.window.showTextDocument(await vscode.workspace.openTextDocument(chip), { preview: false });
                await replaceAll(editor, `${editor.document.getText()}\n// saved\n`);
                assert.ok(await editor.document.save());
                await nandwright.tests.savedRunsSettled();
                assert.equal(nandwright.tests.statusOf(vscode.Uri.file(path.join(folder, 'Majority.tst'))).status, 'none');
            } finally {
                await settings.update('runOnSave', undefined, vscode.ConfigurationTarget.Global);
                await discardEdits();
                await closeAll();
                await removeFolder(folder);
            }
        });

        it('a newer save cancels the run still going and runs what it had not finished', async () => {
            const nandwright = await api();
            const folder = scratchCopy('on-save-twice-', majority);
            fs.writeFileSync(path.join(folder, 'Busy.tst'), BUSY_SCRIPT);
            try {
                const chip = vscode.Uri.file(path.join(folder, 'Majority.hdl'));
                const busy = vscode.Uri.file(path.join(folder, 'Busy.tst'));
                const script = vscode.Uri.file(path.join(folder, 'Majority.tst'));
                const editor = await vscode.window.showTextDocument(await vscode.workspace.openTextDocument(chip), { preview: false });
                const original = editor.document.getText();
                await replaceAll(editor, original.replace('And(a=a, b=c, out=ac);', 'Or(a=a, b=c, out=ac);'));
                assert.ok(await editor.document.save());
                // The first automatic run is under way (Busy.tst runs first and takes a while)...
                await until('the first automatic run', () => nandwright.tests.statusOf(busy).status === 'running', 30_000);
                await replaceAll(editor, original);
                const secondSave = Date.now();
                assert.ok(await editor.document.save());
                // ...and the second save cancels it: both scripts end with results of the second run.
                await until(
                    'current results of both scripts',
                    () => nandwright.tests.statusOf(script).status === 'current' && nandwright.tests.statusOf(busy).status === 'current',
                    90_000,
                    () => `Majority.tst ${nandwright.tests.statusOf(script).status}, Busy.tst ${nandwright.tests.statusOf(busy).status}`,
                );
                await nandwright.tests.savedRunsSettled();
                const status = nandwright.tests.statusOf(script);
                assert.equal(status.status, 'current');
                assert.equal(status.result?.kind, 'passed', JSON.stringify(status.result));
                assert.ok(nandwright.tests.statusOf(busy).result!.started >= secondSave, 'Busy.tst was not run again after the second save');
            } finally {
                await discardEdits();
                await closeAll();
                await removeFolder(folder);
            }
        });

        it('an edit made while a run is going makes the results of its later scripts stale', async () => {
            const nandwright = await api();
            const folder = scratchCopy('edit-during-run-', majority);
            fs.writeFileSync(path.join(folder, 'Busy.tst'), BUSY_SCRIPT);
            try {
                const chip = vscode.Uri.file(path.join(folder, 'Majority.hdl'));
                const busy = vscode.Uri.file(path.join(folder, 'Busy.tst'));
                const script = vscode.Uri.file(path.join(folder, 'Majority.tst'));
                const editor = await vscode.window.showTextDocument(await vscode.workspace.openTextDocument(chip), { preview: false });
                // Busy.tst runs first; Majority.tst starts only after the edit below.
                const run = nandwright.tests.runScripts([busy, script], { quiet: true });
                await until('Busy.tst to run', () => nandwright.tests.statusOf(busy).status === 'running', 30_000);
                await replaceAll(editor, editor.document.getText().replace('And(a=a, b=c, out=ac);', 'Or(a=a, b=c, out=ac);'));
                const results = await run;
                assert.equal(results.length, 2);
                // Whichever text Majority.tst saw, the edit came after the run began: its result is not current.
                assert.equal(nandwright.tests.statusOf(script).status, 'stale');
            } finally {
                await discardEdits();
                await closeAll();
                await removeFolder(folder);
            }
        });

        it('asks the engine about the script as it is now, not unsaved text from before a revert', async () => {
            const nandwright = await api();
            const folder = scratchCopy('forever-', majority);
            try {
                const script = vscode.Uri.file(path.join(folder, 'Majority.tst'));
                const editor = await vscode.window.showTextDocument(await vscode.workspace.openTextDocument(script), { preview: false });
                // (Not asked before the edit: that answer would be remembered for the file as saved.)
                await replaceAll(editor, 'load Majority.hdl,\nrepeat {\n    eval;\n}\n');
                assert.equal(await nandwright.tests.runsForever(script), true);
                await vscode.commands.executeCommand('workbench.action.files.revert');
                assert.equal(editor.document.isDirty, false);
                assert.equal(await nandwright.tests.runsForever(script), false);
            } finally {
                await discardEdits();
                await closeAll();
                await removeFolder(folder);
            }
        });

        it('finds a script that names the saved file in quotes', async () => {
            const nandwright = await api();
            const folder = scratchCopy('quoted-', majority);
            try {
                fs.copyFileSync(path.join(folder, 'Majority.cmp'), path.join(folder, 'Reference.cmp'));
                const tst = path.join(folder, 'Majority.tst');
                fs.writeFileSync(tst, fs.readFileSync(tst, 'utf8').replace('compare-to Majority.cmp', 'compare-to "Reference.cmp"'));
                const script = vscode.Uri.file(tst);
                const reference = vscode.Uri.file(path.join(folder, 'Reference.cmp'));
                const editor = await vscode.window.showTextDocument(await vscode.workspace.openTextDocument(reference), { preview: false });
                await replaceAll(editor, `${editor.document.getText().trimEnd()}\n`);
                const saved = Date.now();
                assert.ok(await editor.document.save());
                await until(
                    'Majority.tst to run after Reference.cmp is saved',
                    () => nandwright.tests.statusOf(script).status === 'current' && nandwright.tests.statusOf(script).result!.started >= saved,
                    30_000,
                );
                assert.equal(nandwright.tests.statusOf(script).result?.kind, 'passed');
            } finally {
                await discardEdits();
                await closeAll();
                await removeFolder(folder);
            }
        });
    });

    describe('project progress', () => {
        it('counts the tests of the folder in the status bar and lists how each stands', async () => {
            const nandwright = await api();
            const folder = scratchCopy('progress-', [
                ...['Majority', 'Parity4'].flatMap((name) => ['hdl', 'tst', 'cmp'].map((ext) => `chips/${name}.${ext}`)),
                'runtime/Stub.hdl',
                'runtime/Stub.tst',
                'runtime/Stub.cmp',
                'runtime/InfiniteLoop.tst',
            ]);
            try {
                // A deliberately broken copy: Or where an And belongs.
                const majority = path.join(folder, 'Majority.hdl');
                fs.writeFileSync(majority, fs.readFileSync(majority, 'utf8').replace('And(a=a, b=c, out=ac);', 'Or(a=a, b=c, out=ac);'));
                await vscode.window.showTextDocument(vscode.Uri.file(path.join(folder, 'Parity4.hdl')), { preview: false });
                await until('the item for the folder', () => nandwright.progress.shown.visible && samePath(nandwright.progress.shown.folder ?? '', folder));
                await nandwright.progress.runAll(folder);
                await nandwright.progress.update();
                const progress = await nandwright.progress.of(folder);
                // InfiniteLoop.tst repeats forever: a demonstration, not counted.
                assert.deepEqual([progress.passed, progress.failed, progress.notRun, progress.total], [1, 2, 0, 3]);
                const expectedText = '$(beaker) 1 of 3 pass';
                await until('the item to show the results', () => nandwright.progress.shown.text === expectedText, 20_000, () => nandwright.progress.shown.text);
                assert.equal(
                    nandwright.progress.shown.tooltip,
                    `Nand2Tetris tests in ${path.basename(folder)}: 1 passing, 2 failing, 0 not run. Click for the list.`,
                );
                assert.deepEqual(
                    progress.entries.map((entry) => [entry.label, entry.description]),
                    [
                        ['$(error) Majority', '2 of 8 rows differ'],
                        ['$(dash) Stub', 'not started: the chip has no parts yet'],
                        ['$(pass) Parity4', 'passes'],
                        ['$(eye) InfiniteLoop', 'a demonstration to watch, not a test'],
                    ],
                );
                await nandwright.progress.open(progress.entries[0]);
                assert.ok(samePath(vscode.window.activeTextEditor?.document.uri.fsPath ?? '', majority), vscode.window.activeTextEditor?.document.uri.fsPath);
                // A script without a chip of its own in the folder opens itself.
                assert.ok(samePath(progress.entries[3].target.fsPath, path.join(folder, 'InfiniteLoop.tst')), progress.entries[3].target.fsPath);
                // An edit: that folder's results no longer count.
                const editor = vscode.window.activeTextEditor!;
                await replaceAll(editor, `${editor.document.getText()}\n`);
                await until('the results to go stale', () => nandwright.progress.shown.text === '$(beaker) 0 of 3 pass', 20_000, () => nandwright.progress.shown.text);
                // A file elsewhere, without tests beside it, hides the item.
                await vscode.window.showTextDocument(file('jack', 'Main.jack'), { preview: false });
                await until('the item to hide', () => !nandwright.progress.shown.visible);
            } finally {
                await discardEdits();
                await closeAll();
                await removeFolder(folder);
            }
        });
    });

    describe('zip project files', () => {
        it('offers the files of a folder, ticks the student\'s own work and writes the chosen ones', async () => {
            const nandwright = await api();
            const folder = fs.mkdtempSync(path.join(workspace, 'zip-'));
            const target = path.join(workspace, `${path.basename(folder)}.zip`);
            try {
                const write = (relative: string, text: string) => {
                    fs.mkdirSync(path.dirname(path.join(folder, relative)), { recursive: true });
                    fs.writeFileSync(path.join(folder, relative), text);
                };
                write('Mine.hdl', 'CHIP Mine { IN a; OUT out; PARTS: Not(in=a, out=out); }\n');
                write('Mine.tst', 'load Mine.hdl;\n');
                write('Mine.cmp', '| a |\n');
                write('Mine.out', '| a |\n');
                write('notes.txt', 'mine\n');
                write('asm/Prog.asm', '@0\n');
                write('asm/Prog.HACK', '0000000000000000\n');
                // A VM translator's output: next to its .vm file, or named
                // after a folder of .vm files. Hand-written .asm stays ticked.
                write('vm/Prog.vm', 'push constant 1\n');
                write('vm/Prog.asm', '@1\n');
                write('vm/Mine.asm', '@2\n');
                write('Calls/A.vm', 'push constant 1\n');
                write('Calls/Calls.ASM', '@1\n');
                write('Sheet.xlsx', 'sheet');
                write('.hidden', 'x');
                write('.idea/settings.xml', 'x');
                write('node_modules/x/index.js', 'x');
                write('build/y.txt', 'x');
                // Another program's bookkeeping: an open Excel sheet's lock
                // file, Windows folder settings, a Mac zip's extra folder.
                write('~$Grades.xlsx', 'x');
                write('Thumbs.db', 'x');
                write('asm/desktop.ini', 'x');
                write('__MACOSX/Mine.hdl', 'x');
                const { files, tooLarge } = await nandwright.zipProject.projectFiles(folder);
                assert.equal(tooLarge, false);
                assert.deepEqual(
                    files.map((f) => [f.path, f.picked, f.description ?? '']),
                    [
                        ['Calls/A.vm', false, 'generated'],
                        ['Calls/Calls.ASM', false, 'generated'],
                        ['Mine.cmp', false, 'test file from the course'],
                        ['Mine.hdl', true, ''],
                        ['Mine.out', false, 'generated'],
                        ['Mine.tst', false, 'test file from the course'],
                        ['Sheet.xlsx', true, ''],
                        ['asm/Prog.HACK', false, 'generated'],
                        ['asm/Prog.asm', true, ''],
                        ['notes.txt', true, ''],
                        ['vm/Mine.asm', true, ''],
                        ['vm/Prog.asm', false, 'generated'],
                        ['vm/Prog.vm', false, 'generated'],
                    ],
                );
                const chosen = files.filter((f) => f.picked).map((f) => f.path);
                await nandwright.zipProject.writeProjectZip(folder, chosen, target);
                const archive = fs.readFileSync(target);
                const end = archive.length - 22;
                assert.equal(archive.readUInt32LE(end), 0x06054b50);
                assert.equal(archive.readUInt16LE(end + 10), 5);
                for (const name of chosen) {
                    assert.ok(archive.includes(Buffer.from(name)), `${name} is not in the zip`);
                }
                assert.ok(!archive.includes(Buffer.from('Mine.tst')));

                // A file from another folder goes to the root of the zip; a
                // name already at the root is refused, whatever its case.
                const elsewhere = fs.mkdtempSync(path.join(workspace, 'zip-other-'));
                try {
                    const sheet = path.join(elsewhere, 'Grades.xlsx');
                    fs.writeFileSync(sheet, 'sheet');
                    fs.writeFileSync(path.join(elsewhere, 'NOTES.TXT'), 'other');
                    fs.writeFileSync(path.join(elsewhere, 'Prog.asm'), 'other');
                    const { added, refused } = await nandwright.zipProject.addedFiles(files, [
                        sheet,
                        path.join(elsewhere, 'NOTES.TXT'),
                        path.join(elsewhere, 'Prog.asm'),
                    ]);
                    assert.deepEqual(
                        added.map((f) => [f.path, f.picked, f.description]),
                        [
                            ['Grades.xlsx', true, 'added'],
                            ['Prog.asm', true, 'added'],
                        ],
                    );
                    assert.deepEqual(refused, ['NOTES.TXT']);
                    await nandwright.zipProject.writeProjectZip(folder, [...files.filter((f) => f.picked), added[0]], target);
                    const withSheet = fs.readFileSync(target);
                    assert.equal(withSheet.readUInt16LE(withSheet.length - 22 + 10), 6);
                    assert.ok(withSheet.includes(Buffer.from('Grades.xlsx')));
                    assert.ok(!withSheet.includes(Buffer.from(path.basename(elsewhere))), 'the zip holds the other folder');
                } finally {
                    await removeFolder(elsewhere);
                }
            } finally {
                await removeFolder(folder);
                fs.rmSync(target, { force: true });
            }
        });
    });

    describe('hand-in setup and course presets', () => {
        const zipConfig = () => vscode.workspace.getConfiguration('nandwright.zip');
        const clear = async () => {
            await vscode.workspace.getConfiguration('nandwright').update('course', undefined, vscode.ConfigurationTarget.Global);
            for (const key of ['familyName', 'firstName']) {
                await vscode.workspace.getConfiguration('nandwright.student').update(key, undefined, vscode.ConfigurationTarget.Global);
            }
            for (const key of ['fileName', 'projects', 'gradeSheetFolder']) {
                await zipConfig().update(key, undefined, vscode.ConfigurationTarget.Global);
                await zipConfig().update(key, undefined, vscode.ConfigurationTarget.Workspace);
            }
        };
        const write = (relative: string, text: string) => {
            fs.mkdirSync(path.dirname(path.join(workspace, relative)), { recursive: true });
            fs.writeFileSync(path.join(workspace, relative), text);
        };
        const picked = (files: { path: string; picked: boolean }[]) => files.filter((f) => f.picked).map((f) => f.path);

        afterEach(clear);

        it('does not offer the setup during the tests', async () => {
            assert.equal((await api()).setup.offerShown, false);
        });

        it('with no course preset, zips a folder as before', async () => {
            const nandwright = await api();
            const folder = path.join(workspace, 'handin-plain', '01');
            write('handin-plain/01/Gate.hdl', 'CHIP Gate { IN a; OUT out; PARTS: }\n');
            write('handin-plain/01/Gate.tst', 'load Gate.hdl;\n');
            write('handin-plain/01/notes.txt', 'mine\n');
            try {
                const plan = await nandwright.zipProject.planZip(folder, nandwright.zipProject.zipSettings(folder), nandwright.zipProject.zipPlaces(folder));
                assert.equal(plan.title, 'Zip 01');
                assert.equal(plan.name, '01');
                assert.deepEqual(picked(plan.files), ['Gate.hdl', 'notes.txt']);
                assert.deepEqual(plan.notes, []);
            } finally {
                await removeFolder(path.join(workspace, 'handin-plain'));
            }
        });

        it('with the KTH EP121U preset, zips both folders of project 1 under its name with the grade sheet', async () => {
            const nandwright = await api();
            await vscode.workspace.getConfiguration('nandwright').update('course', 'kth-ep121u', vscode.ConfigurationTarget.Global);
            const student = vscode.workspace.getConfiguration('nandwright.student');
            await student.update('familyName', 'familyname', vscode.ConfigurationTarget.Global);
            await student.update('firstName', 'firstname', vscode.ConfigurationTarget.Global);
            write('handin-kth/01/Gate.hdl', 'CHIP Gate { IN a; OUT out; PARTS: }\n');
            write('handin-kth/01/Gate.tst', 'load Gate.hdl;\n');
            write('handin-kth/02/sub/Adder.hdl', 'CHIP Adder { IN a; OUT out; PARTS: }\n');
            write('handin-kth/02/notes.txt', 'mine\n');
            write('handin-kth/grade-sheets/Project1.xlsx', 'a made-up sheet');
            const target = path.join(workspace, 'handin-kth', 'out.zip');
            try {
                // A subfolder of a project folder: the project is zipped.
                const folder = nandwright.zipProject.handInFolder(path.join(workspace, 'handin-kth', '02', 'sub'));
                assert.equal(folder, path.join(workspace, 'handin-kth', '02'));
                const plan = await nandwright.zipProject.planZip(folder, nandwright.zipProject.zipSettings(folder), nandwright.zipProject.zipPlaces(folder));
                assert.equal(plan.title, 'Zip project 1 (01, 02)');
                assert.equal(plan.name, 'Project01-familyname-firstname');
                assert.deepEqual(picked(plan.files), ['Gate.hdl', 'sub/Adder.hdl', 'Project1.xlsx']);
                const sheet = plan.files.find((f) => f.path === 'Project1.xlsx');
                assert.match(sheet?.description ?? '', /^grade sheet \u00b7 handin-kth\/grade-sheets \u00b7 last changed \d{4}-/);
                await nandwright.zipProject.writeProjectZip(folder, plan.files.filter((f) => f.picked), target);
                const archive = fs.readFileSync(target);
                assert.equal(archive.readUInt16LE(archive.length - 22 + 10), 3);
                for (const name of ['Gate.hdl', 'sub/Adder.hdl', 'Project1.xlsx']) {
                    assert.ok(archive.includes(Buffer.from(name)), name);
                }
                assert.ok(!archive.includes(Buffer.from('notes.txt')));

                // The student's own setting wins over the preset.
                await zipConfig().update('fileName', 'Mine-{nn}-{folder}', vscode.ConfigurationTarget.Global);
                const own = await nandwright.zipProject.planZip(folder, nandwright.zipProject.zipSettings(folder), nandwright.zipProject.zipPlaces(folder));
                assert.equal(own.name, 'Mine-01-02');
            } finally {
                await removeFolder(path.join(workspace, 'handin-kth'));
            }
        });

        it('setup writes the course and name to the user settings and copies sheets without replacing one', async () => {
            const nandwright = await api();
            const downloads = fs.mkdtempSync(path.join(workspace, 'downloads-'));
            fs.writeFileSync(path.join(downloads, 'Project1.xlsx'), 'new 1');
            fs.writeFileSync(path.join(downloads, 'Project2.xlsx'), 'new 2');
            write('grade-sheets/Project1.xlsx', 'filled in');
            const asked: string[] = [];
            let message = '';
            const answers = ['kth-ep121u', 'copy', 'keep'];
            try {
                const set = await nandwright.setup.run({
                    pick: async (_title, choices) => {
                    const wanted = answers.shift();
                    return choices.find((c) => c.value === wanted);
                },
                    input: async (title) => (title.includes('family') ? 'familyname' : 'firstname'),
                    pickFiles: async () => [path.join(downloads, 'Project1.xlsx'), path.join(downloads, 'Project2.xlsx')],
                    pickFolder: async () => undefined,
                    confirmReplace: async (name) => {
                        asked.push(name);
                        return false;
                    },
                    done: (text) => {
                        message = text;
                    },
                });
                const course = vscode.workspace.getConfiguration('nandwright').inspect<string>('course');
                assert.equal(course?.globalValue, 'kth-ep121u');
                assert.equal(course?.workspaceValue, undefined);
                const family = vscode.workspace.getConfiguration('nandwright.student').inspect<string>('familyName');
                assert.equal(family?.globalValue, 'familyname');
                assert.equal(family?.workspaceValue, undefined);
                assert.deepEqual(asked, ['Project1.xlsx']);
                assert.equal(fs.readFileSync(path.join(workspace, 'grade-sheets', 'Project1.xlsx'), 'utf8'), 'filled in');
                assert.equal(fs.readFileSync(path.join(workspace, 'grade-sheets', 'Project2.xlsx'), 'utf8'), 'new 2');
                assert.ok(set.some((line) => line.includes('Project2.xlsx')));
                assert.match(message, /^Nandwright is set up: course: KTH EP121U/);
            } finally {
                await removeFolder(downloads);
                await removeFolder(path.join(workspace, 'grade-sheets'));
            }
        });

        it('setup stores a grade sheet folder inside the workspace as a relative workspace setting', async () => {
            const nandwright = await api();
            const answers = ['keep', 'folder', 'keep'];
            await nandwright.setup.run({
                pick: async (_title, choices) => {
                    const wanted = answers.shift();
                    return choices.find((c) => c.value === wanted);
                },
                input: async () => '',
                pickFiles: async () => [],
                pickFolder: async () => path.join(workspace, 'my sheets'),
                confirmReplace: async () => false,
                done: () => undefined,
            });
            const folder = zipConfig().inspect<string>('gradeSheetFolder');
            assert.equal(folder?.workspaceValue, 'my sheets');
            assert.equal(folder?.globalValue, undefined);
        });
    });

    describe('simulator panel', () => {
        it('shows a chip, takes inputs and builds the truth table', async () => {
            const nandwright = await api();
            const panel = await nandwright.chips.open(file('chips', 'Majority.hdl'));
            let { state, table, problem } = panel.snapshot();
            assert.equal(problem, null);
            assert.equal(state?.chip, 'Majority');
            assert.equal(state?.clocked, false);
            assert.equal(table?.rows.length, 8);
            await panel.simulate({ type: 'set', pin: 'a', value: 1 });
            await panel.simulate({ type: 'set', pin: 'c', value: '1' });
            ({ state } = panel.snapshot());
            const value = (name: string) => state!.pins.find((pin) => pin.name === name)?.bits;
            assert.deepEqual([value('out'), value('ac'), value('ab')], [1, 1, 0]);
            panel.panel.dispose();
        });

        it('clocks a sequential chip', async () => {
            const nandwright = await api();
            const panel = await nandwright.chips.open(file('chips', 'Accumulator.hdl'));
            assert.equal(panel.snapshot().state?.clocked, true);
            await panel.simulate({ type: 'set', pin: 'in', value: '0x10' });
            await panel.simulate({ type: 'set', pin: 'add', value: 1 });
            await panel.simulate({ type: 'action', action: 'tick' });
            assert.equal(panel.snapshot().state?.time, '0+');
            await panel.simulate({ type: 'action', action: 'tock' });
            await panel.simulate({ type: 'action', action: 'step' });
            const { state } = panel.snapshot();
            assert.equal(state?.time, '2');
            assert.equal(state?.pins.find((pin) => pin.name === 'out')?.bits, 32);
            panel.panel.dispose();
        });

        it('keeps a clock history of the steps taken in the panel', async () => {
            const nandwright = await api();
            const panel = await nandwright.chips.open(file('chips', 'Accumulator.hdl'));
            const steps = () => panel.snapshot().history.map((step) => [step.time, step.pins.find((pin) => pin.name === 'out')?.bits]);
            assert.deepEqual(steps(), []);
            await panel.simulate({ type: 'set', pin: 'in', value: 5 });
            await panel.simulate({ type: 'set', pin: 'add', value: 1 });
            await panel.simulate({ type: 'action', action: 'tick' });
            await panel.simulate({ type: 'action', action: 'tock' });
            await panel.simulate({ type: 'action', action: 'step' });
            assert.deepEqual(steps(), [
                ['0+', 0],
                ['1', 5],
                ['2', 10],
            ]);
            // Inputs and outputs only, as the panel shows them.
            assert.deepEqual(
                panel.snapshot().history[0].pins.map((pin) => [pin.name, pin.role]),
                [
                    ['in', 'input'],
                    ['add', 'input'],
                    ['reset', 'input'],
                    ['out', 'output'],
                ],
            );
            assert.equal(panel.snapshot().history[0].pins.find((pin) => pin.name === 'in')?.bits, 5);
            await panel.simulate({ type: 'action', action: 'reset' });
            assert.deepEqual(steps(), []);
            await panel.simulate({ type: 'action', action: 'step' });
            assert.equal(steps().length, 1);
            await panel.simulate({ type: 'clearHistory' });
            assert.deepEqual(steps(), []);
            // A chip without a clock has no history.
            await open('chips', 'Majority.hdl');
            await until('the panel to show Majority', () => panel.snapshot().state?.chip === 'Majority');
            await panel.settled();
            await panel.simulate({ type: 'set', pin: 'a', value: 1 });
            assert.deepEqual(panel.snapshot().history, []);
            panel.panel.dispose();
        });

        it('follows the editor as the chip is typed', async () => {
            const nandwright = await api();
            const editor = await open('chips', 'Parity4.hdl');
            const panel = await nandwright.chips.open(editor.document.uri);
            assert.equal(panel.snapshot().problem, null);
            const original = editor.document.getText();
            await replaceAll(editor, original.replace('Xor(a=low, b=high, out=out);', 'Xorr(a=low, b=high, out=out);'));
            await until('the panel to show the error', () => /unknown chip `Xorr`/.test(panel.snapshot().problem ?? ''));
            await replaceAll(editor, original);
            await until('the panel to recover', () => panel.snapshot().problem === null);
            panel.panel.dispose();
        });

        describe('expected output', () => {
            it('shows the compare file of the chip shown, and none for a chip without one', async () => {
                const nandwright = await api();
                const panel = await nandwright.chips.open(file('chips', 'Majority.hdl'));
                const majority = panel.snapshot().expected!;
                assert.equal(majority.name, 'Majority.cmp');
                assert.deepEqual(majority.header, ['a', 'b', 'c', 'out']);
                assert.equal(majority.rows.length, 8);
                assert.equal(majority.more, 0);
                await open('chips', 'Parity4.hdl');
                await until('the panel to show Parity4', () => panel.snapshot().state?.chip === 'Parity4');
                await panel.settled();
                assert.equal(panel.snapshot().expected?.name, 'Parity4.cmp');
                const folder = fs.mkdtempSync(path.join(os.tmpdir(), 'nandwright-expected-'));
                try {
                    fs.copyFileSync(file('chips', 'Majority.hdl').fsPath, path.join(folder, 'Majority.hdl'));
                    const alone = vscode.Uri.file(path.join(folder, 'Majority.hdl'));
                    await vscode.window.showTextDocument(await vscode.workspace.openTextDocument(alone), { preview: false });
                    await until('the panel to show the lone chip', () => nandwright.chips.get(alone) === panel);
                    await panel.settled();
                    assert.equal(panel.snapshot().expected, null);
                    // No script: no test result either.
                    assert.equal(panel.snapshot().test, null);
                } finally {
                    panel.panel.dispose();
                    await closeAll();
                    await removeFolder(folder);
                }
            });

            it('follows changes to the compare file and opens it on request', async () => {
                const nandwright = await api();
                const folder = fs.mkdtempSync(path.join(vscode.workspace.workspaceFolders![0].uri.fsPath, 'expected-'));
                try {
                    for (const extension of ['hdl', 'tst', 'cmp']) {
                        fs.copyFileSync(file('chips', `Majority.${extension}`).fsPath, path.join(folder, `Majority.${extension}`));
                    }
                    const chip = vscode.Uri.file(path.join(folder, 'Majority.hdl'));
                    const cmp = path.join(folder, 'Majority.cmp');
                    const panel = await nandwright.chips.open(chip);
                    assert.equal(panel.snapshot().expected?.rows.length, 8);
                    const lines = fs.readFileSync(cmp, 'utf8').split('\n');
                    fs.writeFileSync(cmp, lines.slice(0, 4).join('\n') + '\n');
                    await until('the panel to show the shorter file', () => panel.snapshot().expected?.rows.length === 3);
                    await panel.simulate({ type: 'openExpected' });
                    await until('the compare file in an editor', () =>
                        vscode.window.visibleTextEditors.some((editor) => editor.document.uri.fsPath === cmp),
                    );
                    panel.panel.dispose();
                } finally {
                    await closeAll();
                    await removeFolder(folder);
                }
            });
        });

        describe('the last test run', () => {
            it('runs the test from the panel and marks each row of the expected output', async () => {
                const nandwright = await api();
                const folder = scratchCopy('panel-test-', ['Majority', 'Parity4'].flatMap((name) => ['hdl', 'tst', 'cmp'].map((ext) => `chips/${name}.${ext}`)));
                try {
                    const chip = vscode.Uri.file(path.join(folder, 'Majority.hdl'));
                    const editor = await vscode.window.showTextDocument(await vscode.workspace.openTextDocument(chip), { preview: false });
                    const panel = await nandwright.chips.open(chip);
                    const testOf = () => panel.snapshot().test;
                    assert.deepEqual(testOf(), { script: 'Majority.tst', status: 'none' });
                    const current = (what: string) =>
                        until(what, () => testOf()?.status === 'current' && testOf()!, 30_000, () => `the panel showed ${JSON.stringify(testOf())}`);

                    await panel.simulate({ type: 'runTest' });
                    const passed = await current('the passing run');
                    assert.equal(passed.kind, 'passed');
                    assert.equal(passed.comparedLines, 9);
                    assert.deepEqual(passed.rows, Array(8).fill('match'));

                    // Or where an And belongs: rows 0 0 1 and 1 0 0 come out 1.
                    const original = editor.document.getText();
                    await replaceAll(editor, original.replace('And(a=a, b=c, out=ac);', 'Or(a=a, b=c, out=ac);'));
                    await until('the result to go stale', () => testOf()?.status === 'stale');
                    await panel.simulate({ type: 'runTest' });
                    const failed = await current('the failing run');
                    assert.equal(failed.kind, 'failed');
                    assert.equal(failed.failureCount, 2);
                    assert.equal(failed.comparedLines, 9);
                    assert.deepEqual(failed.rows, ['match', 'differ', 'match', 'match', 'differ', 'match', 'match', 'match']);
                    assert.deepEqual(Object.keys(failed.actual ?? {}), ['1', '4']);
                    assert.deepEqual(failed.actual![1].map((cell) => cell.trim()), ['0', '0', '1', '1']);
                    assert.deepEqual(failed.actual![4].map((cell) => cell.trim()), ['1', '0', '0', '1']);
                    // The cells are kept with their spacing: the line as the chip wrote it.
                    assert.equal(`|${failed.actual![1].join('|')}|`, '|   0   |   0   |   1   |   1   |');

                    await replaceAll(editor, original);
                    await until('the result to go stale again', () => testOf()?.status === 'stale');

                    await vscode.window.showTextDocument(await vscode.workspace.openTextDocument(vscode.Uri.file(path.join(folder, 'Parity4.hdl'))), {
                        preview: false,
                    });
                    await until('the panel to show Parity4', () => panel.snapshot().state?.chip === 'Parity4');
                    await panel.settled();
                    assert.deepEqual(testOf(), { script: 'Parity4.tst', status: 'none' });
                    panel.panel.dispose();
                } finally {
                    await discardEdits();
                    await closeAll();
                    await removeFolder(folder);
                }
            });
        });

        describe('the last test run, against the compare file the script names', () => {
            it('shows that file and marks each row by its line, across a blank line', async () => {
                const nandwright = await api();
                const folder = scratchCopy('panel-reference-', ['chips/Majority.hdl', 'chips/Majority.tst', 'chips/Majority.cmp']);
                try {
                    const cmp = fs.readFileSync(path.join(folder, 'Majority.cmp'), 'utf8').split(/\r?\n/);
                    // The rows the script compares with, with a blank line after the third row...
                    fs.writeFileSync(path.join(folder, 'Reference.cmp'), [...cmp.slice(0, 4), '', ...cmp.slice(4)].join('\n'));
                    // ...and a different Majority.cmp beside the chip, which the script does not use.
                    fs.writeFileSync(path.join(folder, 'Majority.cmp'), cmp.slice(0, 2).join('\n') + '\n');
                    const tst = path.join(folder, 'Majority.tst');
                    fs.writeFileSync(tst, fs.readFileSync(tst, 'utf8').replace('compare-to Majority.cmp', 'compare-to "Reference.cmp"'));
                    const panel = await nandwright.chips.open(vscode.Uri.file(path.join(folder, 'Majority.hdl')));
                    assert.equal(panel.snapshot().expected?.name, 'Reference.cmp');
                    await panel.simulate({ type: 'runTest' });
                    const test = await until(
                        'the run',
                        () => panel.snapshot().test?.status === 'current' && panel.snapshot().test!,
                        30_000,
                        () => JSON.stringify(panel.snapshot().test),
                    );
                    // Lines 2-4 match; the blank line 5 meets the chip's fourth row, so lines 6-9
                    // meet the rows after the ones they hold; line 10 is never reached.
                    assert.equal(test.kind, 'failed');
                    assert.deepEqual(test.rows, ['match', 'match', 'match', 'differ', 'differ', 'differ', 'differ', 'unreached']);
                    assert.deepEqual(test.actual![3].map((cell) => cell.trim()), ['1', '0', '0', '0']);
                    assert.deepEqual([test.differingRows, test.comparedRows], [4, 7]);
                    panel.panel.dispose();
                } finally {
                    await discardEdits();
                    await closeAll();
                    await removeFolder(folder);
                }
            });
        });

        describe('following the editor', () => {
            const chipOf = (panel: { snapshot(): { state: { chip: string } | null } }) => panel.snapshot().state?.chip;

            it('shows the chip of the .hdl editor and ignores other editors', async () => {
                const nandwright = await api();
                const a = file('chips', 'Majority.hdl');
                const b = file('chips', 'Parity4.hdl');
                const panel = await nandwright.chips.open(a);
                await open('chips', 'Parity4.hdl');
                await until('the panel to show Parity4', () => chipOf(panel) === 'Parity4', 20_000, () => `the panel showed ${chipOf(panel)}`);
                await panel.settled();
                assert.equal(panel.panel.title, 'Parity4 · simulator');
                assert.equal(nandwright.chips.get(b), panel);
                assert.equal(nandwright.chips.get(a), undefined);
                assert.equal(panel.snapshot().problem, null);
                await open('chips', 'Majority.tst');
                await new Promise((resolve) => setTimeout(resolve, 500));
                await panel.settled();
                assert.equal(chipOf(panel), 'Parity4');
                assert.equal(nandwright.chips.get(b), panel);
                panel.panel.dispose();
                assert.equal(nandwright.chips.get(b), undefined);
            });

            it('survives quick switching and ends on a working session of the last chip', async () => {
                const nandwright = await api();
                const panel = await nandwright.chips.open(file('chips', 'Majority.hdl'));
                for (const name of ['Parity4', 'Majority', 'Parity4']) {
                    await open('chips', `${name}.hdl`);
                }
                await until('the panel to show Parity4', () => chipOf(panel) === 'Parity4', 20_000, () => `the panel showed ${chipOf(panel)}`);
                await panel.settled();
                assert.equal(chipOf(panel), 'Parity4');
                assert.equal(nandwright.chips.get(file('chips', 'Parity4.hdl')), panel);
                // One bit set is odd parity: only Parity4's own session can answer this.
                await panel.simulate({ type: 'set', pin: 'in', value: 1 });
                assert.equal(panel.snapshot().state?.pins.find((pin) => pin.name === 'out')?.bits, 1);
                await panel.simulate({ type: 'set', pin: 'in', value: 3 });
                assert.equal(panel.snapshot().state?.pins.find((pin) => pin.name === 'out')?.bits, 0);
                assert.equal(panel.snapshot().problem, null);
                panel.panel.dispose();
            });

            it('retargets the one panel when the simulator is opened on another chip', async () => {
                const nandwright = await api();
                const panel = await nandwright.chips.open(file('chips', 'Majority.hdl'));
                const again = await nandwright.chips.open(file('chips', 'Parity4.hdl'));
                assert.equal(again, panel);
                assert.equal(chipOf(panel), 'Parity4');
                assert.equal(nandwright.chips.get(file('chips', 'Majority.hdl')), undefined);
                const back = (await vscode.commands.executeCommand('nandwright.openChip', file('chips', 'Majority.hdl'))) as typeof panel;
                assert.equal(back, panel);
                assert.equal(chipOf(panel), 'Majority');
                panel.panel.dispose();
            });

            it('stays on its chip, one panel per chip, when the setting is off', async () => {
                const nandwright = await api();
                const settings = vscode.workspace.getConfiguration('nandwright.simulator');
                await settings.update('followEditor', false, vscode.ConfigurationTarget.Global);
                try {
                    const first = await nandwright.chips.open(file('chips', 'Majority.hdl'));
                    await open('chips', 'Parity4.hdl');
                    await new Promise((resolve) => setTimeout(resolve, 500));
                    await first.settled();
                    assert.equal(chipOf(first), 'Majority');
                    const second = await nandwright.chips.open(file('chips', 'Parity4.hdl'));
                    assert.notEqual(second, first);
                    assert.equal(chipOf(first), 'Majority');
                    assert.equal(chipOf(second), 'Parity4');
                    assert.equal(nandwright.chips.get(file('chips', 'Majority.hdl')), first);
                    assert.equal(nandwright.chips.get(file('chips', 'Parity4.hdl')), second);
                    first.panel.dispose();
                    second.panel.dispose();
                } finally {
                    await settings.update('followEditor', undefined, vscode.ConfigurationTarget.Global);
                }
            });

            it('leaves nothing behind when the panel is closed while it switches', async () => {
                const nandwright = await api();
                const rejections: unknown[] = [];
                const onRejection = (reason: unknown) => rejections.push(reason);
                process.on('unhandledRejection', onRejection);
                try {
                    for (let i = 0; i < 5; i++) {
                        const panel = await nandwright.chips.open(file('chips', 'Majority.hdl'));
                        await open('chips', 'Parity4.hdl');
                        panel.panel.dispose();
                        await panel.settled();
                        await closeAll();
                    }
                    const panel = await nandwright.chips.open(file('chips', 'Majority.hdl'));
                    assert.equal(chipOf(panel), 'Majority');
                    await panel.simulate({ type: 'set', pin: 'a', value: 1 });
                    panel.panel.dispose();
                    await new Promise((resolve) => setTimeout(resolve, 500));
                    assert.deepEqual(rejections, []);
                } finally {
                    process.off('unhandledRejection', onRejection);
                }
            });
        });
    });

    describe('panels that are closed while they open', () => {
        it('leave nothing behind, and the next panel works', async () => {
            const nandwright = await api();
            const rejections: unknown[] = [];
            const onRejection = (reason: unknown) => rejections.push(reason);
            process.on('unhandledRejection', onRejection);
            try {
                const chip = file('chips', 'Majority.hdl');
                const program = file('programs', 'Countdown.asm');
                for (let i = 0; i < 5; i++) {
                    // The panel exists as soon as `open` is called; the load is still queued.
                    const opening = nandwright.chips.open(chip);
                    nandwright.chips.get(chip)!.panel.dispose();
                    await opening;
                    const starting = nandwright.computers.open(program);
                    nandwright.computers.get(program)!.panel.dispose();
                    await starting;
                }
                assert.equal(nandwright.chips.get(chip), undefined);
                const panel = await nandwright.chips.open(chip);
                const { state, table, problem } = panel.snapshot();
                assert.equal(problem, null);
                assert.equal(state?.chip, 'Majority');
                assert.equal(table?.rows.length, 8);
                await panel.simulate({ type: 'set', pin: 'a', value: 1 });
                await panel.simulate({ type: 'set', pin: 'b', value: 1 });
                assert.equal(panel.snapshot().state?.pins.find((pin) => pin.name === 'out')?.bits, 1);
                panel.panel.dispose();
                const computer = await nandwright.computers.open(program);
                assert.equal(computer.snapshot().problem, null);
                assert.equal(computer.snapshot().state?.steps, 0);
                computer.panel.dispose();
                // Give a late reply time to turn into a rejection if it is going to.
                await new Promise((resolve) => setTimeout(resolve, 500));
                assert.deepEqual(rejections, []);
            } finally {
                process.off('unhandledRejection', onRejection);
            }
        });
    });

    describe('circuit diagram', () => {
        const netOf = (panel: CircuitPanel, name: string) => {
            const { diagram, values } = panel.snapshot();
            const index = diagram!.nets.findIndex((net) => net.name === name);
            assert.ok(index >= 0, `no net ${name}`);
            return values!.nets[index];
        };
        const lastValues = (panel: CircuitPanel) => [...panel.posted].reverse().find((m) => m.type === 'values');

        it('Show Circuit Diagram opens the diagram of a chip', async () => {
            await open('chips', 'Veto.hdl');
            const panel = (await vscode.commands.executeCommand('nandwright.showCircuit')) as CircuitPanel;
            assert.ok(panel, 'the command returns the panel');
            const { diagram, problem, path: shown } = panel.snapshot();
            assert.equal(problem, null);
            assert.deepEqual(shown, []);
            assert.equal(diagram?.chip, 'Veto');
            assert.ok(samePath(diagram!.file!, file('chips', 'Veto.hdl').fsPath));
            assert.deepEqual(diagram!.parts.map((part) => part.chip), ['Majority', 'Not', 'And']);
            assert.equal(diagram!.parts[0].drill, true);
            assert.equal(panel.panel.title, 'Veto · circuit');
            // The webview gets the diagram once it is ready, and says when it has drawn it
            // (`posted` alone holds messages before the webview has them).
            await until('the webview to draw the diagram', () => panel.snapshot().drawn);
            const drawn = [...panel.posted].reverse().find((m) => m.type === 'diagram');
            assert.ok(drawn?.type === 'diagram' && drawn.token === panel.snapshot().token && drawn.diagram.chip === 'Veto');
            panel.panel.dispose();
        });

        it('follows HDL tabs, including a chip already displayed through drill-down', async () => {
            const nandwright = await api();
            const editor = await open('chips', 'Veto.hdl');
            const panel = await nandwright.circuits.open(editor.document.uri);
            await panel.simulate({ type: 'drill', path: [0] });
            assert.deepEqual(panel.snapshot().path, [0]);
            // Explicitly selecting the other file makes it the root of the diagram.
            for (const chip of ['Majority', 'Veto', 'Parity4']) {
                const document = await vscode.workspace.openTextDocument(file('chips', `${chip}.hdl`));
                await vscode.window.showTextDocument(document, { viewColumn: editor.viewColumn });
                await until(`the root diagram of ${chip}`, () => {
                    const shown = panel.snapshot();
                    return shown.diagram?.chip === chip && shown.path.length === 0;
                });
            }
            assert.equal(panel.panel.title, 'Parity4 · circuit');
            assert.equal(nandwright.circuits.get(file('chips', 'Parity4.hdl')), panel);
        });

        it('takes inputs and sends the new values', async () => {
            const nandwright = await api();
            const panel = await nandwright.circuits.open(file('chips', 'Veto.hdl'));
            assert.equal(netOf(panel, 'pass'), 0);
            await panel.simulate({ type: 'set', pin: 'a', value: 1 });
            await panel.simulate({ type: 'set', pin: 'b', value: 1 });
            assert.deepEqual([netOf(panel, 'vote'), netOf(panel, 'most'), netOf(panel, 'pass')], [1, 1, 1]);
            const sent = lastValues(panel);
            assert.ok(sent?.type === 'values' && sent.cause === 'input');
            await panel.simulate({ type: 'set', pin: 'veto', value: 1 });
            assert.deepEqual([netOf(panel, 'most'), netOf(panel, 'pass')], [1, 0]);
            panel.panel.dispose();
        });

        it('clocks a sequential chip', async () => {
            const nandwright = await api();
            const panel = await nandwright.circuits.open(file('chips', 'Accumulator.hdl'));
            await panel.simulate({ type: 'set', pin: 'in', value: 7 });
            await panel.simulate({ type: 'set', pin: 'add', value: 1 });
            await panel.simulate({ type: 'clock', action: 'step' });
            await panel.simulate({ type: 'clock', action: 'step' });
            assert.equal(panel.snapshot().values?.time, '2');
            assert.equal(netOf(panel, 'out'), 14);
            const sent = lastValues(panel);
            assert.ok(sent?.type === 'values' && sent.cause === 'clock');
            await panel.simulate({ type: 'clock', action: 'reset' });
            assert.equal(netOf(panel, 'out'), 0);
            panel.panel.dispose();
        });

        it('drills into a part and back up', async () => {
            const nandwright = await api();
            const panel = await nandwright.circuits.open(file('chips', 'Veto.hdl'));
            await panel.simulate({ type: 'set', pin: 'a', value: 1 });
            await panel.simulate({ type: 'set', pin: 'c', value: 1 });
            await panel.simulate({ type: 'drill', path: [0] });
            let { diagram } = panel.snapshot();
            assert.equal(diagram?.chip, 'Majority');
            assert.ok(samePath(diagram!.file!, file('chips', 'Majority.hdl').fsPath));
            assert.deepEqual(diagram!.breadcrumb.map((crumb) => crumb.chip), ['Veto', 'Majority']);
            // Live values from the same simulation.
            assert.deepEqual([netOf(panel, 'ac'), netOf(panel, 'out')], [1, 1]);
            // A built-in part cannot be opened: the panel stays at the nearest level that can.
            await panel.simulate({ type: 'drill', path: [0, 1] });
            assert.equal(panel.snapshot().diagram?.chip, 'Majority');
            await panel.simulate({ type: 'drill', path: diagram!.breadcrumb[0].path });
            ({ diagram } = panel.snapshot());
            assert.equal(diagram?.chip, 'Veto');
            assert.deepEqual(panel.snapshot().path, []);
            panel.panel.dispose();
        });

        it('reloads on edit, keeping the inputs and the drill path, and shows errors', async () => {
            const nandwright = await api();
            const editor = await open('chips', 'Veto.hdl');
            const panel = await nandwright.circuits.open(editor.document.uri);
            await panel.simulate({ type: 'set', pin: 'b', value: 1 });
            await panel.simulate({ type: 'set', pin: 'c', value: 1 });
            await panel.simulate({ type: 'drill', path: [0] });
            const original = editor.document.getText();
            await replaceAll(editor, original.replace('And(a=vote, b=allowed, out=pass);', 'Andd(a=vote, b=allowed, out=pass);'));
            await until('the panel to show the error', () => /unknown chip `Andd`/.test(panel.snapshot().problem ?? ''));
            assert.equal(panel.snapshot().diagram?.chip, 'Majority', 'the last diagram stays');
            assert.ok(panel.posted.some((m) => m.type === 'error'));
            await replaceAll(editor, original.replace('Not(in=veto, out=allowed);', 'Not(in=veto, out=allowed);\n    // checked'));
            await until('the panel to recover', () => panel.snapshot().problem === null);
            await panel.settled();
            assert.ok((panel.snapshot().diagram?.revision ?? 0) > 1, 'the engine counts the builds of the chip');
            assert.deepEqual(panel.snapshot().values?.path, [0]);
            assert.deepEqual(panel.snapshot().path, [0]);
            assert.equal(panel.snapshot().diagram?.chip, 'Majority');
            assert.equal(netOf(panel, 'bc'), 1, 'the inputs were kept');
            panel.panel.dispose();
        });

        it('shares one simulation with the simulator panel of the same chip', async () => {
            const nandwright = await api();
            const chip = await nandwright.chips.open(file('chips', 'Veto.hdl'));
            const circuit = await nandwright.circuits.open(file('chips', 'Veto.hdl'));
            const pin = (name: string) => chip.snapshot().state?.pins.find((p) => p.name === name)?.bits;
            // A flip in the simulator shows in the diagram...
            await chip.simulate({ type: 'set', pin: 'a', value: 1 });
            await chip.simulate({ type: 'set', pin: 'b', value: 1 });
            await until('the diagram to update', async () => {
                await circuit.settled();
                return netOf(circuit, 'pass') === 1;
            });
            const sent = lastValues(circuit);
            assert.ok(sent?.type === 'values' && sent.cause === 'external');
            // ...and one in the diagram shows in the simulator.
            await circuit.simulate({ type: 'set', pin: 'veto', value: 1 });
            await until('the simulator to update', async () => {
                await chip.settled();
                return pin('veto') === 1 && pin('pass') === 0;
            });
            chip.panel.dispose();
            // The diagram keeps the session when the simulator closes.
            await circuit.simulate({ type: 'set', pin: 'veto', value: 0 });
            assert.equal(netOf(circuit, 'pass'), 1);
            circuit.panel.dispose();
        });

        it('a click on a part or a wire selects its part statements', async () => {
            const nandwright = await api();
            const editor = await open('chips', 'Veto.hdl');
            const panel = await nandwright.circuits.open(editor.document.uri);
            const selected = (name: string) => {
                const shown = vscode.window.visibleTextEditors.find((e) => e.document.uri.fsPath.endsWith(name));
                return shown ? shown.document.getText(shown.selection) : undefined;
            };
            await panel.simulate({ type: 'reveal', part: 1 });
            await until('the Not statement to be selected', () => selected('Veto.hdl')?.startsWith('Not(in=veto, out=allowed)'));
            const vote = panel.snapshot().diagram!.nets.findIndex((net) => net.name === 'vote');
            await panel.simulate({ type: 'reveal', net: vote });
            await until('the first statement using vote to be selected', () => selected('Veto.hdl')?.startsWith('Majority(a=a'));
            // Drilled in, the part is in the other chip's file.
            await panel.simulate({ type: 'drill', path: [0] });
            await panel.simulate({ type: 'reveal', part: 3 });
            await until('the Or statement of Majority to be selected', () => selected('Majority.hdl')?.startsWith('Or(a=ab, b=ac, out=abOrAc)'));
            assert.deepEqual(panel.snapshot().path, [0]);
            panel.panel.dispose();
        });

        it('the editor cursor highlights its part and net', async () => {
            const nandwright = await api();
            const editor = await open('chips', 'Veto.hdl');
            const panel = await nandwright.circuits.open(editor.document.uri);
            const line = editor.document.getText().split('\n').findIndex((text) => text.includes('And(a=vote'));
            const column = editor.document.lineAt(line).text.indexOf('vote') + 1;
            editor.selection = new vscode.Selection(line, column, line, column);
            await until('a highlight', () => {
                const last = [...panel.posted].reverse().find((m) => m.type === 'highlight');
                return last?.type === 'highlight' && last.part === 2 && last.net === panel.snapshot().diagram!.nets.findIndex((n) => n.name === 'vote');
            });
            panel.panel.dispose();
        });

        describe('races', () => {
            it('a click sent about the old chip is not applied to the chip switched to', async () => {
                const nandwright = await api();
                const panel = await nandwright.circuits.open(file('chips', 'Veto.hdl'));
                const before = panel.snapshot().token;
                // The switch comes first; the click (on Veto's `a`, a pin Majority has too) after it.
                const switching = panel.showChip(file('chips', 'Majority.hdl'));
                const clicked = panel.simulate({ type: 'set', pin: 'a', value: 1, diagram: before });
                await Promise.all([switching, clicked]);
                assert.equal(panel.snapshot().diagram?.chip, 'Majority');
                assert.notEqual(panel.snapshot().token, before);
                assert.equal(netOf(panel, 'a'), 0, 'the click was dropped');
                // A click on the diagram shown works.
                await panel.simulate({ type: 'set', pin: 'a', value: 1, diagram: panel.snapshot().token });
                assert.equal(netOf(panel, 'a'), 1);
                panel.panel.dispose();
            });

            it('a step in the simulator taken just as the diagram switches chips does not reach the new chip', async () => {
                const nandwright = await api();
                const chip = await nandwright.chips.open(file('chips', 'Accumulator.hdl'));
                const circuit = await nandwright.circuits.open(file('chips', 'Accumulator.hdl'));
                await circuit.settled();
                const switching = circuit.showChip(file('chips', 'Majority.hdl'));
                const sent = circuit.posted.length;
                await chip.simulate({ type: 'action', action: 'step' });
                await switching;
                await circuit.settled();
                assert.equal(circuit.snapshot().diagram?.chip, 'Majority');
                const late = circuit.posted.slice(sent).filter((m) => m.type === 'values' && m.cause === 'external');
                assert.deepEqual(late, [], 'no values were fetched for the step on Accumulator');
                chip.panel.dispose();
                circuit.panel.dispose();
            });

            it('steps taken quickly in the diagram are each recorded in the simulator history, in order', async () => {
                const nandwright = await api();
                const chip = await nandwright.chips.open(file('chips', 'Accumulator.hdl'));
                const circuit = await nandwright.circuits.open(file('chips', 'Accumulator.hdl'));
                await circuit.simulate({ type: 'set', pin: 'in', value: 3 });
                await circuit.simulate({ type: 'set', pin: 'add', value: 1 });
                await Promise.all([circuit.simulate({ type: 'clock', action: 'step' }), circuit.simulate({ type: 'clock', action: 'step' })]);
                await chip.settled();
                const steps = chip.snapshot().history.map((step) => [step.time, step.pins.find((pin) => pin.name === 'out')?.bits]);
                assert.deepEqual(steps, [
                    ['1', 3],
                    ['2', 6],
                ]);
                chip.panel.dispose();
                circuit.panel.dispose();
            });

            it('R3: Reset then Step clicked quickly in the diagram: the step runs on the reset chip, not dropped', async () => {
                const nandwright = await api();
                const chip = await nandwright.chips.open(file('chips', 'Accumulator.hdl'));
                const circuit = await nandwright.circuits.open(file('chips', 'Accumulator.hdl'));
                await circuit.simulate({ type: 'set', pin: 'in', value: 7 });
                await circuit.simulate({ type: 'set', pin: 'add', value: 1 });
                await circuit.simulate({ type: 'clock', action: 'step' });
                await circuit.simulate({ type: 'clock', action: 'step' });
                assert.equal(netOf(circuit, 'out'), 14);
                // Both clicks are made before the reset has replaced the diagram.
                await Promise.all([circuit.simulate({ type: 'clock', action: 'reset' }), circuit.simulate({ type: 'clock', action: 'step' })]);
                await chip.settled();
                assert.equal(circuit.snapshot().values?.time, '1', 'one step after the reset');
                assert.equal(netOf(circuit, 'out'), 7);
                assert.equal(chip.snapshot().state?.time, '1', 'the simulator shows the same');
                chip.panel.dispose();
                circuit.panel.dispose();
            });

            it('R4: a Reset that fails leaves the error in both panels, also after a Step in either', async () => {
                const nandwright = await api();
                const editor = await open('chips', 'Accumulator.hdl');
                const chip = await nandwright.chips.open(editor.document.uri);
                const circuit = await nandwright.circuits.open(editor.document.uri);
                const original = editor.document.getText();
                try {
                    await replaceAll(editor, original.replace(/\bRegister\(/, 'Registerr('));
                    await until('both panels to show the error', async () => {
                        await Promise.all([chip.settled(), circuit.settled()]);
                        return /Registerr/.test(chip.snapshot().problem ?? '') && /Registerr/.test(circuit.snapshot().problem ?? '');
                    });
                    await circuit.simulate({ type: 'clock', action: 'reset' });
                    await chip.settled();
                    assert.match(chip.snapshot().problem ?? '', /Registerr/, 'the simulator still shows why the chip does not load');
                    assert.match(circuit.snapshot().problem ?? '', /Registerr/);
                    // The old circuit runs on, but a step does not make the error go away.
                    await chip.simulate({ type: 'action', action: 'step' });
                    await circuit.simulate({ type: 'clock', action: 'step' });
                    await chip.settled();
                    assert.equal(chip.snapshot().state?.time, '2', 'the old circuit stepped');
                    assert.match(chip.snapshot().problem ?? '', /Registerr/);
                    assert.match(circuit.snapshot().problem ?? '', /Registerr/);
                    // A build that works clears it in both.
                    await replaceAll(editor, original);
                    await until('both panels to recover', async () => {
                        await Promise.all([chip.settled(), circuit.settled()]);
                        return chip.snapshot().problem === null && circuit.snapshot().problem === null;
                    });
                } finally {
                    await replaceAll(editor, original);
                    await editor.document.save();
                    chip.panel.dispose();
                    circuit.panel.dispose();
                }
            });

            it('R6: a drilled part that becomes built-in: the diagram goes up to the deepest level that exists, not grey', async () => {
                const nandwright = await api();
                const editor = await open('chips', 'Veto.hdl');
                const chip = await nandwright.chips.open(editor.document.uri);
                const circuit = await nandwright.circuits.open(editor.document.uri);
                const original = editor.document.getText();
                try {
                    await circuit.simulate({ type: 'drill', path: [0] });
                    assert.equal(circuit.snapshot().diagram?.chip, 'Majority');
                    await replaceAll(editor, original.replace('Majority(a=a, b=b, c=c, out=vote, out=most);', 'Mux(a=a, b=b, sel=c, out=vote, out=most);'));
                    await until('the diagram to go up to Veto', async () => {
                        await Promise.all([chip.settled(), circuit.settled()]);
                        return circuit.snapshot().diagram?.chip === 'Veto' && circuit.snapshot().diagram?.parts[0].chip === 'Mux';
                    });
                    assert.deepEqual(circuit.snapshot().path, []);
                    assert.equal(circuit.snapshot().problem, null);
                    // A change in the simulator shows; nothing is greyed.
                    await chip.simulate({ type: 'set', pin: 'veto', value: 1 });
                    await circuit.settled();
                    assert.equal(circuit.snapshot().problem, null);
                    assert.equal(netOf(circuit, 'veto'), 1);
                } finally {
                    await replaceAll(editor, original);
                    await editor.document.save();
                    chip.panel.dispose();
                    circuit.panel.dispose();
                }
            });

            it('after the simulator restarts, an old diagram never reaches the session of another chip', async function () {
                const nandwright = await api();
                if (!nandwright.testHooks) {
                    this.skip();
                }
                const circuit = await nandwright.circuits.open(file('chips', 'Veto.hdl'));
                await circuit.simulate({ type: 'set', pin: 'a', value: 1 });
                await nandwright.testHooks!.crashWorker();
                // The new worker numbers its sessions from a range of its own.
                const probe = await nandwright.runtime.call<{ session: number }>({ op: 'chip.open', path: file('chips', 'Parity4.hdl').fsPath });
                assert.ok(probe.session >= firstSession(1, 0), `session ${probe.session} is in the new worker's range`);
                nandwright.runtime.close(probe.session);
                const chip = await nandwright.chips.open(file('chips', 'Majority.hdl'));
                await chip.simulate({ type: 'set', pin: 'a', value: 1 });
                // The diagram's hub opened its chip again ...
                await until('the diagram to reload Veto', async () => {
                    await circuit.settled();
                    return circuit.snapshot().problem === null && circuit.snapshot().diagram?.chip === 'Veto';
                });
                await circuit.simulate({ type: 'set', pin: 'veto', value: 1 });
                assert.equal(netOf(circuit, 'veto'), 1);
                // ... and closing it closes Veto's session, not Majority's.
                circuit.panel.dispose();
                await chip.simulate({ type: 'set', pin: 'b', value: 1 });
                const pin = (name: string) => chip.snapshot().state?.pins.find((p) => p.name === name)?.bits;
                assert.equal(pin('out'), 1, 'Majority is still running');
                chip.panel.dispose();
            });

            it('R6: after a worker crash both panels recover by themselves, and the diagram keeps its drill path', async function () {
                const nandwright = await api();
                if (!nandwright.testHooks) {
                    this.skip();
                }
                const chip = await nandwright.chips.open(file('chips', 'Veto.hdl'));
                const circuit = await nandwright.circuits.open(file('chips', 'Veto.hdl'));
                await circuit.simulate({ type: 'set', pin: 'a', value: 1 });
                await circuit.simulate({ type: 'drill', path: [0] });
                const before = circuit.snapshot().diagram;
                await nandwright.testHooks!.crashWorker();
                await until('the diagram to come back', async () => {
                    await Promise.all([chip.settled(), circuit.settled()]);
                    return circuit.snapshot().diagram !== before && circuit.snapshot().diagram?.chip === 'Majority';
                });
                assert.deepEqual(circuit.snapshot().path, [0], 'the drill path was kept');
                assert.equal(circuit.snapshot().problem, null);
                assert.equal(chip.snapshot().problem, null);
                assert.ok(chip.snapshot().state, 'the simulator shows the chip again');
                // One session again, shared; top-level inputs (a = 1) were restored on reopen.
                assert.equal(chip.snapshot().state?.pins.find((p) => p.name === 'a')?.bits, 1, 'input a restored after crash');
                await chip.simulate({ type: 'set', pin: 'b', value: 1 });
                await circuit.settled();
                assert.equal(netOf(circuit, 'out'), 1);
                chip.panel.dispose();
                circuit.panel.dispose();
            });

            it('R7: the crash hook exists only when the extension runs under test', async () => {
                const nandwright = await api();
                assert.equal((nandwright.runtime as unknown as Record<string, unknown>).crashForTests, undefined, 'no crash method on the runtime');
                if (process.env.NANDWRIGHT_TEST_VSIX) {
                    // The installed VSIX runs as a user's extension (ExtensionMode.Production).
                    assert.equal(nandwright.testHooks, undefined);
                } else {
                    assert.equal(typeof nandwright.testHooks?.crashWorker, 'function');
                }
            });
        });

        describe('the runtime client with a stand-in worker', () => {
            let folder: string;
            const output = { appendLine: () => undefined } as unknown as vscode.OutputChannel;
            /** An extension folder whose `dist/runtime.js` is `source`. */
            const withWorker = (source: string) => {
                folder = fs.mkdtempSync(path.join(os.tmpdir(), 'nandwright worker '));
                fs.mkdirSync(path.join(folder, 'dist'));
                fs.writeFileSync(path.join(folder, 'dist', 'runtime.js'), source);
                return folder;
            };
            afterEach(() => removeFolder(folder));

            it('a reply that says the engine was replaced (`crashed`) ends every session first, and panels reopen and redraw', async () => {
                const runtime = new Runtime(
                    withWorker(`
                        const { parentPort } = require('node:worker_threads');
                        let nextSession = 1;
                        let live = new Set();
                        let trapped = false;
                        const state = (session) => ({ chip: 'Fake', clocked: true, time: '0', revision: 1, pins: [{ name: 'a', role: 'input', width: 1, bits: 0 }] });
                        const diagram = () => ({
                            revision: 1, path: [], chip: 'Fake', file: null, breadcrumb: [],
                            inputs: [{ name: 'a', width: 1 }], outputs: [],
                            nets: [{ name: 'a', width: 1, kind: 'input' }], parts: [],
                            values: { revision: 1, path: [], time: '0', clocked: true, nets: [0], parts: [] },
                        });
                        parentPort.on('message', (m) => {
                            const req = m.request;
                            let reply;
                            if (req.op === 'overlay.replace' || req.op === 'close') {
                                reply = {};
                            } else if (req.op === 'chip.open') {
                                const session = nextSession++;
                                live.add(session);
                                reply = { ...state(session), session };
                            } else if (req.op === 'chip.step' && !trapped) {
                                trapped = true;
                                live.clear();
                                reply = { error: { message: 'trap', crashed: true } };
                            } else if (!live.has(req.session)) {
                                reply = { error: { message: 'this session has ended', code: 'session-ended' } };
                            } else if (req.op === 'chip.diagram') {
                                reply = diagram();
                            } else {
                                reply = state(req.session);
                            }
                            parentPort.postMessage({ type: 'reply', id: m.id, reply });
                        });`),
                    output,
                );
                const sessions = new ChipSessions(runtime);
                const fakeWebview = () => ({
                    onDidReceiveMessage: () => new vscode.Disposable(() => undefined),
                    postMessage: async () => true,
                });
                const fakePanel = () =>
                    ({ webview: fakeWebview(), onDidDispose: () => new vscode.Disposable(() => undefined), title: '', viewColumn: vscode.ViewColumn.One }) as unknown as vscode.WebviewPanel;
                const host = { settings: () => ({ animate: false, radix: 'dec' }), log: () => undefined } as unknown as CircuitPanels;
                const circuit = new CircuitPanel(file('chips', 'Veto.hdl'), fakePanel(), sessions, host, () => undefined);
                try {
                    await circuit.load();
                    const tokenBefore = circuit.snapshot().token;
                    const hub = sessions.hub(file('chips', 'Veto.hdl').fsPath)!;
                    const sessionBefore = hub.status().session;
                    assert.equal(sessionBefore, 1);

                    const order: string[] = [];
                    const sub = runtime.onDidStop(() => order.push('stop'));
                    const heard: string[] = [];
                    const spy = { hubEvent: (e: HubEvent) => heard.push(`${e.op}${e.outcome.ok ? '' : `!${e.outcome.error.code}`}`) };
                    sessions.join(file('chips', 'Veto.hdl').fsPath, spy);

                    const stepEvent = await hub.request(circuit, { op: 'step' });
                    order.push('reply');
                    sub.dispose();
                    await until('the panel to reopen and redraw after engine trap', async () => {
                        await circuit.settled();
                        return hub.status().session === 2 && circuit.snapshot().token > tokenBefore;
                    });
                    assert.deepEqual(order, ['stop', 'reply']);
                    assert.equal(!stepEvent!.outcome.ok && stepEvent!.outcome.error.code, 'stopped');
                    assert.ok(heard.includes('lost!stopped'), heard.join(', '));
                    assert.ok(heard.includes('open'), heard.join(', '));
                    assert.equal(hub.status().session, 2, 'reopened on a fresh session');
                    assert.equal(circuit.snapshot().problem, null);
                    assert.ok(circuit.snapshot().token > tokenBefore, 'diagram redrawn');
                    assert.ok(circuit.posted.some((m) => m.type === 'diagram' && m.token === circuit.snapshot().token));
                    sessions.leave(file('chips', 'Veto.hdl').fsPath, spy);
                } finally {
                    circuit.dispose();
                    sessions.dispose();
                    runtime.dispose();
                }
            });

            it('F2: a live worker that never replies times out, terminates, recovers, and respects the restart budget', async () => {
                // Worker 0 hangs on `chip.step`; worker 1 answers normally.
                const runtime = new Runtime(
                    withWorker(`
                        const { parentPort, workerData } = require('node:worker_threads');
                        let nextSession = (workerData.worker + 1) * 100;
                        const state = () => ({ chip: 'Fake', clocked: true, time: '1', revision: 1, pins: [] });
                        parentPort.on('message', (m) => {
                            if (m.type === 'runTest') {
                                setTimeout(() => parentPort.postMessage({ type: 'reply', id: m.id, reply: { state: 'finished', events: [], failures: [], commands: 0, echo: null } }), 250);
                                return;
                            }
                            const req = m.request;
                            if (workerData.worker === 0 && req.op === 'chip.step') {
                                return; // hang forever on worker 0
                            }
                            if (req.op === 'overlay.replace' || req.op === 'close') {
                                parentPort.postMessage({ type: 'reply', id: m.id, reply: {} });
                            } else if (req.op === 'chip.open') {
                                parentPort.postMessage({ type: 'reply', id: m.id, reply: { ...state(), session: nextSession++ } });
                            } else {
                                parentPort.postMessage({ type: 'reply', id: m.id, reply: state() });
                            }
                        });`),
                    output,
                    { callTimeoutMs: 120 },
                );
                const sessions = new ChipSessions(runtime);
                try {
                    const events: HubEvent[] = [];
                    const hub = sessions.join(file('chips', 'Veto.hdl').fsPath, { hubEvent: (e) => events.push(e) });
                    await hub.request(undefined, { op: 'state' });
                    assert.equal(hub.status().session, 100);
                    // A streaming test run is NOT killed by the single-call deadline.
                    const testStatus = await runtime.runTest(100, {}, () => undefined);
                    assert.equal(testStatus.state, 'finished');
                    // Now trigger the hang on worker 0: deadline fires, terminates worker 0, hub recovers on worker 1.
                    const hung = await hub.request(undefined, { op: 'step' });
                    assert.equal(!hung!.outcome.ok && hung!.outcome.error.code, 'stopped');
                    await until('recovery on worker 1', async () => {
                        await hub.whenIdle();
                        return hub.status().session === 200;
                    });
                    assert.ok(events.some((e) => e.op === 'lost'));
                    const recovered = await hub.request(undefined, { op: 'step' });
                    assert.ok(recovered?.outcome.ok, 'subsequent op succeeds on the fresh worker');
                } finally {
                    sessions.dispose();
                    runtime.dispose();
                }

                // A worker that always hangs exhausts the restart budget and halts.
                const spawns = path.join(os.tmpdir(), `nandwright-hang-${process.pid}-${Date.now()}.log`);
                const hungRuntime = new Runtime(
                    withWorker(`
                        require('node:fs').appendFileSync(${JSON.stringify(spawns)}, 'x');
                        const { parentPort } = require('node:worker_threads');
                        parentPort.on('message', (m) => {
                            if (m.request?.op === 'overlay.replace') {
                                parentPort.postMessage({ type: 'reply', id: m.id, reply: {} });
                            }
                            // never reply to chip.open
                        });`),
                    output,
                    { callTimeoutMs: 80 },
                );
                const hungSessions = new ChipSessions(hungRuntime);
                try {
                    const hub = hungSessions.join(file('chips', 'Veto.hdl').fsPath, { hubEvent: () => undefined });
                    void hub.request(undefined, { op: 'state' });
                    await until('hung worker to exhaust restart budget', () => hub.status().halted !== null);
                    await new Promise((resolve) => setTimeout(resolve, 300));
                    assert.equal(fs.readFileSync(spawns, 'utf8').length, 4, 'first start plus three automatic restarts');
                } finally {
                    hungSessions.dispose();
                    hungRuntime.dispose();
                    fs.rmSync(spawns, { force: true });
                }
            });

            it('R2: a worker that cannot start is started at most four times, then the panels wait for Retry', async () => {
                const spawns = path.join(os.tmpdir(), `nandwright-spawns-${process.pid}-${Date.now()}.log`);
                const runtime = new Runtime(
                    withWorker(`
                        require('node:fs').appendFileSync(${JSON.stringify(spawns)}, 'x');
                        throw new Error('engine.wasm is missing');`),
                    output,
                );
                const sessions = new ChipSessions(runtime);
                try {
                    const events: HubEvent[] = [];
                    const hub = sessions.join(file('chips', 'Veto.hdl').fsPath, { hubEvent: (event) => events.push(event) });
                    void hub.request(undefined, { op: 'state' });
                    await until('the hub to give up', () => hub.status().halted !== null);
                    await new Promise((resolve) => setTimeout(resolve, 1500));
                    assert.equal(fs.readFileSync(spawns, 'utf8').length, 4, 'the first start and three restarts');
                    assert.equal(events.filter((e) => e.op === 'halted').length, 1);
                    // Retry is one more start.
                    await sessions.retry(undefined);
                    await until('the hub to give up again', () => hub.status().halted !== null);
                    await new Promise((resolve) => setTimeout(resolve, 500));
                    assert.equal(fs.readFileSync(spawns, 'utf8').length, 5);
                } finally {
                    sessions.dispose();
                    runtime.dispose();
                    fs.rmSync(spawns, { force: true });
                }
            });
        });

        describe('panels on one hub, with a fake simulator', () => {
            const FAKE = file('chips', 'Fake.hdl');
            let fake: FakeRuntime;
            let sessions: ChipSessions;
            let circuit: CircuitPanel;
            let chip: ChipPanel;
            let chipMessages: unknown[];
            const webview = (log?: unknown[]) => ({
                onDidReceiveMessage: () => new vscode.Disposable(() => undefined),
                postMessage: async (message: unknown) => {
                    log?.push(message);
                    return true;
                },
            });
            const fakePanel = (log?: unknown[]) =>
                ({ webview: webview(log), onDidDispose: () => new vscode.Disposable(() => undefined), title: '', viewColumn: vscode.ViewColumn.One }) as unknown as vscode.WebviewPanel;
            const host = { settings: () => ({ animate: false, radix: 'dec' }), log: () => undefined } as unknown as CircuitPanels;
            const tests = {
                onDidChangeStatus: () => new vscode.Disposable(() => undefined),
                statusOf: () => ({ status: 'none' }),
                runScripts: async () => [],
            } as unknown as TestExplorer;
            /** A message from the webview, as it arrives: not waited for. Missing tokens default to the current view. */
            const send = (panel: CircuitPanel | ChipPanel, message: Record<string, unknown>) => {
                let filled = message;
                if (panel instanceof CircuitPanel) {
                    if (['set', 'drill', 'reveal'].includes(String(message.type)) && message.diagram === undefined) {
                        filled = { ...message, diagram: panel.snapshot().token };
                    } else if (message.type === 'clock' && message.target === undefined) {
                        filled = { ...message, target: panel.snapshot().target };
                    }
                } else {
                    if (['set', 'setAll'].includes(String(message.type))) {
                        filled = {
                            ...message,
                            ...(message.build === undefined ? { build: panel.snapshot().build } : {}),
                            ...(message.target === undefined ? { target: panel.snapshot().target } : {}),
                        };
                    } else if (message.type === 'action' && message.target === undefined) {
                        filled = { ...message, target: panel.snapshot().target };
                    }
                }
                return (panel as unknown as { receive(m: unknown): Promise<unknown> }).receive(filled);
            };
            const flush = () => new Promise((resolve) => setTimeout(resolve, 10));
            const settled = () => Promise.all([circuit.settled(), chip.settled()]);

            beforeEach(async () => {
                fake = new FakeRuntime();
                sessions = new ChipSessions(fake);
                chipMessages = [];
                circuit = new CircuitPanel(FAKE, fakePanel(), sessions, host, () => undefined);
                chip = new ChipPanel(FAKE, fakePanel(chipMessages), sessions, tests, () => undefined);
                await circuit.load();
                await chip.load();
                assert.equal(circuit.snapshot().diagram?.revision, 1);
                assert.equal(fake.count('chip.open'), 1, 'one session for both');
            });
            afterEach(() => {
                circuit.dispose();
                chip.dispose();
                sessions.dispose();
            });

            it('R1: a click on a diagram of build 1 that arrives after the simulator rebuilt the chip is not applied to build 2', async () => {
                const shown = circuit.snapshot().token;
                fake.hold('chip.diagram');
                await send(chip, { type: 'action', action: 'reset' });
                assert.equal(circuit.snapshot().token, shown, 'the new diagram has not come yet');
                const sets = fake.count('chip.set');
                const clicked = send(circuit, { type: 'set', pin: 'a', value: 1 });
                await flush();
                fake.release('chip.diagram');
                await clicked;
                await settled();
                assert.equal(fake.count('chip.set'), sets, 'the input was not set on build 2');
                assert.equal(fake.sessions.get(1)!.pins.get('a'), undefined);
                assert.ok(circuit.posted.some((m) => m.type === 'note' && /rebuilt/.test(m.message)), 'the student is told to click again');
                assert.equal(circuit.snapshot().diagram?.revision, 2);
                assert.equal(circuit.snapshot().problem, null, 'not an error');
                // A click on the new diagram works.
                await circuit.simulate({ type: 'set', pin: 'a', value: 1 });
                assert.equal(fake.sessions.get(1)!.pins.get('a'), 1);
            });

            it('R3: Reset then Step in the diagram: reset, then a step of the new build; the step is not dropped', async () => {
                fake.hold('chip.reset');
                const reset = send(circuit, { type: 'clock', action: 'reset' });
                const step = send(circuit, { type: 'clock', action: 'step' });
                await flush();
                fake.release('chip.reset');
                await Promise.all([reset, step]);
                await settled();
                assert.deepEqual(
                    fake.ops().filter((op) => op === 'chip.reset' || op === 'chip.step'),
                    ['chip.reset', 'chip.step'],
                );
                assert.equal(circuit.snapshot().diagram?.revision, 2);
                assert.equal(circuit.snapshot().values?.time, '1');
                assert.equal(chip.snapshot().state?.time, '1');
            });

            it('R4: the simulator records steps in the engine order when the diagram\'s values come late', async () => {
                fake.hold('chip.values');
                const first = send(circuit, { type: 'clock', action: 'step' });
                await flush();
                // The diagram's values request is waiting; the simulator steps meanwhile.
                const second = send(chip, { type: 'action', action: 'step' });
                await flush();
                fake.release('chip.values');
                await Promise.all([first, second]);
                await settled();
                assert.deepEqual(
                    chip.snapshot().history.map((step) => step.time),
                    ['1', '2'],
                );
                assert.equal(circuit.snapshot().values?.time, '2');
            });

            it('R4: a step in one panel does not clear the build error the other shows', async () => {
                fake.buildError = 'unknown chip `Nott`';
                await circuit.simulate({ type: 'clock', action: 'reset' });
                await settled();
                assert.match(chip.snapshot().problem ?? '', /Nott/);
                assert.match(circuit.snapshot().problem ?? '', /Nott/);
                await chip.simulate({ type: 'action', action: 'step' });
                await circuit.simulate({ type: 'clock', action: 'step' });
                await settled();
                assert.equal(chip.snapshot().state?.time, '2', 'the old circuit runs on');
                assert.match(chip.snapshot().problem ?? '', /Nott/, 'the simulator still shows it');
                assert.match(circuit.snapshot().problem ?? '', /Nott/, 'the diagram still shows it');
                fake.buildError = undefined;
                await chip.simulate({ type: 'action', action: 'reset' });
                await settled();
                assert.equal(chip.snapshot().problem, null);
                assert.equal(circuit.snapshot().problem, null);
            });

            it('R6: a drill path the new build does not have walks up to the deepest level it has, without greying', async () => {
                fake.paths = new Set(['0', '0/1']);
                await chip.simulate({ type: 'action', action: 'reset' });
                await circuit.simulate({ type: 'drill', path: [0, 1] });
                assert.equal(circuit.snapshot().diagram?.chip, 'Part0/1');
                // An edit that turns part 1 of part 0 into a built-in, built by the simulator panel.
                fake.paths = new Set(['0']);
                await chip.simulate({ type: 'action', action: 'reset' });
                await settled();
                assert.deepEqual(circuit.snapshot().path, [0]);
                assert.equal(circuit.snapshot().diagram?.chip, 'Part0');
                assert.equal(circuit.snapshot().problem, null);
                await chip.simulate({ type: 'set', pin: 'a', value: 1 });
                await settled();
                assert.equal(circuit.snapshot().problem, null);
                assert.deepEqual(circuit.snapshot().values?.path, [0]);
            });

            it('R6: a worker that stops while the drilled diagram loads keeps the drill path', async () => {
                fake.hold('chip.diagram');
                const drilled = send(circuit, { type: 'drill', path: [0] });
                await flush();
                fake.stop();
                await drilled;
                fake.release('chip.diagram');
                await settled();
                assert.deepEqual(circuit.snapshot().path, [0]);
                assert.equal(circuit.snapshot().diagram?.chip, 'Part0');
                assert.equal(circuit.snapshot().problem, null);
                assert.ok(chip.snapshot().state, 'the simulator shows the reopened chip');
                assert.equal(fake.count('chip.open'), 2, 'opened once again for both');
            });

            it('values that do not fit the diagram are never drawn on it: a mismatched chip.values reply brings the new diagram', async () => {
                // Start a normal rev-1 step so the circuit panel genuinely requests chip.values.
                fake.hold('chip.values');
                await send(chip, { type: 'action', action: 'step' });
                await until('chip.values to be in flight', () => fake.waiting('chip.values') === 1);
                const sent = circuit.posted.length;
                const valuesCount = fake.count('chip.values');
                // Meanwhile the engine moves to revision 5, so the in-flight chip.values replies with revision 5.
                fake.sessions.get(1)!.revision = 5;
                fake.release('chip.values');
                await settled();
                assert.equal(fake.count('chip.values'), valuesCount, 'the mismatched chip.values reply actually ran');
                assert.deepEqual(
                    circuit.posted.slice(sent).map((m) => m.type),
                    ['diagram'],
                    'the mismatched values were not drawn; the new diagram was fetched instead',
                );
                assert.equal(circuit.snapshot().diagram?.revision, 5);
            });

            it('F1: simulator set and setAll from build 1 are rejected after a peer resets to build 2', async () => {
                const oldBuild = chip.snapshot().build!;
                const target = chip.snapshot().target;
                assert.equal(oldBuild.revision, 1);
                await circuit.simulate({ type: 'clock', action: 'reset' });
                await settled();
                assert.equal(chip.snapshot().build?.revision, 2);

                const setsBefore = fake.count('chip.set');
                const messagesBefore = chipMessages.length;
                await send(chip, { type: 'set', pin: 'a', value: 1, build: oldBuild, target });
                await send(chip, { type: 'setAll', values: { a: 1 }, build: oldBuild, target });
                await settled();
                assert.equal(fake.count('chip.set'), setsBefore, 'stale simulator inputs never mutated build 2');
                assert.equal(fake.sessions.get(1)!.pins.get('a'), undefined);
                const posted = chipMessages.slice(messagesBefore) as { type: string; pin?: string; message?: string }[];
                assert.ok(posted.some((m) => m.type === 'rejected' && m.pin === 'a' && /built again/.test(m.message ?? '')), 'rejected feedback sent for set');
                assert.ok(posted.some((m) => m.type === 'note' && /built again/.test(m.message ?? '')), 'note sent for stale setAll');
            });

            it('F1: inputs and clock/reset actions from an old file target are dropped after switching chips', async () => {
                const oldChipTarget = chip.snapshot().target;
                const oldChipBuild = chip.snapshot().build!;
                const oldCircuitTarget = circuit.snapshot().target;
                const OTHER = file('chips', 'Other.hdl');
                await Promise.all([chip.showChip(OTHER), circuit.showChip(OTHER)]);
                await settled();
                const otherSession = sessions.hub(OTHER.fsPath)!.status().session!;

                await send(chip, { type: 'set', pin: 'a', value: 1, build: oldChipBuild, target: oldChipTarget });
                await send(chip, { type: 'setAll', values: { a: 1 }, build: oldChipBuild, target: oldChipTarget });
                await send(chip, { type: 'action', action: 'step', target: oldChipTarget });
                await send(chip, { type: 'action', action: 'reset', target: oldChipTarget });
                await send(circuit, { type: 'clock', action: 'step', target: oldCircuitTarget });
                await send(circuit, { type: 'clock', action: 'reset', target: oldCircuitTarget });
                await settled();

                assert.equal(fake.sessions.get(otherSession)!.pins.get('a'), undefined, 'old set/setAll did not touch the new chip');
                assert.equal(fake.sessions.get(otherSession)!.time, 0, 'old step did not clock the new chip');
                assert.equal(fake.sessions.get(otherSession)!.revision, 1, 'old reset did not rebuild the new chip');
            });

            it('F4: a drill or breadcrumb click made on build 1 while build 2 is pending is rejected and never opens build 2\'s part', async () => {
                const token1 = circuit.snapshot().token;
                fake.hold('chip.diagram');
                // Simulator resets to build 2; circuit's replacement diagram for [] is held so token1 is still current.
                await send(chip, { type: 'action', action: 'reset' });
                assert.equal(circuit.snapshot().token, token1);
                const drillSent = circuit.posted.length;
                const drilled = send(circuit, { type: 'drill', path: [0], diagram: token1 });
                await flush();
                fake.release('chip.diagram');
                await drilled;
                await settled();
                assert.equal(
                    fake.calls.some((c) => c.op === 'chip.diagram' && Array.isArray(c.path) && c.path.length === 1),
                    false,
                    'part 0 of build 2 was never fetched',
                );
                assert.deepEqual(circuit.snapshot().path, [], 'stayed at root');
                assert.equal(circuit.snapshot().diagram?.revision, 2);
                assert.equal(circuit.snapshot().diagram?.chip, 'Fake');
                assert.ok(circuit.posted.slice(drillSent).some((m) => m.type === 'note' && /rebuilt/.test(m.message)));

                // Now drill legitimately into [0] on build 2, then test a stale breadcrumb click back to [].
                await circuit.simulate({ type: 'drill', path: [0] });
                assert.deepEqual(circuit.snapshot().path, [0]);
                const token2 = circuit.snapshot().token;
                fake.hold('chip.diagram');
                await send(chip, { type: 'action', action: 'reset' });
                const crumbSent = circuit.posted.length;
                const crumb = send(circuit, { type: 'drill', path: [], diagram: token2 });
                await flush();
                fake.release('chip.diagram');
                await crumb;
                await settled();
                assert.deepEqual(circuit.snapshot().path, [0], 'stale breadcrumb click did not navigate');
                assert.equal(circuit.snapshot().diagram?.revision, 3);
                assert.ok(circuit.posted.slice(crumbSent).some((m) => m.type === 'note' && /rebuilt/.test(m.message)));
            });

            it('F3: both panels show Retry alongside a retained build error after a runtime halt', async () => {
                const BROKEN = file('chips', 'Broken.hdl');
                const brokenCircuit = new CircuitPanel(BROKEN, fakePanel(), sessions, host, () => undefined);
                const brokenChipMessages: unknown[] = [];
                const brokenChip = new ChipPanel(BROKEN, fakePanel(brokenChipMessages), sessions, tests, () => undefined);
                try {
                    fake.buildError = 'unknown chip `Nope`';
                    await brokenCircuit.load();
                    await brokenChip.load();
                    assert.match(brokenCircuit.snapshot().problem ?? '', /Nope/);
                    assert.match(brokenChip.snapshot().problem ?? '', /Nope/);
                    assert.equal(brokenCircuit.snapshot().retry, false);
                    assert.equal(brokenChip.snapshot().retry, false);

                    // Exhaust the restart budget via stops so every hub halts.
                    fake.broken = true;
                    fake.stop();
                    await until('both panels to offer Retry alongside the build error', async () => {
                        await Promise.all([brokenCircuit.settled(), brokenChip.settled(), settled()]);
                        return brokenCircuit.snapshot().retry && brokenChip.snapshot().retry;
                    });
                    assert.match(brokenCircuit.snapshot().problem ?? '', /Nope/, 'circuit keeps the build error');
                    assert.match(brokenChip.snapshot().problem ?? '', /Nope/, 'simulator keeps the build error');
                    assert.ok(brokenCircuit.posted.some((m) => m.type === 'error' && m.retry === true));
                    assert.ok(brokenChipMessages.some((m) => (m as { retry?: boolean }).retry === true));

                    // Once fixed, Retry works from either panel.
                    fake.broken = false;
                    fake.buildError = undefined;
                    await brokenChip.simulate({ type: 'retry' });
                    await Promise.all([brokenCircuit.settled(), brokenChip.settled()]);
                    assert.equal(brokenChip.snapshot().retry, false);
                    assert.equal(brokenCircuit.snapshot().retry, false);
                    assert.equal(brokenChip.snapshot().problem, null);
                    assert.equal(brokenCircuit.snapshot().problem, null);
                    assert.ok(brokenCircuit.snapshot().diagram);
                } finally {
                    brokenCircuit.dispose();
                    brokenChip.dispose();
                }
            });

            it('F2: panels show a non-modal note (not an error banner) when the action backlog is full', async () => {
                const tinySessions = new ChipSessions(fake, { maxQueuedActions: 2 });
                const busyCircuit = new CircuitPanel(FAKE, fakePanel(), tinySessions, host, () => undefined);
                const busyChipMessages: unknown[] = [];
                const busyChip = new ChipPanel(FAKE, fakePanel(busyChipMessages), tinySessions, tests, () => undefined);
                try {
                    await busyCircuit.load();
                    await busyChip.load();
                    fake.hold('chip.step');
                    const running = send(busyCircuit, { type: 'clock', action: 'step' });
                    await flush();
                    const q1 = send(busyCircuit, { type: 'clock', action: 'step' });
                    const q2 = send(busyChip, { type: 'action', action: 'step' });
                    // Queue is now full (2 queued actions): extra actions from either panel produce a non-modal note.
                    const circuitBefore = busyCircuit.posted.length;
                    const chipBefore = busyChipMessages.length;
                    await send(busyCircuit, { type: 'clock', action: 'step' });
                    await send(busyChip, { type: 'action', action: 'step' });
                    assert.ok(busyCircuit.posted.slice(circuitBefore).some((m) => m.type === 'note' && /busy/.test(m.message)));
                    assert.ok((busyChipMessages.slice(chipBefore) as { type: string; message?: string }[]).some((m) => m.type === 'note' && /busy/.test(m.message ?? '')));
                    assert.equal(busyCircuit.snapshot().problem, null);
                    assert.equal(busyChip.snapshot().problem, null);
                    fake.release('chip.step');
                    await Promise.all([running, q1, q2]);
                } finally {
                    busyCircuit.dispose();
                    busyChip.dispose();
                    tinySessions.dispose();
                }
            });

            it('R2: after three restarts in a minute both panels show Retry, and Retry brings them back', async () => {
                fake.broken = true;
                fake.stop();
                await until('the panels to offer Retry', async () => {
                    await settled();
                    return circuit.snapshot().retry && chip.snapshot().retry;
                });
                assert.equal(fake.spawns, 1 + 3, 'three automatic restarts');
                assert.ok(circuit.posted.some((m) => m.type === 'error' && m.retry === true));
                assert.ok(chipMessages.some((m) => (m as { retry?: boolean }).retry === true));
                // Nothing happens by itself any more.
                await chip.simulate({ type: 'action', action: 'step' });
                await new Promise((resolve) => setTimeout(resolve, 100));
                assert.equal(fake.spawns, 4);
                fake.broken = false;
                await circuit.simulate({ type: 'retry' });
                await settled();
                assert.equal(circuit.snapshot().retry, false);
                assert.equal(circuit.snapshot().problem, null);
                assert.equal(chip.snapshot().problem, null);
                assert.ok(circuit.snapshot().diagram);
                await chip.simulate({ type: 'action', action: 'step' });
                assert.equal(chip.snapshot().state?.time, '1');
            });
        });

        it('R5: a click on a part of a file that changed on disk since the build selects its line, not stale offsets', async () => {
            const nandwright = await api();
            const folder = scratchCopy('hub offsets ', ['chips/Veto.hdl', 'chips/Majority.hdl']);
            const veto = vscode.Uri.file(path.join(folder, 'Veto.hdl'));
            const majority = path.join(folder, 'Majority.hdl');
            const panel = await nandwright.circuits.open(veto);
            try {
                await panel.simulate({ type: 'drill', path: [0] });
                assert.equal(panel.snapshot().diagram?.chip, 'Majority');
                // Majority.hdl was never open. It changes on disk: its offsets move, its lines do not.
                const text = fs.readFileSync(majority, 'utf8');
                const line = text.split('\n').findIndex((l) => l.includes('Or(a=ab, b=ac'));
                fs.writeFileSync(majority, text.replace('    Or(a=ab, b=ac', '      Or(a=ab, b=ac'));
                await panel.simulate({ type: 'reveal', part: 3 });
                const shown = await until('the Majority editor', () => vscode.window.visibleTextEditors.find((e) => samePath(e.document.uri.fsPath, majority)));
                assert.equal(shown.selection.start.line, line);
                assert.equal(shown.selection.start.character, 0, 'the whole line is selected');
                assert.equal(shown.selection.end.character, shown.document.lineAt(line).text.length);
            } finally {
                panel.panel.dispose();
                await closeAll();
                await removeFolder(folder);
            }
        });

        it('a click on a part after a same-line edit selects its line, not stale offsets', async () => {
            const nandwright = await api();
            const editor = await open('chips', 'Veto.hdl');
            const panel = await nandwright.circuits.open(editor.document.uri);
            const line = editor.document.getText().split('\n').findIndex((text) => text.includes('Not(in=veto'));
            // An edit before the part statement on its own line moves the offsets, but not the lines.
            await editor.edit((edit) => edit.insert(new vscode.Position(line, 0), '  '));
            await panel.simulate({ type: 'reveal', part: 1 });
            const shown = await until('the Veto editor', () => vscode.window.visibleTextEditors.find((e) => e.document === editor.document));
            const selection = shown.selection;
            assert.equal(selection.start.line, line);
            assert.equal(selection.start.character, 0, 'the whole line is selected');
            assert.equal(selection.end.character, editor.document.lineAt(line).text.length);
            // After the reload the offsets match again and select the statement.
            await until('the diagram to reload', async () => {
                await panel.settled();
                return panel.spansMatch(editor.document.getText());
            });
            await panel.simulate({ type: 'reveal', part: 1 });
            await until('the Not statement to be selected', () => {
                const e = vscode.window.visibleTextEditors.find((x) => x.document === editor.document);
                return e?.document.getText(e.selection).startsWith('Not(in=veto, out=allowed)');
            });
            panel.panel.dispose();
        });

        it('F5: reveal ignores stale spans when the diagram changes while editor calls await, and checks the captured diagram hash', async () => {
            const nandwright = await api();
            const folder = scratchCopy('reveal race ', ['chips/Veto.hdl', 'chips/Majority.hdl']);
            const veto = vscode.Uri.file(path.join(folder, 'Veto.hdl'));
            const panel = await nandwright.circuits.open(veto);
            try {
                const diagram1 = panel.snapshot().diagram!;
                const original = fs.readFileSync(veto.fsPath, 'utf8');
                const notLine = original.split('\n').findIndex((l) => l.includes('Not(in=veto'));
                // Edit Veto.hdl on disk so offsets shift on the same line, and build diagram2 for that text.
                const edited = original.replace('    Not(in=veto', '        Not(in=veto');
                fs.writeFileSync(veto.fsPath, edited);
                await panel.simulate({ type: 'clock', action: 'reset' });
                const diagram2 = panel.snapshot().diagram!;
                assert.notEqual(diagram1.sourceHash, diagram2.sourceHash);

                // First: open the editor with selection at line 0, col 0.
                const editor = await vscode.window.showTextDocument(await vscode.workspace.openTextDocument(veto), { preview: false });
                const sentinel = new vscode.Selection(0, 0, 0, 0);
                editor.selection = sentinel;

                // Simulate a reveal whose diagram token changes while openTextDocument awaits:
                // snapshot() returns diagram1 (token 1) before the await, then diagram2 (token 2) after.
                let calls = 0;
                const racingPanel = {
                    isDisposed: false,
                    panel: panel.panel,
                    snapshot: () => (++calls <= 2 ? { diagram: diagram1, token: 1 } : { diagram: diagram2, token: 2 }),
                    spansMatch: (text: string) => panel.spansMatch(text),
                } as unknown as CircuitPanel;
                await nandwright.circuits.reveal(racingPanel, 1, undefined, 1);
                assert.ok(editor.selection.isEqual(sentinel), 'reveal aborted after openTextDocument when token changed');

                // Second: even if token stayed 1 while panel.spansMatch already moved to diagram2's hash,
                // reveal validates against the *captured* diagram1.sourceHash and falls back to whole-line selection
                // instead of applying diagram1's shifted byte offsets to the edited text.
                let calls2 = 0;
                const swappedHashPanel = {
                    isDisposed: false,
                    panel: panel.panel,
                    snapshot: () => ({ diagram: ++calls2 <= 2 ? diagram1 : diagram2, token: 1 }),
                    spansMatch: () => true, // would falsely claim exact match if reveal asked the panel
                } as unknown as CircuitPanel;
                await nandwright.circuits.reveal(swappedHashPanel, 1, undefined, 1);
                assert.equal(editor.selection.start.line, notLine);
                assert.equal(editor.selection.start.character, 0, 'fell back to full line instead of stale offsets');
                assert.equal(editor.selection.end.character, editor.document.lineAt(notLine).text.length);
            } finally {
                panel.panel.dispose();
                await closeAll();
                await removeFolder(folder);
            }
        });
    });

    describe('Hack computer', () => {
        it('follows open program tabs in one panel and stops the previous program', async () => {
            const nandwright = await api();
            const editor = await open('programs', 'Diagonal.asm');
            const panel = await nandwright.computers.open(editor.document.uri);
            await panel.simulate({ type: 'speed', speed: 5 });
            await panel.simulate({ type: 'run' });
            assert.equal(panel.snapshot().running, true);
            const show = async (uri: vscode.Uri) => {
                await vscode.window.showTextDocument(uri, { viewColumn: editor.viewColumn, preview: false });
                await until(`the computer to load ${path.basename(uri.fsPath)}`, () => {
                    const { state, running } = panel.snapshot();
                    return nandwright.computers.get(uri) === panel && state?.status === 'paused' && !running && state;
                }, 20_000, shows(panel));
                assert.equal(panel.snapshot().problem, null);
                assert.equal(panel.snapshot().state?.steps, 0);
            };
            const countdown = file('programs', 'Countdown.asm');
            await show(countdown);
            assert.equal(vscode.window.activeTextEditor?.document.uri.fsPath, countdown.fsPath);
            assert.equal(nandwright.computers.get(editor.document.uri), undefined);
            assert.equal(panel.panel.title, 'Countdown.asm · Hack computer');
            await panel.simulate({ type: 'step' });
            assert.equal(panel.snapshot().state?.steps, 1);

            const hack = file('programs', 'Follow.hack');
            fs.writeFileSync(hack.fsPath, '0000000000000111\n');
            try {
                await show(hack);
                await panel.simulate({ type: 'step' });
                assert.equal(panel.snapshot().state?.registers.find((r) => r.name === 'A')?.value, 7);
                await show(file('programs', 'Adder.vm'));
                assert.equal(panel.panel.title, 'Adder.vm · VM emulator');
                assert.equal(panel.snapshot().state?.kind, 'vm');
                await show(file('jack', 'Main.jack'));
                assert.equal(panel.snapshot().state?.fromJack, true);
                const compareFile = file('programs', 'Countdown.cmp');
                await vscode.window.showTextDocument(compareFile, { viewColumn: editor.viewColumn });
                await panel.settled();
                assert.equal(panel.panel.title, 'Main.jack · VM emulator');
                const reopened = await vscode.commands.executeCommand('nandwright.runProgram', countdown);
                assert.equal(reopened, panel);
                assert.equal(panel.snapshot().state?.kind, 'cpu');
            } finally {
                panel.panel.dispose();
                fs.rmSync(hack.fsPath, { force: true });
            }
        });

        it('keeps individual computer panels fixed when following is disabled', async () => {
            const nandwright = await api();
            const settings = vscode.workspace.getConfiguration('nandwright.simulator');
            await settings.update('followEditor', false, vscode.ConfigurationTarget.Global);
            try {
                const editor = await open('programs', 'Diagonal.asm');
                const first = await nandwright.computers.open(editor.document.uri);
                const other = file('programs', 'Countdown.asm');
                await vscode.window.showTextDocument(other, { viewColumn: editor.viewColumn });
                await first.settled();
                assert.equal(first.panel.title, 'Diagonal.asm · Hack computer');
                const second = await nandwright.computers.open(other);
                assert.notEqual(second, first);
                assert.equal(nandwright.computers.get(editor.document.uri), first);
                first.panel.dispose();
                second.panel.dispose();
            } finally {
                await settings.update('followEditor', undefined, vscode.ConfigurationTarget.Global);
            }
        });

        it('steps, runs, draws on the screen, pauses and resets', async () => {
            const nandwright = await api();
            const computer = await nandwright.computers.open(file('programs', 'Diagonal.asm'));
            const register = (name: string) => computer.snapshot().state?.registers.find((r) => r.name === name)?.value;
            assert.equal(computer.snapshot().problem, null);
            assert.equal(register('PC'), 0);
            await computer.simulate({ type: 'step' });
            assert.equal(register('PC'), 1);
            assert.equal(computer.snapshot().state?.steps, 1);
            await computer.simulate({ type: 'run' });
            assert.equal(computer.snapshot().running, true);
            await until('the program to draw', () => /[1-9a-f]/.test(computer.snapshot().state?.screen ?? ''), 20_000, shows(computer));
            await computer.simulate({ type: 'pause' });
            assert.equal(computer.snapshot().running, false);
            assert.ok((computer.snapshot().state?.steps ?? 0) > 100);
            await computer.simulate({ type: 'reset' });
            assert.equal(register('PC'), 0);
            assert.equal(computer.snapshot().state?.steps, 0);
            computer.panel.dispose();
        });

        it('stops at a breakpoint set in the editor and shows memory', async () => {
            const nandwright = await api();
            const uri = file('programs', 'Countdown.asm');
            const document = await vscode.workspace.openTextDocument(uri);
            const done = document.getText().split(/\r?\n/).findIndex((line) => line.trim() === '(DONE)');
            vscode.debug.addBreakpoints([new vscode.SourceBreakpoint(new vscode.Location(uri, new vscode.Position(done, 0)))]);
            try {
                const computer = await nandwright.computers.open(uri);
                await computer.simulate({ type: 'set', target: 'RAM', address: 0, value: '3' });
                await computer.simulate({ type: 'memory', start: 100 });
                await computer.simulate({ type: 'run' });
                await until('the breakpoint', () => computer.snapshot().state?.status === 'breakpoint', 20_000, shows(computer));
                const { state, running } = computer.snapshot();
                assert.equal(running, false);
                assert.equal(state?.line, done + 1);
                assert.deepEqual(state?.ram?.values.slice(0, 3), [3, 2, 1]);
                computer.panel.dispose();
            } finally {
                vscode.debug.removeBreakpoints(vscode.debug.breakpoints);
            }
        });

        it('feeds the keyboard and explains a program that goes wrong', async () => {
            const nandwright = await api();
            const computer = await nandwright.computers.open(file('programs', 'KeyLog.asm'));
            await computer.simulate({ type: 'memory', start: 24561 });
            await computer.simulate({ type: 'key', code: 75 });
            await computer.simulate({ type: 'memory', start: 24561 });
            assert.equal(computer.snapshot().state?.ram?.values[15], 75);
            assert.equal(computer.snapshot().state?.key, 75);
            // A wrong value is refused without disturbing the program.
            await computer.simulate({ type: 'set', target: 'RAM', address: 0, value: '99999' });
            assert.equal(computer.snapshot().problem, null);
            computer.panel.dispose();
        });
    });

    describe('VM emulator and Jack', () => {
        it('runs a Jack program without writing .vm files, and stops at a Jack breakpoint', async function () {
            this.timeout(90_000);
            const nandwright = await api();
            const uri = file('jack', 'Main.jack');
            // The fixture folder ships the official compiler's output; remove it
            // to prove that running Jack compiles in memory and writes nothing.
            for (const name of ['Main.vm', 'Counter.vm']) {
                fs.rmSync(file('jack', name).fsPath, { force: true });
            }
            const document = await vscode.workspace.openTextDocument(uri);
            const line = document.getText().split(/\r?\n/).findIndex((text) => text.includes('do c.add(25);'));
            vscode.debug.addBreakpoints([new vscode.SourceBreakpoint(new vscode.Location(uri, new vscode.Position(line, 0)))]);
            try {
                const computer = await nandwright.computers.open(uri);
                assert.equal(computer.snapshot().problem, null);
                assert.equal(computer.snapshot().state?.kind, 'vm');
                await computer.simulate({ type: 'run' });
                await until('the Jack breakpoint', () => computer.snapshot().state?.status === 'breakpoint', 20_000, shows(computer));
                assert.equal(computer.snapshot().state?.line, line);
                assert.equal(computer.snapshot().state?.callStack?.[0].function, 'Main.main');
                // A refresh (a fresh page, a moved memory window) must not turn
                // a breakpoint into a plain pause.
                const stopped = computer.snapshot().state;
                for (const message of [{ type: 'ready' }, { type: 'memory', start: 16 }] as const) {
                    await computer.simulate(message);
                    assert.equal(computer.snapshot().state?.status, 'breakpoint', `after ${message.type}`);
                    assert.equal(computer.snapshot().state?.line, line);
                    assert.equal(computer.snapshot().state?.steps, stopped?.steps);
                }
                await computer.simulate({ type: 'step', unit: 'line' });
                assert.equal(computer.snapshot().state?.line, line + 1);
                vscode.debug.removeBreakpoints(vscode.debug.breakpoints);
                await computer.simulate({ type: 'run' });
                await until('the program to end', () => computer.snapshot().state?.status === 'halted', 20_000, shows(computer));
                assert.match(computer.snapshot().state?.screen ?? '', /[1-9a-f]/, 'the program printed nothing');
                assert.ok(!fs.existsSync(file('jack', 'Main.vm').fsPath), 'running Jack wrote a .vm file');
                computer.panel.dispose();
            } finally {
                vscode.debug.removeBreakpoints(vscode.debug.breakpoints);
            }
        });

        it('runs on from a breakpoint inside a loop and stops at it again', async function () {
            this.timeout(90_000);
            const nandwright = await api();
            const dir = file('jackloop');
            fs.mkdirSync(dir.fsPath, { recursive: true });
            const uri = file('jackloop', 'Main.jack');
            fs.writeFileSync(
                uri.fsPath,
                [
                    '// Counts up in a loop that runs long enough to stop in it several times.',
                    'class Main {',
                    '    function void main() {',
                    '        var int i;',
                    '        let i = 0;',
                    '        while (i < 20000) {',
                    '            let i = i + 1;',
                    '        }',
                    '        return;',
                    '    }',
                    '}',
                    '',
                ].join('\n'),
            );
            const document = await vscode.workspace.openTextDocument(uri);
            const line = document.getText().split(/\r?\n/).findIndex((text) => text.includes('let i = i + 1;'));
            vscode.debug.addBreakpoints([new vscode.SourceBreakpoint(new vscode.Location(uri, new vscode.Position(line, 0)))]);
            try {
                const computer = await nandwright.computers.open(uri);
                assert.equal(computer.snapshot().problem, null);
                const steps: number[] = [];
                for (let press = 0; press < 3; press++) {
                    // Run again without taking the breakpoint away.
                    await computer.simulate({ type: 'run' });
                    await until(`the breakpoint, press ${press + 1}`, () => {
                        const { state, running } = computer.snapshot();
                        return !running && state?.status === 'breakpoint' && (press === 0 || state.steps > steps[press - 1]);
                    }, 20_000, shows(computer));
                    const { state } = computer.snapshot();
                    assert.equal(state?.line, line);
                    steps.push(state!.steps);
                }
                assert.ok(steps[0] < steps[1] && steps[1] < steps[2], `steps did not advance: ${steps.join(', ')}`);
                computer.panel.dispose();
            } finally {
                vscode.debug.removeBreakpoints(vscode.debug.breakpoints);
                await closeAll();
                // Best effort: on Windows VS Code may still hold the folder. The
                // runner removes the whole temporary workspace afterwards.
                try {
                    fs.rmSync(dir.fsPath, { recursive: true, force: true });
                } catch {
                    // left for the runner
                }
            }
        });

        it('compiles Jack to the .vm files the official compiler writes, and runs them', async () => {
            const nandwright = await api();
            for (const name of ['Main.vm', 'Counter.vm']) {
                fs.rmSync(file('jack', name).fsPath, { force: true });
            }
            await open('jack', 'Main.jack');
            const reply = await vscode.commands.executeCommand<{ outputs: { path: string }[]; problems: unknown[]; written: boolean }>(
                'nandwright.compileJack',
            );
            assert.equal(reply.problems.length, 0);
            assert.equal(reply.written, true);
            assert.deepEqual(reply.outputs.map((o) => path.basename(o.path)).sort(), ['Counter.vm', 'Main.vm']);
            const vm = fs.readFileSync(file('jack', 'Main.vm').fsPath, 'utf8');
            assert.match(vm, /^function Main\.main 4\r?\n/);
            assert.equal(vm.includes('\r\n'), os.EOL === '\r\n');
            const computer = await nandwright.computers.open(file('jack', 'Main.vm'));
            assert.equal(computer.snapshot().problem, null);
            assert.equal(computer.snapshot().state?.fromJack, false);
            await computer.simulate({ type: 'step' });
            assert.equal(computer.snapshot().state?.steps, 1);
            computer.panel.dispose();
        });

        it("compares a student's own tool output with the reference", async () => {
            const nandwright = await api();
            // As if the student's assembler had written Countdown.hack with one wrong word.
            await open('programs', 'Countdown.asm');
            const produced = await vscode.commands.executeCommand<{ outputs: { path: string; text: string }[] }>('nandwright.assemble');
            const hack = file('programs', 'Countdown.hack');
            assert.equal(produced.outputs[0].path.replace(/\\/g, '/').toLowerCase(), hack.fsPath.replace(/\\/g, '/').toLowerCase());
            assert.equal(await nandwright.outputs.compareWithReference(hack, { quiet: true }), true);
            const lines = fs.readFileSync(hack.fsPath, 'utf8').split(/\r?\n/);
            lines[2] = '1111111111111111';
            fs.writeFileSync(hack.fsPath, lines.join(os.EOL));
            assert.equal(await nandwright.outputs.compareWithReference(hack, { quiet: true }), false);
            // The analyzer's XML, for project 10.
            const xml = file('jack', 'MainT.xml');
            fs.writeFileSync(xml.fsPath, '<tokens>\n</tokens>\n');
            assert.equal(await nandwright.outputs.compareWithReference(xml, { quiet: true }), false);
            // Spaces and tabs inside lines do not count, as in the official
            // TextComparer: an analyzer that writes no indentation matches.
            const reference = await nandwright.runtime.call<{ text: string }>({ op: 'reference', path: xml.fsPath });
            const flat = reference.text.split(/\r?\n/).map((line) => line.trim().replace(/> /g, '>').replace(/ </g, '<'));
            fs.writeFileSync(xml.fsPath, flat.join('\r\n'));
            assert.equal(await nandwright.outputs.compareWithReference(xml, { quiet: true }), true);
            flat.splice(1, 1);
            fs.writeFileSync(xml.fsPath, flat.join('\n'));
            assert.equal(await nandwright.outputs.compareWithReference(xml, { quiet: true }), false);
            // A file with no source next to it has no reference.
            const orphan = file('programs', 'Orphan.hack');
            fs.writeFileSync(orphan.fsPath, '0000000000000000\n');
            assert.equal(await nandwright.outputs.compareWithReference(orphan, { quiet: true }), undefined);
        });
    });

    describe('writing the files the official tools write', () => {
        type Plan = { outputs: { path: string; existing: string; state: string; written: boolean }[]; conflicts: string[]; written: boolean };
        const assemble = async (source: vscode.Uri, extra: Record<string, unknown>): Promise<Plan> =>
            (await api()).runtime.call<Plan>({ op: 'asm.assemble', path: source.fsPath, crlf: false, ...extra });

        let source: vscode.Uri;
        let hack: vscode.Uri;
        beforeEach(() => {
            source = file('programs', 'Overwrite.asm');
            hack = file('programs', 'Overwrite.hack');
            fs.copyFileSync(file('programs', 'Countdown.asm').fsPath, source.fsPath);
            fs.rmSync(hack.fsPath, { force: true });
        });
        afterEach(() => {
            fs.rmSync(source.fsPath, { force: true });
            fs.rmSync(hack.fsPath, { force: true });
        });

        it('writes the .hack file when there is none', async () => {
            await open('programs', 'Overwrite.asm');
            const produced = await vscode.commands.executeCommand<Plan>('nandwright.assemble');
            assert.equal(produced.written, true);
            assert.deepEqual(produced.outputs.map((o) => [o.existing, o.written]), [['missing', true]]);
            assert.equal(fs.readFileSync(hack.fsPath, 'utf8').split(/\r?\n/)[0].length, 16);
        });

        it('leaves an identical .hack file alone, down to its line endings', async () => {
            await open('programs', 'Overwrite.asm');
            await vscode.commands.executeCommand('nandwright.assemble');
            // The same words with the other platform's line endings, as an
            // editor or the student's own assembler may have written them.
            const other = os.EOL === '\r\n' ? '\n' : '\r\n';
            const bytes = Buffer.from(fs.readFileSync(hack.fsPath, 'utf8').replace(/\r?\n/g, other));
            fs.writeFileSync(hack.fsPath, bytes);
            const before = fs.statSync(hack.fsPath).mtimeMs;
            await new Promise((resolve) => setTimeout(resolve, 50));
            const produced = await vscode.commands.executeCommand<Plan>('nandwright.assemble');
            assert.deepEqual(produced.outputs.map((o) => [o.existing, o.written]), [['same', false]]);
            assert.ok(fs.readFileSync(hack.fsPath).equals(bytes), 'the file was rewritten');
            assert.equal(fs.statSync(hack.fsPath).mtimeMs, before);
        });

        it('judges the file on disk, not the unsaved text of an editor', async () => {
            await open('programs', 'Overwrite.asm');
            await vscode.commands.executeCommand('nandwright.assemble');
            const bytes = fs.readFileSync(hack.fsPath);
            const editor = await open('programs', 'Overwrite.hack');
            await replaceAll(editor, '1111111111111111\n');
            assert.ok(editor.document.isDirty);
            const plan = await assemble(source, { write: false });
            assert.equal(plan.outputs[0].existing, 'same');
            assert.ok(fs.readFileSync(hack.fsPath).equals(bytes));
        });

        it('replaces a different file only as it was when the replacement was agreed', async () => {
            const wrong = '1111111111111111\n';
            fs.writeFileSync(hack.fsPath, wrong);
            const plan = await assemble(source, { write: false });
            assert.equal(plan.outputs[0].existing, 'different');
            const key = plan.outputs[0].path;
            // Nothing is replaced without being asked to.
            let reply = await assemble(source, { write: true, expect: {} });
            assert.deepEqual([reply.written, reply.conflicts.length], [false, 1]);
            assert.equal(fs.readFileSync(hack.fsPath, 'utf8'), wrong);
            // The file changes after the question was answered.
            fs.writeFileSync(hack.fsPath, '0000000000000000\n');
            reply = await assemble(source, { write: true, expect: { [key]: plan.outputs[0].state } });
            assert.deepEqual([reply.written, reply.conflicts.length], [false, 1]);
            assert.equal(fs.readFileSync(hack.fsPath, 'utf8'), '0000000000000000\n');
            // Asked about again, the agreement holds.
            const again = await assemble(source, { write: false });
            reply = await assemble(source, { write: true, expect: { [key]: again.outputs[0].state } });
            assert.deepEqual([reply.written, reply.conflicts.length], [true, 0]);
            assert.notEqual(fs.readFileSync(hack.fsPath, 'utf8'), '0000000000000000\n');
            assert.equal((await assemble(source, { write: false })).outputs[0].existing, 'same');
        });

        // Not covered: the modal "already exists with different content" dialog
        // that `nandwright.assemble` shows, which a test cannot click.
    });

    describe('stepping through a script', () => {
        it('stops on entry, steps, shows the pins and runs to the end', async function () {
            this.timeout(90_000);
            const script = file('chips', 'Accumulator.tst');
            const stops: string[] = [];
            const log: string[] = [];
            const tracker = vscode.debug.registerDebugAdapterTrackerFactory('nandwright', {
                createDebugAdapterTracker: () => ({
                    onWillReceiveMessage: (message: any) => log.push(`> ${message.command} ${JSON.stringify(message.arguments ?? {})}`),
                    onDidSendMessage: (message: any) => {
                        log.push(`< ${message.type} ${message.event ?? message.command} ${JSON.stringify(message.body ?? {}).slice(0, 200)}`);
                        if (message.type === 'event' && message.event === 'stopped') {
                            stops.push(message.body.reason);
                        }
                    },
                }),
            });
            const ended = new Promise<void>((resolve) => {
                const subscription = vscode.debug.onDidTerminateDebugSession(() => {
                    subscription.dispose();
                    resolve();
                });
            });
            try {
                vscode.debug.addBreakpoints([new vscode.SourceBreakpoint(new vscode.Location(script, new vscode.Position(22, 0)))]);
                assert.ok(await vscode.debug.startDebugging(undefined, { type: 'nandwright', request: 'launch', name: 'test', script: script.fsPath }));
                await until('the stop on entry', () => stops.length === 1);
                assert.equal(stops[0], 'entry');
                const session = vscode.debug.activeDebugSession!;
                const frame = async () => (await session.customRequest('stackTrace', { threadId: 1 })).stackFrames[0];
                assert.equal((await frame()).line, 1);
                await session.customRequest('next', { threadId: 1 });
                await until('the first step', () => stops.length === 2);
                assert.equal((await frame()).line, 6);
                await session.customRequest('stepIn', { threadId: 1 });
                await until('the command step', () => stops.length === 3);
                const scopes = (await session.customRequest('scopes', { frameId: 1 })).scopes;
                assert.deepEqual(scopes.map((s: any) => s.name), ['Pins']);
                const pins = (await session.customRequest('variables', { variablesReference: scopes[0].variablesReference })).variables;
                assert.match(pins.find((v: any) => v.name === 'in').value, /^5\b.*input$/);
                assert.match(pins.find((v: any) => v.name === 'sum').value, /internal$/);
                await session.customRequest('continue', { threadId: 1 });
                await until('the breakpoint', () => stops.length === 4).catch((error) => {
                    throw new Error(`${error.message}\n${log.join('\n')}`);
                });
                assert.equal(stops[3], 'breakpoint');
                assert.equal((await frame()).line, 23);
                await session.customRequest('continue', { threadId: 1 });
                await ended;
            } finally {
                vscode.debug.removeBreakpoints(vscode.debug.breakpoints);
                tracker.dispose();
            }
        });

        it('pauses a step that never ends, and stops without blocking the engine', async function () {
            this.timeout(90_000);
            // With every input at 0 the chip's output stays 0 and the loop never ends.
            const script = file('chips', 'Endless.tst');
            fs.writeFileSync(script.fsPath, 'load Majority.hdl;\nwhile out <> 1 {\n    eval,\n}\n');
            const stops: string[] = [];
            const log: string[] = [];
            const received: { seq: number; command: string }[] = [];
            const responses = new Map<number, number>();
            let disconnected = false;
            let stopsAfterDisconnect = 0;
            const tracker = vscode.debug.registerDebugAdapterTrackerFactory('nandwright', {
                createDebugAdapterTracker: () => ({
                    onWillReceiveMessage: (message: any) => {
                        log.push(`> ${message.command}`);
                        received.push({ seq: message.seq, command: message.command });
                    },
                    onDidSendMessage: (message: any) => {
                        log.push(`< ${message.type} ${message.event ?? message.command}`);
                        if (message.type === 'response') {
                            responses.set(message.request_seq, (responses.get(message.request_seq) ?? 0) + 1);
                            disconnected ||= message.command === 'disconnect';
                        }
                        if (message.type === 'event' && message.event === 'stopped') {
                            stops.push(message.body.reason);
                            if (disconnected) {
                                stopsAfterDisconnect++;
                            }
                        }
                    },
                }),
            });
            const ended = new Promise<void>((resolve) => {
                const subscription = vscode.debug.onDidTerminateDebugSession(() => {
                    subscription.dispose();
                    resolve();
                });
            });
            try {
                assert.ok(await vscode.debug.startDebugging(undefined, { type: 'nandwright', request: 'launch', name: 'test', script: script.fsPath }));
                await until('the stop on entry', () => stops.length === 1);
                const session = vscode.debug.activeDebugSession!;
                // The first step is the `load`; the next one enters the loop and never comes back.
                await session.customRequest('next', { threadId: 1 });
                await until('the first step', () => stops.length === 2);
                await session.customRequest('next', { threadId: 1 });
                await new Promise((resolve) => setTimeout(resolve, 1000));
                assert.equal(stops.length, 2, 'the endless step stopped by itself');
                await session.customRequest('pause', { threadId: 1 });
                await until('the pause', () => stops.length === 3, 5_000).catch((error) => {
                    throw new Error(`${error.message}\n${log.join('\n')}`);
                });
                assert.equal(stops[2], 'pause');
                // Run it again and stop at once.
                await session.customRequest('continue', { threadId: 1 });
                await vscode.debug.stopDebugging(session);
                await Promise.race([
                    ended,
                    new Promise((_, reject) => setTimeout(() => reject(new Error(`the session did not end\n${log.join('\n')}`)), 10_000)),
                ]);
                // The engine is free: another script runs to the end.
                const [result] = await (await api()).tests.runScripts([file('chips', 'Majority.tst')]);
                assert.equal(result.outcome.kind, 'passed', JSON.stringify(result.outcome));
                await new Promise((resolve) => setTimeout(resolve, 500));
                assert.equal(stopsAfterDisconnect, 0, `a stop came after the disconnect\n${log.join('\n')}`);
                for (const { seq, command } of received) {
                    assert.ok((responses.get(seq) ?? 0) <= 1, `${command} (${seq}) was answered ${responses.get(seq)} times\n${log.join('\n')}`);
                }
                const disconnect = received.find((r) => r.command === 'disconnect');
                assert.equal(responses.get(disconnect!.seq), 1, 'the disconnect was not answered exactly once');
            } finally {
                vscode.debug.removeBreakpoints(vscode.debug.breakpoints);
                tracker.dispose();
                if (vscode.debug.activeDebugSession) {
                    await vscode.debug.stopDebugging(vscode.debug.activeDebugSession);
                }
                fs.rmSync(script.fsPath, { force: true });
            }
        });
    });
});
