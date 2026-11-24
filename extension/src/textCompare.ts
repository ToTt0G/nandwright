// How "Compare with Reference Output" compares a file with the reference: the
// rule of the official TextComparer. No VS Code here, so it can be unit tested.

/**
 * The lines of a text as they are compared: line endings normalised, spaces
 * and tabs inside each line ignored (so indentation and `<a> x </a>` versus
 * `<a>x</a>` do not count), and trailing blank lines dropped. Blank lines in
 * the middle still count, as they do for the TextComparer.
 */
export function comparedLines(text: string): string[] {
    return text
        .replace(/\r\n?/g, '\n')
        .replace(/\s+$/, '')
        .split('\n')
        .map((line) => line.replace(/[ \t]+/g, ''));
}
