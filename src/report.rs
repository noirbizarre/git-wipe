//! Machine-readable output for `--json`.
//!
//! Human output goes to stderr through [`crate::ui::Ui`]; this module owns
//! stdout and emits a single JSON document per run. The shape is versioned
//! ([`Report::VERSION`]) so consumers can detect breaking changes.

use std::io::{self, IsTerminal, Write};
use std::path::Path;

use anyhow::Result;
use serde::Serialize;

use crate::branches::Effort;
use crate::duration::MinAge;
use crate::forge::ForgeSetting;
use crate::git::{GitCommandError, GitErrorKind};
use crate::size::Size;

/// Serialize `value` to stdout as a single JSON document.
///
/// Pretty-printed on a terminal (a human is reading it) and compact otherwise
/// (a pipe or a file, where one line per document is friendlier to consumers).
/// A trailing newline is always written.
pub fn print_json<T: Serialize>(value: &T) -> Result<()> {
    let mut out = io::stdout().lock();
    if out.is_terminal() {
        serde_json::to_writer_pretty(&mut out, value)?;
    } else {
        serde_json::to_writer(&mut out, value)?;
    }
    out.write_all(b"\n")?;
    out.flush()?;
    Ok(())
}

/// Overall outcome of a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Success,
    Error,
}

/// Outcome of a single action on a branch, remote or worktree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemStatus {
    /// Fetched or fast-forwarded successfully.
    Updated,
    /// Branch deleted.
    Deleted,
    /// Worktree removed.
    Removed,
    /// Deliberately left untouched (not selected, or force-removal declined).
    Skipped,
    /// Worktree is locked, so it was left alone.
    Locked,
    /// Worktree changed more recently than `--min-age`, so it was left alone.
    TooYoung,
    /// Worktree is smaller than `--min-size`, so it was left alone.
    TooSmall,
    /// The git operation failed; see the matching entry in `errors`.
    Failed,
    /// Would have been acted upon, but `--dry-run` was in effect.
    DryRun,
}

/// Why a local branch was listed as a candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BranchReason {
    /// Contained in one of the merge targets.
    Merged,
    /// Its upstream branch was deleted.
    Gone,
    /// The forge reports its pull/merge request as merged, ending at the
    /// branch tip. Takes precedence over [`BranchReason::Merged`]: forge
    /// detection runs first.
    #[serde(rename = "pr-merged")]
    PrMerged,
}

/// Whether a worktree was tied to a candidate branch or orphaned.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorktreeKind {
    Branch,
    Orphan,
}

/// A failed operation, classified the same way [`crate::ui::Ui::report_failure`]
/// classifies it for humans.
#[derive(Debug, Clone, Serialize)]
pub struct ReportError {
    /// Infinitive phrase describing what was attempted ("fetch from", "delete").
    pub action: String,
    /// What was acted upon.
    pub target: String,
    pub kind: ErrorKind,
    pub message: String,
}

/// Serializable mirror of [`GitErrorKind`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    Network,
    Auth,
    Other,
}

impl From<GitErrorKind> for ErrorKind {
    fn from(kind: GitErrorKind) -> Self {
        match kind {
            GitErrorKind::Network => Self::Network,
            GitErrorKind::Auth => Self::Auth,
            GitErrorKind::Other => Self::Other,
        }
    }
}

impl ReportError {
    /// Build an entry from an `anyhow` error, extracting the git
    /// classification and short cause when available.
    pub fn new(action: &str, target: &str, err: &anyhow::Error) -> Self {
        let (kind, message) = match err.downcast_ref::<GitCommandError>() {
            Some(gerr) => (ErrorKind::from(gerr.kind), gerr.short_cause().to_string()),
            None => (ErrorKind::Other, err.to_string()),
        };
        Self {
            action: action.to_string(),
            target: target.to_string(),
            kind,
            message,
        }
    }
}

