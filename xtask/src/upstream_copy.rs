//! Whether the upstream copy of `ag-ui` has moved since it was last compared.
//!
//! `crates/ag-ui` is also proposed upstream, as `sdks/community/rust` in
//! ag-ui-protocol/ag-ui, and the two copies are kept in step by hand.
//! `UPSTREAM.md` records the sync point; this check reads the pull request's
//! head and says whether the copy there is still what the record says.
//!
//! It reports two things, and only one of them fails:
//!
//! * **moved** — a git tree under the copy differs from the recorded one, the
//!   copy declares another version, or the pull request is no longer open
//!   from the recorded branch. Somebody has to look. Exit 1.
//! * **ahead** — commits here, since the recorded local commit, that changed
//!   `crates/ag-ui/src`. Fixes land here first, so this is expected; it is
//!   listed so the port is not forgotten. Never a failure.
//!
//! Trees rather than commits: the pull request gets rebased, and a rebase that
//! leaves the copy alone moves the head commit and neither tree.
//!
//! `--publish-guard <version>` is the offline half: it refuses the version
//! the copy has reserved for its own first release, for `publish.yml`.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::drift::fetch;

/// The record, relative to the repo root.
pub const RECORD: &str = "UPSTREAM.md";

pub const EXIT_OK: u8 = 0;
/// The copy moved, or the version asked about is reserved.
pub const EXIT_MOVED: u8 = 1;

#[derive(Debug, Default, Clone)]
pub struct Args {
    /// Offline: refuse this version if the upstream copy reserved it.
    pub publish_guard: Option<String>,
}

/// The sync point, as `UPSTREAM.md` records it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    pub repo: String,
    pub pull: u64,
    /// `owner/repo:branch` the pull request is opened from.
    pub head: String,
    /// The copy's workspace inside `repo`.
    pub root: String,
    /// The directory compared, relative to each side's workspace root.
    pub path: String,
    pub commit: String,
    pub tree: String,
    pub root_tree: String,
    pub local_commit: String,
    /// Empty when nothing is reserved.
    pub reserved_version: String,
}

const KEYS: &[&str] = &[
    "repo",
    "pull",
    "head",
    "root",
    "path",
    "commit",
    "tree",
    "root_tree",
    "local_commit",
    "reserved_version",
];

/// Reads the record from the first ```` ```toml ```` block of `UPSTREAM.md`.
///
/// A handful of `key = "value"` lines does not need a TOML parser; it needs
/// to refuse anything it does not understand, so a typo in the record is an
/// error rather than a field silently left at its default.
pub fn parse(text: &str) -> Result<Record, String> {
    let block = text
        .split("```toml")
        .nth(1)
        .and_then(|rest| rest.split("```").next())
        .ok_or_else(|| format!("{RECORD} has no ```toml block with the sync point"))?;
    let mut values = std::collections::BTreeMap::new();
    for (number, line) in block.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (key, value) = line.split_once('=').ok_or_else(|| {
            format!(
                "{RECORD} sync point, line {}: expected `key = value`",
                number
            )
        })?;
        let key = key.trim();
        if !KEYS.contains(&key) {
            return Err(format!("{RECORD} sync point: unknown key `{key}`"));
        }
        let value = value.trim();
        let value = value
            .strip_prefix('"')
            .and_then(|value| value.strip_suffix('"'))
            .unwrap_or(value);
        if values.insert(key, value.to_owned()).is_some() {
            return Err(format!("{RECORD} sync point: `{key}` is set twice"));
        }
    }
    let mut take = |key: &str| {
        values
            .remove(key)
            .ok_or_else(|| format!("{RECORD} sync point: `{key}` is missing"))
    };
    let record = Record {
        repo: take("repo")?,
        pull: take("pull")?
            .parse()
            .map_err(|_| format!("{RECORD} sync point: `pull` is not a number"))?,
        head: take("head")?,
        root: take("root")?,
        path: take("path")?,
        commit: take("commit")?,
        tree: take("tree")?,
        root_tree: take("root_tree")?,
        local_commit: take("local_commit")?,
        reserved_version: take("reserved_version")?,
    };
    for (key, sha) in [
        ("commit", &record.commit),
        ("tree", &record.tree),
        ("root_tree", &record.root_tree),
        ("local_commit", &record.local_commit),
    ] {
        if sha.len() != 40 || !sha.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(format!(
                "{RECORD} sync point: `{key}` must be a full 40-character SHA, not `{sha}`"
            ));
        }
    }
    if !record.head.contains(':') {
        return Err(format!(
            "{RECORD} sync point: `head` must be `owner/repo:branch`, not `{}`",
            record.head
        ));
    }
    Ok(record)
}

