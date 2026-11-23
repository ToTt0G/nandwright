// A small zip writer, for handing in a project folder. No dependency and no
// `vscode`: the unit tests run it in plain Node.
//
// Plain zip (no zip64): it refuses what would not fit rather than write a
// file that other tools cannot read.

import * as zlib from 'node:zlib';

export interface ZipEntry {
    /** The path inside the archive, with `/` separators. */
    name: string;
    data: Uint8Array;
    mtime: Date;
}

const MAX_ENTRIES = 0xffff;
const MAX_SIZE = 0xffffffff;
/** General purpose flag 11: names are UTF-8. */
const UTF8_FLAG = 0x0800;

const CRC_TABLE = (() => {
    const table = new Uint32Array(256);
    for (let n = 0; n < 256; n++) {
        let c = n;
        for (let k = 0; k < 8; k++) {
            c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
        }
        table[n] = c >>> 0;
    }
    return table;
})();

export function crc32(data: Uint8Array): number {
    let crc = 0xffffffff;
    for (let i = 0; i < data.length; i++) {
        crc = CRC_TABLE[(crc ^ data[i]) & 0xff] ^ (crc >>> 8);
    }
    return (crc ^ 0xffffffff) >>> 0;
}

/** MS-DOS date and time in local time, as zip tools show them; 1980 is the earliest it can say. */
function dosDateTime(date: Date): { time: number; date: number } {
    const year = Math.min(Math.max(date.getFullYear(), 1980), 2107);
    return {
        time: (date.getHours() << 11) | (date.getMinutes() << 5) | Math.floor(date.getSeconds() / 2),
        date: ((year - 1980) << 9) | ((date.getMonth() + 1) << 5) | date.getDate(),
    };
}

export class ZipLimitError extends Error {}

/** The bytes of a zip archive holding the entries, in order. */
export function zip(entries: ZipEntry[]): Buffer {
    if (entries.length > MAX_ENTRIES) {
        throw new ZipLimitError(`A zip file holds at most ${MAX_ENTRIES} files.`);
    }
    const locals: Buffer[] = [];
    const centrals: Buffer[] = [];
    let offset = 0;
    for (const entry of entries) {
        const name = Buffer.from(entry.name.replace(/\\/g, '/'), 'utf8');
        const deflated = zlib.deflateRawSync(entry.data);
        // Stored when deflating does not make it smaller.
        const method = deflated.length < entry.data.length ? 8 : 0;
        const body = method === 8 ? deflated : Buffer.from(entry.data);
        if (entry.data.length >= MAX_SIZE || body.length >= MAX_SIZE) {
            throw new ZipLimitError(`${entry.name} is too large for a zip file.`);
        }
        const crc = crc32(entry.data);
        const { time, date } = dosDateTime(entry.mtime);

        const local = Buffer.alloc(30);
        local.writeUInt32LE(0x04034b50, 0);
        local.writeUInt16LE(20, 4);
        local.writeUInt16LE(UTF8_FLAG, 6);
        local.writeUInt16LE(method, 8);
        local.writeUInt16LE(time, 10);
        local.writeUInt16LE(date, 12);
        local.writeUInt32LE(crc, 14);
        local.writeUInt32LE(body.length, 18);
        local.writeUInt32LE(entry.data.length, 22);
        local.writeUInt16LE(name.length, 26);
        local.writeUInt16LE(0, 28);

        const central = Buffer.alloc(46);
        central.writeUInt32LE(0x02014b50, 0);
        central.writeUInt16LE(20, 4);
        central.writeUInt16LE(20, 6);
        central.writeUInt16LE(UTF8_FLAG, 8);
        central.writeUInt16LE(method, 10);
        central.writeUInt16LE(time, 12);
        central.writeUInt16LE(date, 14);
        central.writeUInt32LE(crc, 16);
        central.writeUInt32LE(body.length, 20);
        central.writeUInt32LE(entry.data.length, 24);
        central.writeUInt16LE(name.length, 28);
        // Extra field, comment, disk number, attributes: none.
        central.writeUInt32LE(offset, 42);

        locals.push(local, name, body);
        centrals.push(central, name);
        offset += local.length + name.length + body.length;
        if (offset >= MAX_SIZE) {
            throw new ZipLimitError('The files are too large for a zip file.');
        }
    }
    const directory = Buffer.concat(centrals);
    const end = Buffer.alloc(22);
    end.writeUInt32LE(0x06054b50, 0);
    end.writeUInt16LE(entries.length, 8);
    end.writeUInt16LE(entries.length, 10);
    end.writeUInt32LE(directory.length, 12);
    end.writeUInt32LE(offset, 16);
    return Buffer.concat([...locals, directory, end]);
}
