//! Forge-backed merge detection.
//!
//! Git history alone cannot say whether a branch was merged through a forge:
//! squash and rebase merges rewrite it. The forge, however, knows. When the
//! user opts in (`--forge` / `wipe.forge`), merge detection asks it first and
//! keeps the offline git strategies as the fallback.
//!
//! Three layers, from the outside in:
//!
//! - [`ForgeSource`] is what merge detection depends on: given the branches of
//!   a remote and their tips, which have a merged pull/merge request? It knows
//!   nothing of providers or transports, so detection is tested with fakes.
//! - [`RemoteForges`] implements it for a real repository: one provider per
//!   remote, picked from the remote URL, with the answers cached so a branch is
//!   never asked about twice in a run.
//! - [`Forge`] is the provider abstraction: [`github`] and [`gitlab`] speak
//!   GraphQL, [`gitea`] (Gitea and Forgejo) speaks REST. All share one HTTP
//!   client; none shells out to a forge CLI.
//!
//! Nothing here is ever fatal. A forge that cannot be reached, or does not
//! recognise the project, surfaces as a [`ForgeOutcome`] the caller turns into
//! a warning before carrying on with git alone.

mod gitea;
mod github;
mod gitlab;
mod http;
pub mod setting;
mod url;

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::Mutex;

pub use setting::{ForgeKind, ForgeSetting};
pub use url::RemoteUrl;

use crate::git::Git;

/// Why a forge could not be consulted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForgeErrorKind {
    /// The forge could not be reached: DNS, connection, TLS, timeout.
    Network,
    /// The forge refused the credentials, or there were none.
    Auth,
    /// Anything else: a server error, an unexpected response.
    Other,
}

/// A failed forge call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForgeError {
    pub kind: ForgeErrorKind,
    pub message: String,
}

impl ForgeError {
    pub fn new(kind: ForgeErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    pub fn other(message: impl Into<String>) -> Self {
        Self::new(ForgeErrorKind::Other, message)
    }

    pub fn auth(message: impl Into<String>) -> Self {
        Self::new(ForgeErrorKind::Auth, message)
    }
}

impl fmt::Display for ForgeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let prefix = match self.kind {
            ForgeErrorKind::Network => "network error: ",
            ForgeErrorKind::Auth => "authentication error: ",
            ForgeErrorKind::Other => "",
        };
        write!(f, "{prefix}{}", self.message)
    }
}

impl std::error::Error for ForgeError {}

/// A forge provider: the canonical interface merge detection relies on.
pub trait Forge: Send + Sync {
    /// A short description for messages, e.g. `GitHub noirbizarre/git-wipe`.
    fn describe(&self) -> String;

    /// For each of `branches`, the head commits of its *merged* pull/merge
    /// requests.
    ///
    /// A branch with no merged request is simply absent from the result: that
    /// is "not found", not a failure. Batching is the provider's business.
    fn merged_heads(
        &self,
        branches: &[&str],
    ) -> Result<HashMap<String, HashSet<String>>, ForgeError>;
}

/// The first non-empty value among the environment variables `names`.
fn token_from_env(names: &[&str]) -> Option<String> {
    names
        .iter()
        .filter_map(|name| std::env::var(name).ok())
        .map(|value| value.trim().to_string())
        .find(|value| !value.is_empty())
}

/// Pick the provider for a remote URL.
///
/// Identification is purely textual: it needs no network and no credentials,
/// which is also what repository setup relies on. `Err` carries the reason a
/// remote is not supported.
pub fn connect(setting: ForgeSetting, url: &str) -> Result<Box<dyn Forge>, String> {
    let (kind, project) = identify(setting, url)?;
    let agent = http::agent();
    Ok(match kind {
        ForgeKind::GitHub => Box::new(github::GitHub::new(project, agent)),
        ForgeKind::GitLab => Box::new(gitlab::GitLab::new(project, agent)),
        ForgeKind::Gitea | ForgeKind::Forgejo => Box::new(gitea::Gitea::new(kind, project, agent)),
    })
}

