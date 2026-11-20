//! The surfaces that restate repository facts by hand, held to the repository.
//!
//! The skills in `skills/`, the Claude Code plugin manifests in
//! `.claude-plugin/` and the skills' `sources.md` each copy something the
//! repository already says: the workspace version, the README's dependency
//! lines, the paths of the files a section was written from. Nothing compiles a
//! version line or a list of paths, so they went stale without a sound —
//! skills on `0.5.0-alpha.1` and manifests on `0.4.1` while the workspace was
//! `0.5.0-alpha.3`, fifteen `sources.md` paths that had moved, and path
//! dependencies on crates already on crates.io. `e2e/src/skills.rs` compiles
//! the skills' Rust; this checks what they say about the repository.
//!
//! Each rule is reported against a file and, where there is one, a line:
//!
//! * **skill version** — every skill states `workspace version **X**`, and X is
//!   the `[workspace.package]` version in the root `Cargo.toml`.
//! * **plugin version** — `.claude-plugin/plugin.json` and every plugin listed
//!   in `.claude-plugin/marketplace.json` carry the workspace version.
//! * **sources path** — every backticked path in a skill's `sources.md` exists.
//! * **dependency block** — every `toml` block in a skill is one of the
//!   README's, and every `ag-ui` / `ag-ui-a2ui` line in either takes the crate
//!   from crates.io at the workspace version. Prereleases use an exact requirement.
//!   Only the README may also show local-development `path` dependencies.
//! * **retired crate name** — `e2e/` and `examples/` never name the crates that
//!   were merged into `ag-ui`. Elsewhere `ag-ui-core` and friends are the
//!   unrelated community SDK and are named on purpose; in those two directories
//!   they can only mean this SDK before the merge. `ag-ui-a2ui`'s
//!   `ag-ui-server` *feature* is not a crate name and passes.

use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

/// Exit codes: 0 clean, 1 something is stale. A check that could not run at
/// all returns `Err`, which `main` turns into 2.
pub const EXIT_CLEAN: u8 = 0;
pub const EXIT_STALE: u8 = 1;

const WORKSPACE_MANIFEST: &str = "Cargo.toml";
const README: &str = "README.md";
const SKILLS_DIR: &str = "skills";
const PLUGIN_MANIFEST: &str = ".claude-plugin/plugin.json";
const MARKETPLACE_MANIFEST: &str = ".claude-plugin/marketplace.json";

/// Skills about the skill set rather than the SDK, which therefore state no
/// version of their own.
const VERSIONLESS_SKILLS: &[&str] = &["ag-ui-rust-update"];

/// The packages this workspace publishes, as dependency keys.
const OUR_CRATES: &[&str] = &["ag-ui", "ag-ui-a2ui"];

/// Where a retired crate name can only mean this SDK before the merge.
const RETIRED_NAME_DIRS: &[&str] = &["e2e", "examples"];

/// Crates merged into `ag-ui`, in both spellings, with what replaced each.
const RETIRED_CRATES: &[(&str, &str)] = &[
    ("ag-ui-core", "the `ag_ui` crate root"),
    ("ag_ui_core", "the `ag_ui` crate root"),
    ("ag-ui-client", "`ag_ui::client`"),
    ("ag_ui_client", "`ag_ui::client`"),
    ("ag-ui-server", "`ag_ui::server`"),
    ("ag_ui_server", "`ag_ui::server`"),
    ("ag-ui-axum", "`ag_ui::axum`"),
    ("ag_ui_axum", "`ag_ui::axum`"),
];

/// One stale statement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    /// Repository-relative, `/`-separated.
    pub file: String,
    /// 1-based, when the statement has a line.
    pub line: Option<usize>,
    pub message: String,
}

impl Finding {
    fn at(file: &str, line: usize, message: String) -> Self {
        Self {
            file: file.to_owned(),
            line: Some(line),
            message,
        }
    }

    fn file(file: &str, message: String) -> Self {
        Self {
            file: file.to_owned(),
            line: None,
            message,
        }
    }
}

impl fmt::Display for Finding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.line {
            Some(line) => write!(f, "{}:{line}: {}", self.file, self.message),
            None => write!(f, "{}: {}", self.file, self.message),
        }
    }
}

