import assert from 'node:assert/strict';
import * as path from 'node:path';
import { test } from 'node:test';
import { resultMatchesFile } from '../../resultStore';
import {
    commandFiles,
    compareFilesForScript,
    findExpectedImage,
    hasNoAsmCode,
    hasReferenceSource,
    isForeverScript,
    isValidBasename,
    orderScriptCandidates,
    primaryScriptTarget,
    rankExpectedImages,
    scriptTestsFile,
    scriptUsesSavedFile,
    topLevelLoads,
} from '../../testLinks';
import { cellMatchesExpected } from '../../values';

test('topLevelLoads distinguishes top-level load, bare load, folder load, and ROM32K load', () => {
    assert.deepEqual(topLevelLoads('load Sample.hdl,\noutput-file Sample.out;\n'), {
        files: ['Sample.hdl'],
        folders: [],
        bare: false,
    });
    assert.deepEqual(topLevelLoads('load,\noutput-file Out.out,\ncompare-to Out.cmp;\n'), {
        files: [],
        folders: [],
        bare: true,
    });
    assert.deepEqual(topLevelLoads('load ProgramFolder,\noutput-file Out.out;\n'), {
        files: [],
        folders: ['ProgramFolder'],
        bare: false,
    });
    // Built-in `ROM32K load Prog.hack` is not a top-level load.
    assert.deepEqual(
        topLevelLoads('load Board.hdl,\nROM32K load Prog.hack,\noutput-file Board.out;\n'),
        { files: ['Board.hdl'], folders: [], bare: false },
    );
    // Comments mentioning load do not count, and block comments separate tokens.
    assert.deepEqual(
        topLevelLoads('// load Fake.hdl;\n/* load Other.asm; */\nload/* gap */Real.asm;\n'),
        { files: ['Real.asm'], folders: [], bare: false },
    );
});

test('echo strings never trigger forever detection or fake compare-to', () => {
    const script = [
        'load Double.hack,',
        'output-file Double.out,',
        'compare-to Real.cmp,',
        'echo "repeat { this is just a message }",',
        'echo "compare-to Fake.cmp";',
        'repeat 10 { ticktock; }',
        'output;',
    ].join('\n');
    assert.equal(isForeverScript(script), false);
    assert.deepEqual(commandFiles(script, 'compare-to'), ['Real.cmp']);
});

test('matches lowercase .asm to scripts loading capitalised .hack on case-sensitive systems', () => {
    const script = 'load Double.hack,\noutput-file Double.out,\ncompare-to Double.cmp;\n';
    const entries = ['double.asm', 'Double.tst', 'Double.cmp'];
    assert.equal(scriptTestsFile(script, 'double.asm', entries, false), true);
    assert.equal(scriptUsesSavedFile(script, 'Double.tst', 'double.asm', entries, false), true);
    assert.equal(primaryScriptTarget(script, entries, false), 'double.asm');
});

test('matches automated script and identifies forever demonstration scripts', () => {
    const demo = 'load Flash.hack;\nrepeat {\n  ticktock;\n}\n';
    const auto = 'load Flash.hack,\noutput-file FlashAutomatic.out,\ncompare-to FlashAutomatic.cmp;\nrepeat 100 { ticktock; }\noutput;\n';
    const entries = ['Flash.asm', 'Flash.tst', 'FlashAutomatic.tst', 'FlashAutomatic.cmp'];
    assert.equal(isForeverScript(demo), true);
    assert.equal(isForeverScript(auto), false);
    assert.equal(scriptTestsFile(auto, 'Flash.asm', entries, false), true);
    assert.equal(scriptUsesSavedFile(auto, 'FlashAutomatic.tst', 'Flash.asm', entries, false), true);
    assert.equal(primaryScriptTarget(auto, entries, false), 'Flash.asm');
});

