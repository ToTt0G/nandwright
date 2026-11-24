// The name Zip Project Files offers for the zip, from the
// `nandwright.zip.fileName` setting. No VS Code here, so it can be unit tested.

/** Characters Windows does not allow in a file name, and control characters. */
// eslint-disable-next-line no-control-regex
const NOT_ALLOWED = /[<>:"/\\|?*\u0000-\u001f]/g;

/**
 * Expands the placeholders of a zip name, without `.zip`:
 * `{folder}` the folder's name, `{n}` the number in it (its last run of
 * digits, without leading zeros), `{nn}` that number with at least two digits.
 * A folder name without a number gives the folder's name for `{n}` and `{nn}`.
 * Characters not allowed in file names are removed; if nothing is left, the
 * folder's name is used.
 */
export function zipFileName(pattern: string, folder: string): string {
    const digits = folder.match(/(\d+)(?!.*\d)/)?.[1];
    const n = digits === undefined ? folder : String(Number.parseInt(digits, 10));
    const nn = digits === undefined ? folder : n.padStart(2, '0');
    const expanded = pattern.replace(/\{(folder|nn|n)\}/g, (_, key: string) => (key === 'folder' ? folder : key === 'nn' ? nn : n));
    const clean = (text: string) =>
        text
            .replace(NOT_ALLOWED, '')
            .replace(/\.zip$/i, '')
            .trim()
            .replace(/[. ]+$/, '');
    return clean(expanded) || clean(folder) || 'project';
}
