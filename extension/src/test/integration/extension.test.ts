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

async function until<T>(what: string, probe: () => T | undefined | false | Promise<T | undefined | false>, timeoutMs = 20_000): Promise<T> {
    const deadline = Date.now() + timeoutMs;
    for (;;) {
        const value = await probe();
        if (value) {
            return value;
        }
        if (Date.now() > deadline) {
            throw new Error(`timed out waiting for ${what}`);
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
            await until('the program to draw', () => /[1-9a-f]/.test(computer.snapshot().state?.screen ?? ''));
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
                await until('the breakpoint', () => computer.snapshot().state?.status === 'breakpoint');
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
                await until('the Jack breakpoint', () => computer.snapshot().state?.status === 'breakpoint');
                assert.equal(computer.snapshot().state?.line, line);
                assert.equal(computer.snapshot().state?.callStack?.[0].function, 'Main.main');
                await computer.simulate({ type: 'step', unit: 'line' });
                assert.equal(computer.snapshot().state?.line, line + 1);
                vscode.debug.removeBreakpoints(vscode.debug.breakpoints);
                await computer.simulate({ type: 'run' });
                await until('the program to end', () => computer.snapshot().state?.status === 'halted');
                assert.match(computer.snapshot().state?.screen ?? '', /[1-9a-f]/, 'the program printed nothing');
                assert.ok(!fs.existsSync(file('jack', 'Main.vm').fsPath), 'running Jack wrote a .vm file');
                computer.panel.dispose();
            } finally {
                vscode.debug.removeBreakpoints(vscode.debug.breakpoints);
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
    });
});
