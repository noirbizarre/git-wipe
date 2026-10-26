"""Exercise cache outages without depending on GitHub's cache service."""

import os
from pathlib import Path
import subprocess
import tempfile
import unittest


HELPER = Path(__file__).with_name("start-sccache.sh").resolve()


class CacheStartupTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        self.github_env = self.root / "github-env"
        self.github_env.touch()
        self.attempts = self.root / "attempts"
        self.environment = {
            **os.environ,
            "PATH": f"{self.root}{os.pathsep}{os.environ['PATH']}",
            "GITHUB_ENV": str(self.github_env),
            "ATTEMPTS": str(self.attempts),
        }
        self.executable(
            "sccache",
            'echo attempt >> "$ATTEMPTS"\n'
            'attempts=$(wc -l < "$ATTEMPTS")\n'
            'if [ "$attempts" -le "$FAILURES" ]; then\n'
            # A real backend error can contain a credential-bearing URL.
            '  echo "signed-storage-url" >&2\n'
            '  exit 1\n'
            'fi\n',
        )
        # Keep retry coverage fast and prove that waits are bounded too.
        self.executable("sleep", 'echo "$1" >> "$ATTEMPTS.waits"\n')

    def executable(self, name, body):
        path = self.root / name
        path.write_text("#!/usr/bin/env bash\nset -euo pipefail\n" + body)
        path.chmod(0o755)

    def run_helper(self, failures, compiler_status=0):
        self.environment["FAILURES"] = str(failures)
        # Run a subsequent build in the same strict shell to catch swallowed
        # build failures as well as accidental nonzero cache-only exits.
        return subprocess.run(
            ["bash", "-e", "-o", "pipefail", "-c",
             'bash "$1"; exit "$2"', "test", str(HELPER), str(compiler_status)],
            env=self.environment, capture_output=True, text=True, check=False,
        )

    def test_a_healthy_cache_enables_the_wrapper_without_retrying(self):
        result = self.run_helper(0)
        self.assertEqual(result.returncode, 0)
        self.assertEqual(self.github_env.read_text(), "RUSTC_WRAPPER=sccache\n")
        self.assertEqual(len(self.attempts.read_text().splitlines()), 1)
        self.assertFalse(self.root.joinpath("attempts.waits").exists())

    def test_a_transient_outage_enables_the_wrapper_after_retrying(self):
        result = self.run_helper(2)
        self.assertEqual(result.returncode, 0)
        self.assertEqual(self.github_env.read_text(), "RUSTC_WRAPPER=sccache\n")
        self.assertEqual(len(self.attempts.read_text().splitlines()), 3)
        self.assertEqual(self.root.joinpath("attempts.waits").read_text(), "5\n10\n")

    def test_a_persistent_outage_leaves_compilation_uncached_and_hides_signed_urls(self):
        result = self.run_helper(10)
        self.assertEqual(result.returncode, 0)
        self.assertEqual(self.github_env.read_text(), "")
        self.assertEqual(len(self.attempts.read_text().splitlines()), 3)
        self.assertIn("::warning::", result.stdout)
        self.assertNotIn("signed-storage-url", result.stdout + result.stderr)

    def test_a_compiler_failure_still_fails_after_cache_fallback(self):
        result = self.run_helper(10, compiler_status=17)
        self.assertEqual(result.returncode, 17)


if __name__ == "__main__":
    unittest.main()
