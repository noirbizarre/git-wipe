//! Read-only inventory of local branches and worktrees (`git wipe status`).
//!
//! Unlike [`crate::cleaner`], this module never fetches, never prompts and
//! never mutates: it observes the repository as it is on disk and renders one
//! row per worktree plus one per branch that has no worktree, oldest first.
//!
//! Two deliberate divergences from the cleanup passes:
//!
//! - A detached-HEAD worktree *is* listed, as an orphan.
//!   [`crate::worktrees::find_orphan_worktrees`] skips it because it cannot be
//!   removed safely; an inventory that hid a real checkout would simply lie.
//! - An ignored branch is *not* listed. `wipe.ignore` means "pretend this does
//!   not exist", and being consistent with every other pass beats completeness.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Result;

use crate::branches::{Effort, Filter, find_gone_local, find_merged_local, resolve_merge_targets};
use crate::duration::MinAge;
use crate::git::Git;
use crate::parallel;
use crate::report::{StatusEntry, StatusFlag, StatusKind, StatusReport, path_string};
use crate::size::{Size, format_bytes};
use crate::ui::{Ui, tilde_path};
use crate::worktrees::{worktree_age, worktree_size};

/// Emitted whenever a `gone` branch is reported: `status` never fetches, so the
/// remote-tracking refs it reads are only as fresh as the last `git fetch`.
const STALE_GONE_WARNING: &str =
    "Remotes were not fetched; deleted-upstream detection may be stale.";

/// What a [`Row`] describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowKind {
    /// A worktree holding a local branch.
    Worktree,
    /// A worktree whose branch no longer exists, or which is detached.
    Orphan,
    /// A local branch with no worktree checked out.
    Branch,
}

/// The independent facts observed about one row.
///
/// The rendered STATUS column and the JSON `status` array are both derived from
/// this, so the two can never disagree.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Flags {
    pub merged: bool,
    pub gone: bool,
    pub unmerged: bool,
    pub dirty: bool,
    pub clean: bool,
    pub locked: bool,
    pub orphan: bool,
}

/// One line of the listing.
#[derive(Debug, Clone)]
pub struct Row {
    pub kind: RowKind,
    /// `None` for a detached-HEAD worktree.
    pub branch: Option<String>,
    /// `None` for a branch without a worktree.
    pub path: Option<PathBuf>,
    /// `None` when the age could not be established.
    pub age: Option<Duration>,
    /// Size in bytes, or `None` when sizing was not requested, or the row has
    /// no worktree (a branch row).
    pub size: Option<u64>,
    pub flags: Flags,
    /// Checked out in the worktree the command was run from.
    pub current: bool,
    pub protected: bool,
}

/// Inputs resolved by `main` before the scan starts.
#[derive(Debug, Clone, Copy)]
pub struct StatusOptions {
    pub effort: Effort,
    pub jobs: usize,
    /// `--min-age`: list only entries at least this old. Zero disables it.
    pub min_age: MinAge,
    /// `--min-size`: list only entries at least this large. Zero disables it.
    pub min_size: Size,
    /// `--size`: force size computation/display without filtering (implied
    /// by a non-zero `min_size`).
    pub show_size: bool,
    /// `--merged`: list only rows flagged as merged.
    pub merged_only: bool,
}

/// Everything the scan produced, ready to render or serialize.
#[derive(Debug, Default)]
pub struct Scan {
    pub rows: Vec<Row>,
    pub warnings: Vec<String>,
}

