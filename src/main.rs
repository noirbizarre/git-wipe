//! `git-wipe`: wipe out merged local branches and worktrees.
//!
//! This is a binary crate, so nothing is exported to downstream users and
//! `pub` therefore means "visible to the other modules" — it carries no
//! stability promise. Items used only within their own module stay private;
//! everything crossing a module boundary is plain `pub` rather than
//! `pub(crate)`, since in a binary the two are equivalent.
//!
//! `main` stays thin: it parses the CLI, dispatches, and renders errors. The
//! work lives in [`cleaner`] (workflow), [`branches`] and [`worktrees`]
//! (detection), [`config`] (settings), [`git`] (git access) and [`ui`]
//! (output).

mod branches;
mod cleaner;
mod cli;
mod config;
mod duration;
mod forge;
mod git;
mod parallel;
mod pid;
mod report;
mod size;
mod status;
#[cfg(test)]
mod test_helpers;
mod ui;
mod worktrees;

use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::Parser;

use cli::{Cli, Command, ConfigAction};
use git::{GitCommandError, GitErrorKind};
use report::Report;

fn main() -> ExitCode {
    let mut cli = Cli::parse();

    // Before any `Style` or `Term` is built: `console` caches the colour
    // decision per stream on first use, so a later call would be a no-op.
    if cli.no_color {
        console::set_colors_enabled(false);
        console::set_colors_enabled_stderr(false);
    }

    let git = git::Git::new(cli.verbose);
    // In JSON mode the document on stdout is the only output.
    let ui = if cli.json {
        ui::Ui::quiet()
    } else {
        ui::Ui::new()
    };

    match git.is_inside_work_tree() {
        Ok(true) => {}
        Ok(false) => {
            let err = anyhow::anyhow!("Not a git repository (or any of the parent directories).");
            if cli.json {
                emit_fatal_json(&err);
            } else {
                ui.error(&err.to_string());
            }
            return ExitCode::FAILURE;
        }
        Err(err) => {
            fail(&ui, cli.json, &err);
            return ExitCode::FAILURE;
        }
    }

    let json = cli.json;
    // Taken out rather than matched by value: the `status` arm needs both the
    // subcommand payload and a borrow of `cli` for the global flags.
    let result = match cli.command.take() {
        Some(Command::Config { action }) => handle_config_command(&git, &ui, json, action),
        Some(Command::Status { merged }) => handle_status(&git, &ui, &cli, merged),
        None => handle_clean(&git, &ui, &cli),
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) if is_cancelled(&err) => {
            report_cancelled(&ui, json);
            ExitCode::FAILURE
        }
        Err(err) => {
            fail(&ui, json, &err);
            ExitCode::FAILURE
        }
    }
}

/// Render a user-cancelled run, as text or as a JSON document.
fn report_cancelled(ui: &ui::Ui, json: bool) {
    if json {
        emit_fatal_json(&anyhow::anyhow!("Cancelled."));
    } else {
        ui.muted("Cancelled.");
    }
}

/// Render a fatal error, as text or as a JSON document.
fn fail(ui: &ui::Ui, json: bool, err: &anyhow::Error) {
    if json {
        emit_fatal_json(err);
    } else {
        report_error(ui, err);
    }
}

/// Print an error-status JSON document on stdout.
///
/// Printing the document is itself best-effort: a broken pipe here must not
/// mask the error we are already reporting through the exit code.
fn emit_fatal_json(err: &anyhow::Error) {
    let _ = report::print_json(&Report::fatal("run", "repository", err));
}

/// Whether an error chain represents a user-cancelled prompt (Esc / Ctrl-C).
///
/// Interactive prompts surface cancellation as an `io::ErrorKind::Interrupted`
/// error; we treat it as a clean abort rather than a failure to report.
fn is_cancelled(err: &anyhow::Error) -> bool {
    err.chain().any(|cause| {
        cause
            .downcast_ref::<std::io::Error>()
            .is_some_and(|io_err| io_err.kind() == std::io::ErrorKind::Interrupted)
    })
}

