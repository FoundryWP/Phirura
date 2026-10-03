#!/usr/bin/env python3
"""Minimal COFF object reader for x86/x64/ARM `.obj` files.

Written because `dumpbin.exe` produces no output when invoked under the sandbox here, and the
question it needs to answer is narrow: **does a bare `unsafe extern "system" { pub fn X(); }`
emit an `__imp_X` reference to a DLL in the object, or just an undefined `X`?**

Everything else in this project's tooling is deflected by that one fact, so the reader is kept
deliberately small and layout-exact rather than general.

Usage:
    python coff_dump.py <obj> [--symbols] [--relocs] [--drectve]
"""

import argparse
import struct
import sys

SECTION_HDR = struct.Struct("<8sIIIIIIHHI")   # 40 bytes, exactly as the PE/COFF spec lays it out
SYMBOL = struct.Struct("<8sIhHBB")            # 18 bytes


class Coff:
    def __init__(self, path):
        with open(path, "rb") as fh:
            self.data = fh.read()
        (self.machine, self.nsec, self.timestamp, self.symptr, self.nsym,
         self.opt_size, self.chars) = struct.unpack_from("<HHIIIHH", self.data, 0)
        if self.opt_size != 0:
            raise ValueError("object with an optional header is not supported")
        self.sh_off = 20
        self.sections = []
        for i in range(self.nsec):
            off = self.sh_off + i * 40
            fields = SECTION_HDR.unpack_from(self.data, off)
            name = fields[0].rstrip(b"\0").decode("ascii", "replace")
            self.sections.append({
                "name": name, "vsize": fields[1], "vaddr": fields[2],
                "raw_size": fields[3], "raw_ptr": fields[4],
                "reloc_ptr": fields[5], "lineno_ptr": fields[6],
                "nreloc": fields[7], "nlineno": fields[8], "chars": fields[9],
            })
        # The string table lives immediately after the symbol table (which itself is placed
        # after all section raw data), NOT after the section headers. Getting this wrong makes
        # every long symbol name read as garbage - which is exactly how the first version of
        # this file reported symbols named '\x13'.
        self.strtab_off = self.symptr + self.nsym * 18

    def _name_at(self, off):
        raw = self.data[off:off + 8]
        if raw[:4] == b"\0\0\0\0":
            so = struct.unpack_from("<I", raw, 4)[0]
            base = self.strtab_off + so
            end = self.data.index(b"\0", base)
            return self.data[base:end].decode("ascii", "replace")
        return raw.rstrip(b"\0").decode("ascii", "replace")

    def symbols(self):
        i = 0
        while i < self.nsym:
            off = self.symptr + i * 18
            n8, value, secnum, typ, cls, naux = SYMBOL.unpack_from(self.data, off)
            yield {"name": self._name_at(off), "value": value, "section": secnum,
                   "type": typ, "storage_class": cls, "aux": naux}
            i += 1 + naux

    def section_data(self, name, n=0):
        seen = 0
        for s in self.sections:
            if s["name"] == name:
                if seen == n:
                    return self.data[s["raw_ptr"]:s["raw_ptr"] + s["raw_size"]]
                seen += 1
        return b""


def main(argv):
    ap = argparse.ArgumentParser()
    ap.add_argument("obj")
    ap.add_argument("--symbols", action="store_true")
    ap.add_argument("--drectve", action="store_true")
    ap.add_argument("--filter", default="")
    args = ap.parse_args(argv[1:])

    c = Coff(args.obj)
    print("machine=%04x sections=%d symbols=%d" % (c.machine, c.nsec, c.nsym))
    for s in c.sections:
        print("  %-9s raw=%-6d size=%-6d reloc=%-4d chars=%08x"
              % (s["name"], s["raw_ptr"], s["raw_size"], s["nreloc"], s["chars"]))

    if args.drectve:
        for i in range(8):
            d = c.section_data(".drectve", i)
            if not d:
                break
            print("--- .drectve [%d] ---" % i)
            print(d.decode("ascii", "replace"))

    if args.symbols:
        print("--- symbols ---")
        for s in c.symbols():
            if args.filter and args.filter not in s["name"]:
                continue
            kind = {2: "EXTERNAL", 3: "STATIC", 105: "WEAK"}.get(s["storage_class"], str(s["storage_class"]))
            print("  %-8s sec=%-3d %s" % (kind, s["section"], s["name"]))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
