// The hash the engine reports as `sourceHash` in `chip.diagram`: 64-bit
// FNV-1a of the UTF-8 bytes of the exact source text the build used, as 16
// lowercase hex digits (docs/CIRCUIT_DIAGRAM.md). A panel compares it with
// the text it has now to know whether span offsets still point at the same
// characters. No `vscode` import: unit-tested.

const OFFSET = 0xcbf29ce484222325n;
const PRIME = 0x100000001b3n;
const MASK = (1n << 64n) - 1n;

/** 64-bit FNV-1a of the UTF-8 encoding of `text`, as 16 lowercase hex digits. */
export function sourceHash(text: string): string {
    // Two 32-bit halves keep the loop in plain numbers, which is much faster
    // than BigInt arithmetic per byte.
    const bytes = new TextEncoder().encode(text);
    let hi = Number(OFFSET >> 32n);
    let lo = Number(OFFSET & 0xffffffffn);
    for (let i = 0; i < bytes.length; i++) {
        lo ^= bytes[i];
        // (hi:lo) * 0x100000001b3 mod 2^64 = (hi:lo) * 0x1b3 + (lo << 40)
        const loLo = (lo & 0xffff) * 0x1b3;
        const loHi = (lo >>> 16) * 0x1b3;
        const carry = Math.floor((loLo + ((loHi & 0xffff) << 16 >>> 0)) / 0x100000000);
        const newLo = (loLo + (((loHi & 0xffff) << 16) >>> 0)) >>> 0;
        const newHi = (hi * 0x1b3 + (loHi >>> 16) + carry + ((lo << 8) >>> 0)) >>> 0;
        hi = newHi;
        lo = newLo;
    }
    return ((BigInt(hi) << 32n) | BigInt(lo)).toString(16).padStart(16, '0');
}

/** The same, with BigInt throughout: the reference the fast version is tested against. */
export function sourceHashReference(text: string): string {
    let hash = OFFSET;
    for (const byte of new TextEncoder().encode(text)) {
        hash = ((hash ^ BigInt(byte)) * PRIME) & MASK;
    }
    return hash.toString(16).padStart(16, '0');
}
