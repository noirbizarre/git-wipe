//! The cleanup workflow: fetch, fast-forward, then delete what is merged.
//!
//! [`run`] drives the four phases described in the README and owns all the
//! interaction: it decides what to offer, asks the user once, and reports what
//! it did. Detection itself lives in [`crate::branches`] and
//! [`crate::worktrees`].

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use anyhow::Result;

use crate::branches::{
    Effort, Filter, find_gone_local, find_merged_local, find_merged_remote, resolve_merge_targets,
};
use crate::config::Config;
use crate::duration::MinAge;
use crate::forge::{ForgeSetting, ForgeSource, RemoteForges};
use crate::git::{Git, Worktree};
use crate::parallel;
use crate::pid;
use crate::report::{
    BranchReason, ItemStatus, LocalBranch, PulledBranch, RemoteBranch, RemoteFetch, RemoteReport,
    Report, StaleLock, StaleLockAction, WorktreeEntry, WorktreeKind, path_string,
};
use crate::size::{Size, format_bytes};
use crate::ui::{Ui, tilde_path};
use crate::worktrees::{find_orphan_worktrees, is_too_young, worktree_size};

/// Report a failed operation to both the user and the JSON report.
fn fail(ui: &Ui, report: &mut Report, action: &str, target: &str, err: &anyhow::Error) {
    ui.report_failure(action, target, err);
    report.push_error(action, target, err);
}

/// Emit a warning to both the user and the JSON report.
fn warn(ui: &Ui, report: &mut Report, message: &str) {
    ui.warning(message);
    report.push_warning(message);
}

