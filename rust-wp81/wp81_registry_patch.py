#!/usr/bin/env python3
r"""Textual patches applied to third-party crates in the cargo registry.

  1. Strip `raw-dylib` link attributes (which/getrandom) - see below.
  2. Give `windows-sys` its missing `target_arch = "arm"` struct definitions.
  3. Textual substitutions (xxhash-rust's NEON cfg) - see `TEXT_REPLACEMENTS`.
  4. Drop `kind = "raw-dylib"` from the binding macros of `windows-link`/`windows-targets`.

All four are the same shape of problem: a dependency is written for a target we are not using,
and no patch point exists between `cargo` and the extracted source. Pass 1 lives at the bottom
of this docstring's original scope; pass 2 is described next to `ARM_CFG_TARGETS`; pass 4 next
to `RAW_DYLIB_CLAUSES`.

Why pass 1 exists, and why it is separate from `wp81_std_patch.py`
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
import hashlib
import os
import re
import sys

CARGO_HOME = os.environ.get("CARGO_HOME",
                            r"C:\Users\admin\.workbuddy-ai\binaries\rust\cargo")
REGISTRY_SRC = os.path.join(CARGO_HOME, "registry", "src")

# crates whose `raw-dylib` binding must be dropped. Matched by directory *prefix* so a patch bump
# (`which-8.0.2` -> `which-8.1.0`) keeps working without editing this file.
TARGET_CRATES = ("which-", "getrandom-")

# crates that need pass 2. Same prefix-matching rationale as TARGET_CRATES.
ARM_CFG_CRATES = ("windows-sys-",)

# Pass 2: (file suffix relative to the crate root, struct name).
#
# windows-sys 0.61.2 emits architecture-split definitions for these three - one under
# `#[cfg(target_arch = "x86")]`, one under
# `#[cfg(any(target_arch = "aarch64", target_arch = "arm64ec", target_arch = "x86_64"))]` - and
# lists no third architecture. `aarch64` is 64-bit ARM; thumbv7a is `target_arch = "arm"`, which
# appears in neither cfg (it appears nowhere in the crate at all), so the type simply does not
# exist for this target and every `link!` declaration taking a pointer to it fails to resolve:
# four errors, at WSAStartup, getservbyname, getservbyport and the delay-load struct. That is an
# omission, not a deliberate restriction - which is why patching is safe rather than papering over
# a design decision.
#
# The 32-bit (`x86`) variant is the one to widen. Windows decides these layouts on `_WIN64` and
# ARM32 does not define it, so the correct layout is the one in the x86 branch: the 5-field
# `WSADATA`, `IMAGE_THUNK_DATA32` in `DELAYLOAD_INFO`. Widening the 64-bit variant would compile
# just as happily and then read the fields at the wrong offsets - a failure that shows up on the
# device, never in the build.
ARM_CFG_TARGETS = (
    ("Networking/WinSock/mod.rs", "WSADATA"),
    ("Networking/WinSock/mod.rs", "SERVENT"),
    ("System/WindowsProgramming/mod.rs", "DELAYLOAD_INFO"),
)

# Pass 3: textual substitutions, as (crate directory prefix, find, replace).
#
# `xxhash-rust`'s NEON path assumes aarch64: `StripeLanes` names
# `core::arch::aarch64::uint8x16_t` with no architecture guard at all, and the intrinsics the path
# then uses (`vld1q_u8!`, `vreinterpretq_u64_u8`, `uint64x2_t`) are gated behind
# `stdarch_arm_neon_intrinsics`, which is *still unstable on 32-bit ARM*. So on `thumbv7a` - which
# does enable `+neon`, because ARMv7-A has it - that path cannot compile, and 43 errors say so.
#
# The crate already ships a complete scalar fallback (`accumulate_512_scalar`,
# `scramble_acc_scalar`, and the dispatcher that selects them), every one of them guarded by
# `not(..., target_feature = "neon", ...)`. Qualifying each `neon` test with
# `not(target_arch = "arm")` therefore selects the scalar path, and the now-undefined `StripeLanes`
# stops being referenced because every user of it is behind one of the SIMD guards.
#
# The substitution is written to be valid in both positions it occurs in: as the whole body of
# `cfg(target_feature = "neon")` and nested inside `not(any(..., target_feature = "neon", ...))`.
# One replacement string handles all 12 sites.
#
# Why not teach the crate arm32 NEON instead? That needs `#![feature(stdarch_arm_neon_intrinsics)]`
# in a registry crate, which ties the build to whatever the rolling nightly currently calls that
# feature - a silent break on someone else's schedule. xxhash here hashes cache keys, where the
# scalar path's only cost is time, and it is not on any path that matters.
TEXT_REPLACEMENTS = (
    (
        "xxhash-rust-",
        'target_feature = "neon"',
        'all(target_feature = "neon", not(target_arch = "arm"))',
    ),
)

# Derived so `targets()` and the substitution loop cannot drift apart.
TEXT_CRATES = tuple(p for p, _f, _r in TEXT_REPLACEMENTS)

# Pass 4: attributes to delete from the Windows binding *macros* of these crates.
#
# `windows-link` (and `windows-targets` under its `windows_raw_dylib` cfg) define `link!` as
# a macro that expands to `#[link(name = $library, kind = "raw-dylib", ...)]`. Every caller -
# `chrono`, `parking_lot_core`, `libloading` and the whole `windows-sys 0.61` surface - is
# therefore bound with raw-dylib, and raw-dylib makes rustc synthesise an *import descriptor*
# into the caller's object no matter what the linker later decides about duplicate symbols.
# That is what puts `__imp_Sleep`, `__imp_QueryDosDeviceW`, `__imp_GetTimeZoneInformationForYear`
# and `__imp_NtCreateFile` in the finished image: `wp81-compat` defines all four as plain
# `#[no_mangle]` functions, but a plain definition never satisfies an `__imp_` reference, so
# `kernel32.dll` and `ntdll.dll` stay on the import table and WP8.1 refuses to load the DLL.
#
# Rust's own `std` does NOT have this problem, because `windows_raw_dylib` is an opt-in cargo
# feature of `library/windows_link` that `-Zbuild-std-features=compiler-builtins-mem` never
# enables - so `std` takes `link_dylib` and emits a plain `#[link(name = "...")]`.
#
# Deleting only the `kind`/`modifiers`/`import_name_type` clauses keeps `#[link(name = $library)]`,
# which is the point: the declaration still tells the linker where the symbol may come from, so
# a symbol `wp81-compat` does not implement resolves from the import library instead of failing
# the link. `check_imports.py` is then the thing that reports it, by name, in the next round -
# a failed link would report nothing but a symbol count.
#
# The clauses are listed longest-coupling first only by accident of ordering: each is removed
# independently and none of them appears in the replacement, so the pass is idempotent by
# construction - a re-run finds 0 occurrences.
RAW_DYLIB_CRATES = ("windows-link-", "windows-targets-")
RAW_DYLIB_CLAUSES = (
    ', kind = "raw-dylib"',
    ', modifiers = "+verbatim"',
    ', import_name_type = "undecorated"',
)

ARM_CFG_OLD = '#[cfg(target_arch = "x86")]'
ARM_CFG_NEW = '#[cfg(any(target_arch = "x86", target_arch = "arm"))]'

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


def widen_arm_cfgs(text, name):
    """Add `arm` to the x86 arch cfg guarding `name`. Returns (text, count).

    The cfg is not necessarily adjacent: for a struct it sits above `#[repr(C)]` and
    `#[derive(Clone, Copy)]`, for the `impl Default` it is directly above. So rather than matching
    a fixed shape, walk back from the item and accept the nearest x86-only cfg provided nothing
    but attributes lies between them - a real gap (statements, another item's body) means the cfg
    belongs to something else and widening it would change code we did not intend to touch.
    """
    n = 0
    for needle in ("pub struct %s {" % name, "impl Default for %s {" % name):
        start = 0
        while True:
            i = text.find(needle, start)
            if i < 0:
                break
            start = i + len(needle)
            base = max(0, i - 300)
            m = text[base:i].rfind(ARM_CFG_OLD)
            if m < 0:
                continue
            abs_m = base + m
            gap = text[abs_m + len(ARM_CFG_OLD):i]
            # A second cfg in the gap means this item is already guarded by a different predicate,
            # so the one we found belongs to its predecessor.
            if "#[cfg(" in gap:
                continue
            if gap and not re.fullmatch(r'[\s#\[\]()",\w]*', gap):
                continue
            text = text[:abs_m] + ARM_CFG_NEW + text[abs_m + len(ARM_CFG_OLD):]
            n += 1
    return text, n


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
            if not entry.startswith(TARGET_CRATES + ARM_CFG_CRATES + TEXT_CRATES
                                     + RAW_DYLIB_CRATES):
                continue
            root = os.path.join(base, entry)
            if not os.path.isdir(root):
                continue
            for dirpath, _dirs, files in os.walk(root):
                for f in files:
                    if f.endswith(".rs"):
                        out.append(os.path.join(dirpath, f))
    return out


# Extra packages that must be cleaned alongside `windows-link`/`windows-targets`.
#
# Pass 4 patches a *macro*, and a macro's expansion is baked into whichever crate called it. The
# wrong expansion therefore lives in chrono (which calls `link!` for
# `GetTimeZoneInformationForYear`), in parking_lot_core (`Sleep`) and in the `windows-sys` users
# (`CloseHandle`, `GetModuleHandleA`, `MoveFileExW`, `SetFileAttributesW`,
# `GetFileInformationByHandle`) - cleaning the macro crate alone rewrites a file nothing links.
DEPENDENT_PACKAGES = ("chrono", "libloading", "parking_lot_core", "windows-sys")


def package_name(dirname):
    """`windows-link-0.2.1` -> `windows-link`: the `-p` spec `cargo clean` accepts.

    The regex is anchored at the end so only a trailing version suffix is removed; `re.sub`
    scans left to right, so `xxhash-rust-0.8.17` loses `-0.8.17` and not `-rust`.
    """
    return re.sub(r"-\d[\w.\-+]*$", "", dirname)


def target_packages():
    """Every cargo package this script can modify, plus the ones that embed its macros.

    Stable across runs - it depends on which crates are *targeted*, not on whether this run
    happened to change anything - because `build-port.ps1` compares a content hash against the
    one from the last build to decide whether to run it at all.
    """
    pkgs = set()
    macro_patched = False
    for entry in sorted(os.listdir(REGISTRY_SRC)):
        base = os.path.join(REGISTRY_SRC, entry)
        if not os.path.isdir(base):
            continue
        for name in sorted(os.listdir(base)):
            if not name.startswith(TARGET_CRATES + ARM_CFG_CRATES + TEXT_CRATES
                                   + RAW_DYLIB_CRATES):
                continue
            if not os.path.isdir(os.path.join(base, name)):
                continue
            pkgs.add(package_name(name))
            if name.startswith(RAW_DYLIB_CRATES):
                macro_patched = True
    if macro_patched:
        pkgs.update(DEPENDENT_PACKAGES)
    return sorted(pkgs)


def main(argv):
    ap = argparse.ArgumentParser()
    ap.add_argument("--check", action="store_true")
    ap.add_argument("--apply", action="store_true")
    args = ap.parse_args(argv[1:])

    files = targets()
    if not files:
        print("no targeted crates under " + REGISTRY_SRC, file=sys.stderr)
        print("(expected which-*, getrandom-*, windows-sys-* or xxhash-rust-*; is CARGO_HOME set?)", file=sys.stderr)
        return 1

    total = 0
    arm_total = 0
    text_total = 0
    macro_total = 0
    touched = []
    # Content of every target file *after* this script is done with it. See the note next to the
    # marker lines at the end of `main` for why this - and not "did this run change anything" -
    # is what decides whether cargo has to be told to rebuild.
    sha = hashlib.sha256()
    for p in files:
        try:
            with open(p, "r", encoding="utf-8") as fh:
                text = fh.read()
        except (UnicodeDecodeError, OSError):
            continue

        rel = os.path.relpath(p, REGISTRY_SRC)
        new_text = text
        segs = re.split(r"[/\\]", p)

        # Pass 1 - raw-dylib. Scoped to `which-`/`getrandom-` by crate prefix rather than by
        # "the file mentions raw-dylib": pass 4 below targets crates whose raw-dylib lives
        # inside a macro, and letting pass 1 reach those would delete the whole `#[link(..)]`
        # attribute instead of just its `kind`, turning every binding into an unresolved
        # extern. Which of the two a crate needs is a property of the crate, not of the text.
        n = 0
        if any(s.startswith(TARGET_CRATES) for s in segs) and "raw-dylib" in new_text:
            new_text, n = strip_all_attrs(new_text)

        # Pass 2 - arch cfg. Indexed off the normalized path rather than off the crate prefix so it
        # can never fire for `which-`/`getrandom-`, which have no such structs. No `break` on
        # success: WinSock/mod.rs carries two of these structs (WSADATA and SERVENT) and stopping
        # at the first leaves the second unresolved.
        m = 0
        norm = p.replace("\\", "/")
        for suffix, name in ARM_CFG_TARGETS:
            if norm.endswith(suffix):
                new_text, added = widen_arm_cfgs(new_text, name)
                m += added

        # Pass 3 - textual substitutions. Off the crate directory prefix, so it can only ever fire
        # in the crates listed in TEXT_REPLACEMENTS, and only in `.rs` files.
        t = 0
        if p.endswith(".rs"):
            for prefix, find, repl in TEXT_REPLACEMENTS:
                if not any(s.startswith(prefix) for s in segs):
                    continue
                # The replacement necessarily still contains `find` - the cfg test itself has to
                # survive inside it - so a plain str.replace would fire again on its own output and
                # nest one more all(...) per run. What follows `find` inside `repl` is exactly what
                # an already-applied site looks like, so match `find` only where that tail is
                # absent. The guard and the idempotency check are then the same thing: a re-run
                # finds 0 sites and leaves the file byte-identical.
                tail = repl[repl.index(find) + len(find):]
                rx = re.compile(re.escape(find) + "(?!" + re.escape(tail) + ")")
                new_text, added = rx.subn(lambda _m: repl, new_text)
                t += added

        # Pass 4 - raw-dylib inside a binding macro. See RAW_DYLIB_CLAUSES for why the
        # attribute is reduced rather than deleted.
        g = 0
        if p.endswith(".rs") and any(s.startswith(RAW_DYLIB_CRATES) for s in segs):
            for clause in RAW_DYLIB_CLAUSES:
                count = new_text.count(clause)
                if count:
                    new_text = new_text.replace(clause, "")
                    g += count

        # Hashed unconditionally and *before* the early-out below: a file the passes leave alone
        # still has to contribute, or a patch applied by hand since the last build would be
        # invisible here and cargo would never be told to rebuild it.
        sha.update(rel.encode("utf-8"))
        sha.update(b"\0")
        sha.update(new_text.encode("utf-8", "replace"))
        sha.update(b"\0")

        if n == 0 and m == 0 and t == 0 and g == 0:
            continue

        if n:
            print("%-58s %d raw-dylib attr(s)" % (rel, n))
        if m:
            print("%-58s %d arch cfg(s) widened for arm" % (rel, m))
        if t:
            print("%-58s %d substitution(s) for arm" % (rel, t))
        if g:
            print("%-58s %d raw-dylib clause(s) dropped from binding macro" % (rel, g))
        touched.append(rel)
        total += n
        arm_total += m
        text_total += t
        macro_total += g
        if new_text != text and args.apply:
            try:
                os.chmod(p, 0o666)
            except OSError:
                pass
            with open(p, "w", encoding="utf-8", newline="") as fh:
                fh.write(new_text)

    print("\n%d file(s), %d raw-dylib attribute(s) %s"
          % (len(touched), total, "stripped" if args.apply else "would be stripped"))
    if arm_total:
        print("%d arch cfg(s) %s" % (arm_total, "widened for arm" if args.apply else "would be widened for arm"))
    if text_total:
        print("%d substitution(s) %s" % (text_total, "applied" if args.apply else "would be applied"))
    if macro_total:
        print("%d raw-dylib clause(s) %s in binding macros"
              % (macro_total, "dropped" if args.apply else "would be dropped"))

    if args.check and total == 0 and arm_total == 0 and text_total == 0 and macro_total == 0:
        print("\nNOTE: nothing to strip. Either the patch is already applied, or the crates' "
              "attribute text changed shape and this script no longer recognises it - which "
              "would put kernel32.dll back in the import table. Verify with check_imports.py "
              "after the next link.", file=sys.stderr)

    # Machine-readable tail for build-port.ps1, printed unconditionally.
    #
    # `cargo` fingerprints a *registry* dependency by package identity, not by the bytes it was
    # extracted with - which is precisely why this script can rewrite those bytes in place and
    # cargo goes on compiling the old ones. Editing a file is therefore not enough on its own:
    # chrono keeps the macro expansion it built hours earlier, `link.exe` keeps finding
    # `__imp_GetTimeZoneInformationForYear` in a raw-dylib descriptor, and `check_imports.py`
    # keeps naming `kernel32.dll` even though every other symbol it used to name is gone.
    #
    # So the consumer needs two facts: what the registry is *meant* to contain, and what to throw
    # away so cargo notices. The hash covers the post-patch content of every target file rather
    # than "what this run changed", because a patch applied by hand - or by a previous version of
    # this script - changes the hash too, and would otherwise be silently skipped. `build-port.ps1`
    # compares it against the hash recorded by the last successful build and runs
    # `cargo clean -p` on the listed packages when they differ; that clean is the only step that
    # actually breaks the link between the patched source and the compiled crate.
    print("REGISTRY_PATCH_HASH=" + sha.hexdigest())
    print("REGISTRY_PATCH_CLEAN=" + ",".join(target_packages()))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
