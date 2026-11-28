//! Terminal output and prompts.
//!
//! All user-facing output goes through [`Ui`] so styling, indentation and
//! error classification stay consistent. Output methods are best-effort and
//! swallow I/O errors; prompts propagate them, since they need an answer.

use choisir::{MultiSelect, SearchMode, theme::MinimalTheme};
use console::{Style, Term};
use dialoguer::{Confirm, Input, theme::Theme};
use indicatif::{MultiProgress, ProgressBar, ProgressFinish, ProgressStyle};
use std::{collections::HashMap, fmt};

/// Choisir sanitizes labels and puts descriptions on a separate row, so the
/// theme owns our inline metadata without adding it to the searchable label.
struct SelectionTheme {
    base: MinimalTheme,
    selected_name: Style,
    path_style: Style,
    status_style: Style,
    details: HashMap<String, SelectionDetails>,
}

struct SelectionDetails {
    path: String,
    hint: String,
    path_only: bool,
}

/// Search branch names independently of display-only worktree paths.
fn searchable_label<'a>(value: &'a str, label: &'a str) -> (&'a str, &'a str) {
    let branch = value.strip_prefix("branch:").unwrap_or(value);
    match label.strip_prefix(branch) {
        Some(path) if path.starts_with(" (") => (branch, path),
        _ => (label, ""),
    }
}

impl SelectionTheme {
    fn new(values: &[String], labels: &[String], hints: &[String]) -> Self {
        let details = values
            .iter()
            .enumerate()
            .map(|(i, value)| {
                let (label, path) = searchable_label(value, labels.get(i).unwrap_or(value));
                let label = clean_prompt_text(label);
                let path_only = value.starts_with("orphan-wt:")
                    || label.starts_with('/')
                    || label.starts_with("~/");
                let hint = hints
                    .get(i)
                    .map(|hint| clean_prompt_text(hint))
                    .unwrap_or_default();
                (
                    label,
                    SelectionDetails {
                        path: clean_prompt_text(path),
                        hint,
                        path_only,
                    },
                )
            })
            .collect();
        Self {
            base: MinimalTheme {
                // The wrapper colors label parts; selection must not recolor paths or status.
                selected_label_style: Style::new(),
                unselected_label_style: Style::new(),
                ..Default::default()
            },
            selected_name: Style::new().for_stderr().green(),
            path_style: Style::new().for_stderr().dim(),
            status_style: Style::new().for_stderr().dim().cyan(),
            details,
        }
    }

    fn label(&self, text: &str, checked: bool) -> String {
        let plain = console::strip_ansi_codes(text);
        let details = self.details.get(plain.as_ref());
        // Preserve chooser-provided highlights on searchable text only.
        let (name, path) = match details {
            Some(details) if details.path_only => ("", text),
            Some(details) => (text, details.path.as_str()),
            None => (text, ""),
        };
        let name = if checked {
            self.selected_name.apply_to(name).to_string()
        } else {
            name.to_owned()
        };
        let mut label = format!("{name}{}", self.path_style.apply_to(path));
        if let Some(details) = details.filter(|details| !details.hint.is_empty()) {
            label.push_str(&format!(" {}", self.status_style.apply_to(&details.hint)));
        }
        label
    }
}

fn clean_prompt_text(text: &str) -> String {
    // Metadata bypasses choisir's description renderer; retain its control sanitization.
    console::strip_ansi_codes(text)
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

impl Theme for SelectionTheme {
    fn format_multi_select_prompt(&self, f: &mut dyn fmt::Write, prompt: &str) -> fmt::Result {
        self.base.format_multi_select_prompt(f, prompt)
    }

    fn format_multi_select_prompt_selection(
        &self,
        f: &mut dyn fmt::Write,
        prompt: &str,
        selections: &[&str],
    ) -> fmt::Result {
        // Trait hooks do not forward automatically; accepted labels need the same partial styling.
        let labels: Vec<_> = selections
            .iter()
            .map(|text| self.label(text, true))
            .collect();
        let refs: Vec<_> = labels.iter().map(String::as_str).collect();
        self.base
            .format_multi_select_prompt_selection(f, prompt, &refs)
    }

    fn format_error(&self, f: &mut dyn fmt::Write, error: &str) -> fmt::Result {
        self.base.format_error(f, error)
    }

    fn format_multi_select_prompt_item(
        &self,
        f: &mut dyn fmt::Write,
        text: &str,
        checked: bool,
        active: bool,
    ) -> fmt::Result {
        self.base
            .format_multi_select_prompt_item(f, &self.label(text, checked), checked, active)
    }
}

/// Keep optional prompt cancellation compatible with the binary's error-chain check.
fn prompt_answer<T>(answer: Option<T>) -> anyhow::Result<T> {
    answer.ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::Interrupted, "prompt cancelled").into()
    })
}