/// Phase 1: fetch and prune.
#[derive(Debug, Default, Serialize)]
pub struct FetchPhase {
    /// `--no-fetch` was used (or there was nothing to fetch).
    pub skipped: bool,
    pub remotes: Vec<RemoteFetch>,
}

#[derive(Debug, Serialize)]
pub struct RemoteFetch {
    pub name: String,
    pub status: ItemStatus,
}

/// Phase 2: fast-forwarding merge targets.
#[derive(Debug, Default, Serialize)]
pub struct PullPhase {
    /// `--no-pull` was used (or no target had an upstream).
    pub skipped: bool,
    pub branches: Vec<PulledBranch>,
}

#[derive(Debug, Serialize)]
pub struct PulledBranch {
    pub branch: String,
    pub remote: String,
    pub upstream: String,
    pub status: ItemStatus,
}

/// Phase 3: local branches and worktrees.
#[derive(Debug, Default, Serialize)]
pub struct LocalPhase {
    /// `--remote-only` was used.
    pub skipped: bool,
    /// Branches detected as merged into a target.
    pub merged: Vec<String>,
    /// The subset of `merged` the forge reported (`reason: "pr-merged"`).
    pub pr_merged: Vec<String>,
    /// Branches whose upstream was deleted.
    pub gone: Vec<String>,
    pub branches: Vec<LocalBranch>,
    pub worktrees: Vec<WorktreeEntry>,
}

#[derive(Debug, Serialize)]
pub struct LocalBranch {
    pub branch: String,
    pub reason: BranchReason,
    /// Whether the branch was retained for deletion (interactively or by `--yes`).
    pub selected: bool,
    pub status: ItemStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worktree: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct WorktreeEntry {
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    pub kind: WorktreeKind,
    pub status: ItemStatus,
    /// Size in kibibytes, absent when sizing was not requested (no
    /// `--min-size` or `--size`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size_kb: Option<u64>,
    /// Present when this worktree's lock was found stale (see
    /// [`crate::cleaner`]) and cleared, regardless of `status`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stale_lock: Option<StaleLock>,
}

/// What happened to a worktree's stale lock, when it had one.
///
/// A lock is stale when its reason embeds a pid ([`crate::pid::extract_pid`])
/// that is confirmed dead ([`crate::pid::pid_is_alive`]). Distinct from
/// [`ItemStatus`], since a stale lock is an audit fact independent of the
/// item's eventual outcome (removed, skipped for some other reason, dry-run,
/// or failed).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StaleLockAction {
    /// `git worktree unlock` actually ran.
    Unlocked,
    /// `--dry-run` was in effect; the lock would have been removed.
    WouldUnlock,
}

/// Recorded when a worktree's lock reason embedded a pid confirmed dead.
#[derive(Debug, Clone, Serialize)]
pub struct StaleLock {
    /// The pid found in the lock reason, confirmed no longer running.
    pub pid: u32,
    /// The original lock reason.
    pub reason: String,
    pub action: StaleLockAction,
}

/// Phase 4: merged branches on each configured remote.
#[derive(Debug, Serialize)]
pub struct RemoteReport {
    pub remote: String,
    /// Merged branches detected on this remote.
    pub merged: Vec<String>,
    /// The subset of `merged` the forge reported (`reason: "pr-merged"`).
    pub pr_merged: Vec<String>,
    pub branches: Vec<RemoteBranch>,
}

#[derive(Debug, Serialize)]
pub struct RemoteBranch {
    pub branch: String,
    pub reason: BranchReason,
    pub status: ItemStatus,
}

/// Aggregate counters, mirroring the text-mode summary lines.
#[derive(Debug, Default, Serialize)]
pub struct Summary {
    pub local_branches_deleted: usize,
    pub remote_branches_deleted: usize,
    pub worktrees_removed: usize,
    pub errors: usize,
    /// Total size of removed worktrees, in kibibytes. Zero when sizing was
    /// not requested or nothing was removed.
    pub freed_kb: u64,
}