/// Collect every row, sorted oldest first.
///
/// Performs read-only git calls only: no fetch, no prompt, no mutation.
pub fn scan(git: &Git, filter: &Filter, ui: &Ui, opts: StatusOptions) -> Result<Scan> {
    let mut warnings = Vec::new();

    // `rev-parse --abbrev-ref HEAD` yields the literal "HEAD" when detached,
    // which is not a branch name.
    let current = git.current_branch().ok().filter(|b| b != "HEAD");

    // Bare entries have no checkout and nothing to report, and are excluded
    // everywhere else too.
    let worktrees: Vec<_> = git
        .worktree_list()?
        .into_iter()
        .filter(|wt| !wt.is_bare)
        .filter(|wt| wt.branch.as_ref().is_none_or(|b| !filter.is_ignored(b)))
        .collect();

    let locals: Vec<String> = git
        .local_branches()?
        .into_iter()
        .filter(|b| !filter.is_ignored(b))
        .collect();
    let live: HashSet<&str> = locals.iter().map(String::as_str).collect();
    let dates = git.branch_committer_dates()?;

    // The spinner clears its own line, so its warnings are surfaced after it
    // returns rather than printed from inside the closure.
    let merged = ui.spinner("Scanning local branches…", || {
        find_merged_local(git, filter, opts.effort, opts.jobs)
    })?;
    for warning in &merged.warnings {
        ui.warning(warning);
    }
    warnings.extend(merged.warnings.iter().cloned());
    let merged_set: HashSet<&str> = merged.candidates.iter().map(String::as_str).collect();

    let gone = find_gone_local(git, filter, &merged.candidates)?;
    if !gone.is_empty() {
        ui.warning(STALE_GONE_WARNING);
        warnings.push(STALE_GONE_WARNING.to_string());
    }
    let gone_set: HashSet<&str> = gone.iter().map(String::as_str).collect();

    let targets = resolve_merge_targets(git, filter)?;

    // Read-only probes, overlapped: each is a git invocation waiting on I/O.
    let dirty: Vec<Result<bool>> =
        parallel::map(&worktrees, opts.jobs, |_, wt| git.worktree_dirty(&wt.path));
    let ages: Vec<Option<Duration>> =
        parallel::map(&worktrees, opts.jobs, |_, wt| worktree_age(git, wt));
    // Sizing walks each worktree's directory tree, so it is skipped entirely
    // unless `--min-size` or `--size` asked for it, keeping the default
    // listing cheap enough for a shell hook.
    let need_size = opts.show_size || !opts.min_size.is_zero();
    let sizes: Vec<Option<u64>> = if need_size {
        parallel::map(&worktrees, opts.jobs, |_, wt| worktree_size(wt))
    } else {
        vec![None; worktrees.len()]
    };

    // Only branches that are not already merged need the unmerged probe.
    let to_probe: Vec<&String> = locals
        .iter()
        .filter(|b| !merged_set.contains(b.as_str()))
        .collect();
    let unmerged: HashMap<&str, bool> = parallel::map(&to_probe, opts.jobs, |_, branch| {
        // An error means we could not prove containment; assume unmerged, the
        // same conservative default the cleaner uses.
        git.branch_has_unmerged_commits(branch, &targets)
            .unwrap_or(true)
    })
    .into_iter()
    .zip(&to_probe)
    .map(|(unmerged, branch)| (branch.as_str(), unmerged))
    .collect();

    let mut rows = Vec::new();
    let mut with_worktree: HashSet<&str> = HashSet::new();

    for (((wt, dirty), age), size) in worktrees.iter().zip(dirty).zip(ages).zip(sizes) {
        let orphan = match &wt.branch {
            Some(branch) => !live.contains(branch.as_str()),
            None => true, // detached HEAD
        };
        let mut flags = Flags {
            locked: wt.is_locked,
            orphan,
            ..Flags::default()
        };
        match dirty {
            Ok(true) => flags.dirty = true,
            Ok(false) => flags.clean = true,
            Err(err) => warnings.push(format!(
                "Could not check the status of '{}': {err}",
                tilde_path(&wt.path)
            )),
        }
        // An orphan has no live branch left to compare against.
        if let Some(branch) = &wt.branch
            && !orphan
        {
            with_worktree.insert(branch.as_str());
            set_merge_flags(&mut flags, branch, &merged_set, &gone_set, &unmerged);
        }
        rows.push(Row {
            kind: if orphan {
                RowKind::Orphan
            } else {
                RowKind::Worktree
            },
            branch: wt.branch.clone(),
            path: Some(wt.path.clone()),
            age,
            size,
            flags,
            current: wt.branch.as_deref() == current.as_deref() && current.is_some(),
            protected: wt.branch.as_ref().is_some_and(|b| filter.is_protected(b)),
        });
    }

    let now = SystemTime::now();
    for branch in &locals {
        if with_worktree.contains(branch.as_str()) {
            continue;
        }
        let mut flags = Flags::default();
        set_merge_flags(&mut flags, branch, &merged_set, &gone_set, &unmerged);
        rows.push(Row {
            kind: RowKind::Branch,
            branch: Some(branch.clone()),
            path: None,
            age: dates.get(branch).and_then(|&secs| {
                // A tip dated in the future has no meaningful age.
                now.duration_since(UNIX_EPOCH + Duration::from_secs(secs))
                    .ok()
            }),
            // A branch without a worktree has nothing on disk to measure.
            size: None,
            flags,
            current: current.as_deref() == Some(branch.as_str()),
            protected: filter.is_protected(branch),
        });
    }

    sort_rows(&mut rows);
    Ok(Scan { rows, warnings })
}