pub fn run(args: Args) -> Result<u8, String> {
    let root = repo_root();
    let path = root.join(RECORD);
    let text = std::fs::read_to_string(&path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let record = parse(&text)?;

    if let Some(version) = args.publish_guard {
        let (exit, message) = publish_guard(&record, &version);
        println!("{message}");
        return Ok(exit);
    }

    let upstream = observe(&record).map_err(|e| {
        format!("the upstream copy could not be read, so nothing was compared: {e}")
    })?;
    let local = local(&record, &root);
    let assessment = assess(&record, &upstream);
    print!("{}", render(&record, &upstream, &local, &assessment));
    Ok(if assessment.moved.is_empty() {
        EXIT_OK
    } else {
        EXIT_MOVED
    })
}

/// Whether `version` may be published from here, and why.
pub fn publish_guard(record: &Record, version: &str) -> (u8, String) {
    let version = version.trim().trim_start_matches('v');
    if !record.reserved_version.is_empty() && version == record.reserved_version {
        return (
            EXIT_MOVED,
            format!(
                "ag-ui {version} is reserved: the upstream copy (PR {} on {}) declares it for its \
                 own first release.\nPublishing it from here would take the number first, and \
                 crates.io has no undo. Release another version, or update `reserved_version` \
                 in {RECORD} once upstream has published or given it up.",
                record.pull, record.repo
            ),
        );
    }
    (
        EXIT_OK,
        match record.reserved_version.as_str() {
            "" => format!("ag-ui {version}: the upstream copy reserves no version."),
            reserved => format!(
                "ag-ui {version} is not the version the upstream copy reserves ({reserved})."
            ),
        },
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PullState {
    Open,
    Merged,
    Closed,
}

/// What the pull request's head holds now.
#[derive(Debug, Clone)]
pub struct Upstream {
    pub state: PullState,
    /// `owner/repo:branch`, or empty when the head repository is gone.
    pub head: String,
    pub commit: String,
    /// The rest is only read while the pull request is open.
    pub tree: Option<String>,
    pub root_tree: Option<String>,
    pub version: Option<String>,
}

/// Reads the pull request, then the copy at its head commit.
fn observe(record: &Record) -> Result<Upstream, String> {
    let pull = fetch::get(&format!(
        "https://api.github.com/repos/{}/pulls/{}",
        record.repo, record.pull
    ))?;
    let pull: serde_json::Value = serde_json::from_str(&pull)
        .map_err(|e| format!("GitHub returned a pull request that is not JSON: {e}"))?;
    let text = |pointer: &str| {
        pull.pointer(pointer)
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .to_owned()
    };
    let state = if pull.get("merged").and_then(serde_json::Value::as_bool) == Some(true) {
        PullState::Merged
    } else if text("/state") == "open" {
        PullState::Open
    } else {
        PullState::Closed
    };
    let repo = text("/head/repo/full_name");
    let commit = text("/head/sha");
    if commit.is_empty() {
        return Err(format!("PR {} has no head commit", record.pull));
    }
    let mut upstream = Upstream {
        state,
        head: if repo.is_empty() {
            String::new()
        } else {
            format!("{repo}:{}", text("/head/ref"))
        },
        commit,
        tree: None,
        root_tree: None,
        version: None,
    };
    if state != PullState::Open || repo.is_empty() {
        return Ok(upstream);
    }
    let copy = format!("{}/{}", record.root, record.path);
    upstream.tree = Some(tree_at(&repo, &upstream.commit, &copy)?);
    upstream.root_tree = Some(tree_at(&repo, &upstream.commit, &record.root)?);
    upstream.version = Some(declared_version(&repo, &upstream.commit, record)?);
    Ok(upstream)
}

/// The git tree SHA of directory `path` at `commit`, from its parent's listing.
fn tree_at(repo: &str, commit: &str, path: &str) -> Result<String, String> {
    let (parent, name) = path.rsplit_once('/').unwrap_or(("", path));
    let listing = fetch::get(&format!(
        "https://api.github.com/repos/{repo}/contents/{parent}?ref={commit}"
    ))?;
    let listing: serde_json::Value = serde_json::from_str(&listing)
        .map_err(|e| format!("GitHub returned a directory listing that is not JSON: {e}"))?;
    listing
        .as_array()
        .and_then(|entries| {
            entries.iter().find(|entry| {
                entry.get("name").and_then(serde_json::Value::as_str) == Some(name)
                    && entry.get("type").and_then(serde_json::Value::as_str) == Some("dir")
            })
        })
        .and_then(|entry| entry.get("sha").and_then(serde_json::Value::as_str))
        .map(str::to_owned)
        .ok_or_else(|| format!("{repo}@{commit} has no directory {path}"))
}

/// The `ag-ui` version the copy declares: the crate manifest's own, or the
/// workspace's when the crate inherits it.
fn declared_version(repo: &str, commit: &str, record: &Record) -> Result<String, String> {
    let crate_dir = record
        .path
        .rsplit_once('/')
        .map_or("", |(parent, _)| parent);
    let raw = |file: &str| {
        fetch::get(&format!(
            "https://raw.githubusercontent.com/{repo}/{commit}/{}/{file}",
            record.root
        ))
    };
    let manifest = raw(&format!("{crate_dir}/Cargo.toml"))?;
    match manifest_version(&manifest, "package") {
        Some(Version::Literal(version)) => Ok(version),
        Some(Version::Workspace) => {
            match manifest_version(&raw("Cargo.toml")?, "workspace.package") {
                Some(Version::Literal(version)) => Ok(version),
                _ => Err(format!(
                    "{}/Cargo.toml at {repo}@{commit} declares no [workspace.package] version",
                    record.root
                )),
            }
        }
        None => Err(format!(
            "{}/{crate_dir}/Cargo.toml at {repo}@{commit} declares no [package] version",
            record.root
        )),
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Version {
    Literal(String),
    /// `version.workspace = true`
    Workspace,
}

/// The `version` under `[section]` of a Cargo manifest, read as lines.
fn manifest_version(manifest: &str, section: &str) -> Option<Version> {
    let header = format!("[{section}]");
    let mut inside = false;
    for line in manifest.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            inside = line == header;
            continue;
        }
        if !inside {
            continue;
        }
        let compact = line.replace(' ', "");
        if compact.starts_with("version.workspace=true") {
            return Some(Version::Workspace);
        }
        if let Some(value) = compact.strip_prefix("version=") {
            return Some(Version::Literal(value.trim_matches('"').to_owned()));
        }
    }
    None
}

/// What this checkout says about its own side. Best effort: a shallow clone
/// has no history to list, which is a note, never a failure.
#[derive(Debug, Clone, Default)]
pub struct Local {
    /// The tree of `path` at `HEAD`.
    pub tree: Option<String>,
    /// The tree of `path` at the recorded local commit.
    pub tree_at_sync: Option<String>,
    /// `%h %s` of each commit since the local commit that changed `path`.
    pub ahead: Option<Vec<String>>,
    pub note: Option<String>,
}

fn local(record: &Record, root: &Path) -> Local {
    let git = |args: &[&str]| -> Option<String> {
        let output = Command::new("git")
            .args(args)
            .current_dir(root)
            .output()
            .ok()?;
        output
            .status
            .success()
            .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
    };
    let mut local = Local {
        tree: git(&["rev-parse", &format!("HEAD:{}", record.path)]),
        ..Local::default()
    };
    local.tree_at_sync = git(&[
        "rev-parse",
        &format!("{}:{}", record.local_commit, record.path),
    ]);
    if local.tree_at_sync.is_none() {
        local.note = Some(format!(
            "{} is not in this clone's history (a shallow clone?); fetch it to list what \
             is ahead — `fetch-depth: 0` in CI",
            short(&record.local_commit)
        ));
        return local;
    }
    local.ahead = git(&[
        "log",
        "--format=%h %s",
        &format!("{}..HEAD", record.local_commit),
        "--",
        &record.path,
    ])
    .map(|log| log.lines().map(str::to_owned).collect());
    local
}

/// What the observation means for the record.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Assessment {
    /// Each way the copy no longer matches the record. Any one fails.
    pub moved: Vec<String>,
    /// Worth knowing, not failures.
    pub notes: Vec<String>,
}

pub fn assess(record: &Record, upstream: &Upstream) -> Assessment {
    let mut out = Assessment::default();
    match upstream.state {
        PullState::Open => {}
        PullState::Merged => out.moved.push(format!(
            "PR {} was merged: the copy now lives on {}'s default branch. Point {RECORD} \
             there.",
            record.pull, record.repo
        )),
        PullState::Closed => out.moved.push(format!(
            "PR {} was closed without merging. Decide what the copy is now, and update \
             {RECORD}.",
            record.pull
        )),
    }
    if upstream.state == PullState::Open && upstream.head != record.head {
        out.moved.push(format!(
            "the pull request is now opened from {}, not {}",
            if upstream.head.is_empty() {
                "a deleted repository"
            } else {
                &upstream.head
            },
            record.head
        ));
    }
    let copy = format!("{}/{}", record.root, record.path);
    match (&upstream.tree, &upstream.root_tree) {
        (Some(tree), _) if *tree != record.tree => out.moved.push(format!(
            "{copy} is tree {} at {}; the record has {} at {}",
            short(tree),
            short(&upstream.commit),
            short(&record.tree),
            short(&record.commit)
        )),
        (_, Some(root_tree)) if *root_tree != record.root_tree => out.moved.push(format!(
            "{} changed outside {}: tree {} at {}, the record has {} — manifests, tests, \
             docs or tooling",
            record.root,
            record.path,
            short(root_tree),
            short(&upstream.commit),
            short(&record.root_tree)
        )),
        _ => {}
    }
    if let Some(version) = &upstream.version {
        if *version != record.reserved_version {
            out.moved.push(format!(
                "the copy declares ag-ui {version}; the record reserves {}",
                match record.reserved_version.as_str() {
                    "" => "nothing",
                    reserved => reserved,
                }
            ));
        }
    }
    if out.moved.is_empty() && upstream.commit != record.commit {
        out.notes.push(format!(
            "the head moved to {} (a rebase, or a change outside {}) but the copy did not; \
             nothing to port. Move `commit` along at the next sync.",
            short(&upstream.commit),
            record.root
        ));
    }
    out
}

pub fn render(
    record: &Record,
    upstream: &Upstream,
    local: &Local,
    assessment: &Assessment,
) -> String {
    let copy = format!("{}/{}", record.root, record.path);
    let mut out = String::from("upstream-sync\n");
    out.push_str(&format!(
        "  record    {RECORD}  (PR {} head {}, {} tree {})\n",
        record.pull,
        short(&record.commit),
        record.path,
        short(&record.tree)
    ));
    out.push_str(&format!(
        "  upstream  {} @ {}  ({})\n",
        if upstream.head.is_empty() {
            "(head repository deleted)"
        } else {
            &upstream.head
        },
        short(&upstream.commit),
        match upstream.state {
            PullState::Open => "open",
            PullState::Merged => "merged",
            PullState::Closed => "closed",
        }
    ));
    out.push_str(&format!(
        "  local     HEAD {} tree {}\n",
        record.path,
        local.tree.as_deref().map_or("(unread)".to_owned(), short)
    ));

    if assessment.moved.is_empty() {
        out.push_str("\nUPSTREAM  unchanged since the recorded sync point\n");
        out.push_str(&format!("    {copy:<44}tree {}\n", short(&record.tree)));
        out.push_str(&format!(
            "    {:<44}tree {}\n",
            record.root,
            short(&record.root_tree)
        ));
        if !record.reserved_version.is_empty() {
            out.push_str(&format!(
                "    {:<44}{} (reserved: publish.yml refuses it here)\n",
                "ag-ui version", record.reserved_version
            ));
        }
    } else {
        out.push_str(&format!("\nMOVED — {}\n", assessment.moved.len()));
        for moved in &assessment.moved {
            out.push_str(&crate::drift::text::wrap(moved, "    ", 80));
        }
        out.push('\n');
        if let Some((repo, _)) = upstream.head.split_once(':') {
            if upstream.commit != record.commit {
                out.push_str(&format!(
                    "  Compare: https://github.com/{repo}/compare/{}...{}\n",
                    record.commit, upstream.commit
                ));
            }
        }
        out.push_str(
            "  Read what changed, port it here or record it in UPSTREAM.md as an intended\n  \
             difference, then update the sync point there.\n",
        );
    }
    for note in &assessment.notes {
        out.push('\n');
        out.push_str(&crate::drift::text::wrap(note, "  ", 80));
    }

    match (&local.tree, &local.ahead) {
        (Some(tree), _) if *tree == record.tree => {
            out.push_str(&format!(
                "\nLOCAL     identical: HEAD's {} is the recorded tree\n",
                record.path
            ));
        }
        (_, Some(ahead)) => {
            out.push_str(&format!(
                "\nAHEAD — {} commit(s) since {} changed {} here\n",
                ahead.len(),
                short(&record.local_commit),
                record.path
            ));
            out.push_str(
                "  Expected: fixes land here first. Each one still has to reach the pull\n  \
                 request, or be recorded in UPSTREAM.md as an intended difference.\n\n",
            );
            for commit in ahead {
                out.push_str(&format!("    {commit}\n"));
            }
            if local.tree_at_sync.as_deref() != Some(record.tree.as_str()) {
                out.push_str(&format!(
                    "\n  {} was not byte-identical with the recorded tree either (a partial\n  \
                     sync), so some of these may already be upstream.\n",
                    short(&record.local_commit)
                ));
            }
        }
        _ => {}
    }
    if let Some(note) = &local.note {
        out.push('\n');
        out.push_str(&crate::drift::text::wrap(note, "  ", 80));
    }

    out.push('\n');
    if assessment.moved.is_empty() {
        out.push_str("OK  the upstream copy has not moved since the recorded sync point.\n");
    } else {
        out.push_str("FAILED  the upstream copy moved since the recorded sync point.\n");
    }
    out
}

fn short(sha: &str) -> String {
    sha.chars().take(10).collect()
}

/// The repo root, from the compile-time location of this crate.
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask/ always has a parent")
        .to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::*;

    const COMMIT: &str = "564be451743105db9c46ef775dc90bdc69f30e2f";
    const TREE: &str = "53421a174bf9298fc72cff9d629a30a71fe3f80a";
    const ROOT_TREE: &str = "8f7dfab5985cf0a44b764bf2288c77233a94723b";
    const OTHER: &str = "0123456789abcdef0123456789abcdef01234567";

    fn record() -> Record {
        parse(include_str!("../../UPSTREAM.md")).unwrap()
    }

    fn open() -> Upstream {
        Upstream {
            state: PullState::Open,
            head: "KimSoungRyoul/ag-ui:feat/rust-sdk-migration".into(),
            commit: COMMIT.into(),
            tree: Some(TREE.into()),
            root_tree: Some(ROOT_TREE.into()),
            version: Some("0.5.0".into()),
        }
    }

    /// The committed record is what the scheduled job reads; it must parse.
    #[test]
    fn the_committed_record_parses() {
        let record = record();
        assert_eq!(record.pull, 2778);
        assert_eq!(record.root, "sdks/community/rust");
        assert_eq!(record.path, "crates/ag-ui/src");
        assert_eq!(record.reserved_version, "0.5.0");
    }

    #[test]
    fn a_record_with_a_typo_or_a_gap_is_refused() {
        let text = include_str!("../../UPSTREAM.md");
        let typo = text.replace("root_tree =", "roottree =");
        assert!(parse(&typo).unwrap_err().contains("unknown key `roottree`"));
        let gap = text.replace("reserved_version = \"0.5.0\"", "");
        assert!(
            parse(&gap)
                .unwrap_err()
                .contains("`reserved_version` is missing")
        );
        let short_sha = text.replace(TREE, "53421a1");
        assert!(
            parse(&short_sha)
                .unwrap_err()
                .contains("full 40-character SHA")
        );
        assert!(parse("# no block").unwrap_err().contains("```toml"));
    }

    #[test]
    fn an_unchanged_copy_passes() {
        let assessment = assess(&record(), &open());
        assert_eq!(assessment, Assessment::default());
    }

    /// A rebase moves the head commit, not the trees. Not a failure.
    #[test]
    fn a_rebase_that_leaves_the_copy_alone_is_a_note() {
        let upstream = Upstream {
            commit: OTHER.into(),
            ..open()
        };
        let assessment = assess(&record(), &upstream);
        assert!(assessment.moved.is_empty());
        assert_eq!(assessment.notes.len(), 1);
        assert!(assessment.notes[0].contains("nothing to port"));
    }

    #[test]
    fn a_changed_source_tree_is_moved() {
        let upstream = Upstream {
            commit: OTHER.into(),
            tree: Some(OTHER.into()),
            root_tree: Some(OTHER.into()),
            ..open()
        };
        let assessment = assess(&record(), &upstream);
        assert_eq!(assessment.moved.len(), 1, "{assessment:?}");
        assert!(
            assessment.moved[0].contains("sdks/community/rust/crates/ag-ui/src is tree 0123456789"),
            "{assessment:?}"
        );
        let text = render(&record(), &upstream, &Local::default(), &assessment);
        assert!(text.contains("MOVED — 1"), "{text}");
        assert!(
            text.contains(&format!(
                "https://github.com/KimSoungRyoul/ag-ui/compare/{COMMIT}...{OTHER}"
            )),
            "{text}"
        );
        assert!(text.contains("FAILED"), "{text}");
    }

    #[test]
    fn a_change_outside_the_source_tree_is_moved_too() {
        let upstream = Upstream {
            root_tree: Some(OTHER.into()),
            ..open()
        };
        let assessment = assess(&record(), &upstream);
        assert_eq!(assessment.moved.len(), 1);
        assert!(assessment.moved[0].contains("changed outside crates/ag-ui/src"));
    }

    #[test]
    fn a_new_version_a_merge_and_a_new_head_are_each_moved() {
        let version = Upstream {
            version: Some("0.6.0".into()),
            ..open()
        };
        assert!(assess(&record(), &version).moved[0].contains("declares ag-ui 0.6.0"));

        let merged = Upstream {
            state: PullState::Merged,
            tree: None,
            root_tree: None,
            version: None,
            ..open()
        };
        let moved = assess(&record(), &merged).moved;
        assert_eq!(moved.len(), 1, "{moved:?}");
        assert!(moved[0].contains("was merged"));

        let elsewhere = Upstream {
            head: "someone/ag-ui:rust".into(),
            ..open()
        };
        assert!(assess(&record(), &elsewhere).moved[0].contains("now opened from someone/ag-ui"));
    }

    #[test]
    fn local_commits_since_the_sync_point_are_listed_and_never_fail() {
        let local = Local {
            tree: Some(OTHER.into()),
            tree_at_sync: Some(TREE.into()),
            ahead: Some(vec!["85b5970 [#20] fix(core): subagents".into()]),
            note: None,
        };
        let text = render(&record(), &open(), &local, &assess(&record(), &open()));
        assert!(
            text.contains("AHEAD — 1 commit(s) since 520a724ccc"),
            "{text}"
        );
        assert!(text.contains("85b5970 [#20]"), "{text}");
        assert!(!text.contains("partial"), "{text}");
        assert!(
            text.contains("OK  the upstream copy has not moved"),
            "{text}"
        );

        let identical = Local {
            tree: Some(TREE.into()),
            ..Local::default()
        };
        let text = render(&record(), &open(), &identical, &Assessment::default());
        assert!(text.contains("LOCAL     identical"), "{text}");
    }

    #[test]
    fn the_publish_guard_refuses_only_the_reserved_version() {
        let record = record();
        let (exit, message) = publish_guard(&record, "0.5.0");
        assert_eq!(exit, EXIT_MOVED);
        assert!(message.contains("ag-ui 0.5.0 is reserved"), "{message}");
        assert_eq!(publish_guard(&record, "v0.5.0").0, EXIT_MOVED);
        assert_eq!(publish_guard(&record, "0.5.0-alpha.3").0, EXIT_OK);
        assert_eq!(publish_guard(&record, "0.5.1").0, EXIT_OK);

        let open = Record {
            reserved_version: String::new(),
            ..record
        };
        assert_eq!(publish_guard(&open, "0.5.0").0, EXIT_OK);
    }

    #[test]
    fn reads_the_version_a_manifest_declares_or_inherits() {
        let member = "[package]\nname = \"ag-ui\"\nversion.workspace = true\n\n[features]\nversion = \"no\"\n";
        assert_eq!(
            manifest_version(member, "package"),
            Some(Version::Workspace)
        );
        let workspace = "[workspace]\nmembers = []\n\n[workspace.package]\nversion = \"0.5.0\"\nedition = \"2024\"\n";
        assert_eq!(
            manifest_version(workspace, "workspace.package"),
            Some(Version::Literal("0.5.0".into()))
        );
        assert_eq!(manifest_version(workspace, "package"), None);
    }
}
