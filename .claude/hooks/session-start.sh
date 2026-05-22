#!/bin/bash
set -euo pipefail

# Installs the system libraries the workspace links against but that the
# Claude Code on the web container ships without:
#   - libasound2-dev (ALSA) -> alsa-sys, pulled in by kira (audio)
#   - libudev-dev          -> libudev-sys, pulled in by gilrs (gamepad)
# Without them `cargo build`/`test`/`clippy` fail in their build scripts.

# Only the remote (web) container needs this; local machines already have it.
if [ "${CLAUDE_CODE_REMOTE:-}" != "true" ]; then
  exit 0
fi

# Idempotent fast path: both .pc files present means a cached container reuse.
if pkg-config --exists alsa libudev 2>/dev/null; then
  exit 0
fi

SUDO=""
if [ "$(id -u)" -ne 0 ]; then
  SUDO="sudo"
fi

export DEBIAN_FRONTEND=noninteractive
$SUDO apt-get update -qq
$SUDO apt-get install -y -qq libasound2-dev libudev-dev >&2
