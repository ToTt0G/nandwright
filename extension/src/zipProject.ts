// Zip Project Files: packs a project folder for handing in. The student's
// own work is ticked; generated files and the course's test files are not.

import * as fs from 'node:fs';
import * as path from 'node:path';
import * as vscode from 'vscode';
import { chooseAndCopyGradeSheets } from './gradeSheets';
import { presetFor } from './presets';
import {
    ProjectFile,
    ZipPlaces,
    ZipPlan,
    ZipSettings,
    addedFiles,
    gradeSheetDescription,
    planZip,
    projectFiles,
    projectFolder,
    writeProjectZip,
} from './zipFiles';
import { StudentName, zipFileName } from './zipName';

export { addedFiles, planZip, projectFiles, writeProjectZip };

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

/** A setting's value if the student set it (user, workspace or folder), else undefined. */
function explicit<T>(config: vscode.WorkspaceConfiguration, key: string): T | undefined {
    const inspected = config.inspect<T>(key);
    return inspected?.workspaceFolderValue ?? inspected?.workspaceValue ?? inspected?.globalValue;
}

/** The zip settings for a folder: what the student set, and the course preset. */
export function zipSettings(folder: string): ZipSettings {
    const uri = vscode.Uri.file(folder);
    const zipConfig = vscode.workspace.getConfiguration('nandwright.zip', uri);
    const student = vscode.workspace.getConfiguration('nandwright.student');
    const preset = presetFor(vscode.workspace.getConfiguration('nandwright', uri).get<string>('course', 'none'));
    const settings: ZipSettings = {
        fileName: explicit<string>(zipConfig, 'fileName'),
        projects: explicit<unknown>(zipConfig, 'projects'),
        gradeSheetFolder: zipConfig.get<string>('gradeSheetFolder', ''),
        names: { familyName: student.get<string>('familyName', ''), firstName: student.get<string>('firstName', '') },
    };
    if (preset) {
        settings.preset = preset;
    }
    if (!settings.fileName && !preset) {
        settings.fileName = zipConfig.get<string>('fileName', '{folder}');
    }
    return settings;
}

/** The workspace folder holding `folder`, and all workspace folders. */
export function zipPlaces(folder: string): ZipPlaces {
    const root = vscode.workspace.getWorkspaceFolder(vscode.Uri.file(folder))?.uri.fsPath;
    const roots = (vscode.workspace.workspaceFolders ?? []).filter((f) => f.uri.scheme === 'file').map((f) => f.uri.fsPath);
    return { ...(root ? { root } : {}), roots };
}

/** The folder to zip for a folder: the project folder above it if the project map lists one. */
export function handInFolder(folder: string): string {
    const settings = zipSettings(folder);
    return projectFolder(folder, settings.projects ?? settings.preset?.projects, zipPlaces(folder).root);
}

const NAME_LABELS: Record<keyof StudentName, string> = { familyName: 'family name', firstName: 'first name' };

/**
 * Asks for the parts of the student's name the zip's name needs, and offers
 * to remember them in the user settings. Undefined if the student cancels.
 */
async function askNames(missing: Array<keyof StudentName>, names: StudentName): Promise<StudentName | undefined> {
    const filled: StudentName = { ...names };
    for (const part of missing) {
        const value = await vscode.window.showInputBox({
            title: `The zip's name needs your ${NAME_LABELS[part]}`,
            prompt: `Your ${NAME_LABELS[part]} as your course wants it in the zip's name.`,
            ignoreFocusOut: true,
            validateInput: (text) => (text.trim() ? undefined : 'Type your name, or press Escape to cancel.'),
        });
        if (value === undefined) {
            return undefined;
        }
        filled[part] = value.trim();
    }
    const remember = await vscode.window.showInformationMessage('Remember your name for the next zips?', 'Remember', 'Not Now');
    if (remember === 'Remember') {
        const student = vscode.workspace.getConfiguration('nandwright.student');
        for (const part of missing) {
            await student.update(part, filled[part], vscode.ConfigurationTarget.Global);
        }
    }
    return filled;
}

