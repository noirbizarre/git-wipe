//! Shared fixtures for the integration tests.
//!
//! Integration tests are compiled as a separate crate and therefore cannot
//! reach `crate::test_helpers`; this module mirrors it for the binary-level
//! tests. Helpers panic on failure rather than returning `Result`, so a broken
//! fixture surfaces as a test failure at the exact setup step.
//!
//! Each test crate uses a different subset, hence the blanket `dead_code`.

#![allow(dead_code)]

use std::process::Command as StdCommand;
use tempfile::TempDir;

/// A pid far beyond any real OS's pid_max (Linux tops out at 4194304 even at
/// its highest configurable ceiling; macOS/BSD/Windows are far lower), so it
/// is guaranteed dead without spawning and reaping a process. Mirrors
/// `crate::test_helpers::DEAD_PID`.
pub const DEAD_PID: u32 = 4_000_000_000;

/// Initialize a minimal git repo with a single commit on `main`.
pub fn init_repo() -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();

    StdCommand::new("git")
        .args(["init", "--initial-branch=main"])
        .current_dir(p)
        .output()
        .unwrap();
    StdCommand::new("git")
        .args(["config", "user.email", "test@test.com"])
        .current_dir(p)
        .output()
        .unwrap();
    StdCommand::new("git")
        .args(["config", "user.name", "Test"])
        .current_dir(p)
        .output()
        .unwrap();

    std::fs::write(p.join("README.md"), "# test").unwrap();
    StdCommand::new("git")
        .args(["add", "."])
        .current_dir(p)
        .output()
        .unwrap();
    StdCommand::new("git")
        .args(["commit", "-m", "init"])
        .current_dir(p)
        .output()
        .unwrap();

    dir
}

/// Seed the `[wipe]` config section so the clean workflow
/// doesn't trigger the interactive setup wizard.
pub fn configure(dir: &TempDir) {
    let p = dir.path();
    StdCommand::new("git")
        .args(["config", "--add", "wipe.protected", "main"])
        .current_dir(p)
        .output()
        .unwrap();
}

/// Add a merged branch (`feature/done`) and an unmerged branch (`feature/wip`).
pub fn add_branches(dir: &TempDir) {
    let p = dir.path();

    // Create and merge feature/done
    StdCommand::new("git")
        .args(["checkout", "-b", "feature/done"])
        .current_dir(p)
        .output()
        .unwrap();
    std::fs::write(p.join("done.txt"), "done").unwrap();
    StdCommand::new("git")
        .args(["add", "."])
        .current_dir(p)
        .output()
        .unwrap();
    StdCommand::new("git")
        .args(["commit", "-m", "done"])
        .current_dir(p)
        .output()
        .unwrap();
    StdCommand::new("git")
        .args(["checkout", "main"])
        .current_dir(p)
        .output()
        .unwrap();
    StdCommand::new("git")
        .args(["merge", "feature/done"])
        .current_dir(p)
        .output()
        .unwrap();

    // Create unmerged feature/wip
    StdCommand::new("git")
        .args(["checkout", "-b", "feature/wip"])
        .current_dir(p)
        .output()
        .unwrap();
    std::fs::write(p.join("wip.txt"), "wip").unwrap();
    StdCommand::new("git")
        .args(["add", "."])
        .current_dir(p)
        .output()
        .unwrap();
    StdCommand::new("git")
        .args(["commit", "-m", "wip"])
        .current_dir(p)
        .output()
        .unwrap();
    StdCommand::new("git")
        .args(["checkout", "main"])
        .current_dir(p)
        .output()
        .unwrap();
}

/// Add a linked worktree on a branch already merged into `main`, so the
/// worktree is a removal candidate. Returns its path.
///
/// The worktree is necessarily brand new, which is exactly what the
/// `--min-age` guard is meant to protect.
pub fn add_merged_worktree(dir: &TempDir, branch: &str, name: &str) -> std::path::PathBuf {
    let p = dir.path();

    for args in [
        vec!["checkout", "-b", branch],
        vec!["commit", "--allow-empty", "-m", "worktree work"],
        vec!["checkout", "main"],
        vec!["merge", branch, "--no-edit"],
    ] {
        StdCommand::new("git")
            .args(&args)
            .current_dir(p)
            .output()
            .unwrap();
    }

    let wt_path = p.join(name);
    StdCommand::new("git")
        .args(["worktree", "add", wt_path.to_str().unwrap(), branch])
        .current_dir(p)
        .output()
        .unwrap();
    assert!(
        wt_path.exists(),
        "fixture worktree should have been created"
    );

    wt_path
}

