"""Dump the ECMA-335 #Strings heap of a .winmd so we can grep real type names.

Grepping raw bytes gives false positives (a substring can live inside another
string), and a naive metadata walk is easy to get wrong. Reading the heap
directly is small, exact, and good enough for "does this type exist" checks.
"""
import struct
import sys


def read_strings_heap(path):
    data = open(path, "rb").read()

    # PE header -> COFF -> optional header -> data directories.
    pe_off = struct.unpack_from("<I", data, 0x3C)[0]
    assert data[pe_off:pe_off + 4] == b"PE\0\0", "not a PE file"
    coff = pe_off + 4
    num_sections = struct.unpack_from("<H", data, coff + 2)[0]
    opt_size = struct.unpack_from("<H", data, coff + 16)[0]
    opt = coff + 20
    magic = struct.unpack_from("<H", data, opt)[0]
    # PE32 (0x10b) has a 4-byte image base; PE32+ (0x20b) has an 8-byte one.
    dir_off = opt + (96 if magic == 0x10B else 112)
    # Data directory index 14 == CLR runtime header.
    clr_rva, clr_size = struct.unpack_from("<II", data, dir_off + 14 * 8)

    sections = []
    sec = opt + opt_size
    for i in range(num_sections):
        base = sec + i * 40
        name = data[base:base + 8].rstrip(b"\0").decode("ascii", "replace")
        vsize, vaddr, rawsize, rawptr = struct.unpack_from("<IIII", data, base + 8)
        sections.append((name, vaddr, vsize, rawptr, rawsize))

    def rva_to_off(rva):
        for name, vaddr, vsize, rawptr, rawsize in sections:
            if vaddr <= rva < vaddr + max(vsize, rawsize):
                return rawptr + (rva - vaddr)
        raise ValueError("rva 0x%x not in any section" % rva)

    clr = rva_to_off(clr_rva)
    md_rva, md_size = struct.unpack_from("<II", data, clr + 8)
    md = rva_to_off(md_rva)
    assert data[md:md + 4] == b"BSJB", "no metadata root"

    ver_len = struct.unpack_from("<I", data, md + 12)[0]
    p = md + 16 + ver_len + 2
    streams = struct.unpack_from("<H", data, p)[0]
    p += 2
    heap = {}
    for _ in range(streams):
        off, size = struct.unpack_from("<II", data, p)
        p += 8
        end = data.index(b"\0", p)
        name = data[p:end].decode("ascii")
        p = end + 1
        p = (p + 3) & ~3
        heap[name] = (md + off, size)

    start, size = heap["#Strings"]
    blob = data[start:start + size]
    out = []
    i = 0
    while i < len(blob):
        if blob[i] == 0:
            i += 1
            continue
        j = blob.index(b"\0", i)
        out.append(blob[i:j].decode("utf-8", "replace"))
        i = j + 1
    return out


if __name__ == "__main__":
    names = read_strings_heap(sys.argv[1])
    needles = sys.argv[2:] or ["Activated"]
    for n in names:
        for needle in needles:
            if needle in n:
                print(n)
                break
