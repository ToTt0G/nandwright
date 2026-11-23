// Zip Project Files: packs a project folder for handing in. The student's
// own work is ticked; generated files and the course's test files are not.

import * as fs from 'node:fs';
import * as path from 'node:path';
import * as vscode from 'vscode';
import { zip } from './zip';

/** Folders never entered: tools, caches and build output. */
const SKIPPED_FOLDERS = new Set(['.git', '.vscode', 'node_modules', 'target', '__pycache__', 'bin', 'obj', 'build', 'dist']);
const MAX_FILES = 2000;
const MAX_BYTES = 100 * 1024 * 1024;

const COURSE_TEST = /\.(tst|cmp)$/i;
const GENERATED = /\.(out|hack|vm|xml|zip|class|pyc|o|exe)$/i;

export interface ProjectFile {
    /** Relative to the folder, with `/` separators. */
    path: string;
    size: number;
    /** Ticked unless it is generated or a course test file. */
    picked: boolean;
    /** Why it is not ticked. */
    description?: string;
}

/** The files under a folder, as the list offers them, or how many there are when there are too many. */
export async function projectFiles(folder: string): Promise<{ files: ProjectFile[]; tooLarge: boolean; bytes: number }> {
    const files: ProjectFile[] = [];
    let bytes = 0;
    const visit = async (dir: string, prefix: string) => {
        const entries = await fs.promises.readdir(dir, { withFileTypes: true });
        entries.sort((a, b) => (a.name < b.name ? -1 : a.name > b.name ? 1 : 0));
        for (const entry of entries) {
            // Hidden files and folders are tool settings, not work to hand in.
            if (entry.name.startsWith('.')) {
                continue;
            }
            const full = path.join(dir, entry.name);
            const relative = prefix ? `${prefix}/${entry.name}` : entry.name;
            if (entry.isDirectory()) {
                if (!SKIPPED_FOLDERS.has(entry.name)) {
                    await visit(full, relative);
                }
            } else if (entry.isFile()) {
                const size = (await fs.promises.stat(full)).size;
                bytes += size;
                const description = COURSE_TEST.test(entry.name)
                    ? 'test file from the course'
                    : GENERATED.test(entry.name)
                      ? 'generated'
                      : undefined;
                files.push({ path: relative, size, picked: description === undefined, ...(description ? { description } : {}) });
            }
        }
    };
    await visit(folder, '');
    return { files, tooLarge: files.length > MAX_FILES || bytes > MAX_BYTES, bytes };
}

/** Writes the chosen files of a folder, at their relative paths, to a zip file. */
export async function writeProjectZip(folder: string, paths: string[], target: string): Promise<void> {
    const entries = [];
    for (const relative of paths) {
        const full = path.join(folder, ...relative.split('/'));
        const [data, stat] = await Promise.all([fs.promises.readFile(full), fs.promises.stat(full)]);
        entries.push({ name: relative, data, mtime: stat.mtime });
    }
    await fs.promises.writeFile(target, zip(entries));
}

/** The folder to zip: the one given (Explorer), the active file's, or one the student picks. */
async function chooseFolder(argument: unknown): Promise<string | undefined> {
    if (argument instanceof vscode.Uri && argument.scheme === 'file') {
        return argument.fsPath;
    }
    const active = vscode.window.activeTextEditor?.document.uri;
    if (active?.scheme === 'file') {
        return path.dirname(active.fsPath);
    }
    const picked = await vscode.window.showOpenDialog({ canSelectFolders: true, canSelectFiles: false, canSelectMany: false });
    return picked?.[0]?.fsPath;
}

export async function zipProject(argument: unknown): Promise<void> {
    const folder = await chooseFolder(argument);
    if (!folder) {
        return;
    }
    const name = path.basename(folder);
    const { files, tooLarge, bytes } = await projectFiles(folder);
    if (files.length === 0) {
        void vscode.window.showInformationMessage(`${name} has no files to zip.`);
        return;
    }
    if (tooLarge) {
        const size = (bytes / (1024 * 1024)).toFixed(1);
        void vscode.window.showWarningMessage(`${name} is too large to zip here (${files.length} files, ${size} MB).`);
        return;
    }
    type Item = vscode.QuickPickItem & { file: ProjectFile };
    const items: Item[] = files.map((file) => ({ label: file.path, description: file.description, picked: file.picked, file }));
    const chosen = await vscode.window.showQuickPick(items, {
        canPickMany: true,
        title: `Zip ${name}: choose the files to hand in`,
        placeHolder: 'Your own work is ticked; generated files and the course\'s test files are not. Check what your course asks for.',
    });
    if (!chosen) {
        return;
    }
    if (chosen.length === 0) {
        void vscode.window.showInformationMessage('No files were chosen, so no zip was written.');
        return;
    }
    const target = await vscode.window.showSaveDialog({
        defaultUri: vscode.Uri.file(path.join(path.dirname(folder), `${name}.zip`)),
        filters: { Zip: ['zip'] },
    });
    if (!target) {
        return;
    }
    try {
        await writeProjectZip(
            folder,
            chosen.map((item) => item.file.path),
            target.fsPath,
        );
    } catch (error) {
        void vscode.window.showErrorMessage(`Nand2Tetris: ${error instanceof Error ? error.message : String(error)}`);
        return;
    }
    const action = await vscode.window.showInformationMessage(`Wrote ${path.basename(target.fsPath)} with ${chosen.length} files.`, 'Show in Folder');
    if (action) {
        await vscode.commands.executeCommand('revealFileInOS', target);
    }
}
