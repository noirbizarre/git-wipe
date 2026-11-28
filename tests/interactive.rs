//! Real-terminal regression coverage for the prompt adapter and cleanup policy.
#![cfg(unix)]

use std::io::{Read, Write};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use portable_pty::{Child, MasterPty};
use tempfile::TempDir;

mod common;
use common::{configure, git_branches, init_repo};

struct Session {
    child: Box<dyn Child + Send + Sync>,
    // Retaining the master keeps the terminal alive and exposes input readiness.
    master: Box<dyn MasterPty + Send>,
    input: Box<dyn Write + Send>,
    output: Receiver<Vec<u8>>,
    transcript: Vec<u8>,
    consumed: usize,
}

impl Session {
    fn start(dir: &TempDir, args: &[&str]) -> Self {
        Self::start_with_rows(dir, args, 30)
    }

    fn start_with_rows(dir: &TempDir, args: &[&str], rows: u16) -> Self {
        let pair = portable_pty::native_pty_system()
            .openpty(portable_pty::PtySize {
                rows,
                cols: 140,
                ..Default::default()
            })
            .unwrap();
        let mut command =
            portable_pty::CommandBuilder::new(assert_cmd::cargo::cargo_bin!("git-wipe"));
        command.cwd(dir.path());
        command.args(args);
        command.env("TERM", "xterm-256color");
        for key in ["NO_COLOR", "FORCE_COLOR", "CLICOLOR", "CLICOLOR_FORCE"] {
            command.env_remove(key);
        }
        // Exercise styling deterministically; --no-color must still take precedence.
        command.env("FORCE_COLOR", "1");
        let child = pair.slave.spawn_command(command).unwrap();
        drop(pair.slave);
        let input = pair.master.take_writer().unwrap();
        let mut reader = pair.master.try_clone_reader().unwrap();
        let (sender, output) = mpsc::channel();
        // Drain independently so a repaint cannot fill the PTY and deadlock a test.
        std::thread::spawn(move || {
            let mut buffer = [0; 4096];
            while let Ok(count) = reader.read(&mut buffer) {
                if count == 0 || sender.send(buffer[..count].to_vec()).is_err() {
                    break;
                }
            }
        });
        Self {
            child,
            master: pair.master,
            input,
            output,
            transcript: Vec::new(),
            consumed: 0,
        }
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.transcript).into_owned()
    }

    fn screen(&self) -> String {
        let mut terminal = vt100::Parser::new(30.try_into().unwrap(), 140.try_into().unwrap(), 0);
        terminal.process(&self.transcript);
        // Indicatif pads full-width rows, which contents() joins as soft wraps.
        // Physical rows match what the user sees on screen.
        terminal
            .screen()
            .rows(0, 140)
            .map(|row| row.trim_end().to_owned())
            .collect::<Vec<_>>()
            .join("\n")
            .trim_end()
            .to_owned()
    }

    fn expect(&mut self, text: &str) {
        self.expect_one_of(&[text]);
    }

    fn expect_one_of(&mut self, texts: &[&str]) -> usize {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let remaining = String::from_utf8_lossy(&self.transcript[self.consumed..]);
            for (choice, text) in texts.iter().enumerate() {
                if let Some(index) = remaining.find(text) {
                    self.consumed += index + text.len();
                    return choice;
                }
            }
            let chunk = self
                .output
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .unwrap_or_else(|error| panic!("waiting for {texts:?}: {error}\n{}", self.text()));
            self.transcript.extend(chunk);
        }
    }

    fn send(&mut self, keys: &[u8]) {
        use nix::sys::termios::LocalFlags;

        let deadline = Instant::now() + Duration::from_secs(15);
        // Prompts flush before entering raw mode. Sending editing/control keys
        // earlier lets the line discipline swallow Backspace or turn Ctrl+C into SIGINT.
        loop {
            let flags = self
                .master
                .get_termios()
                .expect("PTY termios must be available")
                .local_flags;
            if !flags.intersects(LocalFlags::ICANON | LocalFlags::ECHO | LocalFlags::ISIG) {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "prompt did not enter raw mode\n{}",
                self.text()
            );
            std::thread::sleep(Duration::from_millis(1));
        }
        self.write_keys(keys);
    }

    fn write_keys(&mut self, keys: &[u8]) {
        self.input.write_all(keys).unwrap();
        self.input.flush().unwrap();
    }

    fn finish(&mut self, success: bool) -> portable_pty::ExitStatus {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                // EOF follows the final repaint; drain it before inspecting diagnostics.
                while Instant::now() < deadline {
                    match self.output.recv_timeout(Duration::from_secs(1)) {
                        Ok(chunk) => self.transcript.extend(chunk),
                        Err(_) => break,
                    }
                }
                assert_eq!(status.success(), success, "{}", self.text());
                return status;
            }
            assert!(
                Instant::now() < deadline,
                "child did not exit\n{}",
                self.text()
            );
            if let Ok(chunk) = self.output.recv_timeout(Duration::from_millis(20)) {
                self.transcript.extend(chunk);
            }
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // A failed expectation must neither leave a blocked child nor hang the suite.
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

