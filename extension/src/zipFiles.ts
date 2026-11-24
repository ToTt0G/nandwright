// The files Zip Project Files offers and writes: a folder's files, or the
// folders of one project from `nandwright.zip.projects`, and the project's
// grade sheet. No VS Code here, so it can be unit tested.

import * as fs from 'node:fs';
import * as path from 'node:path';
import { zip } from './zip';
import { CoursePreset, matchesPattern } from './presets';
import { StudentName, folderNumber, missingNames, zipFileName } from './zipName';

/** Folders never entered: tools, caches and build output. */
const SKIPPED_FOLDERS = new Set(['.git', '.vscode', 'node_modules', 'target', '__pycache__', 'bin', 'obj', 'build', 'dist']);
export const MAX_FILES = 2000;
export const MAX_BYTES = 100 * 1024 * 1024;

/** Folders of another system's bookkeeping, never entered. */
const CLUTTER_FOLDERS = new Set(['__macosx']);
/** Files of another program's bookkeeping, never offered: Office lock files and Windows folder settings. */
const CLUTTER_FILES = /^(~\$.*|thumbs\.db|desktop\.ini)$/i;

const COURSE_TEST = /\.(tst|cmp)$/i;
const GENERATED = /\.(out|hack|vm|xml|zip|class|pyc|o|exe)$/i;

export interface ProjectFile {
    /** The name in the zip: relative to the folder, with `/` separators. */
    path: string;
    /** Where the file is, for a file added from another folder. */
    source?: string;
    size: number;
    /** Ticked unless it is generated or a course test file. */
    picked: boolean;
    /** Why it is not ticked, or where it is from. */
    description?: string;
    /** The project's grade sheet. */
    gradeSheet?: boolean;
}

/** The files under a folder, as the list offers them, or how many there are when there are too many. */
export async function projectFiles(folder: string): Promise<{ files: ProjectFile[]; tooLarge: boolean; bytes: number }> {
    const files: ProjectFile[] = [];
    let bytes = 0;
    const visit = async (dir: string, prefix: string) => {
        const entries = await fs.promises.readdir(dir, { withFileTypes: true });
        entries.sort((a, b) => (a.name < b.name ? -1 : a.name > b.name ? 1 : 0));
        for (const entry of entries) {
            // Hidden files and folders are tool settings, not work to hand in.
            if (entry.name.startsWith('.')) {
                continue;
            }
            const full = path.join(dir, entry.name);
            const relative = prefix ? `${prefix}/${entry.name}` : entry.name;
            if (entry.isDirectory()) {
                if (!SKIPPED_FOLDERS.has(entry.name) && !CLUTTER_FOLDERS.has(entry.name.toLowerCase()) && !isGradeSheetFolderName(entry.name)) {
                    await visit(full, relative);
                }
            } else if (entry.isFile()) {
                if (CLUTTER_FILES.test(entry.name)) {
                    continue;
                }
                const size = (await fs.promises.stat(full)).size;
                bytes += size;
                const description = COURSE_TEST.test(entry.name)
                    ? 'test file from the course'
                    : GENERATED.test(entry.name)
                      ? 'generated'
                      : undefined;
                files.push({ path: relative, size, picked: description === undefined, ...(description ? { description } : {}) });
            }
        }
    };
    await visit(folder, '');
    for (const file of files) {
        if (file.picked && translatedAssembly(file.path, path.basename(folder), files)) {
            file.picked = false;
            file.description = 'generated';
        }
    }
    return { files, tooLarge: files.length > MAX_FILES || bytes > MAX_BYTES, bytes };
}

/**
 * Whether an `.asm` file is a VM translator's output: `Prog.asm` next to
 * `Prog.vm`, or `Dir/Dir.asm` in a folder of `.vm` files. Hand-written
 * assembly has no `.vm` file of its name.
 */
function translatedAssembly(file: string, folderName: string, files: ProjectFile[]): boolean {
    if (!/\.asm$/i.test(file)) {
        return false;
    }
    const slash = file.lastIndexOf('/');
    const dir = slash < 0 ? '' : file.slice(0, slash);
    const stem = file.slice(slash + 1, -'.asm'.length).toLowerCase();
    const dirName = (slash < 0 ? folderName : dir.slice(dir.lastIndexOf('/') + 1)).toLowerCase();
    const prefix = dir ? `${dir}/`.toLowerCase() : '';
    return files.some((other) => {
        const name = other.path.toLowerCase();
        if (!name.startsWith(prefix) || name.slice(prefix.length).includes('/') || !name.endsWith('.vm')) {
            return false;
        }
        return name.slice(prefix.length, -'.vm'.length) === stem || dirName === stem;
    });
}

