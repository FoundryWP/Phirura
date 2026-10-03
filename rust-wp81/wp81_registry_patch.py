#!/usr/bin/env python3
r"""Strip `raw-dylib` link attributes from third-party crates in the cargo registry.

Why this exists, and why it is separate from `wp81_std_patch.py`
----------------------------------------------------------------
`wp81_std_patch.py` rewrites bindings in Rust's own `rust-src/library/` tree. That covers every
declaration `std` makes, but it cannot reach a declaration made by a *dependency*: `cargo` compiles
those straight out of `$CARGO_HOME/registry/src/`, and there is no patch point in between.

Two dependencies bind Win32 entry points with `#[link(kind = "raw-dylib")]` of their own:

    which-8.x       src/win_ffi.rs      GetBinaryTypeW, GetLastError   (kernel32.dll)
    getrandom-0.3.x src/backends/windows.rs         ProcessPrng    (bcryptprimitives.dll)
    getrandom-0.4.x src/backends/windows.rs         ProcessPrng    (bcryptprimitives.dll)

None of those libraries exist for a WP8.1 app, and none of those symbols is reachable any other
way - `raw-dylib` bakes the DLL name into an *import descriptor* that the linker emits whatever
else happens. `/FORCE:MULTIPLE` (which `build-port.ps1` needs) keeps the first *symbol* definition
- `wp81-compat`'s - and reports the duplicate as LNK4006 "second definition ignored", which reads
like the import is gone. It is not: the descriptor is a separate contribution to `.idata`, and the
import table ends up naming `kernel32.dll` and `bcryptprimitives.dll`. On the device that is a
load-time failure, because WP8.1's loader refuses KERNEL32.dll outright.

So the fix is the same one `wp81_std_patch.py` applies to std: delete the `link` attribute. The
extern then names a symbol nothing imports, and `wp81-compat` - which already defines all three as
`#[no_mangle]` - supplies it. That is exactly how std's own `GetLastError` is handled.

Why patch the registry in place rather than vendor the crates
-------------------------------------------------------------
Vendoring (the `[patch.crates-io]` route used for `rfd`/`macroquad`) is the right tool when a crate
cannot *compile*. Here both crates compile perfectly; one attribute is wrong. Vendoring them would
mean shipping a copy of `getrandom`'s entire API surface to change one line, and re-vendoring on
every upstream bump. A textual patch keyed on the exact attribute text is smaller and fails loudly
if the shape changes.

Cargo does not re-hash extracted registry sources, but it *does* check a `.cargo-ok` marker before
reusing a directory - which we leave in place. The patch is therefore idempotent: re-running it
finds the attribute already gone and reports 0 changes rather than corrupting anything.

Usage:
    python wp81_registry_patch.py --check     # report what would change
    python wp81_registry_patch.py --apply      # rewrite in place
"""

import argparse
import os
import re
import sys

CARGO_HOME = os.environ.get("CARGO_HOME",
                            r"C:\Users\admin\.workbuddy-ai\binaries\rust\cargo")
REGISTRY_SRC = os.path.join(CARGO_HOME, "registry", "src")

# crates whose `raw-dylib` binding must be dropped. Matched by directory *prefix* so a patch bump
# (`which-8.0.2` -> `which-8.1.0`) keeps working without editing this file.
TARGET_CRATES = ("which-", "getrandom-")

# A `#[link(name = "...", kind = "raw-dylib", ...)]` attribute, possibly wrapped in
# `#[cfg_attr(<pred>, link(...))]`, possibly spanning lines.
#
# Scanned for balanced parentheses rather than matched with a single regex: the attribute body
# contains nested brackets and commas, which is the same reason `wp81_std_patch.py` refuses to
# regex its declarations.
LINK_MARKERS = ("#[link(", "#[cfg_attr(")


def _matching_paren(text, i):
    """`text[i]` must be '('. Returns the index just past the matching ')', or -1."""
    if i >= len(text) or text[i] != "(":
        return -1
    depth = 0
    while i < len(text):
        c = text[i]
        if c == "(":
            depth += 1
        elif c == ")":
            depth -= 1
            if depth == 0:
                return i + 1
        i += 1
    return -1