test('keeps .asm CPU scripts and .vm VM scripts separate in the same folder', () => {
    const cpuScript = 'load StackOps.asm,\noutput-file StackOps.out,\ncompare-to StackOps.cmp;\n';
    const vmScript = 'load StackOps.vm,\noutput-file StackOps.out,\ncompare-to StackOps.cmp;\n';
    const entries = ['StackOps.asm', 'StackOps.vm', 'StackOps.tst', 'StackOpsVME.tst', 'StackOps.cmp'];
    assert.equal(scriptTestsFile(cpuScript, 'StackOps.asm', entries, false), true);
    assert.equal(scriptTestsFile(cpuScript, 'StackOps.vm', entries, false), false);
    assert.equal(scriptTestsFile(vmScript, 'StackOps.vm', entries, false), true);
    assert.equal(scriptTestsFile(vmScript, 'StackOps.asm', entries, false), false);
    assert.equal(scriptUsesSavedFile(cpuScript, 'StackOps.tst', 'StackOps.vm', entries, false), false);
    assert.equal(scriptUsesSavedFile(vmScript, 'StackOpsVME.tst', 'StackOps.vm', entries, false), true);
});

test('bare VM folder load and subfolder load match .vm and .jack files cleanly', () => {
    const script = 'load,\noutput-file CalcTest.out,\ncompare-to CalcTest.cmp;\nrepeat 1000 { vmstep; }\noutput;\n';
    const jackEntries = ['Main.jack', 'Calc.jack', 'CalcTest.tst', 'CalcTest.cmp'];
    assert.equal(scriptTestsFile(script, 'Calc.jack', jackEntries, false), true);
    assert.equal(scriptTestsFile(script, 'Main.jack', jackEntries, false), true);
    assert.equal(scriptUsesSavedFile(script, 'CalcTest.tst', 'Calc.jack', jackEntries, false), true);
    // Prefers the class under test over the Main.jack harness.
    assert.equal(primaryScriptTarget(script, jackEntries, false), 'Calc.jack');

    const vmEntries = ['Main.vm', 'Boot.vm', 'FolderVME.tst', 'Folder.cmp'];
    assert.equal(scriptTestsFile(script, 'Boot.vm', vmEntries, false), true);
    assert.equal(primaryScriptTarget(script, vmEntries, false), 'Main.vm');

    const folderScript = 'load SubProg,\noutput-file SubProg.out,\ncompare-to SubProg.cmp;\n';
    assert.equal(scriptTestsFile(folderScript, 'Main.vm', [], false, 'SubProg'), true);
    assert.equal(scriptTestsFile(folderScript, 'Main.jack', [], false, 'SubProg'), true);
    assert.equal(scriptTestsFile(folderScript, 'Main.asm', [], false, 'SubProg'), false);
    assert.equal(scriptTestsFile(folderScript, 'Main.vm', [], false, 'Other'), false);
});

test('validates compare-to paths against traversal, absolute paths and bad extensions', () => {
    assert.equal(isValidBasename('Safe.cmp'), true);
    assert.equal(isValidBasename('../secret.cmp'), false);
    assert.equal(isValidBasename('/etc/passwd'), false);
    assert.equal(isValidBasename('C:\\temp\\a.cmp'), false);
    assert.equal(isValidBasename('..'), false);

    for (const bad of ['../secret.cmp', '/etc/passwd.cmp', 'C:evil.cmp', 'notes.txt']) {
        const pair = compareFilesForScript(
            `load Double.hack,\ncompare-to "${bad}";\n`,
            path.join('/tmp', 'proj', 'Double.tst'),
        );
        assert.equal(pair.explicitCompare, true);
        assert.equal(pair.comparePath, null, `must reject unsafe compare-to ${bad}`);
    }
});