/**
 * Files from other folders, as the list adds them: ticked, at the root of
 * the zip under their own names. A name already at the root of the list is
 * refused, ignoring case as Windows does.
 */
export async function addedFiles(listed: ProjectFile[], sources: string[]): Promise<{ added: ProjectFile[]; refused: string[] }> {
    const taken = new Set(listed.filter((file) => !file.path.includes('/')).map((file) => file.path.toLowerCase()));
    const added: ProjectFile[] = [];
    const refused: string[] = [];
    for (const source of sources) {
        const name = path.basename(source);
        if (taken.has(name.toLowerCase())) {
            refused.push(name);
            continue;
        }
        taken.add(name.toLowerCase());
        const size = (await fs.promises.stat(source)).size;
        added.push({ path: name, source, size, picked: true, description: 'added' });
    }
    return { added, refused };
}

/**
 * Writes the chosen files to a zip file: a folder's files at their relative
 * paths, and added files (with a `source`) under their `path`.
 */
export async function writeProjectZip(folder: string, files: Array<string | Pick<ProjectFile, 'path' | 'source'>>, target: string): Promise<void> {
    const entries = [];
    for (const file of files) {
        const name = typeof file === 'string' ? file : file.path;
        const full = typeof file !== 'string' && file.source ? file.source : path.join(folder, ...name.split('/'));
        const [data, stat] = await Promise.all([fs.promises.readFile(full), fs.promises.stat(full)]);
        entries.push({ name, data, mtime: stat.mtime });
    }
    await fs.promises.writeFile(target, zip(entries));
}


/**
 * What Zip Project Files reads from the settings. A value left out was not
 * set by the student: the course preset's value, or the default, applies.
 */
export interface ZipSettings {
    /** `nandwright.zip.fileName`. */
    fileName?: string;
    /** `nandwright.zip.projects`: project number to the folders that make it up. */
    projects?: unknown;
    /** `nandwright.zip.gradeSheetFolder`: the student's folder of grade sheets. */
    gradeSheetFolder?: string;
    /** `nandwright.course`, as its preset. */
    preset?: CoursePreset;
    /** `nandwright.student.familyName` and `.firstName`. */
    names?: StudentName;
}

/** Where to look: the workspace folder holding the project, and all workspace folders. */
export interface ZipPlaces {
    root?: string;
    roots: string[];
}

/** What the list offers for one run of Zip Project Files. */
export interface ZipPlan {
    /** "Zip project 1 (01, 02)" or "Zip 01". */
    title: string;
    /** The folders zipped, as they exist. */
    folders: string[];
    /** The zip's name, without `.zip`. */
    name: string;
    /** The project's key in `nandwright.zip.projects`, when the folder is in it. */
    project?: string;
    /** The zip name's pattern, before the placeholders are filled in. */
    pattern: string;
    /** Parts of the student's name the pattern needs that are not set. */
    missingNames: Array<keyof StudentName>;
    files: ProjectFile[];
    /** Things the student should know: a missing folder, a grade sheet not found. */
    notes: string[];
    /** The project number whose grade sheet a course preset expects but was not found. */
    missingSheet?: string;
    /** The project number, if any. */
    number?: string;
    tooLarge: boolean;
    bytes: number;
}

const same = (a: string, b: string) => a.toLowerCase() === b.toLowerCase();
const samePath = (a: string, b: string) => same(path.resolve(a), path.resolve(b));

/**
 * The entry of `nandwright.zip.projects` that names `folder`: its key and
 * the folders it lists, in order. A plain name is a folder next to `folder`
 * (matched ignoring case); a name with `/` is relative to the workspace
 * folder (`base`).
 */
