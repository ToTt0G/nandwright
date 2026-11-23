// Commands that produce what the official tools produce: assemble to .hack,
// compile Jack to .vm, and compare a file a student's own tool wrote with
// the reference.
//
// In projects 6, 10 and 11 the student writes the assembler, the analyzer
// and the compiler. The .hack, .xml and .vm files in those folders are then
// their own tool's output, so nothing here replaces an existing, different
// file without asking, and the offer is always to compare first.

import * as path from 'node:path';
import * as vscode from 'vscode';
import { Runtime } from './runtime/client';
import { Range, isError } from './runtime/protocol';

const REFERENCE_SCHEME = 'n2t-reference';

interface Output {
    path: string;
    text: string;
    existing: 'missing' | 'same' | 'different';
}

interface Produced {
    outputs: Output[];
    problems: { path: string; message: string; range: Range | null }[];
    written: boolean;
}

export class Outputs implements vscode.TextDocumentContentProvider, vscode.Disposable {
    /** Reference texts on show, by the path of the file they are compared with. */
    private readonly references = new Map<string, string>();
    private readonly changed = new vscode.EventEmitter<vscode.Uri>();
    readonly onDidChange = this.changed.event;
    private readonly registration: vscode.Disposable;

    constructor(private readonly runtime: Runtime) {
        this.registration = vscode.workspace.registerTextDocumentContentProvider(REFERENCE_SCHEME, this);
    }

    provideTextDocumentContent(uri: vscode.Uri): string {
        return this.references.get(uri.path) ?? '';
    }

    private referenceUri(file: vscode.Uri, text: string): vscode.Uri {
        const uri = vscode.Uri.from({ scheme: REFERENCE_SCHEME, path: file.path });
        this.references.set(uri.path, text);
        this.changed.fire(uri);
        return uri;
    }

    private async showDiff(file: vscode.Uri, reference: string, what: string): Promise<void> {
        const name = path.basename(file.fsPath);
        await vscode.commands.executeCommand(
            'vscode.diff',
            this.referenceUri(file, reference),
            file,
            `${name}: ${what} (left) ↔ the file (right)`,
        );
    }

    private async showProblem(problem: Produced['problems'][number]): Promise<void> {
        const name = path.basename(problem.path);
        const choice = await vscode.window.showErrorMessage(
            `Nothing was written: ${name} has an error. ${problem.message}`,
            'Show',
        );
        if (choice) {
            const position = new vscode.Position(problem.range?.startLine ?? 0, problem.range?.startCharacter ?? 0);
            await vscode.window.showTextDocument(vscode.Uri.file(problem.path), {
                selection: new vscode.Range(position, position),
            });
        }
    }

    /**
     * Assembles a `.asm` file or compiles the Jack classes of a folder,
     * asking before it replaces a file that exists with other content.
     */
    async produce(op: 'asm.assemble' | 'jack.compile', source: vscode.Uri): Promise<Produced | undefined> {
        await this.runtime.syncUnsaved();
        const request = { op, path: source.fsPath, crlf: process.platform === 'win32' };
        const planned = await this.runtime.tryCall<Produced>({ ...request, write: false });
        if (isError(planned)) {
            void vscode.window.showErrorMessage(`Nand2Tetris: ${planned.error.message}`);
            return undefined;
        }
        if (planned.problems.length > 0) {
            await this.showProblem(planned.problems[0]);
            return planned;
        }
        const different = planned.outputs.filter((output) => output.existing === 'different');
        if (different.length > 0) {
            const names = different.map((output) => path.basename(output.path)).join(', ');
            const tool = op === 'asm.assemble' ? 'assembler' : 'compiler';
            const choice = await vscode.window.showWarningMessage(
                `${names} already ${different.length === 1 ? 'exists' : 'exist'} with different content.`,
                {
                    modal: true,
                    detail: `If your own ${tool} wrote ${different.length === 1 ? 'it' : 'them'}, compare instead of replacing: the comparison shows exactly where your ${tool}'s output differs.`,
                },
                'Compare',
                'Replace',
            );
            if (choice === 'Compare') {
                await this.showDiff(vscode.Uri.file(different[0].path), different[0].text, `what the ${tool} writes`);
                return planned;
            }
            if (choice !== 'Replace') {
                return planned;
            }
        }
        const written = await this.runtime.tryCall<Produced>({ ...request, write: true });
        if (isError(written)) {
            void vscode.window.showErrorMessage(`Nand2Tetris: ${written.error.message}`);
            return undefined;
        }
        const names = written.outputs.map((output) => path.basename(output.path)).join(', ');
        vscode.window.setStatusBarMessage(`$(check) Wrote ${names}`, 8000);
        return written;
    }

    /**
     * Compares a `.hack`, `.vm` or `.xml` file with what the built-in
     * assembler, compiler or analyzer makes of the source next to it.
     * Returns whether they are the same, or `undefined` if there is nothing
     * to compare with.
     */
    async compareWithReference(file: vscode.Uri, options: { quiet?: boolean } = {}): Promise<boolean | undefined> {
        await this.runtime.syncUnsaved();
        const reply = await this.runtime.tryCall<{ text: string; from: string; tool: string }>({
            op: 'reference',
            path: file.fsPath,
        });
        if (isError(reply)) {
            if (!options.quiet) {
                void vscode.window.showInformationMessage(`Nand2Tetris: ${reply.error.message}`);
            }
            return undefined;
        }
        // The editor's text if the file is open with unsaved changes,
        // otherwise what is on disk now (an open document can lag behind a
        // file another program has just rewritten).
        const open = vscode.workspace.textDocuments.find((d) => d.isDirty && d.uri.toString() === file.toString());
        const current = open ? open.getText() : new TextDecoder().decode(await vscode.workspace.fs.readFile(file));
        const lines = (text: string) => text.replace(/\r\n?/g, '\n').replace(/\s+$/, '').split('\n').map((line) => line.trimEnd());
        const mine = lines(current);
        const theirs = lines(reply.text);
        const name = path.basename(file.fsPath);
        const source = path.basename(reply.from);
        const same = mine.length === theirs.length && mine.every((line, i) => line === theirs[i]);
        if (options.quiet) {
            return same;
        }
        if (same) {
            void vscode.window.showInformationMessage(
                `${name} matches what ${reply.tool} makes of ${source} (${theirs.length} lines).`,
            );
            return true;
        }
        const first = mine.findIndex((line, i) => line !== theirs[i]);
        const at = first >= 0 ? first + 1 : Math.min(mine.length, theirs.length) + 1;
        vscode.window.setStatusBarMessage(`$(diff) ${name} differs from the reference, first at line ${at}`, 10000);
        await this.showDiff(file, reply.text, `what ${reply.tool} makes of ${source}`);
        return false;
    }

    dispose(): void {
        this.registration.dispose();
        this.changed.dispose();
    }
}
