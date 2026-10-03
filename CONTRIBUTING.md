# Contributing to git-wipe

## Setup

The toolchain is managed by [mise](https://mise.jdx.dev/); every tool is pinned
to an exact build in `mise.lock`, so a local run and a CI run use the same
versions.

```bash
mise install          # the tools
prek install          # the Git hooks
mise run ci           # everything CI runs
```

Rust itself is not managed by mise: `rust-toolchain.toml` pins the channel, and
rustup installs it. CI uses `dtolnay/rust-toolchain` because it needs per-job
components and cross-compilation targets.

## Tasks

| Command | What it does |
|---|---|
| `mise run build` | Build the binary |
| `mise run cli` | Run `git-wipe` from source (passes flags through) |
| `mise run man` | Collect the generated man page and shell completions into `dist/` |
| `mise run test` | Run the tests (accepts nextest selectors) |
| `mise run cover` | Run the tests with coverage |
| `mise run format` | Format |
| `mise run lint` | Clippy, warnings denied |
| `mise run lint:actions` | actionlint over the workflows |
| `mise run lint:md` | markdownlint over AGENTS.md, CONTRIBUTING.md, README.md |
| `mise run spell` | typos |
| `mise run snapshots` | Review pending insta snapshots |
| `mise run check` | Everything that does not modify the working tree |
| `mise run ci` | `check` |
| `mise run changelog` | Preview the changelog for unreleased commits |
| `mise run release` | Show the version the next release would take |
| `mise run tpl:update` | Bring the rendered template ref up to date |
| `mise run tpl:diff` | Show what merging the template would change |

`mise <task>` is a shorthand for `mise run <task>`, but a builtin subcommand of
the same name wins it silently — which is why the format task is `format` and
not `fmt` (`mise fmt` formats `mise.toml`), and why running the binary is
`mise cli` and not `mise run` (`mise run` runs a task). Prefer the explicit
`mise run <task>` in scripts: mise can claim a new name in any release.

## Commits

[Conventional Commits](https://www.conventionalcommits.org/), enforced by
commitlint on `commit-msg`. The type selects the changelog section, and a `!`
or a `BREAKING CHANGE:` footer drives the version bump — so the message is part
of the release, not paperwork around it.

## Releases

Releases are run by [gh-ship](https://github.com/noirbizarre/gh-ship).
**Never bump a version or push a tag by hand.**

1. A push to `main` triggers 🚢 Ship, which runs `gh ship prepare`.
2. `prepare` dispatches 🚀 Prepare Release, which asks git-cliff for the next
   version, writes `CHANGELOG.md`, bumps `Cargo.toml`, commits, and uploads a
   `ship.release.json` artifact describing what would ship.
3. gh-ship opens (or updates) the Release PR from `release/next`. Review it.
4. Merging it triggers 🚢 Ship again, which runs `gh ship release`: it tags the
   merge commit, creates a draft release, dispatches 📦 Publish Release to
   attach the binaries and publish to crates.io, then makes the
   release public.

Nothing to release is the normal case for step 1, and costs one workflow run
reporting `changed: false`.

`gh ship validate` runs on every pull request, so a broken release contract
fails on the PR rather than mid-release.

### Repository requirements

The release jobs authenticate as a GitHub App, not with `GITHUB_TOKEN` — the
default token cannot trigger workflows, so a Release PR it authored would show
no CI results. That means the repository needs:

- a `release` environment holding the variable `APP_CLIENT_ID` and the secret
  `APP_PRIVATE_KEY`.
- squash-merge settings of `squash_merge_commit_title: PR_TITLE` and
  `squash_merge_commit_message: BLANK`, so the squash commit subject is the
  Conventional Commit title from `.github/ship.yml`.
- Trusted Publishing configured on crates.io for the `release` environment.
- a `homebrew` environment holding the secret `TAP_TOKEN`, scoped to push to
  `noirbizarre/homebrew-tap` only.
- an `aur` environment holding the secret `AUR_SSH_PRIVATE_KEY`, for the AUR
  account that owns the `git-wipe-bin` package.

## This repository is generated from a template

The toolchain, hooks, CI and release workflows come from
[rust.tpl](https://github.com/noirbizarre/rust.tpl):

```bash
git tpl status         # is there a template update pending?
mise run tpl:update    # advance refs/tpl/<id> — HEAD, index and worktree untouched
mise run tpl:diff      # read what merging it would change
git tpl merge          # take it
```

`tpl:update` is safe to run at any time: it only advances the rendered ref.
Nothing reaches your branch until the merge.

Requires git-tpl on your PATH (`cargo install git-tpl`). It is not declared in
`mise.toml`'s `[tools]` on purpose — it vendors libgit2, so a global entry
would make every CI job compile a tool no CI job runs.

Files carrying template-owned content — `mise.toml`, `prek.toml`, `Cargo.toml` —
end with a `# --- project-specific ---` marker. Add below it; Git's 3-way merge
then preserves your additions across updates. A fix that belongs to every
project belongs in the template, not here.
