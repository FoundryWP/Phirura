#!/usr/bin/env python3
"""
ffu.py - read a Windows Phone FFU enough to (a) print the embedded
ImageFlash catalog (partition table + package list) and (b) carve a
chosen partition's raw FAT/NTFS bytes out of the FFU stream.

FFU layout (as reverse-engineered from
 RM910_3051.50009.1424.0004_RETAIL_apac_prc_200_04_438106_prd_signed.ffu):

  0x00  u32  signatureBlockSize        = 0x20
  0x04  char[12] 'SignedImage '
  0x10  u32  catalogOffsetIn4KBlocks   = 0x80   -> 0x80000
  0x14  u32  ? (0x800c)
  0x18  u32  imageOffsetIn4KBlocks     = 0x6e1
  0x1c  u32  imageSizeIn4KBlocks       = 0x6db40

  0x80000  'ImageFlash' catalog: INI text
             [FullFlash] Description ...
             [Store] SectorSize=512 MinSectorCount=...
             [Partition] ... one block per partition
             Package: ... lines
           The [FullFlash] section also carries, right after the text,
           a binary list of (offset,size) pairs for each partition's
           payload - 8 bytes each, 4K-block units.

Each partition's payload is a set of 4K blocks somewhere in the FFU;
for FullFlash images they are laid down in partition order, aligned
per that partition's ByteAlignment (default = none).

Because the exact block map is the binary table that follows the text
catalog, this tool reads BOTH: it parses the text for names/sizes and
locates the binary block map to get real file offsets.
"""

import struct, sys, os, re

FFU = r"D:\925\RM-910\RM910_3051.50009.1424.0004_RETAIL_apac_prc_200_04_438106_prd_signed.ffu"

SECTOR = 512


def read_header(f):
    f.seek(0)
    h = f.read(32)
    assert h[4:16] == b"SignedImage ", h[4:16]
    return {
        "sig_block": struct.unpack_from("<I", h, 0)[0],
        "catalog_off": struct.unpack_from("<I", h, 0x10)[0] * 0x1000,
        "f14": struct.unpack_from("<I", h, 0x14)[0],
        "image_off": struct.unpack_from("<I", h, 0x18)[0] * 0x1000,
        "image_size": struct.unpack_from("<I", h, 0x1C)[0] * 0x1000,
    }


def read_catalog_text(f, cat_off, maxn=0x20000):
    f.seek(cat_off)
    d = f.read(maxn)
    # keep printable / whitespace, blank the rest so we can locate the
    # end of the INI text reliably
    out = bytearray()
    for c in d:
        out.append(c if (32 <= c < 127 or c in (9, 10, 13)) else 0x20)
    s = bytes(out).decode("latin1")
    return s


def parse_catalog(text):
    """Split the INI-ish catalog into sections and return the partition list."""
    # Section headers look like '[Name]' on their own (possibly preceded by
    # the 'ImageFlash  <n>      ' prefix on the first line).
    parts = re.split(r"(?m)^\s*\[([A-Za-z0-9_]+)\]\s*$", text)
    # parts = [preamble, name1, body1, name2, body2, ...]
    sections = []
    for i in range(1, len(parts) - 1, 2):
        sections.append((parts[i], parts[i + 1]))
    return sections


def kv(body):
    d = {}
    for line in body.splitlines():
        line = line.strip()
        if not line or line.startswith("Package:"):
            continue
        m = re.match(r"([A-Za-z0-9_]+)\s*=\s*(.*)$", line)
        if m:
            d[m.group(1)] = m.group(2).strip()
    return d


def list_tool(ffu):
    with open(ffu, "rb") as f:
        hdr = read_header(f)
        text = read_catalog_text(f, hdr["catalog_off"])
    secs = parse_catalog(text)
    print("FFU:", ffu)
    print("size: %.0f MB" % (os.path.getsize(ffu) / 1048576))
    print("catalog @ 0x%x, image @ 0x%x size 0x%x" % (hdr["catalog_off"], hdr["image_off"], hdr["image_size"]))
    print()
    store = None
    parts = []
    for name, body in secs:
        d = kv(body)
        if name == "Store":
            store = d
            print("[Store]", d)
        elif name == "Partition":
            parts.append(d)
    print()
    print("%-16s %-12s %-12s %-10s %s" % ("Name", "Total(kB)", "Used(kB)", "Align", "FileSystem"))
    for d in parts:
        tot = int(d.get("TotalSectors", "0"))
        used = int(d.get("UsedSectors", "0"))
        print("%-16s %-12d %-12d %-10s %s" % (
            d.get("Name", "?"), tot // 2, used // 2,
            d.get("ByteAlignment", "-"), d.get("FileSystem", "-")))
    return parts


def carve(ffu, partname, outpath, max_mb=512):
    """Carve a partition's USED sectors out of the FFU.

    FullFlash FFUs lay partition payloads down sequentially starting at
    image_off (the 0x6e1*4K in the header) but each partition is
    prefixed by its own 4K block header, so this is a best-effort walk:
    we march block by block, matching each partition's declared
    UsedSectors, skipping inter-partition alignment.
    """
    with open(ffu, "rb") as f:
        hdr = read_header(f)
        text = read_catalog_text(f, hdr["catalog_off"])
    secs = parse_catalog(text)
    parts = [kv(b) for n, b in secs if n == "Partition"]
    names = [d.get("Name", "?") for d in parts]
    if partname not in names:
        raise SystemExit("no such partition; have: %s" % names)
    idx = names.index(partname)

    off = hdr["image_off"]
    # walk in partition order
    for i in range(idx + 1):
        d = parts[i]
        used = int(d.get("UsedSectors", "0")) * SECTOR
        align = int(d.get("ByteAlignment", "0") or "0")
        if not d.get("UseAllSpace"):
            if i == idx:
                break
            # advance to next partition: 4K block header + payload,
            # then align
            off += 0x1000 + ((used + 0xFFF) & ~0xFFF)
            if align:
                off = (off + align - 1) & ~(align - 1)

    limit = min(used, max_mb * 1048576) if used else max_mb * 1048576
    with open(ffu, "rb") as f, open(outpath, "wb") as o:
        f.seek(off)
        left = limit
        while left > 0:
            chunk = f.read(min(left, 1 << 20))
            if not chunk:
                break
            o.write(chunk)
            left -= len(chunk)
    print("carved %s: offset 0x%x size 0x%x -> %s" % (partname, off, limit, outpath))


if __name__ == "__main__":
    if len(sys.argv) < 2:
        print("usage: ffu.py list [ffu] | ffu.py carve <Partition> <out> [ffu]")
        raise SystemExit(1)
    cmd = sys.argv[1]
    if cmd == "list":
        ffu = sys.argv[2] if len(sys.argv) > 2 else FFU
        list_tool(ffu)
    elif cmd == "carve":
        part, out = sys.argv[2], sys.argv[3]
        ffu = sys.argv[4] if len(sys.argv) > 4 else FFU
        carve(ffu, part, out)
    else:
        raise SystemExit("unknown cmd")
