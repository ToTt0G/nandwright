// Pure rules for linking test scripts (.tst), compare files (.cmp), output
// files (.out), reference images (*Output.gif), and source files (.hdl, .asm,
// .hack, .vm, .jack). No `vscode` import so unit tests run in plain Node.

import * as path from 'node:path';

/**
 * A script's text with `//` and `/* ... *\/` comments replaced by single spaces
 * (never empty string, so `load/* gap *\/Main.vm` stays separated).
 */
export function withoutComments(text: string): string {
    return text.replace(/\/\*[\s\S]*?\*\/|\/\/[^\r\n]*/g, ' ');
}

/** The file name without its last extension. */
function stemOf(name: string): string {
    const dot = name.lastIndexOf('.');
    return dot > 0 ? name.slice(0, dot) : name;
}

/** The lowercase extension without the leading dot, or empty string. */
function extOf(name: string): string {
    const dot = name.lastIndexOf('.');
    return dot > 0 ? name.slice(dot + 1).toLowerCase() : '';
}

function sameName(a: string, b: string, ignoreCase: boolean): boolean {
    return ignoreCase ? a.toLowerCase() === b.toLowerCase() : a === b;
}

/**
 * Matches `crates/core/src/tst/parser.rs::check_basename`: only a plain file
 * or folder name in the script's directory (no separators, no drive/scheme,
 * no `.` or `..`, no control characters).
 */
export function isValidBasename(name: string): boolean {
    if (!name || name === '.' || name === '..') {
        return false;
    }
    if (/[\\/:]/.test(name)) {
        return false;
    }
    for (let i = 0; i < name.length; i++) {
        if (name.charCodeAt(i) < 0x20) {
            return false;
        }
    }
    return true;
}

type TstToken =
    | { kind: 'word'; value: string }
    | { kind: 'string'; value: string }
    | { kind: 'punct'; value: ',' | ';' | '!' | '{' | '}' };

/**
 * Minimal `.tst` tokenizer mirroring `crates/core/src/tst/lexer.rs`: comments
 * separate tokens, quoted strings are single tokens (so `echo "repeat {"` or
 * `echo "compare-to fake.cmp"` never confuse script discovery).
 */
function tokenizeTst(text: string): TstToken[] {
    const tokens: TstToken[] = [];
    let i = 0;
    const len = text.length;
    while (i < len) {
        const ch = text[i];
        if (ch === ' ' || ch === '\t' || ch === '\r' || ch === '\n') {
            i++;
            continue;
        }
        if (ch === '/' && text[i + 1] === '/') {
            i += 2;
            while (i < len && text[i] !== '\n') {
                i++;
            }
            continue;
        }
        if (ch === '/' && text[i + 1] === '*') {
            i += 2;
            while (i < len && !(text[i] === '*' && text[i + 1] === '/')) {
                i++;
            }
            i = Math.min(len, i + 2);
            continue;
        }
        if (ch === ',' || ch === ';' || ch === '!' || ch === '{' || ch === '}') {
            tokens.push({ kind: 'punct', value: ch });
            i++;
            continue;
        }
        if (ch === '"') {
            i++;
            let value = '';
            while (i < len && text[i] !== '"' && text[i] !== '\r' && text[i] !== '\n') {
                value += text[i++];
            }
            if (i < len && text[i] === '"') {
                i++;
            }
            tokens.push({ kind: 'string', value });
            continue;
        }
        let value = '';
        while (i < len) {
            const c = text[i];
            if (
                c === ' ' || c === '\t' || c === '\r' || c === '\n'
                || c === ',' || c === ';' || c === '!' || c === '{' || c === '}' || c === '"'
                || (c === '/' && (text[i + 1] === '/' || text[i + 1] === '*'))
            ) {
                break;
            }
            value += c;
            i++;
        }
        if (value) {
            tokens.push({ kind: 'word', value });
        }
    }
    return tokens;
}

