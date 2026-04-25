#!/usr/bin/env bash
# Two-terminal hot-reload dev workflow, one script.
#
# Terminal 1 (this script): runs cargo-watch on synth-dsp/src and rebuilds the
# dylib on every save. The hot-lib-reloader inside the running app picks it up.
#
# Terminal 2 (you): cargo run -p synth
#
# If you don't have cargo-watch installed:  cargo install cargo-watch
set -euo pipefail
cd "$(dirname "$0")"

if ! command -v cargo-watch >/dev/null 2>&1; then
    echo "cargo-watch not found. Install with:  cargo install cargo-watch" >&2
    exit 1
fi

exec cargo watch -w synth-dsp/src -w synth-state/src -x 'build -p synth-dsp'
