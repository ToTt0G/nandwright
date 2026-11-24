// Zip Project Files: the project map, merging folders and finding the grade
// sheet. Made-up folders and files in a temporary directory.

import assert from 'node:assert/strict';
import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';
import { after, test } from 'node:test';
import {
    copyGradeSheets,
    findGradeSheet,
    findProject,
    gradeSheetDescription,
    isGradeSheetFolderName,
    isGradeSheetName,
    mergeFolders,
    planZip,
    projectFolder,
    type ProjectFile,
} from '../../zipFiles';
import { PRESETS, matchesPattern } from '../../presets';

const temp = fs.mkdtempSync(path.join(os.tmpdir(), 'nandwright-zip-'));
after(() => fs.rmSync(temp, { recursive: true, force: true }));

let count = 0;
/** A new tree of files: `{ 'a/b.txt': 'text' }`. */
function tree(files: Record<string, string>): string {
    const root = path.join(temp, `t${count++}`);
    for (const [name, text] of Object.entries(files)) {
        fs.mkdirSync(path.dirname(path.join(root, name)), { recursive: true });
        fs.writeFileSync(path.join(root, name), text);
    }
    fs.mkdirSync(root, { recursive: true });
    return root;
}

const settings = (extra: Record<string, unknown> = {}) => ({
    fileName: 'Lab{nn}-familyname-firstname',
    projects: {},
    ...extra,
});
/** The list as rows; a grade sheet's date is left out. */
const rows = (files: ProjectFile[]) => files.map((f) => [f.path, f.picked, (f.description ?? '').replace(/ \u00b7 last changed .*$/, '')]);

test('a folder is found in the project map by name, ignoring case, or by path', () => {
    const projects = { '1': ['gates', 'Adders'], '2': ['labs/memory'], '3': 'cpu', bad: [7] };
    const base = path.join(temp, 'ws');
    const gates = path.join(base, 'course', 'Gates');
    assert.deepEqual(findProject(projects, gates, base), {
        key: '1',
        folders: [gates, path.join(base, 'course', 'Adders')],
    });
    assert.deepEqual(findProject(projects, path.join(base, 'course', 'adders'), base)?.folders, [
        path.join(base, 'course', 'gates'),
        path.join(base, 'course', 'adders'),
    ]);
    assert.equal(findProject(projects, path.join(base, 'labs', 'memory'), base)?.key, '2');
    assert.equal(findProject(projects, path.join(base, 'other', 'memory'), base), undefined);
    assert.equal(findProject(projects, path.join(base, 'cpu'), base)?.key, '3');
    assert.equal(findProject(projects, path.join(base, 'alu'), base), undefined);
    assert.equal(findProject(undefined, gates, base), undefined);
    assert.equal(findProject(['gates'], gates, base), undefined);
});

test('folders merge flat; a path in two folders goes under each folder\'s name', () => {
    const file = (p: string, extra: Partial<ProjectFile> = {}): ProjectFile => ({ path: p, size: 1, picked: true, ...extra });
    const merged = mergeFolders([
        { name: 'one', root: '/r/one', files: [file('A.hdl'), file('notes.txt'), file('A.out', { picked: false, description: 'generated' })] },
        { name: 'two', root: '/r/two', files: [file('B.hdl'), file('Notes.txt'), file('sub/C.hdl')] },
    ]);
    assert.deepEqual(rows(merged), [
        ['A.hdl', true, 'one'],
        ['one/notes.txt', true, 'one · under one/: two has the same file'],
        ['A.out', false, 'one · generated'],
        ['B.hdl', true, 'two'],
        ['two/Notes.txt', true, 'two · under two/: one has the same file'],
        ['sub/C.hdl', true, 'two'],
    ]);
    assert.equal(merged[5].source, path.join('/r/two', 'sub', 'C.hdl'));
    const alone = [file('A.hdl')];
    assert.equal(mergeFolders([{ name: 'one', root: '/r/one', files: alone }]), alone);
});

test('grade sheet names: project, then a number with or without leading zeros', () => {
    for (const name of ['Project1.xlsx', 'project_01.ods', 'PROJECT-001.xls', 'Project 1.xlsx']) {
        assert.ok(isGradeSheetName(name, '1'), name);
    }
    for (const name of ['Project10.xlsx', 'Project1.csv', '~$Project1.xlsx', 'MyProject1.xlsx', 'Project1 (1).xlsx']) {
        assert.ok(!isGradeSheetName(name, '1'), name);
    }
    assert.ok(isGradeSheetName('Project10.xlsx', '10'));
    assert.ok(isGradeSheetName('project05.xlsx', '05'));
});

