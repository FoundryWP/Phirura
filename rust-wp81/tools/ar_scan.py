#!/usr/bin/env python3
"""Scan an `ar` archive (MSVC `.lib`) for COFF `.drectve` /DEFAULTLIB directives and
undefined externals.

The question this answers, which `dumpbin` cannot be trusted to answer here (it emits nothing
under the sandbox): **which object in a `.lib` is dragging `kernel32.lib` / `ndll.lib` /
`bcryptprimitives.dll` into the link, and which symbol does it want?**

    python ar_scan.py <lib> --defaultlib
    python ar_scan.py <lib> --undefined <symbol>
"""

import argparse
import re
import struct
import sys

# Reuse the COFF reader rather than re-deriving the section-header layout a third time.
sys.path.insert(0, __file__.rsplit("\\", 1)[0])
from coff_dump import Coff  # noqa: E402

AR_GLOBAL = b"!<arch>\n"


def members(data):
    """Yield (name, offset, size) for each archive member.

    Handles both the long-name table (`//`) and the GNU-style name table (`/`), because MSVC
    archives of Rust rlibs use the former for members with long names.
    """
    off = len(AR_GLOBAL)
    longnames = b""
    while off + 60 <= len(data):
        hdr = data[off:off + 60]
        if hdr[58:60] != b"`\n":
            break
        raw_name = hdr[0:16].decode("ascii", "replace").strip()
        try:
            size = int(hdr[48:58].decode("ascii", "replace").strip() or "0")
        except ValueError:
            break
        body = off + 60
        if raw_name == "//":
            longnames = data[body:body + size]
            yield ("//", body, size)
        elif raw_name.startswith("/") and raw_name[1:].isdigit() and raw_name != "/":
            # Offset into the long-name table.
            start = int(raw_name[1:])
            end = longnames.find(b"/\n", start)
            name = longnames[start:end].decode("ascii", "replace")
            yield (name, body, size)
        else:
            yield (raw_name.rstrip("/"), body, size)
        off = body + size + (size % 2)


def scan(path, want_defaultlib=False, want_undefined=None, want_dll=None):
    data = open(path, "rb").read()
    for name, off, size in members(data):
        blob = data[off:off + size]
        if blob[:4] != b"\x00\x00\xff\xff" and not blob[:2] in (b"\x64\x86", b"\x4c\x01"):
            # Not a COFF object (e.g. the name table or an rmeta member).
            if want_dll:
                for m in re.findall(rb"[A-Za-z0-9_\-]{3,40}\.dll", blob):
                    print("  %-40s dll-string %s" % (name[:40], m.decode()))
            continue
        try:
            c = Coff.__new__(Coff)
            Coff.__init__(c, path) if False else None
        except Exception:
            pass
        # Parse this member in place rather than re-reading the file per member.
        try:
            (machine, nsec, ts, symptr, nsym, optsz, chars) = struct.unpack_from("<HHIIIHH", blob, 0)
        except struct.error:
            continue
        shoff = 20 + optsz
        # .drectve is a section; pull its raw bytes.
        for i in range(nsec):
            o = shoff + i * 40
            if o + 40 > len(blob):
                break
            sname = blob[o:o + 8].rstrip(b"\0").decode("ascii", "replace")
            fields = struct.unpack_from("<IIIIIIHHI", blob, o + 8)
            raw_size, raw_ptr = fields[2], fields[3]
            if sname == ".drectve" and raw_size:
                txt = blob[raw_ptr:raw_ptr + raw_size].decode("ascii", "replace")
                if want_defaultlib:
                    for m in re.findall(r"/DEFAULTLIB:[^ ]+", txt):
                        print("  %-48s %s" % (name[:48], m))
                if want_dll:
                    for m in re.findall(r"[A-Za-z0-9_\-]{3,40}\.dll", txt):
                        print("  %-48s dll %s" % (name[:48], m))
        if want_undefined:
            strtab = symptr + nsym * 18
            j = 0
            while j < nsym:
                o = symptr + j * 18
                if o + 18 > len(blob):
                    break
                n8, val, sec, typ, cls, naux = struct.unpack_from("<8sIhHBB", blob, o)
                if n8[:4] == b"\0\0\0\0":
                    so = struct.unpack_from("<I", n8, 4)[0]
                    base = strtab + so
                    e = blob.find(b"\0", base)
                    nm = blob[base:e].decode("ascii", "replace")
                else:
                    nm = n8.rstrip(b"\0").decode("ascii", "replace")
                if cls == 2 and nm == want_undefined:
                    print("  %-48s defines?? no - references %s (sec=%d)" % (name[:48], nm, sec))
                j += 1 + naux


def main(argv):
    ap = argparse.ArgumentParser()
    ap.add_argument("lib")
    ap.add_argument("--defaultlib", action="store_true")
    ap.add_argument("--dll", action="store_true")
    ap.add_argument("--undefined", default=None)
    ap.add_argument("--list", action="store_true")
    args = ap.parse_args(argv[1:])

    if args.list:
        data = open(args.lib, "rb").read()
        for name, off, size in members(data):
            print("%-60s %d" % (name, size))
        return 0
    scan(args.lib, args.defaultlib, args.undefined, args.dll)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
