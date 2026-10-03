# Consistency Report (SUPERSEDED)

> **This report is superseded and its recommendations must not be acted on.**
> It predates the migration from release-plz to gh-ship + git-cliff and
> references files and dependencies that no longer exist: `release-plz.toml`
> (removed), and `thiserror` (no longer a dependency). Its suggestion to remove
> `git-cliff` from `mise.toml` is now actively wrong -- git-cliff derives both
> the next version and the changelog. All `README.md:NNN` line references are
> also stale by roughly 80 lines.
>
> Run a fresh consistency review rather than relying on this one.

**Date**: 2026-04-10
**Project**: git-synchronizer (git-sync)
**Scope**: Doc-to-Doc, Doc-to-Code, Code-to-Code

## Summary

| Severity | Count |
|----------|-------|
| Critical | 1     |
| Major    | 6     |
| Minor    | 12    |
| **Total** | **19** |

---

## Critical Issues

> Actively misleading or dangerous. These should be fixed before any release.

### [Code-to-Code] `summary()` produces "worktreees" (triple-e) for worktree plurals

- **Files**:
  - `src/ui.rs:117-119` -- pluralizes by unconditionally appending `"es"`
  - `src/cleaner.rs:168` -- calls `ui.summary(removed, "worktree", "removed")`
- **Description**: The `summary()` method appends `"es"` for all plurals. This works for "branch" -> "branches" but produces "worktreees" for "worktree". The function is called with "worktree" as the noun at `cleaner.rs:168`.
- **Impact**: Users see "2 worktreees removed." in the CLI output -- a visible typo that undermines polish and trustworthiness of the tool.
- **Suggested fix**: Change the pluralization logic to accept the plural form as a parameter.

---

## Major Issues

> Significant discrepancies that cause confusion or wasted time.

### [Doc-to-Code] README says `git branch -d` but code uses `git branch -D`

- **Files**:
  - `README.md:145` -- "branches are deleted with `git branch -d`."
  - `README.md:182` -- Mermaid diagram shows `git branch -d`
  - `src/git.rs:172-179` -- code uses `-D` (force delete) with an explaining comment
- **Description**: The README documents the safe `-d` flag (refuses to delete unmerged branches), but the code deliberately uses `-D` (force delete). The code comment explains the rationale (worktree HEAD mismatch), but the docs were never updated.
- **Impact**: Users auditing the tool's safety model from documentation alone get a false impression.
- **Suggested fix**: Update README lines 145 and 182 to say `git branch -D`, and briefly explain why.

### [Doc-to-Code] `config set` subcommand exists in code but is undocumented

- **Files**:
  - `README.md:62-82` -- Configuration management section lists 8 subcommands, not `set`
  - `src/cli.rs:73-79` -- defines `ConfigAction::Set { key, value }`
  - `src/main.rs:80-85` -- implements `ConfigAction::Set`
  - `src/main.rs:182` -- error message tells users to run `git sync config set worktrunk false`
- **Description**: The `config set` subcommand is fully implemented and even referenced in a user-facing error message, but the README does not document it.
- **Impact**: Users cannot discover this command from the docs, yet the tool itself tells them to use it in error messages.
- **Suggested fix**: Add `config set <key> <value>` to the README's configuration management section.

### [Doc-to-Code] Fetch always runs on ALL remotes despite docs saying "configured (or all)"

- **Files**:
  - `README.md:130-131` -- "runs `git remote update --prune` on configured (or all) remotes"
  - `src/git.rs:126-128` -- `self.run(&["remote", "update", "--prune"])` (no remote args)
  - `src/cleaner.rs:24-37` -- `effective_remotes()` is only used for display/filtering, not for fetch
- **Description**: The README implies fetching is scoped to configured remotes. In reality, `git remote update --prune` without arguments fetches ALL remotes.
- **Impact**: Users who configure specific remotes might expect the tool to only contact those remotes during fetch.
- **Suggested fix**: Clarify in README that fetch always targets all remotes.

