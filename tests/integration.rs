//! Integration tests exercising the compiled binary end-to-end via `assert_cmd`.
//!
//! These cover the `main.rs` entry point, `handle_config_command`, and
//! `handle_clean` — code paths that are unreachable from unit tests.

use assert_cmd::Command;
use predicates::prelude::*;
use std::process::Command as StdCommand;
use tempfile::TempDir;

mod common;
use common::{
    add_branches, add_merged_worktree, configure, git_branches, init_repo,
    init_repo_with_worktree_config,
};

// ── CLI basics ───────────────────────────────────────────────────────

#[test]
fn help_flag_shows_usage() {
    Command::cargo_bin("git-wipe")
        .unwrap()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("Wipe out merged local branches"));
}

#[test]
fn version_flag_shows_version() {
    Command::cargo_bin("git-wipe")
        .unwrap()
        .arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::contains("git-wipe"));
}

// ── Config subcommands ───────────────────────────────────────────────

#[test]
fn config_list_no_config() {
    let dir = init_repo();
    Command::cargo_bin("git-wipe")
        .unwrap()
        .args(["config", "list"])
        .current_dir(dir.path())
        .assert()
        .success()
        .stderr(predicate::str::contains("No configuration found"));
}

#[test]
fn config_list_shows_values() {
    let dir = init_repo();
    configure(&dir);

    Command::cargo_bin("git-wipe")
        .unwrap()
        .args(["config", "list"])
        .current_dir(dir.path())
        .assert()
        .success()
        .stderr(predicate::str::contains("protected:"))
        .stderr(predicate::str::contains("main"))
        .stderr(predicate::str::contains("(all)"));
}

#[test]
fn config_list_with_configured_remotes() {
    let dir = init_repo();
    let p = dir.path();

    // Set up a config with specific remotes
    StdCommand::new("git")
        .args(["config", "--add", "wipe.protected", "main"])
        .current_dir(p)
        .output()
        .unwrap();
    StdCommand::new("git")
        .args(["config", "--add", "wipe.remote", "origin"])
        .current_dir(p)
        .output()
        .unwrap();
    StdCommand::new("git")
        .args(["config", "--add", "wipe.remote", "upstream"])
        .current_dir(p)
        .output()
        .unwrap();

    Command::cargo_bin("git-wipe")
        .unwrap()
        .args(["config", "list"])
        .current_dir(p)
        .assert()
        .success()
        .stderr(predicate::str::contains("origin, upstream"));
}

#[test]
fn config_set_value() {
    let dir = init_repo();

    Command::cargo_bin("git-wipe")
        .unwrap()
        .args(["config", "set", "remote", "upstream"])
        .current_dir(dir.path())
        .assert()
        .success()
        .stderr(predicate::str::contains("Set remote = upstream"));

    // Verify with git config
    let output = StdCommand::new("git")
        .args(["config", "--get", "wipe.remote"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "upstream");
}

#[test]
fn config_set_replaces_every_value_of_a_multi_valued_key() {
    let dir = init_repo();

    // Two values: a plain `git config wipe.protected <v>` would fail here.
    for value in ["main", "develop"] {
        StdCommand::new("git")
            .args(["config", "--add", "wipe.protected", value])
            .current_dir(dir.path())
            .output()
            .unwrap();
    }

    Command::cargo_bin("git-wipe")
        .unwrap()
        .args(["config", "set", "protected", "release"])
        .current_dir(dir.path())
        .assert()
        .success()
        .stderr(predicate::str::contains("Set protected = release"));

    let output = StdCommand::new("git")
        .args(["config", "--get-all", "wipe.protected"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "release");
}

#[test]
fn config_add_and_remove_protected() {
    let dir = init_repo();
    let p = dir.path();

    // Add a protected pattern
    Command::cargo_bin("git-wipe")
        .unwrap()
        .args(["config", "add-protected", "release/*"])
        .current_dir(p)
        .assert()
        .success()
        .stderr(predicate::str::contains(
            "Added protected pattern: release/*",
        ));

    // Add another
    Command::cargo_bin("git-wipe")
        .unwrap()
        .args(["config", "add-protected", "main"])
        .current_dir(p)
        .assert()
        .success();

    // Verify both exist
    let output = StdCommand::new("git")
        .args(["config", "--get-all", "wipe.protected"])
        .current_dir(p)
        .output()
        .unwrap();
    let values = String::from_utf8_lossy(&output.stdout);
    assert!(values.contains("release/*"));
    assert!(values.contains("main"));

    // Remove one
    Command::cargo_bin("git-wipe")
        .unwrap()
        .args(["config", "remove-protected", "release/*"])
        .current_dir(p)
        .assert()
        .success()
        .stderr(predicate::str::contains(
            "Removed protected pattern: release/*",
        ));

    // Verify only main remains
    let output = StdCommand::new("git")
        .args(["config", "--get-all", "wipe.protected"])
        .current_dir(p)
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "main");
}

#[test]
fn config_add_and_remove_remote() {
    let dir = init_repo();
    let p = dir.path();

    // Add remotes
    Command::cargo_bin("git-wipe")
        .unwrap()
        .args(["config", "add-remote", "origin"])
        .current_dir(p)
        .assert()
        .success()
        .stderr(predicate::str::contains("Added remote: origin"));

    Command::cargo_bin("git-wipe")
        .unwrap()
        .args(["config", "add-remote", "upstream"])
        .current_dir(p)
        .assert()
        .success();

    // Remove one
    Command::cargo_bin("git-wipe")
        .unwrap()
        .args(["config", "remove-remote", "upstream"])
        .current_dir(p)
        .assert()
        .success()
        .stderr(predicate::str::contains("Removed remote: upstream"));

    // Verify only origin remains
    let output = StdCommand::new("git")
        .args(["config", "--get-all", "wipe.remote"])
        .current_dir(p)
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "origin");
}

// ── Clean workflow ───────────────────────────────────────────────────

#[test]
fn clean_dry_run_preserves_branches() {
    let dir = init_repo();
    configure(&dir);
    add_branches(&dir);

    Command::cargo_bin("git-wipe")
        .unwrap()
        .args(["-y", "-n", "--no-fetch"])
        .current_dir(dir.path())
        .assert()
        .success()
        .stderr(predicate::str::contains("dry-run"))
        .stderr(predicate::str::contains("feature/done"));

    // Branches must still exist
    let branches = git_branches(&dir);
    assert!(branches.contains(&"feature/done".to_string()));
    assert!(branches.contains(&"feature/wip".to_string()));
}

#[test]
fn clean_deletes_merged_branch() {
    let dir = init_repo();
    configure(&dir);
    add_branches(&dir);

    Command::cargo_bin("git-wipe")
        .unwrap()
        .args(["-y", "--no-fetch"])
        .current_dir(dir.path())
        .assert()
        .success();

    let branches = git_branches(&dir);
    assert!(!branches.contains(&"feature/done".to_string()));
    assert!(branches.contains(&"feature/wip".to_string()));
    assert!(branches.contains(&"main".to_string()));
}

#[test]
fn clean_no_merged_branches() {
    let dir = init_repo();
    configure(&dir);
    // No extra branches — nothing to delete

    Command::cargo_bin("git-wipe")
        .unwrap()
        .args(["-y", "--no-fetch"])
        .current_dir(dir.path())
        .assert()
        .success()
        .stderr(predicate::str::contains("No merged local branches"));
}

#[test]
fn clean_remote_only_skips_local_deletion() {
    let dir = init_repo();
    configure(&dir);
    add_branches(&dir);

    Command::cargo_bin("git-wipe")
        .unwrap()
        .args(["-y", "--no-fetch", "--remote-only"])
        .current_dir(dir.path())
        .assert()
        .success();

    // Local merged branch must NOT be deleted
    let branches = git_branches(&dir);
    assert!(branches.contains(&"feature/done".to_string()));
}

#[test]
fn clean_local_only_deletes_local() {
    let dir = init_repo();
    configure(&dir);
    add_branches(&dir);

    Command::cargo_bin("git-wipe")
        .unwrap()
        .args(["-y", "--no-fetch", "--local-only"])
        .current_dir(dir.path())
        .assert()
        .success();

    let branches = git_branches(&dir);
    assert!(!branches.contains(&"feature/done".to_string()));
}

// ── Per-branch protection ───────────────────────────────────────────

#[test]
fn config_protect_and_unprotect() {
    let dir = init_repo();
    let p = dir.path();

    // Protect a branch
    Command::cargo_bin("git-wipe")
        .unwrap()
        .args(["config", "protect", "develop"])
        .current_dir(p)
        .assert()
        .success()
        .stderr(predicate::str::contains(
            "Branch 'develop' marked as protected",
        ));

    // Verify with git config
    let output = StdCommand::new("git")
        .args(["config", "--get", "branch.develop.wipe-protected"])
        .current_dir(p)
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "true");

    // Unprotect
    Command::cargo_bin("git-wipe")
        .unwrap()
        .args(["config", "unprotect", "develop"])
        .current_dir(p)
        .assert()
        .success()
        .stderr(predicate::str::contains(
            "Branch 'develop' is no longer protected",
        ));

    // Verify key is removed
    let output = StdCommand::new("git")
        .args(["config", "--get", "branch.develop.wipe-protected"])
        .current_dir(p)
        .output()
        .unwrap();
    assert!(
        String::from_utf8_lossy(&output.stdout).trim().is_empty(),
        "key should be unset after unprotect"
    );
}

