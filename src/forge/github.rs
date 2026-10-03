//! GitHub (github.com and GitHub Enterprise Server), over GraphQL.

use std::collections::{HashMap, HashSet};

use serde_json::{Value, json};

use super::http;
use super::{Forge, ForgeError, RemoteUrl, token_from_env};

/// Branches asked about per request. Each becomes an aliased `pullRequests`
/// field, so this bounds the query size, not the number of requests per branch.
const BATCH: usize = 40;

/// Merged pull requests inspected per branch name. A name reused by many
/// merged pull requests (`patch-1`) is only searched this deep.
const PER_BRANCH: usize = 20;

pub struct GitHub {
    project: RemoteUrl,
    agent: ureq::Agent,
}

impl GitHub {
    pub fn new(project: RemoteUrl, agent: ureq::Agent) -> Self {
        Self { project, agent }
    }

    fn is_dotcom(&self) -> bool {
        matches!(self.project.host.as_str(), "github.com" | "www.github.com")
    }

    /// The GraphQL endpoint: `api.github.com` for the public service,
    /// `<host>/api/graphql` for Enterprise Server.
    fn endpoint(&self) -> String {
        if self.is_dotcom() {
            format!(
                "{}/graphql",
                http::api_base("https://api.github.com".to_string())
            )
        } else {
            let base = format!("{}://{}", self.project.scheme, self.project.authority);
            format!("{}/api/graphql", http::api_base(base))
        }
    }

    /// GitHub's GraphQL API has no anonymous access. The public service and
    /// each Enterprise host have their own variables, so a github.com token is
    /// never offered to another host.
    fn token(&self) -> Option<String> {
        if self.is_dotcom() {
            token_from_env(&["GITHUB_TOKEN", "GH_TOKEN"])
        } else {
            token_from_env(&["GH_ENTERPRISE_TOKEN", "GITHUB_ENTERPRISE_TOKEN"])
        }
    }
}

impl Forge for GitHub {
    fn describe(&self) -> String {
        format!("GitHub {}/{}", self.project.host, self.project.path)
    }

    fn merged_heads(
        &self,
        branches: &[&str],
    ) -> Result<HashMap<String, HashSet<String>>, ForgeError> {
        let Some(token) = self.token() else {
            let names = if self.is_dotcom() {
                "GITHUB_TOKEN or GH_TOKEN"
            } else {
                "GH_ENTERPRISE_TOKEN"
            };
            return Err(ForgeError::auth(format!(
                "GitHub's GraphQL API requires a token; set {names}"
            )));
        };

        let (owner, name) = self.project.owner_and_name();
        let mut found = HashMap::new();
        for chunk in branches.chunks(BATCH) {
            let (query, variables) = build_query(owner, name, chunk);
            let data = http::graphql(
                &self.agent,
                &self.endpoint(),
                Some(&token),
                &query,
                variables,
            )?;
            found.extend(parse_response(&data, chunk)?);
        }
        Ok(found)
    }
}

/// One aliased `pullRequests` selection per branch, the names passed as
/// variables rather than spliced into the query.
fn build_query(owner: &str, name: &str, branches: &[&str]) -> (String, Value) {
    let mut declarations = String::from("$owner: String!, $name: String!");
    let mut selections = String::new();
    let mut variables = json!({ "owner": owner, "name": name });

    for (i, branch) in branches.iter().enumerate() {
        declarations.push_str(&format!(", $b{i}: String!"));
        selections.push_str(&format!(
            " r{i}: pullRequests(headRefName: $b{i}, states: MERGED, first: {PER_BRANCH}) \
             {{ nodes {{ headRefOid }} }}"
        ));
        variables[format!("b{i}")] = json!(branch);
    }

    let query = format!(
        "query({declarations}) {{ repository(owner: $owner, name: $name) {{{selections} }} }}"
    );
    (query, variables)
}

/// Read the `r<i>` aliases of a [`build_query`] response back into branches.
fn parse_response(
    data: &Value,
    branches: &[&str],
) -> Result<HashMap<String, HashSet<String>>, ForgeError> {
    let repository = data
        .get("repository")
        .filter(|r| !r.is_null())
        .ok_or_else(|| ForgeError::other("repository not found, or not visible to the token"))?;

    let mut found = HashMap::new();
    for (i, branch) in branches.iter().enumerate() {
        let heads: HashSet<String> = repository
            .get(format!("r{i}"))
            .and_then(|r| r.get("nodes"))
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|node| node.get("headRefOid")?.as_str().map(str::to_string))
            .collect();
        if !heads.is_empty() {
            found.insert((*branch).to_string(), heads);
        }
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_passes_branch_names_as_variables() {
        let (query, variables) = build_query("o", "r", &["feature/x", "evil\") { }"]);
        assert!(query.contains("$b0: String!, $b1: String!"));
        assert!(query.contains("r1: pullRequests(headRefName: $b1, states: MERGED"));
        assert!(!query.contains("evil"));
        assert_eq!(variables["owner"], "o");
        assert_eq!(variables["name"], "r");
        assert_eq!(variables["b0"], "feature/x");
        assert_eq!(variables["b1"], "evil\") { }");
    }

    #[test]
    fn response_maps_aliases_back_to_branches() {
        let data = json!({"repository": {
            "r0": {"nodes": [{"headRefOid": "aaa"}, {"headRefOid": "bbb"}]},
            "r1": {"nodes": []},
        }});
        let found = parse_response(&data, &["x", "y"]).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(
            found["x"],
            HashSet::from(["aaa".to_string(), "bbb".to_string()])
        );
    }

    #[test]
    fn a_missing_repository_is_an_error() {
        let data = json!({"repository": null});
        assert!(parse_response(&data, &["x"]).is_err());
    }
}
