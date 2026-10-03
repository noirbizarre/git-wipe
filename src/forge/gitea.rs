//! Gitea and Forgejo (Codeberg included), over their REST API.
//!
//! Neither offers GraphQL, nor a pull-request filter by head branch, so the
//! closed pull requests are listed newest-activity first, page by page, until
//! every branch asked about has been seen or the page budget is spent.

use std::collections::{HashMap, HashSet};

use serde_json::Value;

use super::http;
use super::{Forge, ForgeError, ForgeKind, RemoteUrl, token_from_env};

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
    fn merged_heads(
        &self,
        branches: &[&str],
    ) -> Result<HashMap<String, HashSet<String>>, ForgeError> {
        let authorization =
            token_from_env(&["GITEA_TOKEN", "FORGEJO_TOKEN"]).map(|token| format!("token {token}"));
        let wanted: HashSet<&str> = branches.iter().copied().collect();
        let mut found: HashMap<String, HashSet<String>> = HashMap::new();

        for page in 1..=MAX_PAGES {
            let body =
                http::get_json(&self.agent, &self.pulls_url(page), authorization.as_deref())?;
            let pulls = body
                .as_array()
                .ok_or_else(|| ForgeError::other("the pull request list was not an array"))?;

            collect_merged(pulls, &wanted, &mut found);

            let seen_all = wanted.iter().all(|b| found.contains_key(*b));
            if pulls.len() < PAGE_SIZE || seen_all {
                break;
            }
        }
        Ok(found)
    }
}

/// Keep the merged pull requests whose head branch is in `wanted`.
fn collect_merged(
    pulls: &[Value],
    wanted: &HashSet<&str>,
    found: &mut HashMap<String, HashSet<String>>,
) {
    for pull in pulls {
        if pull.get("merged").and_then(Value::as_bool) != Some(true) {
            continue;
        }
        let head = pull.get("head");
        let branch = head.and_then(|h| h.get("ref")).and_then(Value::as_str);
        let sha = head.and_then(|h| h.get("sha")).and_then(Value::as_str);
        if let (Some(branch), Some(sha)) = (branch, sha)
            && wanted.contains(branch)
        {
            found
                .entry(branch.to_string())
                .or_default()
                .insert(sha.to_string());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn keeps_only_merged_pulls_of_wanted_branches() {
        let pulls = vec![
            json!({"merged": true, "head": {"ref": "a", "sha": "111"}}),
            json!({"merged": false, "head": {"ref": "a", "sha": "222"}}),
            json!({"merged": true, "head": {"ref": "other", "sha": "333"}}),
            json!({"merged": true, "head": {"ref": "b"}}),
            json!({"head": {"ref": "b", "sha": "444"}}),
        ];
        let wanted = HashSet::from(["a", "b"]);
        let mut found = HashMap::new();
        collect_merged(&pulls, &wanted, &mut found);
        assert_eq!(found.len(), 1);
        assert_eq!(found["a"], HashSet::from(["111".to_string()]));
    }
}
