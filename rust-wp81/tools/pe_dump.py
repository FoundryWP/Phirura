#!/usr/bin/env python3
"""Dump the export and import tables of a PE32/PE32+ image.

Needed because the question "did /FORCE:MULTIPLE silently keep a bogus IAT slot?" cannot be
answered from the link log - the log says LNK4006 "second definition ignored" and nothing about
which one survives in the import table. Reading the import descriptors directly does answer it:
if `GetLastError` is absent from the import table, the linker bound every call site to compat's
definition (`wp81_compat`), which is what the port wants; if it is present, `/FORCE:MULTIPLE`
produced an image whose IAT points into a DLL that does not exist on WP8.1, and the failure
would only appear on the device.

Usage:
    python pe_dump.py <dll> [--imports] [--exports] [--sections]
"""

import argparse
import struct
import sys


def rva_to_off(sections, rva):
    for name, va, vsize, raw_ptr, raw_size in sections:
        if va <= rva < va + max(vsize, raw_size):
            return raw_ptr + (rva - va)
    return None


def read_cstr(data, off):
    end = data.index(b"\0", off)
    return data[off:end].decode("latin-1")


def main(argv):
    ap = argparse.ArgumentParser()
    ap.add_argument("dll")
    ap.add_argument("--imports", action="store_true")
    ap.add_argument("--exports", action="store_true")
    ap.add_argument("--sections", action="store_true")
    args = ap.parse_args(argv[1:])

    d = open(args.dll, "rb").read()
    e_lfanew = struct.unpack_from("<I", d, 0x3C)[0]
    machine, nsec, ts, sy, ns, optsz, ch = struct.unpack_from("<HHIIIHH", d, e_lfanew + 4)
    opt = e_lfanew + 24
    magic = struct.unpack_from("<H", d, opt)[0]
    is64 = magic == 0x20B
    dd = opt + (112 if is64 else 96)
    imgbase = struct.unpack_from("<I", d, opt + (24 if is64 else 28))[0]

    sh = opt + optsz
    sections = []
    for i in range(nsec):
        o = sh + i * 40
        nm = d[o:o + 8].rstrip(b"\0").decode("latin-1")
        vs, va, rs, rp = struct.unpack_from("<IIII", d, o + 8)
        sections.append((nm, va, vs, rp, rs))
    if args.sections:
        for nm, va, vs, rp, rs in sections:
            print("  %-9s va=%08x vsize=%-7d raw=%-7d rawsize=%d" % (nm, va, vs, rp, rs))

    print("machine=%04x sections=%d base=%08x %s" % (machine, nsec, imgbase, "PE32+" if is64 else "PE32"))

    if args.exports:
        rva, size = struct.unpack_from("<II", d, dd + 0 * 8)
        print("--- exports (%d bytes) ---" % size)
        if rva:
            off = rva_to_off(sections, rva)
            nfun, nname = struct.unpack_from("<II", d, off + 20)
            names_rva = struct.unpack_from("<I", d, off + 32)[0]
            noff = rva_to_off(sections, names_rva)
            for i in range(nname):
                nrva = struct.unpack_from("<I", d, noff + i * 4)[0]
                print("   " + read_cstr(d, rva_to_off(sections, nrva)))

    if args.imports:
        rva, size = struct.unpack_from("<II", d, dd + 1 * 8)
        print("--- imports (%d bytes, %d descriptor(s) incl. null) ---" % (size, size // 20))
        off = rva_to_off(sections, rva)
        i = 0
        while True:
            olt, ts_, fc, name_rva, fthunk = struct.unpack_from("<IIIII", d, off + i * 20)
            if olt == 0 and name_rva == 0:
                break
            dll = read_cstr(d, rva_to_off(sections, name_rva))
            print("  %s" % dll)
            t = olt or fthunk
            toff = rva_to_off(sections, t)
            j = 0
            while True:
                ent = struct.unpack_from("<I", d, toff + j * 4)[0]
                if ent == 0:
                    break
                if ent & 0x80000000:
                    print("      #%d" % (ent & 0xFFFF))
                else:
                    print("      " + read_cstr(d, rva_to_off(sections, ent) + 2))
                j += 1
            i += 1
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
