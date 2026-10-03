//! Argument types only.
//!
//! No behaviour lives here: parsing is one concern and doing the work is
//! another, and keeping them apart is what lets the library be used without
//! the CLI.

use clap::{Parser, Subcommand};

/// Wipe out merged local branches and worktrees
#[derive(Debug, Parser)]
#[command(name = "git-wipe", version, about, long_about = None)]
pub struct Cli {
    /// Increase verbosity. Repeat for more.
    #[arg(short, long, global = true, action = clap::ArgAction::Count)]
    pub verbose: u8,

    /// The subcommand to run.
    #[command(subcommand)]
    pub command: Command,
}

/// The subcommands.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Do the thing.
    Run,
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    // Catches the derive mistakes that would otherwise only surface as a panic
    // the first time a user runs the binary.
    #[test]
    fn the_command_definition_is_valid() {
        Cli::command().debug_assert();
    }
}
