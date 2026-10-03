//! Build script generating the git-wipe man page and shell completions.
//!
//! Both are derived from the same clap definition as the binary, so they can
//! never drift from `--help`.
//!
//! Artifacts land in `$OUT_DIR`:
//!
//! - `man/git-wipe.1`
//! - `completions/` for bash, zsh, fish, elvish and PowerShell
//!
//! `mise run man` collects them into `dist/`.

use std::path::PathBuf;
use std::{env, fs, io};

use clap::CommandFactory;
use clap_complete::Shell;

#[path = "src/cli.rs"]
#[allow(dead_code)]
mod cli;

fn main() -> io::Result<()> {
    println!("cargo:rerun-if-changed=src/cli.rs");
    println!("cargo:rerun-if-changed=build.rs");

    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR is always set by cargo"));

    let man_dir = out_dir.join("man");
    fs::create_dir_all(&man_dir)?;
    clap_mangen::generate_to(cli::Cli::command(), &man_dir)?;

    let completions_dir = out_dir.join("completions");
    fs::create_dir_all(&completions_dir)?;
    let mut cmd = cli::Cli::command();
    for shell in [
        Shell::Bash,
        Shell::Zsh,
        Shell::Fish,
        Shell::Elvish,
        Shell::PowerShell,
    ] {
        clap_complete::generate_to(shell, &mut cmd, "git-wipe", &completions_dir)?;
    }

    Ok(())
}