/// What a run found.
#[derive(Debug)]
pub struct Report {
    pub version: String,
    pub findings: Vec<Finding>,
}

/// Runs the check against this repository and prints the result.
pub fn run(options: &[String]) -> Result<u8, String> {
    if let Some(option) = options.first() {
        return Err(format!("docs-check takes no options; got `{option}`"));
    }
    let report = check(&repo_root())?;

    println!(
        "WORKSPACE VERSION  {}  ({WORKSPACE_MANIFEST})",
        report.version
    );
    if report.findings.is_empty() {
        println!(
            "OK  skills, plugin manifests, sources.md paths and dependency blocks agree with the repository."
        );
        return Ok(EXIT_CLEAN);
    }
    println!();
    for finding in &report.findings {
        println!("  {finding}");
    }
    println!(
        "\n{} stale statement(s). Fix the file named, not this check: each rule is listed in xtask/src/docs_check.rs.",
        report.findings.len()
    );
    Ok(EXIT_STALE)
}

/// Checks the repository at `root`. `Err` means the check could not run.
pub fn check(root: &Path) -> Result<Report, String> {
    let manifest = read(root, WORKSPACE_MANIFEST)?;
    let version = workspace_version(&manifest)
        .ok_or_else(|| format!("no `version` under [workspace.package] in {WORKSPACE_MANIFEST}"))?;
    let mut findings = Vec::new();

    let readme = read(root, README)?;
    let readme_blocks = fenced_blocks(&readme, "toml");
    for (line, block) in &readme_blocks {
        for (offset, message) in dependency_problems(block, &version, Local::Allowed) {
            findings.push(Finding::at(README, line + offset, message));
        }
    }

    for skill in skill_names(root)? {
        let skill_md = format!("{SKILLS_DIR}/{skill}/SKILL.md");
        let text = read(root, &skill_md)?;
        let stated = stated_versions(&text);
        if stated.is_empty() && !VERSIONLESS_SKILLS.contains(&skill.as_str()) {
            findings.push(Finding::file(
                &skill_md,
                format!("states no `workspace version **…**` line; add one naming {version}"),
            ));
        }
        for (line, stated) in stated {
            if stated != version {
                findings.push(Finding::at(
                    &skill_md,
                    line,
                    format!("states workspace version {stated}; the workspace is {version}"),
                ));
            }
        }

        for page in skill_pages(root, &skill)? {
            let text = read(root, &page)?;
            for (line, block) in fenced_blocks(&text, "toml") {
                if !readme_blocks.iter().any(|(_, theirs)| *theirs == block) {
                    findings.push(Finding::at(
                        &page,
                        line,
                        format!(
                            "this `toml` block is not one of {README}'s; copy the README's block verbatim"
                        ),
                    ));
                }
                for (offset, message) in dependency_problems(&block, &version, Local::Refused) {
                    findings.push(Finding::at(&page, line + offset, message));
                }
            }
        }

        let sources = format!("{SKILLS_DIR}/{skill}/sources.md");
        if root.join(&sources).is_file() {
            for (line, path) in backticked_paths(&read(root, &sources)?) {
                if !root.join(&path).exists() {
                    findings.push(Finding::at(
                        &sources,
                        line,
                        format!("`{path}` does not exist"),
                    ));
                }
            }
        }
    }

    for (file, versions) in [
        (
            PLUGIN_MANIFEST,
            plugin_versions(&read(root, PLUGIN_MANIFEST)?),
        ),
        (
            MARKETPLACE_MANIFEST,
            marketplace_versions(&read(root, MARKETPLACE_MANIFEST)?),
        ),
    ] {
        for (field, stated) in versions.map_err(|e| format!("{file}: {e}"))? {
            match stated {
                Some(stated) if stated == version => {}
                Some(stated) => findings.push(Finding::file(
                    file,
                    format!("`{field}` is {stated}; the workspace is {version}"),
                )),
                None => findings.push(Finding::file(
                    file,
                    format!("`{field}` is missing; the workspace is {version}"),
                )),
            }
        }
    }

    for dir in RETIRED_NAME_DIRS {
        for file in source_files(root, dir)? {
            for (line, name, replacement) in retired_names(&read(root, &file)?) {
                findings.push(Finding::at(
                    &file,
                    line,
                    format!("`{name}` was merged into `ag-ui`; write {replacement}"),
                ));
            }
        }
    }

    Ok(Report { version, findings })
}

