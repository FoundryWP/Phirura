#!/usr/bin/env python3
r"""Drop tokio's `net` feature (and the `TcpStream` it supplies) from the phira-mp git checkout.

Why a text patch on a git checkout, and not a `[patch]` shim
------------------------------------------------------------
`phira-mp` is the *only* crate in the graph that asks tokio for `net`. tokio 1.50 ships
`default = []`, so that single line is what enables `mio` - and `mio` on Windows is the IOCP
plus AFD path, which links `ws2_32.dll` (16 symbols), `kernel32.dll` (4 IOCP symbols) and
`ntdll.dll` (2 AFD symbols) into the image. A WP8.1 process may import none of them, so
`tools/check_imports.py` fails the build until they are gone. This is a *link-time* cost: no
call is ever made, but the import descriptors are emitted regardless.

Shimming `phira-mp-client`/`phira-mp-common` with `[patch]` would work too, but it means
vendoring two crates whose entire API surface would have to be mirrored to change three lines.
The change here is exactly three edits in the checkout cargo already extracted, and none of
them alters phira's compile surface: `tokio::net` is simply never referenced by phira itself.

Why the edits are safe
----------------------
* `enable_all()` in tokio's `runtime::Builder` gates its own `self.enable_io()` call behind
  `any(feature = "net", ...)`, and `enable_io` lives inside `cfg_io_driver!` with the same
  predicate - so with `net` off the I/O driver is compiled out rather than left half-wired.
* `tokio::fs` (used by `prpr` and `phira/src/images.rs`) is feature `fs = []` and never
  depended on `net`.
* Nothing else in the workspace enables `net`, so removing it here removes it everywhere.

The replacement `TcpStream` lives in `phira-mp-common` and fails every operation. It exists
so the crate still compiles, not so networking works: multiplayer has no transport on this
target, and a stub that returned `Ok` would turn a missing feature into silent data loss.

Usage:
    python wp81_phira_mp_patch.py --check   # report what is still unpatched
    python wp81_phira_mp_patch.py --apply   # rewrite in place
"""

import argparse
import hashlib
import os
import sys

CARGO_HOME = os.environ.get("CARGO_HOME",
                            r"C:\Users\admin\.workbuddy-ai\binaries\rust\cargo")
CHECKOUTS = os.path.join(CARGO_HOME, "git", "checkouts")

# The stub body, inserted immediately before `pub const HEARTBEAT_INTERVAL`. Written as a
# plain string rather than a triple-quoted blob so the line endings can be taken from the
# anchor line - this checkout may be CRLF or LF and a mismatch would rewrite the whole file.
STUB = [
    "/// Stand-in for `tokio::net::TcpStream`, which does not exist here: the `net` feature is\n",
    "/// deliberately off because it is the only thing that pulls `mio`, and `mio` links\n",
    "/// `ws2_32.dll`/`kernel32.dll`/`ntdll.dll` - none of which a WP8.1 process may import.\n",
    "///\n",
    "/// Every operation fails instead of pretending to succeed, so an attempted connection\n",
    "/// surfaces as an ordinary error rather than a null IAT slot on the phone.\n",
    "pub struct TcpStream;\n",
    "\n",
    "/// Read half of [`TcpStream`]. See the type's doc comment for why it cannot read.\n",
    "pub struct TcpStreamRead;\n",
    "\n",
    "/// Write half of [`TcpStream`]. See the type's doc comment for why it cannot write.\n",
    "pub struct TcpStreamWrite;\n",
    "\n",
    "fn no_transport() -> std::io::Error {\n",
    "    std::io::Error::new(\n",
    "        std::io::ErrorKind::Unsupported,\n",
    "        \"no network transport on this target\",\n",
    "    )\n",
    "}\n",
    "\n",
    "impl TcpStream {\n",
    "    pub fn set_nodelay(&self, _nodelay: bool) -> std::io::Result<()> {\n",
    "        Err(no_transport())\n",
    "    }\n",
    "\n",
    "    pub fn into_split(self) -> (TcpStreamRead, TcpStreamWrite) {\n",
    "        (TcpStreamRead, TcpStreamWrite)\n",
    "    }\n",
    "\n",
    "    pub async fn connect(_addr: (String, u16)) -> std::io::Result<Self> {\n",
    "        Err(no_transport())\n",
    "    }\n",
    "}\n",
    "\n",
    "impl tokio::io::AsyncRead for TcpStreamRead {\n",
    "    fn poll_read(\n",
    "        self: std::pin::Pin<&mut Self>,\n",
    "        _cx: &mut std::task::Context<'_>,\n",
    "        _buf: &mut tokio::io::ReadBuf<'_>,\n",
    "    ) -> std::task::Poll<std::io::Result<()>> {\n",
    "        std::task::Poll::Ready(Err(no_transport()))\n",
    "    }\n",
    "}\n",
    "\n",
    "impl tokio::io::AsyncWrite for TcpStreamWrite {\n",
    "    fn poll_write(\n",
    "        self: std::pin::Pin<&mut Self>,\n",
    "        _cx: &mut std::task::Context<'_>,\n",
    "        _buf: &[u8],\n",
    "    ) -> std::task::Poll<std::io::Result<usize>> {\n",
    "        std::task::Poll::Ready(Err(no_transport()))\n",
    "    }\n",
    "\n",
    "    fn poll_flush(\n",
    "        self: std::pin::Pin<&mut Self>,\n",
    "        _cx: &mut std::task::Context<'_>,\n",
    "    ) -> std::task::Poll<std::io::Result<()>> {\n",
    "        std::task::Poll::Ready(Ok(()))\n",
    "    }\n",
    "\n",
    "    fn poll_shutdown(\n",
    "        self: std::pin::Pin<&mut Self>,\n",
    "        _cx: &mut std::task::Context<'_>,\n",
    "    ) -> std::task::Poll<std::io::Result<()>> {\n",
    "        std::task::Poll::Ready(Ok(()))\n",
    "    }\n",
    "}\n",
]

