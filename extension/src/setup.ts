// Nand2Tetris: Set Up… — course, name, grade sheets and tests on save, in a
// few quick steps; and the one-time offer to run it.

import * as fs from 'node:fs';
import * as path from 'node:path';
import * as vscode from 'vscode';
import { confirmReplace, SPREADSHEETS } from './gradeSheets';
import { OFFER_KEY, OfferState, afterAnswer, shouldOffer } from './offer';
import { PRESETS, presetFor } from './presets';
import { ZipPlaces, copyGradeSheets, gradeSheetHome, isGradeSheetFolderName } from './zipFiles';

/** A choice in a step. `keep` leaves the setting as it is. */
export interface SetupChoice extends vscode.QuickPickItem {
    value: string;
}

/** What the setup asks through; the integration tests give their own answers. */
export interface SetupUi {
    /** One choice, or undefined to stop the setup. */
    pick(title: string, choices: SetupChoice[]): Promise<SetupChoice | undefined>;
    /** Text; empty keeps the current value; undefined stops the setup. */
    input(title: string, prompt: string, value: string): Promise<string | undefined>;
    /** Spreadsheets the student chose, or none. */
    pickFiles(title: string): Promise<string[]>;
    /** A folder the student chose, or undefined. */
    pickFolder(title: string): Promise<string | undefined>;
    /** Whether to replace a sheet that is already there. */
    confirmReplace(name: string): Promise<boolean>;
    /** The closing message. */
    done(message: string): void;
}

const KEEP = 'keep';
const keep = (current: string): SetupChoice => ({ label: '$(debug-step-over) Skip', description: `keep: ${current}`, value: KEEP });

/** The spreadsheets in a folder. */
function spreadsheetsIn(folder: string): string[] {
    try {
        return fs.readdirSync(folder).filter((name) => /\.(xlsx|xls|ods)$/i.test(name) && !name.startsWith('~$'));
    } catch {
        return [];
    }
}

/** Where the student's grade sheets are now, in words, for the setup. */
function sheetsNow(places: ZipPlaces, settingFolder: string): { text: string; folder?: string } {
    const candidates: string[] = [];
    const home = gradeSheetHome(places, settingFolder);
    if (home) {
        candidates.push(home);
    }
    for (const root of places.roots) {
        try {
            for (const entry of fs.readdirSync(root, { withFileTypes: true })) {
                if (entry.isDirectory() && isGradeSheetFolderName(entry.name)) {
                    candidates.push(path.join(root, entry.name));
                }
            }
        } catch {
            // not readable
        }
    }
    for (const folder of candidates) {
        const count = spreadsheetsIn(folder).length;
        if (count > 0) {
            return { text: `${count} sheet${count === 1 ? '' : 's'} found in ${path.basename(folder)}`, folder };
        }
    }
    return { text: 'none found' };
}

/**
 * Runs the setup steps. Settings go to the user settings, except a grade
 * sheet folder inside the workspace, which goes to the workspace settings
 * as a relative path. Returns what was set, in words.
 */