test('the grade sheet search goes from the project folder outwards; several in one place give none', async () => {
    const root = tree({
        'course/lab3/A.hdl': '',
        'course/lab4/B.hdl': '',
        'course/Project4.xlsx': '',
        'sheets/Project5.xlsx': '',
        'Project6.xlsx': '',
        'course/lab6/Project6.ods': '',
        'course/lab7/x': '',
        'course/Project7.xlsx': '',
        'course/project_07.ods': '',
        'Grade Sheets/Project4.xlsx': '',
        'Grade Sheets/Project8.xlsx': '',
        'course/grade_sheets/project-08.xlsx': '',
        'mine/Project4.xlsx': '',
    });
    const places = { root, roots: [root] };
    const at = (n: string, lab: string, mine?: string) => findGradeSheet(n, [path.join(root, 'course', lab)], places, mine);
    // A grade-sheets folder (any spelling) comes before the folder above the project.
    assert.deepEqual(await at('4', 'lab4'), { file: path.join(root, 'Grade Sheets', 'Project4.xlsx'), inProject: false });
    // The student's own folder comes before that.
    assert.deepEqual(await at('4', 'lab4', path.join(root, 'mine')), { file: path.join(root, 'mine', 'Project4.xlsx'), inProject: false });
    assert.deepEqual(await at('5', 'lab4'), { file: path.join(root, 'sheets', 'Project5.xlsx'), inProject: false });
    // The project's own folder comes first of all.
    assert.deepEqual(await at('6', 'lab6'), { file: path.join(root, 'course', 'lab6', 'Project6.ods'), inProject: true });
    assert.deepEqual(await at('6', 'lab3'), { file: path.join(root, 'Project6.xlsx'), inProject: false });
    const several = await at('7', 'lab7');
    assert.equal(several.file, undefined);
    assert.match(several.note ?? '', /Several grade sheets for project 7: .*Project7\.xlsx.*None was added/);
    // Two grade-sheets folders (in the workspace and beside the project) count as one place.
    assert.match((await at('8', 'lab3')).note ?? '', /Several grade sheets for project 8/);
    assert.deepEqual(await at('9', 'lab3'), {});
});

test('folders of grade sheets: any case, spaces, - or _', () => {
    for (const name of ['grade-sheets', 'Grade Sheets', 'GRADE_SHEETS', 'gradesheets', 'Grade - Sheets']) {
        assert.ok(isGradeSheetFolderName(name), name);
    }
    for (const name of ['grade-sheet', 'sheets', 'my grade sheets']) {
        assert.ok(!isGradeSheetFolderName(name), name);
    }
});

test('a grade-sheets folder inside a project folder is not part of its files', async () => {
    const root = tree({ 'lab2/Ram.hdl': 'x', 'lab2/Grade Sheets/notes.txt': 'n', 'grade-sheets/Project2.xlsx': 's' });
    const plan = await planZip(path.join(root, 'lab2'), settings(), { root, roots: [root] });
    assert.deepEqual(rows(plan.files), [['Ram.hdl', true, ''], ['Project2.xlsx', true, 'grade sheet \u00b7 grade-sheets']]);
});

test('copying grade sheets never replaces one without asking', async () => {
    const downloads = tree({ 'Project1.xlsx': 'new 1', 'Project2.xlsx': 'new 2', 'Project3.xlsx': 'new 3' });
    const home = path.join(tree({ 'grade-sheets/Project1.xlsx': 'filled in 1', 'grade-sheets/Project2.xlsx': 'filled in 2' }), 'grade-sheets');
    const asked: string[] = [];
    const result = await copyGradeSheets(
        ['Project1.xlsx', 'Project2.xlsx', 'Project3.xlsx'].map((name) => path.join(downloads, name)),
        home,
        async (name) => {
            asked.push(name);
            return name === 'Project2.xlsx';
        },
    );
    assert.deepEqual(asked, ['Project1.xlsx', 'Project2.xlsx']);
    assert.deepEqual(result, { copied: ['Project2.xlsx', 'Project3.xlsx'], kept: ['Project1.xlsx'] });
    assert.equal(fs.readFileSync(path.join(home, 'Project1.xlsx'), 'utf8'), 'filled in 1');
    assert.equal(fs.readFileSync(path.join(home, 'Project2.xlsx'), 'utf8'), 'new 2');
    const fresh = path.join(temp, 'new-home', 'grade-sheets');
    assert.deepEqual(await copyGradeSheets([path.join(downloads, 'Project3.xlsx')], fresh, async () => false), { copied: ['Project3.xlsx'], kept: [] });
});