# Markers that say "already applied". Each is unique to the patched shape, so a re-run finds
# them and reports 0 changes rather than applying the edit twice.
MARK_NET = '"net",'
MARK_IMPORT = "net::TcpStream,"
MARK_STUB = "pub struct TcpStream;"


def find_checkout():
    """The phira-mp checkout directory, or None if cargo has not fetched it yet.

    Layout is `git/checkouts/<name>-<hash>/<short-rev>/`; the short rev is fixed for a pinned
    revision, but globbing keeps the script working if the pin moves.
    """
    if not os.path.isdir(CHECKOUTS):
        return None
    cands = [d for d in os.listdir(CHECKOUTS)
             if d.startswith("phira-mp-") and os.path.isdir(os.path.join(CHECKOUTS, d))]
    if len(cands) != 1:
        return None
    inner = os.path.join(CHECKOUTS, cands[0])
    revs = [d for d in os.listdir(inner) if os.path.isdir(os.path.join(inner, d))]
    if len(revs) != 1:
        return None
    return os.path.join(inner, revs[0])


def read_lines(path):
    # newline='' keeps the original terminators, so a CRLF checkout is not rewritten to LF.
    with open(path, "r", encoding="utf-8", newline="") as fh:
        return fh.read().splitlines(keepends=True)


def eol_of(lines, default="\n"):
    for ln in lines:
        if ln.endswith("\r\n"):
            return "\r\n"
        if ln.endswith("\n"):
            return "\n"
    return default


def write_lines(path, lines):
    with open(path, "w", encoding="utf-8", newline="") as fh:
        fh.write("".join(lines))


def patch_root_toml(path, out):
    """Remove `"net"` from the workspace tokio feature list.

    The scan is bounded by the `tokio = { ... }` table so a future unrelated `"net"` elsewhere
    in the file is not touched, and "no such line inside the table" is the already-applied
    case rather than an error - that is exactly what a re-run sees.
    """
    lines = read_lines(path)
    start = next((i for i, ln in enumerate(lines) if ln.startswith("tokio = {")), None)
    if start is None:
        return False, "ERROR: tokio = { ... } not found"
    end = next((i for i in range(start, len(lines)) if lines[i].strip().startswith("]")), None)
    if end is None:
        return False, "ERROR: tokio feature array not closed"
    drop = [i for i in range(start, end + 1) if lines[i].strip() == MARK_NET]
    if not drop:
        return False, "already patched (no \"net\" feature in the tokio table)"
    if len(drop) != 1:
        return False, "ERROR: %d \"net\" lines inside the tokio table" % len(drop)
    out.extend(lines[:drop[0]])
    out.extend(lines[drop[0] + 1:])
    return True, "dropped tokio feature \"net\" (line %d)" % (drop[0] + 1)


def drop_line(path, out, needle, label):
    lines = read_lines(path)
    hits = [i for i, ln in enumerate(lines) if ln.strip() == needle]
    if not hits:
        return False, "already patched (%s)" % label
    if len(hits) != 1:
        return False, "ERROR: %d occurrences of %s" % (len(hits), needle)
    out.extend(lines[:hits[0]])
    out.extend(lines[hits[0] + 1:])
    return True, "removed %s (line %d)" % (label, hits[0] + 1)


def patch_common_use(path, out):
    return drop_line(path, out, MARK_IMPORT, "`net::TcpStream` import")


