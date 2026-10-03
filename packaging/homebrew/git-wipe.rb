# Homebrew formula template.
#
# `@VERSION@` and the `@SHA256_*@` placeholders are substituted by
# .github/workflows/homebrew.yaml from the published release assets, and the
# result is pushed to noirbizarre/homebrew-tap as Formula/git-wipe.rb.
#
# The formula is named after the binary (`git-wipe`), not the crate, because
# that is what `brew install noirbizarre/tap/git-wipe` has to spell.
class GitWipe < Formula
  desc "Wipe out merged local branches and worktrees"
  homepage "https://github.com/noirbizarre/git-wipe"
  version "@VERSION@"
  license "MIT"

  # Prebuilt binaries from the GitHub release rather than a source build: the
  # archives already carry the man pages and completions, and installing takes
  # no Rust toolchain.
  on_macos do
    on_arm do
      url "https://github.com/noirbizarre/git-wipe/releases/download/#{version}/git-wipe-aarch64-apple-darwin.tar.gz"
      sha256 "@SHA256_DARWIN_ARM64@"
    end
    on_intel do
      url "https://github.com/noirbizarre/git-wipe/releases/download/#{version}/git-wipe-x86_64-apple-darwin.tar.gz"
      sha256 "@SHA256_DARWIN_X86_64@"
    end
  end

  # musl rather than gnu: the binaries are statically linked, so they run on
  # any distribution Homebrew supports regardless of its glibc.
  on_linux do
    on_arm do
      url "https://github.com/noirbizarre/git-wipe/releases/download/#{version}/git-wipe-aarch64-unknown-linux-musl.tar.gz"
      sha256 "@SHA256_LINUX_ARM64@"
    end
    on_intel do
      url "https://github.com/noirbizarre/git-wipe/releases/download/#{version}/git-wipe-x86_64-unknown-linux-musl.tar.gz"
      sha256 "@SHA256_LINUX_X86_64@"
    end
  end

  depends_on "git"

  def install
    bin.install "git-wipe"
    # Git rewrites `git wipe --help` into `git help wipe`, which runs
    # `man git-wipe`: the pages are what makes that work.
    man1.install Dir["man/*.1"]
    bash_completion.install "completions/git-wipe.bash" => "git-wipe"
    zsh_completion.install "completions/_git-wipe"
    fish_completion.install "completions/git-wipe.fish"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/git-wipe --version")
    assert_match "worktree", shell_output("#{bin}/git-wipe --help")
  end
end