/// The `version` under `[workspace.package]`.
pub fn workspace_version(manifest: &str) -> Option<String> {
    let mut in_section = false;
    for line in manifest.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_section = line == "[workspace.package]";
            continue;
        }
        if !in_section {
            continue;
        }
        let Some(rest) = line.strip_prefix("version") else {
            continue;
        };
        if let Some(value) = rest.trim_start().strip_prefix('=') {
            return quoted(value.trim());
        }
    }
    None
}

/// Every `workspace version **X**` in a skill, with the line X is on. The
/// phrase may wrap between any two of its words.
pub fn stated_versions(text: &str) -> Vec<(usize, String)> {
    let words: Vec<(usize, &str)> = text
        .lines()
        .enumerate()
        .flat_map(|(index, line)| line.split_whitespace().map(move |word| (index + 1, word)))
        .collect();
    words
        .windows(3)
        .filter_map(|window| {
            let [(_, first), (_, second), (line, third)] = window else {
                return None;
            };
            if !first.eq_ignore_ascii_case("workspace") || *second != "version" {
                return None;
            }
            let version = third.strip_prefix("**")?.split("**").next()?;
            Some((*line, version.to_owned()))
        })
        .collect()
}

/// Every fenced block whose info string starts with `lang`, as the 1-based
/// line of its first content line and its content, trailing spaces trimmed.
pub fn fenced_blocks(text: &str, lang: &str) -> Vec<(usize, String)> {
    let mut blocks = Vec::new();
    let mut lines = text.lines().enumerate();
    while let Some((index, line)) = lines.next() {
        let Some(info) = line.trim_start().strip_prefix("```") else {
            continue;
        };
        let wanted = info
            .split(|c: char| c == ',' || c.is_whitespace())
            .next()
            .is_some_and(|first| first == lang);
        let mut body = Vec::new();
        for (_, inner) in lines.by_ref() {
            if inner.trim_start().starts_with("```") {
                break;
            }
            body.push(inner.trim_end());
        }
        if wanted {
            blocks.push((index + 2, body.join("\n")));
        }
    }
    blocks
}

/// Whether a dependency on one of our crates may come from a path or git.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Local {
    /// The README's local-development block.
    Allowed,
    /// A skill: installed into someone else's project, so crates.io only.
    Refused,
}

/// Problems with the `ag-ui` / `ag-ui-a2ui` lines of a `toml` block, as a
/// 0-based line offset into the block and a message.
pub fn dependency_problems(block: &str, version: &str, local: Local) -> Vec<(usize, String)> {
    let wanted = if version.contains('-') {
        format!("={version}")
    } else {
        version.to_owned()
    };
    let mut problems = Vec::new();
    for (offset, line) in block.lines().enumerate() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        if !OUR_CRATES.contains(&key) {
            continue;
        }
        let value = value.trim();
        let (pinned, from_checkout) = if value.starts_with('{') {
            let from_checkout =
                table_value(value, "path").is_some() || table_value(value, "git").is_some();
            (table_value(value, "version"), from_checkout)
        } else {
            (quoted(value), false)
        };
        if from_checkout && local == Local::Refused {
            problems.push((
                offset,
                format!(
                    "takes `{key}` from a path or git checkout; it is on crates.io, so copy the README's registry line"
                ),
            ));
            continue;
        }
        match pinned {
            Some(pinned) if pinned == wanted || pinned.strip_prefix('=') == Some(version) => {}
            Some(pinned) => problems.push((
                offset,
                format!("pins `{key}` at \"{pinned}\"; the workspace is {version}, so write \"{wanted}\""),
            )),
            None if from_checkout => {}
            None => problems.push((
                offset,
                format!("names `{key}` without a version; write \"{wanted}\""),
            )),
        }
    }
    problems
}