/// Apply the branch-derived flags. `merged` and `unmerged` are exclusive.
fn set_merge_flags(
    flags: &mut Flags,
    branch: &str,
    merged: &HashSet<&str>,
    gone: &HashSet<&str>,
    unmerged: &HashMap<&str, bool>,
) {
    if merged.contains(branch) {
        flags.merged = true;
    } else {
        flags.unmerged = unmerged.get(branch).copied().unwrap_or(false);
    }
    flags.gone = gone.contains(branch);
}

/// Sort oldest first, with a deterministic tiebreak.
///
/// An unknown age sorts *last*: failing to date an entry is not evidence that
/// it is old, and promoting it to the top would misdirect attention.
fn sort_rows(rows: &mut [Row]) {
    rows.sort_by(|a, b| {
        match (a.age, b.age) {
            (Some(x), Some(y)) => y.cmp(&x),
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (None, None) => Ordering::Equal,
        }
        .then_with(|| a.branch.cmp(&b.branch))
        .then_with(|| a.path.cmp(&b.path))
    });
}

/// Apply `--min-age`, `--min-size` and `--merged`.
///
/// Kept separate from [`scan`] so the table and the JSON document filter
/// identically, and so it is testable without a repository.
pub fn filter_rows(rows: Vec<Row>, opts: StatusOptions) -> Vec<Row> {
    rows.into_iter()
        .filter(|row| !opts.merged_only || row.flags.merged)
        .filter(|row| {
            // `--min-age` filters here rather than guarding, as it does on a
            // wipe run. An entry whose age is unknown is kept: silently hiding
            // something we merely failed to date is the worst outcome for an
            // inventory.
            opts.min_age.is_zero() || row.age.is_none_or(|age| age >= opts.min_age.as_duration())
        })
        .filter(|row| {
            // `--min-size` filters here rather than guarding, exactly like
            // `--min-age`. An entry with no known size (not computed, or a
            // branch without a worktree) is kept.
            opts.min_size.is_zero()
                || row
                    .size
                    .is_none_or(|bytes| bytes >= opts.min_size.as_bytes())
        })
        .collect()
}

/// The status tokens of a row, most actionable first.
pub fn status_tokens(flags: Flags) -> Vec<&'static str> {
    let mut tokens = Vec::new();
    for (set, token) in [
        (flags.orphan, "orphan"),
        (flags.locked, "locked"),
        (flags.merged, "merged"),
        (flags.gone, "gone"),
        (flags.unmerged, "unmerged"),
        (flags.dirty, "dirty"),
        (flags.clean, "clean"),
    ] {
        if set {
            tokens.push(token);
        }
    }
    tokens
}

/// Style one status token. A no-op once `--no-color` disabled colours globally.
fn style_token(token: &str) -> String {
    let styled = console::style(token);
    match token {
        "merged" => styled.green(),
        "gone" => styled.yellow(),
        "dirty" => styled.red(),
        "unmerged" => styled.cyan(),
        "clean" => styled.dim(),
        "locked" => styled.magenta(),
        "orphan" => styled.red().bold(),
        _ => styled,
    }
    .to_string()
}

/// Render an age as its largest whole unit: `45s`, `1h`, `3d`, `2w`.
///
/// [`MinAge`]'s own `Display` only renders exact multiples, so a 90-minute age
/// would print as `5400s` — precise, and unreadable in a column.
fn format_age(age: Option<Duration>) -> String {
    let Some(age) = age else {
        return "?".to_string();
    };
    let secs = age.as_secs();
    for (unit_secs, suffix) in [
        (604_800, "w"),
        (86_400, "d"),
        (3_600, "h"),
        (60, "m"),
        (1, "s"),
    ] {
        if secs >= unit_secs {
            return format!("{}{suffix}", secs / unit_secs);
        }
    }
    "0s".to_string()
}

