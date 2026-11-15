//! Whether the vendored AG-UI 1.0 corpus is what its record says, and whether
//! upstream has moved since.
//!
//! `crates/ag-ui/tests/fixtures/ag-ui-1.0/` is a copy of upstream's
//! `spec/1.0` conformance streams and schema fixtures. `SOURCE.json` there
//! records the commit, the git tree of each directory at that commit, and
//! what was left out. This check answers two questions, by git object id:
//!
//! * **differs** — a vendored file is not the upstream file at the recorded
//!   commit, is missing, or is one upstream does not have. Somebody edited
//!   the copy, or the record is wrong.
//! * **moved** — a directory at upstream's default branch is no longer the
//!   recorded tree. The corpus has to be vendored again.
//!
//! Either fails. Both need the network, so this runs in the scheduled
//! `upstream freshness` job; the offline half — the directory against
//! upstream's `MANIFEST.txt` — is in the tests that run the corpus.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Deserialize;

use crate::drift::fetch;

/// The vendored directory, relative to the repo root.
pub const VENDORED: &str = "crates/ag-ui/tests/fixtures/ag-ui-1.0";

pub const EXIT_OK: u8 = 0;
/// The copy differs from the record, or upstream moved.
pub const EXIT_STALE: u8 = 1;

/// `SOURCE.json`.
#[derive(Debug, Deserialize)]
pub struct Record {
    pub source: Pin,
    pub trees: Vec<Tree>,
    pub files: Vec<File>,
    pub excluded: Vec<Excluded>,
}

/// The same keys as the schema baseline's `source`, less `path`.
#[derive(Debug, Deserialize)]
pub struct Pin {
    pub repo: String,
    pub commit: String,
    pub commit_date: String,
    pub fetched_at: String,
}

/// A vendored directory: `local` under [`VENDORED`], `path` upstream.
#[derive(Debug, Deserialize)]
pub struct Tree {
    pub local: String,
    pub path: String,
    pub tree: String,
}

/// A vendored file outside those directories.
#[derive(Debug, Deserialize)]
pub struct File {
    pub local: String,
    pub path: String,
    pub blob: String,
}

/// An upstream file deliberately not vendored.
#[derive(Debug, Deserialize)]
pub struct Excluded {
    pub path: String,
    pub reason: String,
}

/// File path to git blob id.
type Listing = BTreeMap<String, String>;

/// How a vendored directory disagrees with the upstream tree it was copied
/// from, the exclusions aside. Empty when it is that tree.
pub fn differences(
    tree: &Tree,
    local: &Listing,
    upstream: &Listing,
    excluded: &[Excluded],
) -> Vec<String> {
    let left_out = |path: &str| {
        excluded
            .iter()
            .any(|entry| entry.path == format!("{}/{path}", tree.path))
    };
    let mut out = Vec::new();
    for (path, blob) in upstream {
        match local.get(path) {
            _ if left_out(path) => {
                if local.contains_key(path) {
                    out.push(format!(
                        "{}/{path} is vendored, but SOURCE.json lists it as left out",
                        tree.local
                    ));
                }
            }
            None => out.push(format!("{}/{path} is missing", tree.local)),
            Some(here) if here != blob => out.push(format!(
                "{}/{path} is not the file at the recorded commit",
                tree.local
            )),
            Some(_) => {}
        }
    }
    for path in local.keys().filter(|path| !upstream.contains_key(*path)) {
        out.push(format!(
            "{}/{path} is not in upstream's {}",
            tree.local, tree.path
        ));
    }
    for entry in excluded {
        let inside = entry.path.strip_prefix(&format!("{}/", tree.path));
        if inside.is_some_and(|path| !upstream.contains_key(path)) {
            out.push(format!(
                "SOURCE.json leaves out {}, which upstream does not have",
                entry.path
            ));
        }
    }
    out
}