#[test]
fn config_list_shows_branch_protected() {
    let dir = init_repo();
    let p = dir.path();
    configure(&dir);

    // Mark a branch as per-branch protected
    StdCommand::new("git")
        .args(["config", "branch.staging.wipe-protected", "true"])
        .current_dir(p)
        .output()
        .unwrap();

    Command::cargo_bin("git-wipe")
        .unwrap()
        .args(["config", "list"])
        .current_dir(p)
        .assert()
        .success()
        .stderr(predicate::str::contains("branch protected:"))
        .stderr(predicate::str::contains("staging"));
}

#[test]
fn clean_respects_branch_protected() {
    let dir = init_repo();
    configure(&dir);
    add_branches(&dir);

    // Mark the merged branch as per-branch protected
    let p = dir.path();
    StdCommand::new("git")
        .args(["config", "branch.feature/done.wipe-protected", "true"])
        .current_dir(p)
        .output()
        .unwrap();

    Command::cargo_bin("git-wipe")
        .unwrap()
        .args(["-y", "--no-fetch"])
        .current_dir(p)
        .assert()
        .success();

    // feature/done should NOT be deleted because it is per-branch protected
    let branches = git_branches(&dir);
    assert!(
        branches.contains(&"feature/done".to_string()),
        "per-branch protected branch should not be deleted"
    );
    // main should still exist
    assert!(branches.contains(&"main".to_string()));
}

// ── Worktree config support ─────────────────────────────────────────

#[test]
fn config_set_from_linked_worktree_visible_in_main() {
    let (_dir, main_path, wt_path) = init_repo_with_worktree_config();

    // Run config commands from the linked worktree
    Command::cargo_bin("git-wipe")
        .unwrap()
        .args(["config", "add-protected", "main"])
        .current_dir(&wt_path)
        .assert()
        .success();

    Command::cargo_bin("git-wipe")
        .unwrap()
        .args(["config", "add-protected", "release/*"])
        .current_dir(&wt_path)
        .assert()
        .success();

    // Config should be visible from the main worktree
    Command::cargo_bin("git-wipe")
        .unwrap()
        .args(["config", "list"])
        .current_dir(&main_path)
        .assert()
        .success()
        .stderr(predicate::str::contains("main"))
        .stderr(predicate::str::contains("release/*"));
}

#[test]
fn config_protect_from_linked_worktree_visible_in_main() {
    let (_dir, main_path, wt_path) = init_repo_with_worktree_config();

    // Seed minimal config so list doesn't show "no config"
    StdCommand::new("git")
        .args(["config", "--local", "--add", "wipe.protected", "main"])
        .current_dir(&main_path)
        .output()
        .unwrap();

    // Protect a branch from the linked worktree
    Command::cargo_bin("git-wipe")
        .unwrap()
        .args(["config", "protect", "develop"])
        .current_dir(&wt_path)
        .assert()
        .success();

    // Branch protection should be visible from the main worktree
    Command::cargo_bin("git-wipe")
        .unwrap()
        .args(["config", "list"])
        .current_dir(&main_path)
        .assert()
        .success()
        .stderr(predicate::str::contains("develop"));
}

#[test]
fn clean_from_linked_worktree_with_worktree_config() {
    let (_dir, main_path, wt_path) = init_repo_with_worktree_config();

    // Configure from main worktree
    StdCommand::new("git")
        .args(["config", "--local", "--add", "wipe.protected", "main"])
        .current_dir(&main_path)
        .output()
        .unwrap();

    // Create and merge a branch from the main worktree
    StdCommand::new("git")
        .args(["checkout", "-b", "feature/done"])
        .current_dir(&main_path)
        .output()
        .unwrap();
    std::fs::write(main_path.join("done.txt"), "done").unwrap();
    StdCommand::new("git")
        .args(["add", "."])
        .current_dir(&main_path)
        .output()
        .unwrap();
    StdCommand::new("git")
        .args(["commit", "-m", "done"])
        .current_dir(&main_path)
        .output()
        .unwrap();
    StdCommand::new("git")
        .args(["checkout", "main"])
        .current_dir(&main_path)
        .output()
        .unwrap();
    StdCommand::new("git")
        .args(["merge", "feature/done"])
        .current_dir(&main_path)
        .output()
        .unwrap();

    // Run clean from the linked worktree — must succeed and see config
    Command::cargo_bin("git-wipe")
        .unwrap()
        .args(["-y", "--no-fetch", "--no-worktrees"])
        .current_dir(&wt_path)
        .assert()
        .success();

    // The merged branch should have been deleted
    let output = StdCommand::new("git")
        .args(["branch", "--format=%(refname:short)"])
        .current_dir(&main_path)
        .output()
        .unwrap();
    let branches: Vec<String> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| l.to_string())
        .collect();
    assert!(
        !branches.contains(&"feature/done".to_string()),
        "merged branch should be deleted when running from linked worktree"
    );
    assert!(branches.contains(&"main".to_string()));
}

// ── Locked worktree handling ────────────────────────────────────────

#[test]
fn clean_skips_locked_worktree() {
    let dir = init_repo();
    let p = dir.path();
    configure(&dir);

    // Create and merge a branch
    StdCommand::new("git")
        .args(["checkout", "-b", "feature/locked"])
        .current_dir(p)
        .output()
        .unwrap();
    std::fs::write(p.join("locked.txt"), "locked feature").unwrap();
    StdCommand::new("git")
        .args(["add", "."])
        .current_dir(p)
        .output()
        .unwrap();
    StdCommand::new("git")
        .args(["commit", "-m", "locked feature"])
        .current_dir(p)
        .output()
        .unwrap();
    StdCommand::new("git")
        .args(["checkout", "main"])
        .current_dir(p)
        .output()
        .unwrap();
    StdCommand::new("git")
        .args(["merge", "feature/locked"])
        .current_dir(p)
        .output()
        .unwrap();

    // Create a worktree for the merged branch and lock it
    let wt_path = p.join("wt-locked");
    StdCommand::new("git")
        .args([
            "worktree",
            "add",
            wt_path.to_str().unwrap(),
            "feature/locked",
        ])
        .current_dir(p)
        .output()
        .unwrap();
    StdCommand::new("git")
        .args(["worktree", "lock", wt_path.to_str().unwrap()])
        .current_dir(p)
        .output()
        .unwrap();

    // Run clean — should skip the locked worktree
    Command::cargo_bin("git-wipe")
        .unwrap()
        .args(["-y", "--no-fetch"])
        .current_dir(p)
        .assert()
        .success()
        .stderr(predicate::str::contains("Skipping locked worktree"));

    // The locked worktree directory should still exist
    assert!(wt_path.exists(), "locked worktree should not be removed");

    // The branch cannot be deleted because it's still checked out in
    // the locked worktree — git refuses to delete it. This is expected:
    // the worktree removal was skipped, so branch deletion also fails
    // gracefully (logged as a warning).
    let branches = git_branches(&dir);
    assert!(
        branches.contains(&"feature/locked".to_string()),
        "branch should survive because its locked worktree prevents deletion"
    );
}

#[test]
fn clean_skips_locked_worktree_with_reason() {
    let dir = init_repo();
    let p = dir.path();
    configure(&dir);

    // Create and merge a branch
    StdCommand::new("git")
        .args(["checkout", "-b", "feature/locked-reason"])
        .current_dir(p)
        .output()
        .unwrap();
    std::fs::write(p.join("reason.txt"), "reason feature").unwrap();
    StdCommand::new("git")
        .args(["add", "."])
        .current_dir(p)
        .output()
        .unwrap();
    StdCommand::new("git")
        .args(["commit", "-m", "reason feature"])
        .current_dir(p)
        .output()
        .unwrap();
    StdCommand::new("git")
        .args(["checkout", "main"])
        .current_dir(p)
        .output()
        .unwrap();
    StdCommand::new("git")
        .args(["merge", "feature/locked-reason"])
        .current_dir(p)
        .output()
        .unwrap();

    // Create a worktree and lock it with a reason
    let wt_path = p.join("wt-locked-reason");
    StdCommand::new("git")
        .args([
            "worktree",
            "add",
            wt_path.to_str().unwrap(),
            "feature/locked-reason",
        ])
        .current_dir(p)
        .output()
        .unwrap();
    StdCommand::new("git")
        .args([
            "worktree",
            "lock",
            wt_path.to_str().unwrap(),
            "--reason",
            "work in progress",
        ])
        .current_dir(p)
        .output()
        .unwrap();

    // Run clean — should show the lock reason
    Command::cargo_bin("git-wipe")
        .unwrap()
        .args(["-y", "--no-fetch"])
        .current_dir(p)
        .assert()
        .success()
        .stderr(predicate::str::contains("Skipping locked worktree"))
        .stderr(predicate::str::contains("work in progress"));

    // Locked worktree must still exist
    assert!(wt_path.exists(), "locked worktree should not be removed");
}