/// Render an `anyhow::Error` chain using the same UI styling as the rest of
/// the CLI, without ever printing a Rust stack backtrace.
///
/// When the root cause is a [`GitCommandError`] classified as a network or
/// auth failure, the headline gets a matching prefix so users can tell at a
/// glance "this is my network, not a bug". The classification itself is the
/// [`GitErrorKind`] computed once when the command fails, which
/// [`ui::Ui::report_failure`] reads too, so every failure path agrees on *what*
/// went wrong. The wording differs on purpose: `report_failure` words a
/// per-item failure ("cannot delete 'x'"), whereas this renders a fatal error
/// that has no target, and forge errors ([`forge::ForgeError`]) read as the
/// tail of a warning sentence, hence their lower case.
fn report_error(ui: &ui::Ui, err: &anyhow::Error) {
    let headline = match err.downcast_ref::<GitCommandError>().map(|gerr| gerr.kind) {
        Some(GitErrorKind::Network) => {
            format!("Network error: {}", short_cause(err))
        }
        Some(GitErrorKind::Auth) => {
            format!("Authentication error: {}", short_cause(err))
        }
        _ => format!("{err}"),
    };

    ui.error(&headline);

    // Print the rest of the causal chain (skipping the headline we already
    // rendered). Each cause is dimmed for readability.
    for cause in err.chain().skip(1) {
        ui.muted(&format!("  caused by: {cause}"));
    }
}

/// Short, single-line cause of a git failure, when the error is one.
fn short_cause(err: &anyhow::Error) -> String {
    err.downcast_ref::<GitCommandError>()
        .map(|gerr| gerr.short_cause().to_string())
        .unwrap_or_else(|| err.to_string())
}

/// Whether a `[wipe]` key is stored as repeated git config entries.
///
/// Multi-valued keys cannot be written with a plain `git config <key> <value>`
/// once they hold more than one entry.
fn is_multi_valued(key: &str) -> bool {
    matches!(key, "protected" | "ignore" | "remote")
}