export interface ParsedScriptLinks {
    /** File names in top-level `load <file>` commands (valid basenames with extensions). */
    loads: string[];
    /** Folder names in top-level `load <folder>` commands (valid basenames without extensions). */
    loadFolders: string[];
    /** True when the script has a bare `load,` or `load;` (VM Emulator folder load). */
    bareLoad: boolean;
    /** File names in built-in method loads such as `ROM32K load Add.hack`. */
    builtinLoads: string[];
    /** Valid `.cmp` basenames from `compare-to` commands. */
    compareTo: string[];
    /** True when the script contains any `compare-to` command (even with an invalid argument). */
    hasCompareCommand: boolean;
    /** Valid `.out` basenames from `output-file` commands. */
    outputFile: string[];
    /** True when the script contains a `repeat { ... }` loop with no count. */
    forever: boolean;
}

const VALID_LOAD_EXTENSIONS = new Set(['hdl', 'asm', 'hack', 'vm']);

/** Parses all linking metadata of a `.tst` script using token-accurate statement boundaries. */
export function parseScriptLinks(script: string): ParsedScriptLinks {
    const tokens = tokenizeTst(script);
    const loads: string[] = [];
    const loadFolders: string[] = [];
    const builtinLoads: string[] = [];
    const compareTo: string[] = [];
    const outputFile: string[] = [];
    let bareLoad = false;
    let hasCompareCommand = false;
    let forever = false;

    let stmtStart = 0;
    const processStatement = (stmt: TstToken[], nextToken: TstToken | undefined) => {
        if (stmt.length === 0) {
            return;
        }
        const first = stmt[0];
        if (first.kind !== 'word') {
            return;
        }
        const cmd = first.value;
        if (cmd === 'repeat') {
            if (stmt.length === 1 && nextToken?.kind === 'punct' && nextToken.value === '{') {
                forever = true;
            }
            return;
        }
        if (cmd === 'load') {
            if (stmt.length === 1) {
                bareLoad = true;
                return;
            }
            const arg = stmt[1];
            if ((arg.kind === 'word' || arg.kind === 'string') && isValidBasename(arg.value)) {
                const ext = extOf(arg.value);
                if (!ext) {
                    loadFolders.push(arg.value);
                } else if (VALID_LOAD_EXTENSIONS.has(ext) && stemOf(arg.value).length > 0) {
                    loads.push(arg.value);
                }
            }
            return;
        }
        if (cmd === 'compare-to') {
            hasCompareCommand = true;
            const arg = stmt[1];
            if (
                arg
                && (arg.kind === 'word' || arg.kind === 'string')
                && isValidBasename(arg.value)
                && extOf(arg.value) === 'cmp'
                && stemOf(arg.value).length > 0
            ) {
                compareTo.push(arg.value);
            }
            return;
        }
        if (cmd === 'output-file') {
            const arg = stmt[1];
            if (
                arg
                && (arg.kind === 'word' || arg.kind === 'string')
                && isValidBasename(arg.value)
                && extOf(arg.value) === 'out'
                && stemOf(arg.value).length > 0
            ) {
                outputFile.push(arg.value);
            }
            return;
        }
        // Built-in method load, e.g. `ROM32K load Add.hack`.
        if (stmt.length >= 3 && stmt[1].kind === 'word' && stmt[1].value === 'load') {
            const arg = stmt[2];
            if ((arg.kind === 'word' || arg.kind === 'string') && isValidBasename(arg.value)) {
                builtinLoads.push(arg.value);
            }
        }
    };

    for (let i = 0; i < tokens.length; i++) {
        if (tokens[i].kind === 'punct') {
            processStatement(tokens.slice(stmtStart, i), tokens[i]);
            stmtStart = i + 1;
        }
    }
    if (stmtStart < tokens.length) {
        processStatement(tokens.slice(stmtStart), undefined);
    }

    return { loads, loadFolders, bareLoad, builtinLoads, compareTo, hasCompareCommand, outputFile, forever };
}

/** The files a script names in one kind of command; a name may be in double quotes. */
export function commandFiles(script: string, command: 'load' | 'output-file' | 'compare-to'): string[] {
    const parsed = parseScriptLinks(script);
    switch (command) {
        case 'load':
            return [...parsed.loads, ...parsed.loadFolders];
        case 'output-file':
            return parsed.outputFile;
        case 'compare-to':
            return parsed.compareTo;
    }
}