/// Render a size for the table: `-` when none was computed (sizing was not
/// requested, or the row has no worktree).
fn format_size(size: Option<u64>) -> String {
    size.map_or_else(|| "-".to_string(), format_bytes)
}

/// Render the listing as an aligned table.
///
/// Widths are computed on the unstyled cells and colour applied after padding,
/// so ANSI escapes never enter the arithmetic.
pub fn render(ui: &Ui, rows: &[Row], filtered: bool) {
    if rows.is_empty() {
        if filtered {
            ui.muted("No entries match the given filters.");
        } else {
            ui.muted("No branches or worktrees to list.");
        }
        return;
    }

    let cells: Vec<[String; 5]> = rows
        .iter()
        .map(|row| {
            let tokens = status_tokens(row.flags);
            let status = if tokens.is_empty() {
                "-".to_string()
            } else {
                tokens.join(",")
            };
            let branch = row
                .branch
                .clone()
                .unwrap_or_else(|| "(detached)".to_string());
            [
                format!("{} {branch}", if row.current { "*" } else { " " }),
                row.path
                    .as_deref()
                    .map_or_else(|| "-".to_string(), tilde_path),
                format_size(row.size),
                format_age(row.age),
                status,
            ]
        })
        .collect();

    const HEADERS: [&str; 5] = ["BRANCH", "PATH", "SIZE", "AGE", "STATUS"];
    let mut widths = HEADERS.map(|h| h.chars().count());
    for row in &cells {
        for (width, cell) in widths.iter_mut().zip(row) {
            *width = (*width).max(cell.chars().count());
        }
    }
    // BRANCH carries a two-character current-branch marker the header does not.
    widths[0] = widths[0].max(HEADERS[0].chars().count() + 2);

    ui.table_header(&format!(
        "{:<b$}  {:<p$}  {:<z$}  {:<a$}  {}",
        format!("  {}", HEADERS[0]),
        HEADERS[1],
        HEADERS[2],
        HEADERS[3],
        HEADERS[4],
        b = widths[0],
        p = widths[1],
        z = widths[2],
        a = widths[3],
    ));

    for (row, cell) in rows.iter().zip(&cells) {
        let tokens = status_tokens(row.flags);
        let status = if tokens.is_empty() {
            cell[4].clone()
        } else {
            tokens
                .iter()
                .map(|t| style_token(t))
                .collect::<Vec<_>>()
                .join(",")
        };
        ui.table_row(&format!(
            "{:<b$}  {:<p$}  {:<z$}  {:<a$}  {status}",
            cell[0],
            cell[1],
            cell[2],
            cell[3],
            b = widths[0],
            p = widths[1],
            z = widths[2],
            a = widths[3],
        ));
    }
}