fn handle_config_command(
    git: &git::Git,
    ui: &ui::Ui,
    json: bool,
    action: ConfigAction,
) -> Result<()> {
    match action {
        ConfigAction::List => {
            if json {
                return list_config_json(git);
            }
            match config::Config::try_load(git)? {
                Some(cfg) => {
                    ui.heading(&format!("Current configuration [{}]:", config::SECTION));
                    ui.blank();

                    ui.field(
                        "protected",
                        &if cfg.protected.is_empty() {
                            "(none)".to_string()
                        } else {
                            cfg.protected.join(", ")
                        },
                    );
                    ui.field(
                        "ignore",
                        &if cfg.ignore.is_empty() {
                            "(none)".to_string()
                        } else {
                            cfg.ignore.join(", ")
                        },
                    );
                    ui.field(
                        "remotes",
                        &match &cfg.remotes {
                            Some(r) => r.join(", "),
                            None => "(all)".to_string(),
                        },
                    );

                    let branch_protected = git.branch_protected_list()?;
                    ui.field(
                        "branch protected",
                        &if branch_protected.is_empty() {
                            "(none)".to_string()
                        } else {
                            branch_protected.join(", ")
                        },
                    );

                    let branch_ignored = git.branch_ignored_list()?;
                    ui.field(
                        "branch ignored",
                        &if branch_ignored.is_empty() {
                            "(none)".to_string()
                        } else {
                            branch_ignored.join(", ")
                        },
                    );

                    ui.field(
                        "worktrunk",
                        match cfg.worktrunk {
                            Some(true) => "enabled",
                            Some(false) => "disabled",
                            None => "(auto-detect)",
                        },
                    );

                    ui.field(
                        "effort",
                        &match cfg.effort {
                            Some(effort) => effort.to_string(),
                            None => format!("(default: {})", branches::Effort::default()),
                        },
                    );

                    ui.field(
                        "min age",
                        &match cfg.min_age {
                            Some(min_age) => min_age.to_string(),
                            None => format!("(default: {})", duration::MinAge::default()),
                        },
                    );

                    ui.field(
                        "min size",
                        &match cfg.min_size {
                            Some(min_size) => min_size.to_string(),
                            None => format!("(default: {})", size::Size::default()),
                        },
                    );

                    ui.field(
                        "jobs",
                        &match cfg.jobs {
                            Some(jobs) => jobs.to_string(),
                            None => "(default: CPU count)".to_string(),
                        },
                    );

                    ui.field(
                        "forge",
                        &match cfg.forge {
                            Some(forge::ForgeSetting::Off) | None => "disabled".to_string(),
                            Some(forge::ForgeSetting::Auto) => "enabled (auto-detect)".to_string(),
                            Some(forge::ForgeSetting::Kind(kind)) => format!("enabled ({kind})"),
                        },
                    );
                }
                None => {
                    ui.muted("No configuration found. Run `git wipe` to start the setup wizard.");
                }
            }
            Ok(())
        }

        ConfigAction::Set { key, value } => {
            let full_key = format!("{}.{key}", config::SECTION);
            config::validate_value(&key, &value)
                .with_context(|| format!("invalid value for {full_key}"))?;
            if is_multi_valued(&key) {
                // `git config --local <key> <value>` refuses a key that already
                // holds several values. Treat `set` as "replace every value"
                // so the outcome matches the verb regardless of prior state.
                git.config_unset_all(&full_key)?;
                git.config_add(&full_key, &value)?;
            } else {
                git.config_set(&full_key, &value)?;
            }
            ui.success(&format!("Set {key} = {value}"));
            Ok(())
        }

        ConfigAction::AddProtected { pattern } => {
            git.config_add(&format!("{}.protected", config::SECTION), &pattern)?;
            ui.success(&format!(
                "Added protected pattern: {}",
                console::style(pattern).cyan()
            ));
            Ok(())
        }

        ConfigAction::RemoveProtected { pattern } => {
            git.config_remove_value(&format!("{}.protected", config::SECTION), &pattern)?;
            ui.success(&format!(
                "Removed protected pattern: {}",
                console::style(pattern).cyan()
            ));
            Ok(())
        }

        ConfigAction::AddIgnore { pattern } => {
            git.config_add(&format!("{}.ignore", config::SECTION), &pattern)?;
            ui.success(&format!(
                "Added ignore pattern: {}",
                console::style(pattern).cyan()
            ));
            Ok(())
        }

        ConfigAction::RemoveIgnore { pattern } => {
            git.config_remove_value(&format!("{}.ignore", config::SECTION), &pattern)?;
            ui.success(&format!(
                "Removed ignore pattern: {}",
                console::style(pattern).cyan()
            ));
            Ok(())
        }

        ConfigAction::AddRemote { name } => {
            git.config_add(&format!("{}.remote", config::SECTION), &name)?;
            ui.success(&format!("Added remote: {}", console::style(&name).cyan()));
            Ok(())
        }

        ConfigAction::RemoveRemote { name } => {
            git.config_remove_value(&format!("{}.remote", config::SECTION), &name)?;
            ui.success(&format!("Removed remote: {}", console::style(&name).cyan()));
            Ok(())
        }

        ConfigAction::Protect { branch } => {
            git.set_branch_protected(&branch, true)?;
            ui.success(&format!(
                "Branch '{}' marked as protected",
                console::style(&branch).cyan()
            ));
            Ok(())
        }

        ConfigAction::Unprotect { branch } => {
            git.set_branch_protected(&branch, false)?;
            ui.success(&format!(
                "Branch '{}' is no longer protected",
                console::style(&branch).cyan()
            ));
            Ok(())
        }

        ConfigAction::Ignore { branch } => {
            git.set_branch_ignored(&branch, true)?;
            ui.success(&format!(
                "Branch '{}' is now ignored",
                console::style(&branch).cyan()
            ));
            Ok(())
        }

        ConfigAction::Unignore { branch } => {
            git.set_branch_ignored(&branch, false)?;
            ui.success(&format!(
                "Branch '{}' is no longer ignored",
                console::style(&branch).cyan()
            ));
            Ok(())
        }

        ConfigAction::Setup => {
            config::Config::interactive_setup(git, ui)?;
            Ok(())
        }
    }
}

