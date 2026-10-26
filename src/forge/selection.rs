//! Select PR/MR destination repositories independently of cleanup remotes.

use std::collections::HashSet;

use anyhow::{Context, Result};

use super::{ForgeSetting, identify};
use crate::branches::{Filter, resolve_merge_targets};
use crate::git::Git;

/// Resolve explicit selectors or infer the destinations of protected branches.
/// No network access is needed: tracking configuration survives missing refs.
pub fn select_repositories(
    git: &Git,
    filter: &Filter,
    setting: ForgeSetting,
    explicit: Option<&[String]>,
) -> Result<Vec<String>> {
    let mut remotes = git.remotes()?;
    remotes.sort();
    let url_for = |selector: &str| -> Result<String> {
        if remotes.iter().any(|remote| remote == selector) {
            git.remote_url(selector)
        } else {
            Ok(selector.to_string())
        }
    };
    // Missing URLs on an inferred remote must not prevent Git-only detection.
    let supported = |remote: &str| {
        git.remote_url(remote)
            .ok()
            .and_then(|url| identify(setting, &url).ok())
            .is_some()
    };
    let mut candidates = Vec::new();
    if let Some(selectors) = explicit {
        for selector in selectors {
            let url = url_for(selector)?;
            identify(setting, &url).map_err(anyhow::Error::msg).with_context(|| {
                format!(
                    "invalid forge repository '{selector}': use an existing remote name or repository URL \
                     in --forge-repo / wipe.forgeRepo; set --forge=<kind> for a self-hosted forge"
                )
            })?;
            candidates.push(selector.clone());
        }
    } else {
        for branch in resolve_merge_targets(git, filter)? {
            if let Some((remote, _)) = git.branch_upstream(&branch)?
                && remote != "."
                && remotes.contains(&remote)
                && supported(&remote)
            {
                candidates.push(remote);
            }
        }
        if candidates.is_empty() {
            // Prefer the parent project over the fork when tracking gives no clue.
            let ordered = ["upstream", "origin"]
                .into_iter()
                .map(str::to_string)
                .chain(remotes.iter().cloned());
            for remote in ordered {
                if remotes.contains(&remote) && supported(&remote) {
                    candidates.push(remote);
                    break;
                }
            }
        }
    }

    let mut seen = HashSet::new();
    let mut selected = Vec::new();
    for selector in candidates {
        let (kind, project) =
            identify(setting, &url_for(&selector)?).map_err(anyhow::Error::msg)?;
        // SSH and HTTPS aliases share answers; distinct web ports do not.
        if seen.insert((kind.to_string(), project.authority, project.path)) {
            selected.push(selector);
        }
    }
    Ok(selected)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::test_helpers::{git_in, init_repo};

    fn remote(git: &Git, name: &str, url: &str) -> Result<()> {
        git.config_set(&format!("remote.{name}.url"), url)
    }

    fn track(git: &Git, branch: &str, remote: &str) -> Result<()> {
        git.config_set(&format!("branch.{branch}.remote"), remote)?;
        git.config_set(
            &format!("branch.{branch}.merge"),
            "refs/heads/different-name",
        )
    }

    fn select(git: &Git, config: &Config, explicit: Option<&[String]>) -> Result<Vec<String>> {
        select_repositories(
            git,
            &Filter::load(git, config)?,
            ForgeSetting::Auto,
            explicit,
        )
    }

    #[test]
    fn protected_destinations_win_over_conventional_and_cleanup_remotes() -> Result<()> {
        let (_dir, git) = init_repo()?;
        remote(&git, "parent", "git@github.com:team/project.git")?;
        remote(&git, "upstream", "https://github.com/other/project")?;
        remote(&git, "origin", "https://github.com/user/project")?;
        track(&git, "main", "parent")?;
        let config = Config {
            remotes: Some(vec!["origin".into()]),
            ..Config::default()
        };
        // No remote-tracking refs exist, and the upstream branch name differs.
        assert_eq!(select(&git, &config, None)?, ["parent"]);
        Ok(())
    }

    #[test]
    fn all_protected_tracking_repositories_are_selected_but_ignored_ones_are_not() -> Result<()> {
        let (dir, git) = init_repo()?;
        for branch in ["release/1", "release/2", "stable", "ignored", "feature"] {
            git_in(dir.path(), &["branch", branch])?;
        }
        remote(&git, "parent", "https://github.com/team/project")?;
        remote(&git, "releases", "https://gitlab.com/group/releases")?;
        remote(&git, "unused", "https://github.com/user/fork")?;
        track(&git, "main", "parent")?;
        track(&git, "release/1", "releases")?;
        track(&git, "release/2", "releases")?;
        track(&git, "stable", "parent")?;
        track(&git, "ignored", "unused")?;
        track(&git, "feature", "unused")?;
        git.set_branch_protected("stable", true)?;
        git.set_branch_protected("ignored", true)?;
        let config = Config {
            protected: vec!["main".into(), "release/*".into(), "missing".into()],
            ignore: vec!["ignored".into()],
            ..Config::default()
        };
        assert_eq!(select(&git, &config, None)?, ["parent", "releases"]);
        Ok(())
    }

    #[test]
    fn absent_or_local_tracking_falls_back_to_upstream_then_origin_then_other_remotes() -> Result<()>
    {
        let (_dir, git) = init_repo()?;
        let config = Config::default();
        assert!(select(&git, &config, None)?.is_empty());
        remote(&git, "z", "https://github.com/team/z")?;
        remote(&git, "a", "https://github.com/team/a")?;
        assert_eq!(select(&git, &config, None)?, ["a"]);
        remote(&git, "origin", "https://github.com/user/fork")?;
        assert_eq!(select(&git, &config, None)?, ["origin"]);
        remote(&git, "upstream", "https://gitlab.com/group/project")?;
        for tracking in [".", "deleted"] {
            track(&git, "main", tracking)?;
            assert_eq!(select(&git, &config, None)?, ["upstream"]);
        }
        remote(&git, "upstream", "https://unknown.example/group/project")?;
        assert_eq!(select(&git, &config, None)?, ["origin"]);
        Ok(())
    }

    #[test]
    fn explicit_remote_and_url_aliases_are_deduplicated_without_changing_project_case_or_ports()
    -> Result<()> {
        let (_dir, git) = init_repo()?;
        remote(&git, "parent", "git@github.com:Team/Project.git")?;
        remote(&git, "origin", "https://github.com/user/fork")?;
        let selectors = [
            "parent",
            "https://GITHUB.com/Team/Project/",
            "ssh://git@github.com/Team/Project.git",
            "https://github.com/team/project",
            "https://github.com:8443/Team/Project",
        ]
        .map(str::to_string);
        assert_eq!(
            select(&git, &Config::default(), Some(&selectors))?,
            [
                "parent",
                "https://github.com/team/project",
                "https://github.com:8443/Team/Project"
            ]
        );
        Ok(())
    }

    #[test]
    fn invalid_explicit_repositories_never_fall_back_to_inferred_targets() -> Result<()> {
        let (_dir, git) = init_repo()?;
        remote(&git, "origin", "https://github.com/user/fork")?;
        for selector in [
            "missing",
            "file:///tmp/project",
            "https://unknown.example/team/project",
        ] {
            let error = select(
                &git,
                &Config::default(),
                Some(&["origin".into(), selector.into()]),
            )
            .unwrap_err();
            assert!(error.to_string().contains(selector));
            assert!(error.to_string().contains("--forge-repo"));
        }
        Ok(())
    }

    #[test]
    fn forced_provider_allows_explicit_self_hosted_repository_urls() -> Result<()> {
        let (_dir, git) = init_repo()?;
        let selectors = ["https://git.example.com/group/sub/project".to_string()];
        assert_eq!(
            select_repositories(
                &git,
                &Filter::load(&git, &Config::default())?,
                ForgeSetting::Kind(super::super::ForgeKind::GitLab),
                Some(&selectors)
            )?,
            selectors
        );
        Ok(())
    }
}