test('a grade sheet is described with where it is and when it last changed', () => {
    assert.equal(gradeSheetDescription('grade-sheets', new Date(2026, 10, 24, 9, 5)), 'grade sheet \u00b7 grade-sheets \u00b7 last changed 2026-11-24 09:05');
});

test('a project of two folders: one flat zip, named by the project number, with its grade sheet', async () => {
    const root = tree({
        'course/g1/And.hdl': 'a',
        'course/g1/And.tst': 't',
        'course/g2/Add.hdl': 'b',
        'course/g2/sub/Half.hdl': 'c',
        'course/g3/Mem.hdl': 'd',
        'course/Project1.xlsx': 'sheet',
        'course/~$Project1.xlsx': 'lock',
        'course/Project2.xlsx': 'other sheet',
    });
    const g2 = path.join(root, 'course', 'g2');
    const plan = await planZip(g2, settings({ projects: { '1': ['g1', 'g2'], '2': ['g3', 'g9'] } }), { root, roots: [root] });
    assert.equal(plan.title, 'Zip project 1 (g1, g2)');
    assert.equal(plan.name, 'Lab01-familyname-firstname');
    assert.deepEqual(plan.notes, []);
    assert.deepEqual(rows(plan.files), [
        ['And.hdl', true, 'g1'],
        ['And.tst', false, 'g1 · test file from the course'],
        ['Add.hdl', true, 'g2'],
        ['sub/Half.hdl', true, 'g2'],
        ['Project1.xlsx', true, 'grade sheet \u00b7 course'],
    ]);
    assert.equal(plan.files[4].source, path.join(root, 'course', 'Project1.xlsx'));

    // A listed folder that is missing is named; the others are zipped.
    const g3 = await planZip(path.join(root, 'course', 'g3'), settings({ projects: { '1': ['g1', 'g2'], '2': ['g3', 'g9'] } }), { root, roots: [root] });
    assert.equal(g3.title, 'Zip project 2 (g3)');
    assert.equal(g3.name, 'Lab02-familyname-firstname');
    assert.match(g3.notes[0], /^Not found: g9 \(listed for project 2/);
    assert.deepEqual(rows(g3.files), [['Mem.hdl', true, ''], ['Project2.xlsx', true, 'grade sheet \u00b7 course']]);
});

test('a folder in no project behaves as before, and finds its sheet from the number in its name', async () => {
    const root = tree({ 'work/lab05/Cpu.hdl': 'x', 'work/lab05/Project5.xlsx': 'sheet', 'work/Project5.xlsx': 'other' });
    const plan = await planZip(path.join(root, 'work', 'lab05'), settings(), { root, roots: [root] });
    assert.equal(plan.title, 'Zip lab05');
    assert.equal(plan.name, 'Lab05-familyname-firstname');
    // The sheet in the folder is already listed: not added twice.
    assert.deepEqual(rows(plan.files), [['Cpu.hdl', true, ''], ['Project5.xlsx', true, 'grade sheet \u00b7 work/lab05']]);
    const plain = await planZip(path.join(root, 'work'), settings(), { root, roots: [root] });
    assert.deepEqual(plain.notes, []);
    assert.equal(plain.files.length, 3);
});

test('the gradeSheetFolder setting is searched after the project folder', async () => {
    const root = tree({ 'p/03/Ram.hdl': 'x', 'marks/Project2.xlsx': 'sheet', 'p/Project2.xlsx': 'beside' });
    const folder = path.join(root, 'p', '03');
    const places = { root, roots: [root] };
    const projects = { '2': ['03'] };
    const found = await planZip(folder, settings({ projects, gradeSheetFolder: 'marks' }), places);
    assert.deepEqual(rows(found.files), [['Ram.hdl', true, ''], ['Project2.xlsx', true, 'grade sheet \u00b7 marks']]);
    const absolute = await planZip(folder, settings({ projects, gradeSheetFolder: path.join(root, 'marks') }), places);
    assert.equal(absolute.files[1].source, path.join(root, 'marks', 'Project2.xlsx'));
    const unset = await planZip(folder, settings({ projects }), places);
    assert.equal(unset.files[1].source, path.join(root, 'p', 'Project2.xlsx'));
});

test('zipping a subfolder of a listed project folder zips the project folder', () => {
    const root = path.join(temp, 'ws2');
    const projects = { '2': ['mem'] };
    assert.equal(projectFolder(path.join(root, 'mem', 'a'), projects, root), path.join(root, 'mem'));
    assert.equal(projectFolder(path.join(root, 'mem'), projects, root), path.join(root, 'mem'));
    assert.equal(projectFolder(path.join(root, 'other', 'a'), projects, root), path.join(root, 'other', 'a'));
    assert.equal(projectFolder(path.join(root, 'other'), {}, root), path.join(root, 'other'));
});

test('hand-in patterns: * within a name, ** across folders, a bare name at the top only', () => {
    assert.ok(matchesPattern('And.hdl', '**/*.hdl'));
    assert.ok(matchesPattern('a/Bit.hdl', '**/*.hdl'));
    assert.ok(matchesPattern('part2/CompilationEngine.py', '**/CompilationEngine.py'));
    assert.ok(matchesPattern('memory.JACK', 'Memory.jack'));
    assert.ok(!matchesPattern('tester/Memory.jack', 'Memory.jack'));
    assert.ok(!matchesPattern('And.hdl.bak', '**/*.hdl'));
    assert.ok(!matchesPattern('a/b.asm', '*.asm'));
});

const kth = PRESETS.find((preset) => preset.id === 'kth-ep121u')!;

test('a course preset: its folders, names and hand-in files; the student\'s own settings win', async () => {
    const root = tree({
        'projects/01/And.hdl': 'a',
        'projects/01/And.tst': 't',
        'projects/01/And.out': 'o',
        'projects/02/Add.hdl': 'b',
        'projects/02/notes.txt': 'n',
        'projects/03/a/Bit.hdl': 'c',
        'projects/Project1.xlsx': 's1',
        'projects/Project_09/CompilationEngine.py': 'p',
        'projects/Project_09/Tokens.py': 'p',
        'projects/Project_09/Project9.xlsx': 's9',
        'projects/Project_09/Test/Main.jack': 'j',
    });
    const places = { root, roots: [root] };
    const names = { familyName: 'familyname', firstName: 'firstname' };
    const one = await planZip(path.join(root, 'projects', '02'), { preset: kth, names }, places);
    assert.equal(one.title, 'Zip project 1 (01, 02)');
    assert.equal(one.name, 'Project01-familyname-firstname');
    assert.deepEqual(rows(one.files), [
        ['And.hdl', true, '01'],
        ['And.out', false, '01 \u00b7 generated'],
        ['And.tst', false, '01 \u00b7 test file from the course'],
        ['Add.hdl', true, '02'],
        ['notes.txt', false, '02 \u00b7 not handed in for KTH EP121U project 1'],
        ['Project1.xlsx', true, 'grade sheet \u00b7 projects'],
    ]);
    // A subfolder of a project folder: the project folder is zipped.
    assert.equal(projectFolder(path.join(root, 'projects', '03', 'a'), kth.projects, root), path.join(root, 'projects', '03'));
    const two = await planZip(path.join(root, 'projects', '03'), { preset: kth, names }, places);
    assert.equal(two.name, 'Project02-familyname-firstname');
    assert.equal(two.missingSheet, '2');
    assert.match(two.notes[0], /^No grade sheet for project 2 found/);
    // Project 9 is named in lower case, and only CompilationEngine.py is ticked.
    const nine = await planZip(path.join(root, 'projects', 'Project_09'), { preset: kth, names }, places);
    assert.equal(nine.name, 'project09-familyname-firstname');
    assert.deepEqual(
        nine.files.filter((f) => f.picked).map((f) => f.path),
        ['CompilationEngine.py', 'Project9.xlsx'],
    );
    // A name not set is asked for.
    assert.deepEqual((await planZip(path.join(root, 'projects', '01'), { preset: kth, names: { firstName: 'x' } }, places)).missingNames, ['familyName']);
    // The student's own settings win over the preset.
    const own = await planZip(path.join(root, 'projects', '02'), { preset: kth, names, fileName: 'Hand-in {nn}', projects: { '7': ['02'] } }, places);
    assert.equal(own.name, 'Hand-in 07');
    assert.equal(own.title, 'Zip project 7 (02)');
    // Without a preset nothing course-specific happens.
    const none = await planZip(path.join(root, 'projects', '02'), { fileName: '{folder}' }, places);
    assert.equal(none.name, '02');
    assert.deepEqual(rows(none.files), [['Add.hdl', true, ''], ['notes.txt', true, '']]);
    assert.equal(none.missingSheet, undefined);
});