export function findProject(projects: unknown, folder: string, base?: string): { key: string; folders: string[] } | undefined {
    if (!projects || typeof projects !== 'object' || Array.isArray(projects)) {
        return undefined;
    }
    const parent = path.dirname(folder);
    for (const [key, value] of Object.entries(projects as Record<string, unknown>)) {
        const names = (Array.isArray(value) ? value : [value]).filter((v): v is string => typeof v === 'string' && v.trim() !== '');
        const resolved = names.map((name) => {
            const trimmed = name.trim();
            if (/[\\/]/.test(trimmed)) {
                const full = path.resolve(base ?? parent, trimmed);
                return { full, matches: samePath(full, folder) };
            }
            return { full: path.join(parent, trimmed), matches: same(trimmed, path.basename(folder)) };
        });
        if (resolved.some((r) => r.matches)) {
            const folders: string[] = [];
            for (const r of resolved) {
                const full = r.matches ? folder : r.full;
                if (!folders.some((f) => samePath(f, full))) {
                    folders.push(full);
                }
            }
            return { key: key.trim(), folders };
        }
    }
    return undefined;
}

/**
 * One list from the files of several folders: each file at its path in its
 * own folder, so flat folders give a flat zip. When two folders have the
 * same path (ignoring case), each copy goes under its folder's name.
 * With one folder the files are returned as they are.
 */
export function mergeFolders(parts: Array<{ name: string; root: string; files: ProjectFile[] }>): ProjectFile[] {
    if (parts.length === 1) {
        return parts[0].files;
    }
    const owners = new Map<string, string[]>();
    for (const part of parts) {
        for (const file of part.files) {
            const key = file.path.toLowerCase();
            owners.set(key, [...(owners.get(key) ?? []), part.name]);
        }
    }
    const merged: ProjectFile[] = [];
    for (const part of parts) {
        for (const file of part.files) {
            const others = (owners.get(file.path.toLowerCase()) ?? []).filter((name) => name !== part.name);
            const notes = [part.name];
            if (others.length > 0) {
                notes.push(`under ${part.name}/: ${others.join(', ')} has the same file`);
            }
            if (file.description) {
                notes.push(file.description);
            }
            merged.push({
                ...file,
                path: others.length > 0 ? `${part.name}/${file.path}` : file.path,
                source: file.source ?? path.join(part.root, ...file.path.split('/')),
                description: notes.join(' \u00b7 '),
            });
        }
    }
    return merged;
}

/** A grade sheet's name for project `n`: `Project1.xlsx`, `project_01.ods`, `PROJECT-1.xls`. */
export function isGradeSheetName(name: string, n: string): boolean {
    const number = String(Number.parseInt(n, 10));
    return new RegExp(`^project[ _-]?0*${number}\\.(xlsx|xls|ods)$`, 'i').test(name);
}

async function sheetsIn(dir: string, n: string): Promise<string[]> {
    try {
        const entries = await fs.promises.readdir(dir, { withFileTypes: true });
        return entries.filter((e) => e.isFile() && isGradeSheetName(e.name, n)).map((e) => path.join(dir, e.name));
    } catch {
        return [];
    }
}

async function subfolders(dir: string): Promise<string[]> {
    try {
        const entries = await fs.promises.readdir(dir, { withFileTypes: true });
        return entries
            .filter((e) => e.isDirectory() && !e.name.startsWith('.') && !SKIPPED_FOLDERS.has(e.name))
            .map((e) => path.join(dir, e.name))
            .sort();
    } catch {
        return [];
    }
}

/** Whether a folder is a folder of grade sheets: `grade-sheets`, `Grade Sheets`, `grade_sheets`. */
export function isGradeSheetFolderName(name: string): boolean {
    return name.toLowerCase().replace(/[\s_-]+/g, '') === 'gradesheets';
}

async function gradeSheetFolders(dirs: string[]): Promise<string[]> {
    const found: string[] = [];
    for (const dir of dirs) {
        try {
            for (const entry of await fs.promises.readdir(dir, { withFileTypes: true })) {
                if (entry.isDirectory() && isGradeSheetFolderName(entry.name)) {
                    found.push(path.join(dir, entry.name));
                }
            }
        } catch {
            // not there
        }
    }
    return found;
}

/**
 * Searches for project `n`'s grade sheet, in this order: the project's
 * folders; the student's folder of grade sheets (`settingFolder`); a
 * `grade-sheets` folder in a workspace folder or beside the project's
 * folders; the folders above the project's folders; the workspace folders;
 * the folders directly inside the workspace folders. The first place with
 * exactly one sheet wins; a place with several gives none and a note.
 */
