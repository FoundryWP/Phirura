#!/usr/bin/env python3
r"""Fail the build if the linked DLL's TLS directory is missing or does not match `.tls`.

Why this is a separate gate
---------------------------
Thread-local storage in a CRT-free image does not work by itself, and it fails *silently*. The
Windows loader only assigns this DLL a TLS index by walking data directory 9 (`IMAGE_DIRECTORY_ENTRY_
TLS`) and writing back through `AddressOfIndex`. `/NODEFAULTLIB` drops the CRT object that normally
defines that descriptor (`tlssup.obj`, a member of `libcmt.lib` on this target), so nothing defines
`_tls_used`, link.exe emits no directory, and `_tls_index` stays at its initial 0.

`#[thread_local]` then resolves as `ThreadLocalStoragePointer[0]` - the *first* module's TLS block,
not this DLL's. Nothing faults. The values are simply somebody else's. On the phone that surfaced as
`GameCreate -> AlreadyRunning` on a brand-new process, with `running=True` and a frame count of
`325577230000753816`, before anything had created a game: `host::GAME` had read a foreign block and
looked occupied. Eight of `prpr`'s own `thread_local!`s and `std`'s are exposed the same way.

A passing link says nothing about any of this - the `.tls` section was present the whole time while
the directory was absent. Hence a gate that reads the finished PE.

What it asserts
---------------
1. Data directory 9 is present and at least one `IMAGE_TLS_DIRECTORY32` (24 bytes) long.
2. The `.tls` section exists and is large enough to hold the observed template.
3. `StartAddressOfRawData` lands exactly on `_tls_start`, whose byte is `TLS_START_BYTE` at offset 0
   of `.tls` - which also proves the `.tls` subsection really sorted before `.tls$`.
4. `EndAddressOfRawData` points at `_tls_end` (`TLS_END_BYTE`) and is strictly past the start - so
   the template covers the real `#[thread_local]` storage and excludes only our own marker.
5. `AddressOfIndex` resolves into the image, into a writable section.

Exit status is the build contract:
    0  TLS directory present and consistent with `.tls`
    1  present but wrong - the caller must fail the build
    2  the file could not be read / parsed - also a failure, never a silent pass

Usage:
    python check_tls.py <dll>
"""

import argparse
import struct
import sys

sys.path.insert(0, __file__.rsplit("\\", 1)[0])
from pe_dump import rva_to_off  # noqa: E402

TLS_DIR_INDEX = 9
IMAGE_TLS_DIRECTORY32_SIZE = 24

# First/last contribution placed by `wp81_compat`. Matching these is what proves subsection
# ordering held instead of assuming it; if link.exe ever sorts differently the bytes move and this
# fails at build time rather than as a wrong-looking number on the phone.
TLS_START_BYTE = 0xA5
TLS_END_BYTE = 0x5A

SECTION_FLAGS_WRITE = 0x80000000


def read_pe(path):
    d = open(path, "rb").read()
    e_lfanew = struct.unpack_from("<I", d, 0x3C)[0]
    if d[e_lfanew:e_lfanew + 4] != b"PE\0\0":
        raise ValueError("not a PE image")
    _machine, nsec, _ts, _sy, _ns, optsz, _ch = struct.unpack_from(
        "<HHIIIHH", d, e_lfanew + 4)
    opt = e_lfanew + 24
    magic = struct.unpack_from("<H", d, opt)[0]
    if magic == 0x20B:
        raise ValueError("PE32+ image; this gate models IMAGE_TLS_DIRECTORY32 only")
    image_base = struct.unpack_from("<I", d, opt + 28)[0]
    dd = opt + 96

    sections = []
    for i in range(nsec):
        o = opt + optsz + i * 40
        nm = d[o:o + 8].rstrip(b"\0").decode("latin-1")
        vs, va, rs, rp, _rel, _ln, _nrel, _nln, chars = struct.unpack_from(
            "<IIIIIIHHI", d, o + 8)
        sections.append((nm, va, vs, rp, rs, chars))

    tls_rva, tls_size = struct.unpack_from("<II", d, dd + TLS_DIR_INDEX * 8)
    return d, image_base, sections, tls_rva, tls_size


def rva_offset(sections, rva):
    """`pe_dump.rva_to_off` takes five-tuples; ours carry a sixth field (section flags)."""
    return rva_to_off([(s[0], s[1], s[2], s[3], s[4]) for s in sections], rva)


def find_section(sections, name):
    for s in sections:
        if s[0] == name:
            return s
    return None