/// The complete JSON document.
#[derive(Debug, Serialize)]
pub struct Report {
    pub version: u32,
    pub status: Status,
    pub dry_run: bool,
    /// Effective merge-detection effort level (1, 2 or 3).
    pub effort: Effort,
    /// Effective minimum worktree age, e.g. `"0s"` or `"2h"`.
    pub min_age: MinAge,
    /// Effective forge setting: `false`, `true` (auto-detect) or a forge name.
    pub forge: ForgeSetting,
    /// Effective number of concurrent git probes used during analysis.
    /// Reported for reproducibility only: it never changes the results.
    pub jobs: usize,
    pub fetch: FetchPhase,
    pub pull: PullPhase,
    pub local: LocalPhase,
    /// `--local-only` was used.
    pub remotes_skipped: bool,
    pub remotes: Vec<RemoteReport>,
    pub warnings: Vec<String>,
    pub errors: Vec<ReportError>,
    pub summary: Summary,
}

impl Report {
    /// Schema version. Bump on any breaking change to the document shape.
    pub const VERSION: u32 = 1;

    pub fn new(dry_run: bool, effort: Effort, min_age: MinAge, jobs: usize) -> Self {
        Self {
            version: Self::VERSION,
            status: Status::Success,
            dry_run,
            effort,
            min_age,
            forge: ForgeSetting::Off,
            jobs,
            fetch: FetchPhase::default(),
            pull: PullPhase::default(),
            local: LocalPhase::default(),
            remotes_skipped: false,
            remotes: Vec::new(),
            warnings: Vec::new(),
            errors: Vec::new(),
            summary: Summary::default(),
        }
    }

    /// Record a non-fatal failure. The run keeps going, exactly as in text mode.
    pub fn push_error(&mut self, action: &str, target: &str, err: &anyhow::Error) {
        self.errors.push(ReportError::new(action, target, err));
        self.summary.errors = self.errors.len();
    }

    pub fn push_warning(&mut self, warning: impl Into<String>) {
        self.warnings.push(warning.into());
    }

    /// A single-error document for a run that could not complete.
    pub fn fatal(action: &str, target: &str, err: &anyhow::Error) -> Self {
        let mut report = Self::new(false, Effort::default(), MinAge::default(), 1);
        report.status = Status::Error;
        report.push_error(action, target, err);
        report
    }
}

/// Serializable view of `git wipe config list`.
#[derive(Debug, Serialize)]
pub struct ConfigReport {
    /// Whether a `[wipe]` section exists in git config.
    pub configured: bool,
    pub protected: Vec<String>,
    pub ignore: Vec<String>,
    /// `null` means "all remotes".
    pub remotes: Option<Vec<String>>,
    pub branch_protected: Vec<String>,
    pub branch_ignored: Vec<String>,
    /// `null` means "use the built-in default".
    pub effort: Option<Effort>,
    /// `null` means "use the built-in default" (no guard).
    pub min_age: Option<MinAge>,
    /// `null` means "use the built-in default" (no guard).
    pub min_size: Option<Size>,
    /// `null` means "use the CPU count".
    pub jobs: Option<u32>,
    /// `null` means "auto-detect".
    pub worktrunk: Option<bool>,
    /// `null` means "off".
    pub forge: Option<ForgeSetting>,
}

/// Render a path for the JSON document (lossy, like the human output).
pub fn path_string(path: &Path) -> String {
    path.display().to_string()
}

/// What a [`StatusEntry`] describes.
///
/// Distinct from [`WorktreeKind`], which has no case for a branch that is not
/// checked out anywhere — a row `git wipe status` must be able to report.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StatusKind {
    /// A worktree holding a local branch.
    Worktree,
    /// A worktree whose branch no longer exists, or which is detached.
    Orphan,
    /// A local branch with no worktree checked out.
    Branch,
}