/// A single progress block whose pending rows are replaced in place on terminals.
pub struct ProgressGroup<'a> {
    ui: &'a Ui,
    rows: Vec<String>,
    failed: bool,
    live: Option<LiveProgressGroup>,
}

struct LiveProgressGroup {
    display: MultiProgress,
    heading: ProgressBar,
    rows: Vec<ProgressBar>,
}

impl ProgressGroup<'_> {
    /// Replace one pending row without appending a second copy of its result.
    pub fn item_finished(&mut self, index: usize, text: &str, succeeded: bool) {
        let marker = if succeeded {
            console::style("✔").for_stderr().green()
        } else {
            self.failed = true;
            console::style("✘").for_stderr().red()
        };
        self.rows[index] = format!("  {marker} {text}");
        if let Some(live) = &self.live {
            live.rows[index].set_message(self.rows[index].clone());
        }
    }

    /// Leave exactly one completed block, including when output is redirected.
    pub fn finish(mut self, title: &str) {
        let marker = if self.failed {
            console::style("⚠").for_stderr().yellow()
        } else {
            console::style("✔").for_stderr().green()
        };
        let heading = format!("{marker} {}", self.ui.heading_style.apply_to(title));
        if let Some(live) = self.live.take() {
            live.heading.disable_steady_tick();
            live.heading
                .set_style(ProgressStyle::with_template("{msg}").expect("valid progress template"));
            live.heading.set_message(heading);
            // Finish top-to-bottom so MultiProgress commits the block in display order.
            live.heading.finish();
            for row in live.rows {
                row.finish();
            }
        } else {
            self.ui.write_line(&heading);
            for row in &self.rows {
                self.ui.write_line(row);
            }
        }
    }
}

impl Drop for ProgressGroup<'_> {
    fn drop(&mut self) {
        // An unwinding operation must not leave an animation or unfinished frame behind.
        if let Some(live) = &self.live {
            live.heading.finish_and_clear();
            for row in &live.rows {
                row.finish_and_clear();
            }
            let _ = live.display.clear();
        }
    }
}

/// Return a display-friendly path with `$HOME` replaced by `~`.
///
/// Display only: the result is never fed back to git, which would not expand
/// the tilde. JSON reports keep the absolute path via
/// [`crate::report::path_string`].
pub fn tilde_path(abs: &std::path::Path) -> String {
    let abs = abs.to_string_lossy();
    if let Ok(home) = std::env::var("HOME")
        && let Some(rest) = abs.strip_prefix(&home)
    {
        return format!("~{rest}");
    }
    abs.into_owned()
}

/// Terminal handle and style presets for consistent output.
#[derive(Debug)]
pub struct Ui {
    term: Term,
    /// Where tabular *data* goes. See [`Ui::table_row`].
    stdout: Term,
    heading_style: Style,
    muted_style: Style,
    bold_style: Style,
    /// Suppress every output method (used by `--json`, where the machine
    /// readable document is the only output).
    quiet: bool,
}

impl Default for Ui {
    fn default() -> Self {
        Self::new()
    }
}

impl Ui {
    /// Create a handle writing to stderr, so output never pollutes a pipe.
    pub fn new() -> Self {
        Self {
            term: Term::stderr(),
            stdout: Term::stdout(),
            heading_style: Style::new().for_stderr().white().bold(),
            muted_style: Style::new().dim(),
            bold_style: Style::new().bold(),
            quiet: false,
        }
    }