def _strip_one_attr(text):
    """Remove the first `raw-dylib` link attribute. Returns (new_text, removed)."""
    i = 0
    while True:
        # Find the next attribute start, whichever marker comes first.
        cands = [text.find(m, i) for m in LINK_MARKERS]
        cands = [c for c in cands if c >= 0]
        if not cands:
            return text, False
        start = min(cands)

        # Walk the attribute's balanced group(s). `#[link(...)]` is one group; `#[cfg_attr(pred,
        # link(...))]` is one group containing another.
        open_paren = text.find("(", start)
        end = _matching_paren(text, open_paren)
        if end < 0:
            i = start + 1
            continue
        # Include the trailing `]` of the attribute.
        close = text.find("]", end)
        if close < 0:
            i = start + 1
            continue
        body = text[start:close + 1]

        if "raw-dylib" not in body:
            i = close + 1
            continue

        # Consume the rest of the line's leading whitespace and the trailing newline so no blank
        # line is left behind.
        end = close + 1
        while end < len(text) and text[end] in " \t":
            end += 1
        if end < len(text) and text[end] == "\r":
            end += 1
        if end < len(text) and text[end] == "\n":
            end += 1

        return text[:start] + text[end:], True


def strip_all_attrs(text):
    """Remove every `raw-dylib` link attribute in `text`. Returns (new_text, count)."""
    n = 0
    while True:
        text, removed = _strip_one_attr(text)
        if not removed:
            return text, n
        n += 1


def targets():
    """Every .rs file under a targeted registry crate.

    The layout is `registry/src/<index-host-hash>/<crate>-<version>/`, so the crate directories
    are one level below REGISTRY_SRC - not immediate children of it. Missing that indirection is
    why the first version of this script reported "no targeted crates" against a registry that
    plainly contained `which-8.0.2`.
    """
    if not os.path.isdir(REGISTRY_SRC):
        return []
    out = []
    # Walk two levels: registry/src itself (in case a vendored layout is used) and each
    # index-host subdirectory.
    roots = [REGISTRY_SRC]
    for entry in sorted(os.listdir(REGISTRY_SRC)):
        sub = os.path.join(REGISTRY_SRC, entry)
        if os.path.isdir(sub):
            roots.append(sub)
    for base in roots:
        for entry in sorted(os.listdir(base)):
            if not entry.startswith(TARGET_CRATES):
                continue
            root = os.path.join(base, entry)
            if not os.path.isdir(root):
                continue
            for dirpath, _dirs, files in os.walk(root):
                for f in files:
                    if f.endswith(".rs"):
                        out.append(os.path.join(dirpath, f))
    return out


def main(argv):
    ap = argparse.ArgumentParser()
    ap.add_argument("--check", action="store_true")
    ap.add_argument("--apply", action="store_true")
    args = ap.parse_args(argv[1:])

    files = targets()
    if not files:
        print("no targeted crates under " + REGISTRY_SRC, file=sys.stderr)
        print("(expected which-* and getrandom-*; is CARGO_HOME set?)", file=sys.stderr)
        return 1

    total = 0
    touched = []
    for p in files:
        try:
            with open(p, "r", encoding="utf-8") as fh:
                text = fh.read()
        except (UnicodeDecodeError, OSError):
            continue
        if "raw-dylib" not in text:
            continue
        new_text, n = strip_all_attrs(text)
        if n == 0:
            continue
        rel = os.path.relpath(p, REGISTRY_SRC)
        print("%-58s %d raw-dylib attr(s)" % (rel, n))
        touched.append(rel)
        total += n
        if args.apply:
            try:
                os.chmod(p, 0o666)
            except OSError:
                pass
            with open(p, "w", encoding="utf-8", newline="") as fh:
                fh.write(new_text)

    print("\n%d file(s), %d raw-dylib attribute(s) %s"
          % (len(touched), total, "stripped" if args.apply else "would be stripped"))

    if args.check and total == 0:
        print("\nNOTE: nothing to strip. Either the patch is already applied, or the crates' "
              "attribute text changed shape and this script no longer recognises it - which "
              "would put kernel32.dll back in the import table. Verify with check_imports.py "
              "after the next link.", file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