fn git(dir: &TempDir, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .current_dir(dir.path())
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn candidates() -> TempDir {
    let dir = init_repo();
    configure(&dir);
    git(&dir, &["branch", "alpha"]);
    git(&dir, &["branch", "beta"]);
    // The displayed alpha label contains a path, unlike its internal branch:alpha value.
    git(&dir, &["worktree", "add", "linked-alpha", "alpha"]);
    dir
}

fn clean(dir: &TempDir) -> Session {
    Session::start(
        dir,
        &[
            "--local-only",
            "--no-fetch",
            "--no-pull",
            "--no-worktrunk",
            "--no-color",
        ],
    )
}

#[test]
fn accepting_defaults_deletes_values_rather_than_display_labels() {
    let dir = candidates();
    let mut session = clean(&dir);
    session.expect("branch + worktree");
    session.send(b"\r");
    session.finish(true);
    assert_eq!(git_branches(&dir), ["main"]);
    assert!(!dir.path().join("linked-alpha").exists());
    assert!(!session.text().contains("\x1b[36m"));
}

#[test]
fn completed_fetch_and_pull_replace_their_pending_blocks_without_blank_lines() {
    let dir = candidates();
    git(
        &dir,
        &["remote", "add", "origin", dir.path().to_str().unwrap()],
    );
    git(&dir, &["fetch", "origin"]);
    git(&dir, &["branch", "--set-upstream-to=origin/main", "main"]);
    let mut session = Session::start(&dir, &["--local-only", "--no-worktrunk", "--no-color"]);
    session.expect("Select branches to delete");
    session.expect("Esc");
    let screen = session.screen();
    assert!(
        screen.starts_with(concat!(
            "✔ Fetched and pruned 1 remote(s):\n",
            "  ✔ origin updated\n",
            "✔ Pulled 1 target branch(es):\n",
            "  ✔ main updated\n",
            "Found 2 merged local branch(es):\n",
        )),
        "{screen}"
    );
    assert_eq!(screen.matches("origin updated").count(), 1, "{screen}");
    assert_eq!(screen.matches("main updated").count(), 1, "{screen}");
    assert!(!screen.contains("Remotes updated"), "{screen}");
    assert!(!screen.contains("Fetching"), "{screen}");
    assert!(!screen.contains("Pulling"), "{screen}");
    session.send(b"\x03");
    session.finish(false);
}

#[test]
fn a_failed_remote_stays_visible_alongside_successful_results_and_diagnostics() {
    let dir = candidates();
    git(
        &dir,
        &["remote", "add", "origin", dir.path().to_str().unwrap()],
    );
    git(
        &dir,
        &[
            "remote",
            "add",
            "broken",
            dir.path().join("missing.git").to_str().unwrap(),
        ],
    );
    let mut session = Session::start(
        &dir,
        &["--local-only", "--no-pull", "--no-worktrunk", "--no-color"],
    );
    session.expect("Select branches to delete");
    session.expect("Esc");
    let screen = session.screen();
    assert!(
        screen.starts_with(concat!(
            "⚠ Fetch and prune finished for 2 remote(s), 1 failed:\n",
            "  ✘ broken failed\n",
            "  ✔ origin updated\n",
        )),
        "{screen}"
    );
    assert!(screen.contains("Failed to fetch from"), "{screen}");
    assert!(
        screen.contains("Continuing without 1 remote(s)"),
        "{screen}"
    );
    session.send(b"\x03");
    session.finish(false);
}

#[test]
fn branch_paths_are_dimmed_and_status_stays_on_the_same_row() {
    let dir = candidates();
    let mut session = Session::start(
        &dir,
        &["--local-only", "--no-fetch", "--no-pull", "--no-worktrunk"],
    );
    session.expect("branch + worktree");
    let transcript = session.text();
    let plain = console::strip_ansi_codes(&transcript);
    assert!(
        plain
            .lines()
            .any(|line| line.starts_with("Select branches to delete ›")),
        "{transcript:?}"
    );
    assert!(
        plain.lines().any(
            |line| line.contains("alpha (") && line.contains("linked-alpha) branch + worktree")
        ),
        "{transcript:?}"
    );
    assert!(transcript.contains("\x1b[2m ("), "{transcript:?}");
    let status_prefix = transcript
        .split("linked-alpha)")
        .nth(1)
        .unwrap()
        .split("branch + worktree")
        .next()
        .unwrap();
    assert!(status_prefix.contains("\x1b[36m"), "{transcript:?}");
    assert!(transcript.contains("\x1b[33m>\x1b[0m"), "{transcript:?}");
    assert!(transcript.contains("\x1b[32m✔\x1b[0m"), "{transcript:?}");
    assert!(
        transcript.contains("\x1b[32malpha\x1b[0m"),
        "{transcript:?}"
    );
    session.expect("Esc");
    session.send(b" ");
    session.expect("⬚ alpha");
    session.expect("Esc");
    let updated = session.text();
    let row = updated
        .lines()
        .rev()
        .find(|line| line.contains("⬚ alpha"))
        .unwrap();
    assert!(row.contains("\x1b[2m ("), "{row:?}");
    assert!(row.contains("\x1b[36m"), "{row:?}");
    assert!(!row.contains("\x1b[32m"), "{row:?}");
    // Search highlights still belong to the branch, with metadata styled separately.
    session.send(b"alpha");
    session.expect("\x1b[33malpha");
    session.send(b"\x03");
    session.finish(false);
    assert_eq!(git_branches(&dir), ["alpha", "beta", "main"]);
}

#[test]
fn typing_a_branch_query_removes_nonmatching_rows_from_the_frame() {
    let dir = candidates();
    let mut session = Session::start(
        &dir,
        &["--local-only", "--no-fetch", "--no-pull", "--no-worktrunk"],
    );
    session.expect("Esc");
    session.send(b"alpha");
    session.expect("\x1b[33malpha\x1b[0m");
    session.expect("Esc");
    let transcript = session.text();
    let plain = console::strip_ansi_codes(&transcript);
    let frame = plain.rsplit("Select branches to delete").next().unwrap();
    assert!(frame.contains("alpha"), "{frame}");
    assert!(!frame.contains("beta"), "{frame}");
    session.send(b"\x03");
    session.finish(false);
}

#[test]
fn typeahead_searches_branch_names_without_matching_display_paths() {
    let dir = init_repo();
    configure(&dir);
    for (branch, path) in [
        ("feat/yesno-filter", "git-tpl/feat-yesno-filter"),
        ("refactor/dialoger", "git-tpl/refactor-dialoger"),
    ] {
        git(&dir, &["branch", branch]);
        git(&dir, &["worktree", "add", path, branch]);
    }
    let mut session = Session::start(
        &dir,
        &["--local-only", "--no-fetch", "--no-pull", "--no-worktrunk"],
    );
    session.expect("Esc");
    session.send(b"filter");
    // Every intermediate repaint has the same footer; the full highlighted query
    // synchronizes with the final frame rather than an earlier partial search.
    session.expect("\x1b[33mfilter\x1b[0m");
    session.expect("Esc");
    let transcript = session.text();
    let plain = console::strip_ansi_codes(&transcript);
    let frame = plain.rsplit("Select branches to delete").next().unwrap();
    assert!(frame.contains("feat/yesno-filter"), "{frame}");
    assert!(!frame.contains("refactor/dialoger"), "{frame}");
    assert!(!frame.contains("refactor-dialoger"), "{frame}");
    session.send(b"\x1b");
    session.expect("refactor/dialoger");
    session.expect("Esc");
    session.send(b"git-tpl");
    session.expect("No matching options");
    session.expect("Esc");
    let screen = session.screen();
    assert!(!screen.contains("feat/yesno-filter"), "{screen}");
    assert!(!screen.contains("refactor/dialoger"), "{screen}");
    session.send(b"\x03");
    session.finish(false);
}

#[test]
fn typeahead_matches_case_insensitive_substrings_but_not_scattered_letters() {
    let dir = init_repo();
    configure(&dir);
    git(&dir, &["branch", "feature/FILTER"]);
    git(&dir, &["branch", "f-i-l-t-e-r"]);
    let mut session = Session::start(
        &dir,
        &["--local-only", "--no-fetch", "--no-pull", "--no-worktrunk"],
    );
    session.expect("Esc");
    session.send(b"filter");
    session.expect("\x1b[33mFILTER\x1b[0m");
    session.expect("Esc");
    let transcript = session.text();
    let plain = console::strip_ansi_codes(&transcript);
    let frame = plain.rsplit("Select branches to delete").next().unwrap();
    assert!(frame.contains("feature/FILTER"), "{frame}");
    assert!(!frame.contains("f-i-l-t-e-r"), "{frame}");
    session.send(b"\x03");
    session.finish(false);
}

#[test]
fn filtered_inversion_keeps_hidden_items_and_allows_no_selection() {
    for (keys, expected) in [
        // Deselect everything, search alpha, then invert globally and uncheck alpha.
        (b"\x01alpha\x01 \r".as_slice(), vec!["alpha", "main"]),
        // Filtering alone must not discard the preselected hidden beta branch.
        (b"alpha\r".as_slice(), vec!["main"]),
        (b"\x01\r".as_slice(), vec!["alpha", "beta", "main"]),
    ] {
        let dir = candidates();
        let mut session = clean(&dir);
        session.expect("branch + worktree");
        session.send(keys);
        session.finish(true);
        assert_eq!(git_branches(&dir), expected, "{}", session.text());
    }
}

#[test]
fn clearing_search_then_cancelling_does_not_delete_anything() {
    for cancel in [b"\x1b".as_slice(), b"\x03".as_slice()] {
        let dir = candidates();
        let mut session = clean(&dir);
        session.expect("branch + worktree");
        session.send(b"not-a-branch");
        session.expect("No matching options");
        session.send(b"\x1b");
        session.expect("branch + worktree");
        session.send(cancel);
        session.finish(false);
        assert!(session.text().contains("Cancelled."), "{}", session.text());
        assert_eq!(git_branches(&dir), ["alpha", "beta", "main"]);
        assert!(dir.path().join("linked-alpha").exists());
    }
}

#[test]
fn setup_preserves_editable_defaults_and_accepts_empty_patterns() {
    for forge_default in ["false", "true"] {
        let dir = init_repo();
        configure(&dir);
        git(&dir, &["config", "wipe.forge", forge_default]);
        git(&dir, &["config", "--add", "wipe.protected", "release/*"]);
        git(&dir, &["config", "wipe.ignore", "wip/*"]);
        git(
            &dir,
            &[
                "remote",
                "add",
                "origin",
                "https://github.com/example/example.git",
            ],
        );
        let mut session = Session::start(&dir, &["config", "setup", "--no-color"]);
        session.expect("Which branches should be protected from deletion?");
        session.send(b"\r");
        session.expect("Additional patterns to protect");
        // Accept the editable initial value unchanged.
        session.send(b"\r");
        // Wait for the editable initial value, not just the prompt that precedes it.
        session.expect("entirely (comma-separated, e.g. wip/*) › wip/*");
        session.send(b"\x7f\x7f\x7f\x7f\x7f\r");
        session.expect("Which remotes should merged branches be deleted from?");
        session.send(b"\r");
        // Worktrunk availability is machine-dependent; answer its default if offered.
        if session.expect_one_of(&["Worktrunk detected:", "GitHub detected:"]) == 0 {
            session.send(b"\r");
            session.expect("GitHub detected:");
        }
        session.send(b"\r");
        session.finish(true);
        let transcript = session.text();
        let plain = console::strip_ansi_codes(&transcript);
        assert!(!plain.lines().any(|line| line.starts_with("? ")), "{plain}");
        assert_eq!(
            git(&dir, &["config", "--get-all", "wipe.protected"]),
            "main\nrelease/*\n"
        );
        let config = git(&dir, &["config", "--get-regexp", "^wipe\\."]);
        assert!(!config.contains("wipe.ignore"), "{config}");
        assert!(
            config.contains(&format!("wipe.forge {forge_default}")),
            "{config}"
        );
    }
}

#[test]
fn setup_confirmation_buttons_support_arrow_navigation() {
    let dir = init_repo();
    configure(&dir);
    git(&dir, &["config", "wipe.forge", "false"]);
    git(
        &dir,
        &[
            "remote",
            "add",
            "origin",
            "https://github.com/example/example.git",
        ],
    );
    let mut session = Session::start(&dir, &["config", "setup", "--no-color"]);
    session.expect("Which branches should be protected from deletion?");
    session.send(b"\r");
    session.expect("Additional patterns to protect");
    session.send(b"\r");
    session.expect("Branch patterns to ignore entirely");
    session.send(b"\r");
    session.expect("Which remotes should merged branches be deleted from?");
    session.send(b"\r");
    if session.expect_one_of(&["Worktrunk detected:", "GitHub detected:"]) == 0 {
        session.send(b"\r");
        session.expect("GitHub detected:");
    }
    session.expect("No");
    session.send(b"\x1b[D\r");
    session.finish(true);
    assert_eq!(git(&dir, &["config", "--get", "wipe.forge"]), "true\n");
}

#[test]
fn completed_progress_retains_every_result_when_the_group_exceeds_terminal_height() {
    let dir = init_repo();
    configure(&dir);
    for index in 0..9 {
        git(
            &dir,
            &[
                "remote",
                "add",
                &format!("remote-{index}"),
                dir.path().to_str().unwrap(),
            ],
        );
    }
    let mut session = Session::start_with_rows(
        &dir,
        &[
            "--yes",
            "--no-pull",
            "--local-only",
            "--no-worktrunk",
            "--no-color",
        ],
        6,
    );
    session.finish(true);
    // Ordinary final lines must survive in scrollback even when live rows were clipped.
    let mut expected = String::from("✔ Fetched and pruned 9 remote(s):\n");
    for index in 0..9 {
        expected.push_str(&format!("  ✔ remote-{index} updated\n"));
    }
    assert!(
        session.text().replace("\r\n", "\n").contains(&expected),
        "{}",
        session.text()
    );
}

#[test]
fn terminal_verbose_and_json_modes_do_not_animate_fetch_or_pull() {
    let dir = init_repo();
    configure(&dir);
    git(
        &dir,
        &["remote", "add", "origin", dir.path().to_str().unwrap()],
    );
    git(&dir, &["fetch", "origin"]);
    git(&dir, &["branch", "--set-upstream-to=origin/main", "main"]);
    for mode in ["--verbose", "--json"] {
        let mut session = Session::start(
            &dir,
            &[
                "--yes",
                "--local-only",
                "--no-worktrunk",
                "--no-color",
                mode,
            ],
        );
        session.finish(true);
        let transcript = session.text();
        assert!(!transcript.contains("Fetching and pruning"), "{transcript}");
        assert!(!transcript.contains("Pulling"), "{transcript}");
        if mode == "--json" {
            let document: serde_json::Value = serde_json::from_str(&transcript).unwrap();
            assert_eq!(document["fetch"]["remotes"][0]["status"], "updated");
            assert_eq!(document["pull"]["branches"][0]["status"], "updated");
        } else {
            assert_eq!(
                transcript
                    .matches("Fetched and pruned 1 remote(s):")
                    .count(),
                1,
                "{transcript}"
            );
            assert_eq!(
                transcript.matches("Pulled 1 target branch(es):").count(),
                1,
                "{transcript}"
            );
            assert!(transcript.contains("$ git "), "{transcript}");
        }
    }
}

#[test]
fn cancelling_text_input_does_not_save_partial_setup() {
    let dir = init_repo();
    let mut session = Session::start(&dir, &["config", "setup", "--no-color"]);
    session.expect("Which branches should be protected from deletion?");
    session.send(b"\r");
    session.expect("Additional patterns to protect");
    session.send(b"\x03");
    let status = session.finish(false);
    // Dialoguer prints before entering raw mode: an immediate Ctrl+C can be
    // SIGINT, while a key read surfaces Interrupted through the cancellation path.
    assert!(
        status.signal().is_some() || session.text().contains("Cancelled."),
        "{}",
        session.text()
    );
    let output = std::process::Command::new("git")
        .current_dir(dir.path())
        .args(["config", "--get-regexp", "^wipe\\."])
        .output()
        .unwrap();
    assert!(output.stdout.is_empty());
}

#[test]
fn interrupting_a_spinner_uses_normal_process_signal_handling() {
    let dir = init_repo();
    configure(&dir);
    git(
        &dir,
        &["remote", "add", "slow", dir.path().to_str().unwrap()],
    );
    // A local upload-pack delay holds fetch open without contacting a real remote.
    git(
        &dir,
        &["config", "remote.slow.uploadpack", "sh -c 'sleep 30'"],
    );
    let mut session = Session::start(
        &dir,
        &["--yes", "--no-pull", "--no-worktrunk", "--no-color"],
    );
    session.expect("Fetching and pruning 1 remote(s):");
    session.expect("  - slow");
    let screen = session.screen();
    assert!(
        screen.contains("Fetching and pruning 1 remote(s):\n  - slow"),
        "{screen}"
    );
    // Progress deliberately uses normal signal handling rather than a raw-key reader.
    session.write_keys(b"\x03");
    let status = session.finish(false);
    assert!(status.signal().is_some(), "{status}: {}", session.text());
    assert!(!session.text().contains("Cancelled."));
    assert_eq!(git_branches(&dir), ["main"]);
}