/// `config list --json`: the same data as the text listing, structured.
///
/// Unlike the text output, an unconfigured repository is not an error: the
/// document simply reports `configured: false` with empty values.
fn list_config_json(git: &git::Git) -> Result<()> {
    let cfg = config::Config::try_load(git)?;
    let report = report::ConfigReport {
        configured: cfg.is_some(),
        protected: cfg
            .as_ref()
            .map(|c| c.protected.clone())
            .unwrap_or_default(),
        ignore: cfg.as_ref().map(|c| c.ignore.clone()).unwrap_or_default(),
        remotes: cfg.as_ref().and_then(|c| c.remotes.clone()),
        branch_protected: git.branch_protected_list()?,
        branch_ignored: git.branch_ignored_list()?,
        effort: cfg.as_ref().and_then(|c| c.effort),
        min_age: cfg.as_ref().and_then(|c| c.min_age),
        min_size: cfg.as_ref().and_then(|c| c.min_size),
        jobs: cfg.as_ref().and_then(|c| c.jobs),
        worktrunk: cfg.as_ref().and_then(|c| c.worktrunk),
        forge: cfg.as_ref().and_then(|c| c.forge),
    };
    report::print_json(&report)
}

/// `git wipe status`: a read-only inventory, never a setup wizard.
///
/// Uses [`config::Config::try_load`] rather than [`config::load_or_setup`]: an
/// unconfigured repository falls back to [`config::Config::default`] (`main`
/// and `master` protected), so the command answers a question instead of
/// asking one.
fn handle_status(git: &git::Git, ui: &ui::Ui, cli: &Cli, merged_only: bool) -> Result<()> {
    let cfg = config::Config::try_load(git)?.unwrap_or_default();
    let filter = branches::Filter::load(git, &cfg)?;

    let opts = status::StatusOptions {
        effort: resolve_effort(cli, &cfg)?,
        jobs: resolve_jobs(cli, &cfg),
        forge: resolve_forge(cli, &cfg),
        // Deliberately not `resolve_min_age`/`resolve_min_size`: here they are
        // display filters, and a `wipe.minage`/`wipe.minsize` configured as a
        // removal safety net must not silently truncate the listing.
        min_age: cli.min_age.unwrap_or_default(),
        min_size: cli.min_size.unwrap_or_default(),
        show_size: cli.size,
        merged_only,
    };

    let forges = opts
        .forge
        .is_enabled()
        .then(|| cleaner::effective_remotes(git, &cfg))
        .transpose()?
        .map(|remotes| forge::RemoteForges::new(git, opts.forge, remotes));
    let forge = forges.as_ref().map(|f| f as &dyn forge::ForgeSource);

    let scan = status::scan(git, &filter, ui, opts, forge)?;
    let total = scan.rows.len();
    let rows = status::filter_rows(scan.rows, opts);

    if cli.json {
        return report::print_json(&status::to_report(&rows, scan.warnings));
    }
    status::render(ui, &rows, total > 0);
    Ok(())
}

fn handle_clean(git: &git::Git, ui: &ui::Ui, cli: &Cli) -> Result<()> {
    // The setup wizard needs a human; in JSON mode, ask for one explicitly
    // rather than inventing a configuration.
    let cfg = if cli.json {
        config::Config::try_load(git)?.ok_or_else(|| {
            anyhow::anyhow!(
                "git-wipe is not configured in this repository. \
                 Run `git wipe` once interactively to complete the setup wizard."
            )
        })?
    } else {
        config::load_or_setup(git, ui)?
    };

    let use_worktrunk = resolve_worktrunk(git, ui, cli, &cfg)?;
    let effort = resolve_effort(cli, &cfg)?;
    let min_age = resolve_min_age(cli, &cfg);
    let min_size = resolve_min_size(cli, &cfg);
    let jobs = resolve_jobs(cli, &cfg);

    let opts = cleaner::CleanerOptions {
        yes: cli.effective_yes(),
        force: cli.force,
        dry_run: cli.dry_run,
        no_fetch: cli.no_fetch,
        no_pull: cli.no_pull,
        local_only: cli.local_only,
        remote_only: cli.remote_only,
        no_worktrees: cli.no_worktrees,
        delete_gone: cli.delete_gone,
        use_worktrunk,
        effort,
        min_age,
        min_size,
        show_size: cli.size,
        jobs,
        forge: resolve_forge(cli, &cfg),
    };

    let report = cleaner::run(git, &cfg, ui, &opts)?;

    if cli.json {
        report::print_json(&report)?;
    }

    Ok(())
}