/// Every backticked token that reads as a repository path — it has a `/`, no
/// whitespace, and is neither a URL, an absolute path such as an endpoint's
/// `/models`, nor a Rust path — with its 1-based line.
pub fn backticked_paths(text: &str) -> Vec<(usize, String)> {
    let mut paths = Vec::new();
    let mut line = 1;
    for (index, segment) in text.split('`').enumerate() {
        if index % 2 == 1
            && segment.contains('/')
            && !segment.starts_with('/')
            && !segment.contains(char::is_whitespace)
            && !segment.contains("::")
            && !segment.contains("://")
        {
            paths.push((line, segment.to_owned()));
        }
        line += segment.matches('\n').count();
    }
    paths
}

/// Every retired crate name in `text`, with its line and its replacement.
pub fn retired_names(text: &str) -> Vec<(usize, &'static str, &'static str)> {
    let mut found = Vec::new();
    for (index, line) in text.lines().enumerate() {
        for &(name, replacement) in RETIRED_CRATES {
            for (start, _) in line.match_indices(name) {
                let before = &line[..start];
                let after = &line[start + name.len()..];
                let joined = |c: char| c.is_ascii_alphanumeric() || c == '-' || c == '_';
                if before.chars().next_back().is_some_and(joined)
                    || after.chars().next().is_some_and(joined)
                {
                    continue;
                }
                // `ag-ui-a2ui`'s feature, in a feature path or a Cargo string.
                if name == "ag-ui-server"
                    && (before.ends_with("ag-ui-a2ui/")
                        || (before.ends_with('"') && after.starts_with('"')))
                {
                    continue;
                }
                found.push((index + 1, name, replacement));
            }
        }
    }
    found
}

/// `version` of `plugin.json`, as (field, value).
pub fn plugin_versions(json: &str) -> Result<Vec<(String, Option<String>)>, String> {
    let value: serde_json::Value = serde_json::from_str(json).map_err(|e| e.to_string())?;
    Ok(vec![("version".to_owned(), string_at(&value, "version"))])
}

/// `plugins[i].version` of `marketplace.json`, as (field, value).
pub fn marketplace_versions(json: &str) -> Result<Vec<(String, Option<String>)>, String> {
    let value: serde_json::Value = serde_json::from_str(json).map_err(|e| e.to_string())?;
    let plugins = value
        .get("plugins")
        .and_then(serde_json::Value::as_array)
        .ok_or("no `plugins` array")?;
    Ok(plugins
        .iter()
        .enumerate()
        .map(|(index, plugin)| {
            (
                format!("plugins[{index}].version"),
                string_at(plugin, "version"),
            )
        })
        .collect())
}

fn string_at(value: &serde_json::Value, key: &str) -> Option<String> {
    value.get(key)?.as_str().map(str::to_owned)
}

/// The contents of a leading `"…"`.
fn quoted(value: &str) -> Option<String> {
    let rest = value.strip_prefix('"')?;
    Some(rest[..rest.find('"')?].to_owned())
}

/// The string value of `key` in a TOML inline table.
fn table_value(table: &str, key: &str) -> Option<String> {
    table.match_indices(key).find_map(|(start, _)| {
        let before = table[..start].chars().next_back()?;
        if !(before == '{' || before == ',' || before.is_whitespace()) {
            return None;
        }
        let rest = table[start + key.len()..].trim_start().strip_prefix('=')?;
        quoted(rest.trim_start())
    })
}

/// Skill directories under `skills/` that hold a `SKILL.md`, sorted.
fn skill_names(root: &Path) -> Result<Vec<String>, String> {
    let mut names: Vec<String> = entries(&root.join(SKILLS_DIR))?
        .into_iter()
        .filter(|path| path.join("SKILL.md").is_file())
        .filter_map(|path| Some(path.file_name()?.to_str()?.to_owned()))
        .collect();
    names.sort();
    Ok(names)
}

