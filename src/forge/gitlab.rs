//! GitLab (gitlab.com and self-managed), over GraphQL.

use std::collections::{HashMap, HashSet};

use serde_json::{Value, json};

use super::http;
use super::{Forge, ForgeError, RemoteUrl, token_from_env};

/// Source branches per request.
const BATCH: usize = 50;

/// Pages followed per batch before giving up on the remainder. A batch of
/// `BATCH` branches rarely has more than a page of merge requests.
const MAX_PAGES: usize = 10;

const QUERY: &str = "query($path: ID!, $branches: [String!], $after: String) { \
    project(fullPath: $path) { \
        mergeRequests(state: merged, sourceBranches: $branches, first: 100, after: $after) { \
            nodes { sourceBranch diffHeadSha } \
            pageInfo { hasNextPage endCursor } \
        } \
    } \
}";

pub struct GitLab {
    project: RemoteUrl,
    agent: ureq::Agent,
}

impl GitLab {
    pub fn new(project: RemoteUrl, agent: ureq::Agent) -> Self {
        Self { project, agent }
    }

    fn endpoint(&self) -> String {
        let base = format!("{}://{}", self.project.scheme, self.project.authority);
        format!("{}/api/graphql", http::api_base(base))
    }
}

impl Forge for GitLab {
    fn describe(&self) -> String {
        format!("GitLab {}/{}", self.project.host, self.project.path)
    }

    /// Public projects answer anonymously; a token widens that to private ones.
    fn merged_heads(
        &self,
        branches: &[&str],
    ) -> Result<HashMap<String, HashSet<String>>, ForgeError> {
        let token = token_from_env(&["GITLAB_TOKEN", "GL_TOKEN"]);
        let mut found: HashMap<String, HashSet<String>> = HashMap::new();

        for chunk in branches.chunks(BATCH) {
            let mut after = Value::Null;
            for _ in 0..MAX_PAGES {
                let variables = json!({
                    "path": self.project.path,
                    "branches": chunk,
                    "after": after,
                });
                let data = http::graphql(
                    &self.agent,
                    &self.endpoint(),
                    token.as_deref(),
                    QUERY,
                    variables,
                )?;
                let next = parse_page(&data, &mut found)?;
                match next {
                    Some(cursor) => after = Value::String(cursor),
                    None => break,
                }
            }
        }
        Ok(found)
    }
}

/// Fold one page of merge requests into `found`; return the cursor of the next
/// page, if there is one.
fn parse_page(
    data: &Value,
    found: &mut HashMap<String, HashSet<String>>,
) -> Result<Option<String>, ForgeError> {
    let connection = data
        .get("project")
        .filter(|p| !p.is_null())
        .ok_or_else(|| ForgeError::other("project not found, or not visible to the token"))?
        .get("mergeRequests")
        .ok_or_else(|| ForgeError::other("the response carried no merge requests"))?;

    for node in connection
        .get("nodes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let branch = node.get("sourceBranch").and_then(Value::as_str);
        let head = node.get("diffHeadSha").and_then(Value::as_str);
        if let (Some(branch), Some(head)) = (branch, head) {
            found
                .entry(branch.to_string())
                .or_default()
                .insert(head.to_string());
        }
    }

    let page = connection.get("pageInfo");
    let has_next = page
        .and_then(|p| p.get("hasNextPage"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    Ok(page
        .and_then(|p| p.get("endCursor"))
        .and_then(Value::as_str)
        .filter(|_| has_next)
        .map(str::to_string))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_collects_heads_and_the_next_cursor() {
        let data = json!({"project": {"mergeRequests": {
            "nodes": [
                {"sourceBranch": "a", "diffHeadSha": "111"},
                {"sourceBranch": "a", "diffHeadSha": "222"},
                {"sourceBranch": "b", "diffHeadSha": null},
            ],
            "pageInfo": {"hasNextPage": true, "endCursor": "abc"},
        }}});
        let mut found = HashMap::new();
        assert_eq!(
            parse_page(&data, &mut found).unwrap(),
            Some("abc".to_string())
        );
        assert_eq!(
            found["a"],
            HashSet::from(["111".to_string(), "222".to_string()])
        );
        assert!(!found.contains_key("b"));
    }

    #[test]
    fn last_page_has_no_cursor() {
        let data = json!({"project": {"mergeRequests": {
            "nodes": [],
            "pageInfo": {"hasNextPage": false, "endCursor": "abc"},
        }}});
        assert_eq!(parse_page(&data, &mut HashMap::new()).unwrap(), None);
    }

    #[test]
    fn a_missing_project_is_an_error() {
        let data = json!({"project": null});
        assert!(parse_page(&data, &mut HashMap::new()).is_err());
    }
}