/// Work out which forge, and which project on it, a remote URL names.
pub fn identify(setting: ForgeSetting, url: &str) -> Result<(ForgeKind, RemoteUrl), String> {
    let project = RemoteUrl::parse(url)
        .ok_or_else(|| format!("'{url}' is not an owner/repository URL of a forge"))?;
    let kind = match setting {
        ForgeSetting::Off => return Err("forge detection is off".to_string()),
        ForgeSetting::Kind(kind) => kind,
        ForgeSetting::Auto => ForgeKind::detect_host(&project.host).ok_or_else(|| {
            format!(
                "cannot tell which forge '{}' runs; set wipe.forge to github, gitlab, gitea or forgejo",
                project.host
            )
        })?,
    };
    Ok((kind, project))
}

/// A remote recognised as belonging to a forge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Detection {
    pub remote: String,
    pub kind: ForgeKind,
    pub host: String,
}

/// Recognise the forge of every remote that has one, from its URL alone.
///
/// Pure identification: nothing is contacted, so this works offline and
/// without credentials, and a remote that cannot be read or recognised is just
/// left out.
pub fn detect_remotes(git: &Git, remotes: &[String]) -> Vec<Detection> {
    let urls: Vec<(String, String)> = remotes
        .iter()
        .filter_map(|r| Some((r.clone(), git.remote_url(r).ok()?)))
        .collect();
    detect_urls(&urls)
}

/// [`detect_remotes`] over `(remote, url)` pairs.
pub fn detect_urls(urls: &[(String, String)]) -> Vec<Detection> {
    urls.iter()
        .filter_map(|(remote, url)| {
            let (kind, project) = identify(ForgeSetting::Auto, url).ok()?;
            Some(Detection {
                remote: remote.clone(),
                kind,
                host: project.host,
            })
        })
        .collect()
}

/// What a forge said about the branches of one remote.
#[derive(Debug, Clone)]
pub enum ForgeOutcome {
    /// The forge answered. The set holds the branches whose merged pull/merge
    /// request ended exactly at the branch tip; every other branch is "not
    /// found" and left to the git strategies.
    Resolved(HashSet<String>),
    /// No provider fits this remote. Not worth a warning on its own.
    Unsupported(String),
    /// The forge could not be consulted. Worth a warning, then git alone.
    Unavailable(ForgeError),
}

/// What merge detection asks of the forge.
pub trait ForgeSource: Sync {
    /// The remotes the forge may be asked about.
    fn remotes(&self) -> &[String];

    /// Among `tips` (branch name to tip commit), the branches of `remote`
    /// whose pull/merge request was merged at that very commit.
    ///
    /// Matching the commit, not just the name, is what keeps a branch that was
    /// reused or advanced after its merge from being reported as merged.
    fn pr_merged(&self, remote: &str, tips: &HashMap<String, String>) -> ForgeOutcome;
}

/// A remote's forge, with what it has already told us.
enum Slot {
    Ready {
        forge: Box<dyn Forge>,
        /// Branch to the head commits of its merged requests.
        heads: HashMap<String, HashSet<String>>,
        /// Branches already asked about, found or not.
        asked: HashSet<String>,
    },
    Unsupported(String),
    /// Failed once and reported: later calls answer "nothing", silently.
    Failed,
}

/// The forges behind the remotes of a repository.
pub struct RemoteForges {
    remotes: Vec<String>,
    slots: Mutex<HashMap<String, Slot>>,
}

impl RemoteForges {
    /// Identify the forge of each of `remotes`. Never fails and never touches
    /// the network: a remote that cannot be identified is just unsupported.
    pub fn new(git: &Git, setting: ForgeSetting, remotes: Vec<String>) -> Self {
        let slots = remotes
            .iter()
            .map(|remote| {
                let slot = match git.remote_url(remote) {
                    Err(_) => {
                        Slot::Unsupported(format!("cannot read the URL of remote '{remote}'"))
                    }
                    Ok(url) => match connect(setting, &url) {
                        Ok(forge) => Slot::Ready {
                            forge,
                            heads: HashMap::new(),
                            asked: HashSet::new(),
                        },
                        Err(reason) => Slot::Unsupported(format!("remote '{remote}': {reason}")),
                    },
                };
                (remote.clone(), slot)
            })
            .collect();
        Self {
            remotes,
            slots: Mutex::new(slots),
        }
    }
}