/// Return the list of local branch names in the repo.
pub fn git_branches(dir: &TempDir) -> Vec<String> {
    let output = StdCommand::new("git")
        .args(["branch", "--format=%(refname:short)"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| l.to_string())
        .collect()
}

/// Initialize a repo with `extensions.worktreeConfig = true` and a linked
/// worktree. Returns (tempdir, main_path, worktree_path).
pub fn init_repo_with_worktree_config() -> (TempDir, std::path::PathBuf, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let main_path = dir.path().join("main-repo");
    std::fs::create_dir_all(&main_path).unwrap();

    StdCommand::new("git")
        .args(["init", "--initial-branch=main"])
        .current_dir(&main_path)
        .output()
        .unwrap();
    StdCommand::new("git")
        .args(["config", "user.email", "test@test.com"])
        .current_dir(&main_path)
        .output()
        .unwrap();
    StdCommand::new("git")
        .args(["config", "user.name", "Test"])
        .current_dir(&main_path)
        .output()
        .unwrap();

    std::fs::write(main_path.join("README.md"), "# test").unwrap();
    StdCommand::new("git")
        .args(["add", "."])
        .current_dir(&main_path)
        .output()
        .unwrap();
    StdCommand::new("git")
        .args(["commit", "-m", "init"])
        .current_dir(&main_path)
        .output()
        .unwrap();

    // Enable extensions.worktreeConfig
    StdCommand::new("git")
        .args(["config", "extensions.worktreeConfig", "true"])
        .current_dir(&main_path)
        .output()
        .unwrap();

    // Create a branch and a linked worktree
    StdCommand::new("git")
        .args(["branch", "feature/wt"])
        .current_dir(&main_path)
        .output()
        .unwrap();
    let wt_path = dir.path().join("linked-wt");
    StdCommand::new("git")
        .args(["worktree", "add", wt_path.to_str().unwrap(), "feature/wt"])
        .current_dir(&main_path)
        .output()
        .unwrap();

    (dir, main_path, wt_path)
}

// ── Fake forge ───────────────────────────────────────────────────────

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

/// One request received by a [`FakeForge`].
#[derive(Debug, Clone)]
pub struct Recorded {
    pub method: String,
    pub path: String,
    pub authorization: Option<String>,
    pub body: String,
}

/// A tiny HTTP server standing in for a forge API, so the real binary can be
/// exercised without internet access. Serves from a background thread that
/// ends with the test process.
pub struct FakeForge {
    /// `http://127.0.0.1:<port>`, to hand to `GIT_WIPE_FORGE_API_URL`.
    pub url: String,
    requests: Arc<Mutex<Vec<Recorded>>>,
}

impl FakeForge {
    /// Start a server answering every request with `handler(request)`:
    /// a status code and a JSON body.
    pub fn start(handler: impl Fn(&Recorded) -> (u16, String) + Send + Sync + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let requests: Arc<Mutex<Vec<Recorded>>> = Arc::default();
        let handler = Arc::new(handler);

        let log = requests.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                let (log, handler) = (log.clone(), handler.clone());
                std::thread::spawn(move || serve_one(stream, &log, &*handler));
            }
        });

        Self { url, requests }
    }

    /// An address nothing listens on: connections are refused.
    pub fn dead_url() -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        format!("http://{}", listener.local_addr().unwrap())
    }

    pub fn requests(&self) -> Vec<Recorded> {
        self.requests.lock().unwrap().clone()
    }
}

fn serve_one(
    stream: std::net::TcpStream,
    log: &Mutex<Vec<Recorded>>,
    handler: &(dyn Fn(&Recorded) -> (u16, String) + Send + Sync),
) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).unwrap_or(0) == 0 {
        return;
    }
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_string();
    let path = parts.next().unwrap_or_default().to_string();

    let mut length = 0usize;
    let mut authorization = None;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 || line.trim().is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            match name.trim().to_ascii_lowercase().as_str() {
                "content-length" => length = value.trim().parse().unwrap_or(0),
                "authorization" => authorization = Some(value.trim().to_string()),
                _ => {}
            }
        }
    }
    let mut body = vec![0u8; length];
    reader.read_exact(&mut body).unwrap();

    let recorded = Recorded {
        method,
        path,
        authorization,
        body: String::from_utf8_lossy(&body).into_owned(),
    };
    log.lock().unwrap().push(recorded.clone());

    let (status, payload) = handler(&recorded);
    let mut stream = stream;
    let _ = write!(
        stream,
        "HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{payload}",
        payload.len()
    );
    let _ = stream.flush();
}