export async function findGradeSheet(
    n: string,
    folders: string[],
    places: ZipPlaces,
    settingFolder?: string,
): Promise<{ file?: string; inProject?: boolean; note?: string }> {
    const parents = folders.map((f) => path.dirname(f));
    const seen: string[] = [];
    const fresh = (dirs: string[]) => {
        const out: string[] = [];
        for (const dir of dirs) {
            if (!seen.some((s) => samePath(s, dir))) {
                seen.push(dir);
                out.push(dir);
            }
        }
        return out;
    };
    const search = async (dirs: string[], inProject: boolean) => {
        const found = (await Promise.all(fresh(dirs).map((dir) => sheetsIn(dir, n)))).flat();
        if (found.length === 1) {
            return { file: found[0], inProject };
        }
        if (found.length > 1) {
            return {
                note: `Several grade sheets for project ${n}: ${found.map((f) => path.basename(f)).join(', ')} in ${[...new Set(found.map((f) => path.dirname(f)))].join(', ')}. None was added; add the right one with +.`,
            };
        }
        return undefined;
    };
    const steps: Array<() => Promise<string[]>> = [
        async () => folders,
        async () => (settingFolder ? [settingFolder] : []),
        () => gradeSheetFolders([...places.roots, ...parents]),
        async () => parents,
        async () => places.roots,
        async () => (await Promise.all(places.roots.map(subfolders))).flat(),
    ];
    for (const [i, step] of steps.entries()) {
        const result = await search(await step(), i === 0);
        if (result) {
            return result;
        }
    }
    return {};
}

/** Where the student keeps grade sheets: the setting's folder, else `grade-sheets` in the workspace folder. */
export function gradeSheetHome(places: ZipPlaces, settingFolder?: string): string | undefined {
    if (settingFolder?.trim()) {
        return path.resolve(places.root ?? places.roots[0] ?? '.', settingFolder.trim());
    }
    const root = places.root ?? places.roots[0];
    return root ? path.join(root, 'grade-sheets') : undefined;
}

/**
 * Copies grade sheets into `folder` (created if needed). A file of the same
 * name there is replaced only if `replace` says so: it may be a sheet the
 * student already filled in.
 */
export async function copyGradeSheets(
    sources: string[],
    folder: string,
    replace: (name: string) => Promise<boolean>,
): Promise<{ copied: string[]; kept: string[] }> {
    await fs.promises.mkdir(folder, { recursive: true });
    const copied: string[] = [];
    const kept: string[] = [];
    for (const source of sources) {
        const name = path.basename(source);
        const target = path.join(folder, name);
        if (samePath(source, target)) {
            continue;
        }
        const exists = await fs.promises.stat(target).then(() => true, () => false);
        if (exists && !(await replace(name))) {
            kept.push(name);
            continue;
        }
        await fs.promises.copyFile(source, target);
        copied.push(name);
    }
    return { copied, kept };
}

/** "grade sheet · grade-sheets · last changed 2026-11-24 09:30". */
export function gradeSheetDescription(where: string, modified: Date): string {
    const pad = (n: number) => String(n).padStart(2, '0');
    const date = `${modified.getFullYear()}-${pad(modified.getMonth() + 1)}-${pad(modified.getDate())} ${pad(modified.getHours())}:${pad(modified.getMinutes())}`;
    return `grade sheet \u00b7 ${where} \u00b7 last changed ${date}`;
}

/** Where a file is, for the list: relative to the workspace folder when it is inside one. */
function placeName(dir: string, places: ZipPlaces): string {
    for (const root of places.roots) {
        const relative = path.relative(root, dir);
        if (!relative.startsWith('..') && !path.isAbsolute(relative)) {
            return relative === '' ? path.basename(root) : relative.split(path.sep).join('/');
        }
    }
    return dir;
}

/**
 * Everything Zip Project Files offers for `folder`: the folders of its
 * project (from `nandwright.zip.projects`, else the folder alone), their
 * files, the project's grade sheet, the zip's name and notes.
 */