#[test]
fn clean_recovers_from_a_stale_worktree_lock() {
    let dir = init_repo();
    let p = dir.path();
    configure(&dir);

    let wt_path = add_merged_worktree(&dir, "feature/stale-lock", "wt-stale-lock");
    StdCommand::new("git")
        .args([
            "worktree",
            "lock",
            "--reason",
            &format!("pid={}", common::DEAD_PID),
            wt_path.to_str().unwrap(),
        ])
        .current_dir(p)
        .output()
        .unwrap();

    Command::cargo_bin("git-wipe")
        .unwrap()
        .args(["-y", "--no-fetch"])
        .current_dir(p)
        .assert()
        .success()
        .stderr(predicate::str::contains("Unlocked stale lock"));

    assert!(!wt_path.exists(), "a stale lock must not prevent removal");
    assert!(
        !git_branches(&dir).contains(&"feature/stale-lock".to_string()),
        "the branch should be deletable once its stale lock is cleared"
    );
}

#[test]
fn clean_dry_run_reports_stale_lock_intent_without_unlocking() {
    let dir = init_repo();
    let p = dir.path();
    configure(&dir);

    let wt_path = add_merged_worktree(&dir, "feature/stale-lock-dry", "wt-stale-lock-dry");
    StdCommand::new("git")
        .args([
            "worktree",
            "lock",
            "--reason",
            &format!("pid={}", common::DEAD_PID),
            wt_path.to_str().unwrap(),
        ])
        .current_dir(p)
        .output()
        .unwrap();

    Command::cargo_bin("git-wipe")
        .unwrap()
        .args(["-y", "--no-fetch", "--dry-run"])
        .current_dir(p)
        .assert()
        .success()
        .stderr(predicate::str::contains("Would unlock stale lock"));

    assert!(wt_path.exists(), "--dry-run must not remove the worktree");

    let output = StdCommand::new("git")
        .args(["worktree", "list", "--porcelain"])
        .current_dir(p)
        .output()
        .unwrap();
    let listing = String::from_utf8_lossy(&output.stdout);
    assert!(
        listing.contains("locked"),
        "--dry-run must never actually unlock: {listing}"
    );
}

// ── Pull / fast-forward ─────────────────────────────────────────────

