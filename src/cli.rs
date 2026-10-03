//! Command-line surface: flags, subcommands and their help text.
//!
//! Parsed with clap into [`Cli`] and [`ConfigAction`], which `main` dispatches.

use clap::{Parser, Subcommand};

use crate::duration::MinAge;
use crate::forge::setting::ForgeSetting;
use crate::size::Size;

/// Wipe out merged local branches and worktrees.
///
/// Detects branches that have been merged into your main branch(es) and offers
/// to delete them — both locally and on configured remotes. Also handles
/// orphaned worktree cleanup.
///
/// On first run, an interactive setup wizard stores preferences in the
/// git config `[wipe]` section.
#[derive(Parser, Debug)]
#[command(name = "git-wipe", version, about, long_about)]
pub struct Cli {
    /// Skip all confirmation prompts (auto-confirm deletions)
    #[arg(short = 'y', long)]
    pub yes: bool,

    /// Force-remove worktrees with uncommitted changes or unmerged commits
    ///
    /// Without this flag the forced-removal prompt defaults to nothing
    /// selected, and with --yes (or --json) problematic worktrees are skipped
    /// entirely. Interactively, --force pre-selects them; you can still
    /// uncheck any entry.
    #[arg(short = 'f', long)]
    pub force: bool,

    /// Show what would be done without actually doing it
    #[arg(short = 'n', long)]
    pub dry_run: bool,

    /// Show git commands being executed
    #[arg(short, long)]
    pub verbose: bool,

    /// Skip fetching and pruning remotes
    #[arg(long)]
    pub no_fetch: bool,

    /// Skip pulling (fast-forwarding) target branches before detection
    #[arg(long)]
    pub no_pull: bool,

    /// Only clean local branches (skip remote deletion)
    #[arg(long)]
    pub local_only: bool,

    /// Only clean remote branches (skip local deletion)
    #[arg(long)]
    pub remote_only: bool,

    /// Skip worktree cleanup
    #[arg(long)]
    pub no_worktrees: bool,

    /// With --yes, also delete branches whose upstream branch was deleted
    ///
    /// These branches are always listed for interactive selection, but are
    /// never pre-selected because a deleted upstream does not prove the branch
    /// was merged. Requires up-to-date remote-tracking refs, so it has no
    /// effect when --no-fetch is used outside of --dry-run.
    #[arg(long)]
    pub delete_gone: bool,

    /// How thorough merge detection should be (default: 2)
    ///
    /// Levels are cumulative:
    ///
    /// 1: ancestor merges only (`git branch --merged`), fastest.
    ///
    /// 2: adds cherry-pick, tree SHA and empty diff detection.
    ///
    /// 3: adds patch-id, simulated merge and squash patch-id detection,
    /// the most thorough but noticeably slower.
    ///
    /// Global, so `status` classifies branches exactly like a wipe run does.
    #[arg(long, value_name = "LEVEL", global = true, value_parser = clap::value_parser!(u8).range(1..=3))]
    pub effort: Option<u8>,

    /// Skip worktrees created less than this long ago (default: 0s)
    ///
    /// Accepts a single value and unit: 30s, 15m, 2h, 7d, 1w — or a bare 0 to
    /// disable the guard. Protects a worktree you just created from the
    /// default branch from being removed along with its "merged" branch.
    ///
    /// On `status` this is a filter instead of a guard: only entries at least
    /// this old are listed, and the configured `wipe.minage` is not inherited.
    #[arg(long, value_name = "DURATION", global = true)]
    pub min_age: Option<MinAge>,

    /// Skip worktrees smaller than this on disk (default: 0B)
    ///
    /// Accepts a single value and unit: 512B, 100K, 100M, 2G — binary units —
    /// or a bare 0 to disable the filter/guard.
    ///
    /// On a wipe run this excludes smaller worktrees from the candidate list
    /// entirely. On `status` this is a display filter instead, and the
    /// configured `wipe.minsize` is not inherited — same reasoning as
    /// `--min-age`.
    ///
    /// Computing a worktree's size walks its directory tree, so sizing is
    /// skipped unless this or --size is given.
    #[arg(long, value_name = "SIZE", global = true)]
    pub min_size: Option<Size>,