/// Resolve how thorough merge detection should be.
///
/// Priority: CLI flag > config setting > [`Effort::default`].
fn resolve_effort(cli: &Cli, cfg: &config::Config) -> Result<branches::Effort> {
    if let Some(level) = cli.effort {
        // clap already restricted the range; this only maps it to the enum.
        return branches::Effort::try_from(level);
    }
    Ok(cfg.effort.unwrap_or_default())
}

/// Resolve whether the forge is consulted for merge detection.
///
/// Priority: `--no-forge` > `--forge` > config setting > off. Off is the
/// default because the lookup is networked and sends branch names to the forge.
fn resolve_forge(cli: &Cli, cfg: &config::Config) -> forge::ForgeSetting {
    if cli.no_forge {
        return forge::ForgeSetting::Off;
    }
    cli.forge.or(cfg.forge).unwrap_or_default()
}

/// Resolve the minimum age a worktree must have before it may be removed.
///
/// Priority: CLI flag > config setting > [`duration::MinAge::default`] (no guard).
fn resolve_min_age(cli: &Cli, cfg: &config::Config) -> duration::MinAge {
    cli.min_age.or(cfg.min_age).unwrap_or_default()
}

/// Resolve the minimum on-disk size a worktree must have before it may be
/// removed.
///
/// Priority: CLI flag > config setting > [`size::Size::default`] (no guard).
fn resolve_min_size(cli: &Cli, cfg: &config::Config) -> size::Size {
    cli.min_size.or(cfg.min_size).unwrap_or_default()
}

/// Resolve how many read-only git probes analysis may run at once.
///
/// Priority: `--verbose` > CLI flag > config setting > the CPU count.
///
/// `--verbose` wins outright: it echoes every command as it is spawned, and
/// concurrent workers would interleave those lines into an order that no longer
/// reflects what happened. A debugging aid that lies is worse than a slow one.
fn resolve_jobs(cli: &Cli, cfg: &config::Config) -> usize {
    if cli.verbose {
        return 1;
    }
    cli.jobs.or(cfg.jobs).map_or_else(
        || std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get),
        |jobs| jobs as usize,
    )
}

