// The zip writer, read back by a small reader written here (and, where it is
// installed, by Python's zipfile module).

import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { randomBytes } from 'node:crypto';
import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';
import { test } from 'node:test';
import * as zlib from 'node:zlib';
import { ZipLimitError, crc32, zip } from '../../zip';

interface ReadEntry {
    name: string;
    method: number;
    flags: number;
    data: Buffer;
}

/** Reads an archive from its end record and central directory, checking each entry's CRC-32. */
function unzip(archive: Buffer): ReadEntry[] {
    const end = archive.length - 22;
    assert.equal(archive.readUInt32LE(end), 0x06054b50, 'no end record');
    const count = archive.readUInt16LE(end + 10);
    assert.equal(archive.readUInt16LE(end + 8), count);
    const size = archive.readUInt32LE(end + 12);
    let at = archive.readUInt32LE(end + 16);
    assert.equal(at + size, end, 'the central directory does not end at the end record');
    const entries: ReadEntry[] = [];
    for (let i = 0; i < count; i++) {
        assert.equal(archive.readUInt32LE(at), 0x02014b50, 'no central header');
        const flags = archive.readUInt16LE(at + 8);
        const method = archive.readUInt16LE(at + 10);
        const crc = archive.readUInt32LE(at + 16);
        const compressed = archive.readUInt32LE(at + 20);
        const length = archive.readUInt32LE(at + 24);
        const nameLength = archive.readUInt16LE(at + 28);
        const extra = archive.readUInt16LE(at + 30);
        const comment = archive.readUInt16LE(at + 32);
        const offset = archive.readUInt32LE(at + 42);
        const name = archive.toString('utf8', at + 46, at + 46 + nameLength);
        at += 46 + nameLength + extra + comment;

        assert.equal(archive.readUInt32LE(offset), 0x04034b50, `no local header for ${name}`);
        assert.equal(archive.readUInt16LE(offset + 8), method);
        assert.equal(archive.readUInt32LE(offset + 14), crc);
        const localName = archive.readUInt16LE(offset + 26);
        const localExtra = archive.readUInt16LE(offset + 28);
        assert.equal(archive.toString('utf8', offset + 30, offset + 30 + localName), name);
        const start = offset + 30 + localName + localExtra;
        const body = archive.subarray(start, start + compressed);
        const data = method === 8 ? zlib.inflateRawSync(body) : Buffer.from(body);
        assert.equal(data.length, length, `${name} has the wrong length`);
        assert.equal(crc32(data), crc, `${name} has the wrong CRC-32`);
        entries.push({ name, method, flags, data });
    }
    return entries;
}

test('CRC-32 is the standard one', () => {
    assert.equal(crc32(Buffer.from('123456789')), 0xcbf43926);
    assert.equal(crc32(Buffer.alloc(0)), 0);
});

test('a zip of several files reads back with the same names and contents', () => {
    const random = randomBytes(4096);
    const files = [
        { name: 'Empty.hdl', data: Buffer.alloc(0) },
        { name: 'Repeats.hdl', data: Buffer.from('CHIP Repeats {\n'.repeat(200)) },
        { name: 'Noise.bin', data: random },
        { name: 'My Chip é.hdl', data: Buffer.from('// a name with a space and a non-ASCII letter\n') },
        { name: 'sub/deeper/Nested.asm', data: Buffer.from('@0\nD=A\n') },
    ];
    const mtime = new Date(2024, 4, 17, 13, 45, 30);
    const archive = zip(files.map((file) => ({ ...file, mtime })));
    const entries = unzip(archive);
    assert.deepEqual(
        entries.map((entry) => entry.name),
        files.map((file) => file.name),
    );
    entries.forEach((entry, i) => {
        assert.deepEqual(entry.data, files[i].data, entry.name);
        assert.equal(entry.flags & 0x0800, 0x0800, 'names are marked as UTF-8');
    });
    assert.equal(entries[1].method, 8, 'a file that compresses is deflated');
    assert.equal(entries[2].method, 0, 'a file that does not is stored');
    assert.equal(entries[0].method, 0);

    // Another reader agrees, where there is one.
    const python = process.platform === 'linux' ? spawnSync('python3', ['--version']) : null;
    if (python && python.status === 0) {
        const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'nandwright zip '));
        try {
            const file = path.join(dir, 'project.zip');
            fs.writeFileSync(file, archive);
            const listed = spawnSync('python3', ['-c', 'import sys, zipfile; z = zipfile.ZipFile(sys.argv[1]); z.testzip(); print("\\n".join(z.namelist()))', file], {
                encoding: 'utf8',
            });
            assert.equal(listed.status, 0, listed.stderr);
            assert.deepEqual(listed.stdout.trim().split('\n'), files.map((f) => f.name));
            const table = spawnSync('python3', ['-m', 'zipfile', '-l', file], { encoding: 'utf8' });
            assert.equal(table.status, 0, table.stderr);
            for (const f of files) {
                assert.ok(table.stdout.includes(f.name), `python3 -m zipfile -l does not list ${f.name}`);
            }
            assert.ok(table.stdout.includes('2024-05-17 13:45:30'), table.stdout);
        } finally {
            fs.rmSync(dir, { recursive: true, force: true });
        }
    }
});

test('a zip refuses more entries than the format holds', () => {
    const entries = Array.from({ length: 65_536 }, (_, i) => ({ name: `f${i}`, data: Buffer.alloc(0), mtime: new Date() }));
    assert.throws(() => zip(entries), ZipLimitError);
});