### [Code-to-Code] Heavily duplicated test helper `init_repo_with_branches()` (~7 copies)

- **Files**:
  - `src/branches.rs:118-215`, `src/cleaner.rs:257-342`, `src/config.rs:204-239`,
    `src/worktrees.rs:53-101`, `src/git.rs:932-979`, `tests/integration.rs:14-58`
- **Description**: The core "init temp git repo + initial commit" block (~20 lines) is duplicated at least 7 times with subtle divergences.
- **Impact**: If the init pattern needs to change, every copy must be updated independently.
- **Suggested fix**: Extract a shared test_utils module with composable builder functions.

### [Code-to-Code] Hardcoded `"sync."` strings in `main.rs` vs `SECTION` constant in `config.rs`

- **Files**:
  - `src/config.rs:6` -- `const SECTION: &str = "sync"`
  - `src/main.rs:81,88,94,96,98,105,111,113,115` -- hardcoded `"sync."` literals
- **Description**: `config.rs` centralizes the config section name in a constant, but `main.rs` bypasses it with hardcoded string literals.
- **Impact**: If the config section name is ever renamed, `main.rs` would silently break.
- **Suggested fix**: Export `SECTION` from `config.rs` and use it in `main.rs`.

### [Code-to-Code] `Vec::contains()` for O(n) membership checks alongside `HashSet` usage in same module

- **Files**:
  - `src/branches.rs:31` -- uses `HashSet` for protected branches (O(1) lookups)
  - `src/branches.rs:61,70,77,103` -- uses `Vec::contains()` for candidates (O(n) lookups)
- **Description**: Within the same module, `HashSet` is used for one membership-check pattern and `Vec::contains()` for another, creating O(n^2) behavior.
- **Impact**: On repos with many merged branches, this could cause noticeable slowdowns.
- **Suggested fix**: Use `HashSet` for candidates as well.

---

## Minor Issues

> Cosmetic or style issues with low impact.

### [Doc-to-Doc] Duplicate `# Changelog` header in CHANGELOG.md

- **Files**:
  - `CHANGELOG.md:1` and `CHANGELOG.md:10` -- both contain `# Changelog`
  - `release-plz.toml:28-31` -- defines `header` with `# Changelog`
- **Description**: The CHANGELOG has the title header twice, likely from release-plz prepending its template header to an existing file.
- **Suggested fix**: Remove the duplicate header at line 10.

### [Doc-to-Doc] `git-cliff` installed in mise.toml but unused

- **Files**:
  - `mise.toml:14` -- `git-cliff = "latest"`
  - `release-plz.toml` -- changelog generated by release-plz's built-in engine
- **Description**: `git-cliff` is installed as a dev tool but never invoked.
- **Suggested fix**: Remove `git-cliff` from `mise.toml`.

### [Doc-to-Doc] Phase numbering mismatch between README (1-4) and code comments (1,3,4,5)

- **Files**:
  - `README.md:128-161` -- four phases numbered 1 through 4
  - `src/cleaner.rs:22,43,86,132` -- comments numbered 1, 3, 4, 5 (skipping 2)
- **Description**: The code comments skip step 2, suggesting a removed phase that was never renumbered.
- **Suggested fix**: Renumber the code comments to 1, 2, 3, 4 to match the README.

### [Doc-to-Code] README says setup pre-selects `main`, `master` only; code also pre-selects `develop`, `development`

- **Files**:
  - `README.md:120` -- "Auto-detects local branches and pre-selects well-known ones (`main`, `master`)"
  - `src/config.rs:100` -- `let well_known = ["main", "master", "develop", "development"];`
- **Description**: The doc omits two of the four well-known branch names.
- **Suggested fix**: Update README to list all four.

### [Code-to-Code] Inconsistent test function return types across modules

- **Files**:
  - `src/git.rs` -- tests return `-> Result<()>` and use `?`
  - `src/branches.rs`, `src/cleaner.rs`, `src/worktrees.rs`, `src/config.rs` -- tests return `()` and use `.unwrap()`
