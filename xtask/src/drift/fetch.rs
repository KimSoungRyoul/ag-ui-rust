//! Fetching the upstream source of truth.
//!
//! Only `--refresh` and `--upstream` come here. The offline check that CI gates
//! on never touches the network.

use std::time::{SystemTime, UNIX_EPOCH};

use crate::drift::baseline::{Source, UPSTREAM_PATH, UPSTREAM_REPO};

const USER_AGENT: &str = "ag-ui-rust-xtask-drift-check";

/// The upstream file, pinned to the commit it was read at.
pub struct Fetched {
    pub text: String,
    pub source: Source,
}

/// Resolves the newest commit touching `events.ts`, then reads the file at
/// exactly that commit, so the recorded SHA always describes the recorded text.
pub fn events_ts() -> Result<Fetched, String> {
    let commits = get(&format!(
        "https://api.github.com/repos/{UPSTREAM_REPO}/commits?path={UPSTREAM_PATH}&per_page=1"
    ))?;
    let commits: serde_json::Value = serde_json::from_str(&commits)
        .map_err(|e| format!("GitHub returned a commit list that is not JSON: {e}"))?;
    let head = commits
        .get(0)
        .ok_or_else(|| format!("no commits found for {UPSTREAM_PATH} in {UPSTREAM_REPO}"))?;
    let commit = head
        .get("sha")
        .and_then(|v| v.as_str())
        .ok_or("GitHub commit list has no `sha`")?
        .to_string();
    let commit_date = head
        .pointer("/commit/committer/date")
        .or_else(|| head.pointer("/commit/author/date"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .split('T')
        .next()
        .unwrap_or("")
        .to_string();

    let text = get(&format!(
        "https://raw.githubusercontent.com/{UPSTREAM_REPO}/{commit}/{UPSTREAM_PATH}"
    ))?;

    Ok(Fetched {
        text,
        source: Source {
            repo: UPSTREAM_REPO.to_string(),
            path: UPSTREAM_PATH.to_string(),
            commit,
            commit_date,
            fetched_at: today_utc(),
        },
    })
}

fn get(url: &str) -> Result<String, String> {
    let mut request = ureq::get(url)
        .header("User-Agent", USER_AGENT)
        .header("Accept", "application/vnd.github+json");
    // A token is not required, but an unauthenticated runner shares GitHub's
    // 60-requests-per-hour-per-IP budget with every other job on the machine.
    if let Some(token) = github_token() {
        request = request.header("Authorization", &format!("Bearer {token}"));
    }
    let mut response = request.call().map_err(|e| explain(url, &e))?;
    response
        .body_mut()
        .read_to_string()
        .map_err(|e| format!("cannot read the response from {url}: {e}"))
}

fn github_token() -> Option<String> {
    ["GITHUB_TOKEN", "GH_TOKEN"]
        .iter()
        .find_map(|key| std::env::var(key).ok())
        .filter(|token| !token.is_empty())
}

fn explain(url: &str, error: &ureq::Error) -> String {
    match error {
        ureq::Error::StatusCode(403 | 429) => format!(
            "{url} returned {error}.\n\
             That is usually GitHub's unauthenticated rate limit (60 requests/hour/IP).\n\
             Set GITHUB_TOKEN and retry, or run the offline check without --upstream/--refresh."
        ),
        ureq::Error::StatusCode(404) => format!(
            "{url} returned 404 — upstream may have moved the file.\n\
             Check {UPSTREAM_REPO} and update UPSTREAM_PATH in xtask/src/drift/baseline.rs."
        ),
        _ => format!("cannot fetch {url}: {error}"),
    }
}

/// Today's date in UTC as `YYYY-MM-DD`.
///
/// Civil-from-days, so the baseline carries a readable capture date without a
/// calendar dependency.
pub fn today_utc() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (y, m, d) = civil_from_days((secs / 86_400) as i64);
    format!("{y:04}-{m:02}-{d:02}")
}

/// Howard Hinnant's `civil_from_days`, with 1970-01-01 as day 0.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::civil_from_days;

    #[test]
    fn converts_known_days() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(19_723), (2024, 1, 1));
        assert_eq!(civil_from_days(19_784), (2024, 3, 2)); // leap year
        assert_eq!(civil_from_days(20_682), (2026, 8, 17));
    }
}
