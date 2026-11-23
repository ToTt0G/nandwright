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
