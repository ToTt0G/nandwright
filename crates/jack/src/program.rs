//! Compiling a folder of classes and the call checks between them.
//!
//! The official compiler, given a directory, compiles every class and then
//! checks each call `C.f(…)`, `f(…)` and `v.f(…)` whose class `C` is one of the
//! classes it compiled without errors: the subroutine must exist, be called
//! the right way (method versus function/constructor) and get the declared
//! number of arguments. It knows nothing about the OS or any class outside
//! the directory. These checks are errors here as well. In addition, calls to
//! the OS and to classes that are not in the folder at all get warnings,
//! which the official compiler does not give.

use n2t_core::Diagnostic;

use crate::ast::SubroutineKind;
use crate::check::{ClassSig, DeferredCall};
use crate::diag::{JDiag, codes, error, warning};
use crate::{Compiled, Options, Unit, os};

/// The result of [`compile_program`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProgramOutput {
    /// One entry per input file, in input order. Call checks between classes
    /// are part of each file's diagnostics.
    pub files: Vec<FileOutput>,
    /// Findings about the program as a whole (for example, a missing `Main`).
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileOutput {
    pub file_stem: String,
    pub compiled: Compiled,
}

/// Compiles the classes of one program (typically a folder): `files` holds
/// `(file_stem, source)` pairs.
pub fn compile_program(files: &[(&str, &str)]) -> ProgramOutput {
    compile_program_with(files, &Options::default())
}

/// [`compile_program`] with explicit [`Options`].
pub fn compile_program_with(files: &[(&str, &str)], options: &Options) -> ProgramOutput {
    let units: Vec<Unit> = files.iter().map(|(stem, src)| Unit::new(stem, src)).collect();
    let mut clean: Vec<&ClassSig> = Vec::new();
    for u in &units {
        if let Some(sig) = u.sig_if_clean()
            && !clean.iter().any(|c| c.name == sig.name)
        {
            clean.push(sig);
        }
    }
    let all_names: Vec<&str> = units.iter().filter_map(|u| u.class_name.as_deref()).collect();
    let mut extra: Vec<Vec<JDiag>> = Vec::new();
    for (i, u) in units.iter().enumerate() {
        let mut d = check_calls(&u.deferred, &clean, Some(&all_names));
        // The same class declared in two files.
        if let (Some(name), Some(span)) = (&u.class_name, u.class_name_span)
            && let Some((j, _)) = units
                .iter()
                .enumerate()
                .find(|(j, o)| *j != i && o.class_name.as_deref() == Some(name.as_str()))
        {
            let other = files.get(j).map_or("", |f| f.0);
            d.push(error(
                codes::DUPLICATE_CLASS,
                format!("Class `{name}` is also declared in `{other}.jack`."),
                span,
                0,
            ));
        }
        extra.push(d);
    }
    let mut diagnostics = Vec::new();
    let has_main = units.iter().any(|u| {
        u.class_name.as_deref() == Some("Main")
            && u.sig().is_some_and(|s| {
                s.subs
                    .iter()
                    .any(|f| f.name == "main" && f.kind == SubroutineKind::Function)
            })
    });
    if !has_main && !units.is_empty() {
        diagnostics.push(Diagnostic::new(
            n2t_core::Severity::Information,
            n2t_core::DiagnosticCode::Other("jack-no-main"),
            "No class `Main` with a function `main` in this program: `Sys.init` starts a program \
             by calling `Main.main`.",
            None,
        ));
    }
    let out_files = units
        .into_iter()
        .zip(extra)
        .zip(files)
        .map(|((u, d), (stem, _))| FileOutput {
            file_stem: (*stem).to_string(),
            compiled: u.finish(d, options),
        })
        .collect();
    ProgramOutput {
        files: out_files,
        diagnostics,
    }
}

fn plural(n: usize, word: &str) -> String {
    if n == 1 {
        format!("1 {word}")
    } else {
        format!("{n} {word}s")
    }
}

