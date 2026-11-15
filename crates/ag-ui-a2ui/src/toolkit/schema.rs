//! Few-shot example files for a prompt, and the schema documents re-exported.
//!
//! The three schema documents and the surgery on them live in
//! [`crate::schema`], below this layer: a validator and a prompt both read
//! them. This module keeps the one thing that touches the filesystem,
//! [`load_examples`], and re-exports the rest under the paths it had.

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::error::{Error, Result};

pub use crate::protocol::schema::{
    SCHEMA_BLOCK_END, SCHEMA_BLOCK_START, SchemaBundle, remove_strict_validation,
};

/// Loads few-shot examples for a prompt.
///
/// `path` is a directory (every `*.json` inside it) or a glob pattern. Each
/// example is wrapped in `---BEGIN <stem>--- / ---END <stem>---` markers so the
/// model can tell where one ends and the next begins, and files are read in
/// sorted order so the prompt is byte-stable across runs.
///
/// A path that matches nothing yields an empty string: examples are an
/// optimization, and a missing directory should not take an agent down.
///
/// # Errors
///
/// With `validate` set, returns [`Error::Catalog`] if an example is not valid
/// JSON.
pub fn load_examples(path: Option<&Path>, validate: bool) -> Result<String> {
    let Some(path) = path else {
        return Ok(String::new());
    };
    let mut matches = if path.is_dir() {
        collect_matches(path, "*.json")
    } else {
        let pattern = path.to_string_lossy().to_string();
        let (base, pattern) = split_pattern(&pattern);
        collect_matches(&base, &pattern)
    };
    matches.sort();

    let mut blocks = Vec::new();
    for file in matches {
        if !file.is_file() {
            continue;
        }
        let stem = file
            .file_stem()
            .map(|stem| stem.to_string_lossy().to_string())
            .unwrap_or_default();
        let content = std::fs::read_to_string(&file).map_err(|e| {
            Error::catalog(format!("Failed to read example {}: {e}", file.display()))
        })?;
        if validate {
            serde_json::from_str::<Value>(&content).map_err(|e| {
                Error::catalog(format!(
                    "Failed to validate example {}: {e}",
                    file.display()
                ))
            })?;
        }
        blocks.push(format!("---BEGIN {stem}---\n{content}\n---END {stem}---"));
    }
    Ok(blocks.join("\n\n"))
}

/// Splits a glob pattern into its fixed base directory and the pattern tail.
///
/// Works on the string rather than on `PathBuf`, so a leading `/` survives:
/// pushing the empty first segment of an absolute path onto a `PathBuf` would
/// silently make it relative.
fn split_pattern(pattern: &str) -> (PathBuf, String) {
    let segments: Vec<&str> = pattern.split('/').collect();
    let first_wildcard = segments
        .iter()
        .position(|segment| segment.contains(['*', '?', '[']));

    let split_at = match first_wildcard {
        Some(index) => index,
        // No wildcard: the last segment is the name to match.
        None => segments.len().saturating_sub(1),
    };
    let base = segments[..split_at].join("/");
    let base = match base.as_str() {
        "" if pattern.starts_with('/') => PathBuf::from("/"),
        "" => PathBuf::from("."),
        other => PathBuf::from(other),
    };
    (base, segments[split_at..].join("/"))
}

/// Walks `base` collecting files matching `pattern`.
fn collect_matches(base: &Path, pattern: &str) -> Vec<PathBuf> {
    let recursive = pattern.contains("**");
    let mut out = Vec::new();
    let mut stack = vec![base.to_path_buf()];

    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if recursive {
                    stack.push(path);
                }
                continue;
            }
            let relative = path.strip_prefix(base).unwrap_or(&path).to_string_lossy();
            if glob_match(pattern, &relative) {
                out.push(path);
            }
        }
    }
    out
}