- **Description**: Two different error-handling patterns in tests with no clear convention.
- **Suggested fix**: Pick one convention and apply it consistently.

### [Code-to-Code] `Ui` struct has fields and methods with identical names

- **Files**:
  - `src/ui.rs:6-10` -- pub fields: `heading`, `success`, `warning`, `muted`, `bold`
  - `src/ui.rs:32-55` -- methods: `heading()`, `success()`, `warning()`, `muted()`
- **Description**: Fields shadow method names, creating confusion when reading code.
- **Suggested fix**: Rename the fields (e.g., `heading_style`, `success_style`).

### [Code-to-Code] `Ui` output methods silently discard I/O errors

- **Files**:
  - `src/ui.rs:33-71` -- output methods use `let _ =` to discard write errors
  - `src/ui.rs:77-113` -- interactive methods return `Result<T>`
- **Description**: Two categories of `Ui` methods handle errors differently.
- **Suggested fix**: Add a brief comment explaining the design choice.

### [Code-to-Code] Missing `Default` and common derives on `CleanerOptions`

- **Files**:
  - `src/config.rs:21-29` -- `Config` implements `Default`
  - `src/cleaner.rs:10-18` -- `CleanerOptions` has no `Default`, `Debug`, or `Clone`
- **Description**: Similar config-like structs have inconsistent trait derivations.
- **Suggested fix**: Add `#[derive(Debug, Clone, Default)]` to `CleanerOptions`.

### [Code-to-Code] Inconsistent `#[derive]` traits across data structs

- **Files**:
  - `src/git.rs:356` -- `Worktree` derives `Debug, Clone, PartialEq, Eq`
  - `src/config.rs:9` -- `Config` derives `Debug, Clone` (no `PartialEq`)
  - `src/cleaner.rs:10` -- `CleanerOptions` derives nothing
- **Description**: Plain data structs have inconsistent derivation.
- **Suggested fix**: Harmonize derives across data structs.

### [Code-to-Code] `thiserror` dependency declared but never used

- **Files**:
  - `Cargo.toml:23` -- `thiserror = "2"`
- **Description**: The project depends on `thiserror` but uses `anyhow` exclusively.
- **Suggested fix**: Remove `thiserror` from `Cargo.toml` dependencies.

### [Code-to-Code] Different labels for missing branch name ("detached" vs "?")

- **Files**:
  - `src/cleaner.rs:146` -- `wt.branch.as_deref().unwrap_or("detached")`
  - `src/cleaner.rs:205` -- `wt.branch.as_deref().unwrap_or("?")`
- **Description**: The same condition displays as "detached" in one context and "?" in another.
- **Suggested fix**: Use a consistent label, e.g., "detached" everywhere.

### [Code-to-Code] Different confirm defaults for similar worktree removal prompts

- **Files**:
  - `src/cleaner.rs:153` -- orphan worktree removal defaults to `false` (safe)
  - `src/cleaner.rs:212` -- branch-associated worktree removal defaults to `true` (destructive)
- **Description**: Conceptually similar prompts have different default values.
- **Suggested fix**: Align defaults (both `false` for safety).

---

## Potential Issues

> Findings that may be intentional design choices. Included for review.

### [Doc-to-Doc] README title `# git-sync` vs crate name `git-synchronizer`

- **Files**:
  - `README.md:1` -- `# git-sync`
  - `Cargo.toml:2` -- `name = "git-synchronizer"`
  - `README.md:27-28` -- acknowledges the difference
- **Description**: The README uses the binary name as title while the crate is named differently. The README explains this.

### [Doc-to-Doc] Worktrunk auto-detection conditions understated in docs

- **Files**:
  - `README.md:123-124` -- says auto-detection checks `$PATH`
  - `src/main.rs:188-196` -- auto-detection during clean requires BOTH git config section AND `$PATH`
- **Description**: The "First run" section is accurate for the setup wizard, but the runtime auto-detection is stricter.

---

*Report generated by the consistency agent skill.*
