// Zip Project Files: packs a project folder for handing in. The student's
// own work is ticked; generated files and the course's test files are not.

import * as fs from 'node:fs';
import * as path from 'node:path';
import * as vscode from 'vscode';
import { zip } from './zip';
import { zipFileName } from './zipName';

/** Folders never entered: tools, caches and build output. */
const SKIPPED_FOLDERS = new Set(['.git', '.vscode', 'node_modules', 'target', '__pycache__', 'bin', 'obj', 'build', 'dist']);
const MAX_FILES = 2000;
const MAX_BYTES = 100 * 1024 * 1024;

/** Folders of another system's bookkeeping, never entered. */
const CLUTTER_FOLDERS = new Set(['__macosx']);
/** Files of another program's bookkeeping, never offered: Office lock files and Windows folder settings. */
const CLUTTER_FILES = /^(~\$.*|thumbs\.db|desktop\.ini)$/i;

const COURSE_TEST = /\.(tst|cmp)$/i;
const GENERATED = /\.(out|hack|vm|xml|zip|class|pyc|o|exe)$/i;

export interface ProjectFile {
    /** The name in the zip: relative to the folder, with `/` separators. */
    path: string;
    /** Where the file is, for a file added from another folder. */
    source?: string;
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
                if (!SKIPPED_FOLDERS.has(entry.name) && !CLUTTER_FOLDERS.has(entry.name.toLowerCase())) {
                    await visit(full, relative);
                }
            } else if (entry.isFile()) {
                if (CLUTTER_FILES.test(entry.name)) {
                    continue;
                }
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
    for (const file of files) {
        if (file.picked && translatedAssembly(file.path, path.basename(folder), files)) {
            file.picked = false;
            file.description = 'generated';
        }
    }
    return { files, tooLarge: files.length > MAX_FILES || bytes > MAX_BYTES, bytes };
}

/**
 * Whether an `.asm` file is a VM translator's output: `Prog.asm` next to
 * `Prog.vm`, or `Dir/Dir.asm` in a folder of `.vm` files. Hand-written
 * assembly has no `.vm` file of its name.
 */
function translatedAssembly(file: string, folderName: string, files: ProjectFile[]): boolean {
    if (!/\.asm$/i.test(file)) {
        return false;
    }
    const slash = file.lastIndexOf('/');
    const dir = slash < 0 ? '' : file.slice(0, slash);
    const stem = file.slice(slash + 1, -'.asm'.length).toLowerCase();
    const dirName = (slash < 0 ? folderName : dir.slice(dir.lastIndexOf('/') + 1)).toLowerCase();
    const prefix = dir ? `${dir}/`.toLowerCase() : '';
    return files.some((other) => {
        const name = other.path.toLowerCase();
        if (!name.startsWith(prefix) || name.slice(prefix.length).includes('/') || !name.endsWith('.vm')) {
            return false;
        }
        return name.slice(prefix.length, -'.vm'.length) === stem || dirName === stem;
    });
}

/**
 * Files from other folders, as the list adds them: ticked, at the root of
 * the zip under their own names. A name already at the root of the list is
 * refused, ignoring case as Windows does.
 */
export async function addedFiles(listed: ProjectFile[], sources: string[]): Promise<{ added: ProjectFile[]; refused: string[] }> {
    const taken = new Set(listed.filter((file) => !file.path.includes('/')).map((file) => file.path.toLowerCase()));
    const added: ProjectFile[] = [];
    const refused: string[] = [];
    for (const source of sources) {
        const name = path.basename(source);
        if (taken.has(name.toLowerCase())) {
            refused.push(name);
            continue;
        }
        taken.add(name.toLowerCase());
        const size = (await fs.promises.stat(source)).size;
        added.push({ path: name, source, size, picked: true, description: 'added' });
    }
    return { added, refused };
}

/**
 * Writes the chosen files to a zip file: a folder's files at their relative
 * paths, and added files (with a `source`) under their `path`.
 */
