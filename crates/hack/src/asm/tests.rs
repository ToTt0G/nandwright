use super::*;

fn words(src: &str) -> Vec<u16> {
    let a = assemble(src);
    assert!(a.is_ok(), "{src:?}: {:?}", a.diagnostics);
    a.words
}

fn rejects(src: &str) -> bool {
    !assemble(src).is_ok()
}

#[test]
fn basic_program() {
    let src = "// add\n@2\nD=A\n@3\nD=D+A\n@0\nM=D\n";
    assert_eq!(
        hack_text(&words(src)),
        "0000000000000010\n1110110000010000\n0000000000000011\n1110000010010000\n\
         0000000000000000\n1110001100001000\n"
    );
}

#[test]
fn symbols_resolve_like_java() {
    // Variables from 16, in order of first use; labels resolved in pass 1.
    assert_eq!(words("@x\n@y\n@x\n"), [16, 17, 16]);
    assert_eq!(words("@x\n@1\n(x)\n"), [2, 1]);
    // Duplicate labels: the last wins.
    assert_eq!(words("@1\n(L)\n@2\n(L)\n@L\n"), [1, 2, 2]);
    // Labels override predefined symbols.
    assert_eq!(words("@1\n(SP)\n@SP\n"), [1, 1]);
    // Numbers win over labels.
    assert_eq!(words("(5)\n@5\n"), [5]);
    assert_eq!(words("(-1)\n@-1\n"), [0xFFFF]);
    // Out-of-range numbers and odd names are symbols.
    assert_eq!(
        words("@32768\n@65536\n@1abc\n@R16\n@32768\n"),
        [16, 17, 18, 19, 16]
    );
    assert_eq!(words("@-32768\n@+5\n@-0\n@032767\n"), [0x8000, 5, 0, 32767]);
    assert_eq!(
        words("@sp\n@SP\n@SCREEN\n@KBD\n@R15\n"),
        [16, 0, 16384, 24576, 15]
    );
    // Whitespace is removed everywhere.
    assert_eq!(words("@5 6\n@x y\n@xy\n"), [56, 16, 16]);
    // Single punctuation characters are symbols / label names.
    assert_eq!(words("(;)\n@;\n@é\n@é\n"), [0, 16, 16]);
}

#[test]
fn c_instruction_forms() {
    assert_eq!(words("M=D+M\nM=M+D\n"), [0xF088, 0xF088]);
    assert_eq!(words("=M\nD;\nD=M;\n"), [0xFC00, 0xE300, 0xFC10]);
    assert_eq!(words("D=A&D\nD=D&A\n"), [0xE010, 0xE010]);
    // Without `dest=`, one token after the computation is skipped (observed).
    assert_eq!(
        words("D\tJGT\nD\tX;JGT\nM+D;;\nD#JGT\nD;;JMP\nD/\n"),
        [0xE300, 0xE301, 0xF080, 0xE301, 0xE307, 0xE300]
    );
    let a = assemble("D\tJGT\n");
    assert_eq!(a.diagnostics.len(), 1);
    assert!(a.diagnostics[0].message.contains("`D;JGT`"));
}

#[test]
fn whole_file_quirks() {
    assert_eq!(words("D=M\r\n@1\r\n"), [0xFC10, 1]);
    assert_eq!(words("@1\r@2\r"), [1, 2]);
    assert_eq!(words("D=M"), [0xFC10]);
    assert_eq!(words(""), Vec::<u16>::new());
    assert_eq!(words("@1 // héllo \u{0}\n"), [1]);
    assert!(rejects("\u{feff}@1\n"));
    assert!(rejects("\u{feff}\n"));
}

#[test]
fn program_size_limit() {
    let ok = "D=M\n".repeat(MAX_INSTRUCTIONS) + "(END)\n";
    let a = assemble(&ok);
    assert!(a.is_ok());
    assert_eq!(a.words.len(), MAX_INSTRUCTIONS);
    let bad = "D=M\n".repeat(MAX_INSTRUCTIONS + 1);
    let a = assemble(&bad);
    assert!(!a.is_ok());
    let err = a.diagnostics.iter().find(|d| d.is_error()).unwrap();
    assert_eq!(err.span.unwrap().start, MAX_INSTRUCTIONS * 4);
}

