#!/usr/bin/env python3
r"""Minimal Microsoft Cabinet (MSCF) reader - lists and extracts CFFILE entries.

Why hand-rolled: this sandbox blocks or misbehaves for the usual tools. `expand.exe` is a Windows
binary the tool layer will not always run, and the point here is to look *inside* a Windows Phone
package CAB to see which binaries the real device ships. A read-only enumerator is enough, and it
has no dependencies.

Scope: the CFHEADER + CFFOLDER + CFFILE structures of MS-CAB (plain, not cabinet-set spanning).
Compression is reported per folder; extraction only handles `none` (0) in this pass - the WP SPKG
payload lives in a nested format anyway, so listing is what is actually useful.

Usage:
    python cab.py list <file.cab>
    python cab.py list <file.cab> --filter d3d
    python cab.py extract <file.cab> <name-substring> <outdir>
"""

import os
import struct
import sys


def _walk_cffile(data, off, end, base):
    """Yield (name, uncompressed_size, folder_index, payload_offset, comp_type) for each CFFILE."""
    pos = off
    while pos + 16 <= end:
        # CFHEADER continuation: 16 bytes when the previous file spans a block (iFolder != 0xFFFD).
        start = pos
        cb_file, uoff, ifolder, dtime, ddate, fattrib = struct.unpack_from("<IIHHHH", data, pos)
        pos += 16
        # name is NUL-terminated
        nul = data.find(b"\0", pos, end)
        if nul < 0:
            return
        name = data[pos:nul]
        pos = nul + 1
        folder = ifolder
        payload = None
        if folder == 0xFFFD:
            # continuation of the previous file - no name follows
            folder = None
            name = None
        yield (name.decode("latin-1") if name else None, cb_file, folder,
               (base + uoff) if folder is not None else None, None)


def list_cab(path, want_filter=None):
    data = open(path, "rb").read()
    if data[:4] != b"MSCF":
        raise ValueError("not a plain cabinet (expected MSCF)")
    # CFHEADER
    (_sig, reserved1, cbCabinet, reserved2, coffFiles, reserved3,
     versionMinor, versionMajor, cFolders, cFiles, flags, setID, iCabinet) = struct.unpack_from(
        "<4sIIIIIBBHHHHH", data, 0)
    print(f"version        : {versionMajor}.{versionMinor}")
    print(f"cabinet size   : {cbCabinet}")
    print(f"folders/files  : {cFolders} / {cFiles}")
    print(f"flags          : 0x{flags:04x}")

    # CBIF / optional reserved fields
    pos = 36
    if flags & 0x0004:      # prev cabinet
        n = data.find(b"\0", pos); pos = n + 1
    if flags & 0x0001:      # next cabinet
        n = data.find(b"\0", pos); pos = n + 1
    if flags & 0x0008:      # reserve
        cbPrev, cbNext = struct.unpack_from("<HH", data, pos); pos += 4

    folders = []
    for _ in range(cFolders):
        coff_start, cFilesInFolder, compType = struct.unpack_from("<IHH", data, pos)
        pos += 8
        folders.append((coff_start, cFilesInFolder, compType))
    if flags & 0x0004:
        pos += 1  # iFolder in previous cabinet

    print("\nfolders:")
    _COMP = {0: "none", 1: "MSZIP", 2: "Quantum", 3: "LZX"}
    for i, (coff, ncf, ct) in enumerate(folders):
        print(f"  [{i}] data@0x{coff:x}  files={ncf}  compression={_COMP.get(ct, ct)}")

    # CFFILE entries start at coffFiles; each folder's block starts at its coff_start.
    rows = []
    for i, (coff, ncf, ct) in enumerate(folders):
        pos2 = coffFiles if i == 0 else pos2
        # CFFILEs are contiguous from coffFiles; folder boundary is implied by iFolder.
        pass

    pos = coffFiles
    end = len(data)
    count = 0
    for _ in range(cFiles):
        if pos + 16 > end:
            break
        start = pos
        cb_file, uoff, ifolder, dtime, ddate, fattrib = struct.unpack_from("<IIHHHH", data, pos)
        pos += 16
        nul = data.find(b"\0", pos, end)
        if nul < 0:
            break
        raw_name = data[pos:nul]
        name = raw_name.decode("latin-1")
        pos = nul + 1
        fld = folders[ifolder] if ifolder < len(folders) else (0, 0, 0)
        abs_off = fld[0] + uoff
        rows.append((name, cb_file, ifolder, abs_off, fld[2]))
        count += 1

    print(f"\nfiles: {count}")
    for name, size, fld, off, ct in rows:
        if want_filter and want_filter.lower() not in name.lower():
            continue
        print(f"  {size:10d}  f{fld}  @0x{off:08x}  {name}")
    return rows


def extract(path, substr, outdir):
    rows = list_cab(path, want_filter=substr)
    data = open(path, "rb").read()
    os.makedirs(outdir, exist_ok=True)
    for name, size, fld, off, ct in rows:
        if substr.lower() not in name.lower():
            continue
        if ct != 0:
            print(f"  skip (compressed f{fld}): {name}")
            continue
        blob = data[off:off + size]
        dest = os.path.join(outdir, os.path.basename(name.replace("\\", "/")))
        with open(dest, "wb") as f:
            f.write(blob)
        print(f"  wrote {len(blob):9d} -> {dest}")


def main(argv):
    if len(argv) < 3:
        print(__doc__)
        return 2
    cmd, cab = argv[1], argv[2]
    filt = None
    if "--filter" in argv:
        filt = argv[argv.index("--filter") + 1]
    if cmd == "list":
        list_cab(cab, filt)
        return 0
    if cmd == "extract":
        rest = [a for a in argv[3:] if not a.startswith("--")]
        if len(rest) < 2:
            print("usage: cab.py extract <cab> <substr> <outdir>")
            return 2
        extract(cab, rest[0], rest[1])
        return 0
    print(__doc__)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv))