def main(argv):
    ap = argparse.ArgumentParser()
    ap.add_argument("dll")
    args = ap.parse_args(argv[1:])

    try:
        d, image_base, sections, tls_rva, tls_size = read_pe(args.dll)
    except Exception as exc:                                  # noqa: BLE001 - report, then fail
        print("CHECK FAILED: could not read %s: %s" % (args.dll, exc), file=sys.stderr)
        return 2

    print("%s TLS directory (data directory %d):" % (args.dll, TLS_DIR_INDEX))

    if not tls_rva or not tls_size:
        print("   BAD  RVA=%08X size=%d  <- absent" % (tls_rva, tls_size), file=sys.stderr)
        print("\nCHECK FAILED: no TLS directory. The loader therefore never assigns this image a",
              file=sys.stderr)
        print("TLS index, `_tls_index` stays 0, and every `#[thread_local]` in it reads the first",
              file=sys.stderr)
        print("module's block instead. That is `GameCreate -> AlreadyRunning` before anything was",
              file=sys.stderr)
        print("created. Define `_tls_used` in `rust-wp81/tlsdir_start.rs` and link it as the first",
              file=sys.stderr)
        print("to emit.", file=sys.stderr)
        return 1
    if tls_size < IMAGE_TLS_DIRECTORY32_SIZE:
        print("   BAD  size=%d, expected >= %d" % (tls_size, IMAGE_TLS_DIRECTORY32_SIZE),
              file=sys.stderr)
        return 1

    off = rva_offset(sections, tls_rva)
    if off is None:
        print("   BAD  TLS directory RVA %08X is not inside any section" % tls_rva,
              file=sys.stderr)
        return 1
    start, end, index_va, callbacks_va, zero_fill, chars = struct.unpack_from(
        "<IIIIII", d, off)

    tls_sec = find_section(sections, ".tls")
    if tls_sec is None:
        print("   BAD  no .tls section, but a TLS directory points at RVA %08X" % tls_rva,
              file=sys.stderr)
        return 1
    _nm, tls_va, tls_vs, tls_rp, tls_rs, _ch = tls_sec
    tls_img_va = image_base + tls_va

    ok = True

    def check(cond, label, detail):
        nonlocal ok
        print("   %s %s  %s" % (" OK " if cond else "BAD", label, detail))
        if not cond:
            ok = False

    check(tls_vs > 0, "section", ".tls virtual size %d bytes at image VA %08X"
          % (tls_vs, tls_img_va))

    if not (tls_img_va <= start < end <= tls_img_va + tls_vs):
        check(False, "bounds",
              "template [%08X, %08X) escapes .tls [%08X, %08X)"
              % (start, end, tls_img_va, tls_img_va + tls_vs))
        return 1

    start_off = start - tls_img_va
    end_off = end - tls_img_va

    # The binding one. Offsets are computed as `secrel`, i.e. relative to the start of the section
    # holding the variable, so they agree with what the loader copies only if `Start` *is* offset 0.
    # Anything else shifts every thread-local in the image - and still does not fault, which is why
    # this is an assertion rather than a note.
    check(start_off == 0, "Start at .tls+0",
          "offset %d - must be 0, else every `#[thread_local]` reads shifted data" % start_off)
    # `_tls_end` is the last contribution, and `End` points *at* it rather than past it, so the
    # template stops one byte short of the section. That byte is our own marker, so nothing real is
    # dropped; requiring coverage of everything else is what stops a missing end marker going
    # unnoticed.
    check(end_off >= tls_vs - 1, "coverage",
          "template covers %d of %d bytes (.tls) - end marker missing or not linked last?"
          % (end_off, tls_vs))

    s_off = tls_rp + start_off
    e_off = tls_rp + end_off
    check(d[s_off] == TLS_START_BYTE, "_tls_start",
          "byte at .tls+0 = %02X, expected %02X" % (d[s_off], TLS_START_BYTE))
    check(d[e_off] == TLS_END_BYTE, "_tls_end",
          "byte at .tls+%d = %02X, expected %02X (excluded from the template)"
          % (end_off, d[e_off], TLS_END_BYTE))

    idx_sec = None
    for s in sections:
        if image_base + s[1] <= index_va < image_base + s[1] + s[2]:
            idx_sec = s
            break
    check(idx_sec is not None, "AddressOfIndex",
          "%s -> %s" % (index_va and ("%08X" % index_va) or "NULL",
                        idx_sec[0] if idx_sec else "outside the image"))
    if idx_sec is not None:
        check(bool(idx_sec[5] & SECTION_FLAGS_WRITE), "index writable",
              "%s has IMAGE_SCN_MEM_WRITE" % idx_sec[0])
    check(callbacks_va != 0, "AddressOfCallBacks", "%08X" % callbacks_va)
    check(zero_fill == 0, "SizeOfZeroFill", "%d" % zero_fill)
    print("   OK  Characteristics 0x%08X" % chars)

    if not ok:
        print("\nCHECK FAILED: TLS directory is present but does not describe `.tls`.",
              file=sys.stderr)
        print("A wrong directory is worse than none: the loader would allocate a block of the wrong",
              file=sys.stderr)
        print("size and `#[thread_local]` would resolve outside it.", file=sys.stderr)
        return 1

    print("CHECK OK: TLS directory present, template [%08X, %08X) = %d bytes, markers at both ends"
          % (start, end, end - start))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