/// Join fragments as `a`, `a and b`, or `a, b and c`.
fn join_with_and(parts: &[String]) -> String {
    match parts {
        [] => String::new(),
        [only] => only.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// Options controlling cleaner behaviour, derived from CLI flags.
#[derive(Debug, Clone, Default)]
pub struct CleanerOptions {
    pub yes: bool,
    /// Force-remove dirty worktrees (those with uncommitted changes). It drives
    /// the forced-removal prompt and nothing else.
    pub force: bool,
    pub dry_run: bool,
    pub no_fetch: bool,
    pub no_pull: bool,
    pub local_only: bool,
    pub remote_only: bool,
    pub no_worktrees: bool,
    pub delete_gone: bool,
    pub use_worktrunk: bool,
    /// How thorough merge detection should be.
    pub effort: Effort,
    /// Whether the forge is asked about merged pull/merge requests first.
    pub forge: ForgeSetting,
    /// Minimum age a worktree must have before it may be removed.
    pub min_age: MinAge,
    /// Minimum on-disk size a worktree must have before it may be removed.
    pub min_size: Size,
    /// Compute and show worktree sizes, without filtering or excluding
    /// anything.
    pub show_size: bool,
    /// How many read-only git probes may run at once during analysis.
    ///
    /// Purely a wall-clock knob: 0 and 1 both mean "stay on this thread", and
    /// every value produces the same output. Mutating commands — fetch, pull,
    /// deletion, worktree removal — are never affected.
    pub jobs: usize,
}

/// Run the full clean-up workflow.
///
/// Returns a structured [`Report`] of everything that was detected and done;
/// text mode discards it, `--json` serializes it to stdout.
pub fn run(git: &Git, config: &Config, ui: &Ui, opts: &CleanerOptions) -> Result<Report> {
    let mut report = Report::new(opts.dry_run, opts.effort, opts.min_age, opts.jobs.max(1));
    report.forge = opts.forge;

    // Read protection and ignore rules once; every detection pass shares them.
    let filter = Filter::load(git, config)?;

    // The forge behind each remote, identified from its URL (no network yet).
    // Shared by the local and remote scans, which also share its answers.
    let forges = opts
        .forge
        .is_enabled()
        .then(|| effective_remotes(git, config))
        .transpose()?
        .map(|remotes| RemoteForges::new(git, opts.forge, remotes));
    let forge: Option<&dyn ForgeSource> = forges.as_ref().map(|f| f as &dyn ForgeSource);

    // ── 1. Fetch & prune ─────────────────────────────────────────────

    // Whether every configured remote was refreshed this run. Deleted-upstream
    // detection is only trustworthy when remote-tracking refs are up to date.
    let mut fetch_succeeded = false;

    report.fetch.skipped = opts.no_fetch;
    if !opts.no_fetch {
        let remotes = effective_remotes(git, config)?;
        if !remotes.is_empty() {
            ui.heading(&format!(
                "Fetching and pruning {} remote(s):",
                remotes.len()
            ));
            ui.bullet_list(&remotes);
            if opts.dry_run {
                ui.dry_run("Skipping remote update.");
                for remote in &remotes {
                    report.fetch.remotes.push(RemoteFetch {
                        name: remote.clone(),
                        status: ItemStatus::DryRun,
                    });
                }
            } else {
                let mut failed: Vec<String> = Vec::new();
                let mut succeeded = 0usize;
                for remote in &remotes {
                    let result = ui.spinner(&format!("Fetching {remote}…"), || {
                        git.fetch_remote_prune(remote, &config.ignore)
                    });
                    match result {
                        Ok(prune_warning) => {
                            if let Some(message) = prune_warning {
                                warn(ui, &mut report, &message);
                            }
                            succeeded += 1;
                            ui.success(&format!("{} updated.", console::style(remote).cyan()));
                            report.fetch.remotes.push(RemoteFetch {
                                name: remote.clone(),
                                status: ItemStatus::Updated,
                            });
                        }
                        Err(e) => {
                            fail(ui, &mut report, "fetch from", remote, &e);
                            failed.push(remote.clone());
                            report.fetch.remotes.push(RemoteFetch {
                                name: remote.clone(),
                                status: ItemStatus::Failed,
                            });
                        }
                    }
                }
                if !failed.is_empty() {
                    warn(
                        ui,
                        &mut report,
                        &format!(
                            "Continuing without {} remote(s); detection results for {} may be stale.",
                            failed.len(),
                            failed.join(", ")
                        ),
                    );
                } else if succeeded > 0 {
                    fetch_succeeded = true;
                    ui.success("Remotes updated.");
                }
            }
        }
    }

    // ── 2. Pull / fast-forward target branches ─────────────────────

    report.pull.skipped = opts.no_pull;
    if !opts.no_pull {
        let targets = resolve_merge_targets(git, &filter)?;
        if !targets.is_empty() {
            let current = git.current_branch()?;
            let worktrees = git.worktree_list()?;

            // Map branch name → worktree path for branches checked out somewhere.
            let wt_map: HashMap<String, PathBuf> = worktrees
                .iter()
                .filter(|wt| !wt.is_bare)
                .filter_map(|wt| wt.branch.as_ref().map(|b| (b.clone(), wt.path.clone())))
                .collect();

            // Collect targets that have upstream tracking info.
            let mut pullable: Vec<(String, String, String)> = Vec::new(); // (branch, remote, upstream_branch)
            for target in &targets {
                if let Some((remote, upstream_branch)) = git.branch_upstream(target)? {
                    pullable.push((target.clone(), remote, upstream_branch));
                }
            }

            if pullable.is_empty() {
                ui.muted("No target branches with upstream tracking to pull.");
            } else {
                let display: Vec<String> = pullable
                    .iter()
                    .map(|(branch, remote, _)| format!("{branch} (from {remote})"))
                    .collect();
                ui.heading(&format!("Pulling {} target branch(es):", pullable.len()));
                ui.bullet_list(&display);

                for (branch, remote, upstream_branch) in &pullable {
                    if opts.dry_run {
                        ui.dry_run(&format!(
                            "Would pull '{branch}' from {remote}/{upstream_branch}."
                        ));
                        report.pull.branches.push(PulledBranch {
                            branch: branch.clone(),
                            remote: remote.clone(),
                            upstream: upstream_branch.clone(),
                            status: ItemStatus::DryRun,
                        });
                        continue;
                    }

                    let result = ui.spinner(&format!("Pulling {branch}…"), || {
                        if *branch == current {
                            // Checked out in the current working directory
                            git.pull_ff_only()
                        } else if let Some(wt_path) = wt_map.get(branch) {
                            // Checked out in another worktree
                            git.pull_ff_only_in(wt_path)
                        } else {
                            // Not checked out anywhere — fast-forward via fetch
                            git.fetch_update_branch(remote, upstream_branch, branch)
                        }
                    });

                    let status = match result {
                        Ok(()) => {
                            ui.success(&format!("{} updated.", console::style(&branch).cyan()));
                            ItemStatus::Updated
                        }
                        Err(e) => {
                            fail(ui, &mut report, "pull", branch, &e);
                            ItemStatus::Failed
                        }
                    };
                    report.pull.branches.push(PulledBranch {
                        branch: branch.clone(),
                        remote: remote.clone(),
                        upstream: upstream_branch.clone(),
                        status,
                    });
                }
            }
        }
    }

    let mut total_deleted = 0usize;

    // ── 3. Local branches & worktrees ────────────────────────────────
    //
    // Merged local branches, their associated worktrees, and orphan
    // worktrees are presented in a single unified multiselect so the
    // user confirms everything in one pass.

    report.local.skipped = opts.remote_only;
    if !opts.remote_only {
        let merged = ui.spinner("Scanning local branches…", || {
            find_merged_local(git, &filter, opts.effort, opts.jobs, forge)
        })?;
        // Surfaced after the spinner: printing inside it would corrupt the
        // spinner's own line.
        for warning in &merged.warnings {
            warn(ui, &mut report, warning);
        }
        let pr_merged = merged.pr_merged;
        let merged = merged.candidates;

        // Branches whose upstream was deleted. Only meaningful with fresh
        // remote-tracking refs, so this requires a successful fetch — except in
        // --dry-run, where nothing is deleted and an empty preview would be
        // useless.
        let gone = if fetch_succeeded || opts.dry_run {
            let gone = ui.spinner("Scanning for deleted upstreams…", || {
                find_gone_local(git, &filter, &merged)
            })?;
            if !gone.is_empty() && !fetch_succeeded {
                warn(
                    ui,
                    &mut report,
                    "Remotes were not fetched; deleted-upstream detection may be stale.",
                );
            }
            gone
        } else {
            Vec::new()
        };
        let gone_set: HashSet<String> = gone.iter().cloned().collect();

        report.local.merged = merged.clone();
        report.local.pr_merged = merged
            .iter()
            .filter(|b| pr_merged.contains(*b))
            .cloned()
            .collect();
        report.local.gone = gone.clone();

        // Everything the user may act on, in display order: content-merged
        // branches first, deleted-upstream ones after.
        let candidates: Vec<String> = merged.iter().chain(gone.iter()).cloned().collect();

        // Build a map of branch → worktree for branches that have one.
        let mut worktrees = git.worktree_list()?;

        // Resolve stale locks once, up front, over every worktree (branch
        // associated or not) so wt_map, orphans and every guard check below
        // already see them as unlocked. See `resolve_stale_lock` for what
        // "stale" means and how `--dry-run` is handled.
        let mut stale_locks: HashMap<PathBuf, StaleLock> = HashMap::new();
        for wt in &mut worktrees {
            if let Some(entry) = resolve_stale_lock(git, ui, &mut report, opts.dry_run, wt) {
                stale_locks.insert(wt.path.clone(), entry);
            }
        }

        let wt_map: HashMap<String, Worktree> = worktrees
            .iter()
            .filter(|wt| !wt.is_bare)
            .filter_map(|wt| wt.branch.as_ref().map(|b| (b.clone(), (*wt).clone())))
            .collect();

        // Collect orphan worktrees (if worktree cleanup is enabled).
        let mut orphans = if !opts.no_worktrees {
            find_orphan_worktrees(git, &filter)?
        } else {
            Vec::new()
        };
        // `find_orphan_worktrees` re-fetches the worktree list itself. In live
        // mode that fresh fetch already reflects the real unlock above; in
        // `--dry-run` nothing was actually unlocked, so patch in the same
        // decision so orphan worktrees get the same "would unlock" treatment
        // as branch-associated ones.
        for wt in &mut orphans {
            if stale_locks.contains_key(&wt.path) {
                wt.is_locked = false;
                wt.lock_reason = None;
            }
        }

        // Worktrees created too recently to touch. Resolved once, up front:
        // each lookup costs a `git rev-parse` and the answer is consulted at
        // several points below. Those probes are read-only and independent, so
        // they overlap; the result is a set, and so order-insensitive anyway.
        let young: HashSet<PathBuf> = if opts.no_worktrees || opts.min_age.is_zero() {
            HashSet::new()
        } else {
            let to_age: Vec<&Worktree> = candidates
                .iter()
                .filter_map(|branch| wt_map.get(branch))
                .chain(orphans.iter())
                .collect();
            parallel::map(&to_age, opts.jobs, |_, wt| {
                is_too_young(git, wt, opts.min_age)
            })
            .into_iter()
            .zip(&to_age)
            .filter(|(too_young, _)| *too_young)
            .map(|(_, wt)| wt.path.clone())
            .collect()
        };

        // Worktree sizes, in bytes. Resolved once, up front, the same way
        // `young` is: each measurement walks a directory tree, so it is
        // skipped entirely unless `--min-size` or `--size` asked for it.
        let need_size = !opts.no_worktrees && (opts.show_size || !opts.min_size.is_zero());
        let sizes: HashMap<PathBuf, u64> = if !need_size {
            HashMap::new()
        } else {
            let to_size: Vec<&Worktree> = candidates
                .iter()
                .filter_map(|branch| wt_map.get(branch))
                .chain(orphans.iter())
                .collect();
            parallel::map(&to_size, opts.jobs, |_, wt| worktree_size(wt))
                .into_iter()
                .zip(&to_size)
                .filter_map(|(size, wt)| size.map(|s| (wt.path.clone(), s)))
                .collect()
        };

        // Worktrees smaller than `--min-size`. Empty (and therefore free to
        // check) when the guard is disabled.
        let small: HashSet<PathBuf> = if opts.min_size.is_zero() {
            HashSet::new()
        } else {
            sizes
                .iter()
                .filter(|&(_, &bytes)| bytes < opts.min_size.as_bytes())
                .map(|(path, _)| path.clone())
                .collect()
        };

        let (orphan_guarded, orphan_actionable): (Vec<_>, Vec<_>) = orphans
            .into_iter()
            .partition(|wt| worktree_guard(wt, &young, &small).is_some());

        // Report guarded worktrees (both branch-associated and orphan).
        if !opts.no_worktrees {
            for branch in &candidates {
                if let Some(wt) = wt_map.get(branch)
                    && let Some(guard) = worktree_guard(wt, &young, &small)
                {
                    ui.muted(&guard.skip_message(wt, opts.min_age, opts.min_size));
                    report.local.worktrees.push(WorktreeEntry {
                        path: path_string(&wt.path),
                        branch: wt.branch.clone(),
                        kind: WorktreeKind::Branch,
                        status: guard.status(),
                        size_kb: sizes.get(&wt.path).map(|&b| b / 1024),
                        stale_lock: stale_locks.get(&wt.path).cloned(),
                    });
                }
            }
            for wt in &orphan_guarded {
                let guard = worktree_guard(wt, &young, &small).expect("partitioned as guarded");
                ui.muted(&guard.skip_message(wt, opts.min_age, opts.min_size));
                report.local.worktrees.push(WorktreeEntry {
                    path: path_string(&wt.path),
                    branch: wt.branch.clone(),
                    kind: WorktreeKind::Orphan,
                    status: guard.status(),
                    size_kb: sizes.get(&wt.path).map(|&b| b / 1024),
                    stale_lock: stale_locks.get(&wt.path).cloned(),
                });
            }
        }

        let has_merged = !merged.is_empty();
        let has_gone = !gone.is_empty();
        let has_orphans = !orphan_actionable.is_empty();

        // Outcome per candidate branch, filled in as the selections are
        // processed; anything left out stays `Skipped`.
        let mut branch_status: HashMap<String, ItemStatus> = HashMap::new();
        let mut selected_branches: HashSet<String> = HashSet::new();

        if !has_merged && !has_gone && !has_orphans {
            ui.muted("No merged local branches to delete.");
            if !opts.no_worktrees {
                ui.muted("No orphan worktrees to remove.");
            }
        } else {
            // --- Build the unified multiselect ---

            let mut values: Vec<String> = Vec::new();
            let mut labels: Vec<String> = Vec::new();
            let mut defaults: Vec<bool> = Vec::new();
            let mut hints: Vec<String> = Vec::new();

            // Branch candidates (with worktree path shown in label if
            // applicable). Deleted-upstream branches are listed but left
            // unchecked: the signal does not prove they were merged.
            for branch in &candidates {
                values.push(format!("branch:{branch}"));
                let is_gone = gone_set.contains(branch);
                let is_pr = pr_merged.contains(branch);
                let has_actionable_wt = wt_map
                    .get(branch)
                    .is_some_and(|wt| worktree_guard(wt, &young, &small).is_none());
                if !opts.no_worktrees && has_actionable_wt {
                    let wt = &wt_map[branch];
                    labels.push(format!("{branch} ({})", tilde_path(&wt.path)));
                    let mut hint = if is_gone {
                        "upstream gone + worktree".to_string()
                    } else if is_pr {
                        "pr-merged + worktree".to_string()
                    } else {
                        "branch + worktree".to_string()
                    };
                    if let Some(&bytes) = sizes.get(&wt.path) {
                        hint = format!("{hint}, {}", format_bytes(bytes));
                    }
                    hints.push(hint);
                } else {
                    labels.push(branch.clone());
                    hints.push(if is_gone {
                        "upstream gone".to_string()
                    } else if is_pr {
                        "pr-merged".to_string()
                    } else {
                        String::new()
                    });
                }
                defaults.push(!is_gone);
            }

            // Orphan worktrees.
            for wt in &orphan_actionable {
                values.push(format!("orphan-wt:{}", wt.path.display()));
                labels.push(tilde_path(&wt.path));
                let mut hint = format!(
                    "orphan worktree, branch: {}",
                    wt.branch.as_deref().unwrap_or("detached")
                );
                if let Some(&bytes) = sizes.get(&wt.path) {
                    hint = format!("{hint}, {}", format_bytes(bytes));
                }
                hints.push(hint);
                defaults.push(false);
            }

            // Heading.
            let mut found: Vec<String> = Vec::new();
            if has_merged {
                found.push(format!("{} merged local branch(es)", merged.len()));
            }
            if has_gone {
                found.push(format!("{} with a deleted upstream", gone.len()));
            }
            if has_orphans {
                found.push(format!("{} orphan worktree(s)", orphan_actionable.len()));
            }
            ui.heading(&format!("Found {}:", join_with_and(&found)));

            let has_branches = has_merged || has_gone;
            let mut prompt = if has_branches && has_orphans {
                "Select branches and worktrees to delete".to_string()
            } else if has_orphans {
                "Select orphan worktrees to remove".to_string()
            } else {
                "Select branches to delete".to_string()
            };
            if has_gone {
                // Explain the unchecked entries where the user reads them.
                prompt.push_str(" (deleted upstreams unchecked: not proof of a merge)");
            }

            let selected = if opts.yes {
                // Non-interactive: take everything except deleted-upstream
                // branches, which stay opt-in behind --delete-gone.
                values
                    .iter()
                    .filter(|value| {
                        opts.delete_gone
                            || !value
                                .strip_prefix("branch:")
                                .is_some_and(|b| gone_set.contains(b))
                    })
                    .cloned()
                    .collect()
            } else {
                ui.multi_select(&prompt, &values, &labels, &defaults, &hints, true)?
            };

            selected_branches = selected
                .iter()
                .filter_map(|value| value.strip_prefix("branch:").map(str::to_string))
                .collect();

            // --- Detect worktrees that would fail a plain removal ---
            //
            // For each selected merged branch whose worktree is unlocked,
            // check whether the worktree is dirty (untracked / uncommitted
            // changes) or whether the branch has commits not contained in any
            // merge target (relevant for `wt remove`, which refuses unmerged
            // branches without `--force-delete`). Orphan worktrees keep the
            // existing auto-force behavior and are not surfaced here.
            let targets_for_unmerged = if opts.use_worktrunk {
                match resolve_merge_targets(git, &filter) {
                    Ok(targets) => targets,
                    Err(e) => {
                        warn(
                            ui,
                            &mut report,
                            &format!(
                                "Could not resolve merge targets, \
                                 treating worktree branches as unmerged: {e}"
                            ),
                        );
                        Vec::new()
                    }
                }
            } else {
                Vec::new()
            };

            // (branch, force, force_delete)
            let mut problematic: Vec<(String, bool, bool)> = Vec::new();
            // Branches whose commit SHAs are not reachable from any merge
            // target (squash-merged, cherry-picked, or selected on the strength
            // of a deleted upstream). These are auto force-deleted without a
            // second prompt: the user already selected them and there is no
            // uncommitted work at risk.
            let mut auto_force: Vec<String> = Vec::new();

            // Split in two: pick the worktrees worth inspecting (no git calls,
            // so it stays here), then probe them concurrently.
            let to_scan: Vec<(&String, &Worktree)> = candidates
                .iter()
                .filter(|branch| selected.contains(&format!("branch:{branch}")))
                .filter_map(|branch| wt_map.get(branch).map(|wt| (branch, wt)))
                .filter(|(_, wt)| worktree_guard(wt, &young, &small).is_none())
                .collect();

            // Workers report, they never print: the messages are emitted below
            // in candidate order, so `--jobs` cannot reshuffle the output.
            struct Scan {
                dirty: bool,
                unmerged: bool,
                warnings: Vec<String>,
            }

            let scans = parallel::map(&to_scan, opts.jobs, |_, (branch, wt)| {
                let mut warnings = Vec::new();
                let dirty = match git.worktree_dirty(&wt.path) {
                    Ok(v) => v,
                    Err(e) => {
                        warnings.push(format!(
                            "Could not check status of '{}': {e}",
                            tilde_path(&wt.path)
                        ));
                        false
                    }
                };
                let unmerged = if opts.use_worktrunk {
                    match git.branch_has_unmerged_commits(branch, &targets_for_unmerged) {
                        Ok(v) => v,
                        Err(e) => {
                            // Assume unmerged: that only means the worktree is
                            // surfaced for explicit confirmation rather than
                            // silently force-removed.
                            warnings.push(format!(
                                "Could not check whether '{branch}' has unmerged commits: {e}"
                            ));
                            true
                        }
                    }
                } else {
                    false
                };
                Scan {
                    dirty,
                    unmerged,
                    warnings,
                }
            });

            for ((branch, _), scan) in to_scan.iter().zip(scans) {
                for warning in &scan.warnings {
                    warn(ui, &mut report, warning);
                }
                if scan.dirty {
                    // Real data-loss risk: always confirm.
                    problematic.push(((*branch).clone(), scan.dirty, scan.unmerged));
                } else if scan.unmerged {
                    // Commits not reachable from target (squash / cherry-pick /
                    // deleted upstream). Auto force-delete; no prompt.
                    auto_force.push((*branch).clone());
                }
            }

            // --- Second prompt: confirm forced removal ---
            //
            // Selected entries get the appropriate force flag(s); unselected
            // entries are skipped entirely (no worktree removal, no branch
            // deletion). `--force` drives this prompt alone: it pre-selects
            // every entry interactively, and non-interactively (`--yes`) it is
            // what decides between force-removing everything and skipping it
            // all. `--yes` on its own never destroys a dirty worktree.
            let mut force_map: HashMap<String, (bool, bool)> = HashMap::new();
            let mut skip_set: HashSet<String> = HashSet::new();
            // Auto force-delete branches whose commits are unreachable from any
            // target: no prompt, but surface the action so the user can see
            // what's happening.
            for branch in &auto_force {
                force_map.insert(branch.clone(), (false, true));
                let wt = &wt_map[branch];
                ui.muted(&format!(
                    "Auto force-deleting '{}' ({}); commits not reachable from any merge target.",
                    branch,
                    tilde_path(&wt.path),
                ));
            }
            if !problematic.is_empty() {
                let f_values: Vec<String> = problematic.iter().map(|(b, _, _)| b.clone()).collect();
                let f_labels: Vec<String> = problematic
                    .iter()
                    .map(|(b, _, _)| {
                        let wt = &wt_map[b];
                        format!("{b} ({})", tilde_path(&wt.path))
                    })
                    .collect();
                let f_hints: Vec<String> = problematic
                    .iter()
                    .map(|(_, dirty, unmerged)| match (*dirty, *unmerged) {
                        (true, true) => "dirty + unmerged commits".to_string(),
                        (true, false) => "dirty".to_string(),
                        (false, true) => "unmerged commits".to_string(),
                        _ => String::new(),
                    })
                    .collect();
                let f_defaults: Vec<bool> = vec![opts.force; problematic.len()];

                ui.heading(&format!(
                    "{} worktree(s) need forced removal:",
                    problematic.len()
                ));
                let f_selected = if opts.yes {
                    if opts.force {
                        f_values.clone()
                    } else {
                        Vec::new()
                    }
                } else {
                    ui.multi_select(
                        "Select worktrees to force-remove (unselected will be skipped)",
                        &f_values,
                        &f_labels,
                        &f_defaults,
                        &f_hints,
                        true,
                    )?
                };
                let f_selected_set: HashSet<String> = f_selected.into_iter().collect();
                for (branch, dirty, unmerged) in &problematic {
                    if f_selected_set.contains(branch) {
                        force_map.insert(branch.clone(), (*dirty, *unmerged));
                    } else {
                        let wt = &wt_map[branch];
                        skip_set.insert(branch.clone());
                        branch_status.insert(branch.clone(), ItemStatus::Skipped);
                        report.local.worktrees.push(WorktreeEntry {
                            path: path_string(&wt.path),
                            branch: Some(branch.clone()),
                            kind: WorktreeKind::Branch,
                            status: ItemStatus::Skipped,
                            size_kb: sizes.get(&wt.path).map(|&b| b / 1024),
                            stale_lock: stale_locks.get(&wt.path).cloned(),
                        });
                        warn(
                            ui,
                            &mut report,
                            &format!(
                                "Skipping '{}' ({}); worktree and branch left untouched.",
                                console::style(branch).yellow(),
                                tilde_path(&wt.path),
                            ),
                        );
                    }
                }
            }

            // --- Process the selections ---

            // Track branches whose worktree was removed via worktrunk
            // (wt deletes the branch itself, so phase 3 must skip them).
            let mut wt_handled_branches: HashSet<String> = HashSet::new();

            // 1. Remove worktrees for selected merged branches first.
            if !opts.no_worktrees {
                let mut wt_removed = 0usize;
                let mut freed_bytes: u64 = 0;
                for branch in &candidates {
                    let key = format!("branch:{branch}");
                    if !selected.contains(&key) {
                        continue;
                    }
                    if skip_set.contains(branch) {
                        continue; // already warned above
                    }
                    if let Some(wt) = wt_map.get(branch) {
                        if worktree_guard(wt, &young, &small).is_some() {
                            continue; // already reported above
                        }
                        let (force, force_delete) =
                            force_map.get(branch).copied().unwrap_or((false, false));
                        if opts.dry_run {
                            ui.dry_run(&format!("Would remove worktree '{}'.", wt.path.display()));
                            if opts.use_worktrunk {
                                wt_handled_branches.insert(branch.clone());
                            }
                            report.local.worktrees.push(WorktreeEntry {
                                path: path_string(&wt.path),
                                branch: Some(branch.clone()),
                                kind: WorktreeKind::Branch,
                                status: ItemStatus::DryRun,
                                size_kb: sizes.get(&wt.path).map(|&b| b / 1024),
                                stale_lock: stale_locks.get(&wt.path).cloned(),
                            });
                        } else {
                            let result = ui.spinner(
                                &format!("Removing worktree {}…", tilde_path(&wt.path)),
                                || {
                                    remove_worktree(
                                        git,
                                        wt,
                                        opts.use_worktrunk,
                                        force,
                                        force_delete,
                                    )
                                },
                            );
                            let status = match result {
                                Ok(()) => {
                                    wt_removed += 1;
                                    if let Some(&bytes) = sizes.get(&wt.path) {
                                        freed_bytes += bytes;
                                    }
                                    if opts.use_worktrunk {
                                        wt_handled_branches.insert(branch.clone());
                                    }
                                    ui.success(&format!(
                                        "{} removed.",
                                        console::style(tilde_path(&wt.path)).cyan(),
                                    ));
                                    ItemStatus::Removed
                                }
                                Err(e) => {
                                    fail(ui, &mut report, "remove", &tilde_path(&wt.path), &e);
                                    ItemStatus::Failed
                                }
                            };
                            report.local.worktrees.push(WorktreeEntry {
                                path: path_string(&wt.path),
                                branch: Some(branch.clone()),
                                kind: WorktreeKind::Branch,
                                status,
                                size_kb: sizes.get(&wt.path).map(|&b| b / 1024),
                                stale_lock: stale_locks.get(&wt.path).cloned(),
                            });
                        }
                    }
                }

                // 2. Remove selected orphan worktrees.
                for wt in &orphan_actionable {
                    let key = format!("orphan-wt:{}", wt.path.display());
                    if !selected.contains(&key) {
                        continue;
                    }
                    if opts.dry_run {
                        ui.dry_run(&format!("Would remove worktree '{}'.", wt.path.display()));
                        report.local.worktrees.push(WorktreeEntry {
                            path: path_string(&wt.path),
                            branch: wt.branch.clone(),
                            kind: WorktreeKind::Orphan,
                            status: ItemStatus::DryRun,
                            size_kb: sizes.get(&wt.path).map(|&b| b / 1024),
                            stale_lock: stale_locks.get(&wt.path).cloned(),
                        });
                    } else {
                        let result = ui.spinner(
                            &format!("Removing worktree {}…", tilde_path(&wt.path)),
                            || remove_worktree(git, wt, opts.use_worktrunk, true, false),
                        );
                        let status = match result {
                            Ok(()) => {
                                wt_removed += 1;
                                if let Some(&bytes) = sizes.get(&wt.path) {
                                    freed_bytes += bytes;
                                }
                                ui.success(&format!(
                                    "{} removed.",
                                    console::style(tilde_path(&wt.path)).cyan(),
                                ));
                                ItemStatus::Removed
                            }
                            Err(e) => {
                                fail(ui, &mut report, "remove", &tilde_path(&wt.path), &e);
                                ItemStatus::Failed
                            }
                        };
                        report.local.worktrees.push(WorktreeEntry {
                            path: path_string(&wt.path),
                            branch: wt.branch.clone(),
                            kind: WorktreeKind::Orphan,
                            status,
                            size_kb: sizes.get(&wt.path).map(|&b| b / 1024),
                            stale_lock: stale_locks.get(&wt.path).cloned(),
                        });
                    }
                }
                report.summary.worktrees_removed = wt_removed;
                report.summary.freed_kb = freed_bytes / 1024;
                if !opts.dry_run && wt_removed > 0 {
                    ui.summary(wt_removed, "worktree", "worktrees", "removed");
                    if freed_bytes > 0 {
                        ui.freed(freed_bytes);
                    }
                }
            }

            // 3. Delete the selected branches.
            //
            // Branches whose worktree was removed via worktrunk are normally
            // deleted by `wt remove` itself. However, `wt`'s merge check is
            // narrower than git-wipe's (it only considers the local default
            // branch / its upstream and caps its walk for speed), so it may
            // leave behind a branch git-wipe considers merged. For such
            // branches we verify the ref is actually gone and delete it
            // ourselves if it survived. Branches whose worktree the user chose
            // not to force-remove, or that a guard (locked, too young, too
            // small) left in place, are skipped.
            for branch in &candidates {
                let key = format!("branch:{branch}");
                if !selected.contains(&key) {
                    continue;
                }
                if skip_set.contains(branch) {
                    continue;
                }
                // A guarded worktree (locked, too young, too small) was left
                // in place and still has the branch checked out, so git would
                // refuse to delete it. Leave the branch alone too; it stays
                // `Skipped`, like the worktree already reported above.
                if wt_map
                    .get(branch)
                    .is_some_and(|wt| worktree_guard(wt, &young, &small).is_some())
                {
                    continue;
                }
                if opts.dry_run {
                    ui.dry_run(&format!("Would delete local branch '{branch}'."));
                    branch_status.insert(branch.clone(), ItemStatus::DryRun);
                    continue;
                }
                if wt_handled_branches.contains(branch) {
                    // `wt remove` was responsible for deleting this branch.
                    // Confirm it actually did; if the branch survives, delete
                    // it ourselves (branch_delete uses -D, and the branch is
                    // already proven merged by git-wipe).
                    match git.branch_exists(branch) {
                        Ok(false) => {
                            total_deleted += 1;
                            branch_status.insert(branch.clone(), ItemStatus::Deleted);
                            continue;
                        }
                        Ok(true) => {
                            // `wt` may have left stale worktree metadata (e.g.
                            // its cross-filesystem `git worktree remove`
                            // fallback), which blocks branch deletion with
                            // "cannot delete branch used by worktree". Prune
                            // any now-missing worktree registrations first so
                            // the -D can succeed.
                            let _ = git.worktree_prune();
                            match git.branch_delete(branch) {
                                Ok(()) => {
                                    total_deleted += 1;
                                    branch_status.insert(branch.clone(), ItemStatus::Deleted);
                                }
                                Err(e) => {
                                    fail(ui, &mut report, "delete", branch, &e);
                                    branch_status.insert(branch.clone(), ItemStatus::Failed);
                                }
                            }
                        }
                        Err(e) => {
                            fail(ui, &mut report, "verify the existence of", branch, &e);
                            branch_status.insert(branch.clone(), ItemStatus::Failed);
                        }
                    }
                    continue;
                }
                match git.branch_delete(branch) {
                    Ok(()) => {
                        total_deleted += 1;
                        branch_status.insert(branch.clone(), ItemStatus::Deleted);
                    }
                    Err(e) => {
                        fail(ui, &mut report, "delete", branch, &e);
                        branch_status.insert(branch.clone(), ItemStatus::Failed);
                    }
                }
            }
            if !opts.dry_run && total_deleted > 0 {
                ui.summary(total_deleted, "local branch", "local branches", "deleted");
            }
        }

        // Record every candidate, selected or not, with its final outcome.
        for branch in &candidates {
            report.local.branches.push(LocalBranch {
                branch: branch.clone(),
                reason: if gone_set.contains(branch) {
                    BranchReason::Gone
                } else if pr_merged.contains(branch) {
                    BranchReason::PrMerged
                } else {
                    BranchReason::Merged
                },
                selected: selected_branches.contains(branch),
                status: branch_status
                    .get(branch)
                    .copied()
                    .unwrap_or(ItemStatus::Skipped),
                worktree: wt_map.get(branch).map(|wt| path_string(&wt.path)),
            });
        }
        report.summary.local_branches_deleted = total_deleted;
    }

    // ── 4. Remote branches ───────────────────────────────────────────

    report.remotes_skipped = opts.local_only;
    if !opts.local_only {
        let remotes = effective_remotes(git, config)?;

        for remote in &remotes {
            let merged = ui.spinner(&format!("Scanning {remote}…"), || {
                find_merged_remote(git, &filter, remote, opts.effort, opts.jobs, forge)
            })?;
            // Surfaced after the spinner: printing inside it would corrupt the
            // spinner's own line.
            for warning in &merged.warnings {
                warn(ui, &mut report, warning);
            }
            let pr_merged = merged.pr_merged;
            let merged = merged.candidates;

            if merged.is_empty() {
                ui.muted(&format!("No merged remote branches on '{remote}'."));
                report.remotes.push(RemoteReport {
                    remote: remote.clone(),
                    merged: Vec::new(),
                    pr_merged: Vec::new(),
                    branches: Vec::new(),
                });
                continue;
            }

            let reason_of = |branch: &String| {
                if pr_merged.contains(branch) {
                    BranchReason::PrMerged
                } else {
                    BranchReason::Merged
                }
            };
            let display: Vec<String> = merged
                .iter()
                .map(|b| match reason_of(b) {
                    BranchReason::PrMerged => format!("{remote}/{b} (pr-merged)"),
                    _ => format!("{remote}/{b}"),
                })
                .collect();
            ui.heading(&format!(
                "Found {} merged remote branch(es) on '{remote}':",
                merged.len()
            ));
            ui.bullet_list(&display);

            let to_delete = if opts.yes {
                merged.clone()
            } else {
                let defaults: Vec<bool> = vec![true; merged.len()];
                ui.multi_select(
                    "Select branches to delete",
                    &merged,
                    &display,
                    &defaults,
                    &[],
                    true,
                )?
            };
            let selected: HashSet<&String> = to_delete.iter().collect();

            let mut remote_deleted = 0usize;
            let mut entries: Vec<RemoteBranch> = Vec::new();
            for branch in &merged {
                if !selected.contains(branch) {
                    entries.push(RemoteBranch {
                        branch: branch.clone(),
                        reason: reason_of(branch),
                        status: ItemStatus::Skipped,
                    });
                    continue;
                }
                let status = if opts.dry_run {
                    ui.dry_run(&format!("Would delete '{remote}/{branch}'."));
                    ItemStatus::DryRun
                } else {
                    let result = ui.spinner(&format!("Deleting {remote}/{branch}…"), || {
                        git.remote_branch_delete(remote, branch)
                    });
                    match result {
                        Ok(()) => {
                            remote_deleted += 1;
                            ItemStatus::Deleted
                        }
                        Err(e) => {
                            fail(ui, &mut report, "delete", &format!("{remote}/{branch}"), &e);
                            ItemStatus::Failed
                        }
                    }
                };
                entries.push(RemoteBranch {
                    branch: branch.clone(),
                    reason: reason_of(branch),
                    status,
                });
            }
            if !opts.dry_run && remote_deleted > 0 {
                ui.summary(
                    remote_deleted,
                    "remote branch",
                    "remote branches",
                    "deleted",
                );
            }
            report.summary.remote_branches_deleted += remote_deleted;
            report.remotes.push(RemoteReport {
                remote: remote.clone(),
                pr_merged: merged
                    .iter()
                    .filter(|b| pr_merged.contains(*b))
                    .cloned()
                    .collect(),
                merged,
                branches: entries,
            });
        }
    }

    // ── Done ─────────────────────────────────────────────────────────

    ui.blank();
    if opts.dry_run {
        ui.muted("Dry run complete. No changes were made.");
    } else {
        ui.success("Done.");
    }

    Ok(report)
}

/// Determine which remotes to operate on.
pub fn effective_remotes(git: &Git, config: &Config) -> Result<Vec<String>> {
    match &config.remotes {
        Some(configured) => Ok(configured.clone()),
        None => git.remotes(),
    }
}

/// Why git-wipe must leave a worktree alone.
///
/// Guarded worktrees are reported and then excluded from every subsequent
/// step: the multiselect, the dirty/unmerged scan and the removal loops. The
/// enum exists so those four sites share one decision instead of each
/// re-deriving it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WorktreeGuard {
    /// `git worktree lock` was used on it.
    Locked,
    /// It last changed less than `--min-age` ago.
    TooYoung,
    /// It is smaller than `--min-size`.
    TooSmall,
}

impl WorktreeGuard {
    /// How the guarded worktree appears in the JSON report.
    fn status(self) -> ItemStatus {
        match self {
            Self::Locked => ItemStatus::Locked,
            Self::TooYoung => ItemStatus::TooYoung,
            Self::TooSmall => ItemStatus::TooSmall,
        }
    }

    /// The informational line shown to the user.
    fn skip_message(self, wt: &Worktree, min_age: MinAge, min_size: Size) -> String {
        match self {
            Self::Locked => format_locked_skip_message(wt),
            Self::TooYoung => format_too_young_skip_message(wt, min_age),
            Self::TooSmall => format_too_small_skip_message(wt, min_size),
        }
    }
}

/// Whether `wt` is guarded, and why. `young` holds the paths of the worktrees
/// already found to be below the `--min-age` threshold; `small` holds those
/// below `--min-size`.
fn worktree_guard(
    wt: &Worktree,
    young: &HashSet<PathBuf>,
    small: &HashSet<PathBuf>,
) -> Option<WorktreeGuard> {
    if wt.is_locked {
        Some(WorktreeGuard::Locked)
    } else if young.contains(&wt.path) {
        Some(WorktreeGuard::TooYoung)
    } else if small.contains(&wt.path) {
        Some(WorktreeGuard::TooSmall)
    } else {
        None
    }
}

/// Format an informational skip message for a locked worktree.
///
/// When the reason embeds a pid that is confirmed alive, the message names
/// the still-running owner instead of the raw reason alone: a dead pid never
/// reaches this function, since [`resolve_stale_lock`] clears such locks
/// before `worktree_guard` is ever consulted.
fn format_locked_skip_message(wt: &Worktree) -> String {
    let branch_label = wt.branch.as_deref().unwrap_or("detached");
    match &wt.lock_reason {
        Some(reason) => {
            if let Some(pid) = pid::extract_pid(reason)
                && pid::pid_is_alive(pid) == Some(true)
            {
                return format!(
                    "  Skipping locked worktree '{}' (branch: {branch_label}): \
                     owner (pid {pid}) is still running ({reason}).",
                    wt.path.display()
                );
            }
            format!(
                "  Skipping locked worktree '{}' (branch: {branch_label}): {reason}",
                wt.path.display()
            )
        }
        None => {
            format!(
                "  Skipping locked worktree '{}' (branch: {branch_label}).",
                wt.path.display()
            )
        }
    }
}

/// Check whether `wt`'s lock is stale and, if so, clear it.
///
/// A lock is stale when its reason embeds a pid ([`pid::extract_pid`]) and
/// that process is confirmed dead ([`pid::pid_is_alive`] returns
/// `Some(false)`). Any other case — no reason, no pid, a live pid, or
/// liveness that could not be determined — is left untouched: the
/// conservative default, same as an opaque lock has always been.
///
/// `--dry-run` never calls `git worktree unlock`; the worktree is still
/// treated as unlocked in memory for the rest of this run (so the eventual
/// "would remove"/"would skip" reporting is accurate), but the audit trail
/// records [`StaleLockAction::WouldUnlock`] instead of
/// [`StaleLockAction::Unlocked`]. If the real unlock fails, the worktree is
/// left locked and the failure is reported like any other failed operation.
fn resolve_stale_lock(
    git: &Git,
    ui: &Ui,
    report: &mut Report,
    dry_run: bool,
    wt: &mut Worktree,
) -> Option<StaleLock> {
    if !wt.is_locked {
        return None;
    }
    let reason = wt.lock_reason.as_deref()?;
    let pid = pid::extract_pid(reason)?;
    if pid::pid_is_alive(pid) != Some(false) {
        return None; // alive, or unknown — conservative, stays locked
    }
    let reason = reason.to_string();
    let branch_label = wt.branch.as_deref().unwrap_or("detached");

    if dry_run {
        ui.dry_run(&format!(
            "Would unlock stale lock on worktree '{}' (branch: {branch_label}): \
             pid {pid} is no longer running (reason: {reason}).",
            wt.path.display()
        ));
    } else if let Err(err) = git.worktree_unlock(&wt.path) {
        fail(ui, report, "unlock", &path_string(&wt.path), &err);
        return None; // couldn't unlock: stays guarded as locked
    } else {
        ui.muted(&format!(
            "Unlocked stale lock on worktree '{}' (branch: {branch_label}): \
             pid {pid} is no longer running (reason: {reason}).",
            wt.path.display()
        ));
    }

    wt.is_locked = false;
    wt.lock_reason = None;
    Some(StaleLock {
        pid,
        reason,
        action: if dry_run {
            StaleLockAction::WouldUnlock
        } else {
            StaleLockAction::Unlocked
        },
    })
}

/// Format an informational skip message for a worktree below `--min-age`.
fn format_too_young_skip_message(wt: &Worktree, min_age: MinAge) -> String {
    let branch_label = wt.branch.as_deref().unwrap_or("detached");
    format!(
        "  Skipping recent worktree '{}' (branch: {branch_label}): changed less than {min_age} ago.",
        wt.path.display()
    )
}

/// Format an informational skip message for a worktree below `--min-size`.
fn format_too_small_skip_message(wt: &Worktree, min_size: Size) -> String {
    let branch_label = wt.branch.as_deref().unwrap_or("detached");
    format!(
        "  Skipping small worktree '{}' (branch: {branch_label}): smaller than {min_size}.",
        wt.path.display()
    )
}

/// Remove a single worktree, optionally using worktrunk to trigger hooks.
///
/// When `force` is true, passes `--force` so removal succeeds despite
/// untracked / uncommitted changes. When `force_delete` is true (worktrunk
/// only), passes `--force-delete` so `wt remove`'s branch deletion succeeds
/// even if the branch has unmerged commits.
///
/// Plain `git worktree remove` has no equivalent of `--force-delete`; its
/// `--force` flag already covers the dirty case and the subsequent
/// `git branch -D` in phase 3 handles unmerged-branch deletion.
fn remove_worktree(
    git: &Git,
    wt: &Worktree,
    use_worktrunk: bool,
    force: bool,
    force_delete: bool,
) -> Result<()> {
    if use_worktrunk {
        // `wt remove` takes a branch or a path in the same slot; fall back to
        // the path for detached-HEAD worktrees and orphans.
        let path = wt.path.to_string_lossy();
        let target = wt.branch.as_deref().unwrap_or(&path);
        git.worktrunk_remove(target, force, force_delete)
    } else {
        git.worktree_remove(&wt.path, force)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    /// Cleaner tests run their inspection concurrently, so the suite exercises
    /// the parallel paths rather than only the serial fallback.
    const TEST_JOBS: usize = 2;
    use std::process::Command;
    use tempfile::TempDir;

    /// Whether the real `wt` binary is available.
    ///
    /// Tests exercising the worktrunk code path cannot be faked, so they opt
    /// out when it is not installed (notably in CI).
    fn worktrunk_installed() -> bool {
        let available = crate::git::worktrunk_available();
        if !available {
            eprintln!("skipping: worktrunk (`wt`) not available on PATH");
        }
        available
    }

    fn default_config() -> Config {
        Config {
            protected: vec!["main".to_string()],
            ignore: Vec::new(),
            remotes: None,
            worktrunk: None,
            effort: None,
            min_age: None,
            min_size: None,
            jobs: None,
            forge: None,
        }
    }

    fn opts_yes_skip_network() -> CleanerOptions {
        CleanerOptions {
            yes: true,
            force: false,
            dry_run: false,
            no_fetch: true,
            no_pull: true,
            local_only: false,
            remote_only: false,
            no_worktrees: false,
            delete_gone: false,
            use_worktrunk: false,
            effort: Effort::Standard,
            min_age: MinAge::default(),
            min_size: Size::default(),
            show_size: false,
            jobs: TEST_JOBS,
            forge: ForgeSetting::Off,
        }
    }

    #[test]
    fn run_deletes_merged_local_branches() -> Result<()> {
        let (_dir, git) = crate::test_helpers::init_repo_with_branches()?;
        let config = default_config();
        let ui = Ui::new();
        let opts = opts_yes_skip_network();

        let branches_before = git.local_branches()?;
        assert!(branches_before.contains(&"feature/done".to_string()));
        assert!(branches_before.contains(&"feature/wip".to_string()));

        run(&git, &config, &ui, &opts)?;

        let branches_after = git.local_branches()?;
        assert!(!branches_after.contains(&"feature/done".to_string()));
        assert!(branches_after.contains(&"feature/wip".to_string()));
        assert!(branches_after.contains(&"main".to_string()));
        Ok(())
    }

    #[test]
    fn run_dry_run_preserves_branches() -> Result<()> {
        let (_dir, git) = crate::test_helpers::init_repo_with_branches()?;
        let config = default_config();
        let ui = Ui::new();
        let mut opts = opts_yes_skip_network();
        opts.dry_run = true;

        let branches_before = git.local_branches()?;

        run(&git, &config, &ui, &opts)?;

        let branches_after = git.local_branches()?;
        assert_eq!(branches_before, branches_after);
        Ok(())
    }

    #[test]
    fn run_no_merged_branches() -> Result<()> {
        let (_dir, git) = crate::test_helpers::init_repo()?;
        let config = default_config();
        let ui = Ui::new();
        let opts = opts_yes_skip_network();

        run(&git, &config, &ui, &opts)?;
        Ok(())
    }

    /// Exercise the per-remote fetch loop with a remote that cannot be
    /// reached (its URL points to a non-existent path). The fetch must
    /// fail, but the cleaner workflow must continue and return Ok.
    #[test]
    fn run_continues_when_a_remote_fetch_fails() -> Result<()> {
        let (_dir, git) = crate::test_helpers::init_repo()?;

        // Add a bogus remote whose URL cannot resolve. Fetching it will
        // fail with a non-network "Other" error, which is what we want
        // to exercise the failure branch of the loop.
        Command::new("git")
            .args(["remote", "add", "broken", "/this/path/does/not/exist.git"])
            .current_dir(_dir.path())
            .output()?;

        let config = Config {
            protected: vec!["main".to_string()],
            ignore: Vec::new(),
            remotes: Some(vec!["broken".to_string()]),
            worktrunk: None,
            effort: None,
            min_age: None,
            min_size: None,
            jobs: None,
            forge: None,
        };
        let ui = Ui::new();
        let opts = CleanerOptions {
            yes: true,
            force: false,
            dry_run: false,
            no_fetch: false,
            no_pull: true,
            local_only: true, // skip the remote-deletion phase
            remote_only: false,
            no_worktrees: true,
            delete_gone: false,
            use_worktrunk: false,
            effort: Effort::Standard,
            min_age: MinAge::default(),
            min_size: Size::default(),
            show_size: false,
            jobs: TEST_JOBS,
            forge: ForgeSetting::Off,
        };

        // The fetch will fail but the cleaner should not bail out.
        run(&git, &config, &ui, &opts)?;
        Ok(())
    }

    #[test]
    fn run_remote_only_skips_local_deletion() -> Result<()> {
        let (_dir, git) = crate::test_helpers::init_repo_with_branches()?;
        let config = default_config();
        let ui = Ui::new();
        let mut opts = opts_yes_skip_network();
        opts.remote_only = true;

        run(&git, &config, &ui, &opts)?;

        let branches = git.local_branches()?;
        assert!(branches.contains(&"feature/done".to_string()));
        Ok(())
    }

    #[test]
    fn run_local_only_skips_remote_deletion() -> Result<()> {
        let (_dir, git) = crate::test_helpers::init_repo_with_branches()?;
        let config = default_config();
        let ui = Ui::new();
        let mut opts = opts_yes_skip_network();
        opts.local_only = true;

        run(&git, &config, &ui, &opts)?;

        let branches = git.local_branches()?;
        assert!(!branches.contains(&"feature/done".to_string()));
        Ok(())
    }

    #[test]
    fn run_no_worktrees_skips_worktree_cleanup() -> Result<()> {
        let (_dir, git) = crate::test_helpers::init_repo_with_branches()?;
        let config = default_config();
        let ui = Ui::new();
        let mut opts = opts_yes_skip_network();
        opts.no_worktrees = true;

        run(&git, &config, &ui, &opts)?;

        let branches = git.local_branches()?;
        assert!(!branches.contains(&"feature/done".to_string()));
        Ok(())
    }

    /// Options for the gone-upstream fixture: a real fetch is required for
    /// deleted-upstream detection, and the remote phase is skipped so the test
    /// only exercises local deletion.
    fn opts_yes_with_fetch() -> CleanerOptions {
        CleanerOptions {
            yes: true,
            no_fetch: false,
            no_pull: true,
            local_only: true,
            ..opts_yes_skip_network()
        }
    }

    #[test]
    fn run_keeps_gone_branch_without_delete_gone() -> Result<()> {
        let (_dir, git) = crate::test_helpers::init_repo_with_gone_upstream()?;
        let opts = opts_yes_with_fetch();

        run(&git, &default_config(), &Ui::new(), &opts)?;

        let branches = git.local_branches()?;
        assert!(
            branches.contains(&"feature/gone".to_string()),
            "a deleted upstream alone must not trigger deletion under --yes"
        );
        assert!(
            !branches.contains(&"feature/done".to_string()),
            "content-merged branches are still deleted"
        );
        Ok(())
    }

    #[test]
    fn run_delete_gone_removes_branch_with_deleted_upstream() -> Result<()> {
        let (_dir, git) = crate::test_helpers::init_repo_with_gone_upstream()?;
        let mut opts = opts_yes_with_fetch();
        opts.delete_gone = true;

        run(&git, &default_config(), &Ui::new(), &opts)?;

        let branches = git.local_branches()?;
        assert!(!branches.contains(&"feature/gone".to_string()));
        assert!(
            branches.contains(&"feature/alive".to_string()),
            "branches whose upstream still exists are untouched"
        );
        assert!(branches.contains(&"main".to_string()));
        Ok(())
    }

    #[test]
    fn run_no_fetch_disables_gone_detection() -> Result<()> {
        let (_dir, git) = crate::test_helpers::init_repo_with_gone_upstream()?;
        let mut opts = opts_yes_with_fetch();
        opts.no_fetch = true;
        opts.delete_gone = true;

        run(&git, &default_config(), &Ui::new(), &opts)?;

        let branches = git.local_branches()?;
        assert!(
            branches.contains(&"feature/gone".to_string()),
            "stale remote-tracking refs must not be trusted"
        );
        Ok(())
    }

    #[test]
    fn run_dry_run_preserves_gone_branch() -> Result<()> {
        let (_dir, git) = crate::test_helpers::init_repo_with_gone_upstream()?;
        let mut opts = opts_yes_with_fetch();
        opts.delete_gone = true;
        opts.dry_run = true;

        run(&git, &default_config(), &Ui::new(), &opts)?;

        assert!(git.local_branches()?.contains(&"feature/gone".to_string()));
        Ok(())
    }

    #[test]
    fn run_delete_gone_removes_worktree_and_branch() -> Result<()> {
        let (dir, git) = crate::test_helpers::init_repo_with_gone_upstream()?;
        let work = dir.path().join("work");
        let wt_path = dir.path().join("wt-gone");
        Command::new("git")
            .args(["worktree", "add", wt_path.to_str().unwrap(), "feature/gone"])
            .current_dir(&work)
            .output()?;
        assert!(wt_path.exists());

        let mut opts = opts_yes_with_fetch();
        opts.delete_gone = true;

        run(&git, &default_config(), &Ui::new(), &opts)?;

        assert!(!wt_path.exists(), "worktree should be removed");
        assert!(!git.local_branches()?.contains(&"feature/gone".to_string()));
        Ok(())
    }

    #[test]
    fn effective_remotes_uses_config() -> Result<()> {
        let (_dir, git) = crate::test_helpers::init_repo()?;

        let config_with = Config {
            protected: vec!["main".to_string()],
            ignore: Vec::new(),
            remotes: Some(vec!["origin".to_string(), "upstream".to_string()]),
            worktrunk: None,
            effort: None,
            min_age: None,
            min_size: None,
            jobs: None,
            forge: None,
        };
        let remotes = effective_remotes(&git, &config_with)?;
        assert_eq!(remotes, vec!["origin", "upstream"]);

        let config_without = Config {
            protected: vec!["main".to_string()],
            ignore: Vec::new(),
            remotes: None,
            worktrunk: None,
            effort: None,
            min_age: None,
            min_size: None,
            jobs: None,
            forge: None,
        };
        let remotes = effective_remotes(&git, &config_without)?;
        assert!(remotes.is_empty());
        Ok(())
    }

    #[test]
    fn run_with_worktree_for_merged_branch() -> Result<()> {
        let (dir, _git) = crate::test_helpers::init_repo()?;
        let path = dir.path();

        // Create and merge a branch
        Command::new("git")
            .args(["checkout", "-b", "feature/wt-test"])
            .current_dir(path)
            .output()?;
        std::fs::write(path.join("wt.txt"), "worktree test")?;
        Command::new("git")
            .args(["add", "."])
            .current_dir(path)
            .output()?;
        Command::new("git")
            .args(["commit", "-m", "worktree feature"])
            .current_dir(path)
            .output()?;
        Command::new("git")
            .args(["checkout", "main"])
            .current_dir(path)
            .output()?;
        Command::new("git")
            .args(["merge", "feature/wt-test"])
            .current_dir(path)
            .output()?;

        // Create a worktree for the merged branch
        let wt_path = path.join("wt-feature");
        Command::new("git")
            .args([
                "worktree",
                "add",
                wt_path.to_str().unwrap(),
                "feature/wt-test",
            ])
            .current_dir(path)
            .output()?;

        let git = Git::with_workdir(false, path);
        let config = default_config();
        let ui = Ui::new();
        let opts = opts_yes_skip_network();

        run(&git, &config, &ui, &opts)?;

        let branches = git.local_branches()?;
        assert!(!branches.contains(&"feature/wt-test".to_string()));
        assert!(!wt_path.exists());
        Ok(())
    }

    #[test]
    fn run_skips_locked_worktree() -> Result<()> {
        let (_dir, git, wt_path) = crate::test_helpers::init_repo_with_locked_worktree()?;
        let config = default_config();
        let ui = Ui::new();
        let opts = opts_yes_skip_network();

        let report = run(&git, &config, &ui, &opts)?;

        // The locked worktree directory should still exist
        assert!(
            std::path::Path::new(&wt_path).exists(),
            "locked worktree should not be removed"
        );

        // The branch is still checked out in the locked worktree, so it is
        // left alone along with the worktree rather than failing to delete.
        let branches = git.local_branches()?;
        assert!(
            branches.contains(&"feature/locked-wt".to_string()),
            "branch should survive because its locked worktree is kept"
        );
        assert!(
            report.errors.is_empty(),
            "skipping the branch must not record a failed deletion, got {:?}",
            report.errors
        );
        let entry = report
            .local
            .branches
            .iter()
            .find(|b| b.branch == "feature/locked-wt")
            .expect("branch is still reported");
        assert_eq!(entry.status, ItemStatus::Skipped);
        Ok(())
    }

    #[test]
    fn format_locked_skip_message_no_reason() {
        let wt = Worktree {
            path: PathBuf::from("/tmp/wt"),
            branch: Some("feature/x".to_string()),
            is_bare: false,
            is_locked: true,
            lock_reason: None,
        };
        let msg = format_locked_skip_message(&wt);
        assert!(msg.contains("Skipping locked worktree"));
        assert!(msg.contains("/tmp/wt"));
        assert!(msg.contains("feature/x"));
    }

    #[test]
    fn format_locked_skip_message_with_reason() {
        let wt = Worktree {
            path: PathBuf::from("/tmp/wt"),
            branch: Some("feature/x".to_string()),
            is_bare: false,
            is_locked: true,
            lock_reason: Some("do not touch".to_string()),
        };
        let msg = format_locked_skip_message(&wt);
        assert!(msg.contains("Skipping locked worktree"));
        assert!(msg.contains("/tmp/wt"));
        assert!(msg.contains("feature/x"));
        assert!(msg.contains("do not touch"));
    }

    #[test]
    fn format_too_young_skip_message_mentions_the_threshold() {
        let wt = Worktree {
            path: PathBuf::from("/tmp/wt"),
            branch: Some("feature/x".to_string()),
            is_bare: false,
            is_locked: false,
            lock_reason: None,
        };
        let msg = format_too_young_skip_message(&wt, "2h".parse().unwrap());
        assert!(msg.contains("Skipping recent worktree"));
        assert!(msg.contains("/tmp/wt"));
        assert!(msg.contains("feature/x"));
        assert!(msg.contains("less than 2h ago"));
    }

    #[test]
    fn format_too_small_skip_message_mentions_the_threshold() {
        let wt = Worktree {
            path: PathBuf::from("/tmp/wt"),
            branch: Some("feature/x".to_string()),
            is_bare: false,
            is_locked: false,
            lock_reason: None,
        };
        let msg = format_too_small_skip_message(&wt, "100M".parse().unwrap());
        assert!(msg.contains("Skipping small worktree"));
        assert!(msg.contains("/tmp/wt"));
        assert!(msg.contains("feature/x"));
        assert!(msg.contains("smaller than 100M"));
    }

    #[test]
    fn worktree_guard_prefers_locked_over_too_young() {
        let mut wt = Worktree {
            path: PathBuf::from("/tmp/wt"),
            branch: None,
            is_bare: false,
            is_locked: true,
            lock_reason: None,
        };
        let young: HashSet<PathBuf> = [wt.path.clone()].into_iter().collect();
        let empty: HashSet<PathBuf> = HashSet::new();

        assert_eq!(
            worktree_guard(&wt, &young, &empty),
            Some(WorktreeGuard::Locked),
            "a lock is the more specific reason and should win"
        );

        wt.is_locked = false;
        assert_eq!(
            worktree_guard(&wt, &young, &empty),
            Some(WorktreeGuard::TooYoung)
        );
        assert_eq!(worktree_guard(&wt, &HashSet::new(), &empty), None);
    }

    #[test]
    fn worktree_guard_detects_too_small() {
        let wt = Worktree {
            path: PathBuf::from("/tmp/wt"),
            branch: None,
            is_bare: false,
            is_locked: false,
            lock_reason: None,
        };
        let small: HashSet<PathBuf> = [wt.path.clone()].into_iter().collect();

        assert_eq!(
            worktree_guard(&wt, &HashSet::new(), &small),
            Some(WorktreeGuard::TooSmall)
        );
        assert_eq!(worktree_guard(&wt, &HashSet::new(), &HashSet::new()), None);
    }

    #[test]
    fn format_locked_skip_message_mentions_a_live_owner() {
        let wt = Worktree {
            path: PathBuf::from("/tmp/wt"),
            branch: Some("feature/x".to_string()),
            is_bare: false,
            is_locked: true,
            lock_reason: Some(format!("pid={}", std::process::id())),
        };
        let msg = format_locked_skip_message(&wt);
        assert!(msg.contains("Skipping locked worktree"));
        assert!(
            msg.contains("is still running"),
            "message should name the still-running owner: {msg:?}"
        );
    }

    #[test]
    fn resolve_stale_lock_leaves_an_opaque_reason_locked() {
        let (_dir, git) = crate::test_helpers::init_repo().unwrap();
        let ui = Ui::new();
        let mut report = Report::new(false, Effort::default(), MinAge::default(), 1);
        let mut wt = Worktree {
            path: PathBuf::from("/tmp/wt"),
            branch: Some("feature/x".to_string()),
            is_bare: false,
            is_locked: true,
            lock_reason: Some("do not touch".to_string()),
        };

        let result = resolve_stale_lock(&git, &ui, &mut report, false, &mut wt);

        assert!(result.is_none());
        assert!(wt.is_locked, "an opaque reason must stay locked");
    }

    #[test]
    fn resolve_stale_lock_leaves_a_live_pid_locked() {
        let (_dir, git) = crate::test_helpers::init_repo().unwrap();
        let ui = Ui::new();
        let mut report = Report::new(false, Effort::default(), MinAge::default(), 1);
        let mut wt = Worktree {
            path: PathBuf::from("/tmp/wt"),
            branch: Some("feature/x".to_string()),
            is_bare: false,
            is_locked: true,
            lock_reason: Some(format!("pid={}", std::process::id())),
        };

        let result = resolve_stale_lock(&git, &ui, &mut report, false, &mut wt);

        assert!(result.is_none());
        assert!(wt.is_locked, "a live owner must stay locked");
    }

    #[test]
    fn resolve_stale_lock_unlocks_a_dead_pid() -> Result<()> {
        let (_dir, git, wt_path) = crate::test_helpers::init_repo_with_stale_locked_worktree()?;
        let ui = Ui::new();
        let mut report = Report::new(false, Effort::default(), MinAge::default(), 1);
        let mut wt = git
            .worktree_list()?
            .into_iter()
            .find(|wt| wt.path.ends_with("worktree-stale-locked"))
            .expect("the fixture worktree must be listed");
        assert!(wt.is_locked, "fixture worktree should start locked");

        let result = resolve_stale_lock(&git, &ui, &mut report, false, &mut wt);

        let entry = result.expect("a dead pid must be recognised as stale");
        assert_eq!(entry.pid, crate::test_helpers::DEAD_PID);
        assert_eq!(entry.action, StaleLockAction::Unlocked);
        assert!(!wt.is_locked);
        assert!(wt.lock_reason.is_none());

        // The real git state must be unlocked too.
        let after = git.worktree_list()?;
        assert!(
            after
                .iter()
                .any(|wt| wt.path.ends_with("worktree-stale-locked") && !wt.is_locked),
            "the real worktree lock should have been removed"
        );

        let _ = wt_path;
        Ok(())
    }

    #[test]
    fn resolve_stale_lock_dry_run_does_not_unlock() -> Result<()> {
        let (_dir, git, wt_path) = crate::test_helpers::init_repo_with_stale_locked_worktree()?;
        let ui = Ui::new();
        let mut report = Report::new(true, Effort::default(), MinAge::default(), 1);
        let mut wt = git
            .worktree_list()?
            .into_iter()
            .find(|wt| wt.path.ends_with("worktree-stale-locked"))
            .expect("the fixture worktree must be listed");

        let result = resolve_stale_lock(&git, &ui, &mut report, true, &mut wt);

        let entry = result.expect("a dead pid must be recognised as stale");
        assert_eq!(entry.action, StaleLockAction::WouldUnlock);
        assert!(
            !wt.is_locked,
            "in-memory copy is treated as unlocked so the rest of the dry run reports accurately"
        );

        // The real git state must be untouched.
        let after = git.worktree_list()?;
        assert!(
            after
                .iter()
                .any(|wt| wt.path.ends_with("worktree-stale-locked") && wt.is_locked),
            "--dry-run must never actually unlock"
        );

        let _ = wt_path;
        Ok(())
    }

    /// Repo with `feature/merged` merged into `main` and a worktree on it.
    /// The worktree is brand new, so any non-zero `--min-age` protects it.
    fn init_repo_with_merged_worktree(name: &str) -> Result<(TempDir, Git, PathBuf)> {
        let (dir, _git) = crate::test_helpers::init_repo()?;
        let path = dir.path().to_path_buf();

        for args in [
            vec!["checkout", "-b", "feature/merged"],
            vec!["commit", "--allow-empty", "-m", "merged work"],
            vec!["checkout", "main"],
            vec!["merge", "feature/merged", "--no-edit"],
        ] {
            Command::new("git")
                .args(&args)
                .current_dir(&path)
                .output()?;
        }

        let wt_path = path.join(name);
        Command::new("git")
            .args([
                "worktree",
                "add",
                wt_path.to_str().unwrap(),
                "feature/merged",
            ])
            .current_dir(&path)
            .output()?;

        let git = Git::with_workdir(false, &path);
        Ok((dir, git, wt_path))
    }

    #[test]
    fn run_skips_young_worktree() -> Result<()> {
        let (_dir, git, wt_path) = init_repo_with_merged_worktree("wt-young")?;

        let opts = CleanerOptions {
            min_age: "1h".parse()?,
            ..opts_yes_skip_network()
        };
        run(&git, &default_config(), &Ui::new(), &opts)?;

        assert!(
            wt_path.exists(),
            "a worktree created seconds ago must survive --min-age 1h"
        );
        Ok(())
    }

    #[test]
    fn run_removes_worktree_with_zero_min_age() -> Result<()> {
        let (_dir, git, wt_path) = init_repo_with_merged_worktree("wt-old-enough")?;

        // The default: the guard is disabled and behaviour is unchanged.
        run(
            &git,
            &default_config(),
            &Ui::new(),
            &opts_yes_skip_network(),
        )?;

        assert!(!wt_path.exists(), "the worktree should have been removed");
        Ok(())
    }

    #[test]
    fn run_reports_a_young_worktree_as_too_young() -> Result<()> {
        let (_dir, git, wt_path) = init_repo_with_merged_worktree("wt-young-report")?;

        let opts = CleanerOptions {
            min_age: "1h".parse()?,
            ..opts_yes_skip_network()
        };
        let report = run(&git, &default_config(), &Ui::new(), &opts)?;

        // Matched on the branch, not the path: macOS resolves the temp dir
        // through /private, so git reports a different string than the fixture
        // holds.
        let entry = report
            .local
            .worktrees
            .iter()
            .find(|w| w.branch.as_deref() == Some("feature/merged"))
            .expect("the young worktree should be reported");
        assert_eq!(entry.status, ItemStatus::TooYoung);
        assert_eq!(entry.kind, WorktreeKind::Branch);
        assert!(wt_path.exists());
        assert_eq!(report.min_age, "1h".parse()?);
        assert_eq!(report.summary.worktrees_removed, 0);
        Ok(())
    }

    #[test]
    fn run_skips_young_orphan_worktree() -> Result<()> {
        let (dir, _git, wt_path) = init_repo_with_merged_worktree("wt-young-orphan")?;
        let path = dir.path();

        // Drop the branch ref so the worktree becomes an orphan.
        Command::new("git")
            .args(["update-ref", "-d", "refs/heads/feature/merged"])
            .current_dir(path)
            .output()?;

        let git = Git::with_workdir(false, path);
        let opts = CleanerOptions {
            min_age: "1h".parse()?,
            ..opts_yes_skip_network()
        };
        let report = run(&git, &default_config(), &Ui::new(), &opts)?;

        assert!(wt_path.exists(), "young orphan worktree should survive");
        let entry = report
            .local
            .worktrees
            .iter()
            .find(|w| w.branch.as_deref() == Some("feature/merged"))
            .expect("the young orphan should be reported");
        assert_eq!(entry.status, ItemStatus::TooYoung);
        assert_eq!(entry.kind, WorktreeKind::Orphan);
        Ok(())
    }

    #[test]
    fn run_ignores_min_age_when_worktree_cleanup_is_disabled() -> Result<()> {
        let (_dir, git, wt_path) = init_repo_with_merged_worktree("wt-no-worktrees")?;

        let opts = CleanerOptions {
            min_age: "1h".parse()?,
            no_worktrees: true,
            ..opts_yes_skip_network()
        };
        let report = run(&git, &default_config(), &Ui::new(), &opts)?;

        // --no-worktrees leaves worktrees alone entirely, and nothing is
        // reported about them: the guard never runs.
        assert!(wt_path.exists());
        assert!(report.local.worktrees.is_empty());
        Ok(())
    }

    #[test]
    fn run_handles_orphan_worktrees() -> Result<()> {
        let (dir, _git) = crate::test_helpers::init_repo()?;
        let path = dir.path();

        // Create a branch and a worktree for it
        Command::new("git")
            .args(["checkout", "-b", "feature/orphan"])
            .current_dir(path)
            .output()?;
        std::fs::write(path.join("orphan.txt"), "orphan")?;
        Command::new("git")
            .args(["add", "."])
            .current_dir(path)
            .output()?;
        Command::new("git")
            .args(["commit", "-m", "orphan feature"])
            .current_dir(path)
            .output()?;
        Command::new("git")
            .args(["checkout", "main"])
            .current_dir(path)
            .output()?;
        Command::new("git")
            .args(["merge", "feature/orphan"])
            .current_dir(path)
            .output()?;

        let wt_path = path.join("wt-orphan");
        Command::new("git")
            .args([
                "worktree",
                "add",
                wt_path.to_str().unwrap(),
                "feature/orphan",
            ])
            .current_dir(path)
            .output()?;

        // Delete the branch ref, making the worktree orphaned
        Command::new("git")
            .args(["update-ref", "-d", "refs/heads/feature/orphan"])
            .current_dir(path)
            .output()?;

        let git = Git::with_workdir(false, path);
        let config = default_config();
        let ui = Ui::new();
        let opts = opts_yes_skip_network();

        // The run should complete without error — even though `git worktree
        // remove` fails (the worktree appears dirty after its branch ref is
        // deleted), the error is caught and logged as a warning.
        run(&git, &config, &ui, &opts)?;
        Ok(())
    }

    #[test]
    fn run_removes_clean_orphan_worktree() -> Result<()> {
        let (dir, _git) = crate::test_helpers::init_repo()?;
        let path = dir.path();

        // Create a branch at the same commit as main (no diverging content)
        Command::new("git")
            .args(["branch", "feature/clean-orphan"])
            .current_dir(path)
            .output()?;

        let wt_path = path.join("wt-clean-orphan");
        Command::new("git")
            .args([
                "worktree",
                "add",
                wt_path.to_str().unwrap(),
                "feature/clean-orphan",
            ])
            .current_dir(path)
            .output()?;

        // Delete the branch ref, making the worktree orphaned.
        // Since the worktree content matches the commit, it is clean
        // and `git worktree remove` will succeed.
        Command::new("git")
            .args(["update-ref", "-d", "refs/heads/feature/clean-orphan"])
            .current_dir(path)
            .output()?;

        assert!(wt_path.exists(), "worktree should exist before cleanup");

        let git = Git::with_workdir(false, path);
        let config = default_config();
        let ui = Ui::new();
        let opts = opts_yes_skip_network();

        run(&git, &config, &ui, &opts)?;

        assert!(!wt_path.exists(), "clean orphan worktree should be removed");
        Ok(())
    }

    #[test]
    fn run_skips_locked_orphan_worktree() -> Result<()> {
        let (dir, _git) = crate::test_helpers::init_repo()?;
        let path = dir.path();

        // Create a branch and worktree, then merge the branch
        Command::new("git")
            .args(["checkout", "-b", "feature/locked-orphan"])
            .current_dir(path)
            .output()?;
        std::fs::write(path.join("locked-orphan.txt"), "locked orphan")?;
        Command::new("git")
            .args(["add", "."])
            .current_dir(path)
            .output()?;
        Command::new("git")
            .args(["commit", "-m", "locked orphan feature"])
            .current_dir(path)
            .output()?;
        Command::new("git")
            .args(["checkout", "main"])
            .current_dir(path)
            .output()?;
        Command::new("git")
            .args(["merge", "feature/locked-orphan"])
            .current_dir(path)
            .output()?;

        let wt_path = path.join("wt-locked-orphan");
        Command::new("git")
            .args([
                "worktree",
                "add",
                wt_path.to_str().unwrap(),
                "feature/locked-orphan",
            ])
            .current_dir(path)
            .output()?;

        // Lock the worktree
        Command::new("git")
            .args(["worktree", "lock", wt_path.to_str().unwrap()])
            .current_dir(path)
            .output()?;

        // Delete the branch ref, making the worktree orphaned
        Command::new("git")
            .args(["update-ref", "-d", "refs/heads/feature/locked-orphan"])
            .current_dir(path)
            .output()?;

        let git = Git::with_workdir(false, path);
        let config = default_config();
        let ui = Ui::new();
        let opts = opts_yes_skip_network();

        run(&git, &config, &ui, &opts)?;

        // The locked orphan worktree should still exist
        assert!(
            wt_path.exists(),
            "locked orphan worktree should not be removed"
        );
        Ok(())
    }

    #[test]
    fn run_removes_multiple_worktrees_with_merged_branches() -> Result<()> {
        let (dir, _git) = crate::test_helpers::init_repo()?;
        let path = dir.path();

        // Create and merge two branches, each with a worktree
        for name in &["feature/wt-a", "feature/wt-b"] {
            Command::new("git")
                .args(["checkout", "-b", name])
                .current_dir(path)
                .output()?;
            std::fs::write(path.join(format!("{}.txt", name.replace('/', "-"))), name)?;
            Command::new("git")
                .args(["add", "."])
                .current_dir(path)
                .output()?;
            Command::new("git")
                .args(["commit", "-m", &format!("{name} feature")])
                .current_dir(path)
                .output()?;
            Command::new("git")
                .args(["checkout", "main"])
                .current_dir(path)
                .output()?;
            Command::new("git")
                .args(["merge", name])
                .current_dir(path)
                .output()?;
        }

        let wt_a = path.join("wt-a");
        Command::new("git")
            .args(["worktree", "add", wt_a.to_str().unwrap(), "feature/wt-a"])
            .current_dir(path)
            .output()?;

        let wt_b = path.join("wt-b");
        Command::new("git")
            .args(["worktree", "add", wt_b.to_str().unwrap(), "feature/wt-b"])
            .current_dir(path)
            .output()?;

        let git = Git::with_workdir(false, path);
        let config = default_config();
        let ui = Ui::new();
        let opts = opts_yes_skip_network();

        // Both worktrees and branches should exist before cleanup
        assert!(wt_a.exists());
        assert!(wt_b.exists());
        let branches_before = git.local_branches()?;
        assert!(branches_before.contains(&"feature/wt-a".to_string()));
        assert!(branches_before.contains(&"feature/wt-b".to_string()));

        run(&git, &config, &ui, &opts)?;

        // Both worktrees should be removed along with the branches
        assert!(!wt_a.exists(), "worktree A should be removed");
        assert!(!wt_b.exists(), "worktree B should be removed");
        let branches_after = git.local_branches()?;
        assert!(!branches_after.contains(&"feature/wt-a".to_string()));
        assert!(!branches_after.contains(&"feature/wt-b".to_string()));
        assert!(branches_after.contains(&"main".to_string()));
        Ok(())
    }

    #[test]
    fn run_dry_run_preserves_worktrees() -> Result<()> {
        let (dir, _git) = crate::test_helpers::init_repo()?;
        let path = dir.path();

        // Create and merge a branch with a worktree
        Command::new("git")
            .args(["checkout", "-b", "feature/wt-dry"])
            .current_dir(path)
            .output()?;
        std::fs::write(path.join("dry.txt"), "dry run")?;
        Command::new("git")
            .args(["add", "."])
            .current_dir(path)
            .output()?;
        Command::new("git")
            .args(["commit", "-m", "dry run feature"])
            .current_dir(path)
            .output()?;
        Command::new("git")
            .args(["checkout", "main"])
            .current_dir(path)
            .output()?;
        Command::new("git")
            .args(["merge", "feature/wt-dry"])
            .current_dir(path)
            .output()?;

        let wt_path = path.join("wt-dry");
        Command::new("git")
            .args([
                "worktree",
                "add",
                wt_path.to_str().unwrap(),
                "feature/wt-dry",
            ])
            .current_dir(path)
            .output()?;

        let git = Git::with_workdir(false, path);
        let config = default_config();
        let ui = Ui::new();
        let mut opts = opts_yes_skip_network();
        opts.dry_run = true;

        run(&git, &config, &ui, &opts)?;

        // Dry run should preserve both worktree and branch
        assert!(wt_path.exists(), "worktree should survive dry run");
        let branches = git.local_branches()?;
        assert!(
            branches.contains(&"feature/wt-dry".to_string()),
            "branch should survive dry run"
        );
        Ok(())
    }

    #[test]
    fn run_unified_cleanup_branches_and_orphan_worktrees() -> Result<()> {
        // This test verifies that merged branches (with worktrees) and orphan
        // worktrees are all cleaned up in a single unified pass (no separate
        // orphan worktree phase).
        let (dir, _git) = crate::test_helpers::init_repo()?;
        let path = dir.path();

        // Create a merged branch with a worktree
        Command::new("git")
            .args(["checkout", "-b", "feature/with-wt"])
            .current_dir(path)
            .output()?;
        std::fs::write(path.join("with-wt.txt"), "branch with worktree")?;
        Command::new("git")
            .args(["add", "."])
            .current_dir(path)
            .output()?;
        Command::new("git")
            .args(["commit", "-m", "feature with worktree"])
            .current_dir(path)
            .output()?;
        Command::new("git")
            .args(["checkout", "main"])
            .current_dir(path)
            .output()?;
        Command::new("git")
            .args(["merge", "feature/with-wt"])
            .current_dir(path)
            .output()?;

        let branch_wt_path = path.join("wt-branch");
        Command::new("git")
            .args([
                "worktree",
                "add",
                branch_wt_path.to_str().unwrap(),
                "feature/with-wt",
            ])
            .current_dir(path)
            .output()?;

        // Create a branch with a worktree, then orphan it by deleting the branch ref
        Command::new("git")
            .args(["branch", "feature/orphan-wt"])
            .current_dir(path)
            .output()?;

        let orphan_wt_path = path.join("wt-orphan");
        Command::new("git")
            .args([
                "worktree",
                "add",
                orphan_wt_path.to_str().unwrap(),
                "feature/orphan-wt",
            ])
            .current_dir(path)
            .output()?;

        // Delete the branch ref to make the worktree orphaned
        Command::new("git")
            .args(["update-ref", "-d", "refs/heads/feature/orphan-wt"])
            .current_dir(path)
            .output()?;

        // Verify setup
        assert!(branch_wt_path.exists(), "branch worktree should exist");
        assert!(orphan_wt_path.exists(), "orphan worktree should exist");
        let branches_before = Git::with_workdir(false, path).local_branches()?;
        assert!(branches_before.contains(&"feature/with-wt".to_string()));

        let git = Git::with_workdir(false, path);
        let config = default_config();
        let ui = Ui::new();
        let opts = opts_yes_skip_network();

        run(&git, &config, &ui, &opts)?;

        // The merged branch and its worktree should be removed
        assert!(
            !branch_wt_path.exists(),
            "branch worktree should be removed"
        );
        let branches_after = git.local_branches()?;
        assert!(
            !branches_after.contains(&"feature/with-wt".to_string()),
            "merged branch should be deleted"
        );

        // The orphan worktree should also be removed (unified with the branch pass)
        assert!(
            !orphan_wt_path.exists(),
            "orphan worktree should be removed in the unified pass"
        );

        Ok(())
    }

    #[test]
    fn run_no_worktrees_skips_orphan_cleanup() -> Result<()> {
        // When --no-worktrees is set, orphan worktrees should not be touched
        // even though they share the same phase as branch deletion.
        let (dir, _git) = crate::test_helpers::init_repo()?;
        let path = dir.path();

        // Create a branch with a worktree, then orphan it
        Command::new("git")
            .args(["branch", "feature/orphan-skip"])
            .current_dir(path)
            .output()?;

        let orphan_wt_path = path.join("wt-orphan-skip");
        Command::new("git")
            .args([
                "worktree",
                "add",
                orphan_wt_path.to_str().unwrap(),
                "feature/orphan-skip",
            ])
            .current_dir(path)
            .output()?;

        Command::new("git")
            .args(["update-ref", "-d", "refs/heads/feature/orphan-skip"])
            .current_dir(path)
            .output()?;

        assert!(orphan_wt_path.exists(), "orphan worktree should exist");

        let git = Git::with_workdir(false, path);
        let config = default_config();
        let ui = Ui::new();
        let mut opts = opts_yes_skip_network();
        opts.no_worktrees = true;

        run(&git, &config, &ui, &opts)?;

        // Orphan worktree should survive because --no-worktrees was set
        assert!(
            orphan_wt_path.exists(),
            "orphan worktree should survive with --no-worktrees"
        );
        Ok(())
    }

    #[test]
    fn run_dry_run_preserves_orphan_worktrees() -> Result<()> {
        let (dir, _git) = crate::test_helpers::init_repo()?;
        let path = dir.path();

        // Create a branch with a worktree, then orphan it
        Command::new("git")
            .args(["branch", "feature/orphan-dry"])
            .current_dir(path)
            .output()?;

        let orphan_wt_path = path.join("wt-orphan-dry");
        Command::new("git")
            .args([
                "worktree",
                "add",
                orphan_wt_path.to_str().unwrap(),
                "feature/orphan-dry",
            ])
            .current_dir(path)
            .output()?;

        Command::new("git")
            .args(["update-ref", "-d", "refs/heads/feature/orphan-dry"])
            .current_dir(path)
            .output()?;

        assert!(orphan_wt_path.exists());

        let git = Git::with_workdir(false, path);
        let config = default_config();
        let ui = Ui::new();
        let mut opts = opts_yes_skip_network();
        opts.dry_run = true;

        run(&git, &config, &ui, &opts)?;

        // Dry run should preserve the orphan worktree
        assert!(
            orphan_wt_path.exists(),
            "orphan worktree should survive dry run"
        );
        Ok(())
    }

    #[test]
    fn run_force_removes_dirty_worktree_with_yes_and_force() -> Result<()> {
        // With opts.yes + opts.force, the force-confirmation prompt is
        // auto-accepted, so a merged branch whose worktree contains an
        // untracked file is removed.
        let (dir, _git) = crate::test_helpers::init_repo()?;
        let path = dir.path();

        // Create a merged branch
        Command::new("git")
            .args(["checkout", "-b", "feature/dirty-wt"])
            .current_dir(path)
            .output()?;
        std::fs::write(path.join("dirty.txt"), "dirty feature")?;
        Command::new("git")
            .args(["add", "."])
            .current_dir(path)
            .output()?;
        Command::new("git")
            .args(["commit", "-m", "dirty feature"])
            .current_dir(path)
            .output()?;
        Command::new("git")
            .args(["checkout", "main"])
            .current_dir(path)
            .output()?;
        Command::new("git")
            .args(["merge", "feature/dirty-wt"])
            .current_dir(path)
            .output()?;

        // Add a worktree for the merged branch
        let wt_path = path.join("wt-dirty");
        Command::new("git")
            .args([
                "worktree",
                "add",
                wt_path.to_str().unwrap(),
                "feature/dirty-wt",
            ])
            .current_dir(path)
            .output()?;

        // Make the worktree dirty by adding an untracked file
        std::fs::write(wt_path.join("untracked.log"), "noise")?;

        let git = Git::with_workdir(false, path);
        let config = default_config();
        let ui = Ui::new();
        let mut opts = opts_yes_skip_network();
        opts.force = true;

        run(&git, &config, &ui, &opts)?;

        assert!(!wt_path.exists(), "dirty worktree should be force-removed");
        let branches = git.local_branches()?;
        assert!(
            !branches.contains(&"feature/dirty-wt".to_string()),
            "branch should be deleted after worktree force-removal"
        );
        Ok(())
    }

    #[test]
    fn run_yes_without_force_skips_dirty_worktree() -> Result<()> {
        // --yes accepts the deletions git-wipe proposed, but it must not
        // silently destroy uncommitted work: without --force the dirty
        // worktree and its branch are left untouched and reported as skipped.
        let (dir, _git) = crate::test_helpers::init_repo()?;
        let path = dir.path();

        crate::test_helpers::git_in(path, &["checkout", "-b", "feature/keep-dirty"])?;
        std::fs::write(path.join("dirty.txt"), "dirty feature")?;
        commit_all(path, "dirty feature")?;
        crate::test_helpers::git_in(path, &["checkout", "main"])?;
        crate::test_helpers::git_in(path, &["merge", "feature/keep-dirty"])?;

        let wt_path = path.join("wt-keep-dirty");
        crate::test_helpers::git_in(
            path,
            &[
                "worktree",
                "add",
                wt_path.to_str().unwrap(),
                "feature/keep-dirty",
            ],
        )?;
        std::fs::write(wt_path.join("untracked.log"), "noise")?;

        let git = Git::with_workdir(false, path);
        let opts = opts_yes_skip_network();
        assert!(!opts.force);

        let report = run(&git, &default_config(), &Ui::new(), &opts)?;

        assert!(
            wt_path.exists(),
            "dirty worktree must survive --yes without --force"
        );
        assert!(
            git.local_branches()?
                .contains(&"feature/keep-dirty".to_string()),
            "branch of a skipped worktree must survive"
        );
        assert!(
            report.local.worktrees.iter().any(|w| {
                w.branch.as_deref() == Some("feature/keep-dirty") && w.status == ItemStatus::Skipped
            }),
            "skipped worktree should be reported: {:?}",
            report.local.worktrees
        );
        assert!(
            report
                .warnings
                .iter()
                .any(|w| w.contains("feature/keep-dirty")),
            "a warning should explain the skip: {:?}",
            report.warnings
        );
        Ok(())
    }

    #[test]
    fn run_dry_run_dirty_worktree_not_removed() -> Result<()> {
        // With dry-run, even force-removal candidates are preserved
        let (dir, _git) = crate::test_helpers::init_repo()?;
        let path = dir.path();

        // Create a merged branch
        Command::new("git")
            .args(["checkout", "-b", "feature/dry-dirty"])
            .current_dir(path)
            .output()?;
        std::fs::write(path.join("file.txt"), "content")?;
        Command::new("git")
            .args(["add", "."])
            .current_dir(path)
            .output()?;
        Command::new("git")
            .args(["commit", "-m", "add file"])
            .current_dir(path)
            .output()?;
        Command::new("git")
            .args(["checkout", "main"])
            .current_dir(path)
            .output()?;
        Command::new("git")
            .args(["merge", "feature/dry-dirty"])
            .current_dir(path)
            .output()?;

        // Create worktree with dirty content
        let wt_path = path.join("wt-dry-dirty");
        Command::new("git")
            .args([
                "worktree",
                "add",
                wt_path.to_str().unwrap(),
                "feature/dry-dirty",
            ])
            .current_dir(path)
            .output()?;
        std::fs::write(wt_path.join("dirty.log"), "untracked")?;

        let git = Git::with_workdir(false, path);
        let config = default_config();
        let ui = Ui::new();
        let mut opts = opts_yes_skip_network();
        opts.dry_run = true;
        opts.force = true;

        run(&git, &config, &ui, &opts)?;

        // Worktree and branch should survive dry-run
        assert!(wt_path.exists(), "dirty worktree should survive dry-run");
        let branches = git.local_branches()?;
        assert!(
            branches.contains(&"feature/dry-dirty".to_string()),
            "branch should survive dry-run"
        );
        Ok(())
    }

    #[test]
    fn run_modified_file_in_merged_worktree() -> Result<()> {
        // Test that a merged branch's worktree with a modified (not untracked)
        // file is detected as dirty and force-removed with opts.force
        let (dir, _git) = crate::test_helpers::init_repo()?;
        let path = dir.path();

        // Create and commit a tracked file
        std::fs::write(path.join("tracked.txt"), "v1")?;
        Command::new("git")
            .args(["add", "."])
            .current_dir(path)
            .output()?;
        Command::new("git")
            .args(["commit", "-m", "initial commit"])
            .current_dir(path)
            .output()?;

        // Create and merge a feature branch
        Command::new("git")
            .args(["checkout", "-b", "feature/modified"])
            .current_dir(path)
            .output()?;
        std::fs::write(path.join("tracked.txt"), "v2")?;
        Command::new("git")
            .args(["add", "."])
            .current_dir(path)
            .output()?;
        Command::new("git")
            .args(["commit", "-m", "feature change"])
            .current_dir(path)
            .output()?;
        Command::new("git")
            .args(["checkout", "main"])
            .current_dir(path)
            .output()?;
        Command::new("git")
            .args(["merge", "feature/modified"])
            .current_dir(path)
            .output()?;

        // Create a worktree for the merged branch
        let wt_path = path.join("wt-modified");
        Command::new("git")
            .args([
                "worktree",
                "add",
                wt_path.to_str().unwrap(),
                "feature/modified",
            ])
            .current_dir(path)
            .output()?;

        // Modify a tracked file in the worktree (dirty state)
        std::fs::write(wt_path.join("tracked.txt"), "v3")?;

        let git = Git::with_workdir(false, path);
        let config = default_config();
        let ui = Ui::new();
        let mut opts = opts_yes_skip_network();
        opts.force = true;

        run(&git, &config, &ui, &opts)?;

        // Dirty worktree and branch should be force-removed with opts.force
        assert!(
            !wt_path.exists(),
            "modified worktree should be force-removed with opts.force"
        );
        let branches = git.local_branches()?;
        assert!(
            !branches.contains(&"feature/modified".to_string()),
            "branch of modified worktree should be deleted"
        );
        Ok(())
    }

    /// Helper: stage and commit `path`'s tracked files with the given message.
    fn commit_all(path: &Path, msg: &str) -> Result<()> {
        crate::test_helpers::git_in(path, &["add", "-A"])?;
        crate::test_helpers::git_in(path, &["commit", "-m", msg])?;
        Ok(())
    }

    /// Create a squash-merged feature branch + worktree. Returns the
    /// worktree path. Squash-merge produces a target commit whose SHA
    /// differs from the feature tip, so `branch_has_unmerged_commits`
    /// (raw reachability) reports the branch as unmerged, while
    /// `find_merged_local` still detects it via diff/patch-id.
    fn make_squash_merged_with_worktree(
        path: &Path,
        branch: &str,
        wt_dirname: &str,
    ) -> Result<PathBuf> {
        use crate::test_helpers::git_in;

        git_in(path, &["checkout", "-b", branch])?;
        std::fs::write(path.join(format!("{wt_dirname}.txt")), "feature work")?;
        commit_all(path, &format!("{branch}: feature work"))?;
        git_in(path, &["checkout", "main"])?;
        git_in(path, &["merge", "--squash", branch])?;
        commit_all(path, &format!("squash {branch}"))?;

        let wt_path = path.join(wt_dirname);
        git_in(
            path,
            &["worktree", "add", wt_path.to_str().unwrap(), branch],
        )?;
        Ok(wt_path)
    }

    #[test]
    fn run_auto_force_squash_merged_worktree_no_prompt() -> Result<()> {
        // A squash-merged branch is detected as merged by find_merged_local
        // but reported as having unmerged commits by branch_has_unmerged_commits.
        // With use_worktrunk=true and a clean worktree, the cleaner should
        // auto force-delete (set force_delete=true) without surfacing a second
        // prompt. Using dry_run=true so no `wt` binary call is attempted.
        let (dir, _git) = crate::test_helpers::init_repo()?;
        let path = dir.path();

        // Seed an initial commit on main so squash-merge has a parent.
        std::fs::write(path.join("seed.txt"), "seed")?;
        commit_all(path, "seed")?;

        let wt_path = make_squash_merged_with_worktree(path, "feature/squashed", "wt-squashed")?;

        let git = Git::with_workdir(false, path);
        let config = default_config();
        let ui = Ui::new();
        let mut opts = opts_yes_skip_network();
        opts.use_worktrunk = true;
        opts.dry_run = true;

        // Should run without error and exercise the auto_force code path.
        run(&git, &config, &ui, &opts)?;

        // Dry-run preserves everything.
        assert!(wt_path.exists(), "worktree should survive dry-run");
        let branches = git.local_branches()?;
        assert!(
            branches.contains(&"feature/squashed".to_string()),
            "branch should survive dry-run"
        );
        Ok(())
    }

    #[test]
    fn run_dirty_and_unmerged_goes_to_prompt_not_auto_force() -> Result<()> {
        // When a worktree is BOTH dirty and unmerged (squash-merged), the
        // dirty path wins: the entry must reach the second prompt rather
        // than being auto force-deleted. With opts.yes=true the prompt is
        // auto-confirmed, which under dry_run leaves everything in place.
        let (dir, _git) = crate::test_helpers::init_repo()?;
        let path = dir.path();

        std::fs::write(path.join("seed.txt"), "seed")?;
        commit_all(path, "seed")?;

        let wt_path =
            make_squash_merged_with_worktree(path, "feature/dirty-squashed", "wt-dirty-squashed")?;
        // Make it dirty (untracked file).
        std::fs::write(wt_path.join("untracked.log"), "noise")?;

        let git = Git::with_workdir(false, path);
        let config = default_config();
        let ui = Ui::new();
        let mut opts = opts_yes_skip_network();
        opts.use_worktrunk = true;
        opts.dry_run = true;

        run(&git, &config, &ui, &opts)?;

        // Dry-run: branch and worktree preserved.
        assert!(wt_path.exists(), "dirty worktree should survive dry-run");
        let branches = git.local_branches()?;
        assert!(
            branches.contains(&"feature/dirty-squashed".to_string()),
            "branch should survive dry-run"
        );
        Ok(())
    }

    // Gated to Unix: `wt`'s worktree removal on Windows CI uses a
    // cross-filesystem fallback that leaves the worktree directory (and thus
    // the in-use branch) in place, so the scenario this test relies on can't
    // be reproduced there. The branch-deletion logic itself is
    // platform-independent.
    #[cfg(unix)]
    #[test]
    fn run_worktrunk_deletes_branch_wt_leaves_behind() -> Result<()> {
        // Regression test: when worktrunk removes a worktree but leaves the
        // branch behind (because `wt`'s merge check is narrower than
        // git-wipe's), git-wipe must delete the surviving branch itself.
        //
        // Reproduced by merging the feature into a non-default protected
        // target ("develop"). git-wipe detects it as merged (develop is
        // protected), but `wt remove` checks only the default branch ("main")
        // and so refuses to delete the branch without `-D`.
        //
        // Requires the real `wt` binary; skipped otherwise.
        if !worktrunk_installed() {
            return Ok(());
        }

        let (dir, _git) = crate::test_helpers::init_repo()?;
        let path = dir.path();

        std::fs::write(path.join("seed.txt"), "seed")?;
        commit_all(path, "seed")?;

        // Second protected target the feature will be merged into.
        Command::new("git")
            .args(["branch", "develop"])
            .current_dir(path)
            .output()?;

        // Feature branch with one commit, merged into develop (not main).
        Command::new("git")
            .args(["checkout", "-b", "feature/wt-leftover"])
            .current_dir(path)
            .output()?;
        std::fs::write(path.join("work.txt"), "work")?;
        commit_all(path, "feature work")?;
        Command::new("git")
            .args(["checkout", "develop"])
            .current_dir(path)
            .output()?;
        Command::new("git")
            .args(["merge", "feature/wt-leftover"])
            .current_dir(path)
            .output()?;
        Command::new("git")
            .args(["checkout", "main"])
            .current_dir(path)
            .output()?;

        // Worktree for the merged feature branch.
        let wt_path = path.join("wt-leftover");
        Command::new("git")
            .args([
                "worktree",
                "add",
                wt_path.to_str().unwrap(),
                "feature/wt-leftover",
            ])
            .current_dir(path)
            .output()?;

        let git = Git::with_workdir(false, path);
        let config = Config {
            protected: vec!["main".to_string(), "develop".to_string()],
            ignore: Vec::new(),
            remotes: None,
            worktrunk: None,
            effort: None,
            min_age: None,
            min_size: None,
            jobs: None,
            forge: None,
        };
        let ui = Ui::new();
        let mut opts = opts_yes_skip_network();
        opts.use_worktrunk = true;

        run(&git, &config, &ui, &opts)?;

        // The worktree registration is gone: `wt` removed the worktree and
        // git-wipe pruned any stale metadata. (The directory itself may linger
        // briefly depending on `wt`'s platform-specific cleanup — e.g. its
        // trash/background rm — so assert on git's view rather than the path.)
        let still_registered = git
            .worktree_list()?
            .iter()
            .any(|wt| wt.branch.as_deref() == Some("feature/wt-leftover"));
        assert!(
            !still_registered,
            "worktree should no longer be registered after worktrunk removal"
        );
        // The branch `wt remove` left behind must be deleted by git-wipe.
        assert!(
            !git.branch_exists("feature/wt-leftover")?,
            "branch left behind by `wt remove` should be deleted by git-wipe"
        );
        Ok(())
    }

    #[test]
    #[cfg(unix)]
    fn run_dry_run_with_worktrunk_touches_nothing() -> Result<()> {
        // Regression test: under worktrunk, branches recorded as
        // `wt_handled_branches` used to bypass the dry-run guard entirely and
        // were really passed to `git worktree prune` + `git branch -D`.
        //
        // Requires the real `wt` binary; skipped otherwise.
        if !worktrunk_installed() {
            return Ok(());
        }

        let (dir, _git) = crate::test_helpers::init_repo()?;
        let path = dir.path();

        Command::new("git")
            .args(["checkout", "-b", "feature/dry"])
            .current_dir(path)
            .output()?;
        std::fs::write(path.join("dry.txt"), "dry")?;
        commit_all(path, "feature dry")?;
        Command::new("git")
            .args(["checkout", "main"])
            .current_dir(path)
            .output()?;
        Command::new("git")
            .args(["merge", "feature/dry"])
            .current_dir(path)
            .output()?;

        let wt_path = path.join("wt-dry");
        Command::new("git")
            .args(["worktree", "add", wt_path.to_str().unwrap(), "feature/dry"])
            .current_dir(path)
            .output()?;

        let git = Git::with_workdir(false, path);
        let mut opts = opts_yes_skip_network();
        opts.use_worktrunk = true;
        opts.dry_run = true;

        run(&git, &default_config(), &Ui::new(), &opts)?;

        assert!(wt_path.exists(), "dry run must not remove the worktree");
        assert!(
            git.branch_exists("feature/dry")?,
            "dry run must not delete the branch"
        );
        Ok(())
    }

    // ── Report contents ──────────────────────────────────────────────

    /// Push a branch that is already merged into `main` to the local remote,
    /// so the remote-branch phase has something to delete.
    fn push_merged_branch(work_path: &std::path::Path, branch: &str) -> Result<()> {
        Command::new("git")
            .args(["checkout", "-b", branch])
            .current_dir(work_path)
            .output()?;
        Command::new("git")
            .args(["push", "-u", "origin", branch])
            .current_dir(work_path)
            .output()?;
        Command::new("git")
            .args(["checkout", "main"])
            .current_dir(work_path)
            .output()?;
        Ok(())
    }

    #[test]
    fn run_reports_deleted_local_branches() -> Result<()> {
        let (_dir, git) = crate::test_helpers::init_repo_with_branches()?;

        let report = run(
            &git,
            &default_config(),
            &Ui::new(),
            &opts_yes_skip_network(),
        )?;

        assert_eq!(report.version, Report::VERSION);
        assert!(!report.dry_run);
        assert_eq!(report.effort, Effort::Standard);
        assert!(report.fetch.skipped);
        assert!(report.pull.skipped);
        assert!(!report.local.skipped);
        assert_eq!(report.local.merged, vec!["feature/done".to_string()]);
        assert!(report.local.gone.is_empty());

        let branch = &report.local.branches[0];
        assert_eq!(branch.branch, "feature/done");
        assert_eq!(branch.reason, BranchReason::Merged);
        assert!(branch.selected);
        assert_eq!(branch.status, ItemStatus::Deleted);
        assert!(branch.worktree.is_none());

        assert_eq!(report.summary.local_branches_deleted, 1);
        assert_eq!(report.summary.errors, 0);
        assert!(report.errors.is_empty());
        Ok(())
    }

    #[test]
    fn run_reports_dry_run_candidates_without_deleting() -> Result<()> {
        let (_dir, git) = crate::test_helpers::init_repo_with_branches()?;
        let mut opts = opts_yes_skip_network();
        opts.dry_run = true;

        let report = run(&git, &default_config(), &Ui::new(), &opts)?;

        assert!(report.dry_run);
        assert_eq!(report.local.branches[0].status, ItemStatus::DryRun);
        assert_eq!(report.summary.local_branches_deleted, 0);
        Ok(())
    }

    #[test]
    fn run_reports_a_worktree_removal() -> Result<()> {
        let (_dir, git, _wt_path) = crate::test_helpers::init_repo_with_worktree()?;

        let report = run(
            &git,
            &default_config(),
            &Ui::new(),
            &opts_yes_skip_network(),
        )?;

        let entry = report
            .local
            .worktrees
            .iter()
            .find(|wt| wt.path.ends_with("worktree-feature"))
            .expect("the branch worktree must be reported");
        assert_eq!(entry.branch.as_deref(), Some("feature/wt"));
        assert_eq!(entry.kind, WorktreeKind::Branch);
        assert_eq!(entry.status, ItemStatus::Removed);
        assert_eq!(report.summary.worktrees_removed, 1);

        let reported = report
            .local
            .branches
            .iter()
            .find(|b| b.branch == "feature/wt")
            .expect("the branch must be reported");
        assert!(
            reported.worktree.is_some(),
            "a branch with a worktree must expose its path"
        );
        Ok(())
    }

    #[test]
    fn run_reports_a_locked_worktree_as_skipped() -> Result<()> {
        let (_dir, git, _wt_path) = crate::test_helpers::init_repo_with_locked_worktree()?;

        let report = run(
            &git,
            &default_config(),
            &Ui::new(),
            &opts_yes_skip_network(),
        )?;

        let entry = report
            .local
            .worktrees
            .iter()
            .find(|wt| wt.path.ends_with("worktree-locked"))
            .expect("the locked worktree must be reported");
        assert_eq!(entry.branch.as_deref(), Some("feature/locked-wt"));
        assert_eq!(entry.status, ItemStatus::Locked);
        assert_eq!(report.summary.worktrees_removed, 0);
        Ok(())
    }

    #[test]
    fn run_unlocks_a_stale_lock_and_removes_the_worktree() -> Result<()> {
        let (_dir, git, wt_path) = crate::test_helpers::init_repo_with_stale_locked_worktree()?;

        let report = run(
            &git,
            &default_config(),
            &Ui::new(),
            &opts_yes_skip_network(),
        )?;

        assert!(
            !std::path::Path::new(&wt_path).exists(),
            "a stale lock must not prevent removal"
        );

        let branches = git.local_branches()?;
        assert!(
            !branches.contains(&"feature/stale-locked-wt".to_string()),
            "the branch should be deletable once its stale lock is cleared"
        );

        let entry = report
            .local
            .worktrees
            .iter()
            .find(|wt| wt.path.ends_with("worktree-stale-locked"))
            .expect("the worktree must be reported");
        assert_eq!(entry.status, ItemStatus::Removed);
        let stale_lock = entry
            .stale_lock
            .as_ref()
            .expect("a stale lock must be recorded on the entry");
        assert_eq!(stale_lock.pid, crate::test_helpers::DEAD_PID);
        assert_eq!(stale_lock.action, StaleLockAction::Unlocked);
        Ok(())
    }

    #[test]
    fn run_dry_run_never_unlocks_a_stale_lock() -> Result<()> {
        let (_dir, git, wt_path) = crate::test_helpers::init_repo_with_stale_locked_worktree()?;
        let mut opts = opts_yes_skip_network();
        opts.dry_run = true;

        let report = run(&git, &default_config(), &Ui::new(), &opts)?;

        assert!(
            std::path::Path::new(&wt_path).exists(),
            "--dry-run must not remove the worktree"
        );

        // The real git state must still be locked.
        let worktrees = git.worktree_list()?;
        assert!(
            worktrees
                .iter()
                .any(|wt| wt.path.ends_with("worktree-stale-locked") && wt.is_locked),
            "--dry-run must never actually unlock"
        );

        let entry = report
            .local
            .worktrees
            .iter()
            .find(|wt| wt.path.ends_with("worktree-stale-locked"))
            .expect("the worktree must be reported");
        let stale_lock = entry
            .stale_lock
            .as_ref()
            .expect("a stale lock must be recorded on the entry");
        assert_eq!(stale_lock.action, StaleLockAction::WouldUnlock);
        Ok(())
    }

    #[test]
    fn run_reports_a_failed_fetch() -> Result<()> {
        let (dir, git) = crate::test_helpers::init_repo()?;
        Command::new("git")
            .args(["remote", "add", "broken", "/this/path/does/not/exist.git"])
            .current_dir(dir.path())
            .output()?;

        let config = Config {
            remotes: Some(vec!["broken".to_string()]),
            ..default_config()
        };
        let mut opts = opts_yes_skip_network();
        opts.no_fetch = false;
        opts.local_only = true;

        let report = run(&git, &config, &Ui::new(), &opts)?;

        assert!(!report.fetch.skipped);
        assert_eq!(report.fetch.remotes[0].name, "broken");
        assert_eq!(report.fetch.remotes[0].status, ItemStatus::Failed);
        assert_eq!(report.errors[0].action, "fetch from");
        assert_eq!(report.errors[0].target, "broken");
        assert_eq!(report.summary.errors, 1);
        assert!(
            report
                .warnings
                .iter()
                .any(|w| w.contains("Continuing without 1 remote(s)")),
            "the staleness warning must reach the report: {:?}",
            report.warnings
        );
        assert!(report.remotes_skipped, "--local-only must be reported");
        assert!(report.remotes.is_empty());
        Ok(())
    }

    #[test]
    fn run_reports_deleted_remote_branches() -> Result<()> {
        let (_dir, work_path, _bare_path) = crate::test_helpers::init_repo_with_local_remote()?;
        push_merged_branch(&work_path, "feature/remote-done")?;

        let git = Git::with_workdir(false, &work_path);
        let mut opts = opts_yes_skip_network();
        opts.remote_only = true;

        let report = run(&git, &default_config(), &Ui::new(), &opts)?;

        assert!(report.local.skipped);
        assert!(!report.remotes_skipped);
        let remote = &report.remotes[0];
        assert_eq!(remote.remote, "origin");
        assert_eq!(remote.merged, vec!["feature/remote-done".to_string()]);
        assert_eq!(remote.branches[0].branch, "feature/remote-done");
        assert_eq!(remote.branches[0].status, ItemStatus::Deleted);
        assert_eq!(report.summary.remote_branches_deleted, 1);
        Ok(())
    }

    #[test]
    fn run_reports_remote_branches_left_alone_in_dry_run() -> Result<()> {
        let (_dir, work_path, _bare_path) = crate::test_helpers::init_repo_with_local_remote()?;
        push_merged_branch(&work_path, "feature/remote-dry")?;

        let git = Git::with_workdir(false, &work_path);
        let mut opts = opts_yes_skip_network();
        opts.remote_only = true;
        opts.dry_run = true;

        let report = run(&git, &default_config(), &Ui::new(), &opts)?;

        assert_eq!(report.remotes[0].branches[0].status, ItemStatus::DryRun);
        assert_eq!(report.summary.remote_branches_deleted, 0);
        assert!(
            git.merged_remote_branches("main", "origin")?
                .contains(&"feature/remote-dry".to_string()),
            "dry run must not delete the remote branch"
        );
        Ok(())
    }

    #[test]
    fn run_reports_a_remote_without_merged_branches() -> Result<()> {
        let (_dir, work_path, _bare_path) = crate::test_helpers::init_repo_with_local_remote()?;

        let git = Git::with_workdir(false, &work_path);
        let mut opts = opts_yes_skip_network();
        opts.remote_only = true;

        let report = run(&git, &default_config(), &Ui::new(), &opts)?;

        assert_eq!(report.remotes[0].remote, "origin");
        assert!(report.remotes[0].merged.is_empty());
        assert!(report.remotes[0].branches.is_empty());
        Ok(())
    }

    /// A strategy that cannot answer for a remote branch is reported as a
    /// warning instead of aborting the run.
    #[test]
    fn run_reports_remote_merge_detection_warnings() -> Result<()> {
        let (_dir, work_path, _bare_path) = crate::test_helpers::init_repo_with_local_remote()?;
        push_merged_branch(&work_path, "feature/remote-done")?;

        // A remote-tracking ref pointing at an object that does not exist:
        // every strategy probing it fails.
        let broken = work_path.join(".git/refs/remotes/origin/dangling");
        std::fs::create_dir_all(broken.parent().expect("a ref always has a parent"))?;
        std::fs::write(broken, "0000000000000000000000000000000000000001\n")?;

        let git = Git::with_workdir(false, &work_path);
        let mut opts = opts_yes_skip_network();
        opts.remote_only = true;

        let report = run(&git, &default_config(), &Ui::new(), &opts)?;

        assert_eq!(
            report.status,
            crate::report::Status::Success,
            "the run must not abort"
        );
        assert!(
            report
                .warnings
                .iter()
                .any(|w| w.contains("Merge detection partially failed")),
            "the failure should surface as a warning, got {:?}",
            report.warnings
        );
        assert_eq!(
            report.remotes[0].merged,
            vec!["feature/remote-done".to_string()],
            "the other branches are still detected"
        );
        Ok(())
    }

    #[test]
    fn run_reports_a_failed_remote_branch_deletion() -> Result<()> {
        let (_dir, work_path, _bare_path) = crate::test_helpers::init_repo_with_local_remote()?;
        push_merged_branch(&work_path, "feature/remote-fail")?;

        // Detection reads remote-tracking refs, which stay valid; only the
        // deletion (a push) reaches the remote, and now cannot find it.
        Command::new("git")
            .args([
                "remote",
                "set-url",
                "origin",
                "/this/path/does/not/exist.git",
            ])
            .current_dir(&work_path)
            .output()?;

        let git = Git::with_workdir(false, &work_path);
        let mut opts = opts_yes_skip_network();
        opts.remote_only = true;

        let report = run(&git, &default_config(), &Ui::new(), &opts)?;

        assert_eq!(report.remotes[0].branches[0].status, ItemStatus::Failed);
        assert_eq!(report.summary.remote_branches_deleted, 0);
        assert_eq!(report.errors[0].action, "delete");
        assert_eq!(report.errors[0].target, "origin/feature/remote-fail");
        assert_eq!(report.summary.errors, 1);
        Ok(())
    }

    #[test]
    fn run_reports_a_failed_pull() -> Result<()> {
        let (dir, work_path, bare_path) = crate::test_helpers::init_repo_with_local_remote()?;

        // Diverge: the remote gains a commit while `main` gains a different
        // one locally, so the fast-forward-only pull must fail.
        crate::test_helpers::advance_remote(&bare_path, dir.path())?;
        std::fs::write(work_path.join("local.txt"), "local")?;
        Command::new("git")
            .args(["add", "."])
            .current_dir(&work_path)
            .output()?;
        Command::new("git")
            .args(["commit", "-m", "local divergence"])
            .current_dir(&work_path)
            .output()?;

        let git = Git::with_workdir(false, &work_path);
        let mut opts = opts_yes_skip_network();
        opts.no_pull = false;
        opts.local_only = true;

        let report = run(&git, &default_config(), &Ui::new(), &opts)?;

        assert!(!report.pull.skipped);
        let pulled = &report.pull.branches[0];
        assert_eq!(pulled.branch, "main");
        assert_eq!(pulled.remote, "origin");
        assert_eq!(pulled.status, ItemStatus::Failed);
        assert_eq!(report.errors[0].action, "pull");
        assert_eq!(report.errors[0].target, "main");
        Ok(())
    }
}