export async function planZip(folder: string, settings: ZipSettings, places: ZipPlaces): Promise<ZipPlan> {
    const folderName = path.basename(folder);
    const preset = settings.preset;
    const project = findProject(settings.projects ?? preset?.projects, folder, places.root);
    const notes: string[] = [];
    const folders: string[] = [];
    const missing: string[] = [];
    for (const dir of project?.folders ?? [folder]) {
        const exists = dir === folder || (await fs.promises.stat(dir).then((s) => s.isDirectory(), () => false));
        (exists ? folders : missing).push(dir);
    }
    if (project && missing.length > 0) {
        notes.push(`Not found: ${missing.map((m) => path.basename(m)).join(', ')} (listed for project ${project.key} in nandwright.zip.projects). Zipping the others.`);
    }
    const parts = [];
    let bytes = 0;
    let tooLarge = false;
    for (const dir of folders) {
        const listed = await projectFiles(dir);
        bytes += listed.bytes;
        tooLarge ||= listed.tooLarge;
        parts.push({ name: path.basename(dir), root: dir, files: listed.files });
    }
    const files = mergeFolders(parts);
    tooLarge ||= files.length > MAX_FILES || bytes > MAX_BYTES;

    const number = project ? (/^\d+$/.test(project.key) ? project.key : folderNumber(project.key)) : folderNumber(folderName);
    const key = number === undefined ? undefined : String(Number.parseInt(number, 10));
    const handIn = preset && key !== undefined ? preset.handIn[key] : undefined;
    if (handIn) {
        for (const file of files) {
            const own = file.path.includes('/') && parts.length > 1 ? file.path.slice(file.path.indexOf('/') + 1) : file.path;
            const asked = handIn.some((pattern) => matchesPattern(file.path, pattern) || matchesPattern(own, pattern));
            if (file.picked && !asked) {
                file.description = [file.description, `not handed in for ${preset?.label} project ${key}`].filter(Boolean).join(' \u00b7 ');
            }
            file.picked = asked;
        }
    }
    let missingSheet: string | undefined;
    if (number !== undefined && !tooLarge) {
        const settingFolder = settings.gradeSheetFolder?.trim()
            ? path.resolve(places.root ?? path.dirname(folder), settings.gradeSheetFolder.trim())
            : undefined;
        const found = await findGradeSheet(number, folders, places, settingFolder);
        if (found.note) {
            notes.push(found.note);
        }
        const sheet = found.file;
        if (sheet) {
            const stat = await fs.promises.stat(sheet);
            const where = placeName(path.dirname(sheet), places);
            const listed = found.inProject
                ? files.find((file) => samePath(file.source ?? path.join(folder, ...file.path.split('/')), sheet))
                : undefined;
            if (listed) {
                listed.picked = true;
                listed.gradeSheet = true;
                listed.description = gradeSheetDescription(where, stat.mtime);
            } else if (files.some((file) => !file.path.includes('/') && same(file.path, path.basename(sheet)))) {
                notes.push(`The grade sheet ${sheet} was not added: the zip already has a file named ${path.basename(sheet)}.`);
            } else {
                files.push({
                    path: path.basename(sheet),
                    source: sheet,
                    size: stat.size,
                    picked: true,
                    gradeSheet: true,
                    description: gradeSheetDescription(where, stat.mtime),
                });
            }
        } else if (preset && !found.note) {
            missingSheet = String(Number.parseInt(number, 10));
            notes.push(`No grade sheet for project ${missingSheet} found. Use the sheet button above to add it.`);
        }
    }
    const names = folders.map((dir) => path.basename(dir));
    const pattern = settings.fileName ?? (preset ? ((key !== undefined ? preset.fileNames?.[key] : undefined) ?? preset.fileName) : '{folder}');
    return {
        title: project ? `Zip project ${project.key} (${names.join(', ')})` : `Zip ${folderName}`,
        folders,
        name: zipFileName(pattern, folderName, project?.key, settings.names),
        ...(project ? { project: project.key } : {}),
        pattern,
        missingNames: missingNames(pattern, settings.names ?? {}),
        ...(missingSheet ? { missingSheet } : {}),
        ...(key !== undefined ? { number: key } : {}),
        files,
        notes,
        tooLarge,
        bytes,
    };
}

/**
 * The folder to zip for `dir`: the nearest folder at or above it (up to the
 * workspace folder) that `projects` lists, so zipping `03/a` zips project
 * folder `03`; `dir` itself when none is listed.
 */
export function projectFolder(dir: string, projects: unknown, root?: string): string {
    let current = dir;
    for (let depth = 0; depth < 8; depth++) {
        if (findProject(projects, current, root)) {
            return current;
        }
        const parent = path.dirname(current);
        if (parent === current || (root !== undefined && (samePath(current, root) || path.relative(root, parent).startsWith('..')))) {
            break;
        }
        current = parent;
    }
    return dir;
}
