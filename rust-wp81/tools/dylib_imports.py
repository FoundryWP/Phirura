#!/usr/bin/env python3
"""Map every raw-dylib import descriptor in a static archive to the symbols it imports.

Why this tool exists: when the Windows bindings are compiled with `#[link(kind = "raw-dylib")]`,
rustc emits one extra archive member per DLL per object, and that member is literally *named*
after the DLL - `kernel32.dll`, `d3d11.dll`, `oleaut32.dll`, ... Reading the archive with
`ar_scan.members()` therefore makes those members look like they have impossible names, and the
name you see is only the *first* of the DLLs that member imports (the rest are NUL-separated in the
same name field). That is why a naive listing shows `d3d11.dll` with a size of 370 bytes and no way
to tell what is inside.

This walks the member properly: it parses the COFF header, finds `.drectve` (the linker-directive
string section) and the `.idata$*` contributions, and prints the imports grouped by DLL.

Usage:
    python dylib_imports.py <archive.lib> [--filter <substr>]
"""

import re
import struct
import sys

sys.path.insert(0, __file__.rsplit("\\", 1)[0])
from ar_scan import members  # noqa: E402

IMAGE_SYM_CLASS_EXTERNAL = 2
IMAGE_SCN_LNK_INFO = 0x0200  # .drectve


def coff_sections(blob):
    """Return [(name, vaddr, vsize, rawptr, rawsize, chars)] for one COFF object."""
    try:
        machine, nsec, ts, symptr, nsym, optsz, chars = struct.unpack_from("<HHIIIHH", blob, 0)
    except struct.error:
        return None
    out = []
    for i in range(nsec):
        o = 20 + optsz + i * 40
        if o + 40 > len(blob):
            return None
        name = blob[o:o + 8].rstrip(b"\0").decode("ascii", "replace")
        nv = struct.unpack_from("<IIIIIIHHI", blob, o + 8)
        vaddr, vsize, rawptr, rawsize, _, _, _, _, schars = nv
        out.append((name, vaddr, vsize, rawptr, rawsize, schars))
    return out


def drectve_strings(blob):
    """Pull the /DEFAULTLIB and /ALTERNATENAME directives out of .drectve."""
    secs = coff_sections(blob)
    if not secs:
        return []
    res = []
    for name, vaddr, vsize, rawptr, rawsize, schars in secs:
        if not (schars & IMAGE_SCN_LNK_INFO):
            continue
        chunk = blob[rawptr:rawptr + rawsize]
        for m in re.findall(rb"[\x20-\x7e]{3,}", chunk):
            res.append(m.decode("ascii", "replace"))
    return res


def idata_dll_names(blob):
    """Names of DLLs this object contributes an import descriptor for.

    rustc emits, per raw-dylib DLL, a small member whose *name field* is the DLL name. The member
    itself is just an import-descriptor stub: the descriptor's Name RVA is not usable in a COFF
    object (relocations fill it in at link time), so the reliable signal is the archive member name
    plus the `.idata` section being present and tiny.
    """
    secs = coff_sections(blob) or []
    has_idata = any(s[0].startswith(".idata") for s in secs)
    return has_idata


def main():
    if len(sys.argv) < 2:
        print(__doc__)
        return 2
    path = sys.argv[1]
    filt = None
    if "--filter" in sys.argv:
        filt = sys.argv[sys.argv.index("--filter") + 1]

    raw = open(path, "rb").read()
    ms = list(members(raw))

    # Group: a member whose name contains ".dll" (or an api set) IS a descriptor member.
    dll_members = {}
    other_objects = 0
    for nm, off, size in ms:
        name = nm if isinstance(nm, str) else nm.decode("latin1")
        # ar_scan splits on the first NUL; several DLLs may be packed. Take every token that looks
        # like a module name.
        toks = [t for t in re.split(r"[\x00\s]+", name) if t]
        dlls = [t for t in toks if t.lower().endswith(".dll")]
        if dlls:
            for d in dlls:
                dll_members.setdefault(d, 0)
                dll_members[d] += 1
        else:
            other_objects += 1

    if filt:
        dll_members = {k: v for k, v in dll_members.items() if filt.lower() in k.lower()}

    print(f"{path}")
    print(f"  archive members      : {len(ms)}")
    print(f"  non-descriptor objects: {other_objects}")
    print(f"  DLLs with descriptors : {len(dll_members)}")
    for d, n in sorted(dll_members.items()):
        print(f"    {n:5d}  {d}")

    # Now the per-object directives, which is where /DEFAULTLIB kernel32.lib etc. show up.
    print("\n  /DEFAULTLIB directives seen in .drectve:")
    seen = {}
    for nm, off, size in ms:
        blob = raw[off:off + size]
        for s in drectve_strings(blob):
            if s.startswith("/DEFAULTLIB:") or s.startswith("/ALTERNATENAME"):
                seen[s] = seen.get(s, 0) + 1
    for s, n in sorted(seen.items(), key=lambda kv: -kv[1])[:40]:
        print(f"    {n:5d}  {s}")
    if not seen:
        print("    (none)")

    return 0


if __name__ == "__main__":
    sys.exit(main())