    /// Compute and show worktree sizes, without filtering or excluding anything
    ///
    /// Implied by a non-zero --min-size. Exists so sizes can be inspected
    /// without setting a threshold.
    #[arg(long, global = true)]
    pub size: bool,

    /// Number of git probes to run at once during analysis (default: CPU count)
    ///
    /// Analysis spends its time waiting on independent, read-only git commands;
    /// overlapping them shortens the pause before the first prompt. The results
    /// are identical whatever the value — only the wall clock changes.
    ///
    /// Fetching, pulling, deleting branches and removing worktrees always run
    /// serially. `--verbose` forces 1 so the echoed commands stay in order.
    ///
    /// Global, so `status` can overlap its probes the same way.
    #[arg(short = 'j', long, value_name = "N", global = true, value_parser = clap::value_parser!(u32).range(1..))]
    pub jobs: Option<u32>,

    /// Use worktrunk (wt) for worktree removal to trigger pre/post-remove hooks
    #[arg(long, overrides_with = "no_worktrunk")]
    pub worktrunk: bool,

    /// Do not use worktrunk for worktree removal (overrides config)
    #[arg(long, overrides_with = "worktrunk")]
    pub no_worktrunk: bool,

    /// Ask the forge which branches were merged, before trying git (networked)
    ///
    /// Squash and rebase merges rewrite history, so git can only guess at them.
    /// The forge knows: with this flag, the pull/merge request state of each
    /// branch is looked up first, and only the branches it cannot settle go
    /// through the offline strategies. Branches it settles are reported as
    /// `pr-merged`.
    ///
    /// Without a value the forge is identified from each remote URL (GitHub,
    /// GitLab, Gitea and Forgejo). Pass `--forge=<KIND>` (github, gitlab,
    /// gitea or forgejo) for a self-hosted instance whose host name does not
    /// give it away. A branch only counts as merged if the request ended exactly at
    /// the branch tip.
    ///
    /// This sends branch names to the forge. Tokens come from the environment,
    /// never from git config: GITHUB_TOKEN or GH_TOKEN (GH_ENTERPRISE_TOKEN
    /// for Enterprise Server), GITLAB_TOKEN or GL_TOKEN, GITEA_TOKEN or
    /// FORGEJO_TOKEN. GitHub requires one; the others also work anonymously on
    /// public projects.
    ///
    /// If the forge cannot be reached or refuses the request, a warning is
    /// shown and the offline strategies run as usual. Enabled permanently with
    /// `git wipe config set forge true`.
    #[arg(
        long,
        value_name = "KIND",
        num_args = 0..=1,
        require_equals = true,
        default_missing_value = "true",
        global = true
    )]
    pub forge: Option<ForgeSetting>,

    /// Do not consult the forge (overrides config)
    #[arg(long, overrides_with = "forge", global = true)]
    pub no_forge: bool,

    /// Output a single JSON document to stdout (implies --yes)
    ///
    /// Human-readable output is suppressed: stdout carries the document alone.
    /// The document is pretty-printed on a terminal and compact when piped or
    /// redirected.
    ///
    /// Global so it can be given before or after a subcommand
    /// (`git wipe config list --json`).
    #[arg(long, global = true)]
    pub json: bool,

    /// Disable ANSI colours and styling in all output
    ///
    /// Colour is disabled automatically already when the stream is not a
    /// terminal, or when the standard `NO_COLOR` environment variable is set;
    /// this flag forces it off regardless.
    ///
    /// Global so it can be given before or after a subcommand.
    #[arg(long, global = true)]
    pub no_color: bool,

    #[command(subcommand)]
    pub command: Option<Command>,
}

impl Cli {
    /// Whether prompts should be skipped.
    ///
    /// JSON output implies `--yes`: prompts would corrupt the document and
    /// hang non-interactive callers.
    pub fn effective_yes(&self) -> bool {
        self.yes || self.json
    }
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Manage git-wipe configuration
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },

    /// Show a read-only inventory of local branches and worktrees
    ///
    /// One line per worktree and per branch without a worktree, oldest first,
    /// with an age and a combined status. Nothing is fetched, nothing is
    /// prompted and nothing is modified, so it is safe to run at any time —
    /// including in a repository git-wipe has never been configured in.
    ///
    /// Merge detection honours --effort and --jobs; --effort 1 is the fast
    /// path. Because remotes are never fetched, `gone` reflects the
    /// remote-tracking refs as they are on disk and may be stale.
    ///
    /// Ignored branches are not listed, since git-wipe treats them as
    /// non-existent everywhere else.
    #[command(alias = "list")]
    Status {
        /// Only list branches detected as merged into a protected branch
        #[arg(long)]
        merged: bool,
    },
}

