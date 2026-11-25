// Formatting of pin and register values. Shared by the extension and the
// webview panels, so it must not import `vscode` or Node modules.

import type { Pin } from './runtime/protocol';

export type Radix = 'dec' | 'bin' | 'hex';

export function toBigInt(bits: number | string): bigint {
    try {
        return BigInt(bits);
    } catch {
        return 0n;
    }
}

/**
 * A value as the course tools show it: 16-bit values in decimal are signed
 * (two's complement), narrower ones unsigned.
 */
export function formatValue(bits: number | string, width: number, radix: Radix): string {
    const value = toBigInt(bits);
    switch (radix) {
        case 'bin': {
            const digits = value.toString(2).padStart(width, '0');
            // Groups of four from the right, so 16 bits read as 0000 0000 0000 0101.
            return width > 4 ? digits.replace(/\B(?=(\d{4})+(?!\d))/g, ' ') : digits;
        }
        case 'hex':
            return `0x${value.toString(16).toUpperCase().padStart(Math.ceil(width / 4), '0')}`;
        case 'dec':
            if (width === 16 && value >= 0x8000n) {
                return (value - 0x10000n).toString();
            }
            return value.toString();
    }
}

/**
 * What a student typed into a value field, as the engine reads numbers. In
 * binary view the field shows `0000 0000 0000 0101`, so that is also what
 * comes back after changing one digit: the spaces are dropped and the digits
 * marked as binary. Anything else (`5`, `-1`, `0x1F`, `%B101`) goes as typed.
 */
export function typedValue(text: string, radix: Radix): string {
    const typed = text.trim();
    return radix === 'bin' && /^[01][01 ]*$/.test(typed) ? `0b${typed.replace(/ /g, '')}` : typed;
}

/** `name=value` for a pin, in decimal. */
export function describePin(pin: Pin): string {
    return `${pin.name}=${formatValue(pin.bits, pin.width, 'dec')}`;
}

/** The name of a Hack key code, for messages. */
export function keyName(code: number): string {
    const special: Record<number, string> = {
        128: 'Enter',
        129: 'Backspace',
        130: 'Left',
        131: 'Up',
        132: 'Right',
        133: 'Down',
        134: 'Home',
        135: 'End',
        136: 'Page Up',
        137: 'Page Down',
        138: 'Insert',
        139: 'Delete',
        140: 'Esc',
        32: 'Space',
    };
    if (special[code]) {
        return special[code];
    }
    if (code >= 141 && code <= 152) {
        return `F${code - 140}`;
    }
    if (code > 32 && code < 127) {
        return `'${String.fromCharCode(code)}'`;
    }
    return `key code ${code}`;
}

/**
 * The Hack key code of a browser keyboard event, or `undefined` if the Hack
 * keyboard has no such key.
 */
export function hackKeyCode(event: { key: string }): number | undefined {
    const named: Record<string, number> = {
        Enter: 128,
        Backspace: 129,
        ArrowLeft: 130,
        ArrowUp: 131,
        ArrowRight: 132,
        ArrowDown: 133,
        Home: 134,
        End: 135,
        PageUp: 136,
        PageDown: 137,
        Insert: 138,
        Delete: 139,
        Escape: 140,
    };
    if (event.key in named) {
        return named[event.key];
    }
    const functionKey = /^F(\d{1,2})$/.exec(event.key);
    if (functionKey) {
        const n = Number(functionKey[1]);
        return n >= 1 && n <= 12 ? 140 + n : undefined;
    }
    if (event.key.length === 1) {
        const code = event.key.charCodeAt(0);
        return code >= 32 && code < 127 ? code : undefined;
    }
    return undefined;
}

/**
 * What a cell of a compare file says about a pin `width` bits wide: a number,
 * `any` for `*` (which matches whatever the chip gives), or null when the
 * cell cannot be read as a value. A cell of exactly `width` binary digits is
 * binary; otherwise digits are decimal (negative values in two's complement
 * of that width); otherwise hex digits are hexadecimal.
 */
export function cellValue(cell: string, width: number): bigint | 'any' | null {
    const text = cell.trim();
    if (text === '*') {
        return 'any';
    }
    if (width === 1) {
        return text === '0' ? 0n : text === '1' ? 1n : null;
    }
    if (text.length === width && /^[01]+$/.test(text)) {
        return BigInt(`0b${text}`);
    }
    if (/^-?\d+$/.test(text)) {
        return BigInt.asUintN(width, BigInt(text));
    }
    if (/^[0-9a-fA-F]+$/.test(text)) {
        return BigInt(`0x${text}`);
    }
    return null;
}

/**
 * The cells of a line of an output table, with their spacing: `|` and the
 * cells joined give the line back.
 */
export function rawCells(line: string): string[] {
    const pieces = line.split('|');
    if (pieces[0].trim() === '') {
        pieces.shift();
    }
    if (pieces.length > 0 && pieces[pieces.length - 1].trim() === '') {
        pieces.pop();
    }
    return pieces;
}

/**
 * Whether a raw (untrimmed) output cell matches the expected compare-file cell
 * using the simulator engine's exact rule: same character count, and every
 * character matches unless the expected character is `*` (single-character
 * wildcard).
 */
export function cellMatchesExpected(expectedRawCell: string, actualRawCell: string): boolean {
    if (expectedRawCell.length !== actualRawCell.length) {
        return false;
    }
    for (let i = 0; i < expectedRawCell.length; i++) {
        if (expectedRawCell[i] !== '*' && expectedRawCell[i] !== actualRawCell[i]) {
            return false;
        }
    }
    return true;
}

/**
 * The lines of a compare file as cells, at most `limit` rows after the
 * header, with the line of the file each row is on (from 1, as the test
 * engine counts them: a blank line is a line, but not a row).
 */
export function parseCompare(
    text: string,
    limit: number,
): { header: string[]; rows: string[][]; rawRows: string[][]; lines: number[]; more: number } | null {
    const numbered = text
        .split(/\r?\n/)
        .map((line, i) => ({ line, number: i + 1 }))
        .filter(({ line }) => line.trim() !== '');
    if (numbered.length === 0) {
        return null;
    }
    const cells = (line: string): string[] => rawCells(line).map((piece) => piece.trim());
    const rows = numbered.slice(1);
    const shown = rows.slice(0, limit);
    return {
        header: cells(numbered[0].line),
        rows: shown.map(({ line }) => cells(line)),
        rawRows: shown.map(({ line }) => rawCells(line)),
        lines: shown.map(({ number }) => number),
        more: Math.max(0, rows.length - limit),
    };
}