/// A skill's `SKILL.md` and `references/*.md`, repository-relative.
fn skill_pages(root: &Path, skill: &str) -> Result<Vec<String>, String> {
    let mut pages = vec![format!("{SKILLS_DIR}/{skill}/SKILL.md")];
    let references = root.join(SKILLS_DIR).join(skill).join("references");
    if references.is_dir() {
        let mut more: Vec<String> = entries(&references)?
            .into_iter()
            .filter(|path| path.extension().is_some_and(|ext| ext == "md"))
            .filter_map(|path| relative(root, &path))
            .collect();
        more.sort();
        pages.extend(more);
    }
    Ok(pages)
}

/// Every `.rs`, `.md` and `.toml` file under `dir`, repository-relative and
/// sorted. Build output, `node_modules` and dot-directories are skipped.
fn source_files(root: &Path, dir: &str) -> Result<Vec<String>, String> {
    let mut files = Vec::new();
    let mut pending = vec![root.join(dir)];
    while let Some(dir) = pending.pop() {
        for path in entries(&dir)? {
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default();
            if path.is_dir() {
                if name != "target" && name != "node_modules" && !name.starts_with('.') {
                    pending.push(path);
                }
            } else if path
                .extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| matches!(ext, "rs" | "md" | "toml"))
            {
                files.extend(relative(root, &path));
            }
        }
    }
    files.sort();
    Ok(files)
}

fn entries(dir: &Path) -> Result<Vec<PathBuf>, String> {
    let read = fs::read_dir(dir).map_err(|e| format!("cannot list {}: {e}", dir.display()))?;
    read.map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<_, _>>()
        .map_err(|e| format!("cannot list {}: {e}", dir.display()))
}

fn relative(root: &Path, path: &Path) -> Option<String> {
    let relative = path.strip_prefix(root).ok()?;
    let parts: Vec<&str> = relative
        .components()
        .map(|part| part.as_os_str().to_str())
        .collect::<Option<_>>()?;
    Some(parts.join("/"))
}

