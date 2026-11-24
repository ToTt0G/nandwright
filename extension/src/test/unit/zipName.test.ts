// The name offered for a project's zip, from the nandwright.zip.fileName setting.

import assert from 'node:assert/strict';
import { test } from 'node:test';
import { zipFileName } from '../../zipName';

test('the default is the folder name', () => {
    assert.equal(zipFileName('{folder}', '01'), '01');
    assert.equal(zipFileName('{folder}', 'project 3'), 'project 3');
});

test('{n} and {nn} take the last number of the folder name', () => {
    assert.equal(zipFileName('Project{nn}-familyname-firstname', '01'), 'Project01-familyname-firstname');
    assert.equal(zipFileName('Project{nn}-familyname-firstname', 'project1'), 'Project01-familyname-firstname');
    assert.equal(zipFileName('Project{n}', '01'), 'Project1');
    assert.equal(zipFileName('P{n}-{nn}', 'course 2026 week 12'), 'P12-12');
    assert.equal(zipFileName('P{nn}', 'p007'), 'P07');
    assert.equal(zipFileName('P{nn}', 'p123'), 'P123');
    assert.equal(zipFileName('P{n}', 'p00'), 'P0');
});

test('without a number, {n} and {nn} are the folder name', () => {
    assert.equal(zipFileName('Project{nn}-x', 'logic'), 'Projectlogic-x');
    assert.equal(zipFileName('{n}', 'logic'), 'logic');
});

test('characters Windows does not allow are removed', () => {
    assert.equal(zipFileName('a<b>c:d"e/f\\g|h?i*j', '01'), 'abcdefghij');
    assert.equal(zipFileName('name\u0007.', '01'), 'name');
    assert.equal(zipFileName('  name.zip', '01'), 'name');
});

test('an empty result falls back to the folder name', () => {
    assert.equal(zipFileName('', '01'), '01');
    assert.equal(zipFileName('???', 'project 1'), 'project 1');
    assert.equal(zipFileName('', '???'), 'project');
});

test('unknown placeholders are left as they are', () => {
    assert.equal(zipFileName('{name}-{nn}', '5'), '{name}-05');
});

test('a project number from nandwright.zip.projects replaces the folder number', () => {
    assert.equal(zipFileName('Project{nn}-familyname-firstname', '03', '2'), 'Project02-familyname-firstname');
    assert.equal(zipFileName('Project{n}-{folder}', '05', '04'), 'Project4-05');
    assert.equal(zipFileName('Hand-in {n}', 'logic', 'gates'), 'Hand-in gates');
});
