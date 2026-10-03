# Packaging

Templates for the distribution channels that are not driven by `cargo publish`.
Both are rendered and pushed by workflows that run on `release: published` —
after gh-ship has undrafted the release, so the URLs they bake in already
resolve.

| Path | Channel | Workflow |
| --- | --- | --- |
| `aur/git-wipe/` | AUR, built from the release source tarball | `.github/workflows/aur.yaml` |
| `aur/git-wipe-bin/` | AUR, prebuilt binary (x86_64, aarch64) | `.github/workflows/aur.yaml` |
| `aur/git-wipe-git/` | AUR, tracks `main` | `.github/workflows/aur.yaml` |
| `homebrew/git-wipe.rb` | `noirbizarre/homebrew-tap` | `.github/workflows/homebrew.yaml` |

## The placeholder contract

The templates are not valid as they stand: the workflows substitute
`@VERSION@` and the `@SHA256*@` placeholders from the published release assets.

| Placeholder | Filled from |
| --- | --- |
| `@VERSION@` | the tag with its `v` stripped |
| `@SHA256@` | `git-wipe-<version>.tar.gz` |
| `@SHA256_X86_64@`, `@SHA256_AARCH64@` | `git-wipe-<arch>-unknown-linux-gnu.tar.gz` |
| `@SHA256_DARWIN_*@`, `@SHA256_LINUX_*@` | `git-wipe-<target>.tar.gz` (macOS, Linux musl) |

Checksums are always computed from the downloaded asset itself, never read from
the `.sha256` files published beside it: a mismatch between the two must not be
able to reach users.

`git-wipe-git` carries no placeholder — `makepkg` derives its `pkgver`
from the checkout, and its source is a git URL, so there is nothing to pin.

Nothing else in these templates may hardcode a version: adding an asset means
adding both a placeholder and the substitution that fills it, and
`homebrew.yaml` fails if any placeholder survives rendering.

## Renaming or removing a release asset

The templates address assets by name, so `publish-release.yaml` and these files
change together:

- `git-wipe-<target>.tar.gz` comes from `taiki-e/upload-rust-binary-action`
  (`archive: $bin-$target` by default) and carries **no leading directory** —
  `git-wipe`, `man/`, `completions/`, `LICENSE` and `README.md` sit at its root.
  `git-wipe-bin` and the formula both rely on that layout.
- `git-wipe-<version>.tar.gz` is produced by the `source` job, with a
  `git-wipe-<version>/` prefix so a PKGBUILD can `cd "$pkgname-$pkgver"`.

## One-off setup

Only the credentials, their environments and the empty tap repository are
manual. The pkgbases and the formula create themselves on the first run.

### AUR

Create an `aur` environment on the repository holding a single secret,
`AUR_SSH_PRIVATE_KEY`: the private half of an SSH key registered on the AUR
account that maintains the three pkgbases. It is kept out of the `release`
environment on purpose — a key that can push to the AUR has no business sitting
next to the GitHub App credentials.

That is the whole setup. The AUR creates a pkgbase on its first push, so the
workflow imports the three packages itself as long as the names are free and
the key belongs to the account claiming them — which is exactly what happened
for v0.3.0. Expect the AUR's RPC metadata to lag the package page by a few
minutes after an import: `aur.archlinux.org/packages/<name>` is authoritative,
`rpc/v5/info` is a cache.

### Homebrew

Create the public repository `noirbizarre/homebrew-tap` (the `homebrew-`
prefix is what makes `brew install noirbizarre/tap/git-wipe` work). An empty
repository is enough; the workflow creates `Formula/` on the first push.

Then create a `homebrew` environment holding `TAP_TOKEN`, a fine-grained token
with `contents: write` on that repository and nothing else.

## Re-running a failed publish

Both workflows are idempotent — they compare the staged index and exit early
when nothing changed — so a failed leg can simply be replayed:

```sh
gh workflow run aur.yaml -f tag=X.Y.Z
gh workflow run homebrew.yaml -f tag=X.Y.Z
```

## Testing a change

`aur.yaml` builds every non-VCS package before pushing it, so a broken PKGBUILD
fails the workflow rather than reaching users. To check one locally, substitute
the placeholders against an already published release and run:

```sh
cd packaging/aur/git-wipe-bin
makepkg -si --noconfirm
namcap PKGBUILD ./*.pkg.tar.zst
```

For the formula, render it and hand it to brew directly:

```sh
brew install --formula ./packaging/homebrew/git-wipe.rb
brew test git-wipe
brew audit --strict --online git-wipe
```
