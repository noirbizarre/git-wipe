//! Wipe out merged local branches and worktrees
//!
//! This library retains the template's error and entry-point scaffolding.
//! The cleanup engine currently belongs to the binary's modules; it is not
//! exposed through this library. Integration tests exercise the binary.

// miette's `Diagnostic` payloads carry source text and spans, which puts most
// error variants past clippy's 128-byte `Result` threshold. The lint is right
// about the cost and wrong about the trade: a large error that says what to do
// beats a small one that does not.
#![allow(clippy::result_large_err)]
#![warn(missing_docs)]

pub mod error;

pub use error::{Error, Result};

/// Placeholder entry point retained from the template; currently always succeeds.
pub fn run() -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn running_with_nothing_to_do_succeeds() {
        assert!(run().is_ok());
    }
}