/**
 * Top-level `load` commands of a script (never built-in method calls such as
 * `ROM32K load Add.hack`). `bare` is true when the script has a bare `load,`
 * or `load;` (VM Emulator folder load).
 */
export function topLevelLoads(script: string): { files: string[]; folders: string[]; bare: boolean } {
    const parsed = parseScriptLinks(script);
    return { files: parsed.loads, folders: parsed.loadFolders, bare: parsed.bareLoad };
}

/** The files a script names in `load`, built-in `load` and `compare-to` commands. */
export function namedFiles(script: string): string[] {
    const parsed = parseScriptLinks(script);
    return [...parsed.loads, ...parsed.loadFolders, ...parsed.builtinLoads, ...parsed.compareTo];
}

/**
 * Whether `script` has a `repeat { ... }` loop without a count (a
 * demonstration to watch, such as `Fill.tst`, rather than a finite test).
 */
export function isForeverScript(script: string): boolean {
    return parseScriptLinks(script).forever;
}

/**
 * Finds a file in `folderEntries` with extension `ext` matching `targetStem`:
 * exact stem (respecting `ignoreCase`) first, then (when `allowCaseFold` is
 * true, the default) a unique case-insensitive match (so `mult.asm` matches
 * `load Mult.hack` on Linux too).
 */
export function findByStem(
    folderEntries: string[],
    targetStem: string,
    ext: string,
    ignoreCase: boolean,
    allowCaseFold = true,
): string | undefined {
    const candidates = folderEntries.filter((entry) => extOf(entry) === ext);
    const exact = candidates.find((entry) => sameName(stemOf(entry), targetStem, ignoreCase));
    if (exact) {
        return exact;
    }
    if (!allowCaseFold) {
        return undefined;
    }
    const folded = candidates.filter((entry) => stemOf(entry).toLowerCase() === targetStem.toLowerCase());
    return folded.length === 1 ? folded[0] : undefined;
}

function matchesAsmStem(
    fileName: string,
    targetStem: string,
    folderEntries: string[],
    ignoreCase: boolean,
): boolean {
    const found = findByStem(folderEntries, targetStem, 'asm', ignoreCase, true);
    if (found) {
        return sameName(found, fileName, ignoreCase);
    }
    return stemOf(fileName).toLowerCase() === targetStem.toLowerCase();
}

function matchesJackStem(
    fileName: string,
    targetStem: string,
    folderEntries: string[],
    ignoreCase: boolean,
): boolean {
    const found = findByStem(folderEntries, targetStem, 'jack', ignoreCase, ignoreCase);
    if (found) {
        return sameName(found, fileName, ignoreCase);
    }
    return sameName(stemOf(fileName), targetStem, ignoreCase);
}

/**
 * Whether a `.tst` script tests `fileName` (`.hdl`, `.asm`, `.hack`, `.vm` or
 * `.jack`). When `subfolderName` is given, the script lives in the parent
 * folder of `fileName` and is checked for `load <subfolderName>`.
 */
export function scriptTestsFile(
    scriptText: string,
    fileName: string,
    folderEntries: string[],
    ignoreCase: boolean,
    subfolderName?: string,
): boolean {
    const ext = extOf(fileName);
    const { loads, loadFolders, bareLoad } = parseScriptLinks(scriptText);
    if (subfolderName !== undefined) {
        if (ext !== 'vm' && ext !== 'jack') {
            return false;
        }
        return loadFolders.some((folder) => sameName(folder, subfolderName, ignoreCase));
    }
    switch (ext) {
        case 'hdl':
            return loads.some((target) => extOf(target) === 'hdl' && sameName(target, fileName, ignoreCase));
        case 'asm':
            return loads.some((target) => {
                const targetExt = extOf(target);
                if (targetExt === 'asm') {
                    return sameName(target, fileName, ignoreCase)
                        || matchesAsmStem(fileName, stemOf(target), folderEntries, ignoreCase);
                }
                if (targetExt === 'hack') {
                    return matchesAsmStem(fileName, stemOf(target), folderEntries, ignoreCase);
                }
                return false;
            });
        case 'hack':
            return loads.some((target) => extOf(target) === 'hack' && sameName(target, fileName, ignoreCase));
        case 'vm':
            return bareLoad || loads.some((target) => extOf(target) === 'vm' && sameName(target, fileName, ignoreCase));
        case 'jack':
            return bareLoad || loads.some((target) => extOf(target) === 'vm' && matchesJackStem(fileName, stemOf(target), folderEntries, ignoreCase));
        default:
            return false;
    }
}

