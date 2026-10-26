#!/usr/bin/env bash
set -euo pipefail

# Cache storage can throttle independently of the authenticated GitHub API.
# Enable the wrapper only after startup succeeds, so an outage cannot block Cargo.
for attempt in 1 2 3; do
  # Backend errors can include signed storage URLs; report retries without them.
  if sccache --start-server > /dev/null 2>&1; then
    echo "RUSTC_WRAPPER=sccache" >> "$GITHUB_ENV"
    exit 0
  fi

  if [ "$attempt" -lt 3 ]; then
    echo "sccache startup attempt $attempt failed; retrying."
    sleep "$((attempt * 5))"
  fi
done

echo "::warning::Compiler cache unavailable after three attempts; compiling without sccache."
