//! Formats every `.hdl` and `.tst` file under the given folders in memory and
//! reports how many changed, were refused, or were already formatted. Used to
//! check the formatter against a course tree without touching it.

use std::path::{Path, PathBuf};

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk(&path, out);
        } else if matches!(
            path.extension().and_then(|e| e.to_str()),
            Some("hdl" | "tst")
        ) {
            out.push(path);
        }
    }
}

fn main() {
    let mut files = Vec::new();
    for arg in std::env::args().skip(1) {
        walk(Path::new(&arg), &mut files);
    }
    files.sort();
    let (mut same, mut changed, mut refused) = (0, 0, 0);
    for path in &files {
        let bytes = std::fs::read(path).unwrap_or_default();
        let file = n2t_core::host::decode_text(bytes);
        let formatted = if path.extension().is_some_and(|e| e == "hdl") {
            n2t_core::hdl::format_source(&file.text)
        } else {
            n2t_core::tst::format_source(&file.text)
        };
        match formatted {
            None => {
                refused += 1;
                println!("refused  {}", path.display());
            }
            Some(text) if text == file.text => same += 1,
            Some(_) => changed += 1,
        }
    }
    println!(
        "{} files: {same} already formatted, {changed} would change, {refused} refused",
        files.len()
    );
}
