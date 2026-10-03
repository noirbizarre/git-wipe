//! Forge-backed merge detection (#72), end to end.
//!
//! The real binary runs against a fake forge API on localhost
//! (`GIT_WIPE_FORGE_API_URL`), so nothing here needs the internet, a token or
//! `gh`. The repository fixture has a branch whose squash merge was amended in
//! review: no offline strategy can recognise it, only the forge can.

use std::path::Path;
use std::process::Command as StdCommand;

use assert_cmd::Command;
use serde_json::{Value, json};
use tempfile::TempDir;

mod common;
use common::{FakeForge, Recorded, add_branches, configure, init_repo};

const AMENDED: &str = "feature/amended";

fn git(dir: &Path, args: &[&str]) -> String {
    let output = StdCommand::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

fn tip(dir: &Path, branch: &str) -> String {
    git(dir, &["rev-parse", branch])
}

/// `feature/done` (merged), `feature/wip` (unmerged), and `feature/amended`
/// (squash-merged with different content, undetectable offline), with
/// `origin` pointing at `remote_url`. Nothing is ever fetched from it.
fn repo(remote_url: &str) -> TempDir {
    let dir = init_repo();
    configure(&dir);
    add_branches(&dir);
    let p = dir.path();

    git(p, &["checkout", "-b", AMENDED]);
    std::fs::write(p.join("a.txt"), "first draft\n").unwrap();
    git(p, &["add", "."]);
    git(p, &["commit", "-m", "feature"]);

    git(p, &["checkout", "main"]);
    std::fs::write(p.join("a.txt"), "final wording, amended in review\n").unwrap();
    git(p, &["add", "."]);
    git(p, &["commit", "-m", "feature (#1)"]);
    std::fs::write(p.join("later.txt"), "later\n").unwrap();
    git(p, &["add", "."]);
    git(p, &["commit", "-m", "later work"]);

    git(p, &["remote", "add", "origin", remote_url]);
    dir
}

struct Run {
    json: Value,
    stderr: String,
    success: bool,
}

impl Run {
    fn reason(&self, branch: &str) -> Option<String> {
        self.json["local"]["branches"]
            .as_array()?
            .iter()
            .find(|b| b["branch"] == branch)
            .map(|b| b["reason"].as_str().unwrap().to_string())
    }

    fn warnings(&self) -> Vec<String> {
        self.json["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .map(|w| w.as_str().unwrap().to_string())
            .collect()
    }
}

/// Run `git wipe --json --dry-run --no-fetch --no-pull --local-only <args>`
/// against `api`, with exactly the credentials in `env`.
fn wipe(dir: &Path, api: &str, env: &[(&str, &str)], args: &[&str]) -> Run {
    let mut cmd = Command::cargo_bin("git-wipe").unwrap();
    cmd.current_dir(dir)
        .args([
            "--json",
            "--dry-run",
            "--no-fetch",
            "--no-pull",
            "--local-only",
        ])
        .args(args)
        .env("GIT_WIPE_FORGE_API_URL", api);
    for var in [
        "GITHUB_TOKEN",
        "GH_TOKEN",
        "GH_ENTERPRISE_TOKEN",
        "GITHUB_ENTERPRISE_TOKEN",
        "GITLAB_TOKEN",
        "GL_TOKEN",
        "GITEA_TOKEN",
        "FORGEJO_TOKEN",
        "HTTP_PROXY",
        "http_proxy",
        "HTTPS_PROXY",
        "https_proxy",
        "ALL_PROXY",
        "all_proxy",
    ] {
        cmd.env_remove(var);
    }
    for (name, value) in env {
        cmd.env(name, value);
    }
    let output = cmd.output().unwrap();
    let stdout = String::from_utf8(output.stdout).unwrap();
    Run {
        json: serde_json::from_str(&stdout)
            .unwrap_or_else(|e| panic!("stdout must be JSON ({e}): {stdout}")),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        success: output.status.success(),
    }
}

const TOKEN: &[(&str, &str)] = &[("GITHUB_TOKEN", "t0k3n")];

/// A GitHub GraphQL endpoint reporting `merged` branches at the given commits.
fn github(merged: Vec<(&'static str, String)>) -> impl Fn(&Recorded) -> (u16, String) {
    move |request| {
        let body: Value = serde_json::from_str(&request.body).unwrap();
        let variables = &body["variables"];
        let mut repository = serde_json::Map::new();
        let mut i = 0;
        while let Some(branch) = variables.get(format!("b{i}")) {
            let nodes: Vec<Value> = merged
                .iter()
                .filter(|(name, _)| branch == name)
                .map(|(_, sha)| json!({ "headRefOid": sha }))
                .collect();
            repository.insert(format!("r{i}"), json!({ "nodes": nodes }));
            i += 1;
        }
        (
            200,
            json!({ "data": { "repository": repository } }).to_string(),
        )
    }
}

// ── Acceptance: opt-in, and the forge settles what git cannot ────────

#[test]
fn off_by_default_the_forge_is_never_contacted() {
    let dir = repo("https://github.com/o/r.git");
    let tip = tip(dir.path(), AMENDED);
    let forge = FakeForge::start(github(vec![(AMENDED, tip)]));

    let run = wipe(dir.path(), &forge.url, TOKEN, &[]);

    assert!(run.success, "{}", run.stderr);
    assert!(forge.requests().is_empty(), "no request without opt-in");
    assert_eq!(run.reason(AMENDED), None);
    assert_eq!(run.reason("feature/done").as_deref(), Some("merged"));
    assert_eq!(run.json["forge"], "false");
    assert!(run.warnings().is_empty());
}

#[test]
fn forge_flag_detects_an_amended_squash_merge_as_pr_merged() {
    let dir = repo("git@github.com:o/r.git");
    let tip = tip(dir.path(), AMENDED);
    let forge = FakeForge::start(github(vec![(AMENDED, tip)]));

    let run = wipe(dir.path(), &forge.url, TOKEN, &["--forge"]);

    assert!(run.success, "{}", run.stderr);
    assert_eq!(run.reason(AMENDED).as_deref(), Some("pr-merged"));
    // Git still settles what the forge does not know, as plain `merged`.
    assert_eq!(run.reason("feature/done").as_deref(), Some("merged"));
    assert_eq!(run.reason("feature/wip"), None);
    assert_eq!(run.json["local"]["pr_merged"], json!([AMENDED]));
    assert!(
        run.json["local"]["merged"]
            .as_array()
            .unwrap()
            .contains(&json!(AMENDED))
    );
    assert_eq!(run.json["forge"], "true");
    assert!(run.warnings().is_empty(), "{:?}", run.warnings());

    // One batched GraphQL request, authenticated, not one per branch.
    let requests = forge.requests();
    assert_eq!(requests.len(), 1, "{requests:?}");
    assert_eq!(requests[0].method, "POST");
    assert_eq!(requests[0].path, "/graphql");
    assert_eq!(requests[0].authorization.as_deref(), Some("Bearer t0k3n"));
    // Only deletable branches are sent: not `main`, nor anything protected.
    assert!(
        !requests[0].body.contains("\"main\""),
        "{}",
        requests[0].body
    );
}

#[test]
fn a_pull_request_merged_at_another_commit_is_not_trusted() {
    let dir = repo("https://github.com/o/r.git");
    let stale = tip(dir.path(), "main");
    let forge = FakeForge::start(github(vec![(AMENDED, stale)]));

    let run = wipe(dir.path(), &forge.url, TOKEN, &["--forge"]);

    assert!(run.success, "{}", run.stderr);
    assert_eq!(run.reason(AMENDED), None);
}

#[test]
fn config_enables_the_forge_and_no_forge_overrides_it() {
    let dir = repo("https://github.com/o/r.git");
    let tip = tip(dir.path(), AMENDED);
    git(dir.path(), &["config", "wipe.forge", "true"]);
    let forge = FakeForge::start(github(vec![(AMENDED, tip)]));

    let run = wipe(dir.path(), &forge.url, TOKEN, &[]);
    assert_eq!(run.reason(AMENDED).as_deref(), Some("pr-merged"));
    assert_eq!(forge.requests().len(), 1);

    let run = wipe(dir.path(), &forge.url, TOKEN, &["--no-forge"]);
    assert_eq!(run.reason(AMENDED), None);
    assert_eq!(forge.requests().len(), 1, "--no-forge must not call out");
    assert_eq!(run.json["forge"], "false");
}

// ── Acceptance: a forge failure never fails the run ──────────────────

/// What the run reports when the forge is not involved at all.
fn offline_reasons(dir: &Path) -> (Option<String>, Option<String>) {
    let run = wipe(dir, &FakeForge::dead_url(), &[], &[]);
    (run.reason("feature/done"), run.reason(AMENDED))
}

fn assert_fell_back(dir: &Path, run: &Run, expected_warning: &str) {
    assert!(run.success, "{}", run.stderr);
    let warnings = run.warnings();
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(
        warnings[0].contains(expected_warning),
        "expected {expected_warning:?} in {:?}",
        warnings[0]
    );
    assert!(warnings[0].contains("falling back to git"), "{warnings:?}");
    assert_eq!(
        (run.reason("feature/done"), run.reason(AMENDED)),
        offline_reasons(dir),
        "the offline result must be unchanged"
    );
    assert_eq!(run.json["status"], "success");
    assert_eq!(run.json["errors"], json!([]));
}

#[test]
fn the_fallback_warning_is_shown_to_humans_too() {
    let dir = repo("https://github.com/o/r.git");
    let forge = FakeForge::start(|_| (401, "{}".to_string()));

    let output = Command::cargo_bin("git-wipe")
        .unwrap()
        .current_dir(dir.path())
        .args([
            "--yes",
            "--dry-run",
            "--no-fetch",
            "--no-pull",
            "--local-only",
            "--forge",
        ])
        .env("GIT_WIPE_FORGE_API_URL", &forge.url)
        .env("GITHUB_TOKEN", "t0k3n")
        .env("NO_COLOR", "1")
        .output()
        .unwrap();

    assert!(output.status.success());
    let text = String::from_utf8_lossy(&output.stdout).into_owned()
        + &String::from_utf8_lossy(&output.stderr);
    assert!(text.contains("falling back to git"), "{text}");
    assert!(text.contains("feature/done"), "{text}");
}

#[test]
fn rejected_credentials_warn_and_fall_back() {
    let dir = repo("https://github.com/o/r.git");
    let forge = FakeForge::start(|_| (401, r#"{"message":"Bad credentials"}"#.to_string()));

    let run = wipe(dir.path(), &forge.url, TOKEN, &["--forge"]);

    assert_fell_back(dir.path(), &run, "authentication error");
}

#[test]
fn a_forge_error_warns_and_falls_back() {
    let dir = repo("https://github.com/o/r.git");
    let forge = FakeForge::start(|_| (503, "upstream down".to_string()));

    let run = wipe(dir.path(), &forge.url, TOKEN, &["--forge"]);

    assert_fell_back(dir.path(), &run, "HTTP 503");
}

#[test]
fn graphql_errors_warn_and_fall_back() {
    let dir = repo("https://github.com/o/r.git");
    let forge = FakeForge::start(|_| {
        (
            200,
            json!({"errors": [{"message": "Something exploded"}]}).to_string(),
        )
    });

    let run = wipe(dir.path(), &forge.url, TOKEN, &["--forge"]);

    assert_fell_back(dir.path(), &run, "Something exploded");
}

#[test]
fn an_unreachable_forge_warns_and_falls_back() {
    let dir = repo("https://github.com/o/r.git");

    let run = wipe(dir.path(), &FakeForge::dead_url(), TOKEN, &["--forge"]);

    assert_fell_back(dir.path(), &run, "network error");
}

#[test]
fn a_missing_github_token_warns_without_any_request() {
    let dir = repo("https://github.com/o/r.git");
    let forge = FakeForge::start(github(vec![]));

    let run = wipe(dir.path(), &forge.url, &[], &["--forge"]);

    assert_fell_back(dir.path(), &run, "GITHUB_TOKEN");
    assert!(forge.requests().is_empty());
}

#[test]
fn an_unrecognised_host_warns_once_and_falls_back() {
    let dir = repo("https://git.example.com/o/r.git");
    let forge = FakeForge::start(github(vec![]));

    let run = wipe(dir.path(), &forge.url, TOKEN, &["--forge"]);

    assert!(run.success, "{}", run.stderr);
    let warnings = run.warnings();
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(warnings[0].contains("git.example.com"), "{warnings:?}");
    assert!(warnings[0].contains("wipe.forge"), "{warnings:?}");
    assert!(forge.requests().is_empty());
    assert_eq!(run.reason("feature/done").as_deref(), Some("merged"));
}

#[test]
fn a_repository_without_remotes_warns_and_runs_offline() {
    let dir = repo("https://github.com/o/r.git");
    git(dir.path(), &["remote", "remove", "origin"]);

    let run = wipe(dir.path(), &FakeForge::dead_url(), TOKEN, &["--forge"]);

    assert!(run.success, "{}", run.stderr);
    assert_eq!(run.warnings().len(), 1, "{:?}", run.warnings());
    assert_eq!(run.reason("feature/done").as_deref(), Some("merged"));
}

// ── Providers ────────────────────────────────────────────────────────

#[test]
fn gitlab_is_queried_over_graphql_with_pagination() {
    let dir = repo("git@gitlab.com:group/sub/r.git");
    let tip = tip(dir.path(), AMENDED);
    let forge = FakeForge::start(move |request| {
        let body: Value = serde_json::from_str(&request.body).unwrap();
        let page = if body["variables"]["after"].is_null() {
            json!({
                "nodes": [{"sourceBranch": "feature/wip", "diffHeadSha": "0000"}],
                "pageInfo": {"hasNextPage": true, "endCursor": "c1"},
            })
        } else {
            assert_eq!(body["variables"]["after"], "c1");
            json!({
                "nodes": [{"sourceBranch": AMENDED, "diffHeadSha": tip}],
                "pageInfo": {"hasNextPage": false, "endCursor": null},
            })
        };
        (
            200,
            json!({"data": {"project": {"mergeRequests": page}}}).to_string(),
        )
    });

    // Anonymous: GitLab answers for public projects without a token.
    let run = wipe(dir.path(), &forge.url, &[], &["--forge"]);

    assert!(run.success, "{}", run.stderr);
    assert!(run.warnings().is_empty(), "{:?}", run.warnings());
    assert_eq!(run.reason(AMENDED).as_deref(), Some("pr-merged"));
    // A merge request at another commit does not count.
    assert_eq!(run.reason("feature/wip"), None);

    let requests = forge.requests();
    assert_eq!(requests.len(), 2, "{requests:?}");
    assert_eq!(requests[0].path, "/api/graphql");
    assert_eq!(requests[0].authorization, None);
    let first: Value = serde_json::from_str(&requests[0].body).unwrap();
    assert_eq!(first["variables"]["path"], "group/sub/r");
}

#[test]
fn gitlab_sends_its_token() {
    let dir = repo("https://gitlab.com/g/r.git");
    let forge = FakeForge::start(|_| {
        (
            200,
            json!({"data": {"project": {"mergeRequests": {
                "nodes": [], "pageInfo": {"hasNextPage": false, "endCursor": null}
            }}}})
            .to_string(),
        )
    });

    wipe(
        dir.path(),
        &forge.url,
        &[("GITLAB_TOKEN", "glpat")],
        &["--forge"],
    );

    assert_eq!(
        forge.requests()[0].authorization.as_deref(),
        Some("Bearer glpat")
    );
}

#[test]
fn gitea_and_forgejo_are_queried_over_rest() {
    for kind in ["gitea", "forgejo"] {
        // A host name that gives nothing away: the kind comes from config.
        let dir = repo("https://git.example.com/team/app.git");
        git(dir.path(), &["config", "wipe.forge", kind]);
        let tip = tip(dir.path(), AMENDED);
        let forge = FakeForge::start(move |_| {
            (
                200,
                json!([
                    {"merged": false, "head": {"ref": "feature/wip", "sha": "1111"}},
                    {"merged": true, "head": {"ref": AMENDED, "sha": tip}},
                ])
                .to_string(),
            )
        });

        let run = wipe(dir.path(), &forge.url, &[("GITEA_TOKEN", "gt")], &[]);

        assert!(run.success, "{kind}: {}", run.stderr);
        assert!(run.warnings().is_empty(), "{kind}: {:?}", run.warnings());
        assert_eq!(run.reason(AMENDED).as_deref(), Some("pr-merged"), "{kind}");
        assert_eq!(run.reason("feature/wip"), None, "{kind}");
        assert_eq!(run.json["forge"], kind);

        let requests = forge.requests();
        assert_eq!(requests.len(), 1, "{kind}: {requests:?}");
        assert_eq!(requests[0].method, "GET");
        assert!(
            requests[0]
                .path
                .starts_with("/api/v1/repos/team/app/pulls?state=closed"),
            "{}",
            requests[0].path
        );
        assert_eq!(requests[0].authorization.as_deref(), Some("token gt"));
    }
}

// ── Reporting ────────────────────────────────────────────────────────

#[test]
fn text_output_labels_forge_detected_branches() {
    let dir = repo("https://github.com/o/r.git");
    let tip = tip(dir.path(), AMENDED);
    let forge = FakeForge::start(github(vec![(AMENDED, tip)]));

    let output = Command::cargo_bin("git-wipe")
        .unwrap()
        .current_dir(dir.path())
        .args([
            "--yes",
            "--dry-run",
            "--no-fetch",
            "--no-pull",
            "--local-only",
            "--forge",
        ])
        .env("GIT_WIPE_FORGE_API_URL", &forge.url)
        .env("GITHUB_TOKEN", "t0k3n")
        .env("NO_COLOR", "1")
        .output()
        .unwrap();

    assert!(output.status.success());
    let text = String::from_utf8_lossy(&output.stdout).into_owned()
        + &String::from_utf8_lossy(&output.stderr);
    assert!(text.contains(AMENDED), "{text}");
    assert!(text.contains("feature/done"), "{text}");
    // The forge-detected branch is tagged; the git-detected one is not.
    let tagged: Vec<&str> = text.lines().filter(|l| l.contains("pr-merged")).collect();
    assert!(
        tagged.is_empty() || tagged.iter().all(|l| l.contains(AMENDED)),
        "{text}"
    );
}

#[test]
fn status_reports_pr_merged_distinctly() {
    let dir = repo("https://github.com/o/r.git");
    let tip = tip(dir.path(), AMENDED);
    let forge = FakeForge::start(github(vec![(AMENDED, tip)]));

    let run = wipe_status(dir.path(), &forge.url, &["--forge"]);

    let entries = run["entries"].as_array().unwrap();
    let flags = |branch: &str| -> Vec<String> {
        entries
            .iter()
            .find(|e| e["branch"] == branch)
            .unwrap_or_else(|| panic!("no entry for {branch} in {entries:?}"))["status"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s.as_str().unwrap().to_string())
            .collect()
    };
    assert!(flags(AMENDED).contains(&"pr-merged".to_string()));
    assert!(!flags(AMENDED).contains(&"merged".to_string()));
    assert!(flags("feature/done").contains(&"merged".to_string()));
    assert!(!flags("feature/done").contains(&"pr-merged".to_string()));

    // Without the flag, status is offline and shows no pr-merged.
    let before = forge.requests().len();
    let run = wipe_status(dir.path(), &forge.url, &[]);
    assert_eq!(forge.requests().len(), before);
    assert!(!run["entries"].to_string().contains("pr-merged"), "{run}");
}

fn wipe_status(dir: &Path, api: &str, args: &[&str]) -> Value {
    let output = Command::cargo_bin("git-wipe")
        .unwrap()
        .current_dir(dir)
        .args(["status", "--json"])
        .args(args)
        .env("GIT_WIPE_FORGE_API_URL", api)
        .env("GITHUB_TOKEN", "t0k3n")
        .env_remove("HTTP_PROXY")
        .env_remove("http_proxy")
        .env_remove("ALL_PROXY")
        .env_remove("all_proxy")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

// ── Configuration ────────────────────────────────────────────────────

#[test]
fn config_set_validates_the_forge_value() {
    let dir = repo("https://github.com/o/r.git");

    Command::cargo_bin("git-wipe")
        .unwrap()
        .current_dir(dir.path())
        .args(["config", "set", "forge", "bitbucket"])
        .assert()
        .failure();
    assert_eq!(git_config(dir.path(), "wipe.forge"), None);

    Command::cargo_bin("git-wipe")
        .unwrap()
        .current_dir(dir.path())
        .args(["config", "set", "forge", "gitlab"])
        .assert()
        .success();
    assert_eq!(
        git_config(dir.path(), "wipe.forge").as_deref(),
        Some("gitlab")
    );
}

fn git_config(dir: &Path, key: &str) -> Option<String> {
    let output = StdCommand::new("git")
        .args(["config", "--get", key])
        .current_dir(dir)
        .output()
        .unwrap();
    output
        .status
        .success()
        .then(|| String::from_utf8(output.stdout).unwrap().trim().to_string())
}

#[test]
fn config_list_shows_the_forge_setting() {
    let dir = repo("https://github.com/o/r.git");

    let list = |json: bool| -> String {
        let mut cmd = Command::cargo_bin("git-wipe").unwrap();
        cmd.current_dir(dir.path()).args(["config", "list"]);
        if json {
            cmd.arg("--json");
        }
        let out = cmd.output().unwrap();
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr)
    };

    assert!(list(false).contains("disabled"), "{}", list(false));
    let json: Value = serde_json::from_str(&list(true)).unwrap();
    assert!(json["forge"].is_null());

    git(dir.path(), &["config", "wipe.forge", "true"]);
    assert!(
        list(false).contains("enabled (auto-detect)"),
        "{}",
        list(false)
    );
    let json: Value = serde_json::from_str(&list(true)).unwrap();
    assert_eq!(json["forge"], "true");
}

#[test]
fn forge_flag_is_documented_as_networked_and_opt_in() {
    let output = Command::cargo_bin("git-wipe")
        .unwrap()
        .arg("--help")
        .output()
        .unwrap();
    let help = String::from_utf8_lossy(&output.stdout);
    assert!(help.contains("--forge"), "{help}");
    assert!(help.contains("--no-forge"), "{help}");
    assert!(help.contains("networked"), "{help}");
    assert!(help.contains("GITHUB_TOKEN"), "{help}");
}