fn read(root: &Path, file: &str) -> Result<String, String> {
    fs::read_to_string(root.join(file)).map_err(|e| format!("cannot read {file}: {e}"))
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

    #[test]
    fn the_workspace_version_comes_from_workspace_package_only() {
        let manifest = "\
[package]
version = \"9.9.9\"

[workspace.package]
edition = \"2024\"
version = \"0.5.0-alpha.3\"

[workspace.dependencies]
ag-ui = { path = \"crates/ag-ui\", version = \"0.5.0-alpha.3\" }
";
        assert_eq!(
            workspace_version(manifest).as_deref(),
            Some("0.5.0-alpha.3")
        );
        assert_eq!(workspace_version("[package]\nversion = \"1\"\n"), None);
    }

    #[test]
    fn a_version_line_may_wrap_and_prose_about_versions_is_not_one() {
        let text = "\
Docs: <https://example.invalid> · this skill is written against
workspace version **0.5.0-alpha.1**. If the API here disagrees with the compiler,
the skills each state the workspace version they were written against.
This skill targets Workspace version **0.4.1**.";
        assert_eq!(
            stated_versions(text),
            vec![(2, "0.5.0-alpha.1".to_owned()), (4, "0.4.1".to_owned()),]
        );
    }

    #[test]
    fn fenced_blocks_returns_one_language_and_where_its_body_starts() {
        let text = "\
intro
```rust
let toml = \"```toml\";
```
```toml
[dependencies]
ag-ui = \"=1.0.0\"
```
```toml,ignore
x = 1
```
";
        assert_eq!(
            fenced_blocks(text, "toml"),
            vec![
                (6, "[dependencies]\nag-ui = \"=1.0.0\"".to_owned()),
                (10, "x = 1".to_owned()),
            ]
        );
    }

    #[test]
    fn registry_lines_use_compatible_stable_versions_and_exact_prereleases() {
        let block = "\
[dependencies]
ag-ui = { version = \"=0.5.0-alpha.3\", features = [\"http\"] }
ag-ui-a2ui = \"=0.5.0-alpha.3\"
axum = \"0.8\"
tokio = { version = \"1\", features = [\"macros\"] }";
        assert!(dependency_problems(block, "0.5.0-alpha.3", Local::Refused).is_empty());

        let stable = r#"ag-ui = { version = "1.0.0", features = ["http"] }"#;
        assert!(dependency_problems(stable, "1.0.0", Local::Refused).is_empty());
        assert_eq!(
            dependency_problems(stable, "1.1.0", Local::Refused).len(),
            1
        );

        let stale = "\
ag-ui = { version = \"=0.5.0-alpha.1\", features = [\"http\"] }
ag-ui-a2ui = \"0.5.0-alpha.3\"
ag-ui = { features = [\"http\"] }";
        let problems = dependency_problems(stale, "0.5.0-alpha.3", Local::Refused);
        let offsets: Vec<usize> = problems.iter().map(|(offset, _)| *offset).collect();
        assert_eq!(offsets, [0, 1, 2]);
        assert!(
            problems[0].1.contains("\"=0.5.0-alpha.1\""),
            "{}",
            problems[0].1
        );
        assert!(
            problems[2].1.contains("without a version"),
            "{}",
            problems[2].1
        );
    }

    #[test]
    fn a_checkout_dependency_is_allowed_in_the_readme_and_refused_in_a_skill() {
        let block =
            "ag-ui = { path = \"/path/to/ag-ui-rust/crates/ag-ui\", features = [\"axum\"] }";
        assert!(dependency_problems(block, "0.5.0-alpha.3", Local::Allowed).is_empty());

        let problems = dependency_problems(block, "0.5.0-alpha.3", Local::Refused);
        assert_eq!(problems.len(), 1);
        assert!(problems[0].1.contains("crates.io"), "{}", problems[0].1);

        let git = "ag-ui = { git = \"https://example.invalid/ag-ui-rust\" }";
        assert_eq!(dependency_problems(git, "1.0.0", Local::Refused).len(), 1);
    }

    #[test]
    fn backticked_paths_are_repository_paths_only() {
        let text = "\
- `website/src/content/docs/start/index.md` — the dependency declarations
- `crates/ag-ui/src/client/transport/`, `crates/ag-ui/src/client/error.rs`
- `e2e/src/llm.rs` — `QWEN_BASE_URL_ENV`, `Endpoint::resolve`, `ag_ui::server::Agent`
- see `https://example.invalid/a/b`, the endpoint's `/models` and `two words/here`
";
        assert_eq!(
            backticked_paths(text),
            vec![
                (1, "website/src/content/docs/start/index.md".to_owned()),
                (2, "crates/ag-ui/src/client/transport/".to_owned()),
                (2, "crates/ag-ui/src/client/error.rs".to_owned()),
                (3, "e2e/src/llm.rs".to_owned()),
            ]
        );
    }

    #[test]
    fn retired_names_are_found_and_the_a2ui_feature_is_not_one() {
        let text = "\
//! an agent hosted by `ag-ui-server` behind `ag-ui-axum`, driven by `ag-ui-client`.
use ag_ui_core::Event;
ag-ui-a2ui = { workspace = true, features = [\"ag-ui-server\"] }
features are `ag-ui/{client,http}`, `ag-ui-a2ui/ag-ui-server`.
`ag_ui::client`, `ag-ui-a2ui`, `ag-ui-clients`, `my-ag-ui-core`";
        let found: Vec<(usize, &str)> = retired_names(text)
            .into_iter()
            .map(|(line, name, _)| (line, name))
            .collect();
        assert_eq!(
            found,
            vec![
                (1, "ag-ui-client"),
                (1, "ag-ui-server"),
                (1, "ag-ui-axum"),
                (2, "ag_ui_core"),
            ]
        );
    }

    #[test]
    fn manifest_versions_read_the_plugin_and_every_marketplace_entry() {
        let plugin = r#"{ "name": "ag-ui-rust", "version": "0.4.1" }"#;
        assert_eq!(
            plugin_versions(plugin).unwrap(),
            vec![("version".to_owned(), Some("0.4.1".to_owned()))]
        );

        let marketplace = r#"{ "plugins": [ { "version": "0.5.0" }, { "name": "x" } ] }"#;
        assert_eq!(
            marketplace_versions(marketplace).unwrap(),
            vec![
                ("plugins[0].version".to_owned(), Some("0.5.0".to_owned())),
                ("plugins[1].version".to_owned(), None),
            ]
        );
        assert!(marketplace_versions("{}").is_err());
    }

    /// A throwaway repository on disk, removed when dropped.
    struct Fixture(PathBuf);

    impl Fixture {
        fn new(name: &str) -> Self {
            let root = std::env::temp_dir()
                .join(format!("xtask-docs-check-{}-{name}", std::process::id()));
            let _ = fs::remove_dir_all(&root);
            Self(root)
        }

        fn write(&self, file: &str, contents: &str) {
            let path = self.0.join(file);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, contents).unwrap();
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    const BLOCK: &str = "[dependencies]\nag-ui = { version = \"=1.2.0\", features = [\"axum\"] }";

    /// A repository where every rule holds, at workspace version 1.2.0.
    fn consistent(name: &str) -> Fixture {
        let repo = Fixture::new(name);
        repo.write("Cargo.toml", "[workspace.package]\nversion = \"1.2.0\"\n");
        repo.write("README.md", &format!("# r\n\n```toml\n{BLOCK}\n```\n"));
        repo.write("crates/a/src/lib.rs", "");
        repo.write(
            "skills/s/SKILL.md",
            &format!("written against\nworkspace version **1.2.0**.\n\n```toml\n{BLOCK}\n```\n"),
        );
        repo.write(
            "skills/s/sources.md",
            "- `crates/a/src/lib.rs` — the crate\n",
        );
        repo.write("skills/ag-ui-rust-update/SKILL.md", "Refresh the skills.\n");
        repo.write(".claude-plugin/plugin.json", r#"{"version": "1.2.0"}"#);
        repo.write(
            ".claude-plugin/marketplace.json",
            r#"{"plugins": [{"version": "1.2.0"}]}"#,
        );
        repo.write("e2e/src/lib.rs", "//! driven by `ag_ui::client`.\n");
        repo.write("examples/x/Cargo.toml", "f = [\"ag-ui-server\"]\n");
        repo
    }

    #[test]
    fn a_consistent_repository_has_no_findings() {
        let repo = consistent("clean");
        let report = check(&repo.0).unwrap();
        assert_eq!(report.version, "1.2.0");
        assert_eq!(report.findings, []);
    }

    #[test]
    fn each_rule_reports_against_its_file() {
        let repo = consistent("stale");
        repo.write(
            "skills/s/SKILL.md",
            "workspace version **1.1.0**.\n\n```toml\n[dependencies]\nag-ui = { path = \"/x\" }\n```\n",
        );
        repo.write("skills/s/references/more.md", "no blocks here\n");
        repo.write("skills/t/SKILL.md", "No version line.\n");
        repo.write(
            "skills/s/sources.md",
            "- `website/src/content/docs/start/index.md`\n",
        );
        repo.write(".claude-plugin/plugin.json", r#"{"version": "0.4.1"}"#);
        repo.write(
            "examples/x/README.md",
            "Uses `ag-ui-client` and `ag-ui-core`.\n",
        );

        let report = check(&repo.0).unwrap();
        let shown: Vec<String> = report.findings.iter().map(ToString::to_string).collect();
        assert_eq!(
            shown,
            [
                "skills/s/SKILL.md:1: states workspace version 1.1.0; the workspace is 1.2.0",
                "skills/s/SKILL.md:4: this `toml` block is not one of README.md's; copy the README's block verbatim",
                "skills/s/SKILL.md:5: takes `ag-ui` from a path or git checkout; it is on crates.io, so copy the README's registry line",
                "skills/s/sources.md:1: `website/src/content/docs/start/index.md` does not exist",
                "skills/t/SKILL.md: states no `workspace version **…**` line; add one naming 1.2.0",
                ".claude-plugin/plugin.json: `version` is 0.4.1; the workspace is 1.2.0",
                "examples/x/README.md:1: `ag-ui-core` was merged into `ag-ui`; write the `ag_ui` crate root",
                "examples/x/README.md:1: `ag-ui-client` was merged into `ag-ui`; write `ag_ui::client`",
            ]
        );
    }

    #[test]
    fn an_unreadable_workspace_manifest_is_an_error_not_a_finding() {
        let repo = Fixture::new("broken");
        repo.write("README.md", "");
        assert!(check(&repo.0).is_err());
    }
}