export async function zipProject(argument: unknown): Promise<void> {
    const chosenFolder = await chooseFolder(argument);
    if (!chosenFolder) {
        return;
    }
    const folder = handInFolder(chosenFolder);
    const name = path.basename(folder);
    let plan: ZipPlan;
    try {
        plan = await planZip(folder, zipSettings(folder), zipPlaces(folder));
    } catch (error) {
        void vscode.window.showErrorMessage(`Nand2Tetris: ${error instanceof Error ? error.message : String(error)}`);
        return;
    }
    const { files, tooLarge, bytes } = plan;
    if (files.length === 0) {
        void vscode.window.showInformationMessage(`${name} has no files to zip.`);
        return;
    }
    if (tooLarge) {
        const size = (bytes / (1024 * 1024)).toFixed(1);
        void vscode.window.showWarningMessage(`${name} is too large to zip here (${files.length} files, ${size} MB).`);
        return;
    }
    const chosen = await chooseFiles(plan, folder);
    if (!chosen) {
        return;
    }
    if (chosen.length === 0) {
        void vscode.window.showInformationMessage('No files were chosen, so no zip was written.');
        return;
    }
    // Read the settings again: the gear button may have changed them.
    const settings = zipSettings(folder);
    const again = await planZip(folder, settings, zipPlaces(folder));
    let names = settings.names ?? {};
    if (again.missingNames.length > 0) {
        const asked = await askNames(again.missingNames, names);
        if (!asked) {
            return;
        }
        names = asked;
    }
    const zipName = zipFileName(again.pattern, name, again.project, names);
    const target = await vscode.window.showSaveDialog({
        defaultUri: vscode.Uri.file(path.join(path.dirname(folder), `${zipName}.zip`)),
        filters: { Zip: ['zip'] },
    });
    if (!target) {
        return;
    }
    try {
        await writeProjectZip(folder, chosen, target.fsPath);
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
const SET_NAME: vscode.QuickInputButton = { iconPath: new vscode.ThemeIcon('gear'), tooltip: 'Zip settings: name, project folders, grade sheet\u2026' };
const CHOOSE_SHEET: vscode.QuickInputButton = { iconPath: new vscode.ThemeIcon('table'), tooltip: 'Choose the grade sheet you downloaded\u2026' };
const OPEN_SHEET: vscode.QuickInputButton = { iconPath: new vscode.ThemeIcon('link-external'), tooltip: 'Open the grade sheet, to fill it in before you zip' };

/** The list of files, with buttons to add files from elsewhere and to set the zip's name. */
function chooseFiles(plan: ZipPlan, folder: string): Promise<ProjectFile[] | undefined> {
    type Item = vscode.QuickPickItem & { file?: ProjectFile };
    const listed = [...plan.files];
    // What is ticked, by name in the zip; kept when the list is rebuilt.
    const ticked = new Map(listed.map((file) => [file.path, file.picked]));
    const pick = vscode.window.createQuickPick<Item>();
    pick.canSelectMany = true;
    pick.ignoreFocusOut = true;
    pick.title = `${plan.title}: choose the files to hand in`;
    pick.placeholder = 'Your own work is ticked; generated files and the course\'s test files are not. Check what your course asks for.';
    const notes = [...plan.notes];
    let missingSheet = plan.missingSheet;
    const buttons = () => (missingSheet ? [CHOOSE_SHEET, ADD_FILE, SET_NAME] : [ADD_FILE, SET_NAME]);
    pick.buttons = buttons();
    let rebuilding = false;
    const rebuild = () => {
        rebuilding = true;
        // Notes (a missing folder, a grade sheet not found) head the list.
        const heads: Item[] = notes.map((note) => ({ label: note, kind: vscode.QuickPickItemKind.Separator }));
        const items: Item[] = listed.map((file) => ({
            label: file.path,
            description: file.description,
            file,
            ...(file.gradeSheet ? { buttons: [OPEN_SHEET] } : {}),
        }));
        pick.items = [...heads, ...(heads.length > 0 ? [{ label: 'Files', kind: vscode.QuickPickItemKind.Separator }] : []), ...items];
        pick.selectedItems = items.filter((item) => item.file && ticked.get(item.file.path));
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
            const now = new Set(selected.flatMap((item) => (item.file ? [item.file.path] : [])));
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
        pick.onDidTriggerItemButton(async (event) => {
            const file = event.item.file;
            if (event.button === OPEN_SHEET && file) {
                // The zip is read from disk when it is saved, so a sheet
                // filled in meanwhile goes in as saved.
                await vscode.env.openExternal(vscode.Uri.file(file.source ?? path.join(folder, ...file.path.split('/'))));
            }
        });
        pick.onDidTriggerButton(async (button) => {
            if (button === SET_NAME) {
                await vscode.commands.executeCommand('workbench.action.openSettings', 'nandwright.zip');
                return;
            }
            if (button === CHOOSE_SHEET) {
                inDialog = true;
                let result: Awaited<ReturnType<typeof chooseAndCopyGradeSheets>>;
                try {
                    result = await chooseAndCopyGradeSheets(zipPlaces(folder), `Choose the grade sheet for project ${missingSheet}`);
                } finally {
                    inDialog = false;
                }
                if (done) {
                    return;
                }
                pick.show();
                const sheet = result?.copied[0] ?? (result?.kept[0] ? path.join(result.folder, result.kept[0]) : undefined);
                if (!sheet) {
                    return;
                }
                const { added } = await addedFiles(listed, [sheet]);
                for (const file of added) {
                    const stat = await fs.promises.stat(sheet);
                    file.gradeSheet = true;
                    file.description = gradeSheetDescription(path.basename(path.dirname(sheet)), stat.mtime);
                    listed.push(file);
                    ticked.set(file.path, true);
                }
                const note = notes.findIndex((line) => line.startsWith('No grade sheet for project'));
                if (note >= 0) {
                    notes.splice(note, 1);
                }
                missingSheet = undefined;
                pick.buttons = buttons();
                rebuild();
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
