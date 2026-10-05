# Changelog

All notable changes to this project will be documented in this file.

This project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

> **This project was renamed.** It was `git-synchronizer`, providing a
> `git sync` command configured under `[sync]`; it is now `git-wipe`, providing
> `git wipe` configured under `[wipe]`. Entries below the rename refer to the
> old names, and their links resolve through GitHub's repository redirect.
> There is no automatic configuration migration — see the README for how to
> move an existing `[sync]` section across.

## [0.6.0](https://github.com/noirbizarre/git-wipe/compare/0.5.0..0.6.0) - 2026-10-05

### 💫 Features

- **config** Pre-fill the setup wizard from the existing configuration ([#97](https://github.com/noirbizarre/git-wipe/issues/97)) - ([fe45450](https://github.com/noirbizarre/git-wipe/commit/fe454502c7a6f917f881a216a79b6ee9b5753075))
- **forge** Use forge PR/MR state as the primary merge detection ([#95](https://github.com/noirbizarre/git-wipe/issues/95)) - ([6975be3](https://github.com/noirbizarre/git-wipe/commit/6975be3405030db06c3198dfdfc0e5c3d98963a3))
- **release** Publish native-layout archives from the template's archive step - ([21c1249](https://github.com/noirbizarre/git-wipe/commit/21c124977fb8b40a2ef0c7f4a26c8a2e8ba7217a))

### 🐛 Bug Fixes

- **branches** Compare remote ancestor merges against remote-tracking targets - ([1c6d82b](https://github.com/noirbizarre/git-wipe/commit/1c6d82b6bca8c6f95f818ef1b651040d9ea085c3))
- **cleaner** Preselect only pr-merged branches with a forge, and trust git without one ([#98](https://github.com/noirbizarre/git-wipe/issues/98)) - ([cd6b29a](https://github.com/noirbizarre/git-wipe/commit/cd6b29a11a242b964928081027de712549a66e19))
- **cleaner** Refuse a non-UTF-8 worktree path for worktrunk instead of mangling it - ([6df5b77](https://github.com/noirbizarre/git-wipe/commit/6df5b77045f31a172e570a0d2effe9591c74807a))
- **cleaner** Keep absolute paths in JSON errors for worktree removal and unlock - ([2ff2663](https://github.com/noirbizarre/git-wipe/commit/2ff266376720d4f1d216b090a393409e49f26ca0))
- **cleaner** Leave the branch of a guarded worktree alone instead of failing to delete it - ([b7f0111](https://github.com/noirbizarre/git-wipe/commit/b7f01112618eb2e6a818a83c504c315506f27750))
- **cli** --local-only and --remote-only are mutually exclusive - ([198b138](https://github.com/noirbizarre/git-wipe/commit/198b1383bcd0ff847f459fea832130b26b5c8cb3))
- **config** Validate config set values and keys, and read worktrunk as a git boolean - ([2a77aa6](https://github.com/noirbizarre/git-wipe/commit/2a77aa654984734202073c206f9f73269aeabba8))
- **config** Keep effort, minage, minsize, jobs and forge when re-running the setup wizard - ([fd90dae](https://github.com/noirbizarre/git-wipe/commit/fd90daed6a634ba1489e7151315d61781bb72173))
- **fetch** Prune stale tracking refs of ignored branches ([#94](https://github.com/noirbizarre/git-wipe/issues/94)) - ([4bae1e5](https://github.com/noirbizarre/git-wipe/commit/4bae1e56c11ef63d3b4f13205bf7006c33e54cd3))
- **git** Only treat a missing key as a successful config unset - ([cebc837](https://github.com/noirbizarre/git-wipe/commit/cebc8377c00e371feebe4e0773b8a94c77f8481e))
- **git** Report a failing first command of a pipe as a GitCommandError and drain its stderr - ([1be252a](https://github.com/noirbizarre/git-wipe/commit/1be252a9066747af63e31e94675ccc2e8efaa264))
- **git** Propagate unexpected merge-tree failures instead of reading them as not merged - ([6fcc0c9](https://github.com/noirbizarre/git-wipe/commit/6fcc0c9ed7fc9098a11bf6d1c2df9b1e94011271))
- **status** Warn about failed dirty and unmerged probes like the cleaner does - ([879b422](https://github.com/noirbizarre/git-wipe/commit/879b422899b680fc934c250853877e45cd6e7de3))

### 🔨 Refactor

- **status** Reorder table columns to branch, path, size, age, status ([#92](https://github.com/noirbizarre/git-wipe/issues/92)) - ([c2a06ca](https://github.com/noirbizarre/git-wipe/commit/c2a06ca74d6a768177d9df9e75102019b20eb0b4))
- Share the config section, worktrunk prompt and stale-gone warning, and render worktree paths consistently - ([8c01dff](https://github.com/noirbizarre/git-wipe/commit/8c01dff269f9a0f288072575c8e70dc48a052f79))

### 📚 Documentation

- **ci** The release workflow cross-compiles nine targets - ([9f19763](https://github.com/noirbizarre/git-wipe/commit/9f1976338cd5de50dc801a8fca181748c16dff94))
- **ci** Only the git-wipe AUR package consumes the source tarball - ([8e18d5e](https://github.com/noirbizarre/git-wipe/commit/8e18d5e7de219998ae04829ce935e08c491e45b2))
- **cli** --forge=<KIND> is for hosts whose name does not give the forge away - ([8553c94](https://github.com/noirbizarre/git-wipe/commit/8553c94043cad21117b76102d0854023910527ac))
- **cli** --json suppresses human-readable output instead of sending it to stderr - ([957fa32](https://github.com/noirbizarre/git-wipe/commit/957fa32522a6cfe96b57be74ce14819fe58d44ea))
- **config** Fix stale Config::default and wizard doc references, and the MinAge display example - ([41900e7](https://github.com/noirbizarre/git-wipe/commit/41900e7a2c4cd5e287e7b30658f206f30591388e))
- **main** Explain why fatal, per-item and forge error wording differ - ([f5ef641](https://github.com/noirbizarre/git-wipe/commit/f5ef6413dae739c71bc3abe857654db02a1212e1))
- **packaging** Drop the duplicated Homebrew section and fix the manual setup summary - ([900e191](https://github.com/noirbizarre/git-wipe/commit/900e191ec0d87f7040ffa4c1b0fe7f22d64bdf96))
- **readme** Wrap lines to the template's 120-column markdown lint - ([16571f3](https://github.com/noirbizarre/git-wipe/commit/16571f374cdfb4ef54b746a293127d5f5f494e8e))
- **readme** The simulated merge strategy needs git 2.38 - ([71ada22](https://github.com/noirbizarre/git-wipe/commit/71ada22cb1ddca8fb9443c6bc3b750d0a8fc7a5e))
- **readme** Document remotes_skipped and the per-phase skipped flags in the JSON schema - ([f4c0880](https://github.com/noirbizarre/git-wipe/commit/f4c0880550babbdd1f00404197d66b64be110c2a))
- **readme** Describe what mise run setup installs and what ship:validate needs - ([db7ebe4](https://github.com/noirbizarre/git-wipe/commit/db7ebe46db6fc0d5129ca1be72b854a212fc98b4))
- **readme** Deleted-upstream detection is also reported by status from on-disk refs - ([2c7b294](https://github.com/noirbizarre/git-wipe/commit/2c7b294976ef8aebd619864ec2511cec2f5149f7))
- **readme** Commit messages are checked by the local commit-msg hook only - ([833ef98](https://github.com/noirbizarre/git-wipe/commit/833ef985ec482dda78d30018f2114ee480ce279e))
- **readme** Mise does not install Rust, rustup is a prerequisite - ([5e267eb](https://github.com/noirbizarre/git-wipe/commit/5e267eb2bae4cd9da5d5530eb12d5538e0857c64))
- Document the GITHUB_ENTERPRISE_TOKEN variable and the accepted wipe.forge spellings - ([dd8efca](https://github.com/noirbizarre/git-wipe/commit/dd8efca0bdbef297f80fc5e328190033e6ffbf6d))
- --min-age measures the last change in a worktree, not its creation - ([0211b3f](https://github.com/noirbizarre/git-wipe/commit/0211b3fe0c4d0a8aa6a2931a91f9a254b8a7b4f5))
- Drop the pointer to a consistency report that does not exist - ([ae6d497](https://github.com/noirbizarre/git-wipe/commit/ae6d4970f77856904f7e0c8c60bfb3aa4a1d9d30))
- Forced removal prompt covers dirty worktrees only - ([5c68202](https://github.com/noirbizarre/git-wipe/commit/5c682027590e9b90a8a04017807d136d4b4e13da))

### 🏗️ Build

- **deps** Bump the rust-dependencies group with 9 updates ([#101](https://github.com/noirbizarre/git-wipe/issues/101)) - ([3cb70ab](https://github.com/noirbizarre/git-wipe/commit/3cb70abd1827d73b0c9f721ad0cb107d25355b5c))
- **mise** Lock the tools the template adds - ([b2e0e25](https://github.com/noirbizarre/git-wipe/commit/b2e0e253d4ff1f3f34c0216f51c1dce9a8db1179))

### 🔧 CI

- **release** Use bare version tags like the template - ([ff5aadd](https://github.com/noirbizarre/git-wipe/commit/ff5aadd721f4d9a97ed724f7dcc9a80555d55a5b))

### 🧹 Chores

- Adopt the rust.tpl template - ([cb50dc2](https://github.com/noirbizarre/git-wipe/commit/cb50dc2177d75d3da4682f5dbf66f8ac28fb6098))

### Tpl

- Render rust at main - ([318ccee](https://github.com/noirbizarre/git-wipe/commit/318cceee6f8fc050ef5fc9c8d6711882b9981030))

## [0.5.0](https://github.com/noirbizarre/git-wipe/compare/0.4.0..0.5.0) - 2026-08-19

### 💫 Features

- **ui** Enable fuzzy filtering on branch/worktree multi-selects ([#84](https://github.com/noirbizarre/git-wipe/issues/84)) - ([69a77fe](https://github.com/noirbizarre/git-wipe/commit/69a77feb13ba7d8fb5264d368417fb37ebb585fa))
- **worktrees** Recover from stale worktree locks instead of skipping forever ([#89](https://github.com/noirbizarre/git-wipe/issues/89)) - ([960202a](https://github.com/noirbizarre/git-wipe/commit/960202ac8be278197f65395b2c391f6085b135d3))
- **worktrees** Report worktree sizes and add a --min-size guard/filter ([#90](https://github.com/noirbizarre/git-wipe/issues/90)) - ([6914f77](https://github.com/noirbizarre/git-wipe/commit/6914f77a2d145f2761c4fa9bc1e587333bf18431))

### 🐛 Bug Fixes

- **worktrees** Treat a reference newer than now as zero age, not unknown - ([34d1596](https://github.com/noirbizarre/git-wipe/commit/34d159689f30dc279acd987a568cf39de20e09f2))
- **worktrees** Base worktree age on the last real change, not the admin dir timestamp ([#85](https://github.com/noirbizarre/git-wipe/issues/85)) - ([a53d7e0](https://github.com/noirbizarre/git-wipe/commit/a53d7e022ebfa6c1512a138980c4abb625ca5f16))

### 📚 Documentation

- **readme** Improve the badges row (strip spaces, add missing links, add titles, add AUR) ([#82](https://github.com/noirbizarre/git-wipe/issues/82)) - ([750e815](https://github.com/noirbizarre/git-wipe/commit/750e815965cc88383bcfd1424cb9d3c5c8962547))

### 🔧 CI

- Don't fail-fast the OS test matrix - ([ac92063](https://github.com/noirbizarre/git-wipe/commit/ac92063c3aa0626608e46a363219adb24478e1be))

## [0.4.0](https://github.com/noirbizarre/git-wipe/compare/0.3.0..0.4.0) - 2026-08-16

### 💫 Features

- **branches** Parallelise per-branch and per-worktree inspection ([#78](https://github.com/noirbizarre/git-wipe/issues/78)) - ([4765c8d](https://github.com/noirbizarre/git-wipe/commit/4765c8d4f4373454c299e6ba4bfb0dbdb96d8a00))
- **status** Add a read-only inventory subcommand (git sync status) ([#79](https://github.com/noirbizarre/git-wipe/issues/79)) - ([8a33d0c](https://github.com/noirbizarre/git-wipe/commit/8a33d0c58e04607a09e1a4ddc974e2d96e34c140))

### 🐛 Bug Fixes

- **ci** Keep the source tarball check from killing its own job ([#74](https://github.com/noirbizarre/git-wipe/issues/74)) - ([f99d2c4](https://github.com/noirbizarre/git-wipe/commit/f99d2c44e153b2901260f1f897b0eb9107adbffa))

### 🔨 Refactor

-  🚨 **breaking** Rename the project to git-wipe ([#80](https://github.com/noirbizarre/git-wipe/issues/80)) - ([5f5bcca](https://github.com/noirbizarre/git-wipe/commit/5f5bccac530829d5a86ee5108f0aae57d91f1ea2))

### 📚 Documentation

- **packaging** Drop the AUR bootstrap that was never needed ([#76](https://github.com/noirbizarre/git-wipe/issues/76)) - ([3c5a0d5](https://github.com/noirbizarre/git-wipe/commit/3c5a0d575e7b5c3706e8788f439e39fcb19a3458))

## [0.3.0](https://github.com/noirbizarre/git-wipe/compare/0.2.0..0.3.0) - 2026-08-15

### 💫 Features

- **artwork** Add the logo, icon and social preview ([#55](https://github.com/noirbizarre/git-wipe/issues/55)) - ([1645f1a](https://github.com/noirbizarre/git-wipe/commit/1645f1a753fbe028fabe2bcfc38302b0b8352d2e))
- **branches** Apply advanced merge detection to remote branches ([#59](https://github.com/noirbizarre/git-wipe/issues/59)) - ([dbeb892](https://github.com/noirbizarre/git-wipe/commit/dbeb892cbf8bdde021295a58b99cdb9271940195))
- **branches** Add effort levels for merge detection ([#58](https://github.com/noirbizarre/git-wipe/issues/58)) - ([77b40cf](https://github.com/noirbizarre/git-wipe/commit/77b40cf4b5dab86798acea495060c32acacf643b))
- **ci** Publish a source tarball and an Intel macOS binary - ([8579497](https://github.com/noirbizarre/git-wipe/commit/85794979dab58e3933d5d919519d42c82f388713))
- **cli**  🚨 **breaking** Decouple --force from --yes for worktree force-removal ([#63](https://github.com/noirbizarre/git-wipe/issues/63)) - ([19aeb31](https://github.com/noirbizarre/git-wipe/commit/19aeb31079f889d1c124955b09b0dce976e072c4))
- **cli** Add --json machine-readable output ([#57](https://github.com/noirbizarre/git-wipe/issues/57)) - ([e351e4a](https://github.com/noirbizarre/git-wipe/commit/e351e4ae477f6a8df22e5d7d0deb18a738275867))
- **docs** Generate and ship man pages and shell completions ([#60](https://github.com/noirbizarre/git-wipe/issues/60)) - ([a95efc4](https://github.com/noirbizarre/git-wipe/commit/a95efc46f5546724c6300acaa78e2f92b13ee33a))
- **packaging** Publish a Homebrew formula - ([bae2296](https://github.com/noirbizarre/git-wipe/commit/bae2296ee7d9c5d353d980e0d406caf87e15912b))
- **packaging** Publish on the AUR - ([953316b](https://github.com/noirbizarre/git-wipe/commit/953316b58d7f8937e64ca35619ccdd53c3725614))
- **worktrees** Add a min-age guard for worktree removal (`--min-age`) ([#62](https://github.com/noirbizarre/git-wipe/issues/62)) - ([ffb7c24](https://github.com/noirbizarre/git-wipe/commit/ffb7c24973b2784c797dd16d0b8ce1de8984242b))

### 📚 Documentation

- **readme** Defer the CLI reference to --help and man pages ([#64](https://github.com/noirbizarre/git-wipe/issues/64)) - ([5cfe1e7](https://github.com/noirbizarre/git-wipe/commit/5cfe1e73cbdcd3a4bc894e14010b6312a434bda4))

## [0.2.0](https://github.com/noirbizarre/git-wipe/compare/0.1.1..0.2.0) - 2026-08-15

### 💫 Features

- **branches** Ignore branches matching configured patterns ([#53](https://github.com/noirbizarre/git-wipe/issues/53)) - ([2bf4fbc](https://github.com/noirbizarre/git-wipe/commit/2bf4fbce1b027cdd0fba197992e2929be318742d))
- **branches** Detect branches whose upstream was deleted - ([8a0b3d1](https://github.com/noirbizarre/git-wipe/commit/8a0b3d1a509d68cd193893cdc239fa9afc68843b))
- **branches** Detect squash-merged branches via combined patch-id ([#43](https://github.com/noirbizarre/git-wipe/issues/43)) - ([a83bc0a](https://github.com/noirbizarre/git-wipe/commit/a83bc0ad2ab5f9b3898a27609edf631c3888cecf))
- **branches** Detect branches whose simulated merge adds nothing ([#41](https://github.com/noirbizarre/git-wipe/issues/41)) - ([ce92247](https://github.com/noirbizarre/git-wipe/commit/ce92247b47a267665771661c2862fa164e32a20a))
- **branches** Add patch-id match for merged branch detection ([#40](https://github.com/noirbizarre/git-wipe/issues/40)) - ([725fdfa](https://github.com/noirbizarre/git-wipe/commit/725fdfaf65800eab68621869e5fcc66759604909))
- **branches** Add tree SHA comparison for merged branch detection ([#21](https://github.com/noirbizarre/git-wipe/issues/21)) - ([3e9b570](https://github.com/noirbizarre/git-wipe/commit/3e9b570ef16b6e00b771dafb293720abcdf728fa))
- **branches** Add empty diff detection for squash-merged branches ([#20](https://github.com/noirbizarre/git-wipe/issues/20)) - ([454ee95](https://github.com/noirbizarre/git-wipe/commit/454ee95f77bf6eeabbf20639a8572fac1f8c5164))
- **cleaner** Fast-forward target branches before merge detection ([#23](https://github.com/noirbizarre/git-wipe/issues/23)) - ([8dd9386](https://github.com/noirbizarre/git-wipe/commit/8dd9386cc00d3bec0ed9f41416b3be55c00f1283))
- **ui** Show spinners during slow operations ([#48](https://github.com/noirbizarre/git-wipe/issues/48)) - ([b15df03](https://github.com/noirbizarre/git-wipe/commit/b15df038676125763c65e8c9a2dba9171ace41d4))
- **ui** Add reverse-selection shortcut to multi-select prompts ([#47](https://github.com/noirbizarre/git-wipe/issues/47)) - ([ec50ef5](https://github.com/noirbizarre/git-wipe/commit/ec50ef5ddc0e5cb18892896f0528cfc2c61e47f4))
- **worktrees** Skip locked worktrees during cleanup ([#22](https://github.com/noirbizarre/git-wipe/issues/22)) - ([32f7ab0](https://github.com/noirbizarre/git-wipe/commit/32f7ab011f381d6efc5f47f59754ec3a71f0f921))
- Add worktree force-removal prompts with dirty/unmerged detection ([#39](https://github.com/noirbizarre/git-wipe/issues/39)) - ([7353707](https://github.com/noirbizarre/git-wipe/commit/73537073dc1fbec50aa60b367fef8661522420ed))

### 🐛 Bug Fixes

- **changelog** Remove duplicate header - ([7fccf54](https://github.com/noirbizarre/git-wipe/commit/7fccf54d9c414767eec97d222175bee738626d19))
- **ci** Move codecov status config under coverage top-level key ([#35](https://github.com/noirbizarre/git-wipe/issues/35)) - ([f98ecc0](https://github.com/noirbizarre/git-wipe/commit/f98ecc0081c7d68aae811495a5f7499a68d79f55))
- **ci** Use GitHub App token for release-plz to fix PR recreation ([#16](https://github.com/noirbizarre/git-wipe/issues/16)) - ([0bce1f8](https://github.com/noirbizarre/git-wipe/commit/0bce1f831f0a4708c05e94a4746cd83d5a465ffa))
- **cleaner** Honour --dry-run when worktrunk handled the worktree - ([fe93761](https://github.com/noirbizarre/git-wipe/commit/fe93761ea19d389a6fc7a767a0773c3155248e54))
- **cleaner** Delete branches worktrunk leaves behind on worktree removal ([#46](https://github.com/noirbizarre/git-wipe/issues/46)) - ([2f46528](https://github.com/noirbizarre/git-wipe/commit/2f465285cff5377582fb536ea28f37c0dbe73802))
- **cleaner** Skip force-removal prompt for merged-but-unreachable branches ([#44](https://github.com/noirbizarre/git-wipe/issues/44)) - ([79c3d50](https://github.com/noirbizarre/git-wipe/commit/79c3d50e855d7df060125f39b3beba0a43682b4b))
- **cli** Show clean error when run outside a git repository ([#36](https://github.com/noirbizarre/git-wipe/issues/36)) - ([aeb8fba](https://github.com/noirbizarre/git-wipe/commit/aeb8fbae8e9186bf41e66490c2147621486d4d8e))
- **errors** Stop swallowing git failures as empty results - ([c912daf](https://github.com/noirbizarre/git-wipe/commit/c912daf5d7964493c8057033ed63d0afcd3bc3b0))
- **git** Compare against the merge base in empty-diff detection - ([0c2c663](https://github.com/noirbizarre/git-wipe/commit/0c2c66305fc7d8fe3624d048358009f415e8727c))
- **git** Handle network failures gracefully ([#42](https://github.com/noirbizarre/git-wipe/issues/42)) - ([3587d93](https://github.com/noirbizarre/git-wipe/commit/3587d93dd9929a3b54a5fc2423bbe8a5f55cc02b))
- **ui** Correct summary() pluralization to avoid 'worktreees' - ([4c4c8f0](https://github.com/noirbizarre/git-wipe/commit/4c4c8f0253e37cb30cecb143e48bdad90882d1d4))
- Correct worktrunk detection, config set and a bogus test - ([331cc41](https://github.com/noirbizarre/git-wipe/commit/331cc41f618a0cc929758d6bc1bc08d6c7a0095a))

### ⚡ Performance

- **branches** Use HashSet for O(1) candidate membership checks - ([e7894fc](https://github.com/noirbizarre/git-wipe/commit/e7894fcb51b5e6c84cf38ec424661240eaec1f42))

### 🔨 Refactor

- **cleaner** Simplify deleted-upstream messaging - ([ad896b1](https://github.com/noirbizarre/git-wipe/commit/ad896b19602ca43cbe6c047d0c4f4f30222a8fcd))
- **cleaner** Unify branch and worktree deletion into a single multiselect ([#37](https://github.com/noirbizarre/git-wipe/issues/37)) - ([05b7f5d](https://github.com/noirbizarre/git-wipe/commit/05b7f5d719541884895da96b9a48f930876e568e))
- **cleaner** Add Debug, Clone, Default derives to CleanerOptions - ([513bb52](https://github.com/noirbizarre/git-wipe/commit/513bb523d7273458f6924d4ca3897f4871acafac))
- **config** Extract config_remove_value onto Git - ([d2135bf](https://github.com/noirbizarre/git-wipe/commit/d2135bf3d60c61812e21be2d3342e95dbedc5bff))
- **config** Use SECTION constant instead of hardcoded 'sync.' strings - ([0ad468b](https://github.com/noirbizarre/git-wipe/commit/0ad468b26e119d89fd96298860dadfc8241253f9))
- **errors** Unify git error classification and reporting - ([6f67e2e](https://github.com/noirbizarre/git-wipe/commit/6f67e2e1693078bf05a06481aaf4c21c2807e18c))
- **git** Merge the two worktrunk removal methods - ([b1cc419](https://github.com/noirbizarre/git-wipe/commit/b1cc419235833496fb7bfd0a7abd6bccae8fb4d3))
- **git** Use a single working-directory mechanism - ([37ec64a](https://github.com/noirbizarre/git-wipe/commit/37ec64af33ff9fae05e1dfa0f9228e44e0866be1))
- **git** Route every git invocation through the shared run helper - ([5d8aa71](https://github.com/noirbizarre/git-wipe/commit/5d8aa71badbdae20e50e905fd628f256bad80ae2))
- **tests** Extract shared test helpers into test_helpers module - ([e99f0a5](https://github.com/noirbizarre/git-wipe/commit/e99f0a5e40490d242646e8576fbba2f5daa82715))
- **tests** Standardize test return types to Result<()> - ([c5c2afb](https://github.com/noirbizarre/git-wipe/commit/c5c2afbcbed18e804436a4e109db6f0062acf8f5))
- **ui** Render config output through the Ui abstraction - ([0f50e12](https://github.com/noirbizarre/git-wipe/commit/0f50e123782d67e56d91b13f266cd938281cd9e6))
- **ui** Rename Ui fields to avoid shadowing method names - ([40c8653](https://github.com/noirbizarre/git-wipe/commit/40c8653c2f9b5e094a11dcdd31b2006250ae2ae7))
- **worktrees** Integrate worktree selection into branch multiselect ([#32](https://github.com/noirbizarre/git-wipe/issues/32)) - ([9da4a1c](https://github.com/noirbizarre/git-wipe/commit/9da4a1c11548920f4b8a698df68d01cec455a2e0))
- Use Path types and consistent derives across peer APIs - ([4ba8639](https://github.com/noirbizarre/git-wipe/commit/4ba8639ea6a07aff08bc0dd1f05f13e9300900f4))

### 📚 Documentation

- **readme** Correct merge-detection diagram, refspec and worktrunk notes - ([dda94db](https://github.com/noirbizarre/git-wipe/commit/dda94dbaa68bedb2a07b44910d57534975ed869a))
- **readme** Describe the dirty-worktree confirmation pass - ([95b3d93](https://github.com/noirbizarre/git-wipe/commit/95b3d930a7b306eed4b9fedc1888caf299897713))
- **readme** Document prek hook installation in the dev workflow - ([ed15f09](https://github.com/noirbizarre/git-wipe/commit/ed15f09fe80bbc33dd6a942853f06a03a0d8a0ba))
- **readme** Correct the fetch step command and scope - ([f95d215](https://github.com/noirbizarre/git-wipe/commit/f95d21538a0d4e2bddcee097cdede75985b0b4c0))
- **readme** Document the full merge detection stack - ([fc0c576](https://github.com/noirbizarre/git-wipe/commit/fc0c576b1d42249cc10ad916594b7ed0eb6682de))
- **readme** List all four well-known protected branch names - ([bb4969d](https://github.com/noirbizarre/git-wipe/commit/bb4969d7ae5cf25e171c38fc07c80103606f700b))
- **readme** Clarify that fetch always targets all remotes - ([c067300](https://github.com/noirbizarre/git-wipe/commit/c067300ca30f3e33f2ded8dd113b2473dbca8d5a))
- **readme** Document the config set subcommand - ([4ac1bba](https://github.com/noirbizarre/git-wipe/commit/4ac1bba6b11398257052f733724c318e83b49266))
- **readme** Fix branch deletion flag from -d to -D - ([0a83097](https://github.com/noirbizarre/git-wipe/commit/0a8309798bf44a38779f2507ba00a5d614d4daf7))
- **rustdoc** Correct docs contradicting their implementations - ([9694c8a](https://github.com/noirbizarre/git-wipe/commit/9694c8a13e95854d1a0f0efaa8cef96b67e51cd7))
- **ui** Document why output methods discard I/O errors - ([1e483b2](https://github.com/noirbizarre/git-wipe/commit/1e483b23725347b03caaf59fffd76d05290cf327))

### 🧪 Tests

- **branches** Pin remote detection to ancestor merges - ([533bfae](https://github.com/noirbizarre/git-wipe/commit/533bfae3999cdd9191030e4eb4470c993f8e72a1))
- Drop the redundant test_ prefix from unit test names - ([0a379da](https://github.com/noirbizarre/git-wipe/commit/0a379daf760a3272153ed6e275a8a164e981beeb))
- Consolidate repository fixtures into test_helpers - ([980218d](https://github.com/noirbizarre/git-wipe/commit/980218d824be8c5d83f164795915fb69a9aefcb9))

### 🎨 Style

- **cleaner** Renumber workflow phase comments to match README - ([170d31b](https://github.com/noirbizarre/git-wipe/commit/170d31b4713dae8655389b89b77169f42ce35e7e))
- **ui** Add colored prefixes and per-fragment styling to output methods ([#34](https://github.com/noirbizarre/git-wipe/issues/34)) - ([09bde99](https://github.com/noirbizarre/git-wipe/commit/09bde99afd49f2deb70533b6e15b52651b408256))
- Unify imports, visibility, naming and doc coverage - ([cc8df59](https://github.com/noirbizarre/git-wipe/commit/cc8df595d04f9c4cb5d669367ef3651244af4334))

### 🏗️ Build

- **lint** Align mise lint task with the enforced clippy scope - ([fe06fd7](https://github.com/noirbizarre/git-wipe/commit/fe06fd783d45e8c51f1cb2e93ac1473a67d85379))

### 🔧 CI

- **codecov** Get Codecov under control ([#33](https://github.com/noirbizarre/git-wipe/issues/33)) - ([17e6e57](https://github.com/noirbizarre/git-wipe/commit/17e6e576f7d9f395780acbbbad907eeb84c691b4))
- **mise** Remove the debug path prefix ([#45](https://github.com/noirbizarre/git-wipe/issues/45)) - ([5cc6bff](https://github.com/noirbizarre/git-wipe/commit/5cc6bff6183848fede3ab3849f81078de4a1c9b5))
- **release** Fix release commit rule ([#19](https://github.com/noirbizarre/git-wipe/issues/19)) - ([67421dd](https://github.com/noirbizarre/git-wipe/commit/67421dda2904c95f204d84b2fa265879e9219fce))
- Release with gh-ship and git-cliff instead of release-plz ([#50](https://github.com/noirbizarre/git-wipe/issues/50)) - ([8e73537](https://github.com/noirbizarre/git-wipe/commit/8e73537f47f7b4add45a08c1c677c530079ac5e4))
- Force the worktrunk install so coverage stays deterministic - ([f9722bd](https://github.com/noirbizarre/git-wipe/commit/f9722bd7babc331eadb43c86f65e72b5a20de52a))

### 🧹 Chores

- **mise** Remove unused git-cliff dependency - ([e431570](https://github.com/noirbizarre/git-wipe/commit/e431570d59823e39cea8479374ac5b5248448c42))

## [0.1.1](https://github.com/noirbizarre/git-wipe/compare/0.1.0..0.1.1) - 2026-04-09

### 📚 Documentation

- **changelog** Exclude ci, test, style and merge commits ([#10](https://github.com/noirbizarre/git-wipe/issues/10)) - ([12d7c86](https://github.com/noirbizarre/git-wipe/commit/12d7c86329d4801f161a14f6d9e1b61145e51740))

### 🔧 CI

- **release** Enable release for first trial - ([bfd5f1c](https://github.com/noirbizarre/git-wipe/commit/bfd5f1c62efcab3882cd7e3d6b7423b5a02123e2))
- Rewrite release-binaries workflow based on release-plz model ([#12](https://github.com/noirbizarre/git-wipe/issues/12)) - ([366fac8](https://github.com/noirbizarre/git-wipe/commit/366fac8289a9b91a5626cc75ecd2c9c71f77ee29))
- Replace check job with lint using prek-action v2 ([#8](https://github.com/noirbizarre/git-wipe/issues/8)) - ([28d5f1b](https://github.com/noirbizarre/git-wipe/commit/28d5f1bac7e3a6b80079a8f16a13eaa1e42914ec))
- Merge coverage into test job ([#6](https://github.com/noirbizarre/git-wipe/issues/6)) - ([115505b](https://github.com/noirbizarre/git-wipe/commit/115505bbee21cc49078fd11fcba0b081642a318b))
- Release and CI improvements - ([c41c73d](https://github.com/noirbizarre/git-wipe/commit/c41c73d2f970c00fd8f79ea327300c3911634d33))

## 0.1.0 - 2026-04-08

### 💫 Features

- **config** Per-branch config protection support - ([c950fb5](https://github.com/noirbizarre/git-wipe/commit/c950fb59a8f810bfa2f7a726304ff691aece4833))
- **worktrunk** Delegate worktree removal to worktrunk when available ([#1](https://github.com/noirbizarre/git-wipe/issues/1)) - ([926c4bf](https://github.com/noirbizarre/git-wipe/commit/926c4bf1248ffe0b4e980409994142474be8de16))

### 🐛 Bug Fixes

- **ci** Fix changelog config for release-plz ([#4](https://github.com/noirbizarre/git-wipe/issues/4)) - ([2089e1a](https://github.com/noirbizarre/git-wipe/commit/2089e1a63fb96ab9f982d4beb5a2e1640e1f6005))
- **config** Support repositories with extensions.worktreeConfig enabled ([#3](https://github.com/noirbizarre/git-wipe/issues/3)) - ([64a2ffa](https://github.com/noirbizarre/git-wipe/commit/64a2ffaf66e5065d0ae6fe72cd1bb0c9a3016b0a))

### 🔨 Refactor

- Rename into Git Synchronizer / `git-sync` - ([5d59a1a](https://github.com/noirbizarre/git-wipe/commit/5d59a1a57b891fb183bced2304f0a99c3a6074ed))

### 📚 Documentation

- **readme** Document per-branch protection and worktrunk integration ([#2](https://github.com/noirbizarre/git-wipe/issues/2)) - ([efc11b2](https://github.com/noirbizarre/git-wipe/commit/efc11b2b0baac20776de74595d35e69a74281e7f))

## ❤️ New Contributors

* @noirbizarre made their first contribution in [#4](https://github.com/noirbizarre/git-wipe/pull/4)
