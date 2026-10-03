//! Thin, typed wrapper around the git command line.
//!
//! Everything that shells out to git funnels through [`Git`], so verbose
//! echoing, the working directory and error classification are decided in one
//! place. Failures surface as [`GitCommandError`], whose [`GitErrorKind`] lets
//! callers tell a network or auth problem from a genuine git error.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};

/// Classification of a failing git/external command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitErrorKind {
    /// Network connectivity failure (DNS, route, refused, timed out, …).
    Network,
    /// Authentication / authorization failure.
    Auth,
    /// Anything else (refspec issues, conflicts, …).
    Other,
}

/// Structured error for failing external commands invoked by this module.
///
/// Wrapped in `anyhow::Error` by [`run_git`] / [`run_cmd`] so callers can
/// downcast and react differently to network vs. auth vs. other failures.
#[derive(Debug, Clone)]
pub struct GitCommandError {
    /// Program name, e.g. `git` or `wt`.
    pub program: String,
    /// Arguments passed to the program.
    pub args: Vec<String>,
    /// Exit code, when one is available (signals yield `None`).
    pub exit_code: Option<i32>,
    /// Captured stderr (trimmed).
    pub stderr: String,
    /// Classified failure kind.
    pub kind: GitErrorKind,
}

impl GitCommandError {
    /// First non-empty line of stderr, used as a short cause summary.
    pub fn short_cause(&self) -> &str {
        self.stderr
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty())
            .unwrap_or("")
    }
}

impl fmt::Display for GitCommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let args = self.args.join(" ");
        match self.exit_code {
            Some(code) => write!(
                f,
                "{} {} failed (exit status: {}):\n{}",
                self.program,
                args,
                code,
                self.stderr.trim()
            ),
            None => write!(
                f,
                "{} {} terminated by signal:\n{}",
                self.program,
                args,
                self.stderr.trim()
            ),
        }
    }
}

impl std::error::Error for GitCommandError {}

/// Classify a stderr blob into a [`GitErrorKind`].
///
/// Recognises common network and authentication signatures emitted by
/// `git`/`ssh`/`curl`. Matching is case-insensitive on the lowered string;
/// every signature is plain ASCII so locale-translated tail messages (e.g.
/// French `"Impossible de lire le dépôt distant"`) do not derail the
/// classifier as long as the underlying tool emits its standard prefix
/// somewhere in stderr.
pub fn classify_git_stderr(stderr: &str) -> GitErrorKind {
    let lower = stderr.to_ascii_lowercase();

    const NETWORK_SIGNATURES: &[&str] = &[
        "no route to host",
        "could not resolve host",
        "name or service not known",
        "temporary failure in name resolution",
        "connection timed out",
        "connection refused",
        "network is unreachable",
        "operation timed out",
        "ssh: connect to host",
        "ssh: could not resolve hostname",
        "failed to connect to",
        "couldn't connect to server",
        "unable to access",
    ];
    if NETWORK_SIGNATURES.iter().any(|sig| lower.contains(sig)) {
        return GitErrorKind::Network;
    }

    const AUTH_SIGNATURES: &[&str] = &[
        "permission denied (publickey)",
        "permission denied, please try again",
        "authentication failed",
        "invalid credentials",
        "403 forbidden",
        "access denied",
    ];
    if AUTH_SIGNATURES.iter().any(|sig| lower.contains(sig)) {
        return GitErrorKind::Auth;
    }

    GitErrorKind::Other
}

/// Build a [`GitCommandError`] from a finished `std::process::Output`.
fn command_error(program: &str, args: &[&str], output: &std::process::Output) -> GitCommandError {
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    GitCommandError {
        program: program.to_string(),
        args: args.iter().map(|s| s.to_string()).collect(),
        exit_code: output.status.code(),
        kind: classify_git_stderr(&stderr),
        stderr,
    }
}