#[test]
fn variables_past_the_screen_and_32767() {
    let src: String = (0..32767).map(|i| format!("@v{i}\n")).collect();
    let a = assemble(&src);
    assert!(a.is_ok());
    assert_eq!(a.words[16368], 16384);
    assert_eq!(*a.words.last().unwrap(), 16 + 32766);
    assert!(
        a.diagnostics
            .iter()
            .any(|d| d.code.as_str() == "asm-too-many-variables")
    );
}

#[test]
fn symbols_have_spans() {
    let src = "(LOOP)\n@x\n@LOOP\n@x\n@SP\n0;JMP\n";
    let a = assemble(src);
    let find = |n: &str| a.symbols.iter().find(|s| s.name == n).unwrap();
    let lp = find("LOOP");
    assert_eq!(lp.kind, SymbolKind::Label);
    assert_eq!(lp.address, 0);
    assert_eq!(lp.definition.unwrap().slice(src), "LOOP");
    assert_eq!(lp.definition.unwrap().start, 1);
    assert_eq!(lp.references.len(), 1);
    let x = find("x");
    assert_eq!(
        (x.kind, x.address, x.references.len()),
        (SymbolKind::Variable, 16, 2)
    );
    assert_eq!(find("SP").kind, SymbolKind::Predefined);
    assert_eq!(a.source_map.len(), 5);
    assert_eq!(a.source_map[4].slice(src), "0;JMP");
    assert_eq!(a.address_at(src.find("0;JMP").unwrap() + 2), Some(4));
    assert_eq!(a.address_at(0), None);
    assert_eq!(a.symbol_at(src.find("@x").unwrap() + 1).unwrap().name, "x");
}

#[test]
fn errors_keep_the_tree_and_addresses() {
    let src = "@1\nD=X\n(L)\n@L\n";
    let a = assemble(src);
    assert!(a.words.is_empty());
    assert_eq!(a.source_map.len(), 3);
    let l = a.symbols.iter().find(|s| s.name == "L").unwrap();
    assert_eq!(l.address, 2);
    let errs: Vec<_> = a.diagnostics.iter().filter(|d| d.is_error()).collect();
    assert_eq!(errs.len(), 1);
    assert_eq!(errs[0].span.unwrap().slice(src), "X");
}

#[test]
fn warnings_for_surprising_but_accepted_input() {
    let codes = |s: &str| -> Vec<&'static str> {
        assemble(s)
            .diagnostics
            .iter()
            .map(|d| d.code.as_str())
            .collect()
    };
    assert_eq!(codes("@-1\n"), ["asm-negative-constant"]);
    assert_eq!(codes("@40000\n"), ["asm-number-out-of-range"]);
    assert_eq!(codes("@1abc\n"), ["asm-nonportable-symbol"]);
    assert_eq!(codes("(L)\n(L)\n"), ["asm-duplicate-label"]);
    assert_eq!(codes("(SP)\n"), ["asm-label-redefines-predefined"]);
    assert_eq!(codes("(5)\n"), ["asm-unreachable-label"]);
    assert!(codes("@LOOP\n(LOOP)\n@R0\n").is_empty());
}

#[test]
fn hack_files() {
    assert_eq!(
        parse_hack("0000000000000101\r\n1110110000010000\r\n\r\n"),
        Ok(vec![5, 0xEC10])
    );
    assert_eq!(parse_hack("0000000000000101"), Ok(vec![5]));
    assert_eq!(parse_hack(""), Ok(vec![]));
    let err = parse_hack("0000000000000101\n\n000000000000011\n0000000000000201\n").unwrap_err();
    assert_eq!(err.len(), 3);
    assert!(err[0].message.starts_with("line 2:"));
    assert!(err[1].message.starts_with("line 3:"));
    assert!(parse_hack("\u{feff}0000000000000101\n").is_err());
    assert!(parse_hack(&"0000000000000000\n".repeat(32769)).is_err());
    assert!(parse_hack(&"0000000000000000\n".repeat(32768)).is_ok());
    let words: Vec<u16> = (0..=u16::MAX).step_by(7).collect();
    assert_eq!(parse_hack(&hack_text(&words)), Ok(words));
}
