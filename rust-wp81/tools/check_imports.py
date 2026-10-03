#!/usr/bin/env python3
r"""Fail the build if the linked DLL imports a library Windows Phone 8.1 does not have.

Why this is a separate gate rather than a comment
-------------------------------------------------
`build-port.ps1` links with `/FORCE:MULTIPLE`, which is what lets the link succeed at all. The
four LNK2005 it silences are benign - in each case `wp81-compat` defines the symbol and the
`raw-dylib` import descriptor from a third-party crate (`which`, `winapi-util`, `walkdir`,
`same-file`, `getrandom`, `twox-hash`, `rand`) is the copy that gets dropped - and the resulting
image was checked by hand to import only API sets. But that manual check does not survive the
next dependency.

`/FORCE:MULTIPLE` cannot distinguish "compat implements this, drop the import" from "nobody
implements this, link anyway". The second case produces a DLL that links cleanly, loads on the
device, and jumps through a null IAT slot the first time the function is called. A build-time
error is strictly better than a device crash, so the assertion belongs in the build.

What counts as a failure
------------------------
Any imported library that is not an `api-ms-win-*` API set, plus an explicit deny list of
desktop-only names that a WP8.1 process must not import. The API-set test is deliberately
positive (allow-list by shape) rather than "matches the deny list": a new DLL dependency is
worth a human look either way.

Exit status is the build contract:
    0  every imported library is an API set (or reads clean)
    1  at least one imported library is not - the caller must fail the build
    2  the file could not be read / parsed - also a failure, never a silent pass

Usage:
    python check_imports.py <dll>
    python check_imports.py <dll> --allow extra.dll,other.dll
"""

import argparse
import struct
import sys

sys.path.insert(0, __file__.rsplit("\\", 1)[0])
from pe_dump import rva_to_off, read_cstr  # noqa: E402

# Libraries that exist on desktop Windows and must never appear in a WP8.1 image. The API-set
# shape test below would already reject them; listing them names the mistake in the output
# instead of making the reader work out why `kernel32.dll` was rejected.
DENY = {
    "kernel32.dll", "ntdll.dll", "advapi32.dll", "user32.dll", "gdi32.dll",
    "ole32.dll", "oleaut32.dll", "shell32.dll", "shlwapi.dll", "bcrypt.dll",
    "bcryptprimitives.dll", "ws2_32.dll", "userenv.dll", "dbghelp.dll",
    "synchronization.dll", "clfsw32.dll", "ktmw32.dll", "netapi32.dll",
    "txfw32.dll", "wofutil.dll", "version.dll", "api-ms-win-core-ioring-l1-1-0.dll",
}

API_SET_PREFIX = "api-ms-win-"

# Libraries that are NOT named `api-ms-win-*` but ARE on the phone, proven by the WP8.1 SDK: the
# kit ships a matching import lib for each in `Windows Phone Kits\8.1\lib\ARM\`. The GPU DLLs are
# the reason this list exists - a D3D11 renderer in Rust must import `d3d11.dll` / `dxgi.dll`
# directly (there is no API-set spelling for them), and the C++ probe already reached the Adreno
# 225 through them on a real device.
#
# Add to this only with an import lib to point at. "It worked on desktop" is not evidence.
PRESENT_ON_PHONE = {
    "d3d11.dll",           # Windows Phone Kits\8.1\lib\ARM\d3d11.lib
    "dxgi.dll",            # Windows Phone Kits\8.1\lib\ARM\dxgi.lib
    "d3dcompiler_47.dll",  # Windows Phone Kits\8.1\lib\ARM\d3dcompiler.lib
}


def imported_libraries(path):
    d = open(path, "rb").read()
    e_lfanew = struct.unpack_from("<I", d, 0x3C)[0]
    if d[e_lfanew:e_lfanew + 4] != b"PE\0\0":
        raise ValueError("not a PE image")
    _machine, nsec, _ts, _sy, _ns, optsz, _ch = struct.unpack_from("<HHIIIHH", d, e_lfanew + 4)
    opt = e_lfanew + 24
    magic = struct.unpack_from("<H", d, opt)[0]
    is64 = magic == 0x20B
    dd = opt + (112 if is64 else 96)
    sh = opt + optsz
    sections = []
    for i in range(nsec):
        o = sh + i * 40
        nm = d[o:o + 8].rstrip(b"\0").decode("latin-1")
        vs, va, rs, rp = struct.unpack_from("<IIII", d, o + 8)
        sections.append((nm, va, vs, rp, rs))

    rva, size = struct.unpack_from("<II", d, dd + 1 * 8)
    libs = []
    if rva and size:
        off = rva_to_off(sections, rva)
        i = 0
        while True:
            olt, _ts, _fc, name_rva, _ft = struct.unpack_from("<IIIII", d, off + i * 20)
            if olt == 0 and name_rva == 0:
                break
            libs.append(read_cstr(d, rva_to_off(sections, name_rva)))
            i += 1
            if i > 512:
                raise ValueError("import descriptor walk did not terminate")
    return libs


def main(argv):
    ap = argparse.ArgumentParser()
    ap.add_argument("dll")
    ap.add_argument("--allow", default="", help="comma-separated extra library names to permit")
    args = ap.parse_args(argv[1:])

    allow = {s.strip().lower() for s in args.allow.split(",") if s.strip()}
    try:
        libs = imported_libraries(args.dll)
    except Exception as exc:                                  # noqa: BLE001 - report, then fail
        print("CHECK FAILED: could not read %s: %s" % (args.dll, exc), file=sys.stderr)
        return 2

    bad = []
    for lib in libs:
        low = lib.lower()
        if low in allow:
            continue
        if low in PRESENT_ON_PHONE:
            continue
        if low.startswith(API_SET_PREFIX):
            continue
        bad.append(lib)

    print("%s imports %d libraries:" % (args.dll, len(libs)))
    for lib in libs:
        mark = "  OK " if lib not in bad else "  BAD"
        why = ""
        if lib in bad:
            why = "  <- not an API set" + (" (on the WP8.1 deny list)" if lib.lower() in DENY else "")
        elif lib.lower() in PRESENT_ON_PHONE:
            why = "  <- ships on WP8.1 (proven by the kit's ARM import lib)"
        print("   %s %s%s" % (mark, lib, why))

    if bad:
        print("\nCHECK FAILED: %d non-API-set import(s). A WP8.1 process cannot load these;"
              % len(bad), file=sys.stderr)
        print("either compat must implement the missing symbol, or the crate needs a shim.",
              file=sys.stderr)
        for lib in bad:
            print("    " + lib, file=sys.stderr)
        return 1
    print("CHECK OK: every import is an API set or a DLL the WP8.1 kit links against")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