#[derive(Subcommand, Debug)]
pub enum ConfigAction {
    /// Display current configuration
    List,

    /// Set a configuration value
    Set {
        /// Configuration key (e.g. worktrunk)
        ///
        /// For multi-valued keys (protected, ignore, remote) this replaces
        /// every existing value; use the add-*/remove-* subcommands to edit
        /// them individually.
        key: String,
        /// Value to set
        value: String,
    },

    /// Add a protected branch pattern
    AddProtected {
        /// Glob pattern (e.g. release/*)
        pattern: String,
    },

    /// Remove a protected branch pattern
    RemoveProtected {
        /// Glob pattern to remove
        pattern: String,
    },

    /// Add an ignored branch pattern
    ///
    /// Ignored branches are not fetched and are invisible to every detection
    /// pass. Ignoring takes precedence over protection.
    AddIgnore {
        /// Glob pattern (e.g. wip/*)
        pattern: String,
    },

    /// Remove an ignored branch pattern
    RemoveIgnore {
        /// Glob pattern to remove
        pattern: String,
    },

    /// Add a remote to operate on
    AddRemote {
        /// Remote name (e.g. origin)
        name: String,
    },

    /// Remove a remote from the configured list
    RemoveRemote {
        /// Remote name to remove
        name: String,
    },

    /// Mark a branch as protected via per-branch config
    Protect {
        /// Branch name to protect
        branch: String,
    },

    /// Remove per-branch protection from a branch
    Unprotect {
        /// Branch name to unprotect
        branch: String,
    },

    /// Mark a branch as ignored via per-branch config
    Ignore {
        /// Branch name to ignore
        branch: String,
    },

    /// Remove the per-branch ignore flag from a branch
    Unignore {
        /// Branch name to stop ignoring
        branch: String,
    },

    /// Re-run the interactive setup wizard
    Setup,
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn cli_default_flags() {
        let cli = Cli::parse_from(["git-wipe"]);
        assert!(!cli.yes);
        assert!(!cli.force);
        assert!(!cli.dry_run);
        assert!(!cli.verbose);
        assert!(!cli.no_fetch);
        assert!(!cli.no_pull);
        assert!(!cli.local_only);
        assert!(!cli.remote_only);
        assert!(!cli.no_worktrees);
        assert!(!cli.delete_gone);
        assert!(!cli.worktrunk);
        assert!(!cli.no_worktrunk);
        assert!(cli.effort.is_none());
        assert!(!cli.json);
        assert!(!cli.effective_yes());
        assert!(cli.command.is_none());
    }

    #[test]
    fn cli_json_flag_implies_yes() {
        let cli = Cli::parse_from(["git-wipe", "--json"]);
        assert!(cli.json);
        assert!(!cli.yes);
        assert!(cli.effective_yes());
        // --json implies --yes, but never --force: forced removal of dirty
        // worktrees stays an explicit opt-in.
        assert!(!cli.force);
    }

    #[test]
    fn cli_effective_yes_follows_yes_flag() {
        let cli = Cli::parse_from(["git-wipe", "-y"]);
        assert!(!cli.json);
        assert!(cli.effective_yes());
    }

    #[test]
    fn cli_flag_parsing() {
        let cli = Cli::parse_from([
            "git-wipe",
            "-y",
            "-f",
            "-n",
            "-v",
            "--no-fetch",
            "--local-only",
        ]);
        assert!(cli.yes);
        assert!(cli.force);
        assert!(cli.dry_run);
        assert!(cli.verbose);
        assert!(cli.no_fetch);
        assert!(cli.local_only);
        assert!(!cli.remote_only);
    }

    #[test]
    fn cli_force_flag() {
        assert!(Cli::parse_from(["git-wipe", "--force"]).force);
        assert!(Cli::parse_from(["git-wipe", "-f"]).force);
        // --force does not auto-confirm the main selection prompt.
        assert!(!Cli::parse_from(["git-wipe", "--force"]).effective_yes());
    }

