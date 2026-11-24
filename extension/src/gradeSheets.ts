// Grade sheets: choosing the sheets a student downloaded and copying them
// into the workspace's `grade-sheets` folder, where the hand-in finds them.

import * as path from 'node:path';
import * as vscode from 'vscode';
import { ZipPlaces, copyGradeSheets, gradeSheetHome } from './zipFiles';

export const SPREADSHEETS = { 'Spreadsheets': ['xlsx', 'xls', 'ods'] };

/** Asks before replacing a sheet: it may already be filled in. */
export async function confirmReplace(name: string): Promise<boolean> {
    const answer = await vscode.window.showWarningMessage(
        `${name} is already in your grade sheets folder. Replace it? If you have filled it in, your answers there are lost.`,
        { modal: true },
        'Replace',
    );
    return answer === 'Replace';
}

/**
 * Lets the student choose grade sheets and copies them into `grade-sheets`
 * in the workspace folder. With no workspace folder open, the student picks
 * a folder, which is kept in `nandwright.zip.gradeSheetFolder` (user settings).
 * Returns the copied sheets' new paths, or undefined if nothing was chosen.
 */
export async function chooseAndCopyGradeSheets(places: ZipPlaces, title: string): Promise<{ folder: string; copied: string[]; kept: string[] } | undefined> {
    const sources = await vscode.window.showOpenDialog({
        title,
        canSelectFiles: true,
        canSelectFolders: false,
        canSelectMany: true,
        filters: SPREADSHEETS,
        openLabel: 'Use These Sheets',
    });
    const files = (sources ?? []).filter((uri) => uri.scheme === 'file').map((uri) => uri.fsPath);
    if (files.length === 0) {
        return undefined;
    }
    let folder = gradeSheetHome(places);
    if (!folder) {
        const picked = await vscode.window.showOpenDialog({
            title: 'Choose a folder to keep your grade sheets in',
            canSelectFiles: false,
            canSelectFolders: true,
            canSelectMany: false,
            openLabel: 'Keep Sheets Here',
        });
        if (!picked?.[0] || picked[0].scheme !== 'file') {
            return undefined;
        }
        folder = picked[0].fsPath;
        await vscode.workspace
            .getConfiguration('nandwright.zip')
            .update('gradeSheetFolder', folder, vscode.ConfigurationTarget.Global);
    }
    const result = await copyGradeSheets(files, folder, confirmReplace);
    return { folder, copied: result.copied.map((name) => path.join(folder, name)), kept: result.kept };
}