/// Build the JSON document.
pub fn to_report(rows: &[Row], warnings: Vec<String>) -> StatusReport {
    let entries = rows
        .iter()
        .map(|row| StatusEntry {
            kind: match row.kind {
                RowKind::Worktree => StatusKind::Worktree,
                RowKind::Orphan => StatusKind::Orphan,
                RowKind::Branch => StatusKind::Branch,
            },
            branch: row.branch.clone(),
            path: row.path.as_deref().map(path_string),
            age_seconds: row.age.map(|age| age.as_secs()),
            size_kb: row.size.map(|bytes| bytes / 1024),
            status: status_tokens(row.flags)
                .into_iter()
                .map(|token| match token {
                    "orphan" => StatusFlag::Orphan,
                    "locked" => StatusFlag::Locked,
                    "merged" => StatusFlag::Merged,
                    "gone" => StatusFlag::Gone,
                    "unmerged" => StatusFlag::Unmerged,
                    "dirty" => StatusFlag::Dirty,
                    "clean" => StatusFlag::Clean,
                    other => unreachable!("unknown status token: {other}"),
                })
                .collect(),
            current: row.current,
            protected: row.protected,
        })
        .collect();
    StatusReport::new(entries, warnings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::test_helpers;
    use std::path::Path;

    fn opts() -> StatusOptions {
        StatusOptions {
            effort: Effort::default(),
            jobs: 1,
            min_age: MinAge::default(),
            min_size: Size::default(),
            show_size: false,
            merged_only: false,
        }
    }

    fn row(branch: &str, age: Option<Duration>, flags: Flags) -> Row {
        Row {
            kind: RowKind::Branch,
            branch: Some(branch.to_string()),
            path: None,
            age,
            size: None,
            flags,
            current: false,
            protected: false,
        }
    }

    fn names(rows: &[Row]) -> Vec<&str> {
        rows.iter()
            .map(|r| r.branch.as_deref().unwrap_or("(detached)"))
            .collect()
    }

    // ── Status tokens ────────────────────────────────────────────────

    #[test]
    fn status_tokens_orders_flags_consistently() {
        let flags = Flags {
            merged: true,
            gone: true,
            clean: true,
            ..Flags::default()
        };
        assert_eq!(status_tokens(flags), ["merged", "gone", "clean"]);
    }

    #[test]
    fn status_tokens_for_an_orphan_row() {
        let flags = Flags {
            orphan: true,
            dirty: true,
            ..Flags::default()
        };
        assert_eq!(status_tokens(flags), ["orphan", "dirty"]);
    }

    #[test]
    fn status_tokens_for_a_locked_dirty_worktree() {
        let flags = Flags {
            locked: true,
            unmerged: true,
            dirty: true,
            ..Flags::default()
        };
        assert_eq!(status_tokens(flags), ["locked", "unmerged", "dirty"]);
    }

    #[test]
    fn status_tokens_are_empty_without_any_flag() {
        assert!(status_tokens(Flags::default()).is_empty());
    }

    #[test]
    fn set_merge_flags_never_yields_merged_and_unmerged_together() {
        let merged: HashSet<&str> = ["done"].into_iter().collect();
        let gone: HashSet<&str> = HashSet::new();
        // Claim the branch is unmerged too: being a merged candidate wins.
        let unmerged: HashMap<&str, bool> = [("done", true)].into_iter().collect();

        let mut flags = Flags::default();
        set_merge_flags(&mut flags, "done", &merged, &gone, &unmerged);
        assert!(flags.merged);
        assert!(!flags.unmerged);
    }

    #[test]
    fn set_merge_flags_marks_gone_alongside_merged() {
        let merged: HashSet<&str> = ["done"].into_iter().collect();
        let gone: HashSet<&str> = ["done"].into_iter().collect();
        let mut flags = Flags::default();
        set_merge_flags(&mut flags, "done", &merged, &gone, &HashMap::new());
        assert_eq!(status_tokens(flags), ["merged", "gone"]);
    }

    // ── Age formatting ───────────────────────────────────────────────

    #[test]
    fn format_age_renders_the_largest_whole_unit() {
        assert_eq!(format_age(Some(Duration::from_secs(45))), "45s");
        assert_eq!(format_age(Some(Duration::from_secs(90 * 60))), "1h");
        assert_eq!(format_age(Some(Duration::from_secs(26 * 3600))), "1d");
        assert_eq!(format_age(Some(Duration::from_secs(15 * 86_400))), "2w");
        assert_eq!(format_age(Some(Duration::from_secs(0))), "0s");
    }

    #[test]
    fn format_age_renders_unknown_as_question_mark() {
        assert_eq!(format_age(None), "?");
    }

    #[test]
    fn format_size_renders_dash_for_none() {
        assert_eq!(format_size(None), "-");
    }

    #[test]
    fn format_size_renders_a_human_value() {
        assert_eq!(format_size(Some(1024)), "1.0K");
    }

    // ── Sorting ──────────────────────────────────────────────────────

    #[test]
    fn sort_rows_puts_the_oldest_first() {
        let mut rows = vec![
            row("young", Some(Duration::from_secs(10)), Flags::default()),
            row("old", Some(Duration::from_secs(1000)), Flags::default()),
            row("middle", Some(Duration::from_secs(100)), Flags::default()),
        ];
        sort_rows(&mut rows);
        assert_eq!(names(&rows), ["old", "middle", "young"]);
    }

    #[test]
    fn sort_rows_puts_unknown_age_last() {
        let mut rows = vec![
            row("unknown", None, Flags::default()),
            row("young", Some(Duration::from_secs(1)), Flags::default()),
        ];
        sort_rows(&mut rows);
        assert_eq!(names(&rows), ["young", "unknown"]);
    }

    #[test]
    fn sort_rows_is_stable_on_equal_ages() {
        let age = Some(Duration::from_secs(5));
        let mut rows = vec![
            row("zeta", age, Flags::default()),
            row("alpha", age, Flags::default()),
        ];
        sort_rows(&mut rows);
        assert_eq!(names(&rows), ["alpha", "zeta"]);
    }

    // ── Filters ──────────────────────────────────────────────────────

    #[test]
    fn filter_rows_min_age_keeps_only_older_entries() {
        let rows = vec![
            row("old", Some(Duration::from_secs(7200)), Flags::default()),
            row("young", Some(Duration::from_secs(60)), Flags::default()),
        ];
        let opts = StatusOptions {
            min_age: "1h".parse().unwrap(),
            ..opts()
        };
        assert_eq!(names(&filter_rows(rows, opts)), ["old"]);
    }

    #[test]
    fn filter_rows_min_age_keeps_entries_with_unknown_age() {
        let rows = vec![row("unknown", None, Flags::default())];
        let opts = StatusOptions {
            min_age: "1w".parse().unwrap(),
            ..opts()
        };
        assert_eq!(names(&filter_rows(rows, opts)), ["unknown"]);
    }

    #[test]
    fn filter_rows_zero_min_age_keeps_everything() {
        let rows = vec![
            row("a", Some(Duration::from_secs(1)), Flags::default()),
            row("b", None, Flags::default()),
        ];
        assert_eq!(filter_rows(rows, opts()).len(), 2);
    }

    #[test]
    fn filter_rows_min_size_keeps_only_larger_entries() {
        let rows = vec![
            Row {
                size: Some(200 * 1024 * 1024),
                ..row("big", None, Flags::default())
            },
            Row {
                size: Some(10 * 1024 * 1024),
                ..row("small", None, Flags::default())
            },
        ];
        let opts = StatusOptions {
            min_size: "100M".parse().unwrap(),
            ..opts()
        };
        assert_eq!(names(&filter_rows(rows, opts)), ["big"]);
    }

    #[test]
    fn filter_rows_min_size_keeps_entries_with_unknown_size() {
        let rows = vec![row("unknown", None, Flags::default())];
        let opts = StatusOptions {
            min_size: "1G".parse().unwrap(),
            ..opts()
        };
        assert_eq!(names(&filter_rows(rows, opts)), ["unknown"]);
    }

    #[test]
    fn filter_rows_zero_min_size_keeps_everything() {
        let rows = vec![
            Row {
                size: Some(1),
                ..row("a", None, Flags::default())
            },
            row("b", None, Flags::default()),
        ];
        assert_eq!(filter_rows(rows, opts()).len(), 2);
    }

    #[test]
    fn filter_rows_merged_only_drops_orphans_and_unmerged() {
        let merged = Flags {
            merged: true,
            ..Flags::default()
        };
        let unmerged = Flags {
            unmerged: true,
            ..Flags::default()
        };
        let orphan = Flags {
            orphan: true,
            ..Flags::default()
        };
        let rows = vec![
            row("done", None, merged),
            row("wip", None, unmerged),
            row("stray", None, orphan),
        ];
        let opts = StatusOptions {
            merged_only: true,
            ..opts()
        };
        assert_eq!(names(&filter_rows(rows, opts)), ["done"]);
    }

    // ── Rendering ────────────────────────────────────────────────────

    #[test]
    fn render_reports_an_empty_listing() {
        let ui = Ui::new();
        render(&ui, &[], false);
        render(&ui, &[], true);
    }

    #[test]
    fn render_does_not_panic_on_mixed_rows() {
        let ui = Ui::new();
        let rows = vec![
            Row {
                kind: RowKind::Orphan,
                branch: None,
                path: Some(PathBuf::from("/tmp/detached")),
                age: Some(Duration::from_secs(3600)),
                size: Some(1024 * 1024),
                flags: Flags {
                    orphan: true,
                    dirty: true,
                    ..Flags::default()
                },
                current: false,
                protected: false,
            },
            row("main", None, Flags::default()),
        ];
        render(&ui, &rows, false);
    }

    // ── Scan (against real repositories) ─────────────────────────────

    fn scan_repo(git: &Git) -> Result<Scan> {
        let filter = Filter::load(git, &Config::default())?;
        scan(git, &filter, &Ui::quiet(), opts())
    }

    #[test]
    fn scan_lists_every_branch_of_a_plain_repository() -> Result<()> {
        let (_dir, git) = test_helpers::init_repo_with_branches()?;
        let scan = scan_repo(&git)?;
        let listed = names(&scan.rows);
        for branch in git.local_branches()? {
            assert!(listed.contains(&branch.as_str()), "{branch} must be listed");
        }
        Ok(())
    }

    #[test]
    fn scan_marks_the_current_branch() -> Result<()> {
        let (_dir, git) = test_helpers::init_repo_with_branches()?;
        let current = git.current_branch()?;
        let scan = scan_repo(&git)?;
        let marked: Vec<&str> = scan
            .rows
            .iter()
            .filter(|r| r.current)
            .filter_map(|r| r.branch.as_deref())
            .collect();
        assert_eq!(marked, [current.as_str()]);
        Ok(())
    }

    #[test]
    fn scan_reports_a_worktree_with_its_path() -> Result<()> {
        let (_dir, git, wt_path) = test_helpers::init_repo_with_worktree()?;
        let scan = scan_repo(&git)?;
        let row = scan
            .rows
            .iter()
            .find(|r| r.branch.as_deref() == Some("feature/wt"))
            .expect("the linked worktree must be listed");
        assert_eq!(row.kind, RowKind::Worktree);
        // Compared by trailing component, not in full: on macOS the fixture
        // hands back `/var/...` while git resolves it to `/private/var/...`.
        let name = Path::new(&wt_path)
            .file_name()
            .expect("a worktree directory");
        assert!(
            row.path.as_ref().is_some_and(|p| p.ends_with(name)),
            "expected a path ending in {name:?}, got {:?}",
            row.path
        );
        // A freshly created worktree has nothing uncommitted.
        assert!(row.flags.clean, "expected clean, got {:?}", row.flags);
        Ok(())
    }

    #[test]
    fn scan_reports_a_branch_without_a_worktree_without_a_path() -> Result<()> {
        let (_dir, git) = test_helpers::init_repo_with_branches()?;
        let scan = scan_repo(&git)?;
        for row in &scan.rows {
            if row.kind == RowKind::Branch {
                assert!(row.path.is_none());
                // No working tree, so neither clean nor dirty applies.
                assert!(!row.flags.clean && !row.flags.dirty);
            }
        }
        Ok(())
    }

    #[test]
    fn scan_flags_a_locked_worktree() -> Result<()> {
        let (_dir, git, wt_path) = test_helpers::init_repo_with_locked_worktree()?;
        let scan = scan_repo(&git)?;
        let row = scan
            .rows
            .iter()
            .find(|r| r.branch.as_deref() == Some("feature/locked-wt"))
            .expect("the locked worktree must be listed");
        assert!(row.flags.locked);
        let name = Path::new(&wt_path)
            .file_name()
            .expect("a worktree directory");
        assert!(row.path.as_ref().is_some_and(|p| p.ends_with(name)));
        Ok(())
    }

    #[test]
    fn scan_warns_that_remotes_were_not_fetched() -> Result<()> {
        let (_dir, git) = test_helpers::init_repo_with_gone_upstream()?;
        let scan = scan_repo(&git)?;
        assert!(
            scan.warnings.iter().any(|w| w == STALE_GONE_WARNING),
            "a gone branch must carry the staleness warning: {:?}",
            scan.warnings
        );
        assert!(scan.rows.iter().any(|r| r.flags.gone));
        Ok(())
    }

    #[test]
    fn scan_does_not_warn_without_a_gone_branch() -> Result<()> {
        let (_dir, git) = test_helpers::init_repo_with_branches()?;
        let scan = scan_repo(&git)?;
        assert!(!scan.warnings.iter().any(|w| w == STALE_GONE_WARNING));
        Ok(())
    }

    #[test]
    fn scan_skips_ignored_branches() -> Result<()> {
        let (_dir, git) = test_helpers::init_repo_with_branches()?;
        let ignored = git
            .local_branches()?
            .into_iter()
            .find(|b| b != &git.current_branch().unwrap_or_default())
            .expect("the fixture has more than one branch");
        git.config_add("wipe.ignore", &ignored)?;

        // Reload so the filter picks up the pattern just written.
        let cfg = Config::try_load(&git)?.expect("wipe.ignore makes the repo configured");
        let filter = Filter::load(&git, &cfg)?;
        let scan = scan(&git, &filter, &Ui::quiet(), opts())?;
        assert!(!names(&scan.rows).contains(&ignored.as_str()));
        Ok(())
    }

    #[test]
    fn scan_returns_rows_sorted_oldest_first() -> Result<()> {
        let (_dir, git) = test_helpers::init_repo_with_branches()?;
        let scan = scan_repo(&git)?;
        let ages: Vec<_> = scan.rows.iter().map(|r| r.age).collect();
        let known: Vec<_> = ages.iter().flatten().collect();
        assert!(
            known.windows(2).all(|w| w[0] >= w[1]),
            "ages must be non-increasing: {ages:?}"
        );
        // Unknown ages, if any, come last.
        let first_unknown = ages.iter().position(Option::is_none);
        if let Some(idx) = first_unknown {
            assert!(ages[idx..].iter().all(Option::is_none));
        }
        Ok(())
    }

    // ── Sizing ───────────────────────────────────────────────────────

    #[test]
    fn scan_skips_sizing_unless_requested() -> Result<()> {
        let (_dir, git, _wt_path) = test_helpers::init_repo_with_worktree()?;
        let filter = Filter::load(&git, &Config::default())?;
        let scan = scan(&git, &filter, &Ui::quiet(), opts())?;
        assert!(
            scan.rows.iter().all(|r| r.size.is_none()),
            "sizing must be skipped by default: {:?}",
            scan.rows
        );
        Ok(())
    }

    #[test]
    fn scan_computes_size_when_requested() -> Result<()> {
        let (_dir, git, _wt_path) = test_helpers::init_repo_with_worktree()?;
        let filter = Filter::load(&git, &Config::default())?;
        let opts = StatusOptions {
            show_size: true,
            ..opts()
        };
        let scan = scan(&git, &filter, &Ui::quiet(), opts)?;
        let wt_row = scan
            .rows
            .iter()
            .find(|r| r.kind == RowKind::Worktree)
            .expect("the fixture has a worktree row");
        assert!(wt_row.size.is_some_and(|s| s > 0));
        Ok(())
    }

    // ── JSON ─────────────────────────────────────────────────────────

    #[test]
    fn to_report_omits_action_fields() {
        let rows = vec![row(
            "done",
            Some(Duration::from_secs(60)),
            Flags {
                merged: true,
                ..Flags::default()
            },
        )];
        let doc = serde_json::to_value(to_report(&rows, Vec::new())).unwrap();
        assert_eq!(doc["version"], 1);
        assert_eq!(doc["status"], "success");
        assert_eq!(doc["entries"][0]["status"][0], "merged");
        assert_eq!(doc["entries"][0]["age_seconds"], 60);
        for absent in ["summary", "dry_run", "fetch", "pull", "local", "remotes"] {
            assert!(doc.get(absent).is_none(), "{absent} must not be reported");
        }
        assert!(doc["entries"][0].get("selected").is_none());
    }

    #[test]
    fn to_report_size_kb_is_null_when_unset_and_populated_when_set() {
        let rows = vec![
            row("no-size", None, Flags::default()),
            Row {
                size: Some(2 * 1024 * 1024),
                ..row("with-size", None, Flags::default())
            },
        ];
        let doc = serde_json::to_value(to_report(&rows, Vec::new())).unwrap();
        assert!(doc["entries"][0]["size_kb"].is_null());
        assert_eq!(doc["entries"][1]["size_kb"], 2048);
    }

    #[test]
    fn to_report_carries_the_warnings_through() {
        let doc =
            serde_json::to_value(to_report(&[], vec![STALE_GONE_WARNING.to_string()])).unwrap();
        assert_eq!(doc["warnings"][0], STALE_GONE_WARNING);
        assert_eq!(doc["entries"].as_array().unwrap().len(), 0);
    }
}
