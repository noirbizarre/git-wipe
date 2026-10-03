//! The error type, and the diagnostics it renders to.
//!
//! `thiserror` defines them, `miette` renders them. A diagnostic must carry the
//! two things the user does not already know: what specifically failed, and
//! what to do about it.

use miette::Diagnostic;
use thiserror::Error;

/// The crate's result type.
pub type Result<T> = std::result::Result<T, Error>;

/// Everything that can go wrong.
///
/// Diagnostic codes are `git_wipe::<module>::<kind>`. A code is a public
/// identifier users grep for, so renaming one is a breaking change.
#[derive(Debug, Error, Diagnostic)]
#[non_exhaustive]
pub enum Error {
    /// Reading or writing a file failed.
    #[error("failed to access `{path}`")]
    #[diagnostic(code(git_wipe::error::io))]
    Io {
        /// The path that could not be accessed.
        path: String,
        /// Why.
        #[source]
        source: std::io::Error,
    },
}