/// Run a git command and return its stdout as a trimmed string.
///
/// If `verbose` is true the command is printed to stderr before execution.
fn run_git(args: &[&str], verbose: bool, workdir: Option<&Path>) -> Result<String> {
    if verbose {
        eprintln!("  $ git {}", args.join(" "));
    }

    let mut cmd = Command::new("git");
    cmd.args(args);
    if let Some(dir) = workdir {
        cmd.current_dir(dir);
    }

    let output = cmd
        .output()
        .with_context(|| format!("failed to execute: git {}", args.join(" ")))?;

    if !output.status.success() {
        return Err(anyhow::Error::new(command_error("git", args, &output)));
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Run an external command (not git) and return its stdout as a trimmed string.
///
/// If `verbose` is true the command is printed to stderr before execution.
fn run_cmd(bin: &str, args: &[&str], verbose: bool, workdir: Option<&Path>) -> Result<String> {
    if verbose {
        eprintln!("  $ {} {}", bin, args.join(" "));
    }

    let mut cmd = Command::new(bin);
    cmd.args(args);
    if let Some(dir) = workdir {
        cmd.current_dir(dir);
    }

    let output = cmd
        .output()
        .with_context(|| format!("failed to execute: {} {}", bin, args.join(" ")))?;

    if !output.status.success() {
        return Err(anyhow::Error::new(command_error(bin, args, &output)));
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Whether a `git config` failure just means "the key is not set".
///
/// `git config --get`, `--get-all` and `--get-regexp` all exit 1 when they
/// find nothing, which is a normal result rather than an error. Every other
/// exit code — 128 for a broken repository, or a failure to spawn git at all —
/// must be propagated, otherwise a genuinely broken environment is silently
/// reported as an empty configuration.
fn is_unset_key(err: &anyhow::Error) -> bool {
    err.downcast_ref::<GitCommandError>()
        .is_some_and(|gerr| gerr.exit_code == Some(1))
}

/// Parse a boolean the way `git config --type=bool` does: `true`, `yes`, `on`
/// and `1` against `false`, `no`, `off` and `0`, case-insensitively.
///
/// `None` for anything else, so callers can reject a typo instead of silently
/// reading it as `false`.
pub fn parse_git_bool(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" | "yes" | "on" | "1" => Some(true),
        "false" | "no" | "off" | "0" => Some(false),
        _ => None,
    }
}

/// Whether a `git config --unset` failure just means "there was nothing to
/// unset".
///
/// `--unset` and `--unset-all` exit 5 when the key is absent. Anything else — a
/// locked or unwritable config file, a broken repository — is a real failure
/// that must not be reported as a successful unset.
fn is_unset_noop(err: &anyhow::Error) -> bool {
    err.downcast_ref::<GitCommandError>()
        .is_some_and(|gerr| gerr.exit_code == Some(5))
}

/// Render a path as a git command-line argument.
///
/// git arguments must be UTF-8; a path that is not is a hard error rather than
/// something to silently mangle with `to_string_lossy`.
pub fn path_arg(path: &Path) -> Result<&str> {
    path.to_str()
        .with_context(|| format!("path is not valid UTF-8: {}", path.display()))
}

/// Check if the worktrunk CLI (`wt`) is available on `$PATH`.
pub fn worktrunk_available() -> bool {
    Command::new("wt")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        // `is_ok` alone only proves the process was spawned; a `wt` that exists
        // but fails to run would still be reported as available.
        .is_ok_and(|status| status.success())
}

/// A thin wrapper around git CLI invocations.
#[derive(Debug, Clone)]
pub struct Git {
    verbose: bool,
    workdir: Option<PathBuf>,
}

impl Git {
    /// Create a handle operating in the process's current directory.
    pub fn new(verbose: bool) -> Self {
        Self {
            verbose,
            workdir: None,
        }
    }

    /// Create a Git instance that operates in a specific directory.
    pub fn with_workdir(verbose: bool, workdir: &Path) -> Self {
        Self {
            verbose,
            workdir: Some(workdir.to_path_buf()),
        }
    }

    /// Re-root this instance at `dir`, preserving the verbose setting.
    ///
    /// Preferred over passing `git -C <dir>`: mixing `-C` with a configured
    /// working directory makes a relative `-C` resolve against the latter,
    /// which is surprising. Keeping the directory in one place removes the
    /// interaction entirely.
    pub fn in_dir(&self, dir: &Path) -> Self {
        Self::with_workdir(self.verbose, dir)
    }

    fn run(&self, args: &[&str]) -> Result<String> {
        run_git(args, self.verbose, self.workdir.as_deref())
    }

    fn run_wt(&self, args: &[&str]) -> Result<String> {
        run_cmd("wt", args, self.verbose, self.workdir.as_deref())
    }

    /// Spawn a git command to completion and return its raw [`Output`].
    ///
    /// Owns the verbose echo and the working-directory wiring so that every
    /// git invocation in this module behaves identically. Callers that need
    /// the trimmed stdout and a bail-on-failure contract should use
    /// [`Self::run`]; this is for the handful of commands that encode their
    /// result in the exit status.
    fn spawn(&self, args: &[&str]) -> Result<std::process::Output> {
        if self.verbose {
            eprintln!("  $ git {}", args.join(" "));
        }

        let mut cmd = Command::new("git");
        cmd.args(args);
        if let Some(dir) = &self.workdir {
            cmd.current_dir(dir);
        }

        cmd.output()
            .with_context(|| format!("failed to execute: git {}", args.join(" ")))
    }

    /// Pipe the stdout of one git command into the stdin of another and
    /// return the second command's trimmed stdout.
    ///
    /// Both commands inherit the verbose echo and working directory from
    /// `self`. A failure of either side is reported as a [`GitCommandError`].
    fn run_piped(&self, first: &[&str], second: &[&str]) -> Result<String> {
        if self.verbose {
            eprintln!("  $ git {} | git {}", first.join(" "), second.join(" "));
        }

        let mut first_cmd = Command::new("git");
        first_cmd.args(first);
        if let Some(dir) = &self.workdir {
            first_cmd.current_dir(dir);
        }
        first_cmd.stdout(std::process::Stdio::piped());
        first_cmd.stderr(std::process::Stdio::piped());

        let mut first_child = first_cmd
            .spawn()
            .with_context(|| format!("failed to execute: git {}", first.join(" ")))?;

        let first_stdout = first_child
            .stdout
            .take()
            .with_context(|| format!("failed to capture stdout of: git {}", first.join(" ")))?;

        // Drain stderr concurrently: left unread, a chatty first command could
        // fill the pipe and block, which would stall the second one on EOF.
        let first_stderr = first_child.stderr.take().map(|mut stderr| {
            std::thread::spawn(move || {
                use std::io::Read;
                let mut buf = Vec::new();
                let _ = stderr.read_to_end(&mut buf);
                buf
            })
        });

        let mut second_cmd = Command::new("git");
        second_cmd.args(second);
        if let Some(dir) = &self.workdir {
            second_cmd.current_dir(dir);
        }
        second_cmd.stdin(first_stdout);
        second_cmd.stdout(std::process::Stdio::piped());
        second_cmd.stderr(std::process::Stdio::piped());

        let second_output = second_cmd
            .spawn()
            .with_context(|| format!("failed to execute: git {}", second.join(" ")))?
            .wait_with_output()
            .with_context(|| format!("failed to wait for: git {}", second.join(" ")))?;

        let first_status = first_child
            .wait()
            .with_context(|| format!("failed to wait for: git {}", first.join(" ")))?;

        let first_output = std::process::Output {
            status: first_status,
            stdout: Vec::new(),
            stderr: first_stderr
                .map(|handle| handle.join().unwrap_or_default())
                .unwrap_or_default(),
        };
        if !first_output.status.success() {
            return Err(anyhow::Error::new(command_error(
                "git",
                first,
                &first_output,
            )));
        }
        if !second_output.status.success() {
            return Err(anyhow::Error::new(command_error(
                "git",
                second,
                &second_output,
            )));
        }

        Ok(String::from_utf8_lossy(&second_output.stdout)
            .trim()
            .to_string())
    }

    /// Resolve the merge-base of two refs.
    ///
    /// Returns `Ok(None)` when the refs have no common ancestor (unrelated
    /// histories, which `git merge-base` signals with exit code 1). Any other
    /// failure — a bad ref, a missing git binary — is propagated so it is not
    /// silently mistaken for "not merged".
    fn merge_base(&self, target: &str, branch: &str) -> Result<Option<String>> {
        let args = ["merge-base", target, branch];
        let output = self.spawn(&args)?;

        match output.status.code() {
            Some(0) => {
                let merge_base = String::from_utf8_lossy(&output.stdout).trim().to_string();
                Ok((!merge_base.is_empty()).then_some(merge_base))
            }
            // Exit 1: no merge base found (unrelated histories).
            Some(1) => Ok(None),
            _ => Err(anyhow::Error::new(command_error("git", &args, &output))),
        }
    }

    /// Run a git command and return whether it exited successfully.
    ///
    /// Unlike [`run`], this method does **not** bail on a non-zero exit code.
    /// Exit code 0 returns `Ok(true)`, exit code 1 returns `Ok(false)`.
    /// Any other exit code (e.g. 128 for bad refs) is treated as a real error.
    ///
    /// This is useful for commands like `git diff --quiet` that encode their
    /// result in the exit status rather than in stdout.
    fn run_exit_code(&self, args: &[&str]) -> Result<bool> {
        let output = self.spawn(args)?;

        match output.status.code() {
            Some(0) => Ok(true),
            Some(1) => Ok(false),
            _ => Err(anyhow::Error::new(command_error("git", args, &output))),
        }
    }

    // ── Repository info ──────────────────────────────────────────────

    /// Check whether the current working directory is inside a git work tree.
    ///
    /// Returns `Ok(true)` when inside a repository, `Ok(false)` otherwise.
    /// This intentionally swallows stderr so callers can show a friendly
    /// error message instead of raw git output.
    pub fn is_inside_work_tree(&self) -> Result<bool> {
        let output = self.spawn(&["rev-parse", "--is-inside-work-tree"])?;

        Ok(output.status.success())
    }

    /// Return the current branch name.
    pub fn current_branch(&self) -> Result<String> {
        self.run(&["rev-parse", "--abbrev-ref", "HEAD"])
    }

    /// Return the list of configured remotes.
    pub fn remotes(&self) -> Result<Vec<String>> {
        let out = self.run(&["remote"])?;
        Ok(out.lines().map(|l| l.to_string()).collect())
    }

    // ── Fetch ────────────────────────────────────────────────────────

    /// Fetch a single remote and prune deleted remote-tracking branches.
    ///
    /// Runs `git fetch --prune <remote>`. Failures are returned to the caller
    /// so that one unreachable remote does not abort the whole workflow.
    ///
    /// When `exclude` contains patterns, negative refspecs are appended so the
    /// matching refs are never transferred. Negative refspecs require git 2.29
    /// or later and only understand a single `*` wildcard, so patterns git
    /// cannot express are dropped here (they are still filtered out later by
    /// the globset matcher). Passing explicit refspecs also bypasses any custom
    /// `remote.<name>.fetch` configuration, so the default mapping is restated.
    ///
    /// A negative refspec also takes the excluded refs out of `--prune`'s
    /// scope, so a tracking ref for an ignored branch that was deleted on the
    /// remote would linger forever. When negatives are used, a follow-up
    /// `git remote prune <remote>` (a ref listing only, no object transfer)
    /// removes those stale refs, giving the same result as `git fetch --prune`.
    ///
    /// Returns `Err` when the fetch itself fails. When only that follow-up
    /// prune fails, the fetch is still good: `Ok(Some(message))` carries a
    /// warning for the caller to surface.
    pub fn fetch_remote_prune(&self, remote: &str, exclude: &[String]) -> Result<Option<String>> {
        let negatives: Vec<String> = exclude
            .iter()
            .filter(|p| refspec_safe(p))
            .map(|p| format!("^refs/heads/{p}"))
            .collect();

        if negatives.is_empty() {
            self.run(&["fetch", "--prune", remote])?;
            return Ok(None);
        }

        let positive = format!("+refs/heads/*:refs/remotes/{remote}/*");
        let mut args: Vec<&str> = vec!["fetch", "--prune", remote, &positive];
        args.extend(negatives.iter().map(String::as_str));
        self.run(&args)?;

        Ok(self.run(&["remote", "prune", remote]).err().map(|e| {
            format!("Could not prune stale tracking refs of ignored branches on {remote}: {e}")
        }))
    }

    // ── Pull / fast-forward ─────────────────────────────────────────

    /// Look up the upstream remote and branch for a local branch.
    ///
    /// Reads `branch.<name>.remote` and `branch.<name>.merge` from git config.
    /// Returns `Some((remote, branch))` if both are set, `None` otherwise.
    /// The merge ref (e.g. `refs/heads/main`) is stripped to just the branch name.
    pub fn branch_upstream(&self, branch: &str) -> Result<Option<(String, String)>> {
        let remote = match self.config_get(&format!("branch.{branch}.remote"))? {
            Some(r) => r,
            None => return Ok(None),
        };
        let merge = match self.config_get(&format!("branch.{branch}.merge"))? {
            Some(m) => m,
            None => return Ok(None),
        };
        let upstream_branch = merge
            .strip_prefix("refs/heads/")
            .unwrap_or(&merge)
            .to_string();
        Ok(Some((remote, upstream_branch)))
    }

    /// Run `git pull --ff-only` in the current working directory.
    ///
    /// Used for target branches checked out in the current worktree.
    pub fn pull_ff_only(&self) -> Result<()> {
        self.run(&["pull", "--ff-only"])?;
        Ok(())
    }

    /// Run `git pull --ff-only` in the given directory.
    ///
    /// Used for target branches checked out in a different worktree
    /// (the one we are running from is handled by [`Self::pull_ff_only`]).
    pub fn pull_ff_only_in(&self, dir: &Path) -> Result<()> {
        self.in_dir(dir).pull_ff_only()
    }

    /// Fast-forward a local branch ref to match its remote-tracking branch.
    ///
    /// Runs `git fetch <remote> <remote_branch>:<local_branch>`.
    /// Only works for branches **not** checked out in any worktree.
    pub fn fetch_update_branch(
        &self,
        remote: &str,
        remote_branch: &str,
        local_branch: &str,
    ) -> Result<()> {
        let refspec = format!("{remote_branch}:{local_branch}");
        self.run(&["fetch", remote, &refspec])?;
        Ok(())
    }

    // ── Branch queries ───────────────────────────────────────────────

    /// Return local branches that have been merged into `target`.
    pub fn merged_branches(&self, target: &str) -> Result<Vec<String>> {
        let out = self.run(&["branch", "--merged", target])?;
        Ok(parse_branch_list(&out))
    }

    /// Return all local branch names.
    pub fn local_branches(&self) -> Result<Vec<String>> {
        let out = self.run(&["branch", "--format=%(refname:short)"])?;
        Ok(out
            .lines()
            .filter(|l| !l.is_empty())
            .map(|l| l.to_string())
            .collect())
    }

    /// Return local branches configured with an upstream whose remote-tracking
    /// ref no longer exists.
    ///
    /// This is the state `git branch -vv` renders as `[origin/x: gone]`, and is
    /// the typical footprint of a branch whose pull request was merged and whose
    /// remote branch was deleted, followed by `git fetch --prune`.
    ///
    /// Existence is resolved by set-difference against `refs/remotes` rather
    /// than by reading `%(upstream:track)`, so the result does not depend on
    /// git's output locale.
    pub fn branches_with_gone_upstream(&self) -> Result<Vec<String>> {
        let heads = self.run(&[
            "for-each-ref",
            "--format=%(refname:short)%00%(upstream)",
            "refs/heads",
        ])?;
        let remotes = self.run(&["for-each-ref", "--format=%(refname)", "refs/remotes"])?;
        Ok(parse_gone_upstreams(&heads, &remotes))
    }

    /// Return the committer date, in Unix seconds, of every local branch tip.
    ///
    /// One `git for-each-ref` over `refs/heads` rather than one probe per
    /// branch: the status listing needs an age for *every* branch, and the
    /// per-branch cost would dominate the command on a large repository.
    ///
    /// Branch names are short (`feature/x`). A ref whose date git cannot render
    /// is simply absent from the map; callers must read a missing entry as
    /// "unknown age" rather than as zero.
    pub fn branch_committer_dates(&self) -> Result<HashMap<String, u64>> {
        let out = self.run(&[
            "for-each-ref",
            "--format=%(refname:short)%00%(committerdate:unix)",
            "refs/heads",
        ])?;
        Ok(parse_committer_dates(&out))
    }

    /// Return remote-tracking branches merged into `target` for the given remote.
    pub fn merged_remote_branches(&self, target: &str, remote: &str) -> Result<Vec<String>> {
        let out = self.run(&["branch", "-r", "--merged", target])?;
        Ok(parse_remote_branch_list(&out, remote))
    }

    /// Return every remote-tracking branch of `remote`, in short form
    /// (`feature/x`, not `origin/feature/x`).
    ///
    /// The `origin/HEAD -> origin/main` symref line is skipped, so the result
    /// only contains branches that can meaningfully be compared or deleted.
    pub fn remote_branches(&self, remote: &str) -> Result<Vec<String>> {
        let out = self.run(&["branch", "-r", "--format=%(refname:short)"])?;
        Ok(parse_remote_branch_list(&out, remote))
    }

    /// Return the URL of `remote`, with `url.<base>.insteadOf` rewrites applied.
    pub fn remote_url(&self, remote: &str) -> Result<String> {
        self.run(&["remote", "get-url", remote])
    }

    /// Return the tip commit SHA of every ref under `prefix`, keyed by the ref
    /// name with `prefix` stripped.
    ///
    /// `refs/heads/` yields local branches (`feature/x`); `refs/remotes/origin/`
    /// yields the remote-tracking branches of `origin` in short form. A
    /// symbolic `HEAD` entry is skipped.
    pub fn ref_tips(&self, prefix: &str) -> Result<HashMap<String, String>> {
        let out = self.run(&[
            "for-each-ref",
            "--format=%(refname)%00%(objectname)",
            prefix.trim_end_matches('/'),
        ])?;
        Ok(parse_ref_tips(&out, prefix))
    }

    /// Use `git cherry` to detect rebase-merged branches.
    ///
    /// Returns `true` when every commit of `branch` has already been applied
    /// on `upstream` — that is, when `git cherry` prefixes every line with
    /// `-`. Returns `false` when the output is empty, which means the branch
    /// has no commits ahead of `upstream`.
    pub fn cherry_merged(&self, upstream: &str, branch: &str) -> Result<bool> {
        let out = self.run(&["cherry", upstream, branch])?;
        // If all lines start with `-`, every commit was cherry-picked upstream.
        Ok(!out.is_empty() && out.lines().all(|l| l.starts_with('-')))
    }

    /// Compare the tree objects of two refs.
    ///
    /// Runs `git rev-parse <target>^{tree}` and `git rev-parse <branch>^{tree}`
    /// and returns `true` when the SHA hashes are identical — meaning the two
    /// refs point at exactly the same file content.  This is the cheapest
    /// possible content-equality check (two rev-parse calls, no diff traversal).
    pub fn trees_match(&self, target: &str, branch: &str) -> Result<bool> {
        let target_tree = self.run(&["rev-parse", &format!("{target}^{{tree}}")])?;
        let branch_tree = self.run(&["rev-parse", &format!("{branch}^{{tree}}")])?;
        Ok(target_tree.trim() == branch_tree.trim())
    }

    /// Check whether `branch` introduces no content change of its own.
    ///
    /// Runs `git diff --quiet <target>...<branch>` — the three-dot form, which
    /// diffs the merge base of `target` and `branch` against `branch`. An empty
    /// diff (exit 0) means the branch's commits net out to nothing relative to
    /// the point where it forked: a commit followed by its revert, a branch
    /// that only rewrote history without changing content, or a branch created
    /// but never meaningfully advanced. Such a branch has nothing to lose.
    ///
    /// Note: this deliberately does *not* detect squash merges. Once `branch`
    /// has been squashed onto `target`, the three-dot diff still shows the
    /// branch's changes (the merge base predates the squash commit). Squash
    /// merges are covered by [`Self::merge_adds_nothing`] and
    /// [`Self::squash_patch_id_match`]; branches whose tree is identical to the
    /// target's are covered by the cheaper [`Self::trees_match`].
    pub fn diff_empty(&self, target: &str, branch: &str) -> Result<bool> {
        self.run_exit_code(&["diff", "--quiet", &format!("{target}...{branch}")])
    }

    /// Compute the patch-ids of the commits in `range` (e.g. `"a..b"`).
    ///
    /// Pipes `git log -p --no-merges <range>` into `git patch-id --stable`
    /// and returns the first column (the patch-id) of each output line.
    /// Merge commits are filtered out — they have no meaningful patch-id.
    fn patch_ids_for_range(&self, range: &str) -> Result<Vec<String>> {
        let stdout = self.run_piped(
            &["log", "-p", "--no-merges", range],
            &["patch-id", "--stable"],
        )?;

        let ids: Vec<String> = stdout
            .lines()
            .filter_map(|l| l.split_whitespace().next().map(|s| s.to_string()))
            .collect();
        Ok(ids)
    }

    /// Detect branches whose commits have been re-applied on `target` with
    /// different SHAs (rebase + conflict resolution, partial cherry-picks,
    /// history rewrites) by comparing `git patch-id` fingerprints.
    ///
    /// For every commit in `target..branch`, computes its patch-id and
    /// checks that the same patch-id appears in `<merge-base>..<target>`.
    /// Returns `true` only when every branch commit has a match.
    ///
    /// Acts as a fallback when `cherry_merged`, `trees_match` and
    /// `diff_empty` miss — e.g. a branch that was rebased onto target with
    /// conflict resolution that altered the diff slightly is **not** caught
    /// here either (patch-id is content-sensitive), but a rebase that
    /// preserved the diff while changing commit metadata or parent linkage
    /// is.
    ///
    /// Returns `Ok(false)` (rather than bailing) when the two refs have no
    /// common merge-base (unrelated histories).
    pub fn patch_id_match(&self, target: &str, branch: &str) -> Result<bool> {
        let range = format!("{target}..{branch}");
        let branch_ids = self.patch_ids_for_range(&range)?;
        if branch_ids.is_empty() {
            // Branch has no commits beyond target — already merged.
            return Ok(true);
        }

        // Resolve merge-base; if unrelated histories, bail out as "no match".
        let merge_base = match self.merge_base(target, branch)? {
            Some(mb) => mb,
            None => return Ok(false),
        };

        let target_range = format!("{merge_base}..{target}");
        let target_ids: std::collections::HashSet<String> = self
            .patch_ids_for_range(&target_range)?
            .into_iter()
            .collect();
        if target_ids.is_empty() {
            return Ok(false);
        }

        Ok(branch_ids.iter().all(|id| target_ids.contains(id)))
    }

    /// Simulate merging `branch` into `target` and check the result tree.
    ///
    /// Runs `git merge-tree --write-tree <target> <branch>` which performs an
    /// in-memory three-way merge and prints the resulting tree OID on stdout.
    /// If that OID matches `target`'s current tree, the merge would add
    /// nothing — meaning the branch's content is already fully represented in
    /// target.
    ///
    /// This is the strongest squash-merge detector available: unlike
    /// [`Self::diff_empty`] and [`Self::trees_match`], it still returns
    /// `true` after the target has advanced with unrelated commits that
    /// touch different files.
    ///
    /// Returns `Ok(false)` when the merge would conflict (exit 1) or when the
    /// resulting tree differs from `target`'s tree. Any other failure is an
    /// error. Requires `git >= 2.38` for the `--write-tree` option.
    pub fn merge_adds_nothing(&self, target: &str, branch: &str) -> Result<bool> {
        let args = ["merge-tree", "--write-tree", target, branch];
        let output = self.spawn(&args)?;

        match output.status.code() {
            Some(0) => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let merged_tree = match stdout.lines().next() {
                    Some(line) => line.trim().to_string(),
                    None => return Ok(false),
                };
                let target_tree = self.run(&["rev-parse", &format!("{target}^{{tree}}")])?;
                Ok(merged_tree == target_tree.trim())
            }
            // Exit 1: the merge conflicts. Treat as "merge would add
            // something" rather than an error, so callers can keep probing
            // other strategies.
            Some(1) => Ok(false),
            // Anything else (129 on git < 2.38 which lacks `--write-tree`,
            // 128 for a fatal error, a signal) is a real failure, propagated so
            // the caller can surface it instead of silently reading "not
            // merged".
            _ => Err(anyhow::Error::new(command_error("git", &args, &output))),
        }
    }

    /// Compute the combined patch-id of `git diff <base> <branch>` — i.e. the
    /// single patch-id of the branch's cumulative diff (as if it had been
    /// squashed into one commit).
    ///
    /// Returns `Ok(None)` when the diff is empty (branch is identical to
    /// `base`).
    fn squash_patch_id(&self, base: &str, branch: &str) -> Result<Option<String>> {
        let stdout = self.run_piped(&["diff", base, branch], &["patch-id", "--stable"])?;

        let id = stdout
            .lines()
            .next()
            .and_then(|l| l.split_whitespace().next().map(|s| s.to_string()));
        Ok(id)
    }

    /// Detect squash-merged branches by matching the combined patch-id of the
    /// branch's full diff (vs the merge-base) against the patch-id of any
    /// single commit in `<merge-base>..<target>`.
    ///
    /// Complements [`Self::patch_id_match`] (which compares per-commit
    /// patch-ids and therefore cannot relate N branch commits to 1 squash
    /// commit) and [`Self::merge_adds_nothing`] (which performs a textual
    /// in-memory merge and is defeated when the squash commit on `target`
    /// was later edited — e.g. PR review changes, formatting tweaks).
    ///
    /// Returns `Ok(false)` when the two refs have no common merge-base
    /// (unrelated histories).
    pub fn squash_patch_id_match(&self, target: &str, branch: &str) -> Result<bool> {
        // Resolve merge-base; unrelated histories ⇒ no match.
        let merge_base = match self.merge_base(target, branch)? {
            Some(mb) => mb,
            None => return Ok(false),
        };

        // Combined patch-id of the whole branch as a single squashed diff.
        let branch_pid = match self.squash_patch_id(&merge_base, branch)? {
            Some(id) => id,
            // Empty diff: branch is already at merge-base ⇒ fully merged.
            None => return Ok(true),
        };

        // Per-commit patch-ids on target since merge-base. A squash-merge
        // produces exactly one commit on target whose patch-id equals the
        // combined patch-id of the branch.
        let target_range = format!("{merge_base}..{target}");
        let target_ids = self.patch_ids_for_range(&target_range)?;
        Ok(target_ids.contains(&branch_pid))
    }

    // ── Branch mutations ─────────────────────────────────────────────

    /// Delete a local branch (force).
    ///
    /// Uses `-D` instead of `-d` because the caller has already verified the
    /// branch is merged into a protected target. The soft `-d` flag only
    /// checks against HEAD which fails when running from a linked worktree
    /// whose HEAD differs from the merge target.
    pub fn branch_delete(&self, branch: &str) -> Result<()> {
        self.run(&["branch", "-D", branch])?;
        Ok(())
    }

    /// Return true if a local branch ref exists.
    ///
    /// Used to verify whether `wt remove` actually deleted the associated
    /// branch; `wt`'s own merge check is narrower than git-wipe's, so it may
    /// leave behind a branch git-wipe considers merged.
    pub fn branch_exists(&self, branch: &str) -> Result<bool> {
        self.run_exit_code(&[
            "show-ref",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ])
    }

    /// Delete a branch on a remote (with --force-with-lease for safety).
    pub fn remote_branch_delete(&self, remote: &str, branch: &str) -> Result<()> {
        self.run(&["push", "--delete", "--force-with-lease", remote, branch])?;
        Ok(())
    }

    // ── Worktree operations ──────────────────────────────────────────

    /// Return the parsed entries of `git worktree list --porcelain`.
    pub fn worktree_list(&self) -> Result<Vec<Worktree>> {
        let out = self.run(&["worktree", "list", "--porcelain"])?;
        Ok(parse_worktree_list(&out))
    }

    /// Absolute path of a worktree's administrative directory.
    ///
    /// For a linked worktree this is `<common-dir>/worktrees/<id>`, created by
    /// `git worktree add` and never recreated afterwards, which makes its
    /// birth time a faithful worktree creation time. For the main worktree it
    /// is the repository's `.git` directory.
    pub fn worktree_git_dir(&self, path: &Path) -> Result<PathBuf> {
        let out = self
            .in_dir(path)
            .run(&["rev-parse", "--absolute-git-dir"])?;
        Ok(PathBuf::from(out.trim()))
    }

    /// Remove a worktree by path.
    pub fn worktree_remove(&self, path: &Path, force: bool) -> Result<()> {
        let path = path_arg(path)?;
        if force {
            self.run(&["worktree", "remove", "--force", path])?;
        } else {
            self.run(&["worktree", "remove", path])?;
        }
        Ok(())
    }

    /// Remove the lock on a worktree (`git worktree unlock`).
    pub fn worktree_unlock(&self, path: &Path) -> Result<()> {
        let path = path_arg(path)?;
        self.run(&["worktree", "unlock", path])?;
        Ok(())
    }

    /// Prune stale worktree administrative entries.
    ///
    /// `wt remove` may fall back to `git worktree remove` on cross-filesystem
    /// worktrees and can leave the worktree's git metadata registered even
    /// after its directory is gone. A stale registration makes `git branch -D`
    /// fail with "cannot delete branch used by worktree", so prune before
    /// deleting a branch whose worktree `wt` claims to have removed.
    pub fn worktree_prune(&self) -> Result<()> {
        self.run(&["worktree", "prune"])?;
        Ok(())
    }

    /// Check whether the worktree at `path` has untracked or uncommitted changes.
    ///
    /// Runs `git status --porcelain` from `path`; a non-empty output means
    /// the worktree is dirty.
    pub fn worktree_dirty(&self, path: &Path) -> Result<bool> {
        let out = self.in_dir(path).status_porcelain()?;
        Ok(!out.trim().is_empty())
    }

    /// List every tracked and untracked-but-not-ignored file inside `path`.
    ///
    /// Runs `git ls-files --cached --others --exclude-standard` from `path`.
    /// Gitignored files (build output, caches, dependency directories) are
    /// excluded, so scanning these for the newest mtime cannot be fooled by a
    /// build or a dependency install.
    pub fn worktree_files(&self, path: &Path) -> Result<Vec<String>> {
        let out =
            self.in_dir(path)
                .run(&["ls-files", "--cached", "--others", "--exclude-standard"])?;
        Ok(out
            .lines()
            .filter(|l| !l.is_empty())
            .map(String::from)
            .collect())
    }

    /// Return the committer date, in Unix seconds, of `rev` as seen from `path`.
    ///
    /// `Ok(None)` covers every reason git can't render a date for `rev` — an
    /// unborn `HEAD`, a dangling ref left by a deleted branch, anything short
    /// of the command failing to launch — since a caller building a fallback
    /// chain must be able to treat "no date" the same way regardless of cause.
    pub fn committer_date(&self, path: &Path, rev: &str) -> Result<Option<u64>> {
        let output = self
            .in_dir(path)
            .spawn(&["log", "-1", "--format=%ct", rev])?;
        if !output.status.success() {
            return Ok(None);
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().parse().ok())
    }

    /// Return the porcelain status of the current working directory.
    pub fn status_porcelain(&self) -> Result<String> {
        self.run(&["status", "--porcelain"])
    }

    /// Check whether `branch` has at least one commit not present in **any**
    /// of the given `targets`.
    ///
    /// Used to detect branches that would be rejected by `wt remove` (without
    /// `--force-delete`) or by `git branch -d`. When `targets` is empty the
    /// branch is considered unmerged.
    pub fn branch_has_unmerged_commits(&self, branch: &str, targets: &[String]) -> Result<bool> {
        if targets.is_empty() {
            return Ok(true);
        }
        for t in targets {
            // `git log -1 --format=%H <target>..<branch>` is empty when the
            // branch contains no commits absent from `target`.
            let out = self.run(&["log", "-1", "--format=%H", &format!("{t}..{branch}")])?;
            if out.trim().is_empty() {
                return Ok(false);
            }
        }
        Ok(true)
    }

    // ── Worktrunk integration ────────────────────────────────────────

    /// Check if a worktrunk config section exists in git config.
    ///
    /// Worktrunk stores its state under the `[worktrunk]` git config section.
    /// Its presence indicates the repository is managed by worktrunk.
    pub fn worktrunk_config_exists(&self) -> Result<bool> {
        self.config_section_exists("worktrunk")
    }

    /// Remove a worktree via the worktrunk CLI, triggering pre/post-remove hooks.
    ///
    /// `target` is either a branch name or a worktree path — `wt remove`
    /// accepts both in the same positional slot, so path-based removal (for
    /// detached-HEAD worktrees and orphans, where no branch name is
    /// available) needs no separate entry point.
    ///
    /// Uses `--foreground` to wait for hooks to complete and `--yes` to skip
    /// wt's approval prompts (git-wipe already confirmed with the user).
    /// `wt` deletes the associated branch itself; git-wipe skips its own
    /// `git branch -D` for branches removed this way.
    ///
    /// When `force` is true, passes `--force` so removal succeeds despite
    /// untracked/uncommitted changes. When `force_delete` is true, passes
    /// `--force-delete` so branch deletion succeeds even when the branch has
    /// commits not merged into a target.
    pub fn worktrunk_remove(&self, target: &str, force: bool, force_delete: bool) -> Result<()> {
        let mut args: Vec<&str> = vec!["remove", target, "--foreground", "--yes"];
        if force {
            args.push("--force");
        }
        if force_delete {
            args.push("--force-delete");
        }
        self.run_wt(&args)?;
        Ok(())
    }

    // ── Config operations ────────────────────────────────────────────

    /// Get all values for a multi-valued config key.
    pub fn config_get_all(&self, key: &str) -> Result<Vec<String>> {
        match self.run(&["config", "--get-all", key]) {
            Ok(out) => Ok(out
                .lines()
                .filter(|l| !l.is_empty())
                .map(|l| l.to_string())
                .collect()),
            Err(e) if is_unset_key(&e) => Ok(vec![]),
            Err(e) => Err(e),
        }
    }

    /// Get a single config value.
    pub fn config_get(&self, key: &str) -> Result<Option<String>> {
        match self.run(&["config", "--get", key]) {
            Ok(val) if !val.is_empty() => Ok(Some(val)),
            Ok(_) => Ok(None),
            Err(e) if is_unset_key(&e) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Set a single-valued config key.
    ///
    /// Uses `--local` to ensure the value is written to the shared
    /// `.git/config` even when `extensions.worktreeConfig` is enabled
    /// (where the default write scope would target the per-worktree
    /// config file instead).
    pub fn config_set(&self, key: &str, value: &str) -> Result<()> {
        self.run(&["config", "--local", key, value])?;
        Ok(())
    }

    /// Add a value to a multi-valued config key.
    ///
    /// Uses `--local` to target the shared `.git/config`.
    /// See [`config_set`](Self::config_set) for rationale.
    pub fn config_add(&self, key: &str, value: &str) -> Result<()> {
        self.run(&["config", "--local", "--add", key, value])?;
        Ok(())
    }

    /// Remove all values for a config key.
    ///
    /// Uses `--local` to target the shared `.git/config`.
    /// See [`config_set`](Self::config_set) for rationale.
    pub fn config_unset_all(&self, key: &str) -> Result<()> {
        // --unset-all exits 5 if the key doesn't exist; that's fine.
        match self.run(&["config", "--local", "--unset-all", key]) {
            Ok(_) => Ok(()),
            Err(e) if is_unset_noop(&e) => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// Remove a single value from a multi-valued config key, preserving the
    /// order of the values that remain.
    ///
    /// `git config --unset` takes a value regex rather than a literal, so
    /// removing one entry safely means rewriting the whole key: read the
    /// values, drop the matching ones, clear the key and re-add the rest in
    /// their original order. Removing a value that is not present is a no-op.
    pub fn config_remove_value(&self, key: &str, value: &str) -> Result<()> {
        let mut values = self.config_get_all(key)?;
        let before = values.len();
        values.retain(|v| v != value);
        if values.len() == before {
            return Ok(());
        }

        self.config_unset_all(key)?;
        for remaining in &values {
            self.config_add(key, remaining)?;
        }
        Ok(())
    }

    /// Check whether a config section exists.
    pub fn config_section_exists(&self, section: &str) -> Result<bool> {
        match self.run(&["config", "--get-regexp", &format!("^{section}\\.")]) {
            Ok(out) => Ok(!out.is_empty()),
            Err(e) if is_unset_key(&e) => Ok(false),
            Err(e) => Err(e),
        }
    }

    // ── Per-branch flags ─────────────────────────────────────────────

    /// Return the names of branches whose `branch.<name>.<suffix>` key is set
    /// to `true` in git config.
    fn branch_flag_list(&self, suffix: &str) -> Result<Vec<String>> {
        let pattern = format!(r"^branch\..*\.{suffix}$");
        let dot_suffix = format!(".{suffix}");
        match self.run(&["config", "--get-regexp", &pattern]) {
            Ok(out) => {
                let mut branches = Vec::new();
                for line in out.lines().filter(|l| !l.is_empty()) {
                    // Each line: "branch.<name>.<suffix> true"
                    let mut parts = line.splitn(2, ' ');
                    if let (Some(key), Some(value)) = (parts.next(), parts.next())
                        && parse_git_bool(value) == Some(true)
                    {
                        // Extract branch name from "branch.<name>.<suffix>"
                        if let Some(name) = key
                            .strip_prefix("branch.")
                            .and_then(|s| s.strip_suffix(&dot_suffix))
                        {
                            branches.push(name.to_string());
                        }
                    }
                }
                Ok(branches)
            }
            Err(e) if is_unset_key(&e) => Ok(vec![]),
            Err(e) => Err(e),
        }
    }

    /// Set `branch.<name>.<suffix> = true`, or unset the key entirely.
    ///
    /// Uses `--local` to target the shared `.git/config`.
    /// See [`config_set`](Self::config_set) for rationale.
    fn set_branch_flag(&self, branch: &str, suffix: &str, enabled: bool) -> Result<()> {
        let key = format!("branch.{branch}.{suffix}");
        if enabled {
            self.run(&["config", "--local", &key, "true"])?;
        } else {
            // --unset exits 5 if the key doesn't exist; that's fine.
            match self.run(&["config", "--local", "--unset", &key]) {
                Ok(_) => {}
                Err(e) if is_unset_noop(&e) => {}
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }

    /// Return the names of branches that have
    /// `branch.<name>.wipe-protected = true` in git config.
    pub fn branch_protected_list(&self) -> Result<Vec<String>> {
        self.branch_flag_list("wipe-protected")
    }

    /// Set or unset per-branch protection for a given branch.
    pub fn set_branch_protected(&self, branch: &str, protected: bool) -> Result<()> {
        self.set_branch_flag(branch, "wipe-protected", protected)
    }

    /// Return the names of branches that have
    /// `branch.<name>.wipe-ignored = true` in git config.
    pub fn branch_ignored_list(&self) -> Result<Vec<String>> {
        self.branch_flag_list("wipe-ignored")
    }

    /// Set or unset the per-branch ignore flag for a given branch.
    pub fn set_branch_ignored(&self, branch: &str, ignored: bool) -> Result<()> {
        self.set_branch_flag(branch, "wipe-ignored", ignored)
    }
}

// ── Parsing helpers ──────────────────────────────────────────────────

/// Whether a glob pattern can be expressed as a git refspec pattern.
///
/// Git refspecs support at most one `*` and no other glob metacharacter.
/// Anything richer (`?`, character classes, alternates, `**`) is rejected so
/// the fetch falls back to transferring the ref; the globset matcher still
/// filters it out afterwards.
fn refspec_safe(pattern: &str) -> bool {
    !pattern.is_empty()
        && pattern.matches('*').count() <= 1
        && !pattern.contains(['?', '[', ']', '{', '}', '!', '^'])
}

/// A worktree entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Worktree {
    /// Absolute path of the worktree directory.
    pub path: PathBuf,
    /// Checked-out branch, or `None` for a detached HEAD.
    pub branch: Option<String>,
    /// Whether this entry is the bare repository itself.
    pub is_bare: bool,
    /// Whether the worktree is locked; locked worktrees are never removed.
    pub is_locked: bool,
    /// Reason recorded with the lock, when one was given.
    pub lock_reason: Option<String>,
}

/// Parse `git branch` output (with leading `*`, `+` and whitespace).
///
/// `*` marks the current branch and those entries are **excluded** from the
/// result. `+` marks branches checked out in other linked worktrees; only its
/// `+ ` prefix is stripped and the branch is kept.
fn parse_branch_list(output: &str) -> Vec<String> {
    output
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty() && !l.starts_with('*'))
        .map(|l| l.strip_prefix("+ ").unwrap_or(l).to_string())
        .collect()
}

/// Parse `git branch -r` output, keeping only the branches of `remote` and
/// returning them in short form (without the `<remote>/` prefix).
///
/// The `origin/HEAD -> origin/main` symref line is dropped: it is an alias, not
/// a branch, and must never be offered for comparison or deletion.
fn parse_remote_branch_list(output: &str, remote: &str) -> Vec<String> {
    let prefix = format!("{remote}/");
    output
        .lines()
        .map(str::trim)
        .filter(|l| l.starts_with(&prefix) && !l.contains("->"))
        .map(|l| l.strip_prefix(&prefix).unwrap_or(l).to_string())
        .collect()
}

/// Determine which local branches have a configured upstream that no longer
/// exists.
///
/// `heads` is the output of
/// `git for-each-ref --format=%(refname:short)%00%(upstream) refs/heads` and
/// `remotes` the output of `git for-each-ref --format=%(refname) refs/remotes`.
/// Branches without an upstream are ignored: absence of tracking information
/// carries no signal about whether the branch was merged.
fn parse_gone_upstreams(heads: &str, remotes: &str) -> Vec<String> {
    let existing: HashSet<&str> = remotes
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();

    heads
        .lines()
        .filter_map(|line| {
            let (branch, upstream) = line.split_once('\0')?;
            if branch.is_empty() || upstream.is_empty() || existing.contains(upstream) {
                return None;
            }
            Some(branch.to_string())
        })
        .collect()
}

/// Parse `%(refname:short)\0%(committerdate:unix)` lines into a name → epoch map.
///
/// Lines without a separator, with an empty name, or with an unparsable date are
/// dropped: "unknown age" is a valid outcome for the status listing, and is far
/// better than inventing an epoch of zero and calling the branch 56 years old.
fn parse_committer_dates(output: &str) -> HashMap<String, u64> {
    output
        .lines()
        .filter_map(|line| {
            let (name, date) = line.split_once('\0')?;
            let name = name.trim();
            if name.is_empty() {
                return None;
            }
            Some((name.to_string(), date.trim().parse().ok()?))
        })
        .collect()
}

/// Parse `<refname>\0<sha>` lines into a map keyed by `refname` minus `prefix`.
fn parse_ref_tips(output: &str, prefix: &str) -> HashMap<String, String> {
    output
        .lines()
        .filter_map(|line| {
            let (name, sha) = line.split_once('\0')?;
            let name = name.trim().strip_prefix(prefix)?;
            let sha = sha.trim();
            if name.is_empty() || name == "HEAD" || sha.is_empty() {
                return None;
            }
            Some((name.to_string(), sha.to_string()))
        })
        .collect()
}

/// Parse `git worktree list --porcelain` output.
fn parse_worktree_list(output: &str) -> Vec<Worktree> {
    let mut worktrees = Vec::new();
    let mut current: Option<Worktree> = None;

    for line in output.lines() {
        if let Some(path) = line.strip_prefix("worktree ") {
            if let Some(wt) = current.take() {
                worktrees.push(wt);
            }
            current = Some(Worktree {
                path: PathBuf::from(path),
                branch: None,
                is_bare: false,
                is_locked: false,
                lock_reason: None,
            });
        } else if let Some(branch) = line.strip_prefix("branch ") {
            if let Some(ref mut wt) = current {
                // Strip refs/heads/ prefix
                wt.branch = Some(
                    branch
                        .strip_prefix("refs/heads/")
                        .unwrap_or(branch)
                        .to_string(),
                );
            }
        } else if line == "bare"
            && let Some(ref mut wt) = current
        {
            wt.is_bare = true;
        } else if line == "locked"
            && let Some(ref mut wt) = current
        {
            wt.is_locked = true;
        } else if let Some(reason) = line.strip_prefix("locked ")
            && let Some(ref mut wt) = current
        {
            wt.is_locked = true;
            wt.lock_reason = Some(reason.to_string());
        }
    }

    if let Some(wt) = current {
        worktrees.push(wt);
    }

    worktrees
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::{
        advance_remote, init_repo_with_local_remote, init_repo_with_worktree_config,
    };

    #[test]
    fn config_readers_propagate_failures_that_are_not_an_unset_key() -> Result<()> {
        let (_dir, git) = crate::test_helpers::init_repo()?;

        // An invalid key pattern makes `git config --get-regexp` exit 6, not
        // the exit 1 that means "nothing matched". It must not be reported as
        // "the section does not exist".
        assert!(
            git.config_section_exists("[").is_err(),
            "a malformed pattern must surface as an error"
        );
        Ok(())
    }

    #[test]
    fn config_readers_treat_an_unset_key_as_empty() -> Result<()> {
        let (_dir, git) = crate::test_helpers::init_repo()?;

        assert_eq!(git.config_get("wipe.never-set")?, None);
        assert_eq!(git.config_get_all("wipe.never-set")?, Vec::<String>::new());
        assert!(!git.config_section_exists("never-set")?);
        Ok(())
    }

    #[test]
    fn config_remove_value_preserves_the_order_of_the_survivors() -> Result<()> {
        let (_dir, git) = crate::test_helpers::init_repo()?;
        let key = "wipe.protected";

        for value in ["main", "develop", "release", "staging"] {
            git.config_add(key, value)?;
        }

        git.config_remove_value(key, "release")?;

        assert_eq!(
            git.config_get_all(key)?,
            vec!["main", "develop", "staging"],
            "the remaining values keep their original order"
        );
        Ok(())
    }

    #[test]
    fn config_remove_value_is_a_noop_for_an_absent_value() -> Result<()> {
        let (_dir, git) = crate::test_helpers::init_repo()?;
        let key = "wipe.protected";

        git.config_add(key, "main")?;
        git.config_remove_value(key, "never-added")?;

        assert_eq!(git.config_get_all(key)?, vec!["main"]);
        Ok(())
    }

    #[test]
    fn is_inside_work_tree_distinguishes_repos_from_plain_directories() -> Result<()> {
        let (dir, git) = crate::test_helpers::init_repo()?;
        assert!(git.is_inside_work_tree()?, "a git repo is a work tree");

        let plain = tempfile::tempdir()?;
        let outside = Git::with_workdir(false, plain.path());
        assert!(
            !outside.is_inside_work_tree()?,
            "a plain directory is not a work tree"
        );

        drop(dir);
        Ok(())
    }

    #[test]
    fn merge_base_returns_none_for_unrelated_histories() -> Result<()> {
        let (dir, git) = crate::test_helpers::init_repo()?;
        let path = dir.path();

        // An orphan branch shares no history with the initial commit.
        Command::new("git")
            .args(["checkout", "--orphan", "unrelated"])
            .current_dir(path)
            .output()?;
        std::fs::write(path.join("other.txt"), "other")?;
        Command::new("git")
            .args(["add", "."])
            .current_dir(path)
            .output()?;
        Command::new("git")
            .args(["commit", "-m", "unrelated root"])
            .current_dir(path)
            .output()?;

        assert!(
            git.merge_base("main", "unrelated")?.is_none(),
            "unrelated histories have no merge base"
        );
        Ok(())
    }

    #[test]
    fn merge_base_propagates_a_bad_ref_instead_of_reporting_no_match() -> Result<()> {
        let (_dir, git) = crate::test_helpers::init_repo()?;

        // A non-existent ref makes git exit 128, which must not be conflated
        // with the exit-1 "no merge base" signal.
        assert!(
            git.merge_base("main", "does-not-exist").is_err(),
            "a bad ref must surface as an error"
        );
        Ok(())
    }

    #[test]
    fn classify_network_signatures() {
        let cases = [
            "ssh: connect to host github.com port 22: No route to host\nfatal: Could not read from remote repository.",
            "ssh: Could not resolve hostname github.com: Temporary failure in name resolution",
            "fatal: unable to access 'https://github.com/foo/bar.git/': Failed to connect to github.com port 443: Connection timed out",
            "Connection refused",
            "Network is unreachable",
        ];
        for stderr in cases {
            assert_eq!(
                classify_git_stderr(stderr),
                GitErrorKind::Network,
                "expected Network for: {stderr}"
            );
        }
    }

    #[test]
    fn classify_auth_signatures() {
        let cases = [
            "git@github.com: Permission denied (publickey).\nfatal: Could not read from remote repository.",
            "remote: HTTP Basic: Access denied\nfatal: Authentication failed for 'https://example.com/foo.git/'",
        ];
        for stderr in cases {
            assert_eq!(
                classify_git_stderr(stderr),
                GitErrorKind::Auth,
                "expected Auth for: {stderr}"
            );
        }
    }

    #[test]
    fn classify_other() {
        let cases = [
            "",
            "fatal: ambiguous argument 'HEAD~10': unknown revision",
            "error: failed to push some refs",
        ];
        for stderr in cases {
            assert_eq!(
                classify_git_stderr(stderr),
                GitErrorKind::Other,
                "expected Other for: {stderr}"
            );
        }
    }

    #[test]
    fn git_command_error_display_has_no_double_exit() {
        let err = GitCommandError {
            program: "git".into(),
            args: vec!["fetch".into(), "origin".into()],
            exit_code: Some(1),
            stderr: "boom".into(),
            kind: GitErrorKind::Other,
        };
        let s = err.to_string();
        assert!(s.contains("(exit status: 1)"), "got: {s}");
        assert!(!s.contains("exit exit"), "got: {s}");
    }

    #[test]
    fn git_command_error_short_cause_skips_blank_lines() {
        let err = GitCommandError {
            program: "git".into(),
            args: vec!["fetch".into()],
            exit_code: Some(1),
            stderr: "\n   \nssh: connect to host github.com port 22: No route to host\nfatal: …"
                .into(),
            kind: GitErrorKind::Network,
        };
        assert_eq!(
            err.short_cause(),
            "ssh: connect to host github.com port 22: No route to host"
        );
    }

    #[test]
    fn run_git_failure_returns_command_error() {
        // Run `git` against a non-existent option to provoke a guaranteed
        // non-zero exit without depending on a workdir.
        let err = run_git(&["--definitely-not-a-real-flag"], false, None).unwrap_err();
        let gerr = err
            .downcast_ref::<GitCommandError>()
            .expect("expected GitCommandError");
        assert_eq!(gerr.program, "git");
        assert!(!gerr.stderr.is_empty());
    }

    #[test]
    fn parse_branch_list_excludes_the_current_branch() {
        let output = "  feature/foo\n* main\n  bugfix/bar\n";
        let branches = parse_branch_list(output);
        assert_eq!(branches, vec!["feature/foo", "bugfix/bar"]);
    }

    #[test]
    fn parse_branch_list_strips_worktree_marker() {
        let output = "  feature/foo\n* main\n+ feature/wt\n  bugfix/bar\n";
        let branches = parse_branch_list(output);
        assert_eq!(branches, vec!["feature/foo", "feature/wt", "bugfix/bar"]);
    }

    #[test]
    fn parse_branch_list_empty() {
        let branches = parse_branch_list("");
        assert!(branches.is_empty());
    }

    #[test]
    fn parse_gone_upstreams_finds_branches_whose_remote_ref_vanished() {
        let heads = "\
main\0refs/remotes/origin/main
feature/gone\0refs/remotes/origin/feature/gone
feature/alive\0refs/remotes/origin/feature/alive
local-only\0";
        let remotes = "refs/remotes/origin/main\nrefs/remotes/origin/feature/alive\n";
        assert_eq!(
            parse_gone_upstreams(heads, remotes),
            vec!["feature/gone".to_string()]
        );
    }

    #[test]
    fn parse_committer_dates_reads_name_and_epoch() {
        let out = "main\x001700000000\nfeature/x\x001600000000\n";
        let dates = parse_committer_dates(out);
        assert_eq!(dates.len(), 2);
        assert_eq!(dates["main"], 1_700_000_000);
        assert_eq!(dates["feature/x"], 1_600_000_000);
    }

    #[test]
    fn parse_committer_dates_skips_malformed_lines() {
        // No separator, empty name, unparsable date, and an empty line.
        let out = "no-separator\n\x001700000000\nbroken\x00soon\n\nok\x0042\n";
        let dates = parse_committer_dates(out);
        assert_eq!(dates.len(), 1);
        assert_eq!(dates["ok"], 42);
    }

    #[test]
    fn parse_committer_dates_of_empty_output_is_empty() {
        assert!(parse_committer_dates("").is_empty());
    }

    #[test]
    fn branch_committer_dates_covers_every_local_branch() -> Result<()> {
        let (_dir, git) = crate::test_helpers::init_repo_with_branches()?;
        let dates = git.branch_committer_dates()?;
        let branches = git.local_branches()?;
        assert!(!branches.is_empty());
        for branch in &branches {
            let date = dates
                .get(branch)
                .unwrap_or_else(|| panic!("{branch} must have a committer date"));
            // The fixture commits just now; allow generous clock slack.
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs();
            assert!(
                now.abs_diff(*date) < 3600,
                "{branch} dated {date}, now is {now}"
            );
        }
        assert_eq!(dates.len(), branches.len());
        Ok(())
    }

    #[test]
    fn parse_gone_upstreams_ignores_branches_without_upstream() {
        let heads = "solo\0\nother\0";
        assert_eq!(parse_gone_upstreams(heads, ""), Vec::<String>::new());
    }

    #[test]
    fn parse_gone_upstreams_all_gone_when_no_remote_refs() {
        let heads = "a\0refs/remotes/origin/a\nb\0refs/remotes/origin/b";
        assert_eq!(
            parse_gone_upstreams(heads, ""),
            vec!["a".to_string(), "b".to_string()]
        );
    }

    #[test]
    fn parse_gone_upstreams_handles_slashed_names_and_empty_input() {
        let heads = "feat/a/b/c\0refs/remotes/upstream/feat/a/b/c";
        assert_eq!(
            parse_gone_upstreams(heads, "refs/remotes/origin/feat/a/b/c"),
            vec!["feat/a/b/c".to_string()]
        );
        assert!(parse_gone_upstreams("", "").is_empty());
    }

    #[test]
    fn parse_worktree_list_reads_the_porcelain_format() {
        let output = "\
worktree /home/user/project
HEAD abc1234
branch refs/heads/main

worktree /home/user/project-feature
HEAD def5678
branch refs/heads/feature/foo

worktree /home/user/project-bare
HEAD 000000
bare
";
        let worktrees = parse_worktree_list(output);
        assert_eq!(worktrees.len(), 3);

        assert_eq!(worktrees[0].path, PathBuf::from("/home/user/project"));
        assert_eq!(worktrees[0].branch.as_deref(), Some("main"));
        assert!(!worktrees[0].is_bare);
        assert!(!worktrees[0].is_locked);

        assert_eq!(
            worktrees[1].path,
            PathBuf::from("/home/user/project-feature")
        );
        assert_eq!(worktrees[1].branch.as_deref(), Some("feature/foo"));
        assert!(!worktrees[1].is_locked);

        assert_eq!(worktrees[2].path, PathBuf::from("/home/user/project-bare"));
        assert!(worktrees[2].is_bare);
        assert!(!worktrees[2].is_locked);
    }

    #[test]
    fn parse_worktree_list_empty() {
        let worktrees = parse_worktree_list("");
        assert!(worktrees.is_empty());
    }

    #[test]
    fn parse_worktree_list_locked_no_reason() {
        let output = "\
worktree /home/user/project
HEAD abc1234
branch refs/heads/main

worktree /home/user/project-feature
HEAD def5678
branch refs/heads/feature/foo
locked

";
        let worktrees = parse_worktree_list(output);
        assert_eq!(worktrees.len(), 2);

        assert!(!worktrees[0].is_locked);
        assert!(worktrees[0].lock_reason.is_none());

        assert!(worktrees[1].is_locked);
        assert!(worktrees[1].lock_reason.is_none());
    }

    #[test]
    fn parse_worktree_list_locked_with_reason() {
        let output = "\
worktree /home/user/project
HEAD abc1234
branch refs/heads/main

worktree /home/user/project-feature
HEAD def5678
branch refs/heads/feature/foo
locked work in progress, do not remove

";
        let worktrees = parse_worktree_list(output);
        assert_eq!(worktrees.len(), 2);

        assert!(!worktrees[0].is_locked);

        assert!(worktrees[1].is_locked);
        assert_eq!(
            worktrees[1].lock_reason.as_deref(),
            Some("work in progress, do not remove")
        );
    }

    /// Integration test: verify basic git operations in a temporary repo.
    #[test]
    fn git_in_temp_repo() -> Result<()> {
        let (dir, git) = crate::test_helpers::init_repo()?;
        let path = dir.path();

        // Test current branch
        assert_eq!(git.current_branch()?, "main");

        // Test local branches
        let branches = git.local_branches()?;
        assert_eq!(branches, vec!["main"]);

        // Create a feature branch and merge it
        Command::new("git")
            .args(["checkout", "-b", "feature/test"])
            .current_dir(path)
            .output()?;
        std::fs::write(path.join("feature.txt"), "feature")?;
        Command::new("git")
            .args(["add", "."])
            .current_dir(path)
            .output()?;
        Command::new("git")
            .args(["commit", "-m", "feature"])
            .current_dir(path)
            .output()?;
        Command::new("git")
            .args(["checkout", "main"])
            .current_dir(path)
            .output()?;
        Command::new("git")
            .args(["merge", "feature/test"])
            .current_dir(path)
            .output()?;

        // The feature branch should show up as merged
        let merged = git.merged_branches("main")?;
        assert!(merged.contains(&"feature/test".to_string()));

        // Config operations
        git.config_add("wipe.protected", "main")?;
        git.config_add("wipe.protected", "release/*")?;
        let protected = git.config_get_all("wipe.protected")?;
        assert_eq!(protected, vec!["main", "release/*"]);

        assert!(git.config_section_exists("wipe")?);
        assert!(!git.config_section_exists("nonexistent")?);

        Ok(())
    }

    #[test]
    fn branch_delete_removes_a_merged_branch() -> Result<()> {
        let (dir, git) = crate::test_helpers::init_repo()?;
        let path = dir.path();

        // Create and merge a branch
        Command::new("git")
            .args(["checkout", "-b", "feature/to-delete"])
            .current_dir(path)
            .output()?;
        std::fs::write(path.join("f.txt"), "feature")?;
        Command::new("git")
            .args(["add", "."])
            .current_dir(path)
            .output()?;
        Command::new("git")
            .args(["commit", "-m", "feature"])
            .current_dir(path)
            .output()?;
        Command::new("git")
            .args(["checkout", "main"])
            .current_dir(path)
            .output()?;
        Command::new("git")
            .args(["merge", "feature/to-delete"])
            .current_dir(path)
            .output()?;

        let branches = git.local_branches()?;
        assert!(branches.contains(&"feature/to-delete".to_string()));

        git.branch_delete("feature/to-delete")?;

        let branches = git.local_branches()?;
        assert!(!branches.contains(&"feature/to-delete".to_string()));

        Ok(())
    }

    #[test]
    fn remotes_empty() -> Result<()> {
        let (_dir, git) = crate::test_helpers::init_repo()?;

        let remotes = git.remotes()?;
        assert!(remotes.is_empty());

        Ok(())
    }

    #[test]
    fn cherry_merged_detects_a_rebase_merged_branch() -> Result<()> {
        let (dir, git) = crate::test_helpers::init_repo()?;
        let path = dir.path();

        // Create a feature branch
        Command::new("git")
            .args(["checkout", "-b", "feature/cherry-test"])
            .current_dir(path)
            .output()?;
        std::fs::write(path.join("cherry.txt"), "cherry content")?;
        Command::new("git")
            .args(["add", "."])
            .current_dir(path)
            .output()?;
        Command::new("git")
            .args(["commit", "-m", "cherry commit"])
            .current_dir(path)
            .output()?;
        let sha_output = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(path)
            .output()?;
        let sha = String::from_utf8_lossy(&sha_output.stdout)
            .trim()
            .to_string();

        // Add a diverging commit on main so cherry-pick creates a different SHA
        Command::new("git")
            .args(["checkout", "main"])
            .current_dir(path)
            .output()?;
        std::fs::write(path.join("diverge.txt"), "diverge")?;
        Command::new("git")
            .args(["add", "."])
            .current_dir(path)
            .output()?;
        Command::new("git")
            .args(["commit", "-m", "diverge"])
            .current_dir(path)
            .output()?;

        // Cherry-pick the feature commit onto the now-diverged main
        Command::new("git")
            .args(["cherry-pick", &sha])
            .current_dir(path)
            .output()?;

        // The branch's commit was cherry-picked, so cherry_merged should be true
        assert!(git.cherry_merged("main", "feature/cherry-test")?);

        // Create an unmerged branch
        Command::new("git")
            .args(["checkout", "-b", "feature/not-cherry"])
            .current_dir(path)
            .output()?;
        std::fs::write(path.join("not-cherry.txt"), "not cherry")?;
        Command::new("git")
            .args(["add", "."])
            .current_dir(path)
            .output()?;
        Command::new("git")
            .args(["commit", "-m", "not cherry-picked"])
            .current_dir(path)
            .output()?;

        // This branch's commit was NOT cherry-picked, so cherry_merged should be false
        assert!(!git.cherry_merged("main", "feature/not-cherry")?);

        Ok(())
    }

    #[test]
    fn diff_empty_detects_a_branch_that_nets_out_to_no_change() -> Result<()> {
        let (dir, git) = crate::test_helpers::init_repo()?;
        let path = dir.path();

        // A branch that adds a file and then reverts it: its commits net out to
        // no content change relative to the fork point.
        Command::new("git")
            .args(["checkout", "-b", "feature/no-op"])
            .current_dir(path)
            .output()?;
        std::fs::write(path.join("temp.txt"), "temporary")?;
        Command::new("git")
            .args(["add", "."])
            .current_dir(path)
            .output()?;
        Command::new("git")
            .args(["commit", "-m", "add temp"])
            .current_dir(path)
            .output()?;
        std::fs::remove_file(path.join("temp.txt"))?;
        Command::new("git")
            .args(["add", "-A"])
            .current_dir(path)
            .output()?;
        Command::new("git")
            .args(["commit", "-m", "remove temp"])
            .current_dir(path)
            .output()?;

        // Advance main with an unrelated change so the two trees differ. A
        // two-dot diff would be non-empty here; the three-dot diff must still
        // report that the branch itself contributes nothing.
        Command::new("git")
            .args(["checkout", "main"])
            .current_dir(path)
            .output()?;
        std::fs::write(path.join("main.txt"), "main content")?;
        Command::new("git")
            .args(["add", "."])
            .current_dir(path)
            .output()?;
        Command::new("git")
            .args(["commit", "-m", "unrelated main work"])
            .current_dir(path)
            .output()?;

        assert!(git.diff_empty("main", "feature/no-op")?);

        // Create an unmerged branch — the three-dot diff must NOT be empty.
        Command::new("git")
            .args(["checkout", "-b", "feature/unmerged"])
            .current_dir(path)
            .output()?;
        std::fs::write(path.join("unmerged.txt"), "unmerged content")?;
        Command::new("git")
            .args(["add", "."])
            .current_dir(path)
            .output()?;
        Command::new("git")
            .args(["commit", "-m", "unmerged commit"])
            .current_dir(path)
            .output()?;

        assert!(!git.diff_empty("main", "feature/unmerged")?);

        Ok(())
    }

    /// Regression test for the two-dot → three-dot fix.
    ///
    /// A squash-merged branch is *not* the responsibility of `diff_empty`: the
    /// merge base predates the squash commit, so the three-dot diff still shows
    /// the branch's changes. This case is covered by `squash_patch_id_match` /
    /// `merge_adds_nothing` instead.
    #[test]
    fn diff_empty_does_not_claim_squash_merges() -> Result<()> {
        let (dir, git) = crate::test_helpers::init_repo()?;
        let path = dir.path();

        Command::new("git")
            .args(["checkout", "-b", "feature/squash-test"])
            .current_dir(path)
            .output()?;
        std::fs::write(path.join("squash.txt"), "squash content")?;
        Command::new("git")
            .args(["add", "."])
            .current_dir(path)
            .output()?;
        Command::new("git")
            .args(["commit", "-m", "squash commit"])
            .current_dir(path)
            .output()?;

        Command::new("git")
            .args(["checkout", "main"])
            .current_dir(path)
            .output()?;
        Command::new("git")
            .args(["merge", "--squash", "feature/squash-test"])
            .current_dir(path)
            .output()?;
        Command::new("git")
            .args(["commit", "-m", "squash merge feature"])
            .current_dir(path)
            .output()?;

        // diff_empty stays silent…
        assert!(!git.diff_empty("main", "feature/squash-test")?);
        // …but the branch is still detected as merged by the dedicated strategies.
        assert!(git.squash_patch_id_match("main", "feature/squash-test")?);

        Ok(())
    }

    #[test]
    fn trees_match_detects_an_identical_tree() -> Result<()> {
        let (dir, git) = crate::test_helpers::init_repo()?;
        let path = dir.path();

        // Create a feature branch with a commit
        Command::new("git")
            .args(["checkout", "-b", "feature/squash-test"])
            .current_dir(path)
            .output()?;
        std::fs::write(path.join("squash.txt"), "squash content")?;
        Command::new("git")
            .args(["add", "."])
            .current_dir(path)
            .output()?;
        Command::new("git")
            .args(["commit", "-m", "squash commit"])
            .current_dir(path)
            .output()?;

        // Switch back to main and squash-merge the feature branch
        Command::new("git")
            .args(["checkout", "main"])
            .current_dir(path)
            .output()?;
        Command::new("git")
            .args(["merge", "--squash", "feature/squash-test"])
            .current_dir(path)
            .output()?;
        Command::new("git")
            .args(["commit", "-m", "squash merge feature"])
            .current_dir(path)
            .output()?;

        // After squash-merge, main and the branch have the same tree
        assert!(git.trees_match("main", "feature/squash-test")?);

        // Create an unmerged branch — trees should NOT match
        Command::new("git")
            .args(["checkout", "-b", "feature/unmerged"])
            .current_dir(path)
            .output()?;
        std::fs::write(path.join("unmerged.txt"), "unmerged content")?;
        Command::new("git")
            .args(["add", "."])
            .current_dir(path)
            .output()?;
        Command::new("git")
            .args(["commit", "-m", "unmerged commit"])
            .current_dir(path)
            .output()?;

        assert!(!git.trees_match("main", "feature/unmerged")?);

        Ok(())
    }

    #[test]
    fn merge_adds_nothing_detects_a_fully_contained_branch() -> Result<()> {
        let (dir, git) = crate::test_helpers::init_repo()?;
        let path = dir.path();

        // Create a feature branch that touches a.txt
        Command::new("git")
            .args(["checkout", "-b", "feature/squash"])
            .current_dir(path)
            .output()?;
        std::fs::write(path.join("a.txt"), "feature content")?;
        Command::new("git")
            .args(["add", "."])
            .current_dir(path)
            .output()?;
        Command::new("git")
            .args(["commit", "-m", "feature: add a.txt"])
            .current_dir(path)
            .output()?;

        // Squash-merge into main
        Command::new("git")
            .args(["checkout", "main"])
            .current_dir(path)
            .output()?;
        Command::new("git")
            .args(["merge", "--squash", "feature/squash"])
            .current_dir(path)
            .output()?;
        Command::new("git")
            .args(["add", "."])
            .current_dir(path)
            .output()?;
        Command::new("git")
            .args(["commit", "-m", "squash merge feature/squash"])
            .current_dir(path)
            .output()?;

        // Advance main with an unrelated commit touching a different file —
        // this defeats trees_match and diff_empty.
        std::fs::write(path.join("b.txt"), "unrelated change")?;
        Command::new("git")
            .args(["add", "."])
            .current_dir(path)
            .output()?;
        Command::new("git")
            .args(["commit", "-m", "main: unrelated b.txt"])
            .current_dir(path)
            .output()?;

        // Sanity check: the cheaper detectors no longer fire.
        assert!(!git.trees_match("main", "feature/squash")?);
        assert!(!git.diff_empty("main", "feature/squash")?);

        // Simulated merge still catches it: merging feature/squash into main
        // produces main's current tree (the squashed changes are already
        // present, the unrelated b.txt is preserved).
        assert!(git.merge_adds_nothing("main", "feature/squash")?);

        // Negative case: a branch with real new content should not be detected.
        Command::new("git")
            .args(["checkout", "-b", "feature/divergent"])
            .current_dir(path)
            .output()?;
        std::fs::write(path.join("c.txt"), "new content")?;
        Command::new("git")
            .args(["add", "."])
            .current_dir(path)
            .output()?;
        Command::new("git")
            .args(["commit", "-m", "feature: c.txt"])
            .current_dir(path)
            .output()?;
        Command::new("git")
            .args(["checkout", "main"])
            .current_dir(path)
            .output()?;

        assert!(!git.merge_adds_nothing("main", "feature/divergent")?);

        Ok(())
    }

    #[test]
    fn squash_patch_id_match_detects_a_squash_merge() -> Result<()> {
        let (dir, git) = crate::test_helpers::init_repo()?;
        let path = dir.path();

        // Feature branch with TWO commits — combined diff = "line1\nline2\n"
        // on a.txt. This is the canonical case `patch_id_match` per-commit
        // cannot resolve: 2 branch patch-ids ≠ 1 squash patch-id on target.
        Command::new("git")
            .args(["checkout", "-b", "feature/multi-squash"])
            .current_dir(path)
            .output()?;
        std::fs::write(path.join("a.txt"), "line1\n")?;
        Command::new("git")
            .args(["add", "."])
            .current_dir(path)
            .output()?;
        Command::new("git")
            .args(["commit", "-m", "feature: line1"])
            .current_dir(path)
            .output()?;
        std::fs::write(path.join("a.txt"), "line1\nline2\n")?;
        Command::new("git")
            .args(["add", "."])
            .current_dir(path)
            .output()?;
        Command::new("git")
            .args(["commit", "-m", "feature: line2"])
            .current_dir(path)
            .output()?;

        // Squash-merge into main as a single commit, then advance main with
        // an unrelated commit (defeats trees_match / diff_empty).
        Command::new("git")
            .args(["checkout", "main"])
            .current_dir(path)
            .output()?;
        Command::new("git")
            .args(["merge", "--squash", "feature/multi-squash"])
            .current_dir(path)
            .output()?;
        Command::new("git")
            .args(["commit", "-m", "squash merge feature/multi-squash"])
            .current_dir(path)
            .output()?;
        std::fs::write(path.join("b.txt"), "unrelated\n")?;
        Command::new("git")
            .args(["add", "."])
            .current_dir(path)
            .output()?;
        Command::new("git")
            .args(["commit", "-m", "main: unrelated b.txt"])
            .current_dir(path)
            .output()?;

        // Sanity: textual detectors that compare per-commit fail here.
        // (Two branch patch-ids cannot all be found among target's
        // single squash patch-id.)
        assert!(
            !git.patch_id_match("main", "feature/multi-squash")?,
            "per-commit patch_id_match cannot relate 2 branch commits to 1 squash commit"
        );

        // The new strategy resolves the gap.
        assert!(
            git.squash_patch_id_match("main", "feature/multi-squash")?,
            "combined branch patch-id must match the single squash commit on main"
        );

        // Negative case: a branch with no matching squash commit on main.
        Command::new("git")
            .args(["checkout", "-b", "feature/divergent"])
            .current_dir(path)
            .output()?;
        std::fs::write(path.join("d.txt"), "divergent\n")?;
        Command::new("git")
            .args(["add", "."])
            .current_dir(path)
            .output()?;
        Command::new("git")
            .args(["commit", "-m", "feature: d.txt"])
            .current_dir(path)
            .output()?;
        Command::new("git")
            .args(["checkout", "main"])
            .current_dir(path)
            .output()?;

        assert!(!git.squash_patch_id_match("main", "feature/divergent")?);

        // Empty-diff branch (no commits beyond merge-base) is trivially merged.
        Command::new("git")
            .args(["branch", "feature/empty", "main"])
            .current_dir(path)
            .output()?;
        assert!(git.squash_patch_id_match("main", "feature/empty")?);

        Ok(())
    }

    #[test]
    fn worktree_list_integration() -> Result<()> {
        let (dir, git) = crate::test_helpers::init_repo()?;
        let path = dir.path();

        // Create a branch and worktree
        Command::new("git")
            .args(["branch", "feature/wt"])
            .current_dir(path)
            .output()?;
        let wt_path = path.join("wt-dir");
        Command::new("git")
            .args(["worktree", "add", wt_path.to_str().unwrap(), "feature/wt"])
            .current_dir(path)
            .output()?;

        let worktrees = git.worktree_list()?;

        // Should have at least 2 worktrees: main repo + the added one
        assert!(worktrees.len() >= 2);

        let wt_branches: Vec<Option<&str>> =
            worktrees.iter().map(|wt| wt.branch.as_deref()).collect();
        assert!(wt_branches.contains(&Some("main")));
        assert!(wt_branches.contains(&Some("feature/wt")));

        Ok(())
    }

    #[test]
    fn worktree_git_dir_returns_the_admin_dir() -> Result<()> {
        let (_dir, git, wt_path) = crate::test_helpers::init_repo_with_worktree()?;

        let admin = git.worktree_git_dir(Path::new(&wt_path))?;

        assert!(admin.is_dir(), "{} should exist", admin.display());
        assert!(
            admin.parent().is_some_and(|p| p.ends_with("worktrees")),
            "expected an admin dir under .git/worktrees, got {}",
            admin.display()
        );
        Ok(())
    }

    #[test]
    fn branch_protected_list_empty() -> Result<()> {
        let (_dir, git) = crate::test_helpers::init_repo()?;

        let protected = git.branch_protected_list()?;
        assert!(protected.is_empty());

        Ok(())
    }

    #[test]
    fn branch_protected_list_returns_flagged_branches() -> Result<()> {
        let (dir, git) = crate::test_helpers::init_repo()?;
        let path = dir.path();

        // Mark two branches as protected via per-branch config
        Command::new("git")
            .args(["config", "branch.develop.wipe-protected", "true"])
            .current_dir(path)
            .output()?;
        Command::new("git")
            .args(["config", "branch.staging.wipe-protected", "true"])
            .current_dir(path)
            .output()?;

        let mut protected = git.branch_protected_list()?;
        protected.sort();
        assert_eq!(protected, vec!["develop", "staging"]);

        Ok(())
    }

    #[test]
    fn set_branch_protected_and_unset() -> Result<()> {
        let (_dir, git) = crate::test_helpers::init_repo()?;

        // Set protection
        git.set_branch_protected("develop", true)?;
        let protected = git.branch_protected_list()?;
        assert_eq!(protected, vec!["develop"]);

        // Unset protection
        git.set_branch_protected("develop", false)?;
        let protected = git.branch_protected_list()?;
        assert!(protected.is_empty());

        // Unsetting a non-existent key should not error
        git.set_branch_protected("nonexistent", false)?;

        Ok(())
    }

    // ── Worktree-config tests ────────────────────────────────────────

    #[test]
    fn config_set_from_linked_worktree_writes_to_shared_config() -> Result<()> {
        let (_dir, main_path, wt_path) = init_repo_with_worktree_config()?;

        // Write config from the linked worktree
        let git_wt = Git::with_workdir(false, &wt_path);
        git_wt.config_set("wipe.worktrunk", "true")?;

        // Read from the main worktree — must see the value
        let git_main = Git::with_workdir(false, &main_path);
        let val = git_main.config_get("wipe.worktrunk")?;
        assert_eq!(val.as_deref(), Some("true"));

        Ok(())
    }

    #[test]
    fn config_add_from_linked_worktree_writes_to_shared_config() -> Result<()> {
        let (_dir, main_path, wt_path) = init_repo_with_worktree_config()?;

        // Add config values from the linked worktree
        let git_wt = Git::with_workdir(false, &wt_path);
        git_wt.config_add("wipe.protected", "main")?;
        git_wt.config_add("wipe.protected", "release/*")?;

        // Read from the main worktree
        let git_main = Git::with_workdir(false, &main_path);
        let protected = git_main.config_get_all("wipe.protected")?;
        assert_eq!(protected, vec!["main", "release/*"]);

        Ok(())
    }

    #[test]
    fn config_unset_all_from_linked_worktree_clears_shared_config() -> Result<()> {
        let (_dir, main_path, wt_path) = init_repo_with_worktree_config()?;

        // Set some values from the main worktree
        let git_main = Git::with_workdir(false, &main_path);
        git_main.config_add("wipe.protected", "main")?;
        git_main.config_add("wipe.protected", "develop")?;

        // Unset from the linked worktree
        let git_wt = Git::with_workdir(false, &wt_path);
        git_wt.config_unset_all("wipe.protected")?;

        // Verify from the main worktree
        let protected = git_main.config_get_all("wipe.protected")?;
        assert!(protected.is_empty());

        Ok(())
    }

    #[test]
    fn config_unset_all_tolerates_a_missing_key() -> Result<()> {
        let (_dir, git) = crate::test_helpers::init_repo()?;

        git.config_unset_all("wipe.never-set")?;
        git.set_branch_protected("never-flagged", false)?;

        Ok(())
    }

    #[test]
    fn config_unset_all_reports_a_locked_config_file() -> Result<()> {
        let (dir, git) = crate::test_helpers::init_repo()?;
        git.config_add("wipe.protected", "main")?;

        // A stale lock file makes every config write fail.
        std::fs::write(dir.path().join(".git/config.lock"), "")?;

        assert!(git.config_unset_all("wipe.protected").is_err());
        assert!(git.set_branch_protected("main", false).is_err());

        Ok(())
    }

    #[test]
    fn set_branch_protected_from_linked_worktree() -> Result<()> {
        let (_dir, main_path, wt_path) = init_repo_with_worktree_config()?;

        // Set per-branch protection from the linked worktree
        let git_wt = Git::with_workdir(false, &wt_path);
        git_wt.set_branch_protected("develop", true)?;

        // Read from the main worktree
        let git_main = Git::with_workdir(false, &main_path);
        let protected = git_main.branch_protected_list()?;
        assert_eq!(protected, vec!["develop"]);

        // Unset from the linked worktree
        git_wt.set_branch_protected("develop", false)?;
        let protected = git_main.branch_protected_list()?;
        assert!(protected.is_empty());

        Ok(())
    }

    #[test]
    fn config_section_exists_across_worktrees() -> Result<()> {
        let (_dir, main_path, wt_path) = init_repo_with_worktree_config()?;

        // Write from linked worktree
        let git_wt = Git::with_workdir(false, &wt_path);
        git_wt.config_add("wipe.protected", "main")?;

        // Section should be visible from both worktrees
        assert!(git_wt.config_section_exists("wipe")?);
        let git_main = Git::with_workdir(false, &main_path);
        assert!(git_main.config_section_exists("wipe")?);

        Ok(())
    }

    // ── Pull / fast-forward tests ────────────────────────────────────

    #[test]
    fn branch_upstream_with_tracking() -> Result<()> {
        let (_dir, work_path, _bare_path) = init_repo_with_local_remote()?;
        let git = Git::with_workdir(false, &work_path);

        let upstream = git.branch_upstream("main")?;
        assert!(upstream.is_some());
        let (remote, branch) = upstream.unwrap();
        assert_eq!(remote, "origin");
        assert_eq!(branch, "main");

        Ok(())
    }

    #[test]
    fn branch_upstream_without_tracking() -> Result<()> {
        let (dir, _git) = crate::test_helpers::init_repo()?;
        let git = Git::with_workdir(false, dir.path());

        // Local-only repo — no upstream tracking
        let upstream = git.branch_upstream("main")?;
        assert!(upstream.is_none());

        Ok(())
    }

    #[test]
    fn pull_ff_only_fast_forwards_the_current_branch() -> Result<()> {
        let (dir, work_path, bare_path) = init_repo_with_local_remote()?;

        // Advance the remote with a new commit
        advance_remote(&bare_path, dir.path())?;

        // Fetch so we have the remote ref
        let git = Git::with_workdir(false, &work_path);
        git.fetch_remote_prune("origin", &[])?;

        // Record the commit before pulling
        let before = git.run(&["rev-parse", "HEAD"])?;

        // Pull should fast-forward
        git.pull_ff_only()?;

        let after = git.run(&["rev-parse", "HEAD"])?;
        assert_ne!(before, after, "HEAD should have advanced after pull");

        // The new file from the remote should exist
        assert!(work_path.join("new.txt").exists());

        Ok(())
    }

    #[test]
    fn pull_ff_only_in_fast_forwards_a_linked_worktree() -> Result<()> {
        let (dir, work_path, bare_path) = init_repo_with_local_remote()?;

        // Create a branch and a linked worktree
        let git = Git::with_workdir(false, &work_path);
        Command::new("git")
            .args(["checkout", "-b", "feature/wt-pull"])
            .current_dir(&work_path)
            .output()?;
        Command::new("git")
            .args(["push", "-u", "origin", "feature/wt-pull"])
            .current_dir(&work_path)
            .output()?;
        Command::new("git")
            .args(["checkout", "main"])
            .current_dir(&work_path)
            .output()?;

        let wt_path = dir.path().join("wt-linked");
        Command::new("git")
            .args([
                "worktree",
                "add",
                wt_path.to_str().unwrap(),
                "feature/wt-pull",
            ])
            .current_dir(&work_path)
            .output()?;

        // Advance the remote branch via a second clone
        let pusher = dir.path().join("pusher-wt");
        Command::new("git")
            .args([
                "clone",
                "-b",
                "feature/wt-pull",
                bare_path.to_str().unwrap(),
                pusher.to_str().unwrap(),
            ])
            .output()?;
        Command::new("git")
            .args(["config", "user.email", "test@test.com"])
            .current_dir(&pusher)
            .output()?;
        Command::new("git")
            .args(["config", "user.name", "Test"])
            .current_dir(&pusher)
            .output()?;
        std::fs::write(pusher.join("wt-new.txt"), "wt new")?;
        Command::new("git")
            .args(["add", "."])
            .current_dir(&pusher)
            .output()?;
        Command::new("git")
            .args(["commit", "-m", "advance wt branch"])
            .current_dir(&pusher)
            .output()?;
        Command::new("git")
            .args(["push"])
            .current_dir(&pusher)
            .output()?;

        // Pull in the linked worktree
        git.pull_ff_only_in(&wt_path)?;

        // The new file should exist in the worktree
        assert!(wt_path.join("wt-new.txt").exists());

        Ok(())
    }

    #[test]
    fn fetch_update_branch_advances_a_branch_without_checkout() -> Result<()> {
        let (dir, work_path, bare_path) = init_repo_with_local_remote()?;

        // Create a branch, push it, then check out main
        let git = Git::with_workdir(false, &work_path);
        Command::new("git")
            .args(["checkout", "-b", "feature/fetch-update"])
            .current_dir(&work_path)
            .output()?;
        Command::new("git")
            .args(["push", "-u", "origin", "feature/fetch-update"])
            .current_dir(&work_path)
            .output()?;
        Command::new("git")
            .args(["checkout", "main"])
            .current_dir(&work_path)
            .output()?;

        // Record the branch SHA before
        let before = git.run(&["rev-parse", "feature/fetch-update"])?;

        // Advance the remote branch via a second clone
        let pusher = dir.path().join("pusher-fetch");
        Command::new("git")
            .args([
                "clone",
                "-b",
                "feature/fetch-update",
                bare_path.to_str().unwrap(),
                pusher.to_str().unwrap(),
            ])
            .output()?;
        Command::new("git")
            .args(["config", "user.email", "test@test.com"])
            .current_dir(&pusher)
            .output()?;
        Command::new("git")
            .args(["config", "user.name", "Test"])
            .current_dir(&pusher)
            .output()?;
        std::fs::write(pusher.join("fetch-new.txt"), "fetch new")?;
        Command::new("git")
            .args(["add", "."])
            .current_dir(&pusher)
            .output()?;
        Command::new("git")
            .args(["commit", "-m", "advance fetch branch"])
            .current_dir(&pusher)
            .output()?;
        Command::new("git")
            .args(["push"])
            .current_dir(&pusher)
            .output()?;

        // Fast-forward the local branch without checkout
        git.fetch_update_branch("origin", "feature/fetch-update", "feature/fetch-update")?;

        let after = git.run(&["rev-parse", "feature/fetch-update"])?;
        assert_ne!(
            before, after,
            "branch ref should have advanced after fetch update"
        );

        Ok(())
    }

    #[test]
    fn worktree_dirty_detects_untracked() -> Result<()> {
        let (dir, git) = crate::test_helpers::init_repo()?;
        let path = dir.path();

        // Clean repo → not dirty
        assert!(!git.worktree_dirty(path)?);

        // Add an untracked file → dirty
        std::fs::write(path.join("untracked.txt"), "noise")?;
        assert!(git.worktree_dirty(path)?);

        Ok(())
    }

    #[test]
    fn worktree_dirty_detects_modified() -> Result<()> {
        let (dir, git) = crate::test_helpers::init_repo()?;
        let path = dir.path();

        // Commit a tracked file
        std::fs::write(path.join("file.txt"), "v1")?;
        Command::new("git")
            .args(["add", "file.txt"])
            .current_dir(path)
            .output()?;
        Command::new("git")
            .args(["commit", "-m", "add file"])
            .current_dir(path)
            .output()?;

        assert!(!git.worktree_dirty(path)?);

        // Modify it → dirty
        std::fs::write(path.join("file.txt"), "v2")?;
        assert!(git.worktree_dirty(path)?);

        Ok(())
    }

    #[test]
    fn worktree_unlock_clears_the_lock() -> Result<()> {
        let (_dir, git, _wt_path) = crate::test_helpers::init_repo_with_locked_worktree()?;

        // Match by suffix rather than exact path equality: on macOS the
        // fixture's tempdir path and git's own reported path can differ in
        // representation (e.g. `/private/var/...` vs `/var/...`).
        let before = git.worktree_list()?;
        let locked = before
            .iter()
            .find(|wt| wt.path.ends_with("worktree-locked"))
            .expect("fixture worktree must be listed");
        assert!(locked.is_locked, "fixture worktree should start locked");
        let path = locked.path.clone();

        git.worktree_unlock(&path)?;

        let after = git.worktree_list()?;
        assert!(
            after.iter().any(|wt| wt.path.ends_with("worktree-locked")
                && !wt.is_locked
                && wt.lock_reason.is_none()),
            "worktree should no longer be reported as locked"
        );

        Ok(())
    }

    #[test]
    fn branch_has_unmerged_commits_detects_commits_outside_every_target() -> Result<()> {
        let (dir, git) = crate::test_helpers::init_repo_with_branches()?;
        let path = dir.path();

        let targets = vec!["main".to_string()];

        // feature/done is merged into main → no unmerged commits
        assert!(!git.branch_has_unmerged_commits("feature/done", &targets)?);

        // feature/wip is NOT merged into main → has unmerged commits
        assert!(git.branch_has_unmerged_commits("feature/wip", &targets)?);

        // Empty targets → considered unmerged
        assert!(git.branch_has_unmerged_commits("feature/done", &[])?);

        // Make sure `path` is consumed so the temp dir lives long enough
        let _ = path;
        Ok(())
    }

    #[test]
    fn patch_id_match_detects_commits_reapplied_with_new_shas() -> Result<()> {
        let (dir, git) = crate::test_helpers::init_repo()?;
        let path = dir.path();

        // Create feature/patch with a commit
        Command::new("git")
            .args(["checkout", "-b", "feature/patch"])
            .current_dir(path)
            .output()?;
        std::fs::write(path.join("patch.txt"), "patch content\n")?;
        Command::new("git")
            .args(["add", "."])
            .current_dir(path)
            .output()?;
        Command::new("git")
            .args(["commit", "-m", "patch feature"])
            .current_dir(path)
            .output()?;
        let feature_sha = String::from_utf8_lossy(
            &Command::new("git")
                .args(["rev-parse", "HEAD"])
                .current_dir(path)
                .output()?
                .stdout,
        )
        .trim()
        .to_string();

        // Diverge main with an unrelated commit, then cherry-pick + amend so the
        // SHA differs but the patch-id (diff content) is identical.
        Command::new("git")
            .args(["checkout", "main"])
            .current_dir(path)
            .output()?;
        std::fs::write(path.join("diverge.txt"), "diverge\n")?;
        Command::new("git")
            .args(["add", "."])
            .current_dir(path)
            .output()?;
        Command::new("git")
            .args(["commit", "-m", "diverge"])
            .current_dir(path)
            .output()?;
        Command::new("git")
            .args(["cherry-pick", &feature_sha])
            .current_dir(path)
            .output()?;
        Command::new("git")
            .args(["commit", "--amend", "-m", "patch feature (reworded)"])
            .current_dir(path)
            .output()?;

        // Patch-id of the reworded commit on main matches feature/patch.
        assert!(git.patch_id_match("main", "feature/patch")?);

        // Unmerged branch: brand-new commit, no patch-id on main.
        Command::new("git")
            .args(["checkout", "-b", "feature/unmerged"])
            .current_dir(path)
            .output()?;
        std::fs::write(path.join("new.txt"), "new\n")?;
        Command::new("git")
            .args(["add", "."])
            .current_dir(path)
            .output()?;
        Command::new("git")
            .args(["commit", "-m", "unmerged"])
            .current_dir(path)
            .output()?;
        Command::new("git")
            .args(["checkout", "main"])
            .current_dir(path)
            .output()?;
        assert!(!git.patch_id_match("main", "feature/unmerged")?);

        // Empty branch range (branch tip == target tip) → trivially merged.
        Command::new("git")
            .args(["branch", "feature/empty", "main"])
            .current_dir(path)
            .output()?;
        assert!(git.patch_id_match("main", "feature/empty")?);

        // Unrelated histories → false, no bail.
        Command::new("git")
            .args(["checkout", "--orphan", "feature/orphan"])
            .current_dir(path)
            .output()?;
        Command::new("git")
            .args(["rm", "-rf", "."])
            .current_dir(path)
            .output()?;
        std::fs::write(path.join("orphan.txt"), "orphan\n")?;
        Command::new("git")
            .args(["add", "."])
            .current_dir(path)
            .output()?;
        Command::new("git")
            .args(["commit", "-m", "orphan root"])
            .current_dir(path)
            .output()?;
        assert!(!git.patch_id_match("main", "feature/orphan")?);

        Ok(())
    }

    // ── Ignored branches ─────────────────────────────────────────────

    #[test]
    fn refspec_safe_accepts_only_git_expressible_patterns() {
        assert!(refspec_safe("wip"));
        assert!(refspec_safe("wip/*"));
        assert!(refspec_safe("*-scratch"));
        assert!(!refspec_safe(""));
        assert!(!refspec_safe("wip/*/*"));
        assert!(!refspec_safe("wip/?"));
        assert!(!refspec_safe("wip/[ab]"));
        assert!(!refspec_safe("{wip,tmp}/*"));
    }

    #[test]
    fn branch_ignored_flag_roundtrip() -> Result<()> {
        let (_dir, git) = crate::test_helpers::init_repo_with_branches()?;

        assert!(git.branch_ignored_list()?.is_empty());
        git.set_branch_ignored("feature/wip", true)?;
        assert_eq!(git.branch_ignored_list()?, vec!["feature/wip".to_string()]);

        // The ignore flag is independent from the protection flag.
        assert!(git.branch_protected_list()?.is_empty());

        git.set_branch_ignored("feature/wip", false)?;
        assert!(git.branch_ignored_list()?.is_empty());
        Ok(())
    }

    #[test]
    fn fetch_remote_prune_skips_excluded_branches() -> Result<()> {
        let (_dir, work_path, bare_path) = init_repo_with_local_remote()?;

        // Push two branches from a second clone so the first one has to fetch
        // them.
        let other = _dir.path().join("other");
        Command::new("git")
            .args([
                "clone",
                bare_path.to_str().unwrap(),
                other.to_str().unwrap(),
            ])
            .output()?;
        for (branch, file) in [("feature/keep", "keep.txt"), ("wip/spike", "spike.txt")] {
            Command::new("git")
                .args(["checkout", "-b", branch, "main"])
                .current_dir(&other)
                .output()?;
            std::fs::write(other.join(file), "x")?;
            Command::new("git")
                .args(["-c", "user.email=t@t", "-c", "user.name=T", "add", "."])
                .current_dir(&other)
                .output()?;
            Command::new("git")
                .args([
                    "-c",
                    "user.email=t@t",
                    "-c",
                    "user.name=T",
                    "commit",
                    "-m",
                    branch,
                ])
                .current_dir(&other)
                .output()?;
            Command::new("git")
                .args(["push", "origin", branch])
                .current_dir(&other)
                .output()?;
        }

        let git = Git::with_workdir(false, &work_path);
        git.fetch_remote_prune("origin", &["wip/*".to_string()])?;

        let refs = git.run(&["for-each-ref", "--format=%(refname)", "refs/remotes"])?;
        assert!(
            refs.contains("refs/remotes/origin/feature/keep"),
            "non-ignored branch should be fetched, got {refs}"
        );
        assert!(
            !refs.contains("refs/remotes/origin/wip/spike"),
            "ignored branch should never be fetched, got {refs}"
        );
        Ok(())
    }

    #[test]
    fn fetch_remote_prune_removes_deleted_ignored_branches() -> Result<()> {
        let (_dir, work_path, bare_path) = init_repo_with_local_remote()?;

        // Publish `wip/spike` from a second clone, then fetch it plainly so the
        // work repo holds a tracking ref for it.
        let other = _dir.path().join("other");
        Command::new("git")
            .args([
                "clone",
                bare_path.to_str().unwrap(),
                other.to_str().unwrap(),
            ])
            .output()?;
        Command::new("git")
            .args(["checkout", "-b", "wip/spike", "main"])
            .current_dir(&other)
            .output()?;
        Command::new("git")
            .args(["push", "origin", "wip/spike"])
            .current_dir(&other)
            .output()?;

        let git = Git::with_workdir(false, &work_path);
        git.run(&["fetch", "origin"])?;
        let refs = git.run(&["for-each-ref", "--format=%(refname)", "refs/remotes"])?;
        assert!(
            refs.contains("refs/remotes/origin/wip/spike"),
            "precondition: tracking ref should exist, got {refs}"
        );

        // The branch disappears from the remote, then becomes ignored.
        Command::new("git")
            .args(["push", "origin", "--delete", "wip/spike"])
            .current_dir(&other)
            .output()?;

        let warning = git.fetch_remote_prune("origin", &["wip/*".to_string()])?;
        assert_eq!(warning, None, "a successful prune carries no warning");

        let refs = git.run(&["for-each-ref", "--format=%(refname)", "refs/remotes"])?;
        assert!(
            !refs.contains("refs/remotes/origin/wip/spike"),
            "stale tracking ref of an ignored branch should be pruned, got {refs}"
        );
        Ok(())
    }

    #[test]
    fn remote_branches_lists_short_names_and_skips_head() -> Result<()> {
        let (_dir, work_path, bare_path) = init_repo_with_local_remote()?;

        // Push a branch from a second clone so the first one has to fetch it.
        let other = _dir.path().join("other");
        Command::new("git")
            .args([
                "clone",
                bare_path.to_str().unwrap(),
                other.to_str().unwrap(),
            ])
            .output()?;
        Command::new("git")
            .args(["checkout", "-b", "feature/x", "main"])
            .current_dir(&other)
            .output()?;
        std::fs::write(other.join("x.txt"), "x")?;
        Command::new("git")
            .args(["-c", "user.email=t@t", "-c", "user.name=T", "add", "."])
            .current_dir(&other)
            .output()?;
        Command::new("git")
            .args([
                "-c",
                "user.email=t@t",
                "-c",
                "user.name=T",
                "commit",
                "-m",
                "x",
            ])
            .current_dir(&other)
            .output()?;
        Command::new("git")
            .args(["push", "origin", "feature/x"])
            .current_dir(&other)
            .output()?;

        let git = Git::with_workdir(false, &work_path);
        git.run(&["fetch", "origin"])?;
        // Materialise origin/HEAD so the symref line is actually exercised.
        git.run(&["remote", "set-head", "origin", "--auto"])?;

        let mut branches = git.remote_branches("origin")?;
        branches.sort();
        assert_eq!(
            branches,
            vec!["feature/x".to_string(), "main".to_string()],
            "short names only, no HEAD alias"
        );
        assert!(
            git.remote_branches("nope")?.is_empty(),
            "an unknown remote has no branches"
        );
        Ok(())
    }

    #[test]
    fn parse_remote_branch_list_strips_prefix_and_head() {
        let out =
            "  origin/HEAD -> origin/main\n  origin/main\n  origin/feature/x\n  upstream/main\n";
        assert_eq!(
            parse_remote_branch_list(out, "origin"),
            vec!["main".to_string(), "feature/x".to_string()]
        );
    }

    #[test]
    fn remote_url_applies_the_remote_configuration() -> Result<()> {
        let (dir, git) = crate::test_helpers::init_repo()?;
        crate::test_helpers::git_in(
            dir.path(),
            &["remote", "add", "origin", "git@github.com:o/r.git"],
        )?;
        assert_eq!(git.remote_url("origin")?, "git@github.com:o/r.git");
        assert!(git.remote_url("nope").is_err());
        Ok(())
    }

    #[test]
    fn ref_tips_reads_local_and_remote_branch_tips() -> Result<()> {
        let (_dir, work, _bare) = crate::test_helpers::init_repo_with_local_remote()?;
        let git = Git::with_workdir(false, &work);

        let local = git.ref_tips("refs/heads/")?;
        assert_eq!(local["main"], git.run(&["rev-parse", "main"])?);

        let remote = git.ref_tips("refs/remotes/origin/")?;
        assert_eq!(remote["main"], git.run(&["rev-parse", "origin/main"])?);
        assert!(!remote.contains_key("HEAD"), "{remote:?}");
        Ok(())
    }

    #[test]
    fn parse_ref_tips_strips_the_prefix_and_skips_head() {
        let out = "refs/remotes/origin/HEAD\0aaa\nrefs/remotes/origin/feature/x\0bbb\n\
                   refs/remotes/other/y\0ccc\nrefs/remotes/origin/z\0\n";
        let tips = parse_ref_tips(out, "refs/remotes/origin/");
        assert_eq!(tips.len(), 1);
        assert_eq!(tips["feature/x"], "bbb");
    }
}
