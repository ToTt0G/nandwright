// The name Zip Project Files offers for the zip, from the
// `nandwright.zip.fileName` setting. No VS Code here, so it can be unit tested.

/** Characters Windows does not allow in a file name, and control characters. */
// eslint-disable-next-line no-control-regex
const NOT_ALLOWED = /[<>:"/\\|?*\u0000-\u001f]/g;

/** The project number in a folder's name: its last run of digits, without leading zeros. */
export function folderNumber(folder: string): string | undefined {
    const digits = folder.match(/(\d+)(?!.*\d)/)?.[1];
    return digits === undefined ? undefined : String(Number.parseInt(digits, 10));
}

/**
 * Expands `{folder}`, `{n}`, `{nn}`, `{familyname}` and `{firstname}` in a pattern. `{n}` is the project
 * number: `project` when given (from `nandwright.zip.projects`), otherwise the
 * last number in the folder's name, without leading zeros; `{nn}` is that
 * number with at least two digits. Without a number, `{n}` and `{nn}` give
 * the folder's name; a project key that is not a number is used as it is.
 */
export function expandPlaceholders(pattern: string, folder: string, project?: string, names: StudentName = {}): string {
    const number = project === undefined ? folderNumber(folder) : /^\d+$/.test(project) ? String(Number.parseInt(project, 10)) : undefined;
    const n = number ?? project ?? folder;
    const nn = number === undefined ? n : number.padStart(2, '0');
    const values: Record<string, string> = {
        folder,
        n,
        nn,
        familyname: names.familyName?.trim() ?? '',
        firstname: names.firstName?.trim() ?? '',
    };
    return pattern.replace(/\{(folder|nn|n|familyname|firstname)\}/g, (_, key: string) => values[key]);
}

/** The student's name, from `nandwright.student.familyName` and `.firstName`. */
export interface StudentName {
    familyName?: string;
    firstName?: string;
}

/** The parts of the student's name a pattern uses but that are not set. */
export function missingNames(pattern: string, names: StudentName): Array<keyof StudentName> {
    const missing: Array<keyof StudentName> = [];
    if (/\{familyname\}/.test(pattern) && !names.familyName?.trim()) {
        missing.push('familyName');
    }
    if (/\{firstname\}/.test(pattern) && !names.firstName?.trim()) {
        missing.push('firstName');
    }
    return missing;
}

/**
 * The zip's name without `.zip`, from the `nandwright.zip.fileName` pattern
 * (see {@link expandPlaceholders}). Characters not allowed in file names are
 * removed; if nothing is left, the folder's name is used.
 */
export function zipFileName(pattern: string, folder: string, project?: string, names: StudentName = {}): string {
    const expanded = expandPlaceholders(pattern, folder, project, names);
    const clean = (text: string) =>
        text
            .replace(NOT_ALLOWED, '')
            .replace(/\.zip$/i, '')
            .trim()
            .replace(/[. ]+$/, '');
    return clean(expanded) || clean(folder) || 'project';
}
