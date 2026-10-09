#!/usr/bin/env python3
"""Verify the kernel32 -> API-set mapping in `wp81_std_patch.py` against the real loader.

Why this exists
---------------
`KERNEL32_REMAP` is a hand-written table of ~143 entries saying which WP8.1 API set exports
each Win32 function. Get one wrong and the DLL still links cleanly, but the device refuses to
load it:

    ERROR_PROC_NOT_FOUND (0x8007007F): "The specified procedure could not be found"

The error names no symbol, so guessing is hopeless. This asks Windows directly.

How it works
------------
The api-ms-win-* DLLs are resolved by the loader from the API set schema, so `ctypes.WinDLL`
can open them by name and `getattr` probes an export. **A miss is a definite bug** in the
table.

Caveats, both learned the hard way:

* A **hit is not proof**. Desktop Windows may export more than WP8.1 does, so this cannot
  prove a mapping correct - only find the ones that are definitely wrong.
* Do **not** use `System32\\downlevel\\api-ms-win-*.dll` instead. Those are partial forwarder
  stubs (`api-ms-win-core-file-l1-2-1.dll` has three exports), so they report misses that are
  not real.

Usage:
    python verify_mapping.py            # report every entry that is wrong
    python verify_mapping.py --fix      # rewrite wp81_std_patch.py where a better set is known
"""

import argparse
import ctypes
import importlib.util
import os
import re
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
PATCHER = os.path.join(HERE, "wp81_std_patch.py")


def load_patcher():
    spec = importlib.util.spec_from_file_location("wp81_std_patch", PATCHER)
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


def main(argv):
    ap = argparse.ArgumentParser()
    ap.add_argument("--fix", action="store_true",
                    help="rewrite the mapping where a correct API set is known")
    args = ap.parse_args(argv[1:])

    m = load_patcher()

    libs = {}
    for dll in sorted(set(m.API_SETS.values())):
        try:
            libs[dll] = ctypes.WinDLL(dll)
        except OSError:
            libs[dll] = None

    # reverse lookup: API set key -> dll name, so we can name a replacement as a key
    key_of = {v: k for k, v in m.API_SETS.items()}

    ok = 0
    bad = []
    for fn, key in sorted(m.KERNEL32_REMAP.items()):
        dll = m.API_SETS[key]
        lib = libs.get(dll)
        if lib is None:
            continue
        try:
            getattr(lib, fn)
            ok += 1
        except AttributeError:
            real = None
            for d2, l2 in libs.items():
                if l2 is None:
                    continue
                try:
                    getattr(l2, fn)
                    real = d2
                    break
                except AttributeError:
                    pass
            bad.append((fn, key, real))

    print("mapping entries checked : %d" % (ok + len(bad)))
    print("correct                 : %d" % ok)
    print("WRONG                   : %d" % len(bad))
    print()
    for fn, key, real in bad:
        if real:
            print("  %-32s %-20s SHOULD BE %s" % (fn, key, key_of.get(real, real)))
        else:
            print("  %-32s %-20s in NO API set (must be stubbed in compat)" % (fn, key))

    if not args.fix or not bad:
        return 1 if bad else 0

    text = open(PATCHER, encoding="utf-8").read()
    applied = 0
    for fn, key, real in bad:
        if not real:
            continue
        new_key = key_of.get(real)
        if not new_key:
            continue
        pattern = re.compile(r'("%s":\s*)"%s"' % (re.escape(fn), re.escape(key)))
        text, n = pattern.subn(lambda mo: mo.group(1) + '"%s"' % new_key, text, count=1)
        if n:
            applied += 1
            print("fixed %-32s %s -> %s" % (fn, key, new_key))
    open(PATCHER, "w", encoding="utf-8").write(text)
    print("\n%d entry(ies) rewritten in %s" % (applied, PATCHER))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
