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
import type { ScriptResult } from '../../testing';

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
        for (const document of vscode.workspace.textDocuments) {
            if (document.isDirty) {
                await vscode.window.showTextDocument(document);
                await vscode.commands.executeCommand('workbench.action.files.revert');
            }
        }
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
        for (const command of ['runTest', 'debugTest', 'openChip', 'compareOutput', 'getStarted']) {
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

        it('runs the test of the chip in the editor', async () => {
            await open('chips', 'Parity4.hdl');
            const results = await vscode.commands.executeCommand<ScriptResult[]>('nandwright.runTest');
            assert.deepEqual(
                results.map((r) => [path.basename(r.uri.fsPath), r.outcome.kind]),
                [['Parity4.tst', 'passed']],
            );
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

    describe('Hack computer', () => {
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