/// Resolve whether to use worktrunk for worktree removal.
///
/// Priority: CLI flag > config setting > auto-detect from worktrunk config presence.
fn resolve_worktrunk(git: &git::Git, ui: &ui::Ui, cli: &Cli, cfg: &config::Config) -> Result<bool> {
    // 1. Explicit CLI flags take highest priority
    if cli.worktrunk {
        if !git::worktrunk_available() {
            anyhow::bail!(
                "Worktrunk (wt) not found on $PATH. \
                 Install it from https://worktrunk.dev or remove --worktrunk."
            );
        }
        return Ok(true);
    }
    if cli.no_worktrunk {
        return Ok(false);
    }

    // 2. Config setting
    if let Some(val) = cfg.worktrunk {
        if val && !git::worktrunk_available() {
            anyhow::bail!(
                "{}.worktrunk is enabled but worktrunk (wt) is not found on $PATH. \
                 Install it from https://worktrunk.dev or run: \
                 git wipe config set worktrunk false",
                config::SECTION
            );
        }
        return Ok(val);
    }

    // 3. Auto-detect: check if worktrunk config section exists in git config
    if git.worktrunk_config_exists()? && git::worktrunk_available() {
        if cli.effective_yes() {
            return Ok(true);
        }
        return ui.confirm(config::WORKTRUNK_PROMPT, true);
    }

    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::{GitCommandError, GitErrorKind};

    fn git_err(kind: GitErrorKind, stderr: &str) -> anyhow::Error {
        anyhow::Error::new(GitCommandError {
            program: "git".into(),
            args: vec!["fetch".into()],
            exit_code: Some(1),
            stderr: stderr.into(),
            kind,
        })
    }

    #[test]
    fn resolve_min_age_prefers_cli_then_config_then_default() {
        let cfg_default = config::Config::default();
        let cfg_2h = config::Config {
            min_age: Some("2h".parse().unwrap()),
            ..config::Config::default()
        };

        let cli_unset = Cli::parse_from(["git-wipe"]);
        let cli_30m = Cli::parse_from(["git-wipe", "--min-age", "30m"]);

        // Nothing set anywhere: no guard.
        assert!(resolve_min_age(&cli_unset, &cfg_default).is_zero());
        // Config only.
        assert_eq!(resolve_min_age(&cli_unset, &cfg_2h), "2h".parse().unwrap());
        // CLI wins over config.
        assert_eq!(resolve_min_age(&cli_30m, &cfg_2h), "30m".parse().unwrap());
    }

    #[test]
    fn resolve_min_size_prefers_cli_then_config_then_default() {
        let cfg_default = config::Config::default();
        let cfg_100m = config::Config {
            min_size: Some("100M".parse().unwrap()),
            ..config::Config::default()
        };

        let cli_unset = Cli::parse_from(["git-wipe"]);
        let cli_1g = Cli::parse_from(["git-wipe", "--min-size", "1G"]);

        // Nothing set anywhere: no guard.
        assert!(resolve_min_size(&cli_unset, &cfg_default).is_zero());
        // Config only.
        assert_eq!(
            resolve_min_size(&cli_unset, &cfg_100m),
            "100M".parse().unwrap()
        );
        // CLI wins over config.
        assert_eq!(resolve_min_size(&cli_1g, &cfg_100m), "1G".parse().unwrap());
    }

    #[test]
    fn resolve_effort_prefers_cli_then_config_then_default() -> Result<()> {
        let cfg_default = config::Config::default();
        let cfg_thorough = config::Config {
            effort: Some(branches::Effort::Thorough),
            ..config::Config::default()
        };

        let cli_unset = Cli::parse_from(["git-wipe"]);
        let cli_quick = Cli::parse_from(["git-wipe", "--effort", "1"]);

        // Nothing set anywhere: the built-in default.
        assert_eq!(
            resolve_effort(&cli_unset, &cfg_default)?,
            branches::Effort::Standard
        );
        // Config only.
        assert_eq!(
            resolve_effort(&cli_unset, &cfg_thorough)?,
            branches::Effort::Thorough
        );
        // CLI wins over config.
        assert_eq!(
            resolve_effort(&cli_quick, &cfg_thorough)?,
            branches::Effort::Quick
        );
        Ok(())
    }

    #[test]
    fn resolve_jobs_prefers_verbose_then_cli_then_config_then_cpu_count() {
        let cfg_default = config::Config::default();
        let cfg_four = config::Config {
            jobs: Some(4),
            forge: None,
            ..config::Config::default()
        };

        let cli_unset = Cli::parse_from(["git-wipe"]);
        let cli_two = Cli::parse_from(["git-wipe", "--jobs", "2"]);
        let cli_verbose_two = Cli::parse_from(["git-wipe", "--verbose", "-j", "2"]);

        // Nothing set anywhere: as many workers as the machine has cores.
        let cpus = std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get);
        assert_eq!(resolve_jobs(&cli_unset, &cfg_default), cpus);
        // Config only.
        assert_eq!(resolve_jobs(&cli_unset, &cfg_four), 4);
        // CLI wins over config.
        assert_eq!(resolve_jobs(&cli_two, &cfg_four), 2);
        // Verbose wins over everything: an out-of-order trace is worthless.
        assert_eq!(resolve_jobs(&cli_verbose_two, &cfg_four), 1);
    }

    #[test]
    fn report_error_runs_for_network_kind() {
        let ui = ui::Ui::new();
        report_error(
            &ui,
            &git_err(
                GitErrorKind::Network,
                "ssh: connect to host github.com port 22: No route to host",
            ),
        );
    }

    #[test]
    fn report_error_runs_for_auth_kind() {
        let ui = ui::Ui::new();
        report_error(
            &ui,
            &git_err(GitErrorKind::Auth, "Permission denied (publickey)."),
        );
    }

    #[test]
    fn report_error_runs_for_other_kind() {
        let ui = ui::Ui::new();
        report_error(&ui, &git_err(GitErrorKind::Other, "fatal: bad refspec"));
    }

    #[test]
    fn report_error_runs_for_plain_anyhow() {
        let ui = ui::Ui::new();
        let err = anyhow::anyhow!("top").context("inner").context("outer");
        report_error(&ui, &err);
    }

    #[test]
    fn is_cancelled_detects_interrupted_io_error() {
        let err = anyhow::Error::new(std::io::Error::from(std::io::ErrorKind::Interrupted));
        assert!(is_cancelled(&err));
    }

    #[test]
    fn is_cancelled_detects_interrupted_deep_in_chain() {
        let err = anyhow::Error::new(std::io::Error::from(std::io::ErrorKind::Interrupted))
            .context("selecting branches")
            .context("running cleanup");
        assert!(is_cancelled(&err));
    }

    #[test]
    fn is_cancelled_ignores_other_io_errors() {
        let err = anyhow::Error::new(std::io::Error::from(std::io::ErrorKind::NotFound));
        assert!(!is_cancelled(&err));
    }

    #[test]
    fn is_cancelled_ignores_non_io_errors() {
        let err = anyhow::anyhow!("plain error");
        assert!(!is_cancelled(&err));
        assert!(!is_cancelled(&git_err(
            GitErrorKind::Other,
            "fatal: bad refspec"
        )));
    }

    #[test]
    fn fail_renders_text_or_json_without_panicking() {
        let ui = ui::Ui::new();
        let err = git_err(GitErrorKind::Auth, "fatal: Authentication failed");
        // Text mode: goes through report_error on stderr.
        fail(&ui, false, &err);
        // JSON mode: an error document on stdout.
        fail(&ui::Ui::quiet(), true, &err);
    }

    #[test]
    fn report_cancelled_renders_text_or_json_without_panicking() {
        report_cancelled(&ui::Ui::new(), false);
        report_cancelled(&ui::Ui::quiet(), true);
    }

    #[test]
    fn list_config_json_reports_an_unconfigured_repository() -> Result<()> {
        let (_dir, git) = test_helpers::init_repo()?;
        list_config_json(&git)
    }

    #[test]
    fn list_config_json_reports_a_configured_repository() -> Result<()> {
        let (_dir, git) = test_helpers::init_repo()?;
        config::Config {
            protected: vec!["main".to_string()],
            effort: Some(branches::Effort::Quick),
            ..config::Config::default()
        }
        .save(&git)?;
        list_config_json(&git)
    }

    #[test]
    fn handle_clean_refuses_to_run_json_without_a_configuration() -> Result<()> {
        let (_dir, git) = test_helpers::init_repo()?;
        let cli = Cli::parse_from(["git-wipe", "--json", "--no-fetch"]);

        let err = handle_clean(&git, &ui::Ui::quiet(), &cli)
            .expect_err("an unconfigured repository must not start the wizard");
        assert!(
            err.to_string().contains("not configured"),
            "unexpected error: {err}"
        );
        Ok(())
    }

    #[test]
    fn handle_clean_prints_a_json_document() -> Result<()> {
        let (_dir, git) = test_helpers::init_repo_with_branches()?;
        config::Config {
            protected: vec!["main".to_string()],
            ..config::Config::default()
        }
        .save(&git)?;
        let cli = Cli::parse_from(["git-wipe", "--json", "--no-fetch", "--no-pull", "--dry-run"]);

        handle_clean(&git, &ui::Ui::quiet(), &cli)
    }

    #[test]
    fn handle_status_runs_in_an_unconfigured_repository() -> Result<()> {
        // The regression test for the acceptance criterion: no configuration,
        // and still no wizard. A prompt would fail on the quiet UI.
        let (_dir, git) = test_helpers::init_repo_with_branches()?;
        let cli = Cli::parse_from(["git-wipe", "status"]);
        handle_status(&git, &ui::Ui::quiet(), &cli, false)
    }

    #[test]
    fn handle_status_prints_a_json_document() -> Result<()> {
        let (_dir, git) = test_helpers::init_repo_with_branches()?;
        let cli = Cli::parse_from(["git-wipe", "status", "--json"]);
        handle_status(&git, &ui::Ui::quiet(), &cli, false)
    }

    #[test]
    fn handle_status_does_not_inherit_the_configured_min_age() -> Result<()> {
        // `wipe.minage` guards worktree removal; it must not silently truncate
        // an inventory the user asked no filter for.
        let (_dir, git) = test_helpers::init_repo_with_branches()?;
        config::Config {
            protected: vec!["main".to_string()],
            min_age: Some("52w".parse().unwrap()),
            ..config::Config::default()
        }
        .save(&git)?;

        let cli = Cli::parse_from(["git-wipe", "status", "--json"]);
        let cfg = config::Config::try_load(&git)?.expect("just saved");
        let filter = branches::Filter::load(&git, &cfg)?;
        let opts = status::StatusOptions {
            effort: resolve_effort(&cli, &cfg)?,
            jobs: resolve_jobs(&cli, &cfg),
            min_age: cli.min_age.unwrap_or_default(),
            min_size: cli.min_size.unwrap_or_default(),
            show_size: cli.size,
            merged_only: false,
            forge: forge::ForgeSetting::Off,
        };
        assert!(opts.min_age.is_zero(), "wipe.minage must not leak in");

        let scan = status::scan(&git, &filter, &ui::Ui::quiet(), opts, None)?;
        assert!(!status::filter_rows(scan.rows, opts).is_empty());
        Ok(())
    }

    #[test]
    fn handle_status_does_not_inherit_the_configured_min_size() -> Result<()> {
        // `wipe.minsize` guards worktree removal; it must not silently
        // truncate an inventory the user asked no filter for.
        let (_dir, git) = test_helpers::init_repo_with_branches()?;
        config::Config {
            protected: vec!["main".to_string()],
            min_size: Some("1G".parse().unwrap()),
            ..config::Config::default()
        }
        .save(&git)?;

        let cli = Cli::parse_from(["git-wipe", "status", "--json"]);
        let cfg = config::Config::try_load(&git)?.expect("just saved");
        let filter = branches::Filter::load(&git, &cfg)?;
        let opts = status::StatusOptions {
            effort: resolve_effort(&cli, &cfg)?,
            jobs: resolve_jobs(&cli, &cfg),
            min_age: cli.min_age.unwrap_or_default(),
            min_size: cli.min_size.unwrap_or_default(),
            show_size: cli.size,
            merged_only: false,
            forge: forge::ForgeSetting::Off,
        };
        assert!(opts.min_size.is_zero(), "wipe.minsize must not leak in");

        let scan = status::scan(&git, &filter, &ui::Ui::quiet(), opts, None)?;
        assert!(!status::filter_rows(scan.rows, opts).is_empty());
        Ok(())
    }

    #[test]
    fn resolve_forge_prefers_no_forge_then_cli_then_config_then_off() {
        use forge::{ForgeKind, ForgeSetting};
        let cfg_off = config::Config::default();
        let cfg_on = config::Config {
            forge: Some(ForgeSetting::Auto),
            ..config::Config::default()
        };

        let plain = Cli::parse_from(["git-wipe"]);
        let flag = Cli::parse_from(["git-wipe", "--forge=gitea"]);
        let bare = Cli::parse_from(["git-wipe", "--forge"]);
        let no = Cli::parse_from(["git-wipe", "--no-forge"]);

        // Opt-in: nothing set means off.
        assert_eq!(resolve_forge(&plain, &cfg_off), ForgeSetting::Off);
        // Config alone enables it.
        assert_eq!(resolve_forge(&plain, &cfg_on), ForgeSetting::Auto);
        // CLI wins over config.
        assert_eq!(
            resolve_forge(&flag, &cfg_on),
            ForgeSetting::Kind(ForgeKind::Gitea)
        );
        assert_eq!(resolve_forge(&bare, &cfg_off), ForgeSetting::Auto);
        // `--no-forge` beats config.
        assert_eq!(resolve_forge(&no, &cfg_on), ForgeSetting::Off);
    }
}