impl ForgeSource for RemoteForges {
    fn remotes(&self) -> &[String] {
        &self.remotes
    }

    fn pr_merged(&self, remote: &str, tips: &HashMap<String, String>) -> ForgeOutcome {
        let mut slots = self.slots.lock().unwrap_or_else(|e| e.into_inner());
        let Some(slot) = slots.get_mut(remote) else {
            return ForgeOutcome::Unsupported(format!("unknown remote '{remote}'"));
        };

        let failure = match slot {
            Slot::Unsupported(reason) => return ForgeOutcome::Unsupported(reason.clone()),
            Slot::Failed => return ForgeOutcome::Resolved(HashSet::new()),
            Slot::Ready {
                forge,
                heads,
                asked,
            } => {
                let mut missing: Vec<&str> = tips
                    .keys()
                    .map(String::as_str)
                    .filter(|b| !asked.contains(*b))
                    .collect();
                missing.sort_unstable();

                let result = if missing.is_empty() {
                    Ok(HashMap::new())
                } else {
                    forge.merged_heads(&missing)
                };
                match result {
                    Ok(found) => {
                        asked.extend(missing.into_iter().map(str::to_string));
                        heads.extend(found);
                        let merged = tips
                            .iter()
                            .filter(|(branch, sha)| {
                                heads.get(*branch).is_some_and(|shas| shas.contains(*sha))
                            })
                            .map(|(branch, _)| branch.clone())
                            .collect();
                        return ForgeOutcome::Resolved(merged);
                    }
                    Err(error) => ForgeError {
                        message: format!("{}: {}", forge.describe(), error.message),
                        ..error
                    },
                }
            }
        };

        *slot = Slot::Failed;
        ForgeOutcome::Unavailable(failure)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A provider answering from a table.
    struct Fake {
        merged: HashMap<String, HashSet<String>>,
        fail: Option<ForgeError>,
    }

    impl Forge for Fake {
        fn describe(&self) -> String {
            "Fake o/r".to_string()
        }

        fn merged_heads(
            &self,
            branches: &[&str],
        ) -> Result<HashMap<String, HashSet<String>>, ForgeError> {
            if let Some(error) = &self.fail {
                return Err(error.clone());
            }
            Ok(branches
                .iter()
                .filter_map(|b| Some((b.to_string(), self.merged.get(*b)?.clone())))
                .collect())
        }
    }

    fn forges_with(fake: Fake) -> RemoteForges {
        let mut slots = HashMap::new();
        slots.insert(
            "origin".to_string(),
            Slot::Ready {
                forge: Box::new(fake),
                heads: HashMap::new(),
                asked: HashSet::new(),
            },
        );
        slots.insert("odd".to_string(), Slot::Unsupported("odd host".to_string()));
        RemoteForges {
            remotes: vec!["origin".to_string(), "odd".to_string()],
            slots: Mutex::new(slots),
        }
    }

    fn tips(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(b, s)| (b.to_string(), s.to_string()))
            .collect()
    }

    fn fake(merged: &[(&str, &str)]) -> Fake {
        Fake {
            merged: merged
                .iter()
                .map(|(b, s)| (b.to_string(), HashSet::from([s.to_string()])))
                .collect(),
            fail: None,
        }
    }

    #[test]
    fn only_a_matching_tip_counts_as_merged() {
        let forges = forges_with(fake(&[("a", "111"), ("b", "222")]));
        let ForgeOutcome::Resolved(found) =
            forges.pr_merged("origin", &tips(&[("a", "111"), ("b", "999"), ("c", "333")]))
        else {
            panic!("expected a resolution");
        };
        assert_eq!(found, HashSet::from(["a".to_string()]));
    }