    #[test]
    fn cli_no_pull_flag() {
        let cli = Cli::parse_from(["git-wipe", "--no-pull"]);
        assert!(cli.no_pull);
        assert!(!cli.no_fetch);
    }

    #[test]
    fn cli_effort_flag() {
        assert_eq!(
            Cli::parse_from(["git-wipe", "--effort", "1"]).effort,
            Some(1)
        );
        assert_eq!(
            Cli::parse_from(["git-wipe", "--effort", "3"]).effort,
            Some(3)
        );
    }

    #[test]
    fn cli_effort_rejects_out_of_range_levels() {
        assert!(Cli::try_parse_from(["git-wipe", "--effort", "0"]).is_err());
        assert!(Cli::try_parse_from(["git-wipe", "--effort", "4"]).is_err());
        assert!(Cli::try_parse_from(["git-wipe", "--effort", "max"]).is_err());
    }

    #[test]
    fn cli_min_age_flag() {
        let cli = Cli::parse_from(["git-wipe", "--min-age", "2h"]);
        assert_eq!(cli.min_age, Some("2h".parse().unwrap()));
        assert_eq!(Cli::parse_from(["git-wipe"]).min_age, None);
    }

    #[test]
    fn cli_min_age_rejects_garbage() {
        assert!(Cli::try_parse_from(["git-wipe", "--min-age", "soon"]).is_err());
        assert!(Cli::try_parse_from(["git-wipe", "--min-age", "5x"]).is_err());
    }

    #[test]
    fn cli_min_size_flag() {
        let cli = Cli::parse_from(["git-wipe", "--min-size", "100M"]);
        assert_eq!(cli.min_size, Some("100M".parse().unwrap()));
        assert_eq!(Cli::parse_from(["git-wipe"]).min_size, None);
    }

    #[test]
    fn cli_min_size_rejects_garbage() {
        assert!(Cli::try_parse_from(["git-wipe", "--min-size", "soon"]).is_err());
        assert!(Cli::try_parse_from(["git-wipe", "--min-size", "5x"]).is_err());
    }

    #[test]
    fn cli_size_flag_defaults_to_false() {
        assert!(!Cli::parse_from(["git-wipe"]).size);
        assert!(Cli::parse_from(["git-wipe", "--size"]).size);
    }

    #[test]
    fn cli_worktrunk_flag() {
        let cli = Cli::parse_from(["git-wipe", "--worktrunk"]);
        assert!(cli.worktrunk);
        assert!(!cli.no_worktrunk);
    }

    #[test]
    fn cli_no_worktrunk_flag() {
        let cli = Cli::parse_from(["git-wipe", "--no-worktrunk"]);
        assert!(!cli.worktrunk);
        assert!(cli.no_worktrunk);
    }

    #[test]
    fn cli_worktrunk_overrides() {
        // Last flag wins with overrides_with
        let cli = Cli::parse_from(["git-wipe", "--worktrunk", "--no-worktrunk"]);
        assert!(!cli.worktrunk);
        assert!(cli.no_worktrunk);

        let cli = Cli::parse_from(["git-wipe", "--no-worktrunk", "--worktrunk"]);
        assert!(cli.worktrunk);
        assert!(!cli.no_worktrunk);
    }

    #[test]
    fn cli_forge_flag() {
        assert_eq!(Cli::parse_from(["git-wipe"]).forge, None);
        assert_eq!(
            Cli::parse_from(["git-wipe", "--forge"]).forge,
            Some(ForgeSetting::Auto)
        );
        assert_eq!(
            Cli::parse_from(["git-wipe", "--forge=gitlab"]).forge,
            Some(ForgeSetting::Kind(crate::forge::setting::ForgeKind::GitLab))
        );
        assert!(Cli::try_parse_from(["git-wipe", "--forge=svn"]).is_err());
    }

    #[test]
    fn cli_forge_flag_does_not_swallow_a_subcommand() {
        let cli = Cli::parse_from(["git-wipe", "--forge", "status"]);
        assert_eq!(cli.forge, Some(ForgeSetting::Auto));
        assert!(matches!(cli.command, Some(Command::Status { .. })));
        let cli = Cli::parse_from(["git-wipe", "status", "--forge"]);
        assert_eq!(cli.forge, Some(ForgeSetting::Auto));
    }