/**
 * Whether saving `savedName` should re-run `scriptName` (whose text is
 * `scriptText`). When `subfolderName` is given, `savedName` lives inside
 * `<scriptDir>/<subfolderName>/`.
 */
export function scriptUsesSavedFile(
    scriptText: string,
    scriptName: string,
    savedName: string,
    folderEntries: string[],
    ignoreCase: boolean,
    subfolderName?: string,
): boolean {
    if (subfolderName !== undefined) {
        return scriptTestsFile(scriptText, savedName, folderEntries, ignoreCase, subfolderName);
    }
    if (sameName(scriptName, savedName, ignoreCase)) {
        return true;
    }
    const parsed = parseScriptLinks(scriptText);
    if (parsed.compareTo.some((target) => sameName(target, savedName, ignoreCase))) {
        return true;
    }
    if (scriptTestsFile(scriptText, savedName, folderEntries, ignoreCase)) {
        return true;
    }
    if (extOf(savedName) === 'hack' && parsed.builtinLoads.some((target) => sameName(target, savedName, ignoreCase))) {
        return true;
    }
    // A custom chip script without a `load` command still matches by stem.
    if (
        extOf(savedName) === 'hdl'
        && parsed.loads.length === 0
        && parsed.loadFolders.length === 0
        && !parsed.bareLoad
        && sameName(stemOf(scriptName), stemOf(savedName), ignoreCase)
    ) {
        return true;
    }
    return false;
}

/**
 * The primary file a script tests (the file a student edits for that script),
 * if one exists in `folderEntries`.
 */
export function primaryScriptTarget(
    scriptText: string,
    folderEntries: string[],
    ignoreCase: boolean,
): string | undefined {
    const findExact = (name: string) => folderEntries.find((entry) => sameName(entry, name, ignoreCase));
    const { loads, bareLoad } = parseScriptLinks(scriptText);
    const first = loads[0];
    if (first && !/[\\/]/.test(first)) {
        const ext = extOf(first);
        if (ext === 'hdl') {
            return findExact(first);
        }
        if (ext === 'asm') {
            return findExact(first) ?? findByStem(folderEntries, stemOf(first), 'asm', ignoreCase, true);
        }
        if (ext === 'hack') {
            // Prefer the `.asm` source the student edits over the built `.hack`.
            return findByStem(folderEntries, stemOf(first), 'asm', ignoreCase, true) ?? findExact(first);
        }
        if (ext === 'vm') {
            return findExact(first) ?? findByStem(folderEntries, stemOf(first), 'jack', ignoreCase, ignoreCase);
        }
        return findExact(first);
    }
    if (bareLoad) {
        const jacks = folderEntries.filter((entry) => extOf(entry) === 'jack').sort((a, b) => a.localeCompare(b));
        if (jacks.length > 0) {
            // In OS tests (e.g. MathTest with Main.jack + Math.jack), prefer the
            // class under test over the harness Main.jack.
            return jacks.find((entry) => stemOf(entry).toLowerCase() !== 'main') ?? jacks[0];
        }
        const vms = folderEntries.filter((entry) => extOf(entry) === 'vm').sort((a, b) => a.localeCompare(b));
        if (vms.length > 0) {
            return vms.find((entry) => stemOf(entry).toLowerCase() === 'main')
                ?? vms.find((entry) => stemOf(entry).toLowerCase() === 'sys')
                ?? vms[0];
        }
    }
    return undefined;
}

/**
 * The compare and output file paths of a `.tst` script. Only validated plain
 * basenames with `.cmp` / `.out` extensions are resolved inside the script's
 * directory, matching `crates/core/src/tst/parser.rs`.
 */