    #[test]
    fn a_repeat_question_makes_no_second_call() {
        let counter = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));

        struct Counting(std::sync::Arc<std::sync::atomic::AtomicUsize>);
        impl Forge for Counting {
            fn describe(&self) -> String {
                "Counting".into()
            }
            fn merged_heads(
                &self,
                _: &[&str],
            ) -> Result<HashMap<String, HashSet<String>>, ForgeError> {
                self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(HashMap::new())
            }
        }

        let mut slots = HashMap::new();
        slots.insert(
            "origin".to_string(),
            Slot::Ready {
                forge: Box::new(Counting(counter.clone())),
                heads: HashMap::new(),
                asked: HashSet::new(),
            },
        );
        let forges = RemoteForges {
            remotes: vec!["origin".to_string()],
            slots: Mutex::new(slots),
        };
        let t = tips(&[("a", "1"), ("b", "2")]);
        forges.pr_merged("origin", &t);
        forges.pr_merged("origin", &t);
        assert_eq!(counter.load(std::sync::atomic::Ordering::SeqCst), 1);
        forges.pr_merged("origin", &tips(&[("a", "1"), ("c", "3")]));
        assert_eq!(counter.load(std::sync::atomic::Ordering::SeqCst), 2);
    }

    #[test]
    fn a_failure_is_reported_once_then_silenced() {
        let forges = forges_with(Fake {
            fail: Some(ForgeError::new(ForgeErrorKind::Network, "timed out")),
            ..fake(&[])
        });
        let t = tips(&[("a", "1")]);
        let ForgeOutcome::Unavailable(error) = forges.pr_merged("origin", &t) else {
            panic!("expected a failure");
        };
        assert_eq!(error.kind, ForgeErrorKind::Network);
        assert_eq!(error.message, "Fake o/r: timed out");
        assert!(matches!(
            forges.pr_merged("origin", &t),
            ForgeOutcome::Resolved(s) if s.is_empty()
        ));
    }

    #[test]
    fn unsupported_and_unknown_remotes_are_reported_as_such() {
        let forges = forges_with(fake(&[]));
        assert!(matches!(
            forges.pr_merged("odd", &tips(&[("a", "1")])),
            ForgeOutcome::Unsupported(reason) if reason == "odd host"
        ));
        assert!(matches!(
            forges.pr_merged("nope", &tips(&[("a", "1")])),
            ForgeOutcome::Unsupported(_)
        ));
    }

    #[test]
    fn detection_skips_remotes_without_a_known_forge() {
        let urls = vec![
            ("origin".to_string(), "git@github.com:o/r.git".to_string()),
            (
                "mirror".to_string(),
                "https://git.example.com/o/r".to_string(),
            ),
            ("local".to_string(), "/srv/git/r.git".to_string()),
            ("gl".to_string(), "https://gitlab.com/g/s/r".to_string()),
        ];
        assert_eq!(
            detect_urls(&urls),
            vec![
                Detection {
                    remote: "origin".into(),
                    kind: ForgeKind::GitHub,
                    host: "github.com".into()
                },
                Detection {
                    remote: "gl".into(),
                    kind: ForgeKind::GitLab,
                    host: "gitlab.com".into()
                },
            ]
        );
        assert!(detect_urls(&[]).is_empty());
    }

    #[test]
    fn identify_uses_the_setting_over_the_host() {
        let url = "git@git.example.com:team/app.git";
        assert!(identify(ForgeSetting::Auto, url).is_err());
        let (kind, project) = identify(ForgeSetting::Kind(ForgeKind::Gitea), url).unwrap();
        assert_eq!(kind, ForgeKind::Gitea);
        assert_eq!(project.path, "team/app");
        assert_eq!(
            identify(ForgeSetting::Auto, "https://github.com/o/r")
                .unwrap()
                .0,
            ForgeKind::GitHub
        );
        assert!(identify(ForgeSetting::Auto, "/srv/git/r.git").is_err());
        assert!(identify(ForgeSetting::Off, "https://github.com/o/r").is_err());
    }
}