/// A single observation about a [`StatusEntry`]. Several may apply at once.
///
/// Deliberately not [`ItemStatus`]: those variants are *outcomes*
/// (`deleted`, `removed`, `dry_run`), and `git wipe status` never acts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StatusFlag {
    /// Detected as merged into a protected branch.
    Merged,
    /// The forge reports its pull/merge request as merged.
    #[serde(rename = "pr-merged")]
    PrMerged,
    /// Configured upstream no longer exists.
    Gone,
    /// Working tree has no uncommitted change.
    Clean,
    /// Working tree has uncommitted changes.
    Dirty,
    /// Holds commits absent from every merge target.
    Unmerged,
    /// Worktree is locked.
    Locked,
    /// Worktree has no live branch.
    Orphan,
}

/// One branch or worktree in the [`StatusReport`] inventory.
#[derive(Debug, Serialize)]
pub struct StatusEntry {
    pub kind: StatusKind,
    /// Absent for a detached-HEAD worktree.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// Absolute worktree path; absent for a branch without a worktree.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Age in seconds, or `null` when it could not be established.
    ///
    /// Serialized rather than skipped, so consumers can tell "unknown" from
    /// "this tool version does not report an age".
    pub age_seconds: Option<u64>,
    /// Size in kibibytes, or `null` when it was not computed (sizing needs
    /// `--min-size` or `--size`) or the row has no worktree (a plain branch).
    pub size_kb: Option<u64>,
    /// The observations that apply, in the order the STATUS column renders them.
    pub status: Vec<StatusFlag>,
    /// Checked out in the worktree the command was run from.
    pub current: bool,
    /// Matched by a protected pattern, and therefore never a deletion candidate.
    pub protected: bool,
}

/// Serializable view of `git wipe status`.
///
/// A read-only document: it carries no action or outcome field, because
/// `status` neither fetches nor mutates. `version` is [`Report::VERSION`] —
/// both documents belong to the same versioned schema.
#[derive(Debug, Serialize)]
pub struct StatusReport {
    pub version: u32,
    pub status: Status,
    pub entries: Vec<StatusEntry>,
    pub warnings: Vec<String>,
    pub errors: Vec<ReportError>,
}