export function compareFilesForScript(
    scriptText: string,
    scriptPath: string,
): { comparePath: string | null; outputPath: string; explicitCompare: boolean } {
    const dir = path.dirname(scriptPath);
    const base = scriptPath.replace(/\.[^.\\/]+$/, '');
    const parsed = parseScriptLinks(scriptText);
    const compareName = parsed.compareTo[0];
    const outputName = parsed.outputFile[0];
    // If the script explicitly wrote `compare-to` with an invalid path/extension,
    // return `null` rather than guessing or reading outside the script's folder.
    const comparePath = compareName
        ? path.join(dir, compareName)
        : parsed.hasCompareCommand
          ? null
          : `${base}.cmp`;
    return {
        comparePath,
        outputPath: outputName ? path.join(dir, outputName) : `${base}.out`,
        explicitCompare: parsed.hasCompareCommand,
    };
}

/** Whether a `.asm` file has no instructions or labels yet (comments/whitespace only). */
export function hasNoAsmCode(asm: string): boolean {
    return withoutComments(asm).trim() === '';
}

/**
 * Ranks reference screen images in a folder (e.g. `ScreenTestOutput.gif`,
 * `OutputTestOutput.gif` in project 12 visual OS tests), preferring images
 * whose stem matches one of `preferredStems` (selected script or program stem).
 */
export function rankExpectedImages(folderEntries: string[], preferredStems: string[] = []): string[] {
    const stems = preferredStems.map((s) => s.toLowerCase()).filter(Boolean);
    const score = (entry: string): number => {
        const lower = stemOf(entry).toLowerCase();
        const prefix = lower.replace(/output$/, '');
        for (let i = 0; i < stems.length; i++) {
            const stem = stems[i];
            if (prefix === stem || prefix === `${stem}test`) {
                return i * 2;
            }
            if (prefix.startsWith(stem) || stem.startsWith(prefix)) {
                return i * 2 + 1;
            }
        }
        return stems.length * 2 + 1;
    };
    return folderEntries
        .filter((entry) => /output\.(gif|png)$/i.test(entry))
        .sort((a, b) => score(a) - score(b) || a.localeCompare(b));
}

/** Best matching reference screen image in a folder, if any. */
export function findExpectedImage(folderEntries: string[], preferredStems: string[] = []): string | undefined {
    return rankExpectedImages(folderEntries, preferredStems)[0];
}

export interface RankedScriptCandidate<T> {
    item: T;
    scriptName: string;
    currentFailure: boolean;
    hasCompare: boolean;
}

/**
 * Shared script ordering used by ComputerPanel, Step Through and Compare Output:
 * 1. Currently failing script (not stale)
 * 2. Script with a readable `.cmp` file
 * 3. Exact stem match with `targetFileName`
 * 4. Alphabetical
 */
export function orderScriptCandidates<T>(
    candidates: RankedScriptCandidate<T>[],
    targetFileName: string,
): T[] {
    const targetStem = stemOf(targetFileName).toLowerCase();
    return [...candidates]
        .sort((a, b) => {
            if (a.currentFailure !== b.currentFailure) {
                return a.currentFailure ? -1 : 1;
            }
            if (a.hasCompare !== b.hasCompare) {
                return a.hasCompare ? -1 : 1;
            }
            const aExact = stemOf(a.scriptName).toLowerCase() === targetStem;
            const bExact = stemOf(b.scriptName).toLowerCase() === targetStem;
            if (aExact !== bExact) {
                return aExact ? -1 : 1;
            }
            return a.scriptName.localeCompare(b.scriptName);
        })
        .map((c) => c.item);
}

/**
 * Whether `fileName` (`.hack`, `.vm` or `.xml`) has a sibling source file that
 * `Compare with Reference Output` can compare it against.
 */
export function hasReferenceSource(
    fileName: string,
    folderEntries: string[],
    ignoreCase: boolean,
): boolean {
    const ext = extOf(fileName);
    const stem = stemOf(fileName);
    const exists = (candidate: string) => folderEntries.some((entry) => sameName(entry, candidate, ignoreCase));
    if (ext === 'hack') {
        return exists(`${stem}.asm`);
    }
    if (ext === 'vm') {
        return exists(`${stem}.jack`);
    }
    if (ext === 'xml') {
        if (exists(`${stem}.jack`)) {
            return true;
        }
        return stem.endsWith('T') && stem.length > 1 && exists(`${stem.slice(0, -1)}.jack`);
    }
    return false;
}
