// Comparing file paths the way the file system holding them does.
//
// On Windows and (usually) macOS, `And.hdl` and `and.hdl` are one file; on
// Linux they are two. Treating them as one everywhere would let an edit to
// one file change what runs for the other.

import * as fs from 'node:fs';
import * as path from 'node:path';

/** Whether file names on this platform ignore case unless a volume says otherwise. */
const PLATFORM_IGNORES_CASE = process.platform === 'win32' || process.platform === 'darwin';

/**
 * A path in a form that is equal for two spellings of the same file on its
 * volume (`c:\x` and `C:/x` on Windows; `demo.asm` and `Demo.asm` on a
 * case-insensitive macOS/Linux volume), and different for distinct files.
 */
export function pathKey(file: string, ignoreCase?: boolean): string {
    const normalized = path.normalize(file).replace(/\\/g, '/');
    const fold = ignoreCase ?? ignoresCase(file);
    return fold ? normalized.toLowerCase() : normalized;
}

export function samePath(a: string, b: string, ignoreCase?: boolean): boolean {
    return pathKey(a, ignoreCase) === pathKey(b, ignoreCase);
}

const byFolder = new Map<string, boolean>();

function swapCase(name: string): string {
    return [...name].map((c) => (c === c.toLowerCase() ? c.toUpperCase() : c.toLowerCase())).join('');
}

/**
 * Whether the folder holding `file` ignores case in file names. The file
 * system is asked (does the name in the other case reach the same file?),
 * because a macOS volume can be case-sensitive and a Linux mount need not be.
 * If that cannot be told, the platform's usual answer is used (without
 * caching a fallback for a file that does not exist yet).
 */
export function ignoresCase(file: string): boolean {
    const folder = path.dirname(file);
    const known = byFolder.get(folder);
    if (known !== undefined) {
        return known;
    }
    let answer = PLATFORM_IGNORES_CASE;
    let probed = false;
    try {
        const name = path.basename(file);
        const other = swapCase(name);
        if (other !== name) {
            const here = fs.statSync(file, { bigint: true });
            probed = true;
            try {
                const there = fs.statSync(path.join(folder, other), { bigint: true });
                answer = here.ino === there.ino && here.dev === there.dev;
            } catch {
                answer = false;
            }
        }
    } catch {
        // The file is not there to ask about: keep the platform's answer.
    }
    if (probed) {
        if (byFolder.size > 256) {
            byFolder.clear();
        }
        byFolder.set(folder, answer);
    }
    return answer;
}