pub fn run() -> Result<u8, String> {
    let root = repo_root();
    let path = root.join(VENDORED).join("SOURCE.json");
    let text = std::fs::read_to_string(&path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let record: Record = serde_json::from_str(&text)
        .map_err(|e| format!("{} is not the record this check reads: {e}", path.display()))?;
    let source = &record.source;
    let repo = &source.repo;

    let mut differs = Vec::new();
    let mut moved = Vec::new();
    let branch = default_branch(repo)?;
    println!(
        "corpus-check\n  record    {VENDORED}/SOURCE.json  ({repo} @ {}, committed {}, copied {})\n  \
         upstream  {branch}",
        short(&source.commit),
        source.commit_date,
        source.fetched_at
    );

    for tree in &record.trees {
        let local = local_listing(&root, &format!("{VENDORED}/{}", tree.local))?;
        let upstream = upstream_listing(repo, &tree.tree)?;
        differs.extend(differences(tree, &local, &upstream, &record.excluded));
        let head = entry_at(repo, &branch, &tree.path, "dir")?;
        if head != tree.tree {
            moved.push(format!(
                "{} is tree {} on {branch}; the record has {}",
                tree.path,
                short(&head),
                short(&tree.tree)
            ));
        }
        println!(
            "  {:<10}{} file(s), tree {}",
            tree.local,
            local.len(),
            short(&tree.tree)
        );
    }
    for entry in &record.excluded {
        println!("  left out  {}\n            {}", entry.path, entry.reason);
    }
    for file in &record.files {
        let local = git(
            &root,
            &["rev-parse", &format!("HEAD:{VENDORED}/{}", file.local)],
        )?;
        if local != file.blob {
            differs.push(format!(
                "{} is not the file at the recorded commit",
                file.local
            ));
        }
        let head = entry_at(repo, &branch, &file.path, "file")?;
        if head != file.blob {
            moved.push(format!("{} changed on {branch}", file.path));
        }
    }

    for (label, found, fix) in [
        (
            "DIFFERS",
            &differs,
            "Restore the files from the recorded commit, or correct SOURCE.json.",
        ),
        (
            "MOVED",
            &moved,
            "Copy both directories again from a newer commit and update SOURCE.json, the \
             README beside it and UPSTREAM_COMMIT in crates/ag-ui/tests/conformance_1_0.rs.",
        ),
    ] {
        if !found.is_empty() {
            println!("\n{label}  {} finding(s):", found.len());
            for line in found {
                println!("    {line}");
            }
            println!("  {fix}");
        }
    }
    if differs.is_empty() && moved.is_empty() {
        println!("\nOK  The vendored corpus is the recorded commit, and upstream has not moved.");
        return Ok(EXIT_OK);
    }
    Ok(EXIT_STALE)
}

/// Every file under `dir` at `HEAD`, by blob id.
fn local_listing(root: &Path, dir: &str) -> Result<Listing, String> {
    let listing = git(root, &["ls-tree", "-r", &format!("HEAD:{dir}")])?;
    listing
        .lines()
        .map(|line| {
            // `<mode> blob <id>\t<path>`
            let (meta, path) = line
                .split_once('\t')
                .ok_or("unexpected `git ls-tree` output")?;
            let id = meta
                .split(' ')
                .nth(2)
                .ok_or("unexpected `git ls-tree` output")?;
            Ok((path.to_owned(), id.to_owned()))
        })
        .collect()
}

/// Every file of upstream's tree `tree`, by blob id.
fn upstream_listing(repo: &str, tree: &str) -> Result<Listing, String> {
    let url = format!("https://api.github.com/repos/{repo}/git/trees/{tree}?recursive=1");
    let listing: serde_json::Value = serde_json::from_str(&fetch::get(&url)?)
        .map_err(|e| format!("GitHub returned a tree that is not JSON: {e}"))?;
    if listing
        .get("truncated")
        .and_then(serde_json::Value::as_bool)
        == Some(true)
    {
        return Err(format!(
            "{url} was truncated; the tree is too large to compare"
        ));
    }
    let entries = listing
        .get("tree")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| format!("{url} has no `tree`"))?;
    Ok(entries
        .iter()
        .filter(|entry| entry["type"] == "blob")
        .filter_map(|entry| {
            Some((
                entry["path"].as_str()?.to_owned(),
                entry["sha"].as_str()?.to_owned(),
            ))
        })
        .collect())
}

fn default_branch(repo: &str) -> Result<String, String> {
    let body = fetch::get(&format!("https://api.github.com/repos/{repo}"))?;
    let body: serde_json::Value = serde_json::from_str(&body)
        .map_err(|e| format!("GitHub returned a repository that is not JSON: {e}"))?;
    body.get("default_branch")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| format!("{repo} names no default branch"))
}