/// Create a local bare "remote", clone it, push an initial commit, and
/// configure git-wipe.  Returns (tempdir, work_path, bare_path).
fn init_repo_with_remote() -> (TempDir, std::path::PathBuf, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();

    // Bare remote
    let bare_path = dir.path().join("remote.git");
    StdCommand::new("git")
        .args([
            "init",
            "--bare",
            "--initial-branch=main",
            bare_path.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    // Clone → working repo
    let work_path = dir.path().join("work");
    StdCommand::new("git")
        .args([
            "clone",
            bare_path.to_str().unwrap(),
            work_path.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    StdCommand::new("git")
        .args(["config", "user.email", "test@test.com"])
        .current_dir(&work_path)
        .output()
        .unwrap();
    StdCommand::new("git")
        .args(["config", "user.name", "Test"])
        .current_dir(&work_path)
        .output()
        .unwrap();

    // Initial commit + push
    std::fs::write(work_path.join("README.md"), "# test").unwrap();
    StdCommand::new("git")
        .args(["add", "."])
        .current_dir(&work_path)
        .output()
        .unwrap();
    StdCommand::new("git")
        .args(["commit", "-m", "init"])
        .current_dir(&work_path)
        .output()
        .unwrap();
    StdCommand::new("git")
        .args(["push", "-u", "origin", "main"])
        .current_dir(&work_path)
        .output()
        .unwrap();

    // Configure git-wipe
    StdCommand::new("git")
        .args(["config", "--add", "wipe.protected", "main"])
        .current_dir(&work_path)
        .output()
        .unwrap();

    (dir, work_path, bare_path)
}

/// Push a new commit to the bare remote from a temporary second clone.
fn advance_remote_branch(bare_path: &std::path::Path, parent_dir: &std::path::Path) {
    let pusher = parent_dir.join("pusher");
    StdCommand::new("git")
        .args([
            "clone",
            bare_path.to_str().unwrap(),
            pusher.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    StdCommand::new("git")
        .args(["config", "user.email", "test@test.com"])
        .current_dir(&pusher)
        .output()
        .unwrap();
    StdCommand::new("git")
        .args(["config", "user.name", "Test"])
        .current_dir(&pusher)
        .output()
        .unwrap();
    std::fs::write(pusher.join("remote-new.txt"), "remote content").unwrap();
    StdCommand::new("git")
        .args(["add", "."])
        .current_dir(&pusher)
        .output()
        .unwrap();
    StdCommand::new("git")
        .args(["commit", "-m", "advance remote"])
        .current_dir(&pusher)
        .output()
        .unwrap();
    StdCommand::new("git")
        .args(["push"])
        .current_dir(&pusher)
        .output()
        .unwrap();
}

/// Return the SHA of a ref in a repo.
fn git_rev_parse(dir: &std::path::Path, refname: &str) -> String {
    let output = StdCommand::new("git")
        .args(["rev-parse", refname])
        .current_dir(dir)
        .output()
        .unwrap();
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

#[test]
fn no_pull_flag_accepted() {
    let dir = init_repo();
    configure(&dir);

    Command::cargo_bin("git-wipe")
        .unwrap()
        .args(["-y", "--no-fetch", "--no-pull"])
        .current_dir(dir.path())
        .assert()
        .success();
}

#[test]
fn effort_flag_accepted() {
    let dir = init_repo();
    configure(&dir);

    for level in ["1", "2", "3"] {
        Command::cargo_bin("git-wipe")
            .unwrap()
            .args(["-y", "--no-fetch", "--local-only", "--effort", level])
            .current_dir(dir.path())
            .assert()
            .success();
    }
}

#[test]
fn effort_flag_rejects_out_of_range_levels() {
    let dir = init_repo();
    configure(&dir);

    Command::cargo_bin("git-wipe")
        .unwrap()
        .args(["-y", "--no-fetch", "--effort", "4"])
        .current_dir(dir.path())
        .assert()
        .failure()
        .stderr(predicate::str::contains("4"));
}

#[test]
fn config_set_effort_roundtrips() {
    let dir = init_repo();

    Command::cargo_bin("git-wipe")
        .unwrap()
        .args(["config", "set", "effort", "3"])
        .current_dir(dir.path())
        .assert()
        .success();

    let output = StdCommand::new("git")
        .args(["config", "--get", "wipe.effort"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "3");

    Command::cargo_bin("git-wipe")
        .unwrap()
        .args(["config", "list"])
        .current_dir(dir.path())
        .assert()
        .success()
        .stderr(predicate::str::contains("3 (thorough)"));
}

#[test]
fn jobs_flag_rejects_zero() {
    let dir = init_repo();
    configure(&dir);

    Command::cargo_bin("git-wipe")
        .unwrap()
        .args(["-y", "--no-fetch", "--jobs", "0"])
        .current_dir(dir.path())
        .assert()
        .failure()
        .stderr(predicate::str::contains("0 is not in 1.."));
}

#[test]
fn config_set_jobs_roundtrips() {
    let dir = init_repo();

    Command::cargo_bin("git-wipe")
        .unwrap()
        .args(["config", "set", "jobs", "4"])
        .current_dir(dir.path())
        .assert()
        .success();

    let output = StdCommand::new("git")
        .args(["config", "--get", "wipe.jobs"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "4");

    Command::cargo_bin("git-wipe")
        .unwrap()
        .args(["config", "list"])
        .current_dir(dir.path())
        .assert()
        .success()
        .stderr(predicate::str::contains("jobs:"));
}

/// The acceptance criterion for #73: `--jobs` may only change the wall clock.
///
/// Both the JSON document (candidate ordering) and the human output (warning
/// ordering, prompts, summary) are compared verbatim; only the echoed job count
/// itself is allowed to differ.
#[test]
fn output_is_identical_whatever_the_job_count() {
    let dir = init_repo();
    configure(&dir);
    add_branches(&dir);

    // Enough branches that the work is actually spread over several workers.
    for n in 0..12 {
        let branch = format!("feature/extra-{n}");
        StdCommand::new("git")
            .args(["checkout", "-b", &branch, "main"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        std::fs::write(dir.path().join(format!("extra-{n}.txt")), "extra").unwrap();
        StdCommand::new("git")
            .args(["add", "."])
            .current_dir(dir.path())
            .output()
            .unwrap();
        StdCommand::new("git")
            .args(["commit", "-m", &format!("extra {n}")])
            .current_dir(dir.path())
            .output()
            .unwrap();
        StdCommand::new("git")
            .args(["checkout", "main"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        // Squash-merge half of them so the expensive strategies do real work.
        if n % 2 == 0 {
            StdCommand::new("git")
                .args(["merge", "--squash", &branch])
                .current_dir(dir.path())
                .output()
                .unwrap();
            StdCommand::new("git")
                .args(["commit", "-m", &format!("squash extra {n}")])
                .current_dir(dir.path())
                .output()
                .unwrap();
        }
    }

    let run = |jobs: &str, extra: &[&str]| {
        let mut args = vec![
            "-y",
            "--dry-run",
            "--no-fetch",
            "--local-only",
            "--effort",
            "3",
            "--jobs",
            jobs,
        ];
        args.extend_from_slice(extra);
        Command::cargo_bin("git-wipe")
            .unwrap()
            .args(&args)
            .current_dir(dir.path())
            .output()
            .unwrap()
    };

    // Human output: warnings, prompts and the summary, in order.
    assert_eq!(
        String::from_utf8_lossy(&run("1", &[]).stderr),
        String::from_utf8_lossy(&run("8", &[]).stderr),
        "--jobs must not change the human output"
    );

    // JSON document: candidate ordering above all.
    let mut serial: serde_json::Value =
        serde_json::from_slice(&run("1", &["--json"]).stdout).unwrap();
    let mut parallel: serde_json::Value =
        serde_json::from_slice(&run("8", &["--json"]).stdout).unwrap();

    // The effective job count is echoed in the document by design.
    serial.as_object_mut().unwrap().remove("jobs");
    parallel.as_object_mut().unwrap().remove("jobs");
    assert_eq!(serial, parallel);
}

/// The document echoes the job count actually used, so a run can be reproduced.
#[test]
fn json_reports_the_effective_job_count() {
    let dir = init_repo();
    configure(&dir);

    let doc = json_output(&dir, &["--json", "--dry-run", "--no-fetch", "--jobs", "3"]);
    assert_eq!(doc["jobs"], 3);
}

#[test]
fn min_age_flag_accepted() {
    let dir = init_repo();
    configure(&dir);

    for value in ["0", "30s", "15m", "2h", "7d", "1w"] {
        Command::cargo_bin("git-wipe")
            .unwrap()
            .args(["-y", "--no-fetch", "--local-only", "--min-age", value])
            .current_dir(dir.path())
            .assert()
            .success();
    }
}

#[test]
fn min_age_flag_rejects_garbage() {
    let dir = init_repo();
    configure(&dir);

    Command::cargo_bin("git-wipe")
        .unwrap()
        .args(["-y", "--no-fetch", "--min-age", "soon"])
        .current_dir(dir.path())
        .assert()
        .failure()
        .stderr(predicate::str::contains("invalid duration"));
}

#[test]
fn min_age_keeps_a_freshly_created_worktree() {
    let dir = init_repo();
    configure(&dir);
    let wt_path = add_merged_worktree(&dir, "feature/fresh", "wt-fresh");

    Command::cargo_bin("git-wipe")
        .unwrap()
        .args(["-y", "--no-fetch", "--local-only", "--min-age", "1h"])
        .current_dir(dir.path())
        .assert()
        .success();

    assert!(
        wt_path.exists(),
        "a worktree created seconds ago must survive --min-age 1h"
    );
}

#[test]
fn min_size_flag_accepted() {
    let dir = init_repo();
    configure(&dir);

    for value in ["0", "512B", "100K", "100M", "2G"] {
        Command::cargo_bin("git-wipe")
            .unwrap()
            .args(["-y", "--no-fetch", "--local-only", "--min-size", value])
            .current_dir(dir.path())
            .assert()
            .success();
    }
}

#[test]
fn min_size_flag_rejects_garbage() {
    let dir = init_repo();
    configure(&dir);

    Command::cargo_bin("git-wipe")
        .unwrap()
        .args(["-y", "--no-fetch", "--min-size", "soon"])
        .current_dir(dir.path())
        .assert()
        .failure()
        .stderr(predicate::str::contains("invalid size"));
}

#[test]
fn size_flag_accepted() {
    let dir = init_repo();
    configure(&dir);

    Command::cargo_bin("git-wipe")
        .unwrap()
        .args(["-y", "--no-fetch", "--local-only", "--size"])
        .current_dir(dir.path())
        .assert()
        .success();
}

#[test]
fn min_size_excludes_a_small_worktree() {
    let dir = init_repo();
    configure(&dir);
    let wt_path = add_merged_worktree(&dir, "feature/small", "wt-small");

    // The fixture worktree holds only a couple of small tracked files, so any
    // realistic --min-size excludes it.
    let doc = json_output(
        &dir,
        &["--json", "--no-fetch", "--local-only", "--min-size", "1G"],
    );

    let worktrees = doc["local"]["worktrees"].as_array().unwrap();
    let entry = worktrees
        .iter()
        .find(|w| w["branch"] == "feature/small")
        .expect("the small worktree should be reported");
    assert_eq!(entry["status"], "too_small");
    assert_eq!(doc["summary"]["worktrees_removed"], 0);
    assert!(wt_path.exists());
}

// ── Forced worktree removal ──────────────────────────────────────────

/// Make `path` dirty with an untracked file.
fn dirty_worktree(path: &std::path::Path) {
    std::fs::write(path.join("untracked.log"), "noise").unwrap();
}

#[test]
fn yes_alone_keeps_a_dirty_worktree() {
    let dir = init_repo();
    configure(&dir);
    let wt_path = add_merged_worktree(&dir, "feature/dirty", "wt-dirty");
    dirty_worktree(&wt_path);

    Command::cargo_bin("git-wipe")
        .unwrap()
        .args(["-y", "--no-fetch", "--local-only"])
        .current_dir(dir.path())
        .assert()
        .success()
        .stderr(predicate::str::contains("Skipping"));

    assert!(
        wt_path.exists(),
        "--yes without --force must not destroy uncommitted work"
    );
    assert!(git_branches(&dir).contains(&"feature/dirty".to_string()));
}

#[test]
fn yes_with_force_removes_a_dirty_worktree() {
    let dir = init_repo();
    configure(&dir);
    let wt_path = add_merged_worktree(&dir, "feature/dirty", "wt-dirty");
    dirty_worktree(&wt_path);

    Command::cargo_bin("git-wipe")
        .unwrap()
        .args(["-y", "--force", "--no-fetch", "--local-only"])
        .current_dir(dir.path())
        .assert()
        .success();

    assert!(
        !wt_path.exists(),
        "--yes --force should force-remove the dirty worktree"
    );
    assert!(!git_branches(&dir).contains(&"feature/dirty".to_string()));
}

#[test]
fn config_set_minage_roundtrips() {
    let dir = init_repo();

    Command::cargo_bin("git-wipe")
        .unwrap()
        .args(["config", "set", "minage", "2h"])
        .current_dir(dir.path())
        .assert()
        .success();

    let output = StdCommand::new("git")
        .args(["config", "--get", "wipe.minage"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "2h");

    Command::cargo_bin("git-wipe")
        .unwrap()
        .args(["config", "list"])
        .current_dir(dir.path())
        .assert()
        .success()
        .stderr(predicate::str::contains("min age"))
        .stderr(predicate::str::contains("2h"));
}

#[test]
fn config_set_minsize_roundtrips() {
    let dir = init_repo();

    Command::cargo_bin("git-wipe")
        .unwrap()
        .args(["config", "set", "minsize", "100M"])
        .current_dir(dir.path())
        .assert()
        .success();

    let output = StdCommand::new("git")
        .args(["config", "--get", "wipe.minsize"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "100M");

    Command::cargo_bin("git-wipe")
        .unwrap()
        .args(["config", "list"])
        .current_dir(dir.path())
        .assert()
        .success()
        .stderr(predicate::str::contains("min size"))
        .stderr(predicate::str::contains("100M"));
}

#[test]
fn pull_updates_current_branch() {
    let (dir, work_path, bare_path) = init_repo_with_remote();

    // Advance remote
    advance_remote_branch(&bare_path, dir.path());

    let before = git_rev_parse(&work_path, "HEAD");

    // Run git-wipe with pull enabled (default), fetch enabled
    Command::cargo_bin("git-wipe")
        .unwrap()
        .args(["-y"])
        .current_dir(&work_path)
        .assert()
        .success()
        .stderr(predicate::str::contains("Pulling"));

    let after = git_rev_parse(&work_path, "HEAD");
    assert_ne!(before, after, "main should have been fast-forwarded");
    assert!(
        work_path.join("remote-new.txt").exists(),
        "new file from remote should exist after pull"
    );
}

#[test]
fn no_pull_skips_fast_forward() {
    let (dir, work_path, bare_path) = init_repo_with_remote();

    // Advance remote
    advance_remote_branch(&bare_path, dir.path());

    let before = git_rev_parse(&work_path, "HEAD");

    // Run git-wipe with --no-pull
    Command::cargo_bin("git-wipe")
        .unwrap()
        .args(["-y", "--no-pull"])
        .current_dir(&work_path)
        .assert()
        .success();

    let after = git_rev_parse(&work_path, "HEAD");
    assert_eq!(
        before, after,
        "main should NOT have been updated with --no-pull"
    );
}

#[test]
fn pull_updates_branch_in_worktree() {
    let (dir, work_path, bare_path) = init_repo_with_remote();

    // Create a second protected branch, push it, check out in a worktree
    StdCommand::new("git")
        .args(["checkout", "-b", "develop"])
        .current_dir(&work_path)
        .output()
        .unwrap();
    StdCommand::new("git")
        .args(["push", "-u", "origin", "develop"])
        .current_dir(&work_path)
        .output()
        .unwrap();
    StdCommand::new("git")
        .args(["checkout", "main"])
        .current_dir(&work_path)
        .output()
        .unwrap();

    // Add develop as protected
    StdCommand::new("git")
        .args(["config", "--add", "wipe.protected", "develop"])
        .current_dir(&work_path)
        .output()
        .unwrap();

    // Create a worktree for develop
    let wt_path = dir.path().join("wt-develop");
    StdCommand::new("git")
        .args(["worktree", "add", wt_path.to_str().unwrap(), "develop"])
        .current_dir(&work_path)
        .output()
        .unwrap();

    // Advance develop on the remote
    let pusher = dir.path().join("pusher-dev");
    StdCommand::new("git")
        .args([
            "clone",
            "-b",
            "develop",
            bare_path.to_str().unwrap(),
            pusher.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    StdCommand::new("git")
        .args(["config", "user.email", "test@test.com"])
        .current_dir(&pusher)
        .output()
        .unwrap();
    StdCommand::new("git")
        .args(["config", "user.name", "Test"])
        .current_dir(&pusher)
        .output()
        .unwrap();
    std::fs::write(pusher.join("dev-new.txt"), "dev content").unwrap();
    StdCommand::new("git")
        .args(["add", "."])
        .current_dir(&pusher)
        .output()
        .unwrap();
    StdCommand::new("git")
        .args(["commit", "-m", "advance develop"])
        .current_dir(&pusher)
        .output()
        .unwrap();
    StdCommand::new("git")
        .args(["push"])
        .current_dir(&pusher)
        .output()
        .unwrap();

    let before = git_rev_parse(&work_path, "develop");

    // Run git-wipe — should pull develop via the worktree
    Command::cargo_bin("git-wipe")
        .unwrap()
        .args(["-y"])
        .current_dir(&work_path)
        .assert()
        .success()
        .stderr(predicate::str::contains("Pulling"));

    let after = git_rev_parse(&work_path, "develop");
    assert_ne!(
        before, after,
        "develop should have been fast-forwarded via worktree"
    );
    assert!(
        wt_path.join("dev-new.txt").exists(),
        "new file should be in the worktree after pull"
    );
}

#[test]
fn pull_updates_non_checked_out_branch_via_fetch() {
    let (dir, work_path, bare_path) = init_repo_with_remote();

    // Create a second protected branch, push it, but do NOT check it out in a worktree
    StdCommand::new("git")
        .args(["checkout", "-b", "develop"])
        .current_dir(&work_path)
        .output()
        .unwrap();
    StdCommand::new("git")
        .args(["push", "-u", "origin", "develop"])
        .current_dir(&work_path)
        .output()
        .unwrap();
    StdCommand::new("git")
        .args(["checkout", "main"])
        .current_dir(&work_path)
        .output()
        .unwrap();

    // Add develop as protected
    StdCommand::new("git")
        .args(["config", "--add", "wipe.protected", "develop"])
        .current_dir(&work_path)
        .output()
        .unwrap();

    // Advance develop on the remote
    let pusher = dir.path().join("pusher-dev2");
    StdCommand::new("git")
        .args([
            "clone",
            "-b",
            "develop",
            bare_path.to_str().unwrap(),
            pusher.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    StdCommand::new("git")
        .args(["config", "user.email", "test@test.com"])
        .current_dir(&pusher)
        .output()
        .unwrap();
    StdCommand::new("git")
        .args(["config", "user.name", "Test"])
        .current_dir(&pusher)
        .output()
        .unwrap();
    std::fs::write(pusher.join("dev-new2.txt"), "dev content 2").unwrap();
    StdCommand::new("git")
        .args(["add", "."])
        .current_dir(&pusher)
        .output()
        .unwrap();
    StdCommand::new("git")
        .args(["commit", "-m", "advance develop 2"])
        .current_dir(&pusher)
        .output()
        .unwrap();
    StdCommand::new("git")
        .args(["push"])
        .current_dir(&pusher)
        .output()
        .unwrap();

    let before = git_rev_parse(&work_path, "develop");

    // Run git-wipe — should update develop via fetch refspec
    Command::cargo_bin("git-wipe")
        .unwrap()
        .args(["-y"])
        .current_dir(&work_path)
        .assert()
        .success();

    let after = git_rev_parse(&work_path, "develop");
    assert_ne!(
        before, after,
        "develop should have been fast-forwarded via fetch"
    );
}

#[test]
fn pull_dry_run_does_not_update() {
    let (dir, work_path, bare_path) = init_repo_with_remote();

    // Advance remote
    advance_remote_branch(&bare_path, dir.path());

    let before = git_rev_parse(&work_path, "HEAD");

    // Run git-wipe with --dry-run
    Command::cargo_bin("git-wipe")
        .unwrap()
        .args(["-y", "-n"])
        .current_dir(&work_path)
        .assert()
        .success()
        .stderr(predicate::str::contains("dry-run"));

    let after = git_rev_parse(&work_path, "HEAD");
    assert_eq!(before, after, "HEAD should NOT change in dry-run mode");
}

// ── Not a git repository ────────────────────────────────────────────

#[test]
fn exits_with_error_when_not_in_a_git_repo() {
    let dir = tempfile::tempdir().unwrap();

    Command::cargo_bin("git-wipe")
        .unwrap()
        .current_dir(dir.path())
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("Not a git repository"));
}

#[test]
fn no_stack_trace_when_not_in_a_git_repo() {
    let dir = tempfile::tempdir().unwrap();

    Command::cargo_bin("git-wipe")
        .unwrap()
        .current_dir(dir.path())
        .assert()
        .failure()
        .stderr(predicate::str::contains("Error:").not())
        .stderr(predicate::str::contains("stack backtrace").not());
}

// ── Deleted upstream detection ───────────────────────────────────────

/// Create a repo with a `feature/gone` branch whose remote branch was deleted,
/// plus a `feature/alive` branch still present on the remote. Neither branch is
/// merged into `main`, so only the deleted-upstream signal can find them.
fn init_repo_with_gone_upstream() -> (TempDir, std::path::PathBuf) {
    let (dir, work_path, bare_path) = init_repo_with_remote();

    for branch in ["feature/gone", "feature/alive"] {
        StdCommand::new("git")
            .args(["checkout", "-b", branch, "main"])
            .current_dir(&work_path)
            .output()
            .unwrap();
        std::fs::write(
            work_path.join(format!("{}.txt", branch.replace('/', "-"))),
            branch,
        )
        .unwrap();
        StdCommand::new("git")
            .args(["add", "."])
            .current_dir(&work_path)
            .output()
            .unwrap();
        StdCommand::new("git")
            .args(["commit", "-m", branch])
            .current_dir(&work_path)
            .output()
            .unwrap();
        StdCommand::new("git")
            .args(["push", "-u", "origin", branch])
            .current_dir(&work_path)
            .output()
            .unwrap();
    }
    StdCommand::new("git")
        .args(["checkout", "main"])
        .current_dir(&work_path)
        .output()
        .unwrap();

    // Delete the remote branch, as a merge-and-delete PR workflow would.
    StdCommand::new("git")
        .args(["branch", "-D", "feature/gone"])
        .current_dir(&bare_path)
        .output()
        .unwrap();

    (dir, work_path)
}

/// Local branch names in an arbitrary directory.
fn branches_in(path: &std::path::Path) -> Vec<String> {
    let output = StdCommand::new("git")
        .args(["branch", "--format=%(refname:short)"])
        .current_dir(path)
        .output()
        .unwrap();
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| l.to_string())
        .collect()
}

#[test]
fn gone_upstream_branch_is_reported_but_kept_by_default() {
    let (_dir, work_path) = init_repo_with_gone_upstream();

    Command::cargo_bin("git-wipe")
        .unwrap()
        .args(["-y", "--no-pull", "--local-only"])
        .current_dir(&work_path)
        .assert()
        .success()
        .stderr(predicate::str::contains("with a deleted upstream"));

    let branches = branches_in(&work_path);
    assert!(
        branches.contains(&"feature/gone".to_string()),
        "must not be deleted without --delete-gone: {branches:?}"
    );
}

#[test]
fn delete_gone_removes_branch_with_deleted_upstream() {
    let (_dir, work_path) = init_repo_with_gone_upstream();

    Command::cargo_bin("git-wipe")
        .unwrap()
        .args(["-y", "--no-pull", "--local-only", "--delete-gone"])
        .current_dir(&work_path)
        .assert()
        .success();

    let branches = branches_in(&work_path);
    assert!(!branches.contains(&"feature/gone".to_string()));
    assert!(
        branches.contains(&"feature/alive".to_string()),
        "branches with a live upstream must survive: {branches:?}"
    );
}

#[test]
fn gone_upstream_detection_requires_a_fetch() {
    let (_dir, work_path) = init_repo_with_gone_upstream();

    Command::cargo_bin("git-wipe")
        .unwrap()
        .args([
            "-y",
            "--no-fetch",
            "--no-pull",
            "--local-only",
            "--delete-gone",
        ])
        .current_dir(&work_path)
        .assert()
        .success()
        .stderr(predicate::str::contains("with a deleted upstream").not());

    assert!(branches_in(&work_path).contains(&"feature/gone".to_string()));
}

#[test]
fn gone_upstream_detection_runs_in_dry_run_without_fetch() {
    let (_dir, work_path) = init_repo_with_gone_upstream();

    // Refresh remote-tracking refs out of band, as a user would before a preview.
    StdCommand::new("git")
        .args(["fetch", "--prune", "origin"])
        .current_dir(&work_path)
        .output()
        .unwrap();

    Command::cargo_bin("git-wipe")
        .unwrap()
        .args([
            "-y",
            "--dry-run",
            "--no-fetch",
            "--no-pull",
            "--local-only",
            "--delete-gone",
        ])
        .current_dir(&work_path)
        .assert()
        .success()
        .stderr(predicate::str::contains("with a deleted upstream"))
        .stderr(predicate::str::contains(
            "(dry-run) Would delete local branch 'feature/gone'",
        ));

    assert!(branches_in(&work_path).contains(&"feature/gone".to_string()));
}

// ── Ignored branches ─────────────────────────────────────────────────

#[test]
fn ignored_branch_is_not_offered_for_deletion() {
    let dir = init_repo();
    configure(&dir);
    add_branches(&dir);

    StdCommand::new("git")
        .args(["config", "--add", "wipe.ignore", "feature/*"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    Command::cargo_bin("git-wipe")
        .unwrap()
        .args(["--dry-run", "--no-fetch"])
        .current_dir(dir.path())
        .assert()
        .success()
        .stderr(predicate::str::contains("feature/done").not());

    Command::cargo_bin("git-wipe")
        .unwrap()
        .args(["-y", "--no-fetch"])
        .current_dir(dir.path())
        .assert()
        .success();

    let branches = git_branches(&dir);
    assert!(
        branches.contains(&"feature/done".to_string()),
        "an ignored branch must survive an auto-confirmed clean, got {branches:?}"
    );
}

#[test]
fn per_branch_ignore_flag_survives_clean() {
    let dir = init_repo();
    configure(&dir);
    add_branches(&dir);

    Command::cargo_bin("git-wipe")
        .unwrap()
        .args(["config", "ignore", "feature/done"])
        .current_dir(dir.path())
        .assert()
        .success();

    Command::cargo_bin("git-wipe")
        .unwrap()
        .args(["-y", "--no-fetch"])
        .current_dir(dir.path())
        .assert()
        .success();

    assert!(git_branches(&dir).contains(&"feature/done".to_string()));
}

#[test]
fn config_add_ignore_and_list() {
    let dir = init_repo();
    configure(&dir);

    Command::cargo_bin("git-wipe")
        .unwrap()
        .args(["config", "add-ignore", "wip/*"])
        .current_dir(dir.path())
        .assert()
        .success()
        .stderr(predicate::str::contains("Added ignore pattern: wip/*"));

    Command::cargo_bin("git-wipe")
        .unwrap()
        .args(["config", "list"])
        .current_dir(dir.path())
        .assert()
        .success()
        .stderr(predicate::str::contains("ignore:"))
        .stderr(predicate::str::contains("wip/*"));

    Command::cargo_bin("git-wipe")
        .unwrap()
        .args(["config", "remove-ignore", "wip/*"])
        .current_dir(dir.path())
        .assert()
        .success()
        .stderr(predicate::str::contains("Removed ignore pattern: wip/*"));

    Command::cargo_bin("git-wipe")
        .unwrap()
        .args(["config", "list"])
        .current_dir(dir.path())
        .assert()
        .success()
        .stderr(predicate::str::contains("wip/*").not());
}

#[test]
fn config_ignore_and_unignore_individual_branch() {
    let dir = init_repo();
    configure(&dir);
    add_branches(&dir);

    Command::cargo_bin("git-wipe")
        .unwrap()
        .args(["config", "ignore", "feature/done"])
        .current_dir(dir.path())
        .assert()
        .success()
        .stderr(predicate::str::contains("is now ignored"));

    Command::cargo_bin("git-wipe")
        .unwrap()
        .args(["config", "list"])
        .current_dir(dir.path())
        .assert()
        .success()
        .stderr(predicate::str::contains("branch ignored: feature/done"));

    Command::cargo_bin("git-wipe")
        .unwrap()
        .args(["config", "unignore", "feature/done"])
        .current_dir(dir.path())
        .assert()
        .success()
        .stderr(predicate::str::contains("no longer ignored"));

    Command::cargo_bin("git-wipe")
        .unwrap()
        .args(["config", "list"])
        .current_dir(dir.path())
        .assert()
        .success()
        .stderr(predicate::str::contains("branch ignored: (none)"));

    // Once unignored, the merged branch is a deletion candidate again.
    Command::cargo_bin("git-wipe")
        .unwrap()
        .args(["-y", "--no-fetch"])
        .current_dir(dir.path())
        .assert()
        .success();
    assert!(!git_branches(&dir).contains(&"feature/done".to_string()));
}

#[test]
fn config_remove_ignore_keeps_the_other_patterns() {
    let dir = init_repo();
    configure(&dir);

    for pattern in ["wip/*", "scratch", "tmp/*"] {
        Command::cargo_bin("git-wipe")
            .unwrap()
            .args(["config", "add-ignore", pattern])
            .current_dir(dir.path())
            .assert()
            .success();
    }

    Command::cargo_bin("git-wipe")
        .unwrap()
        .args(["config", "remove-ignore", "scratch"])
        .current_dir(dir.path())
        .assert()
        .success();

    let output = StdCommand::new("git")
        .args(["config", "--get-all", "wipe.ignore"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    let patterns: Vec<String> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|l| l.to_string())
        .collect();
    assert_eq!(
        patterns,
        vec!["wip/*".to_string(), "tmp/*".to_string()],
        "remove-ignore must preserve the remaining patterns and their order"
    );
}

#[test]
fn config_list_renders_empty_sections_as_none() {
    let dir = init_repo();

    // A config section that exists but holds no protected pattern.
    Command::cargo_bin("git-wipe")
        .unwrap()
        .args(["config", "add-ignore", "wip/*"])
        .current_dir(dir.path())
        .assert()
        .success();

    Command::cargo_bin("git-wipe")
        .unwrap()
        .args(["config", "list"])
        .current_dir(dir.path())
        .assert()
        .success()
        .stderr(predicate::str::contains("protected: (none)"))
        .stderr(predicate::str::contains("ignore: wip/*"))
        .stderr(predicate::str::contains("branch ignored: (none)"));
}

// ── JSON output ──────────────────────────────────────────────────────

/// Run the binary and parse its stdout as the single JSON document it must be.
fn json_output(dir: &TempDir, args: &[&str]) -> serde_json::Value {
    json_output_at(dir.path(), args)
}

fn json_output_at(path: &std::path::Path, args: &[&str]) -> serde_json::Value {
    let output = Command::cargo_bin("git-wipe")
        .unwrap()
        .args(args)
        .current_dir(path)
        .output()
        .unwrap();
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert_eq!(
        stdout.lines().count(),
        1,
        "piped output must be a single compact JSON line, got: {stdout}"
    );
    serde_json::from_str(&stdout).expect("stdout must be valid JSON")
}

/// Remote detection runs the same strategies as local detection (#28): a
/// squash-merged remote branch is reported at the default effort, but not at
/// `--effort 1`.
#[test]
fn json_reports_squash_merged_remote_branches() {
    let (_dir, work, _bare) = init_repo_with_remote();

    StdCommand::new("git")
        .args(["checkout", "-b", "feature/squashed", "main"])
        .current_dir(&work)
        .output()
        .unwrap();
    std::fs::write(work.join("squashed.txt"), "squashed").unwrap();
    StdCommand::new("git")
        .args(["add", "."])
        .current_dir(&work)
        .output()
        .unwrap();
    StdCommand::new("git")
        .args(["commit", "-m", "squashed feature"])
        .current_dir(&work)
        .output()
        .unwrap();
    StdCommand::new("git")
        .args(["push", "-u", "origin", "feature/squashed"])
        .current_dir(&work)
        .output()
        .unwrap();

    StdCommand::new("git")
        .args(["checkout", "main"])
        .current_dir(&work)
        .output()
        .unwrap();
    StdCommand::new("git")
        .args(["merge", "--squash", "feature/squashed"])
        .current_dir(&work)
        .output()
        .unwrap();
    StdCommand::new("git")
        .args(["commit", "-m", "squash merge"])
        .current_dir(&work)
        .output()
        .unwrap();
    StdCommand::new("git")
        .args(["push", "origin", "main"])
        .current_dir(&work)
        .output()
        .unwrap();

    let doc = json_output_at(&work, &["--json", "--dry-run", "--no-fetch"]);
    assert_eq!(doc["remotes"][0]["remote"], "origin");
    let merged = doc["remotes"][0]["merged"].as_array().unwrap();
    assert!(
        merged.iter().any(|b| b == "feature/squashed"),
        "default effort should report the squash-merged remote branch, got {merged:?}"
    );

    let quick = json_output_at(
        &work,
        &["--json", "--dry-run", "--no-fetch", "--effort", "1"],
    );
    let quick_merged = quick["remotes"][0]["merged"].as_array().unwrap();
    assert!(
        !quick_merged.iter().any(|b| b == "feature/squashed"),
        "quick effort keeps remote detection on ancestor merges, got {quick_merged:?}"
    );
}

#[test]
fn json_dry_run_reports_candidates_without_deleting() {
    let dir = init_repo();
    configure(&dir);
    add_branches(&dir);

    let doc = json_output(&dir, &["--json", "--dry-run", "--no-fetch"]);

    assert_eq!(doc["version"], 1);
    assert_eq!(doc["status"], "success");
    assert_eq!(doc["dry_run"], true);
    assert_eq!(doc["effort"], 2, "the default effort level is reported");
    assert_eq!(doc["local"]["merged"][0], "feature/done");
    assert_eq!(doc["local"]["branches"][0]["branch"], "feature/done");
    assert_eq!(doc["local"]["branches"][0]["reason"], "merged");
    assert_eq!(doc["local"]["branches"][0]["selected"], true);
    assert_eq!(doc["local"]["branches"][0]["status"], "dry_run");
    assert_eq!(doc["summary"]["local_branches_deleted"], 0);
    assert_eq!(doc["errors"].as_array().unwrap().len(), 0);

    // Nothing was touched.
    let branches = git_branches(&dir);
    assert!(branches.contains(&"feature/done".to_string()));
}

#[test]
fn json_run_reports_deleted_branches() {
    let dir = init_repo();
    configure(&dir);
    add_branches(&dir);

    let doc = json_output(&dir, &["--json", "--no-fetch"]);

    assert_eq!(doc["status"], "success");
    assert_eq!(doc["dry_run"], false);
    assert_eq!(doc["summary"]["local_branches_deleted"], 1);
    assert_eq!(doc["local"]["branches"][0]["branch"], "feature/done");
    assert_eq!(doc["local"]["branches"][0]["status"], "deleted");

    let branches = git_branches(&dir);
    assert!(!branches.contains(&"feature/done".to_string()));
    assert!(branches.contains(&"feature/wip".to_string()));
}

#[test]
fn json_implies_yes_without_prompting() {
    let dir = init_repo();
    configure(&dir);
    add_branches(&dir);

    // No `-y`: the run must still complete non-interactively.
    Command::cargo_bin("git-wipe")
        .unwrap()
        .args(["--json", "--no-fetch"])
        .current_dir(dir.path())
        .assert()
        .success();

    assert!(!git_branches(&dir).contains(&"feature/done".to_string()));
}

#[test]
fn json_keeps_stdout_free_of_human_output() {
    let dir = init_repo();
    configure(&dir);
    add_branches(&dir);

    let output = Command::cargo_bin("git-wipe")
        .unwrap()
        .args(["--json", "--dry-run", "--no-fetch"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(
        output.stderr.is_empty(),
        "stderr must stay silent in JSON mode"
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.starts_with('{') && stdout.trim_end().ends_with('}'));
}

#[test]
fn json_reports_a_young_worktree_as_too_young() {
    let dir = init_repo();
    configure(&dir);
    let wt_path = add_merged_worktree(&dir, "feature/young", "wt-young");

    let doc = json_output(
        &dir,
        &["--json", "--no-fetch", "--local-only", "--min-age", "1h"],
    );

    assert_eq!(doc["min_age"], "1h");
    // Matched on the branch, not the path: macOS resolves the temp dir through
    // /private, so git reports a different string than the fixture holds.
    let worktrees = doc["local"]["worktrees"].as_array().unwrap();
    let entry = worktrees
        .iter()
        .find(|w| w["branch"] == "feature/young")
        .expect("the young worktree should be reported");
    assert_eq!(entry["status"], "too_young");
    assert_eq!(entry["kind"], "branch");
    assert_eq!(doc["summary"]["worktrees_removed"], 0);
    assert!(wt_path.exists());
}

#[test]
fn json_reports_fatal_error_outside_a_repository() {
    let dir = tempfile::tempdir().unwrap();

    let output = Command::cargo_bin("git-wipe")
        .unwrap()
        .args(["--json", "--no-fetch"])
        .current_dir(dir.path())
        .env("GIT_CEILING_DIRECTORIES", dir.path())
        .output()
        .unwrap();

    assert!(!output.status.success(), "a fatal error must exit non-zero");
    let doc: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(doc["status"], "error");
    assert_eq!(doc["errors"][0]["kind"], "other");
    assert!(
        doc["errors"][0]["message"]
            .as_str()
            .unwrap()
            .contains("Not a git repository")
    );
}

#[test]
fn json_requires_an_existing_configuration() {
    let dir = init_repo();
    // No `[wipe]` section: the wizard cannot run without a human.

    let output = Command::cargo_bin("git-wipe")
        .unwrap()
        .args(["--json", "--no-fetch"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(!output.status.success());
    let doc: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(doc["status"], "error");
    assert!(
        doc["errors"][0]["message"]
            .as_str()
            .unwrap()
            .contains("not configured")
    );
}

#[test]
fn config_list_json_reports_values() {
    let dir = init_repo();
    configure(&dir);

    let doc = json_output(&dir, &["config", "list", "--json"]);

    assert_eq!(doc["configured"], true);
    assert_eq!(doc["protected"][0], "main");
    assert_eq!(doc["ignore"].as_array().unwrap().len(), 0);
    assert!(
        doc["remotes"].is_null(),
        "no configured remote means all remotes"
    );
    assert!(
        doc["worktrunk"].is_null(),
        "unset worktrunk means auto-detect"
    );
}

#[test]
fn config_list_json_reports_unconfigured_repository() {
    let dir = init_repo();

    let doc = json_output(&dir, &["config", "list", "--json"]);

    assert_eq!(doc["configured"], false);
    assert_eq!(doc["protected"].as_array().unwrap().len(), 0);
}

// ── Status subcommand ────────────────────────────────────────────────

/// Run `git wipe status` (or an alias) and return its stdout.
fn status_stdout(path: &std::path::Path, args: &[&str]) -> String {
    let output = Command::cargo_bin("git-wipe")
        .unwrap()
        .args(args)
        .current_dir(path)
        .output()
        .unwrap();
    assert!(output.status.success(), "status must exit 0");
    String::from_utf8(output.stdout).unwrap()
}

/// Snapshot of everything `status` promises not to touch.
fn repo_state(path: &std::path::Path) -> (String, String, String) {
    let read = |args: &[&str]| {
        let out = StdCommand::new("git")
            .args(args)
            .current_dir(path)
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).to_string()
    };
    (
        read(&["branch", "--format=%(refname:short)"]),
        read(&["worktree", "list", "--porcelain"]),
        read(&["for-each-ref", "--format=%(refname) %(objectname)"]),
    )
}

#[test]
fn status_lists_branches_and_worktrees() {
    let dir = init_repo();
    configure(&dir);
    add_branches(&dir);
    add_merged_worktree(&dir, "feature/wt", "wt");

    let out = status_stdout(dir.path(), &["status", "--no-color"]);

    assert!(
        out.lines()
            .next()
            .unwrap()
            .trim_start()
            .starts_with("BRANCH"),
        "first line must be the header, got: {out}"
    );
    for expected in [
        "STATUS",
        "BRANCH",
        "PATH",
        "main",
        "feature/done",
        "feature/wip",
        "feature/wt",
    ] {
        assert!(out.contains(expected), "{expected} missing from:\n{out}");
    }
}

#[test]
fn status_alias_list_behaves_identically() {
    let dir = init_repo();
    configure(&dir);
    add_branches(&dir);

    // Whether a whitespace-separated token is exactly what `format_age`
    // renders: a bare `?`, or digits followed by one of its unit suffixes.
    // Nothing else in the table (branch names, paths, sizes, statuses)
    // matches this shape — sizes use uppercase `B`/`K`/`M`/`G` and often a
    // `.`, so there is no ambiguity.
    fn is_age_token(token: &str) -> bool {
        token == "?"
            || (token.len() > 1
                && token.as_bytes()[..token.len() - 1]
                    .iter()
                    .all(u8::is_ascii_digit)
                && matches!(
                    token.as_bytes()[token.len() - 1],
                    b's' | b'm' | b'h' | b'd' | b'w'
                ))
    }

    // Ages tick between the two runs, so mask out the AGE column by its
    // value shape rather than its position — the point is that the alias
    // resolves to the same command, not that ages are frozen in time.
    let without_age = |args: &[&str]| {
        status_stdout(dir.path(), args)
            .lines()
            .map(|line| {
                line.split_whitespace()
                    .filter(|token| !is_age_token(token))
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(
        without_age(&["status", "--no-color"]),
        without_age(&["list", "--no-color"]),
    );
}

#[test]
fn status_works_without_configuration() {
    // No `configure(&dir)`: the setup wizard must not fire, and the command
    // must answer rather than ask. A prompt would block on a non-TTY stdin.
    let dir = init_repo();
    add_branches(&dir);

    let assertion = Command::cargo_bin("git-wipe")
        .unwrap()
        .args(["status", "--no-color"])
        .current_dir(dir.path())
        .assert()
        .success();
    let out = String::from_utf8(assertion.get_output().stdout.clone()).unwrap();
    assert!(out.contains("feature/wip"), "got:\n{out}");
    assert!(
        !String::from_utf8_lossy(&assertion.get_output().stderr).contains("setup"),
        "the setup wizard must not be mentioned"
    );
}

#[test]
fn status_changes_nothing() {
    let dir = init_repo();
    configure(&dir);
    add_branches(&dir);
    add_merged_worktree(&dir, "feature/wt", "wt");

    let before = repo_state(dir.path());
    status_stdout(dir.path(), &["status"]);
    status_stdout(dir.path(), &["status", "--merged"]);
    assert_eq!(before, repo_state(dir.path()), "status must not mutate");
}

#[test]
fn status_does_not_fetch() {
    let (dir, work, bare) = init_repo_with_remote();
    advance_remote_branch(&bare, dir.path());

    let before = git_rev_parse(&work, "refs/remotes/origin/main");
    status_stdout(&work, &["status"]);
    assert_eq!(
        before,
        git_rev_parse(&work, "refs/remotes/origin/main"),
        "status must never fetch"
    );
}

#[test]
fn status_json_emits_entries_and_no_action_fields() {
    let dir = init_repo();
    configure(&dir);
    add_branches(&dir);

    let doc = json_output(&dir, &["status", "--json"]);

    assert_eq!(doc["version"], 1);
    assert_eq!(doc["status"], "success");
    let entries = doc["entries"].as_array().unwrap();
    assert!(!entries.is_empty());
    assert!(entries[0]["status"].is_array());
    for absent in ["summary", "dry_run", "fetch", "pull", "local", "remotes"] {
        assert!(
            doc.get(absent).is_none(),
            "{absent} is an action field and must be absent"
        );
    }

    let merged: Vec<&str> = entries
        .iter()
        .filter(|e| {
            e["status"]
                .as_array()
                .unwrap()
                .iter()
                .any(|s| s == "merged")
        })
        .map(|e| e["branch"].as_str().unwrap())
        .collect();
    assert_eq!(merged, ["feature/done"]);
}

#[test]
fn status_json_warns_about_stale_gone_detection() {
    let (_dir, work) = init_repo_with_gone_upstream();
    // `status` never fetches, so it reports what a *previous* prune left on
    // disk — which is exactly the staleness the warning is about.
    StdCommand::new("git")
        .args(["fetch", "--prune"])
        .current_dir(&work)
        .output()
        .unwrap();

    let doc = json_output_at(&work, &["status", "--json"]);

    let warnings = doc["warnings"].as_array().unwrap();
    assert!(
        warnings
            .iter()
            .any(|w| w.as_str().unwrap().contains("not fetched")),
        "expected a staleness warning, got {warnings:?}"
    );
    assert!(
        doc["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["status"].as_array().unwrap().iter().any(|s| s == "gone")),
        "the gone branch must be reported"
    );
}

#[test]
fn status_min_age_filters_entries() {
    let dir = init_repo();
    configure(&dir);
    add_branches(&dir);

    // Everything in the fixture was created seconds ago.
    let doc = json_output(&dir, &["status", "--json", "--min-age", "1w"]);
    assert_eq!(doc["entries"].as_array().unwrap().len(), 0);

    let doc = json_output(&dir, &["status", "--json", "--min-age", "0"]);
    assert!(!doc["entries"].as_array().unwrap().is_empty());
}

#[test]
fn status_skips_sizing_by_default() {
    let dir = init_repo();
    configure(&dir);
    add_merged_worktree(&dir, "feature/unsized", "wt-unsized");

    let doc = json_output(&dir, &["status", "--json"]);
    assert!(
        doc["entries"]
            .as_array()
            .unwrap()
            .iter()
            .all(|e| e["size_kb"].is_null()),
        "sizing must be skipped unless --min-size or --size is given: {doc}"
    );
}

#[test]
fn status_size_flag_computes_sizes_without_filtering() {
    let dir = init_repo();
    configure(&dir);
    add_merged_worktree(&dir, "feature/sized", "wt-sized");

    let doc = json_output(&dir, &["status", "--json", "--size"]);
    let entries = doc["entries"].as_array().unwrap();
    assert!(
        entries.iter().any(|e| e["kind"] == "worktree"
            && e["branch"] == "feature/sized"
            && !e["size_kb"].is_null()),
        "the worktree row should have a computed size: {doc}"
    );
}

#[test]
fn status_min_size_filters_entries() {
    let dir = init_repo();
    configure(&dir);
    add_merged_worktree(&dir, "feature/small-wt", "wt-small");

    // The fixture worktree is tiny, so a large threshold excludes it, while
    // an unknown-size branch row (none here, since every row has a worktree
    // or is a plain branch) is unaffected either way.
    let doc = json_output(&dir, &["status", "--json", "--min-size", "1G"]);
    assert!(
        !doc["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["branch"] == "feature/small-wt"),
        "the small worktree must be filtered out: {doc}"
    );

    let doc = json_output(&dir, &["status", "--json", "--min-size", "0"]);
    assert!(
        doc["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["branch"] == "feature/small-wt"),
        "zero --min-size must keep everything: {doc}"
    );
}

#[test]
fn status_merged_only_filters_entries() {
    let dir = init_repo();
    configure(&dir);
    add_branches(&dir);

    let out = status_stdout(dir.path(), &["status", "--merged", "--no-color"]);
    assert!(out.contains("feature/done"), "got:\n{out}");
    assert!(!out.contains("feature/wip"), "got:\n{out}");
}

#[test]
fn status_no_color_strips_ansi() {
    let dir = init_repo();
    configure(&dir);
    add_branches(&dir);

    assert!(
        !status_stdout(dir.path(), &["status", "--no-color"]).contains('\x1b'),
        "--no-color must emit no ANSI escape"
    );
}

#[test]
fn status_orders_oldest_first() {
    let dir = init_repo();
    configure(&dir);
    let p = dir.path();

    // Two branches whose tips are years apart, so ordering is unambiguous.
    for (branch, date) in [
        ("old/one", "2001-01-01T00:00:00"),
        ("new/one", "2020-01-01T00:00:00"),
    ] {
        StdCommand::new("git")
            .args(["checkout", "-b", branch])
            .current_dir(p)
            .output()
            .unwrap();
        StdCommand::new("git")
            .args(["commit", "--allow-empty", "-m", branch])
            .env("GIT_COMMITTER_DATE", date)
            .env("GIT_AUTHOR_DATE", date)
            .current_dir(p)
            .output()
            .unwrap();
        StdCommand::new("git")
            .args(["checkout", "main"])
            .current_dir(p)
            .output()
            .unwrap();
    }

    let doc = json_output(&dir, &["status", "--json"]);
    let order: Vec<&str> = doc["entries"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|e| e["branch"].as_str())
        .collect();
    let old = order.iter().position(|b| *b == "old/one").unwrap();
    let new = order.iter().position(|b| *b == "new/one").unwrap();
    assert!(old < new, "oldest first, got {order:?}");
}

#[test]
fn status_help_documents_its_flags() {
    let dir = init_repo();

    Command::cargo_bin("git-wipe")
        .unwrap()
        .args(["status", "--help"])
        .current_dir(dir.path())
        .assert()
        .success()
        .stdout(
            predicate::str::contains("--merged")
                .and(predicate::str::contains("--min-age"))
                .and(predicate::str::contains("--min-size"))
                .and(predicate::str::contains("--size"))
                .and(predicate::str::contains("--effort"))
                .and(predicate::str::contains("--no-color")),
        );
}