export async function runSetup(ui: SetupUi, places: ZipPlaces): Promise<string[]> {
    const set: string[] = [];
    const config = () => vscode.workspace.getConfiguration('nandwright');
    const finish = () => {
        ui.done(
            [
                set.length > 0 ? `Nandwright is set up: ${set.join('; ')}.` : 'Nothing was changed.',
                'To hand in a project, open one of its files and click Hand In in the status bar, or right-click its folder and choose Zip Project for Hand-In.',
            ].join(' '),
        );
        return set;
    };

    // (a) Course.
    const currentCourse = config().get<string>('course', 'none');
    const currentLabel = presetFor(currentCourse)?.label ?? 'no course preset';
    const course = await ui.pick('Set up Nandwright (1/4): your course', [
        { label: 'No course preset', description: currentCourse === 'none' ? 'current' : undefined, detail: 'You choose the zip\'s name, folders and files yourself.', value: 'none' },
        ...PRESETS.map((preset) => ({
            label: preset.label,
            description: currentCourse === preset.id ? 'current' : undefined,
            detail: 'Names the zip, puts the right folders together, ticks the files the course asks for and adds the grade sheet.',
            value: preset.id,
        })),
        keep(currentLabel),
    ]);
    if (!course) {
        return finish();
    }
    if (course.value !== KEEP) {
        await config().update('course', course.value, vscode.ConfigurationTarget.Global);
        set.push(`course: ${course.label}`);
    }

    // (b) Name, if the zip's name needs it.
    const preset = presetFor(config().get<string>('course', 'none'));
    const own = vscode.workspace.getConfiguration('nandwright.zip').inspect<string>('fileName');
    const pattern = own?.workspaceFolderValue ?? own?.workspaceValue ?? own?.globalValue ?? preset?.fileName ?? '';
    if (/\{familyname\}|\{firstname\}/.test(pattern)) {
        const student = () => vscode.workspace.getConfiguration('nandwright.student');
        for (const [key, label, step] of [
            ['familyName', 'family name', 2],
            ['firstName', 'first name', 2],
        ] as const) {
            const current = student().get<string>(key, '');
            const value = await ui.input(
                `Set up Nandwright (${step}/4): your ${label}`,
                `For the zip's name. Kept in your user settings only.${current ? ' Leave it as it is to keep it.' : ''}`,
                current,
            );
            if (value === undefined) {
                return finish();
            }
            if (value.trim() && value.trim() !== current) {
                await student().update(key, value.trim(), vscode.ConfigurationTarget.Global);
                set.push(`${label}: ${value.trim()}`);
            }
        }
    }

    // (c) Grade sheets.
    const settingFolder = vscode.workspace.getConfiguration('nandwright.zip').get<string>('gradeSheetFolder', '');
    const now = sheetsNow(places, settingFolder);
    const sheets = await ui.pick(`Set up Nandwright (3/4): grade sheets (now: ${now.text})`, [
        { label: '$(cloud-download) Choose the sheets I downloaded…', detail: 'They are copied into a folder named grade-sheets in your workspace. A sheet already there is never replaced without asking.', value: 'copy' },
        { label: '$(folder) They are already in a folder…', detail: 'Choose the folder; the hand-in takes each project\'s sheet from it.', value: 'folder' },
        { label: '$(circle-slash) My course has none / skip', value: KEEP },
    ]);
    if (!sheets) {
        return finish();
    }
    if (sheets.value === 'copy') {
        const files = await ui.pickFiles('Choose the grade sheets you downloaded');
        if (files.length > 0) {
            let folder = gradeSheetHome(places);
            if (!folder) {
                folder = await ui.pickFolder('Choose a folder to keep your grade sheets in');
                if (folder) {
                    await vscode.workspace.getConfiguration('nandwright.zip').update('gradeSheetFolder', folder, vscode.ConfigurationTarget.Global);
                }
            }
            if (folder) {
                const result = await copyGradeSheets(files, folder, (name) => ui.confirmReplace(name));
                if (result.copied.length > 0) {
                    set.push(`grade sheets copied to ${folder}: ${result.copied.join(', ')}`);
                }
                if (result.kept.length > 0) {
                    set.push(`kept your own ${result.kept.join(', ')}`);
                }
            }
        }
    } else if (sheets.value === 'folder') {
        const folder = await ui.pickFolder('Choose the folder that holds your grade sheets');
        if (folder) {
            const root = places.root ?? places.roots[0];
            const relative = root ? path.relative(root, folder) : '..';
            const inside = !relative.startsWith('..') && !path.isAbsolute(relative);
            const zip = vscode.workspace.getConfiguration('nandwright.zip');
            if (inside) {
                await zip.update('gradeSheetFolder', relative.split(path.sep).join('/') || '.', vscode.ConfigurationTarget.Workspace);
            } else {
                await zip.update('gradeSheetFolder', folder, vscode.ConfigurationTarget.Global);
            }
            set.push(`grade sheets from ${folder}`);
        }
    }

    // (d) Tests on save.
    const onSave = config().get<boolean>('test.runOnSave', true);
    const save = await ui.pick('Set up Nandwright (4/4): run a chip\'s tests when you save it?', [
        { label: 'Yes', description: onSave ? 'current' : undefined, value: 'yes' },
        { label: 'No', description: onSave ? undefined : 'current', value: 'no' },
        keep(onSave ? 'yes' : 'no'),
    ]);
    if (save && save.value !== KEEP && (save.value === 'yes') !== onSave) {
        await config().update('test.runOnSave', save.value === 'yes', vscode.ConfigurationTarget.Global);
        set.push(`tests on save: ${save.value}`);
    }
    return finish();
}

/** The setup with VS Code's own dialogs. */
export const vscodeSetupUi: SetupUi = {
    async pick(title, choices) {
        return vscode.window.showQuickPick(choices, { title, ignoreFocusOut: true, placeHolder: 'Press Escape to stop here; what you chose so far is kept.' });
    },
    async input(title, prompt, value) {
        return vscode.window.showInputBox({ title, prompt, value, ignoreFocusOut: true });
    },
    async pickFiles(title) {
        const picked = await vscode.window.showOpenDialog({ title, canSelectFiles: true, canSelectFolders: false, canSelectMany: true, filters: SPREADSHEETS, openLabel: 'Use These Sheets' });
        return (picked ?? []).filter((uri) => uri.scheme === 'file').map((uri) => uri.fsPath);
    },
    async pickFolder(title) {
        const picked = await vscode.window.showOpenDialog({ title, canSelectFiles: false, canSelectFolders: true, canSelectMany: false, openLabel: 'Choose Folder' });
        return picked?.[0]?.scheme === 'file' ? picked[0].fsPath : undefined;
    },
    confirmReplace,
    done(message) {
        void vscode.window.showInformationMessage(message);
    },
};

/** The workspace folders, for the setup. */
export function setupPlaces(): ZipPlaces {
    const roots = (vscode.workspace.workspaceFolders ?? []).filter((f) => f.uri.scheme === 'file').map((f) => f.uri.fsPath);
    return { ...(roots[0] ? { root: roots[0] } : {}), roots };
}

/**
 * Offers the setup once, as an ordinary notification, and remembers the
 * answer. Never in the integration tests or the screenshots. Returns whether it was shown.
 */
export function offerSetup(context: vscode.ExtensionContext): boolean {
    if (process.env.NANDWRIGHT_TEST_WORKSPACE || process.env.NANDWRIGHT_SCREENSHOTS || context.extensionMode === vscode.ExtensionMode.Test) {
        return false;
    }
    const version = String(context.extension.packageJSON.version ?? '');
    const state = context.globalState.get<OfferState>(OFFER_KEY);
    if (!shouldOffer(state, version)) {
        return false;
    }
    void (async () => {
        const answer = await vscode.window.showInformationMessage(
            'Nandwright: set up your course and hand-in in a few clicks?',
            'Set Up',
            'Not Now',
            "Don't Ask Again",
        );
        const kind = answer === 'Set Up' ? 'setUp' : answer === "Don't Ask Again" ? 'never' : 'notNow';
        await context.globalState.update(OFFER_KEY, afterAnswer(state, kind, version));
        if (kind === 'setUp') {
            await vscode.commands.executeCommand('nandwright.setup');
        }
    })();
    return true;
}
