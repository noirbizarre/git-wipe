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
    wipe_scope(dir, api, env, args, Some("--local-only"))
}

fn wipe_scope(
    dir: &Path,
    api: &str,
    env: &[(&str, &str)],
    args: &[&str],
    scope: Option<&str>,
) -> Run {
    let mut cmd = Command::cargo_bin("git-wipe").unwrap();
    cmd.current_dir(dir)
        .args(["--json", "--dry-run", "--no-fetch", "--no-pull"])
        .args(scope)
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
    github_requests(
        merged
            .into_iter()
            .map(|(branch, sha)| (branch, sha, "MERGED"))
            .collect(),
    )
}

/// Enforce the query filter so closed fixtures cannot mask a merged-only query.
fn github_requests(
    requests: Vec<(&'static str, String, &'static str)>,
) -> impl Fn(&Recorded) -> (u16, String) {
    move |request| {
        let body: Value = serde_json::from_str(&request.body).unwrap();
        let query = body["query"].as_str().unwrap();
        assert!(query.contains("headRefOid state"));
        let variables = &body["variables"];
        let mut repository = serde_json::Map::new();
        let mut i = 0;
        while let Some(branch) = variables.get(format!("b{i}")) {
            for (alias, filter) in [("m", "MERGED"), ("c", "CLOSED")] {
                assert!(query.contains(&format!(
                    "{alias}{i}: pullRequests(headRefName: $b{i}, states: {filter}, first: 20)"
                )));
                // Mirror the real connection limit so request history can expose truncation bugs.
                let nodes: Vec<Value> = requests
                    .iter()
                    .filter(|(name, _, state)| branch == name && *state == filter)
                    .take(20)
                    .map(|(_, sha, state)| json!({ "headRefOid": sha, "state": state }))
                    .collect();
                repository.insert(format!("{alias}{i}"), json!({ "nodes": nodes }));
            }
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
fn closed_requests_at_the_tip_qualify_locally_remotely_and_in_status() {
    let dir = repo("https://github.com/o/r.git");
    let p = dir.path();
    let head = tip(p, "feature/wip");
    git(p, &["update-ref", "refs/remotes/origin/feature/wip", &head]);
    git(
        p,
        &["update-ref", "refs/remotes/origin/main", &tip(p, "main")],
    );
    let forge = FakeForge::start(github_requests(vec![
        ("feature/wip", head.clone(), "CLOSED"),
        ("feature/wip", head, "OPEN"),
        (AMENDED, tip(p, "main"), "CLOSED"),
        (AMENDED, tip(p, AMENDED), "OPEN"),
    ]));
    let run = wipe_scope(p, &forge.url, TOKEN, &["--forge", "--yes"], None);
    assert!(run.success, "{:?}", run.json);
    assert_eq!(run.reason("feature/wip").as_deref(), Some("pr-closed"));
    assert_eq!(
        run.reason(AMENDED),
        None,
        "stale closed and open requests cannot qualify"
    );
    assert_eq!(run.json["local"]["pr_closed"], json!(["feature/wip"]));
    assert_eq!(run.json["local"]["pr_merged"], json!([]));
    assert_eq!(run.json["remotes"][0]["pr_closed"], json!(["feature/wip"]));
    assert!(
        run.json["remotes"][0]["branches"]
            .as_array()
            .unwrap()
            .iter()
            .any(|b| b["branch"] == "feature/wip" && b["reason"] == "pr-closed")
    );
    assert_eq!(
        forge.requests().len(),
        1,
        "local and remote scans share evidence"
    );

    let status = wipe_status(p, &forge.url, &["--forge", "--merged"]);
    let entry = status["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["branch"] == "feature/wip")
        .unwrap();
    assert_eq!(entry["status"], json!(["pr-closed"]));
    let text = Command::cargo_bin("git-wipe")
        .unwrap()
        .current_dir(p)
        .args(["status", "--forge", "--no-color"])
        .env("GIT_WIPE_FORGE_API_URL", &forge.url)
        .env("GITHUB_TOKEN", "t0k3n")
        .output()
        .unwrap();
    assert!(text.status.success());
    assert!(
        String::from_utf8_lossy(&text.stdout)
            .lines()
            .any(|line| line.contains("feature/wip") && line.contains("pr-closed")),
        "{}",
        String::from_utf8_lossy(&text.stdout)
    );
}

#[test]
fn cached_closed_evidence_is_checked_against_each_namespaces_tip() {
    let dir = repo("https://github.com/o/r.git");
    let p = dir.path();
    git(
        p,
        &[
            "update-ref",
            "refs/remotes/origin/feature/wip",
            &tip(p, "main"),
        ],
    );
    git(
        p,
        &["update-ref", "refs/remotes/origin/main", &tip(p, "main")],
    );
    let forge = FakeForge::start(github_requests(vec![(
        "feature/wip",
        tip(p, "feature/wip"),
        "CLOSED",
    )]));
    let run = wipe_scope(p, &forge.url, TOKEN, &["--forge"], None);
    assert!(run.success, "{:?}", run.json);
    assert_eq!(run.reason("feature/wip").as_deref(), Some("pr-closed"));
    assert_eq!(run.json["remotes"][0]["pr_closed"], json!([]));
    assert_eq!(forge.requests().len(), 1);
}

#[test]
fn a_matching_merge_wins_over_closed_evidence_regardless_of_repository_order() {
    let dir = repo("https://github.com/o/r.git");
    let p = dir.path();
    let merged = github_requests(vec![(AMENDED, tip(p, AMENDED), "MERGED")]);
    let closed = github_requests(vec![(AMENDED, tip(p, AMENDED), "CLOSED")]);
    let forge = FakeForge::start(move |request| {
        let body: Value = serde_json::from_str(&request.body).unwrap();
        if body["variables"]["name"] == "merged" {
            merged(request)
        } else {
            closed(request)
        }
    });
    for repositories in [["merged", "closed"], ["closed", "merged"]] {
        let urls = repositories.map(|name| format!("https://github.com/o/{name}"));
        let run = wipe(
            p,
            &forge.url,
            TOKEN,
            &[
                "--forge",
                "--forge-repo",
                &urls[0],
                "--forge-repo",
                &urls[1],
            ],
        );
        assert!(run.success, "{:?}", run.json);
        assert_eq!(run.reason(AMENDED).as_deref(), Some("pr-merged"));
        assert_eq!(run.json["local"]["pr_closed"], json!([]));
    }
}

#[test]
fn github_closed_requests_cannot_crowd_out_a_matching_merge() {
    let dir = repo("https://github.com/o/r.git");
    let p = dir.path();
    let head = tip(p, AMENDED);
    let mut requests = vec![(AMENDED, head.clone(), "CLOSED")];
    requests.extend((0..20).map(|i| (AMENDED, format!("stale-{i}"), "CLOSED")));
    requests.push((AMENDED, head, "MERGED"));
    let forge = FakeForge::start(github_requests(requests));
    let run = wipe(p, &forge.url, TOKEN, &["--forge"]);
    assert!(run.success, "{:?}", run.json);
    assert_eq!(run.reason(AMENDED).as_deref(), Some("pr-merged"));
    assert_eq!(run.json["local"]["pr_closed"], json!([]));
    assert_eq!(
        forge.requests().len(),
        1,
        "both states share one batched HTTP request"
    );
}

#[test]
fn yes_deletes_closed_unmerged_branches_their_clean_worktrees_and_remote_branches() {
    let dir = repo("https://github.com/o/r.git");
    let p = dir.path();
    let worktrees = TempDir::new().unwrap();
    let wt = worktrees.path().join("closed");
    git(p, &["worktree", "add", wt.to_str().unwrap(), "feature/wip"]);
    let remote = TempDir::new().unwrap();
    git(remote.path(), &["init", "--bare"]);
    // Keep the forge URL for detection, but direct mutations into an isolated bare repo.
    git(
        p,
        &[
            "remote",
            "set-url",
            "--push",
            "origin",
            remote.path().to_str().unwrap(),
        ],
    );
    git(p, &["push", "origin", "main", "feature/wip"]);
    let forge = FakeForge::start(github_requests(vec![(
        "feature/wip",
        tip(p, "feature/wip"),
        "CLOSED",
    )]));
    let output = Command::cargo_bin("git-wipe")
        .unwrap()
        .current_dir(p)
        .args([
            "--json",
            "--yes",
            "--no-fetch",
            "--no-pull",
            "--no-worktrunk",
            "--forge",
        ])
        .env("GIT_WIPE_FORGE_API_URL", &forge.url)
        .env("GITHUB_TOKEN", "t0k3n")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(
        !wt.exists(),
        "clean closed-request worktree must be removed"
    );
    assert!(!git(p, &["branch", "--list", "feature/wip"]).contains("feature/wip"));
    assert!(git(remote.path(), &["branch", "--list", "feature/wip"]).is_empty());
    assert!(
        report["local"]["branches"]
            .as_array()
            .unwrap()
            .iter()
            .any(|b| b["branch"] == "feature/wip"
                && b["reason"] == "pr-closed"
                && b["status"] == "deleted")
    );
    assert!(
        report["remotes"][0]["branches"]
            .as_array()
            .unwrap()
            .iter()
            .any(|b| b["branch"] == "feature/wip"
                && b["reason"] == "pr-closed"
                && b["status"] == "deleted")
    );
}

#[test]
fn closed_requests_keep_existing_worktree_safeguards_under_yes() {
    for (guard, args) in [
        ("dirty", vec![]),
        ("locked", vec![]),
        ("young", vec!["--min-age", "1h"]),
        ("small", vec!["--min-size", "1G"]),
        ("disabled", vec!["--no-worktrees"]),
    ] {
        let dir = repo("https://github.com/o/r.git");
        let p = dir.path();
        let worktrees = TempDir::new().unwrap();
        let wt = worktrees.path().join("closed");
        git(p, &["worktree", "add", wt.to_str().unwrap(), "feature/wip"]);
        match guard {
            "dirty" => std::fs::write(wt.join("uncommitted.txt"), "keep me\n").unwrap(),
            "locked" => {
                git(p, &["worktree", "lock", wt.to_str().unwrap()]);
            }
            _ => {}
        }
        let forge = FakeForge::start(github_requests(vec![(
            "feature/wip",
            tip(p, "feature/wip"),
            "CLOSED",
        )]));
        let output = Command::cargo_bin("git-wipe")
            .unwrap()
            .current_dir(p)
            .args([
                "--json",
                "--yes",
                "--local-only",
                "--no-fetch",
                "--no-pull",
                "--no-worktrunk",
                "--forge",
            ])
            .args(args)
            .env("GIT_WIPE_FORGE_API_URL", &forge.url)
            .env("GITHUB_TOKEN", "t0k3n")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{guard}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            wt.exists(),
            "{guard} worktree must survive closed-request cleanup"
        );
        assert!(
            git(p, &["branch", "--list", "feature/wip"]).contains("feature/wip"),
            "{guard} branch must survive"
        );
        let report: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(
            report["local"]["pr_closed"],
            json!(["feature/wip"]),
            "{guard}: the forge evidence must still qualify"
        );
    }
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
    assert!(
        warnings[0].contains("no supported repository"),
        "{warnings:?}"
    );
    assert!(warnings[0].contains("--forge-repo"), "{warnings:?}");
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
    let closed_tip = self::tip(dir.path(), "feature/wip");
    let forge = FakeForge::start(move |request| {
        let body: Value = serde_json::from_str(&request.body).unwrap();
        assert!(body["query"].as_str().unwrap().contains("state: $state"));
        let closed = body["variables"]["state"] == "closed";
        let page = if body["variables"]["after"].is_null() {
            json!({
                "nodes": [{"sourceBranch": "feature/wip", "diffHeadSha": "0000"}],
                "pageInfo": {"hasNextPage": true, "endCursor": "c1"},
            })
        } else {
            assert_eq!(body["variables"]["after"], "c1");
            json!({
                "nodes": [
                    {"sourceBranch": if closed { "feature/wip" } else { AMENDED }, "diffHeadSha": if closed { &closed_tip } else { &tip }},
                    {"sourceBranch": AMENDED, "diffHeadSha": &tip},
                ],
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
    // A stale merged request must not override a closed request at the current tip.
    assert_eq!(run.reason("feature/wip").as_deref(), Some("pr-closed"));

    let requests = forge.requests();
    assert_eq!(requests.len(), 4, "{requests:?}");
    assert_eq!(requests[0].path, "/api/graphql");
    assert_eq!(requests[0].authorization, None);
    let first: Value = serde_json::from_str(&requests[0].body).unwrap();
    assert_eq!(first["variables"]["path"], "group/sub/r");
    let states: Vec<Value> = requests
        .iter()
        .map(|request| serde_json::from_str::<Value>(&request.body).unwrap()["variables"].clone())
        .collect();
    assert_eq!(states[0]["state"], "merged");
    assert_eq!(states[2]["state"], "closed");
    assert!(
        states[2]["after"].is_null(),
        "closed pagination must restart"
    );
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
        let closed_tip = self::tip(dir.path(), "feature/wip");
        let forge = FakeForge::start(move |_| {
            (
                200,
                json!([
                    {"merged": false, "state": "closed", "head": {"ref": "feature/wip", "sha": closed_tip}},
                    {"merged": true, "state": "closed", "head": {"ref": AMENDED, "sha": tip}},
                ])
                .to_string(),
            )
        });

        let run = wipe(dir.path(), &forge.url, &[("GITEA_TOKEN", "gt")], &[]);

        assert!(run.success, "{kind}: {}", run.stderr);
        assert!(run.warnings().is_empty(), "{kind}: {:?}", run.warnings());
        assert_eq!(run.reason(AMENDED).as_deref(), Some("pr-merged"), "{kind}");
        assert_eq!(
            run.reason("feature/wip").as_deref(),
            Some("pr-closed"),
            "{kind}"
        );
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

#[test]
fn gitea_searches_older_pages_after_every_branch_name_has_been_seen() {
    let dir = repo("https://gitea.example.com/team/app.git");
    let head = tip(dir.path(), AMENDED);
    let forge = FakeForge::start(move |request| {
        assert!(request.path.contains("state=closed"));
        let page = if request.path.ends_with("page=1") {
            // Cover every requested name with stale heads; the exact tip is on page two.
            (0..50)
                .map(|i| {
                    json!({"state": "closed", "merged": false, "head": {
                        "ref": ([AMENDED, "feature/wip", "feature/done"][i % 3]), "sha": "stale"
                    }})
                })
                .collect::<Vec<_>>()
        } else if request.path.ends_with("page=2") {
            // A later page must still be read when closed evidence already matches the tip.
            vec![
                json!({"state": "closed", "merged": false, "head": {"ref": AMENDED, "sha": head}});
                50
            ]
        } else {
            assert!(request.path.ends_with("page=3"));
            vec![json!({"state": "closed", "merged": true, "head": {"ref": AMENDED, "sha": head}})]
        };
        (200, json!(page).to_string())
    });
    let run = wipe(dir.path(), &forge.url, &[], &["--forge"]);
    assert!(run.success, "{:?}", run.json);
    assert_eq!(run.reason(AMENDED).as_deref(), Some("pr-merged"));
    assert_eq!(forge.requests().len(), 3);
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

// ── Destination repositories, independent of fork cleanup remotes ─────

#[test]
fn protected_tracking_selects_the_parent_project_for_github_and_gitlab() {
    for host in ["github.com", "gitlab.com"] {
        let dir = repo(&format!("https://{host}/user/fork.git"));
        let p = dir.path();
        let project = if host == "github.com" {
            "team/project"
        } else {
            "team/sub/project"
        };
        git(
            p,
            &[
                "remote",
                "add",
                "parent",
                &format!("git@{host}:{project}.git"),
            ],
        );
        git(p, &["config", "branch.main.remote", "parent"]);
        git(p, &["config", "branch.main.merge", "refs/heads/trunk"]);
        // Only the fork is selected for cleanup; no parent tracking ref exists.
        git(p, &["config", "wipe.remote", "origin"]);
        let head = tip(p, AMENDED);
        let gh = github(vec![(AMENDED, head.clone())]);
        let forge = FakeForge::start(move |request| {
            let body: Value = serde_json::from_str(&request.body).unwrap();
            if host == "github.com" {
                assert_eq!(body["variables"]["owner"], "team");
                assert_eq!(body["variables"]["name"], "project");
                gh(request)
            } else {
                assert_eq!(body["variables"]["path"], "team/sub/project");
                (
                    200,
                    json!({"data": {"project": {"mergeRequests": {
                        "nodes": [{"sourceBranch": AMENDED, "diffHeadSha": head}],
                        "pageInfo": {"hasNextPage": false, "endCursor": null}
                    }}}})
                    .to_string(),
                )
            }
        });
        let run = wipe(p, &forge.url, TOKEN, &["--forge"]);
        assert!(run.success, "{host}: {:?}", run.json);
        assert_eq!(run.reason(AMENDED).as_deref(), Some("pr-merged"), "{host}");
        let status = wipe_status(p, &forge.url, &["--forge"]);
        assert!(status["entries"].as_array().unwrap().iter().any(|entry| {
            entry["branch"] == AMENDED
                && entry["status"]
                    .as_array()
                    .unwrap()
                    .contains(&json!("pr-merged"))
        }));
        assert_eq!(
            forge.requests().len(),
            if host == "github.com" { 2 } else { 4 },
            "{host}"
        );

        // Remote evidence must be checked against the fork's tip, not the local tip.
        git(
            p,
            &[
                "update-ref",
                &format!("refs/remotes/origin/{AMENDED}"),
                &tip(p, AMENDED),
            ],
        );
        git(
            p,
            &["update-ref", "refs/remotes/origin/main", &tip(p, "main")],
        );
        let run = wipe_scope(p, &forge.url, TOKEN, &["--forge"], Some("--remote-only"));
        assert!(run.success, "{host}: {:?}", run.json);
        assert_eq!(run.json["remotes"].as_array().unwrap().len(), 1);
        assert_eq!(run.json["remotes"][0]["remote"], "origin");
        assert_eq!(run.json["remotes"][0]["pr_merged"], json!([AMENDED]));
        git(
            p,
            &[
                "update-ref",
                &format!("refs/remotes/origin/{AMENDED}"),
                &tip(p, "feature/wip"),
            ],
        );
        let run = wipe_scope(p, &forge.url, TOKEN, &["--forge"], Some("--remote-only"));
        assert!(run.success);
        assert_eq!(run.json["remotes"][0]["pr_merged"], json!([]), "{host}");
    }
}

#[test]
fn repeated_cli_repositories_replace_config_and_union_answers_from_multiple_projects() {
    let dir = repo("https://github.com/user/fork.git");
    let p = dir.path();
    git(p, &["config", "wipe.forgeRepo", "origin"]);
    git(
        p,
        &["remote", "add", "parent", "git@github.com:team/first.git"],
    );
    let answer = github(vec![(AMENDED, tip(p, AMENDED))]);
    let first = github(vec![("feature/wip", tip(p, "feature/wip"))]);
    let forge = FakeForge::start(move |request| {
        let body: Value = serde_json::from_str(&request.body).unwrap();
        assert_eq!(body["variables"]["owner"], "team");
        match body["variables"]["name"].as_str().unwrap() {
            "first" => first(request),
            "second" => answer(request),
            other => panic!("unexpected project {other}"),
        }
    });
    let run = wipe(
        p,
        &forge.url,
        TOKEN,
        &[
            "--forge",
            "--forge-repo",
            "parent",
            "--forge-repo",
            "https://github.com/team/second.git",
            "--forge-repo",
            "ssh://git@github.com/team/first.git",
        ],
    );
    assert!(run.success, "{:?}", run.json);
    assert_eq!(run.reason(AMENDED).as_deref(), Some("pr-merged"));
    assert_eq!(run.reason("feature/wip").as_deref(), Some("pr-merged"));
    assert_eq!(
        forge.requests().len(),
        2,
        "alias must share the first repository's cache"
    );
}

#[test]
fn explicit_repository_urls_work_without_git_remotes_and_are_global_flags() {
    let dir = repo("https://github.com/user/fork.git");
    git(dir.path(), &["remote", "remove", "origin"]);
    let forge = FakeForge::start(github(vec![(AMENDED, tip(dir.path(), AMENDED))]));
    let output = Command::cargo_bin("git-wipe")
        .unwrap()
        .current_dir(dir.path())
        .args([
            "--forge-repo",
            "https://github.com/team/project",
            "--forge",
            "status",
            "--json",
        ])
        .env("GITHUB_TOKEN", "test")
        .env("GIT_WIPE_FORGE_API_URL", &forge.url)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let status: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(status["entries"].to_string().contains("pr-merged"));
    let status = wipe_status(
        dir.path(),
        &forge.url,
        &["--forge", "--forge-repo", "https://github.com/team/project"],
    );
    assert!(status["entries"].to_string().contains("pr-merged"));
}

#[test]
fn invalid_explicit_selection_is_actionable_and_sends_no_partial_queries() {
    let dir = repo("https://github.com/user/fork.git");
    let forge = FakeForge::start(github(vec![]));
    let run = wipe(
        dir.path(),
        &forge.url,
        TOKEN,
        &[
            "--forge",
            "--forge-repo",
            "origin",
            "--forge-repo",
            "missing",
        ],
    );
    assert!(!run.success);
    let error = run.json.to_string();
    assert!(error.contains("missing"), "{error}");
    assert!(error.contains("--forge-repo"), "{error}");
    assert!(forge.requests().is_empty());
    for args in [
        vec!["--forge-repo", "missing"],
        vec!["--forge", "--no-forge", "--forge-repo", "missing"],
    ] {
        let run = wipe(dir.path(), &forge.url, TOKEN, &args);
        assert!(run.success, "{:?}", run.json);
    }
    assert!(forge.requests().is_empty());
}

#[test]
fn configured_repository_lists_can_be_edited_replaced_and_used_for_detection() {
    let dir = repo("https://github.com/user/fork.git");
    let p = dir.path();
    let config = |args: &[&str]| {
        Command::cargo_bin("git-wipe")
            .unwrap()
            .current_dir(p)
            .arg("config")
            .args(args)
            .assert()
            .success();
    };
    config(&["add-forge-repo", "origin"]);
    config(&["add-forge-repo", "https://github.com/team/project.git"]);
    config(&["remove-forge-repo", "origin"]);
    let forge = FakeForge::start(github(vec![(AMENDED, tip(p, AMENDED))]));
    let run = wipe(p, &forge.url, TOKEN, &["--forge"]);
    assert_eq!(run.reason(AMENDED).as_deref(), Some("pr-merged"));
    let body: Value = serde_json::from_str(&forge.requests()[0].body).unwrap();
    assert_eq!(body["variables"]["owner"], "team");
    config(&["add-forge-repo", "origin"]);
    config(&["set", "forgerepo", "origin"]);
    assert_eq!(git(p, &["config", "--get-all", "wipe.forgeRepo"]), "origin");
    let output = Command::cargo_bin("git-wipe")
        .unwrap()
        .current_dir(p)
        .args(["config", "list", "--json"])
        .output()
        .unwrap();
    let listing: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(listing["forge_repos"], json!(["origin"]));
    config(&["set", "forgeRepo", "https://github.com/team/project"]);
    assert_eq!(
        git(p, &["config", "--get-all", "wipe.forgeRepo"]),
        "https://github.com/team/project"
    );
}

#[test]
fn a_shared_destination_cache_still_checks_each_candidate_namespaces_tip() {
    let dir = repo("https://github.com/user/fork.git");
    let p = dir.path();
    git(
        p,
        &[
            "remote",
            "add",
            "upstream",
            "https://github.com/team/project.git",
        ],
    );
    git(p, &["config", "wipe.remote", "origin"]);
    git(
        p,
        &[
            "update-ref",
            &format!("refs/remotes/origin/{AMENDED}"),
            &tip(p, "feature/wip"),
        ],
    );
    git(
        p,
        &["update-ref", "refs/remotes/origin/main", &tip(p, "main")],
    );
    let forge = FakeForge::start(github(vec![(AMENDED, tip(p, AMENDED))]));
    let run = wipe_scope(p, &forge.url, TOKEN, &["--forge"], None);
    assert!(run.success, "{:?}", run.json);
    assert_eq!(run.reason(AMENDED).as_deref(), Some("pr-merged"));
    assert_eq!(run.json["remotes"][0]["pr_merged"], json!([]));
    assert_eq!(
        forge.requests().len(),
        1,
        "local and remote scans share repository answers"
    );
}

#[test]
fn missing_destinations_warn_once_even_when_only_remote_branches_are_scanned() {
    let dir = repo("https://unknown.example/user/fork.git");
    let p = dir.path();
    git(
        p,
        &[
            "update-ref",
            &format!("refs/remotes/origin/{AMENDED}"),
            &tip(p, AMENDED),
        ],
    );
    git(
        p,
        &["update-ref", "refs/remotes/origin/main", &tip(p, "main")],
    );
    let forge = FakeForge::start(github(vec![]));
    for scope in [None, Some("--remote-only")] {
        let run = wipe_scope(p, &forge.url, TOKEN, &["--forge"], scope);
        assert!(run.success, "{:?}", run.json);
        assert_eq!(run.warnings().len(), 1, "{:?}", run.json);
        assert!(run.warnings()[0].contains("--forge-repo"));
    }
    assert!(forge.requests().is_empty());
}

#[test]
fn shared_destination_failures_warn_once_across_local_and_remote_scans() {
    let dir = repo("https://github.com/user/fork.git");
    let p = dir.path();
    git(
        p,
        &[
            "remote",
            "add",
            "upstream",
            "https://github.com/team/project.git",
        ],
    );
    git(p, &["config", "wipe.remote", "origin"]);
    git(
        p,
        &[
            "update-ref",
            &format!("refs/remotes/origin/{AMENDED}"),
            &tip(p, AMENDED),
        ],
    );
    git(
        p,
        &["update-ref", "refs/remotes/origin/main", &tip(p, "main")],
    );
    let forge = FakeForge::start(|_| (503, "unavailable".into()));
    let run = wipe_scope(p, &forge.url, TOKEN, &["--forge"], None);
    assert!(run.success, "{:?}", run.json);
    assert_eq!(run.reason(AMENDED), None);
    assert_eq!(run.warnings().len(), 1, "{:?}", run.json);
    assert!(run.warnings()[0].contains("team/project"));
    assert_eq!(forge.requests().len(), 1);
}
