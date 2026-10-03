//! Remote URL parsing: just enough to tell which forge project a remote is.

/// The parts of a git remote URL a forge client needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteUrl {
    /// The scheme to reach the forge API with: `http` only when the remote
    /// itself says so, `https` otherwise (ssh and scp-like remotes included).
    pub scheme: &'static str,
    /// The bare host name, lowercased.
    pub host: String,
    /// `host` plus the port, for http(s) remotes only: an ssh port says
    /// nothing about where the web API listens.
    pub authority: String,
    /// The project path without surrounding slashes or a `.git` suffix:
    /// `owner/repo`, or `group/subgroup/repo` on GitLab.
    pub path: String,
}

impl RemoteUrl {
    /// Parse the URL of a remote.
    ///
    /// Understands `https://`, `http://`, `ssh://`, `git://` and the scp-like
    /// `user@host:path` form. Local paths, `file://` URLs and anything without
    /// an `owner/repo` path yield `None`.
    pub fn parse(url: &str) -> Option<Self> {
        let url = url.trim();

        let (scheme, authority, path) = if let Some((scheme, rest)) = url.split_once("://") {
            let (authority, path) = rest.split_once('/')?;
            (scheme.to_ascii_lowercase(), authority, path)
        } else {
            // scp-like: `[user@]host:path`. A `/` before the colon means a
            // local path, and a one-letter host is a Windows drive.
            let (authority, path) = url.split_once(':')?;
            if authority.contains('/') || authority.len() < 2 {
                return None;
            }
            ("ssh".to_string(), authority, path)
        };

        // Drop userinfo, and split off the port.
        let authority = authority.rsplit_once('@').map_or(authority, |(_, a)| a);
        let (host, port) = match authority.rsplit_once(':') {
            // A bracketed IPv6 literal ends in `]`, not in a port.
            Some((host, port)) if !port.contains(']') => (host, Some(port)),
            _ => (authority, None),
        };
        let host = host.trim_matches(['[', ']']).to_ascii_lowercase();
        if host.is_empty() {
            return None;
        }

        let web = match scheme.as_str() {
            "http" => Some("http"),
            "https" => Some("https"),
            "ssh" | "git" | "git+ssh" | "ssh+git" => None,
            _ => return None,
        };

        let path = path
            .split(['?', '#'])
            .next()
            .unwrap_or_default()
            .trim_matches('/');
        let path = path.strip_suffix(".git").unwrap_or(path).trim_matches('/');
        if path.split('/').filter(|s| !s.is_empty()).count() < 2 {
            return None;
        }

        let authority = match (web, port) {
            (Some(_), Some(port)) => format!("{host}:{port}"),
            _ => host.clone(),
        };

        Some(Self {
            scheme: web.unwrap_or("https"),
            host,
            authority,
            path: path.to_string(),
        })
    }

    /// The `(owner, name)` pair of an `owner/repo` path. Everything but the
    /// last segment is the owner, so nested GitLab groups stay intact.
    pub fn owner_and_name(&self) -> (&str, &str) {
        self.path.rsplit_once('/').unwrap_or(("", &self.path))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(url: &str) -> (String, String, String, String) {
        let u = RemoteUrl::parse(url).unwrap_or_else(|| panic!("{url} should parse"));
        (u.scheme.to_string(), u.host, u.authority, u.path)
    }

    fn t(a: &str, b: &str, c: &str, d: &str) -> (String, String, String, String) {
        (a.into(), b.into(), c.into(), d.into())
    }

    #[test]
    fn parses_https_urls() {
        assert_eq!(
            parsed("https://github.com/noirbizarre/git-wipe.git"),
            t("https", "github.com", "github.com", "noirbizarre/git-wipe")
        );
        assert_eq!(
            parsed("https://user:secret@Gitea.Example.com:3000/org/repo/"),
            t(
                "https",
                "gitea.example.com",
                "gitea.example.com:3000",
                "org/repo"
            )
        );
        assert_eq!(
            parsed("http://git.local/o/r"),
            t("http", "git.local", "git.local", "o/r")
        );
    }

    #[test]
    fn parses_ssh_and_scp_urls_onto_the_https_api() {
        assert_eq!(
            parsed("git@github.com:noirbizarre/git-wipe.git"),
            t("https", "github.com", "github.com", "noirbizarre/git-wipe")
        );
        assert_eq!(
            parsed("ssh://git@gitlab.com:2222/group/sub/repo.git"),
            t("https", "gitlab.com", "gitlab.com", "group/sub/repo")
        );
        assert_eq!(
            parsed("git://example.com/o/r.git"),
            t("https", "example.com", "example.com", "o/r")
        );
    }

    #[test]
    fn rejects_what_is_not_a_forge_project() {
        for url in [
            "",
            "/srv/git/repo.git",
            "../repo",
            "C:\\repos\\thing",
            "C:/repos/thing",
            "file:///srv/git/repo.git",
            "https://github.com/onlyowner",
            "https://github.com/",
            "ftp://example.com/o/r",
            "git@github.com:repo",
        ] {
            assert_eq!(RemoteUrl::parse(url), None, "{url:?}");
        }
    }

    #[test]
    fn splits_owner_and_name_keeping_nested_groups() {
        let u = RemoteUrl::parse("https://gitlab.com/a/b/c.git").unwrap();
        assert_eq!(u.owner_and_name(), ("a/b", "c"));
        let u = RemoteUrl::parse("https://github.com/o/r").unwrap();
        assert_eq!(u.owner_and_name(), ("o", "r"));
    }
}