    /// Create a handle that prints nothing.
    ///
    /// Used by `--json`, where human-readable output would be noise. Prompt
    /// methods return an error instead: JSON mode implies `--yes`, so they are
    /// unreachable, and silently answering on the user's behalf would be worse
    /// than failing loudly.
    pub fn quiet() -> Self {
        Self {
            quiet: true,
            ..Self::new()
        }
    }

    /// Write a line unless muted. Every output method funnels through this.
    fn write_line(&self, line: &str) {
        if self.quiet {
            return;
        }
        let _ = self.term.write_line(line);
    }

    /// Write a line of *data* to stdout unless muted. See [`Ui::table_row`].
    fn write_data_line(&self, line: &str) {
        if self.quiet {
            return;
        }
        let _ = self.stdout.write_line(line);
    }

    fn prompt_unavailable<T>(&self) -> anyhow::Result<T> {
        anyhow::bail!("cannot prompt in --json mode")
    }

    // Output methods below are best-effort: I/O errors (e.g. broken pipe)
    // are silently discarded because failing to *display* a message should
    // not abort the cleanup workflow. Interactive methods (confirm,
    // multi_select, input) propagate errors because they need a response.

    /// Print a section heading.
    pub fn heading(&self, text: &str) {
        self.write_line(&self.heading_style.apply_to(text).to_string());
    }

