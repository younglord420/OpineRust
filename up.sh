#!/usr/bin/env bash
# Installer script for the `pinecone` CLI.
#
# DISABLED IN THIS FORK.
#
# This script used to install binaries published by the upstream Pinecone
# project. OpineRust does not publish releases yet, so running it would install
# a Pinecone binary and present it as if it were an OpineRust build. That is
# misleading, so the script now refuses to run and points at the build-from-source
# path instead.
#
# When releases are prepared for this fork, replace the URL below with this
# repository's release assets and update the README accordingly.
#
# Upstream reference (unchanged for attribution):
#   https://github.com/ferranbt/pinecone
#
set -euo pipefail

say() { printf '\033[1;32m%s\033[0m %s\n' "opinerust:" "$*"; }
die() { printf '\033[1;31m%s\033[0m %s\n' "opinerust:" "$*" >&2; exit 1; }

cat >&2 <<'EOF'
opinerust: no releases are published from this fork yet.

This installer previously fetched binaries from the upstream Pinecone release
page. Installing those would put a Pinecone binary on your PATH under a name
that suggests it came from OpineRust. That is not accurate, so the script stops
here instead of doing it silently.

Build from source instead:

    git clone git@github.com:younglord420/OpineRust.git
    cd OpineRust
    cargo build --release -p pinecone
    install -m 755 target/release/pinecone ~/.local/bin/pinecone

See README.md ("Status") for what the runtime does and does not support.
EOF
exit 1
