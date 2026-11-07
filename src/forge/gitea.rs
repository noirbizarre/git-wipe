//! Gitea and Forgejo (Codeberg included), over their REST API.
//!
//! Neither offers GraphQL, nor a pull-request filter by head branch, so the
//! closed pull requests are listed newest-activity first, page by page, until
//! the list ends or the page budget is spent. Reused branch names may have
//! several heads, so seeing a name once is not enough to stop searching.

use std::collections::{HashMap, HashSet};

use serde_json::Value;

use super::http;
use super::{
    FinishedHeads, Forge, ForgeError, ForgeKind, RemoteUrl, RequestHead, RequestState,
    token_from_env,
};

const PAGE_SIZE: usize = 50;

/// Pages read at most: the 500 most recently updated closed pull requests.
/// Branches left unresolved after that fall back to git.
const MAX_PAGES: usize = 10;

pub struct Gitea {
    kind: ForgeKind,
    project: RemoteUrl,
    agent: ureq::Agent,
}

impl Gitea {
    pub fn new(kind: ForgeKind, project: RemoteUrl, agent: ureq::Agent) -> Self {
        Self {
            kind,
            project,
            agent,
        }
    }

    fn pulls_url(&self, page: usize) -> String {
        let base = format!("{}://{}", self.project.scheme, self.project.authority);
        format!(
            "{}/api/v1/repos/{}/pulls?state=closed&sort=recentupdate&limit={PAGE_SIZE}&page={page}",
            http::api_base(base),
            self.project.path,
        )
    }
}

impl Forge for Gitea {
    fn describe(&self) -> String {
        format!(
            "{} {}/{}",
            self.kind.label(),
            self.project.host,
            self.project.path
        )
    }

    /// Public repositories answer anonymously; a token widens that to private
    /// ones.
    fn finished_heads(&self, branches: &[&str]) -> Result<FinishedHeads, ForgeError> {
        let authorization =
            token_from_env(&["GITEA_TOKEN", "FORGEJO_TOKEN"]).map(|token| format!("token {token}"));
        let wanted: HashSet<&str> = branches.iter().copied().collect();
        let mut found = HashMap::new();

        for page in 1..=MAX_PAGES {
            let body =
                http::get_json(&self.agent, &self.pulls_url(page), authorization.as_deref())?;
            let pulls = body
                .as_array()
                .ok_or_else(|| ForgeError::other("the pull request list was not an array"))?;

            collect_finished(pulls, &wanted, &mut found);

            // Older pages may contain another head or stronger merged evidence for a branch.
            if pulls.len() < PAGE_SIZE {
                break;
            }
        }
        Ok(found)
    }
}

/// Keep finished pull requests whose head branch is in `wanted`.
fn collect_finished(pulls: &[Value], wanted: &HashSet<&str>, found: &mut FinishedHeads) {
    for pull in pulls {
        let state = match (
            pull.get("merged").and_then(Value::as_bool),
            pull.get("state").and_then(Value::as_str),
        ) {
            (Some(true), Some("closed")) => RequestState::Merged,
            (Some(false), Some("closed")) => RequestState::Closed,
            _ => continue,
        };
        let head = pull.get("head");
        let branch = head.and_then(|h| h.get("ref")).and_then(Value::as_str);
        let sha = head.and_then(|h| h.get("sha")).and_then(Value::as_str);
        if let (Some(branch), Some(sha)) = (branch, sha)
            && wanted.contains(branch)
        {
            found
                .entry(branch.to_string())
                .or_default()
                .insert(RequestHead {
                    sha: sha.to_string(),
                    state,
                });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn keeps_only_finished_pulls_of_wanted_branches() {
        let pulls = vec![
            json!({"merged": true, "state": "closed", "head": {"ref": "a", "sha": "111"}}),
            json!({"merged": false, "state": "closed", "head": {"ref": "a", "sha": "222"}}),
            json!({"merged": false, "state": "open", "head": {"ref": "a", "sha": "555"}}),
            json!({"merged": true, "state": "closed", "head": {"ref": "other", "sha": "333"}}),
            json!({"merged": true, "state": "closed", "head": {"ref": "b"}}),
            json!({"head": {"ref": "b", "sha": "444"}}),
        ];
        let wanted = HashSet::from(["a", "b"]);
        let mut found = HashMap::new();
        collect_finished(&pulls, &wanted, &mut found);
        assert_eq!(found.len(), 1);
        assert_eq!(
            found["a"],
            HashSet::from([
                RequestHead {
                    sha: "111".into(),
                    state: RequestState::Merged
                },
                RequestHead {
                    sha: "222".into(),
                    state: RequestState::Closed
                },
            ])
        );
    }
}