    /// Animate a heading and its item rows; redirected output gets only the final block.
    /// Verbose command echoes require a plain log rather than a repainting region.
    pub fn progress_group(&self, title: &str, items: &[String], live: bool) -> ProgressGroup<'_> {
        let rows: Vec<_> = items.iter().map(|item| format!("  - {item}")).collect();
        let live = if live && !self.quiet && self.term.is_term() {
            let display = MultiProgress::new();
            let heading = display.add(
                ProgressBar::new_spinner()
                    .with_style(
                        ProgressStyle::with_template("{spinner:.blue} {msg}")
                            .expect("valid progress template"),
                    )
                    .with_message(self.heading_style.apply_to(title).to_string()),
            );
            let bars = rows
                .iter()
                .map(|row| {
                    let bar = display.add(
                        ProgressBar::new_spinner()
                            .with_style(
                                ProgressStyle::with_template("{msg}")
                                    .expect("valid progress template"),
                            )
                            .with_message(row.clone()),
                    );
                    // Static rows need an initial tick to enter MultiProgress's draw state.
                    bar.tick();
                    bar
                })
                .collect();
            heading.enable_steady_tick(std::time::Duration::from_millis(80));
            Some(LiveProgressGroup {
                display,
                heading,
                rows: bars,
            })
        } else {
            None
        };
        ProgressGroup {
            ui: self,
            rows,
            failed: false,
            live,
        }
    }

    /// Print a success message with a green checkmark prefix.
    ///
    /// The text is printed as-is (not re-colored), so callers can embed
    /// pre-styled fragments via `console::style()`.
    pub fn success(&self, text: &str) {
        self.write_line(&format!("{} {text}", console::style("✔").green()));
    }

    /// Print a warning with a yellow ⚠ prefix.
    ///
    /// The text is printed as-is, so callers can embed pre-styled fragments.
    pub fn warning(&self, text: &str) {
        self.write_line(&format!("{} {text}", console::style("⚠").yellow()));
    }

    /// Print an error with a red ✘ prefix.
    ///
    /// The text is printed as-is, so callers can embed pre-styled fragments.
    pub fn error(&self, text: &str) {
        self.write_line(&format!("{} {text}", console::style("✘").red()));
    }

    /// Render a failed git operation with a friendly classification.
    ///
    /// Emits a single coloured line whose prefix is chosen from the
    /// [`GitErrorKind`] of the underlying [`GitCommandError`], so a network or
    /// auth failure reads as such instead of a bare "Failed to ...". Returns
    /// the detected kind so callers can decide whether to surface a follow-up
    /// warning (e.g. "detection may be stale").
    ///
    /// `action` is an infinitive phrase ("fetch from", "delete", "remove") and
    /// `target` the thing acted upon.
    pub fn report_failure(
        &self,
        action: &str,
        target: &str,
        err: &anyhow::Error,
    ) -> crate::git::GitErrorKind {
        use crate::git::{GitCommandError, GitErrorKind};

        let styled = console::style(target).red();
        if let Some(gerr) = err.downcast_ref::<GitCommandError>() {
            let cause = gerr.short_cause();
            match gerr.kind {
                GitErrorKind::Network => {
                    self.error(&format!(
                        "Network error: cannot {action} '{styled}' ({cause})."
                    ));
                }
                GitErrorKind::Auth => {
                    self.error(&format!(
                        "Authentication failed while trying to {action} '{styled}': {cause}"
                    ));
                }
                GitErrorKind::Other => {
                    self.error(&format!("Failed to {action} '{styled}': {cause}"));
                }
            }
            gerr.kind
        } else {
            self.error(&format!("Failed to {action} '{styled}': {err}"));
            GitErrorKind::Other
        }
    }

    /// Print muted/dim text.
    pub fn muted(&self, text: &str) {
        self.write_line(&self.muted_style.apply_to(text).to_string());
    }

    /// Print a blank line.
    pub fn blank(&self) {
        self.write_line("");
    }

    /// Print a list of items with a bullet prefix.
    pub fn bullet_list(&self, items: &[String]) {
        for item in items {
            self.write_line(&format!("  {} {}", self.muted_style.apply_to("-"), item));
        }
    }

    /// Print an indented, dimmed line describing an action `--dry-run`
    /// suppressed.
    ///
    /// Owns both the indentation and the `(dry-run)` marker so every preview
    /// line is formatted identically.
    pub fn dry_run(&self, text: &str) {
        self.muted(&format!("  (dry-run) {text}"));
    }

    /// Print an indented `label: value` pair, with the label emphasised.
    ///
    /// Owns both the indentation and the label styling so callers never need
    /// to reach for the style presets themselves.
    pub fn field(&self, label: &str, value: &str) {
        self.write_line(&format!(
            "  {} {}",
            self.bold_style.apply_to(format!("{label}:")),
            value
        ));
    }

    /// Print a table header row in bold.
    ///
    /// The caller owns the column padding: widths must be computed on the
    /// unstyled strings, since ANSI escapes would otherwise be counted as
    /// visible characters.
    pub fn table_header(&self, line: &str) {
        self.write_data_line(&self.bold_style.apply_to(line).to_string());
    }

    /// Print a pre-formatted, possibly pre-styled table row verbatim.
    ///
    /// Tables go to **stdout**, unlike every other output method: a listing is
    /// the answer the user asked for, not a log about producing it, and
    /// `git wipe status | grep` must work. Nothing else writes to stdout in
    /// text mode, so the stream stays a clean, greppable table; `--json` mutes
    /// this along with everything else and prints its document instead.
    pub fn table_row(&self, line: &str) {
        self.write_data_line(line);
    }

    /// Ask for confirmation, pre-selecting `default`.
    pub fn confirm(&self, prompt: &str, default: bool) -> anyhow::Result<bool> {
        if self.quiet {
            return self.prompt_unavailable();
        }
        prompt_answer(
            Confirm::with_theme(&MinimalTheme::default())
                .with_prompt(prompt)
                .default(default)
                .interact_on_opt(&self.term)?,
        )
    }

    /// Present a multi-select list. Returns the selected values.
    ///
    /// `values` are the returned items; `labels` are what the user sees;
    /// `hints` are optional dimmed secondary text rendered inline with each item
    /// (pass an empty slice to omit hints).
    /// Ctrl+A or Home inverts the whole selection, including filtered-out items.
    ///
    /// `filterable` enables case-insensitive substring typeahead over branch names
    /// (or paths for orphan worktrees)
    /// (arrow keys, Space and Enter still work while filtering; Esc clears it).
    /// Pass `true` for lists that can grow long (branches, worktrees);
    /// `false` keeps the plain list for short, fixed-size ones (remotes).
    pub fn multi_select(
        &self,
        prompt: &str,
        values: &[String],
        labels: &[String],
        defaults: &[bool],
        hints: &[String],
        filterable: bool,
    ) -> anyhow::Result<Vec<String>> {
        if self.quiet {
            return self.prompt_unavailable();
        }
        // Choisir rejects empty lists, but no candidates is a valid empty answer.
        if values.is_empty() {
            return Ok(Vec::new());
        }
        let theme = SelectionTheme::new(values, labels, hints);
        let selected: Vec<usize> = (0..values.len())
            .filter(|&i| defaults.get(i).copied().unwrap_or(false))
            .collect();
        let mut ms = MultiSelect::new()
            .with_theme(&theme)
            .with_prompt(prompt)
            .min(0)
            .search(if filterable {
                SearchMode::Typeahead
            } else {
                SearchMode::Disabled
            });
        for (i, val) in values.iter().enumerate() {
            let label = labels.get(i).unwrap_or(val);
            let (search_label, _) = searchable_label(val, label);
            ms = ms.item(val.clone(), search_label);
        }
        prompt_answer(ms.defaults(&selected).interact_on_opt(&self.term)?)
    }

    /// Ask for a text input.
    pub fn input(&self, prompt: &str, default: &str) -> anyhow::Result<String> {
        if self.quiet {
            return self.prompt_unavailable();
        }
        Ok(Input::<String>::with_theme(&MinimalTheme::default())
            .with_prompt(prompt)
            // Initial text remains editable; a fallback default would prevent clearing it.
            .with_initial_text(default)
            .allow_empty(true)
            .interact_text_on(&self.term)?)
    }

    /// Run `op` while showing a spinner labelled `title` on stderr.
    ///
    /// Only renders on a TTY. On non-TTY stderr (pipes, CI, tests) it runs
    /// `op` directly, without a background thread. SIGINT retains the normal
    /// process behavior rather than installing a progress-specific handler.
    ///
    /// The spinner clears its own line before returning, so callers must
    /// print any success/error line *after* this returns, never inside `op`.
    pub fn spinner<T, F>(&self, title: &str, op: F) -> T
    where
        F: FnOnce() -> T + Send,
        T: Send,
    {
        if self.quiet || !self.term.is_term() {
            return op();
        }
        let spinner = ProgressBar::new_spinner()
            .with_style(
                ProgressStyle::with_template("{spinner:.blue} {msg}")
                    .expect("valid progress template"),
            )
            .with_message(self.heading_style.apply_to(title).to_string())
            // Drop also clears the display if the operation unwinds.
            .with_finish(ProgressFinish::AndClear);
        spinner.enable_steady_tick(std::time::Duration::from_millis(80));
        let result = op();
        spinner.finish_and_clear();
        result
    }

    /// Print a summary line: "✔ 1 branch deleted." or "✔ 3 branches deleted."
    ///
    /// The count and noun are styled in cyan; the verb and period use the
    /// terminal's default colour.
    pub fn summary(&self, count: usize, singular: &str, plural: &str, verb: &str) {
        let noun = if count == 1 { singular } else { plural };
        self.success(&format!(
            "{} {verb}.",
            console::style(format!("{count} {noun}")).cyan(),
        ));
    }

    /// Print a "freed" summary line: "✔ 3.4G freed."
    ///
    /// Styled like [`Ui::summary`]: the value in cyan, the verb and period in
    /// the terminal's default colour.
    pub fn freed(&self, bytes: u64) {
        self.success(&format!(
            "{} freed.",
            console::style(crate::size::format_bytes(bytes)).cyan(),
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::{GitCommandError, GitErrorKind};

    fn git_err(kind: GitErrorKind, stderr: &str) -> anyhow::Error {
        anyhow::Error::new(GitCommandError {
            program: "git".into(),
            args: vec!["fetch".into(), "--prune".into(), "origin".into()],
            exit_code: Some(1),
            stderr: stderr.into(),
            kind,
        })
    }

    #[test]
    fn report_failure_classifies_network() {
        let ui = Ui::new();
        let err = git_err(
            GitErrorKind::Network,
            "ssh: connect to host github.com port 22: No route to host",
        );
        assert_eq!(
            ui.report_failure("fetch from", "origin", &err),
            GitErrorKind::Network
        );
    }

    #[test]
    fn report_failure_classifies_auth() {
        let ui = Ui::new();
        let err = git_err(GitErrorKind::Auth, "Permission denied (publickey).");
        assert_eq!(
            ui.report_failure("fetch from", "origin", &err),
            GitErrorKind::Auth
        );
    }

    #[test]
    fn report_failure_classifies_other() {
        let ui = Ui::new();
        let err = git_err(GitErrorKind::Other, "fatal: refusing to fetch");
        assert_eq!(
            ui.report_failure("fetch from", "origin", &err),
            GitErrorKind::Other
        );
    }

    #[test]
    fn report_failure_handles_non_git_error() {
        let ui = Ui::new();
        let err = anyhow::anyhow!("something went wrong");
        assert_eq!(
            ui.report_failure("pull", "feature", &err),
            GitErrorKind::Other
        );
    }

    #[test]
    fn ui_default() {
        let ui = Ui::default();
        // Smoke test: styles should be constructable
        let styled = ui.heading_style.apply_to("test");
        assert!(styled.to_string().contains("test"));
    }

    #[test]
    fn success_does_not_panic() {
        let ui = Ui::new();
        ui.success("plain message");
        ui.success(&format!("with {} styling", console::style("cyan").cyan()));
    }

    #[test]
    fn warning_does_not_panic() {
        let ui = Ui::new();
        ui.warning("plain warning");
        ui.warning(&format!(
            "with {} styling",
            console::style("yellow").yellow()
        ));
    }

    #[test]
    fn error_does_not_panic() {
        let ui = Ui::new();
        ui.error("plain error");
        ui.error(&format!("with {} styling", console::style("red").red()));
    }

    #[test]
    fn summary_singular() {
        let ui = Ui::new();
        ui.summary(1, "branch", "branches", "deleted");
    }

    #[test]
    fn summary_plural() {
        let ui = Ui::new();
        ui.summary(5, "branch", "branches", "deleted");
    }

    #[test]
    fn spinner_is_noop_and_returns_value_when_not_a_tty() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let ui = Ui::new(); // stderr not a TTY under tests
        let ran = AtomicBool::new(false);
        let out: i32 = ui.spinner("work", || {
            ran.store(true, Ordering::SeqCst);
            42
        });
        assert!(ran.load(Ordering::SeqCst));
        assert_eq!(out, 42);
    }

    #[test]
    fn quiet_output_methods_do_not_panic() {
        let ui = Ui::quiet();
        ui.heading("heading");
        ui.success("success");
        ui.warning("warning");
        ui.error("error");
        ui.muted("muted");
        ui.blank();
        ui.bullet_list(&["one".to_string()]);
        ui.dry_run("would do something");
        ui.field("label", "value");
        ui.summary(2, "branch", "branches", "deleted");
        assert_eq!(
            ui.report_failure("pull", "feature", &anyhow::anyhow!("boom")),
            GitErrorKind::Other
        );
    }

    #[test]
    fn quiet_spinner_still_runs_the_operation() {
        let ui = Ui::quiet();
        assert_eq!(ui.spinner("work", || 7), 7);
    }

    #[test]
    fn quiet_prompts_fail() {
        let ui = Ui::quiet();
        assert!(ui.confirm("ok?", true).is_err());
        assert!(ui.input("name", "default").is_err());
        assert!(
            ui.multi_select("pick", &["a".to_string()], &[], &[], &[], true)
                .is_err()
        );
        assert!(ui.multi_select("pick", &[], &[], &[], &[], true).is_err());
    }

    #[test]
    fn cancelling_an_optional_prompt_is_recognized_as_cancellation() {
        let error = prompt_answer::<bool>(None)
            .unwrap_err()
            .context("confirmation failed");
        assert!(crate::is_cancelled(&error));
        assert!(!prompt_answer(Some(false)).unwrap());
    }

    #[test]
    fn an_empty_candidate_list_needs_no_terminal() {
        assert!(
            Ui::new()
                .multi_select("pick", &[], &[], &[], &[], true)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn inline_status_is_sanitized_without_becoming_part_of_the_label() {
        let theme = SelectionTheme::new(
            &["branch:topic".into()],
            &["topic (/tmp/topic)".into()],
            &["pr-merged + worktree\n\x1b[31mextra".into()],
        );
        let mut rendered = String::new();
        theme
            .format_multi_select_prompt_item(&mut rendered, "topic", true, false)
            .unwrap();
        assert_eq!(
            console::strip_ansi_codes(&rendered),
            "  ✔ topic (/tmp/topic) pr-merged + worktree extra"
        );
    }

    #[test]
    fn orphan_worktrees_remain_searchable_by_path() {
        assert_eq!(
            searchable_label("orphan-wt:/tmp/topic", "/tmp/topic"),
            ("/tmp/topic", "")
        );
    }

    #[test]
    fn focus_and_selection_leave_path_and_status_styles_unchanged() {
        let mut theme = SelectionTheme::new(
            &["branch:topic".into()],
            &["topic (/tmp/topic)".into()],
            &["pr-merged + worktree".into()],
        );
        // Force individual styles instead of mutating console's process-wide color policy.
        theme.base.focus_style = theme.base.focus_style.force_styling(true);
        theme.base.checked_style = theme.base.checked_style.force_styling(true);
        theme.selected_name = theme.selected_name.force_styling(true);
        theme.path_style = theme.path_style.force_styling(true);
        theme.status_style = theme.status_style.force_styling(true);
        let metadata = format!(
            "{} {}",
            theme.path_style.apply_to(" (/tmp/topic)"),
            theme.status_style.apply_to("pr-merged + worktree")
        );
        for checked in [false, true] {
            for active in [false, true] {
                let mut row = String::new();
                theme
                    .format_multi_select_prompt_item(&mut row, "topic", checked, active)
                    .unwrap();
                assert!(row.ends_with(&metadata), "{row:?}");
                assert_eq!(row.contains("\x1b[32mtopic\x1b[0m"), checked, "{row:?}");
                assert_eq!(row.contains("\x1b[32m✔\x1b[0m"), checked, "{row:?}");
                assert_eq!(row.contains("\x1b[33m>\x1b[0m"), active, "{row:?}");
                if !checked {
                    assert!(row.contains("⬚ topic"), "{row:?}");
                }
            }
        }
        let mut summary = String::new();
        theme
            .format_multi_select_prompt_selection(&mut summary, "Branches", &["topic"])
            .unwrap();
        assert!(summary.contains("\x1b[32mtopic\x1b[0m"), "{summary:?}");
        assert!(summary.ends_with(&metadata), "{summary:?}");
    }

    #[test]
    #[cfg(unix)]
    fn tilde_path_replaces_home() {
        let home = std::env::var("HOME").expect("HOME must be set for this test");
        assert_eq!(
            tilde_path(&std::path::PathBuf::from(format!("{home}/projects/repo"))),
            "~/projects/repo"
        );
    }

    #[test]
    #[cfg(unix)]
    fn tilde_path_preserves_non_home_path() {
        assert_eq!(
            tilde_path(std::path::Path::new("/tmp/some/path")),
            "/tmp/some/path"
        );
    }

    #[test]
    #[cfg(unix)]
    fn tilde_path_exact_home() {
        let home = std::env::var("HOME").expect("HOME must be set for this test");
        // Exact HOME path (no trailing slash) should become just "~"
        assert_eq!(tilde_path(std::path::Path::new(&home)), "~");
    }

    #[test]
    fn table_helpers_do_not_panic() {
        let ui = Ui::new();
        ui.table_header("AGE  STATUS  BRANCH  PATH");
        ui.table_row("3d   merged  feature ~/repo");
        let quiet = Ui::quiet();
        quiet.table_header("AGE");
        quiet.table_row("3d");
    }
}