/// Checks deferred calls against the classes in `table`. With `program`
/// (the names of every class in the program), calls to other classes are
/// checked against the OS API and get warnings.
pub(crate) fn check_calls(
    calls: &[DeferredCall],
    table: &[&ClassSig],
    program: Option<&[&str]>,
) -> Vec<JDiag> {
    let mut out = Vec::new();
    for c in calls {
        if c.name.is_empty() {
            continue;
        }
        let qualified = format!("{}.{}", c.class, c.name);
        if let Some(cls) = table.iter().find(|t| t.name == c.class) {
            let Some(sub) = cls.subs.iter().find(|s| s.name == c.name) else {
                let msg = if c.static_form {
                    format!(
                        "Class `{}` has no function or constructor `{}`.{}",
                        c.class,
                        c.name,
                        suggestion(cls.subs.iter().map(|s| s.name.as_str()), &c.name)
                    )
                } else {
                    format!(
                        "Class `{}` has no method `{}`.{}",
                        c.class,
                        c.name,
                        suggestion(cls.subs.iter().map(|s| s.name.as_str()), &c.name)
                    )
                };
                out.push(error(codes::UNKNOWN_SUBROUTINE, msg, c.name_span, c.java_line));
                continue;
            };
            if c.static_form && sub.kind == SubroutineKind::Method {
                out.push(
                    error(
                        codes::CALL_KIND,
                        format!(
                            "`{qualified}` is a method, so it must be called on an object \
                             (`obj.{}(…)`), not on the class.",
                            c.name
                        ),
                        c.name_span,
                        c.java_line,
                    )
                    .related("declared here", sub.span),
                );
            } else if !c.static_form && sub.kind != SubroutineKind::Method {
                let what = if sub.kind == SubroutineKind::Function {
                    "a function"
                } else {
                    "a constructor"
                };
                out.push(
                    error(
                        codes::CALL_KIND,
                        format!(
                            "`{qualified}` is {what}, so call it on the class: `{qualified}(…)`."
                        ),
                        c.name_span,
                        c.java_line,
                    )
                    .related("declared here", sub.span),
                );
            }
            if sub.params != c.args {
                out.push(
                    error(
                        codes::ARGUMENT_COUNT,
                        format!(
                            "`{qualified}` takes {} but is called with {}.",
                            plural(sub.params, "argument"),
                            plural(c.args, "argument")
                        ),
                        c.name_span,
                        c.java_line,
                    )
                    .related("declared here", sub.span),
                );
            }
            continue;
        }
        let Some(program) = program else { continue };
        if program.contains(&c.class.as_str()) || c.field_in_function {
            // Declared in the folder but not compilable, or already warned about.
            continue;
        }
        if os::is_os_class(&c.class) {
            match os::find(&c.class, &c.name) {
                None => out.push(warning(
                    codes::UNKNOWN_SUBROUTINE,
                    format!(
                        "The OS class `{}` has no subroutine `{}`.{}",
                        c.class,
                        c.name,
                        suggestion(os::class_api(&c.class).map(|s| s.name), &c.name)
                    ),
                    c.name_span,
                    0,
                )),
                Some(f) => {
                    if c.static_form && f.kind == SubroutineKind::Method {
                        out.push(warning(
                            codes::CALL_KIND,
                            format!(
                                "`{qualified}` is a method; call it on a `{}` object.",
                                c.class
                            ),
                            c.name_span,
                            0,
                        ));
                    } else if !c.static_form && f.kind != SubroutineKind::Method {
                        out.push(warning(
                            codes::CALL_KIND,
                            format!("`{qualified}` is a {}; call it as `{qualified}(…)`.", f.kind.as_str()),
                            c.name_span,
                            0,
                        ));
                    }
                    if f.params.len() != c.args {
                        out.push(warning(
                            codes::ARGUMENT_COUNT,
                            format!(
                                "`{}` takes {} but is called with {}.",
                                f.signature(),
                                plural(f.params.len(), "argument"),
                                plural(c.args, "argument")
                            ),
                            c.name_span,
                            0,
                        ));
                    }
                }
            }
            continue;
        }
        let msg = match c.class.as_str() {
            "int" | "char" | "boolean" => format!(
                "This calls `{qualified}`, but the variable is a `{}`, not an object; only \
                 objects have methods.",
                c.class
            ),
            _ => format!(
                "Class `{}` is neither in this folder nor part of the Jack OS, so `{qualified}` \
                 will not be found when the program runs.",
                c.class
            ),
        };
        out.push(warning(codes::UNKNOWN_SUBROUTINE, msg, c.name_span, 0));
    }
    out
}

fn suggestion<'a>(names: impl Iterator<Item = &'a str>, wanted: &str) -> String {
    let lower = wanted.to_ascii_lowercase();
    let best = names
        .map(|n| (edit_distance(&n.to_ascii_lowercase(), &lower), n))
        .filter(|(d, _)| *d <= 2)
        .min_by_key(|(d, _)| *d);
    match best {
        Some((_, n)) => format!(" Did you mean `{n}`?"),
        None => String::new(),
    }
}

fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    if a.len().abs_diff(b.len()) > 2 || a.len() > 64 || b.len() > 64 {
        return usize::MAX;
    }
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut cur = vec![i + 1; b.len() + 1];
        for (j, cb) in b.iter().enumerate() {
            let sub = prev[j] + usize::from(ca != cb);
            cur[j + 1] = sub.min(prev[j + 1] + 1).min(cur[j] + 1);
        }
        prev = cur;
    }
    prev[b.len()]
}
