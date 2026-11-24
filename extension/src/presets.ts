// Course presets: the hand-in rules of a course, as values for the general
// Zip Project Files settings. Facts only (folder names, file names, name
// patterns); no course file or course text. To add a course, add an entry to
// PRESETS and its id to the `nandwright.course` enum in package.json.

export interface CoursePreset {
    /** The value of `nandwright.course`. */
    id: string;
    /** As the setting and the setup show it. */
    label: string;
    /** `nandwright.zip.projects`: the folders of each project, as the course numbers them. */
    projects: Record<string, string[]>;
    /** `nandwright.zip.fileName` for every project, unless `fileNames` has one for it. */
    fileName: string;
    /** Zip names that differ for some projects. */
    fileNames?: Record<string, string>;
    /**
     * The files ticked by default for each project, as patterns (see
     * `matchesPattern`); the grade sheet is ticked as well. Other files are
     * listed but not ticked. A project without patterns ticks as without a preset.
     */
    handIn: Record<string, string[]>;
}

const PY = (...names: string[]) => names.map((name) => `**/${name}`);

export const PRESETS: CoursePreset[] = [
    {
        id: 'kth-ep121u',
        label: 'KTH EP121U',
        // Projects 1 to 4 are the book's folders 01 to 05; projects 5 to 10
        // are the course's own folders, named as its zips unpack.
        projects: {
            '1': ['01', '02'],
            '2': ['03'],
            '3': ['04'],
            '4': ['05'],
            '5': ['Project_05'],
            '6': ['Project_06'],
            '7': ['Project_07'],
            '8': ['Project_08'],
            '9': ['project9-pre'],
            '10': ['Project_10'],
        },
        fileName: 'Project{nn}-{familyname}-{firstname}',
        fileNames: { '9': 'project{nn}-{familyname}-{firstname}' },
        handIn: {
            '1': ['**/*.hdl'],
            '2': ['**/*.hdl'],
            '3': ['**/*.asm'],
            '4': ['**/*.hdl'],
            '5': PY('hasm.py', 'hasmParser.py', 'hasmCode.py', 'hasmSymbols.py', 'hasmError.py'),
            '6': PY('hvm.py', 'hvmCodeWriter.py', 'hvmCommands.py', 'hvmParser.py'),
            '7': PY('hvm.py', 'hvmCodeWriter.py', 'hvmCommands.py', 'hvmParser.py'),
            '8': PY('List.jack', 'CompilationEngine.py'),
            '9': ['CompilationEngine.py'],
            '10': ['Memory.jack', 'MemoryWasteful.jack'],
        },
    },
];

/** The preset of a `nandwright.course` value, or none. */
export function presetFor(id: string | undefined): CoursePreset | undefined {
    return PRESETS.find((preset) => preset.id === id);
}

/**
 * Whether a path in the zip (with `/`) matches a pattern. `*` matches within
 * a name, `**` any number of folders. A pattern without `/` matches the file's
 * name in the project's top folder only; `**\/name` matches it in any folder.
 * Case is ignored.
 */
export function matchesPattern(file: string, pattern: string): boolean {
    let source = '';
    for (let i = 0; i < pattern.length; i++) {
        const c = pattern[i];
        if (c === '*' && pattern[i + 1] === '*') {
            const slash = pattern[i + 2] === '/';
            source += slash ? '(?:.*/)?' : '.*';
            i += slash ? 2 : 1;
        } else if (c === '*') {
            source += '[^/]*';
        } else if (c === '?') {
            source += '[^/]';
        } else {
            source += c.replace(/[.+^${}()|[\]\\]/g, '\\$&');
        }
    }
    return new RegExp(`^${source}$`, 'i').test(file);
}