/// The git object id of `path` at `reference`, from its parent's listing.
fn entry_at(repo: &str, reference: &str, path: &str, kind: &str) -> Result<String, String> {
    let (parent, name) = path.rsplit_once('/').unwrap_or(("", path));
    let url = format!("https://api.github.com/repos/{repo}/contents/{parent}?ref={reference}");
    let listing: serde_json::Value = serde_json::from_str(&fetch::get(&url)?)
        .map_err(|e| format!("GitHub returned a directory listing that is not JSON: {e}"))?;
    listing
        .as_array()
        .and_then(|entries| {
            entries
                .iter()
                .find(|entry| entry["name"] == name && entry["type"] == kind)
        })
        .and_then(|entry| entry["sha"].as_str())
        .map(str::to_owned)
        .ok_or_else(|| format!("{repo}@{reference} has no {kind} {path}"))
}

fn git(root: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .map_err(|e| format!("cannot run git: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "`git {}` failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn short(sha: &str) -> &str {
    sha.get(..12).unwrap_or(sha)
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask/ always has a parent")
        .to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree() -> Tree {
        Tree {
            local: "streams".to_owned(),
            path: "spec/1.0/conformance/streams".to_owned(),
            tree: "36faadc4c40b0458e9bd0118ce007ca5087c028d".to_owned(),
        }
    }

    fn listing(files: &[(&str, &str)]) -> Listing {
        files
            .iter()
            .map(|(path, blob)| ((*path).to_owned(), (*blob).to_owned()))
            .collect()
    }

    fn left_out(path: &str) -> Vec<Excluded> {
        vec![Excluded {
            path: format!("spec/1.0/conformance/streams/{path}"),
            reason: "a reason".to_owned(),
        }]
    }

    #[test]
    fn the_committed_record_parses_and_names_full_object_ids() {
        let text = std::fs::read_to_string(repo_root().join(VENDORED).join("SOURCE.json")).unwrap();
        let record: Record = serde_json::from_str(&text).unwrap();
        let ids = std::iter::once(&record.source.commit)
            .chain(record.trees.iter().map(|tree| &tree.tree))
            .chain(record.files.iter().map(|file| &file.blob));
        for id in ids {
            assert!(
                id.len() == 40 && id.chars().all(|c| c.is_ascii_hexdigit()),
                "{id}"
            );
        }
        for tree in &record.trees {
            assert!(
                repo_root().join(VENDORED).join(&tree.local).is_dir(),
                "{}",
                tree.local
            );
        }
        for entry in &record.excluded {
            assert!(!entry.reason.is_empty(), "{} needs a reason", entry.path);
            assert!(
                record
                    .trees
                    .iter()
                    .any(|tree| entry.path.starts_with(&tree.path)),
                "{} is outside every vendored tree",
                entry.path
            );
        }
    }

    #[test]
    fn an_identical_copy_minus_the_exclusions_has_no_differences() {
        let upstream = listing(&[("a.json", "1"), ("b.json", "2"), ("MANIFEST.txt", "3")]);
        let local = listing(&[("a.json", "1"), ("MANIFEST.txt", "3")]);
        assert!(differences(&tree(), &local, &upstream, &left_out("b.json")).is_empty());
    }

    #[test]
    fn an_edited_a_missing_an_extra_and_a_wrongly_excluded_file_are_each_named() {
        let upstream = listing(&[("a.json", "1"), ("b.json", "2"), ("c.json", "3")]);
        let local = listing(&[("a.json", "edited"), ("c.json", "3"), ("mine.json", "4")]);
        let found = differences(&tree(), &local, &upstream, &left_out("c.json"));
        let said = |words: &str| found.iter().any(|line| line.contains(words));
        assert!(
            said("streams/a.json is not the file at the recorded commit"),
            "{found:?}"
        );
        assert!(said("streams/b.json is missing"), "{found:?}");
        assert!(said("streams/mine.json is not in upstream's"), "{found:?}");
        assert!(
            said("streams/c.json is vendored, but SOURCE.json lists it"),
            "{found:?}"
        );
        assert_eq!(found.len(), 4, "{found:?}");

        let found = differences(
            &tree(),
            &listing(&[]),
            &listing(&[]),
            &left_out("gone.json"),
        );
        assert!(
            found[0].contains("which upstream does not have"),
            "{found:?}"
        );
    }
}