export async function writeProjectZip(folder: string, files: Array<string | Pick<ProjectFile, 'path' | 'source'>>, target: string): Promise<void> {
    const entries = [];
    for (const file of files) {
        const name = typeof file === 'string' ? file : file.path;
        const full = typeof file !== 'string' && file.source ? file.source : path.join(folder, ...name.split('/'));
        const [data, stat] = await Promise.all([fs.promises.readFile(full), fs.promises.stat(full)]);
        entries.push({ name, data, mtime: stat.mtime });
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
    const chosen = await chooseFiles(name, files);
    if (!chosen) {
        return;
    }
    if (chosen.length === 0) {
        void vscode.window.showInformationMessage('No files were chosen, so no zip was written.');
        return;
    }
    const pattern = vscode.workspace.getConfiguration('nandwright.zip').get<string>('fileName', '{folder}');
    const target = await vscode.window.showSaveDialog({
        defaultUri: vscode.Uri.file(path.join(path.dirname(folder), `${zipFileName(pattern, name)}.zip`)),
        filters: { Zip: ['zip'] },
    });
    if (!target) {
        return;
    }
    try {
        await writeProjectZip(
            folder,
            chosen,
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

const ADD_FILE: vscode.QuickInputButton = { iconPath: new vscode.ThemeIcon('add'), tooltip: 'Add a file from another folder\u2026' };
const SET_NAME: vscode.QuickInputButton = { iconPath: new vscode.ThemeIcon('gear'), tooltip: 'Set the zip\'s name\u2026' };

/** The list of files, with buttons to add files from elsewhere and to set the zip's name. */
function chooseFiles(name: string, files: ProjectFile[]): Promise<ProjectFile[] | undefined> {
    type Item = vscode.QuickPickItem & { file: ProjectFile };
    const listed = [...files];
    // What is ticked, by name in the zip; kept when the list is rebuilt.
    const ticked = new Map(listed.map((file) => [file.path, file.picked]));
    const pick = vscode.window.createQuickPick<Item>();
    pick.canSelectMany = true;
    pick.ignoreFocusOut = true;
    pick.title = `Zip ${name}: choose the files to hand in`;
    pick.placeholder = 'Your own work is ticked; generated files and the course\'s test files are not. Check what your course asks for.';
    pick.buttons = [ADD_FILE, SET_NAME];
    let rebuilding = false;
    const rebuild = () => {
        rebuilding = true;
        const items: Item[] = listed.map((file) => ({ label: file.path, description: file.description, file }));
        pick.items = items;
        pick.selectedItems = items.filter((item) => ticked.get(item.file.path));
        rebuilding = false;
    };
    rebuild();
    return new Promise((resolve) => {
        let done = false;
        let inDialog = false;
        const finish = (result: ProjectFile[] | undefined) => {
            if (!done) {
                done = true;
                resolve(result);
                pick.dispose();
            }
        };
        pick.onDidChangeSelection((selected) => {
            if (rebuilding) {
                return;
            }
            const now = new Set(selected.map((item) => item.file.path));
            for (const file of listed) {
                ticked.set(file.path, now.has(file.path));
            }
        });
        pick.onDidAccept(() => finish(listed.filter((file) => ticked.get(file.path))));
        pick.onDidHide(() => {
            if (!inDialog) {
                finish(undefined);
            }
        });
        pick.onDidTriggerButton(async (button) => {
            if (button === SET_NAME) {
                await vscode.commands.executeCommand('workbench.action.openSettings', 'nandwright.zip.fileName');
                return;
            }
            inDialog = true;
            let sources: vscode.Uri[] | undefined;
            try {
                sources = await vscode.window.showOpenDialog({ canSelectFiles: true, canSelectFolders: false, canSelectMany: true, openLabel: 'Add to Zip' });
            } finally {
                inDialog = false;
            }
            if (done) {
                return;
            }
            pick.show();
            const paths = (sources ?? []).filter((uri) => uri.scheme === 'file').map((uri) => uri.fsPath);
            if (paths.length === 0) {
                return;
            }
            try {
                const { added, refused } = await addedFiles(listed, paths);
                for (const file of added) {
                    listed.push(file);
                    ticked.set(file.path, true);
                }
                rebuild();
                if (refused.length > 0) {
                    void vscode.window.showWarningMessage(
                        `Not added: the zip already has ${refused.map((file) => `"${file}"`).join(', ')} at the top level. Rename the file and add it again.`,
                    );
                }
            } catch (error) {
                void vscode.window.showErrorMessage(`Nand2Tetris: ${error instanceof Error ? error.message : String(error)}`);
            }
        });
        pick.show();
    });
}
