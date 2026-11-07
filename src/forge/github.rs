//! GitHub (github.com and GitHub Enterprise Server), over GraphQL.

use std::collections::{HashMap, HashSet};

use serde_json::{Value, json};

use super::http;
use super::{
    FinishedHeads, Forge, ForgeError, RemoteUrl, RequestHead, RequestState, token_from_env,
};

/// Branches asked about per request. Each becomes two aliased `pullRequests`
/// fields, so this bounds the query size, not the number of requests per branch.
const BATCH: usize = 40;

/// Pull requests inspected per state and branch name. Separate budgets keep
/// closed requests from crowding previously detectable merges out of the query.
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

    fn finished_heads(&self, branches: &[&str]) -> Result<FinishedHeads, ForgeError> {
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

/// Two aliased `pullRequests` selections per branch, the names passed as
/// variables rather than spliced into the query.
fn build_query(owner: &str, name: &str, branches: &[&str]) -> (String, Value) {
    let mut declarations = String::from("$owner: String!, $name: String!");
    let mut selections = String::new();
    let mut variables = json!({ "owner": owner, "name": name });

    for (i, branch) in branches.iter().enumerate() {
        declarations.push_str(&format!(", $b{i}: String!"));
        for (alias, state) in [("m", "MERGED"), ("c", "CLOSED")] {
            // Give merged requests their original search budget despite abandoned requests.
            selections.push_str(&format!(
                " {alias}{i}: pullRequests(headRefName: $b{i}, states: {state}, first: {PER_BRANCH}) \
                 {{ nodes {{ headRefOid state }} }}"
            ));
        }
        variables[format!("b{i}")] = json!(branch);
    }

    let query = format!(
        "query({declarations}) {{ repository(owner: $owner, name: $name) {{{selections} }} }}"
    );
    (query, variables)
}

/// Read both state-specific aliases of a [`build_query`] response back into branches.
fn parse_response(data: &Value, branches: &[&str]) -> Result<FinishedHeads, ForgeError> {
    let repository = data
        .get("repository")
        .filter(|r| !r.is_null())
        .ok_or_else(|| ForgeError::other("repository not found, or not visible to the token"))?;

    let mut found = HashMap::new();
    for (i, branch) in branches.iter().enumerate() {
        let heads: HashSet<RequestHead> = [format!("m{i}"), format!("c{i}")]
            .iter()
            .filter_map(|alias| repository.get(alias))
            .filter_map(|r| r.get("nodes").and_then(Value::as_array))
            .flatten()
            .filter_map(|node| {
                let state = match node.get("state")?.as_str()? {
                    "MERGED" => RequestState::Merged,
                    "CLOSED" => RequestState::Closed,
                    _ => return None,
                };
                Some(RequestHead {
                    sha: node.get("headRefOid")?.as_str()?.to_string(),
                    state,
                })
            })
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
        assert!(query.contains("m1: pullRequests(headRefName: $b1, states: MERGED, first: 20)"));
        assert!(query.contains("c1: pullRequests(headRefName: $b1, states: CLOSED, first: 20)"));
        assert!(!query.contains("evil"));
        assert_eq!(variables["owner"], "o");
        assert_eq!(variables["name"], "r");
        assert_eq!(variables["b0"], "feature/x");
        assert_eq!(variables["b1"], "evil\") { }");
    }

    #[test]
    fn response_maps_aliases_back_to_branches() {
        let data = json!({"repository": {
            "m0": {"nodes": [{"headRefOid": "aaa", "state": "MERGED"}]},
            "c0": {"nodes": [{"headRefOid": "bbb", "state": "CLOSED"}, {"headRefOid": "ccc", "state": "OPEN"}]},
            "m1": {"nodes": []},
            "c1": {"nodes": []},
        }});
        let found = parse_response(&data, &["x", "y"]).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(
            found["x"],
            HashSet::from([
                RequestHead {
                    sha: "aaa".into(),
                    state: RequestState::Merged
                },
                RequestHead {
                    sha: "bbb".into(),
                    state: RequestState::Closed
                },
            ])
        );
    }

    #[test]
    fn a_missing_repository_is_an_error() {
        let data = json!({"repository": null});
        assert!(parse_response(&data, &["x"]).is_err());
    }
}