impl StatusReport {
    /// Build a successful status document.
    pub fn new(entries: Vec<StatusEntry>, warnings: Vec<String>) -> Self {
        Self {
            version: Report::VERSION,
            status: Status::Success,
            entries,
            warnings,
            errors: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_serializes_expected_shape() {
        let mut report = Report::new(true, Effort::Thorough, MinAge::default(), 1);
        report.local.merged.push("feature".to_string());
        report.local.branches.push(LocalBranch {
            branch: "feature".to_string(),
            reason: BranchReason::Merged,
            selected: true,
            status: ItemStatus::DryRun,
            worktree: None,
        });
        let value: serde_json::Value = serde_json::to_value(&report).unwrap();
        assert_eq!(value["version"], 1);
        assert_eq!(value["status"], "success");
        assert_eq!(value["dry_run"], true);
        assert_eq!(value["local"]["merged"][0], "feature");
        assert_eq!(value["local"]["branches"][0]["status"], "dry_run");
        assert_eq!(value["local"]["branches"][0]["reason"], "merged");
        // Omitted when absent
        assert!(value["local"]["branches"][0].get("worktree").is_none());
        assert_eq!(value["summary"]["local_branches_deleted"], 0);
    }

    #[test]
    fn push_error_classifies_and_counts() {
        let mut report = Report::new(false, Effort::default(), MinAge::default(), 1);
        let err = anyhow::Error::new(GitCommandError {
            program: "git".into(),
            args: vec!["fetch".into()],
            exit_code: Some(1),
            stderr: "ssh: Could not resolve hostname github.com".into(),
            kind: GitErrorKind::Network,
        });
        report.push_error("fetch from", "origin", &err);
        assert_eq!(report.errors.len(), 1);
        assert_eq!(report.errors[0].kind, ErrorKind::Network);
        assert_eq!(report.errors[0].action, "fetch from");
        assert_eq!(report.errors[0].target, "origin");
        assert_eq!(report.summary.errors, 1);
    }

    #[test]
    fn fatal_report_has_error_status() {
        let report = Report::fatal("run", "repository", &anyhow::anyhow!("boom"));
        assert_eq!(report.status, Status::Error);
        assert_eq!(report.errors[0].message, "boom");
        assert_eq!(report.errors[0].kind, ErrorKind::Other);
    }

    #[test]
    fn error_kind_mirrors_every_git_error_kind() {
        assert_eq!(ErrorKind::from(GitErrorKind::Network), ErrorKind::Network);
        assert_eq!(ErrorKind::from(GitErrorKind::Auth), ErrorKind::Auth);
        assert_eq!(ErrorKind::from(GitErrorKind::Other), ErrorKind::Other);
    }

    #[test]
    fn effort_serializes_as_its_numeric_level() {
        let report = Report::new(false, Effort::Thorough, MinAge::default(), 1);
        let value: serde_json::Value = serde_json::to_value(&report).unwrap();
        assert_eq!(value["effort"], 3);
    }

    #[test]
    fn min_age_serializes_as_its_canonical_string() {
        let report = Report::new(false, Effort::default(), "2h".parse().unwrap(), 1);
        let value: serde_json::Value = serde_json::to_value(&report).unwrap();
        assert_eq!(value["min_age"], "2h");
    }

    #[test]
    fn print_json_writes_a_document_to_stdout() {
        // stdout is captured by the test harness and is not a terminal here,
        // so this exercises the compact branch.
        print_json(&Report::new(true, Effort::default(), MinAge::default(), 1)).unwrap();
        print_json(&ConfigReport {
            configured: false,
            protected: Vec::new(),
            ignore: Vec::new(),
            remotes: None,
            branch_protected: Vec::new(),
            branch_ignored: Vec::new(),
            worktrunk: None,
            forge: None,
            effort: None,
            min_age: None,
            min_size: None,
            jobs: None,
        })
        .unwrap();
    }

    #[test]
    fn config_report_serializes_unset_values_as_null() {
        let value = serde_json::to_value(ConfigReport {
            configured: true,
            protected: vec!["main".to_string()],
            ignore: Vec::new(),
            remotes: None,
            branch_protected: Vec::new(),
            branch_ignored: Vec::new(),
            worktrunk: Some(true),
            forge: Some(ForgeSetting::Auto),
            min_age: None,
            min_size: None,
            effort: Some(Effort::Quick),
            jobs: None,
        })
        .unwrap();
        assert_eq!(value["configured"], true);
        assert_eq!(value["protected"][0], "main");
        assert!(value["remotes"].is_null());
        assert_eq!(value["effort"], 1);
    }

    #[test]
    fn path_string_renders_the_path() {
        assert_eq!(path_string(Path::new("/tmp/wt")), "/tmp/wt");
    }

    #[test]
    fn pr_merged_is_distinct_in_json() {
        let value = serde_json::to_value(BranchReason::PrMerged).unwrap();
        assert_eq!(value, "pr-merged");
        let value = serde_json::to_value(StatusFlag::PrMerged).unwrap();
        assert_eq!(value, "pr-merged");
        assert_eq!(
            serde_json::to_value(BranchReason::Merged).unwrap(),
            "merged"
        );
    }

    #[test]
    fn report_carries_the_effective_forge_setting() {
        let mut report = Report::new(false, Effort::default(), MinAge::default(), 1);
        assert_eq!(serde_json::to_value(&report).unwrap()["forge"], "false");
        report.forge = ForgeSetting::Auto;
        assert_eq!(serde_json::to_value(&report).unwrap()["forge"], "true");
        assert!(serde_json::to_value(&report).unwrap()["local"]["pr_merged"].is_array());
    }
}
