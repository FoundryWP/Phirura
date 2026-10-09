#!/usr/bin/env bash
# Fill cargo's crate cache for the phira workspace.
#
# Why this exists: `cargo fetch` hangs on this machine. Not the network - `curl` pulls the same
# crates in under a second - but cargo's own HTTP client stalls, and it stalls on the FIRST crate
# it has to download, so nothing else gets a chance. (`cargo fetch --offline` names the victim,
# which is how this was found.)
#
# The workaround is to put the .crate files in the cache by hand, which is a supported layout:
#
#     registry/cache/<registry-hash>/<name>-<version>.crate
#
# cargo then treats them as already downloaded. Loop until --offline stops complaining.

set -u

CARGO_HOME="${CARGO_HOME:-C:/Users/admin/.workbuddy-ai/binaries/rust/cargo}"
export RUSTUP_HOME="${RUSTUP_HOME:-C:/Users/admin/.workbuddy-ai/binaries/rust/rustup}"
export RUSTUP_TOOLCHAIN=nightly
CARGO="$CARGO_HOME/bin/cargo.exe"
CACHE="$CARGO_HOME/registry/cache/index.crates.io-1949cf8c6b5b557f"
TARGET="E:/Users/admin/Documents/visual studio 2013/Phirura/rust-wp81/targets/thumbv7a-pc-windows-msvc.json"

# The probe must be the *real* command, not `cargo fetch`.
#
# Cargo unifies features per invocation, and the three invocations disagree:
#
#   cargo fetch                                    -> "everything present"
#   cargo fetch --target <json>                     -> asks for aws-lc-sys
#   cargo check -p prpr --target <json>             -> asks for av-scenechange
#
# `cargo fetch` also has no `-p`, so it cannot be scoped to match. Only the actual check resolves
# the graph the build will use, so that is the probe.
#
# It is cheap while crates are missing: cargo resolves and downloads before it compiles anything,
# so a missing crate fails in seconds with no compilation output. Once nothing is missing the probe
# turns into a real build, which is the point at which this script should stop being used.
PROBE=("$CARGO" check -p prpr --offline
       --target "$TARGET" -Zjson-target-spec
       -Zbuild-std=std,panic_abort
       -Zbuild-std-features=compiler-builtins-mem)

mkdir -p "$CACHE"

# Explicit: `bash script.sh` does not reliably inherit the caller's working directory here, and
# cargo resolves the workspace from the CWD. Getting this wrong makes the probe run in the wrong
# workspace and report a different (smaller) crate set - which looks exactly like "all present".
WORKSPACE="E:/Users/admin/Documents/visual studio 2013/Phirura/phira-main/phira-main"
cd "$WORKSPACE" || { echo "cannot cd to $WORKSPACE"; exit 1; }
echo "workspace: $(pwd)"

for i in $(seq 1 400); do
  out="$("${PROBE[@]}" 2>&1)"

  name="$(printf '%s\n' "$out" | sed -n 's/.*failed to download `\([^`]*\)`.*/\1/p' | head -1)"

  if [ -z "$name" ]; then
    echo "=== ALL CRATES PRESENT (after $i iteration(s)) ==="
    printf '%s\n' "$out" | grep -cE "error" || true
    printf '%s\n' "$out" | tail -15
    exit 0
  fi

  pkg="$(printf '%s' "$name" | cut -d' ' -f1)"
  ver="$(printf '%s' "$name" | cut -d' ' -f2)"

  url="https://static.crates.io/crates/$pkg/$pkg-$ver.crate"
  code="$(curl -sSL -o "$CACHE/$pkg-$ver.crate" -w '%{http_code}' "$url" || echo 000)"

  echo "[$i] $pkg-$ver -> http=$code"

  if [ "$code" != "200" ]; then
    rm -f "$CACHE/$pkg-$ver.crate"
    echo "=== STOPPED: could not download $pkg-$ver ==="
    printf '%s\n' "$out" | tail -5
    exit 1
  fi
done

echo "=== gave up after 400 iterations ==="
exit 1