    #[test]
    fn cli_no_forge_overrides_forge() {
        let cli = Cli::parse_from(["git-wipe", "--no-forge"]);
        assert!(cli.no_forge);
        let cli = Cli::parse_from(["git-wipe", "--forge", "--no-forge"]);
        assert!(cli.no_forge);
        assert_eq!(cli.forge, None);
        let cli = Cli::parse_from(["git-wipe", "--no-forge", "--forge"]);
        assert!(!cli.no_forge);
        assert_eq!(cli.forge, Some(ForgeSetting::Auto));
    }

    #[test]
    fn cli_config_subcommand() {
        let cli = Cli::parse_from(["git-wipe", "config", "list"]);
        assert!(cli.command.is_some());
        match cli.command.unwrap() {
            Command::Config { action } => match action {
                ConfigAction::List => {} // expected
                _ => panic!("Expected ConfigAction::List"),
            },
            other => panic!("Expected Command::Config, got {other:?}"),
        }

        let cli = Cli::parse_from(["git-wipe", "config", "set", "remote", "origin"]);
        match cli.command.unwrap() {
            Command::Config { action } => match action {
                ConfigAction::Set { key, value } => {
                    assert_eq!(key, "remote");
                    assert_eq!(value, "origin");
                }
                _ => panic!("Expected ConfigAction::Set"),
            },
            other => panic!("Expected Command::Config, got {other:?}"),
        }

        let cli = Cli::parse_from(["git-wipe", "config", "add-protected", "release/*"]);
        match cli.command.unwrap() {
            Command::Config { action } => match action {
                ConfigAction::AddProtected { pattern } => {
                    assert_eq!(pattern, "release/*");
                }
                _ => panic!("Expected ConfigAction::AddProtected"),
            },
            other => panic!("Expected Command::Config, got {other:?}"),
        }
    }

    #[test]
    fn cli_status_subcommand() {
        let cli = Cli::parse_from(["git-wipe", "status"]);
        match cli.command.unwrap() {
            Command::Status { merged } => assert!(!merged),
            other => panic!("Expected Command::Status, got {other:?}"),
        }
    }

    #[test]
    fn cli_status_list_alias() {
        let cli = Cli::parse_from(["git-wipe", "list"]);
        match cli.command.unwrap() {
            Command::Status { merged } => assert!(!merged),
            other => panic!("Expected Command::Status, got {other:?}"),
        }
    }

    #[test]
    fn cli_status_merged_flag() {
        let cli = Cli::parse_from(["git-wipe", "status", "--merged"]);
        match cli.command.unwrap() {
            Command::Status { merged } => assert!(merged),
            other => panic!("Expected Command::Status, got {other:?}"),
        }
    }

    #[test]
    fn cli_global_flags_after_a_subcommand() {
        let cli = Cli::parse_from([
            "git-wipe",
            "status",
            "--effort",
            "3",
            "--jobs",
            "2",
            "--min-age",
            "2h",
            "--min-size",
            "100M",
            "--size",
            "--json",
            "--no-color",
        ]);
        assert_eq!(cli.effort, Some(3));
        assert_eq!(cli.jobs, Some(2));
        assert_eq!(cli.min_age, Some("2h".parse().unwrap()));
        assert_eq!(cli.min_size, Some("100M".parse().unwrap()));
        assert!(cli.size);
        assert!(cli.json);
        assert!(cli.no_color);
    }

    #[test]
    fn cli_global_flags_before_a_subcommand() {
        let cli = Cli::parse_from([
            "git-wipe",
            "--effort",
            "3",
            "--jobs",
            "2",
            "--min-age",
            "2h",
            "--min-size",
            "100M",
            "--size",
            "--json",
            "--no-color",
            "status",
        ]);
        assert_eq!(cli.effort, Some(3));
        assert_eq!(cli.jobs, Some(2));
        assert_eq!(cli.min_age, Some("2h".parse().unwrap()));
        assert_eq!(cli.min_size, Some("100M".parse().unwrap()));
        assert!(cli.size);
        assert!(cli.json);
        assert!(cli.no_color);
    }

    #[test]
    fn cli_no_color_default_is_false() {
        assert!(!Cli::parse_from(["git-wipe"]).no_color);
        assert!(Cli::parse_from(["git-wipe", "--no-color"]).no_color);
    }
}
