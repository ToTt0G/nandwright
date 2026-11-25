// Inspecting one row of a test's compare file in the computer panel: which
// machine state a compare column names, and which columns of a row differ.
// No `vscode` import: the webview and the unit tests use it too.

import type { Expected, InspectedMismatch } from './runtime/protocol';
import { cellMatchesExpected } from './values';

/** The highest RAM address (the keyboard). */
const LAST_ADDRESS = 24576;

const REGISTERS = ['A', 'D', 'PC', 'SP', 'LCL', 'ARG', 'THIS', 'THAT'] as const;
export type RegisterName = (typeof REGISTERS)[number];

/** VM scripts also write `local` and `argument` for LCL and ARG. */
const REGISTER_ALIASES: Record<string, RegisterName> = {
    LOCAL: 'LCL',
    ARGUMENT: 'ARG',
};

export type TargetColumn = { target: 'RAM'; address: number } | { target: 'register'; name: RegisterName };

/**
 * What a compare-file column header names in the machine: a RAM cell
 * (`RAM[261]`), a register (`A`, `D`, `PC`, `SP`, `LCL`/`local`,
 * `ARG`/`argument`, `THIS`, `THAT`; `PC[]` is `PC`), or null for anything
 * else (`time`, `out`, …).
 */
export function parseTargetColumn(header: string): TargetColumn | null {
    const text = header.trim();
    const ram = /^ram\s*\[\s*(\d+)\s*\]$/i.exec(text);
    if (ram) {
        const address = Number(ram[1]);
        return address <= LAST_ADDRESS ? { target: 'RAM', address } : null;
    }
    const register = /^([a-z]+)\s*(?:\[\s*\])?$/i.exec(text);
    if (register) {
        const raw = register[1].toUpperCase();
        const name = REGISTER_ALIASES[raw] ?? raw;
        if ((REGISTERS as readonly string[]).includes(name)) {
            return { target: 'register', name: name as RegisterName };
        }
    }
    return null;
}

/**
 * The columns of row `rowIndex` whose produced cell differs from the expected
 * one, by the engine's rule (same width; `*` matches any character). Missing
 * or extra cells are reported too, and columns that name no RAM cell or
 * register come back with `target: 'other'`.
 */
export function rowMismatches(expected: Expected, actual: string[] | undefined, rowIndex: number): InspectedMismatch[] {
    const expectedRow = expected.rows[rowIndex];
    if (!expectedRow || !actual) {
        return [];
    }
    const rawRow = expected.rawRows?.[rowIndex];
    const mismatches: InspectedMismatch[] = [];
    expected.header.forEach((column, i) => {
        const cell = expectedRow[i] ?? '';
        const rawExpected = rawRow?.[i];
        const rawActual = actual[i];
        if (rawActual === undefined) {
            const parsed = parseTargetColumn(column);
            const base = { column, expected: cell, actual: '(missing)' };
            if (parsed?.target === 'RAM') {
                mismatches.push({ ...base, target: 'RAM', address: parsed.address });
            } else if (parsed?.target === 'register') {
                mismatches.push({ ...base, target: 'register', name: parsed.name });
            } else {
                mismatches.push({ ...base, target: 'other' });
            }
            return;
        }
        const differs =
            rawExpected !== undefined
                ? !cellMatchesExpected(rawExpected, rawActual)
                : !/^\*+$/.test(cell) && rawActual.trim() !== cell;
        if (!differs) {
            return;
        }
        const parsed = parseTargetColumn(column);
        const base = { column, expected: cell, actual: rawActual.trim() };
        if (parsed?.target === 'RAM') {
            mismatches.push({ ...base, target: 'RAM', address: parsed.address });
        } else if (parsed?.target === 'register') {
            mismatches.push({ ...base, target: 'register', name: parsed.name });
        } else {
            mismatches.push({ ...base, target: 'other' });
        }
    });
    for (let i = expected.header.length; i < actual.length; i++) {
        mismatches.push({
            column: `column ${i + 1}`,
            target: 'other',
            expected: '(none)',
            actual: actual[i].trim(),
        });
    }
    return mismatches;
}

/** A memory window start that shows `address` near the top. */
export function memoryStartForAddress(address: number, windowSize = 16): number {
    if (address < windowSize) {
        return 0;
    }
    return Math.max(0, Math.min(LAST_ADDRESS - windowSize + 1, address - 2));
}

/** The RAM address the memory window should show for an inspected row, if any. */
export function inspectedAddress(expected: Expected, mismatches: InspectedMismatch[]): number | null {
    const differing = mismatches.find((m) => m.target === 'RAM' && m.address !== undefined);
    if (differing?.address !== undefined) {
        return differing.address;
    }
    for (const column of expected.header) {
        const parsed = parseTargetColumn(column);
        if (parsed?.target === 'RAM') {
            return parsed.address;
        }
    }
    return null;
}
