//! The forge opt-in accepted by `--forge` and `wipe.forge`.
//!
//! Kept free of any other crate module: `build.rs` compiles it alongside
//! `cli.rs` to derive the man pages and completions.

use std::fmt;
use std::str::FromStr;

use anyhow::{Result, anyhow};

/// A forge flavour git-wipe can talk to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ForgeKind {
    /// github.com and GitHub Enterprise Server, over GraphQL.
    GitHub,
    /// gitlab.com and self-managed GitLab, over GraphQL.
    GitLab,
    /// Gitea, over its REST API.
    Gitea,
    /// Forgejo (and Codeberg), over its REST API.
    Forgejo,
}

impl ForgeKind {
    /// The lowercase name accepted by `--forge=<kind>` and `wipe.forge`.
    pub fn name(self) -> &'static str {
        match self {
            Self::GitHub => "github",
            Self::GitLab => "gitlab",
            Self::Gitea => "gitea",
            Self::Forgejo => "forgejo",
        }
    }

    /// The name as its vendor spells it, for messages.
    pub fn label(self) -> &'static str {
        match self {
            Self::GitHub => "GitHub",
            Self::GitLab => "GitLab",
            Self::Gitea => "Gitea",
            Self::Forgejo => "Forgejo",
        }
    }

    /// Guess the forge behind a remote host name, without any network access.
    ///
    /// Only well-known hosts and the usual self-hosting naming conventions
    /// (`gitlab.example.com`, `gitea.example.com`...) are recognised; a host
    /// like `git.example.com` needs an explicit `wipe.forge = <kind>`.
    pub fn detect_host(host: &str) -> Option<Self> {
        let host = host.to_ascii_lowercase();
        let labels: Vec<&str> = host.split('.').collect();
        let has = |needle: &str| labels.iter().any(|l| l.contains(needle));

        if host == "github.com" || host == "www.github.com" {
            Some(Self::GitHub)
        } else if host == "codeberg.org" || has("forgejo") {
            Some(Self::Forgejo)
        } else if has("gitlab") {
            Some(Self::GitLab)
        } else if has("gitea") {
            Some(Self::Gitea)
        } else if has("github") {
            Some(Self::GitHub)
        } else {
            None
        }
    }
}

impl FromStr for ForgeKind {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "github" => Ok(Self::GitHub),
            "gitlab" => Ok(Self::GitLab),
            "gitea" => Ok(Self::Gitea),
            "forgejo" | "codeberg" => Ok(Self::Forgejo),
            _ => Err(anyhow!(
                "invalid forge {s:?}, expected true, false, github, gitlab, gitea or forgejo"
            )),
        }
    }
}

impl fmt::Display for ForgeKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// Whether, and how, the forge is consulted to detect merged branches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ForgeSetting {
    /// Never contact a forge. The default.
    #[default]
    Off,
    /// Identify the forge from each remote URL.
    Auto,
    /// Treat every remote as this kind of forge, for hosts that cannot be
    /// recognised by name (self-hosted instances on arbitrary domains).
    Kind(ForgeKind),
}

impl ForgeSetting {
    /// Whether the forge is consulted at all.
    pub fn is_enabled(self) -> bool {
        self != Self::Off
    }
}

impl FromStr for ForgeSetting {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "false" | "off" | "no" | "0" => Ok(Self::Off),
            "true" | "on" | "yes" | "1" | "auto" => Ok(Self::Auto),
            _ => s.parse::<ForgeKind>().map(Self::Kind),
        }
    }
}

impl fmt::Display for ForgeSetting {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Off => f.write_str("false"),
            Self::Auto => f.write_str("true"),
            Self::Kind(kind) => f.write_str(kind.name()),
        }
    }
}

/// Serialized as it is written in `wipe.forge`.
impl serde::Serialize for ForgeSetting {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setting_parses_booleans_and_kinds() {
        assert_eq!("true".parse::<ForgeSetting>().unwrap(), ForgeSetting::Auto);
        assert_eq!("Auto".parse::<ForgeSetting>().unwrap(), ForgeSetting::Auto);
        assert_eq!("false".parse::<ForgeSetting>().unwrap(), ForgeSetting::Off);
        assert_eq!(
            " GitLab ".parse::<ForgeSetting>().unwrap(),
            ForgeSetting::Kind(ForgeKind::GitLab)
        );
        assert_eq!(
            "codeberg".parse::<ForgeSetting>().unwrap(),
            ForgeSetting::Kind(ForgeKind::Forgejo)
        );
        assert!("bitbucket".parse::<ForgeSetting>().is_err());
        assert_eq!(ForgeSetting::default(), ForgeSetting::Off);
    }

    #[test]
    fn setting_round_trips_through_display() {
        for setting in [
            ForgeSetting::Off,
            ForgeSetting::Auto,
            ForgeSetting::Kind(ForgeKind::GitHub),
            ForgeSetting::Kind(ForgeKind::Gitea),
        ] {
            assert_eq!(
                setting.to_string().parse::<ForgeSetting>().unwrap(),
                setting
            );
        }
        assert!(ForgeSetting::Auto.is_enabled());
        assert!(!ForgeSetting::Off.is_enabled());
    }

    #[test]
    fn host_detection_recognises_well_known_and_conventional_hosts() {
        let detect = ForgeKind::detect_host;
        assert_eq!(detect("github.com"), Some(ForgeKind::GitHub));
        assert_eq!(detect("GitHub.com"), Some(ForgeKind::GitHub));
        assert_eq!(detect("github.example.com"), Some(ForgeKind::GitHub));
        assert_eq!(detect("gitlab.com"), Some(ForgeKind::GitLab));
        assert_eq!(detect("gitlab.corp.example"), Some(ForgeKind::GitLab));
        assert_eq!(detect("codeberg.org"), Some(ForgeKind::Forgejo));
        assert_eq!(detect("forgejo.example.com"), Some(ForgeKind::Forgejo));
        assert_eq!(detect("gitea.example.com"), Some(ForgeKind::Gitea));
        assert_eq!(detect("git.example.com"), None);
        assert_eq!(detect("bitbucket.org"), None);
    }
}