test('resolves compare-to, output-file, ranked reference images, empty asm, and reference sources', () => {
    const script = 'load Flash.hack,\noutput-file "FlashOut.out",\ncompare-to "FlashRef.cmp";\n';
    assert.deepEqual(commandFiles(script, 'output-file'), ['FlashOut.out']);
    const pair = compareFilesForScript(script, path.join('/tmp', 'proj', 'FlashAutomatic.tst'));
    assert.equal(pair.comparePath, path.join('/tmp', 'proj', 'FlashRef.cmp'));
    assert.equal(pair.outputPath, path.join('/tmp', 'proj', 'FlashOut.out'));
    assert.equal(pair.explicitCompare, true);

    assert.equal(hasNoAsmCode('// header\n/* block */\n   \n'), true);
    assert.equal(hasNoAsmCode('// header\n@0\n'), false);

    assert.equal(findExpectedImage(['Main.jack', 'CanvasTestOutput.gif', 'notes.txt']), 'CanvasTestOutput.gif');
    assert.equal(findExpectedImage(['Main.jack', 'icon.png']), undefined);
    // Ranking prefers matching script/program stem over alphabetical order.
    assert.deepEqual(
        rankExpectedImages(['AlphaTestOutput.gif', 'CanvasTestOutput.gif'], ['CanvasTest', 'Canvas']),
        ['CanvasTestOutput.gif', 'AlphaTestOutput.gif'],
    );

    // Shared script ranking: current failure -> readable compare -> exact stem -> alphabetical.
    const ranked = orderScriptCandidates(
        [
            { item: 'A-extra.tst', scriptName: 'A-extra.tst', currentFailure: false, hasCompare: true },
            { item: 'Chip.tst', scriptName: 'Chip.tst', currentFailure: false, hasCompare: true },
            { item: 'Z-fail.tst', scriptName: 'Z-fail.tst', currentFailure: true, hasCompare: true },
            { item: 'B-nocmp.tst', scriptName: 'B-nocmp.tst', currentFailure: false, hasCompare: false },
        ],
        'Chip.hdl',
    );
    assert.deepEqual(ranked, ['Z-fail.tst', 'Chip.tst', 'A-extra.tst', 'B-nocmp.tst']);

    assert.equal(hasReferenceSource('Prog.hack', ['Prog.asm', 'Prog.hack'], false), true);
    assert.equal(hasReferenceSource('Prog.hack', ['Prog.hack'], false), false);
    assert.equal(hasReferenceSource('Main.vm', ['Main.jack', 'Main.vm'], false), true);
    assert.equal(hasReferenceSource('MainT.xml', ['Main.jack', 'MainT.xml'], false), true);
    assert.equal(hasReferenceSource('Main.xml', ['Main.jack', 'Main.xml'], false), true);
    assert.equal(hasReferenceSource('Orphan.xml', ['Orphan.xml'], false), false);
});

test('resultMatchesFile distinguishes source builds from student artifacts and cellMatchesExpected follows engine wildcards', () => {
    const fromAsm = { inputs: [{ path: '/work/double.asm', builtFromSource: true }] };
    assert.equal(resultMatchesFile(fromAsm, '/work/double.asm', false), true);
    assert.equal(resultMatchesFile(fromAsm, '/work/Double.hack', false), false);

    const fromHack = { inputs: [{ path: '/work/Double.hack', builtFromSource: false }] };
    assert.equal(resultMatchesFile(fromHack, '/work/Double.hack', false), true);
    assert.equal(resultMatchesFile(fromHack, '/work/double.asm', false), false);

    const fromJack = { inputs: [{ path: '/work/Calc.jack', builtFromSource: true }] };
    assert.equal(resultMatchesFile(fromJack, '/work/Calc.jack', false), true);
    assert.equal(resultMatchesFile(fromJack, '/work/Calc.vm', false), false);

    assert.equal(cellMatchesExpected('  1*  ', '  10  '), true);
    assert.equal(cellMatchesExpected('  1*  ', '  20  '), false);
    assert.equal(cellMatchesExpected('   6  ', '  6   '), false);
});
