//! The built-in rule pack "risky APIs", as ast-grep YAML rules.
//!
//! The table below becomes YAML text, and the YAML loader (the same one that loads
//! `.aivory/rules/*.yml`) turns it into rules. A team can copy the YAML shape for its own rules.

use aivory_syntax::Language;

#[derive(Debug)]
/// One risky API: a rule id, a message, the languages, and one or more ast-grep patterns.
pub struct Risky {
    /// The id without the prefix `aivory/`.
    pub id: &'static str,
    pub message: &'static str,
    pub languages: &'static [&'static str],
    pub patterns: &'static [&'static str],
    /// Skip test code (a Rust `#[test]` or `#[cfg(test)]` item, or a file in a test folder).
    pub skip_tests: bool,
}

const JS: &[&str] = &["javascript", "typescript", "tsx"];

pub const RISKY: &[Risky] = &[
    Risky {
        id: "risky-unwrap",
        message: "`unwrap()` panics on None or Err. Handle the error or use `expect` with a reason.",
        languages: &["rust"],
        patterns: &["$X.unwrap()"],
        skip_tests: true,
    },
    Risky {
        id: "risky-eval",
        message: "`eval` runs text as code. Avoid it.",
        languages: &["javascript", "typescript", "tsx", "python"],
        patterns: &["eval($$$)"],
        skip_tests: false,
    },
    Risky {
        id: "risky-new-function",
        message: "`new Function` runs text as code. Avoid it.",
        languages: JS,
        patterns: &["new Function($$$)"],
        skip_tests: false,
    },
    Risky {
        id: "risky-document-write",
        message: "`document.write` can inject markup. Use DOM methods.",
        languages: JS,
        patterns: &["document.write($$$)"],
        skip_tests: false,
    },
    Risky {
        id: "risky-inner-html",
        message: "Assigning `innerHTML` can inject markup. Use `textContent` or sanitize.",
        languages: JS,
        patterns: &["$X.innerHTML = $Y"],
        skip_tests: false,
    },
    Risky {
        id: "risky-exec",
        message: "`exec` runs text as code. Avoid it.",
        languages: &["python"],
        patterns: &["exec($$$)"],
        skip_tests: false,
    },
    Risky {
        id: "risky-pickle",
        message: "`pickle.loads` runs code from its input. Do not load untrusted data.",
        languages: &["python"],
        patterns: &["pickle.loads($$$)", "pickle.load($$$)"],
        skip_tests: false,
    },
    Risky {
        id: "risky-os-system",
        message: "`os.system` starts a shell. Use `subprocess.run` with a list of arguments.",
        languages: &["python"],
        patterns: &["os.system($$$)"],
        skip_tests: false,
    },
    Risky {
        id: "risky-unsafe-pointer",
        message: "`unsafe.Pointer` bypasses the type system.",
        languages: &["go"],
        patterns: &["unsafe.Pointer($$$)"],
        skip_tests: false,
    },
    Risky {
        id: "risky-runtime-exec",
        message: "`Runtime.exec` starts a process from text. Use `ProcessBuilder` with arguments.",
        languages: &["java"],
        patterns: &["Runtime.getRuntime().exec($$$)"],
        skip_tests: false,
    },
    Risky {
        id: "risky-unbounded-copy",
        message: "This C function does not check the size of the target buffer.",
        languages: &["c", "cpp"],
        patterns: &["strcpy($$$)", "strcat($$$)", "sprintf($$$)", "gets($$$)"],
        skip_tests: false,
    },
    Risky {
        id: "risky-system",
        message: "`system` starts a shell. Use `execv` with a list of arguments.",
        languages: &["c", "cpp"],
        patterns: &["system($$$)"],
        skip_tests: false,
    },
    Risky {
        id: "risky-process-start",
        message: "`Process.Start` starts a process. Check the arguments.",
        languages: &["csharp"],
        patterns: &["Process.Start($$$)"],
        skip_tests: false,
    },
];

fn yaml_str(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// The YAML of one pattern. Some grammars cannot parse a call without a statement around it,
/// so their patterns get a context and a selector.
fn pattern_yaml(lang: &str, pattern: &str) -> String {
    let (context, selector) = match lang {
        "c" | "cpp" => (format!("void f() {{ {pattern}; }}"), "call_expression"),
        "go" => (format!("func f() {{ {pattern} }}"), "call_expression"),
        "csharp" => (
            format!("class C {{ void M() {{ {pattern}; }} }}"),
            "invocation_expression",
        ),
        _ => return yaml_str(pattern),
    };
    format!(
        "{{ context: {}, selector: {selector} }}",
        yaml_str(&context)
    )
}

/// The YAML text of the built-in pack: one document per rule and language.
pub fn yaml() -> String {
    let mut out = String::new();
    for r in RISKY {
        for lang in r.languages {
            out.push_str(
                "---
",
            );
            out.push_str(&format!(
                "id: aivory/{}
",
                r.id
            ));
            out.push_str(&format!(
                "language: {lang}
"
            ));
            out.push_str(
                "severity: warning
",
            );
            out.push_str(&format!(
                "message: {}
",
                yaml_str(r.message)
            ));
            out.push_str(
                "rule:
",
            );
            if let [one] = r.patterns {
                out.push_str(&format!(
                    "  pattern: {}
",
                    pattern_yaml(lang, one)
                ));
            } else {
                out.push_str(
                    "  any:
",
                );
                for p in r.patterns {
                    out.push_str(&format!(
                        "    - pattern: {}
",
                        pattern_yaml(lang, p)
                    ));
                }
            }
        }
    }
    out
}

/// The ids of the rules that skip test code.
pub fn skip_test_ids() -> Vec<String> {
    RISKY
        .iter()
        .filter(|r| r.skip_tests)
        .map(|r| format!("aivory/{}", r.id))
        .collect()
}

/// True for a path that holds test code.
pub fn is_test_path(path: &str, language: Language) -> bool {
    let lower = path.to_ascii_lowercase();
    let in_test_dir = lower
        .split('/')
        .rev()
        .skip(1)
        .any(|c| matches!(c, "tests" | "test" | "__tests__" | "spec" | "benches"));
    let name = lower.rsplit('/').next().unwrap_or("");
    let test_name = name.starts_with("test_")
        || name.contains("_test.")
        || name.contains(".test.")
        || name.contains(".spec.")
        || (language == Language::Java && name.ends_with("test.java"));
    in_test_dir || test_name
}
