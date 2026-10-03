//! The one HTTP client every forge provider shares, and the two request
//! shapes they need: a GraphQL POST and a REST GET.
//!
//! Every failure is folded into a [`ForgeError`] whose kind tells the caller
//! (and the user) whether to blame the network, the credentials or the forge.

use std::time::Duration;

use serde_json::{Value, json};

use super::{ForgeError, ForgeErrorKind};

/// Test hook: replace the scheme, host and port of every API call.
///
/// Providers still append their own path (`/graphql`, `/api/graphql`,
/// `/api/v1/...`), so a fake server can stand in for any forge. It is
/// deliberately undocumented outside the code: it exists so the integration
/// tests can run the real binary without internet access.
pub const API_BASE_ENV: &str = "GIT_WIPE_FORGE_API_URL";

/// The base URL replacing `default`, when [`API_BASE_ENV`] is set.
pub fn api_base(default: String) -> String {
    match std::env::var(API_BASE_ENV) {
        Ok(base) if !base.trim().is_empty() => base.trim().trim_end_matches('/').to_string(),
        _ => default,
    }
}

/// How long a connection may take to establish.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// How long a whole request may take. A forge outage must never hang a clean-up.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);

/// Build the HTTP agent: bounded in time, and reporting HTTP error statuses as
/// plain responses so they can be classified here instead of as transport
/// failures.
pub fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_connect(Some(CONNECT_TIMEOUT))
        .timeout_global(Some(REQUEST_TIMEOUT))
        .http_status_as_error(false)
        .user_agent(concat!("git-wipe/", env!("CARGO_PKG_VERSION")))
        .build()
        .into()
}

/// Send a GraphQL `query` and return its `data` object.
pub fn graphql(
    agent: &ureq::Agent,
    url: &str,
    token: Option<&str>,
    query: &str,
    variables: Value,
) -> Result<Value, ForgeError> {
    let mut request = agent.post(url).header("Accept", "application/json");
    if let Some(token) = token {
        request = request.header("Authorization", &format!("Bearer {token}"));
    }
    let response = request
        .send_json(json!({ "query": query, "variables": variables }))
        .map_err(transport_error)?;

    let mut body = read_body(response, url)?;
    if let Some(errors) = body.get("errors").and_then(Value::as_array)
        && !errors.is_empty()
    {
        return Err(graphql_error(errors));
    }
    match body.get_mut("data").map(Value::take) {
        Some(data) if !data.is_null() => Ok(data),
        _ => Err(ForgeError::other("the response carried no data")),
    }
}

/// GET `url` and return the JSON body.
pub fn get_json(
    agent: &ureq::Agent,
    url: &str,
    authorization: Option<&str>,
) -> Result<Value, ForgeError> {
    let mut request = agent.get(url).header("Accept", "application/json");
    if let Some(authorization) = authorization {
        request = request.header("Authorization", authorization);
    }
    let response = request.call().map_err(transport_error)?;
    read_body(response, url)
}

/// Check the status line, then parse the body as JSON.
fn read_body(
    mut response: ureq::http::Response<ureq::Body>,
    url: &str,
) -> Result<Value, ForgeError> {
    let status = response.status().as_u16();
    if !(200..300).contains(&status) {
        let detail = response.body_mut().read_to_string().unwrap_or_default();
        return Err(status_error(status, url, &detail));
    }
    response
        .body_mut()
        .read_json::<Value>()
        .map_err(|e| ForgeError::other(format!("unreadable response from {url}: {e}")))
}

/// Classify a non-2xx HTTP status.
pub fn status_error(status: u16, url: &str, detail: &str) -> ForgeError {
    let hint = match status {
        401 => "authentication failed (is the token valid?)",
        403 => "access denied (missing token scope, or rate limited)",
        404 => "not found (wrong project, or the token cannot see it)",
        429 => "rate limited",
        s if s >= 500 => "the forge is unavailable",
        _ => "unexpected response",
    };
    let kind = match status {
        401 | 403 => ForgeErrorKind::Auth,
        _ => ForgeErrorKind::Other,
    };
    let snippet: String = detail.split_whitespace().collect::<Vec<_>>().join(" ");
    let snippet = snippet.chars().take(120).collect::<String>();
    let message = if snippet.is_empty() {
        format!("HTTP {status} from {url}: {hint}")
    } else {
        format!("HTTP {status} from {url}: {hint} ({snippet})")
    };
    ForgeError::new(kind, message)
}

/// Classify a failure to complete the exchange.
fn transport_error(error: ureq::Error) -> ForgeError {
    use ureq::Error;
    match error {
        Error::Http(_) | Error::BadUri(_) | Error::InvalidProxyUrl | Error::BodyExceedsLimit(_) => {
            ForgeError::other(error.to_string())
        }
        other => ForgeError::new(ForgeErrorKind::Network, other.to_string()),
    }
}

/// Fold a GraphQL `errors` array into one error, classifying auth failures.
fn graphql_error(errors: &[Value]) -> ForgeError {
    let messages: Vec<&str> = errors
        .iter()
        .filter_map(|e| e.get("message").and_then(Value::as_str))
        .collect();
    let message = if messages.is_empty() {
        "the forge returned an unspecified GraphQL error".to_string()
    } else {
        messages.join("; ")
    };
    let lowered = message.to_ascii_lowercase();
    let auth = errors.iter().any(|e| {
        matches!(
            e.get("type").and_then(Value::as_str),
            Some("FORBIDDEN" | "UNAUTHORIZED")
        )
    }) || lowered.contains("bad credentials")
        || lowered.contains("unauthorized")
        || lowered.contains("must be authenticated");
    ForgeError::new(
        if auth {
            ForgeErrorKind::Auth
        } else {
            ForgeErrorKind::Other
        },
        message,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statuses_are_classified() {
        assert_eq!(status_error(401, "u", "").kind, ForgeErrorKind::Auth);
        assert_eq!(status_error(403, "u", "").kind, ForgeErrorKind::Auth);
        assert_eq!(status_error(404, "u", "").kind, ForgeErrorKind::Other);
        assert_eq!(
            status_error(502, "u", "bad\n gateway").kind,
            ForgeErrorKind::Other
        );
        assert!(
            status_error(502, "u", "bad\n gateway")
                .message
                .contains("(bad gateway)")
        );
    }

    #[test]
    fn graphql_errors_are_classified() {
        let auth = graphql_error(&[json!({"message": "Bad credentials"})]);
        assert_eq!(auth.kind, ForgeErrorKind::Auth);
        let typed = graphql_error(&[json!({"type": "FORBIDDEN", "message": "nope"})]);
        assert_eq!(typed.kind, ForgeErrorKind::Auth);
        let other = graphql_error(&[json!({"message": "Field 'x' doesn't exist"})]);
        assert_eq!(other.kind, ForgeErrorKind::Other);
        assert_eq!(other.message, "Field 'x' doesn't exist");
        assert!(!graphql_error(&[]).message.is_empty());
    }
}