def patch_client_use(path, out):
    lines = read_lines(path)
    if any(ln.strip() == "TcpStream," for ln in lines):
        return False, "already patched (`TcpStream` import)"
    hits = [i for i, ln in enumerate(lines) if ln.strip() == MARK_IMPORT]
    if not hits:
        return False, "already patched (`net::TcpStream` import)"
    if len(hits) != 1:
        return False, "ERROR: %d `net::TcpStream,` lines" % len(hits)
    # The stub type lives in phira-mp-common, which is already imported just above; add it to
    # that list rather than opening a second `use`, so the two imports cannot drift apart.
    marker = "use phira_mp_common::{"
    at = next((i for i, ln in enumerate(lines) if ln.strip() == marker), None)
    if at is None:
        return False, "ERROR: `use phira_mp_common::{` not found"
    eol = eol_of(lines)
    new = []
    for i, ln in enumerate(lines):
        if i == hits[0]:
            continue
        new.append(ln)
        if i == at:
            new.append("    TcpStream," + eol)
    out.extend(new)
    return True, "imported stub `TcpStream` from phira_mp_common (dropped line %d)" % (hits[0] + 1)


def patch_common_stub(path, out):
    lines = read_lines(path)
    if any(MARK_STUB in ln for ln in lines):
        return False, "already patched (TcpStream stub)"
    marker = "pub const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(3);"
    at = next((i for i, ln in enumerate(lines) if ln.strip() == marker), None)
    if at is None:
        return False, "ERROR: anchor `pub const HEARTBEAT_INTERVAL` not found"
    eol = eol_of(lines)
    out.extend(lines[:at])
    # STUB entries are written with plain "\n"; strip that and take the checkout's own
    # terminator, otherwise a CRLF file would end up with "\n\r\n" on every inserted line.
    out.extend(s.rstrip("\r\n") + eol for s in STUB)
    out.append(eol)
    out.extend(lines[at:])
    return True, "inserted TcpStream stub (%d lines)" % len(STUB)


def plan(root):
    """[(label, path, fn)] for every edit, in application order."""
    return [
        ("phira-mp/Cargo.toml (tokio features)",
         os.path.join(root, "Cargo.toml"), patch_root_toml),
        ("phira-mp-common/src/lib.rs (drop tokio::net import)",
         os.path.join(root, "phira-mp-common", "src", "lib.rs"), patch_common_use),
        ("phira-mp-common/src/lib.rs (TcpStream stub)",
         os.path.join(root, "phira-mp-common", "src", "lib.rs"), patch_common_stub),
        ("phira-mp-client/src/lib.rs (import stub instead of tokio::net)",
         os.path.join(root, "phira-mp-client", "src", "lib.rs"), patch_client_use),
    ]


def content_hash(root):
    """Hash of the four files cargo will actually compile, on disk, right now.

    Deliberately *not* "did this run change anything": a patch applied by hand, or by a previous
    version of this script, must produce the same answer as one applied here, or the build would
    skip the clean and link against whatever was there before it.
    """
    h = hashlib.sha256()
    for _label, path, _fn in plan(root):
        h.update(os.path.relpath(path, root).encode("utf-8"))
        h.update(b"\0")
        try:
            with open(path, "rb") as fh:
                h.update(fh.read())
        except OSError:
            h.update(b"<missing>")
        h.update(b"\0")
    return h.hexdigest()


def main(argv):
    ap = argparse.ArgumentParser()
    ap.add_argument("--check", action="store_true")
    ap.add_argument("--apply", action="store_true")
    args = ap.parse_args(argv[1:])

    root = find_checkout()
    if root is None:
        print("phira-mp checkout not found under " + CHECKOUTS, file=sys.stderr)
        print("(cargo fetches it on the first build; re-run this script afterwards)",
              file=sys.stderr)
        # Not an error on a cold tree - but the build that follows WILL be unpatched, so the
        # import gate is what has to catch it. See tools/check_imports.py.
        return 0

    print("phira-mp checkout: " + root)
    pending = 0
    for label, path, fn in plan(root):
        if not os.path.isfile(path):
            print("  MISSING  %-52s %s" % (label, path))
            pending += 1
            continue
        out = []
        changed, why = fn(path, out)
        if changed:
            pending += 1
            print("  %-9s%-52s %s" % ("PATCH" if args.apply else "PENDING", label, why))
            if args.apply:
                write_lines(path, out)
        else:
            print("  %-9s%-52s %s" % ("OK", label, why))

    rc = 0
    if pending and args.apply:
        print("applied %d edit(s)" % pending)
    elif pending:
        print("%d edit(s) pending - run with --apply" % pending, file=sys.stderr)
        rc = 1
    else:
        print("phira-mp checkout already patched")

    # The same two facts build-port.ps1 takes from wp81_registry_patch.py, for the same reason.
    # cargo identifies a git dependency by revision, not by the bytes of the working tree it
    # extracted, so an edit to the stub above is not enough on its own to make the next build
    # re-read it - the hash tells the build script when that has happened, and the clean list is
    # what forces cargo to notice. `phira` itself is not in the list: it calls functions from
    # these crates rather than expanding a macro of theirs, so its own units carry nothing stale.
    print("PHIRA_MP_HASH=" + content_hash(root))
    print("PHIRA_MP_CLEAN=phira-mp-client,phira-mp-common")
    return rc


if __name__ == "__main__":
    sys.exit(main(sys.argv))