/// Matches a path against a glob pattern.
///
/// Supports `*`, `?`, character classes with ranges and `!` negation, and `**`
/// for "any number of directories". `*` alone never crosses a `/`, which is
/// what keeps `dir/*.json` from reaching into subdirectories.
pub(crate) fn glob_match(pattern: &str, text: &str) -> bool {
    // `**/` matches zero or more directories, so try it both ways.
    if let Some(rest) = pattern.strip_prefix("**/") {
        if glob_match(rest, text) {
            return true;
        }
        if let Some((_, tail)) = text.split_once('/') {
            return glob_match(pattern, tail);
        }
        return false;
    }
    match_here(
        &pattern.chars().collect::<Vec<_>>(),
        &text.chars().collect::<Vec<_>>(),
    )
}

fn match_here(pattern: &[char], text: &[char]) -> bool {
    let mut p = 0;
    let mut t = 0;
    // Backtracking point for the most recent `*`.
    let mut star: Option<(usize, usize)> = None;

    while t < text.len() {
        match pattern.get(p) {
            Some('*') => {
                star = Some((p, t));
                p += 1;
            }
            Some('?') if text[t] != '/' => {
                p += 1;
                t += 1;
            }
            Some('[') => match match_class(pattern, p, text[t]) {
                Some(next) => {
                    p = next;
                    t += 1;
                }
                None => match retry(&mut star, &mut p, &mut t, text) {
                    true => continue,
                    false => return false,
                },
            },
            Some(ch) if *ch == text[t] => {
                p += 1;
                t += 1;
            }
            _ => {
                if !retry(&mut star, &mut p, &mut t, text) {
                    return false;
                }
            }
        }
    }
    while pattern.get(p) == Some(&'*') {
        p += 1;
    }
    p == pattern.len()
}

/// Resumes from the last `*`, consuming one more character.
fn retry(star: &mut Option<(usize, usize)>, p: &mut usize, t: &mut usize, text: &[char]) -> bool {
    match star {
        // A single `*` never matches a path separator.
        Some((sp, st)) if text[*st] != '/' => {
            *p = *sp + 1;
            *st += 1;
            *t = *st;
            true
        }
        _ => false,
    }
}

/// Matches one character class starting at `open`, returning the index after it.
fn match_class(pattern: &[char], open: usize, ch: char) -> Option<usize> {
    let mut i = open + 1;
    let negated = matches!(pattern.get(i), Some('!') | Some('^'));
    if negated {
        i += 1;
    }
    let mut matched = false;
    let mut first = true;
    while i < pattern.len() {
        if pattern[i] == ']' && !first {
            return (matched != negated).then_some(i + 1);
        }
        first = false;
        // A range, unless the `-` is the last character before `]`.
        if pattern.get(i + 1) == Some(&'-') && pattern.get(i + 2).is_some_and(|c| *c != ']') {
            let (low, high) = (pattern[i], pattern[i + 2]);
            if low <= ch && ch <= high {
                matched = true;
            }
            i += 3;
            continue;
        }
        if pattern[i] == ch {
            matched = true;
        }
        i += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn globs_cover_stars_classes_ranges_and_negation() {
        assert!(glob_match("*.json", "example1.json"));
        assert!(!glob_match("*.json", "notes.txt"));
        assert!(glob_match("user_*.json", "user_profile.json"));
        assert!(!glob_match("user_*.json", "admin_profile.json"));
        assert!(glob_match("step[1-2].json", "step1.json"));
        assert!(!glob_match("step[1-2].json", "step3.json"));
        assert!(glob_match("[!i]*.json", "visible.json"));
        assert!(!glob_match("[!i]*.json", "index.json"));
        assert!(glob_match("**/*.json", "top.json"));
        assert!(glob_match("**/*.json", "nested/deep.json"));
        // A bare star must not cross a directory boundary.
        assert!(!glob_match("*.json", "nested/deep.json"));
        assert!(glob_match("a?c.json", "abc.json"));
    }

    #[test]
    fn a_missing_examples_path_is_not_an_error() {
        assert_eq!(load_examples(None, false).unwrap(), "");
        assert_eq!(
            load_examples(Some(Path::new("/no/such/place")), false).unwrap(),
            ""
        );
    }
}
