#!/usr/bin/env python3
"""Answer one question per object: is this symbol an *import* (bound to a DLL) or a plain
*undefined external*?

Why it matters here: `wp81prpr.dll` fails to link with
    LNK2005: GetLastError already defined in compat…rcgu.o   (from kernel32.dll)
The difference between "std declares GetLastError as a plain extern" (fine, compat supplies it)
and "std imports GetLastError from kernel32.dll" (collision) is invisible in the source after
patching - both read `unsafe extern "system" { pub fn GetLastError() -> u32; }`. It is only
visible in the object: an import carries a `__imp_GetLastError` symbol and a `.drectve` section
naming the DLL; a plain extern carries just `GetLastError` and has nothing but a `/DEFAULTLIB`.

Usage:
    python probe_imports.py <rlib-or-lib> <symbol> [<symbol> ...]
"""

import re
import struct
import sys

sys.path.insert(0, __file__.rsplit("\\", 1)[0])
from ar_scan import members  # noqa: E402


def object_symbols(blob):
    """Return (defined, undefined, imported) symbol-name sets for one COFF object."""
    try:
        machine, nsec, ts, symptr, nsym, optsz, chars = struct.unpack_from("<HHIIIHH", blob, 0)
    except struct.error:
        return None
    shoff = 20 + optsz
    strtab = symptr + nsym * 18
    defined, undef, imported = set(), set(), set()
    directives = []
    for i in range(nsec):
        o = shoff + i * 40
        if o + 40 > len(blob):
            break
        sname = blob[o:o + 8].rstrip(b"\0").decode("ascii", "replace")
        fields = struct.unpack_from("<IIIIIIHHI", blob, o + 8)
        raw_size, raw_ptr = fields[2], fields[3]
        if sname == ".drectve" and raw_size:
            directives.append(blob[raw_ptr:raw_ptr + raw_size].decode("ascii", "replace"))
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
        if nm:
            if cls == 2:
                if sec == 0:
                    (imported if nm.startswith("__imp_") else undef).add(nm)
                else:
                    defined.add(nm)
        j += 1 + naux
    return defined, undef, imported, directives


def main(argv):
    lib = argv[1]
    wants = set(argv[2:])
    data = open(lib, "rb").read()
    real = [m for m in members(data) if m[0] not in ("//", "/")]
    print("%s: %d object members" % (lib, len(real)))
    hits = {w: {"import": [], "undef": [], "define": []} for w in wants}
    for name, off, size in real:
        blob = data[off:off + size]
        res = object_symbols(blob)
        if not res:
            continue
        defined, undef, imported, directives = res
        short = name.split("/")[-1][:44] if name else "?"
        for w in wants:
            if w in defined:
                hits[w]["define"].append(short)
            if w in undef:
                hits[w]["undef"].append(short)
            if "__imp_" + w in imported:
                dlls = set()
                for d in directives:
                    dlls |= set(re.findall(r"[A-Za-z0-9_\-]+\.dll", d))
                hits[w]["import"].append(short + (" [" + ",".join(sorted(dlls)) + "]" if dlls else ""))
    for w in wants:
        h = hits[w]
        print("\n=== %s ===" % w)
        print("  IMPORTED (__imp_): %d" % len(h["import"]))
        for x in h["import"][:6]:
            print("     " + x)
        print("  plain undefined  : %d" % len(h["undef"]))
        for x in h["undef"][:6]:
            print("     " + x)
        print("  DEFINED          : %d" % len(h["define"]))
        for x in h["define"][:6]:
            print("     " + x)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
