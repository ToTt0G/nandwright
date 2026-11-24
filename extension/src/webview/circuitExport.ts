// Sizing and checking a PNG export of the circuit diagram. Pure (no DOM), so
// it is unit-tested.

/** No side of the canvas may be longer than this (Chromium's limit is 32,767; this leaves room). */
export const MAX_CANVAS_SIDE = 16_384;
/** Nor may it have more pixels than this (2^28, a quarter of Chromium's limit). */
export const MAX_CANVAS_AREA = 2 ** 28;

export interface PngSize {
    width: number;
    height: number;
    /** Canvas pixels per diagram unit: 2 unless the diagram is too large for that. */
    scale: number;
}

/**
 * The canvas for a `width` × `height` drawing at `scale` (2×), scaled down
 * so that neither side is over `MAX_CANVAS_SIDE` and the area is under
 * `MAX_CANVAS_AREA`. Null when the drawing has no size.
 */
export function pngSize(width: number, height: number, scale = 2): PngSize | null {
    if (!(width > 0) || !(height > 0) || !Number.isFinite(width) || !Number.isFinite(height)) {
        return null;
    }
    let k = scale;
    k = Math.min(k, MAX_CANVAS_SIDE / width, MAX_CANVAS_SIDE / height, Math.sqrt(MAX_CANVAS_AREA / (width * height)));
    let w = Math.max(1, Math.floor(width * k));
    let h = Math.max(1, Math.floor(height * k));
    // A drawing thinner than a pixel at that scale: keep the side limits.
    w = Math.min(w, MAX_CANVAS_SIDE);
    h = Math.min(h, MAX_CANVAS_SIDE);
    return { width: w, height: h, scale: k };
}

const PNG_SIGNATURE = [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];

/**
 * Whether `base64` is PNG data: a canvas too large to paint gives
 * `data:,` (nothing) rather than an error.
 */
export function isPngBase64(base64: string): boolean {
    if (base64.length < 16 || !/^[A-Za-z0-9+/]+=*$/.test(base64.slice(0, 64))) {
        return false;
    }
    const head = atob(base64.slice(0, 12));
    return PNG_SIGNATURE.every((byte, i) => head.charCodeAt(i) === byte);
}
