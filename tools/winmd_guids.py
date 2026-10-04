#!/usr/bin/env python3
"""Read a .winmd's interface GUIDs by walking its metadata properly.

Why not reflection: `Assembly.LoadFrom` refuses a WinRT .winmd ("operation not supported",
0x80131515) because it is a reference-only metadata file, and `System.Reflection.Metadata` is not
present on this machine's .NET 4.0. A byte-scan for 16-byte GUIDs also fails, because the `#GUID`
heap stores an *index* into a linear stream, not the GUIDs themselves in the file body.

So this walks the CLI metadata directly:
  * parse the PE to find the CLI header -> metadata root
  * read the stream directory to locate `#~` (tables), `#Strings`, `#GUID`, `#Blob`
  * decode the `#~` table header, then walk TypeDef (0x02) and InterfaceImpl (0x09)
  * for each TypeDef, if it has a CustomAttribute (0x0C) whose ctor is
    Windows.Foundation.Metadata.GuidAttribute, read the blob and format the GUID

That is the authoritative reader: it returns exactly what the WinRT metadata generator wrote.
"""

import struct
import sys

# ---- table indices we care about ----
TYPEDEF = 0x02
CUSTOMATTRIBUTE = 0x0C

# CustomAttribute.Type values that mean "member ref / method def"
CA_TYPE_METHODDEF = 0x06
CA_TYPE_MEMBERREF = 0x0A


class Stream:
    def __init__(self, data, name, off, size):
        self.data = data
        self.name = name
        self.off = off
        self.size = size

    def read(self, p, n):
        return self.data[self.off + p:self.off + p + n]


def u16(b, o):
    return struct.unpack_from("<H", b, o)[0]


def u32(b, o):
    return struct.unpack_from("<I", b, o)[0]


def parse_pe(data):
    if data[:2] != b"MZ":
        raise ValueError("not a PE")
    pe_off = u32(data, 0x3C)
    if data[pe_off:pe_off + 4] != b"PE\0\0":
        raise ValueError("no PE signature")
    coff = pe_off + 4
    n_sections = u16(data, coff + 2)
    opt_size = u16(data, coff + 16)
    opt_off = coff + 20
    magic = u16(data, opt_off)
    # data directory 14 = CLI header
    if magic == 0x20B:      # PE32+
        dd_off = opt_off + 112
    else:                    # PE32
        dd_off = opt_off + 96
    cli_rva = u32(data, dd_off + 14 * 8)
    cli_size = u32(data, dd_off + 14 * 8 + 4)
    # section table
    sec_off = opt_off + opt_size
    sections = []
    for i in range(n_sections):
        s = sec_off + i * 40
        name = data[s:s + 8].rstrip(b"\0").decode("latin1")
        vsize = u32(data, s + 8)
        vaddr = u32(data, s + 12)
        rsize = u32(data, s + 16)
        raw = u32(data, s + 20)
        sections.append((name, vaddr, vsize, raw, rsize))

    def rva_to_off(rva):
        for name, vaddr, vsize, raw, rsize in sections:
            if vaddr <= rva < vaddr + max(vsize, rsize):
                return raw + (rva - vaddr)
        raise ValueError("rva %x not mapped" % rva)

    return rva_to_off, cli_rva, cli_size


def parse_metadata(data):
    rva_to_off, cli_rva, cli_size = parse_pe(data)
    cli = rva_to_off(cli_rva)
    md_rva = u32(data, cli + 8)
    md = rva_to_off(md_rva)

    if data[md:md + 4] != b"BSJB":
        raise ValueError("no BSJB signature")
    ver_len = u32(data, md + 12)
    p = md + 16 + ver_len + 2 + 2      # version + flags(2) + stream count(2)
    n_streams = u16(data, md + 16 + ver_len + 2)
    streams = {}
    for _ in range(n_streams):
        s_off = u32(data, p)
        s_size = u32(data, p + 4)
        p += 8
        end = data.index(b"\0", p)
        s_name = data[p:end].decode("latin1")
        p = end + 1
        p = (p + 3) & ~3
        streams[s_name] = Stream(data, s_name, md + s_off, s_size)
    return streams


def decode_tables(streams):
    """Return the `#~` tables as a dict of {table_id: (rows, row_size, offsets)}."""
    td = streams["#~"]
    b = td.data[td.off:td.off + td.size]
    heap_sizes = b[6]
    valid = struct.unpack_from("<Q", b, 8)[0]
    sorted_ = struct.unpack_from("<Q", b, 16)[0]
    p = 24
    row_counts = {}
    for i in range(64):
        if valid & (1 << i):
            row_counts[i] = u32(b, p)
            p += 4
    tables = {}
    for i in range(64):
        if not (valid & (1 << i)):
            continue
        rows = row_counts[i]
        size = row_size_of(i, rows, row_counts, heap_sizes)
        tables[i] = (rows, size, p)
        p += size * rows
    return tables, heap_sizes, b


def _str_index_size(heap_sizes):
    return 4 if heap_sizes & 1 else 2


def _guid_index_size(heap_sizes):
    return 4 if heap_sizes & 2 else 2


def _blob_index_size(heap_sizes):
    return 4 if heap_sizes & 4 else 2


def _table_index_size(table, row_counts):
    return 4 if row_counts.get(table, 0) >= 0x10000 else 2


def _coded_index_size(tags, table_types, row_counts):
    """Size in bytes of an ECMA-335 *coded* index.

    A coded index packs a tag (the index into `table_types`) into the low bits together with a
    row index. Its width is 2 bytes if the largest row count among `table_types` fits in
    `16 - bits` bits, otherwise 4. Treating a coded index as a plain 2-byte table index silently
    shifts every later column of the row - which is what produced garbage string indices.
    """
    bits = 0
    n = len(table_types) - 1
    while n:
        bits += 1
        n >>= 1
    limit = 1 << (16 - bits)
    biggest = max((row_counts.get(t, 0) for t in table_types), default=0)
    return 4 if biggest >= limit else 2


def _simple_or_coded(tid, spec, row_counts, heap_sizes):
    """Decode one column spec string into a byte width.

    Specs are strings like "str", "blob", "guid", "T04" (simple table index) or
    "C:TypeDefOrRef" (coded index). This keeps the layouts readable and, more importantly,
    makes the coded-index widths correct by construction.
    """
    if spec == "str":
        return _str_index_size(heap_sizes)
    if spec == "blob":
        return _blob_index_size(heap_sizes)
    if spec == "guid":
        return _guid_index_size(heap_sizes)
    if spec.startswith("T"):
        return _table_index_size(int(spec[1:], 16), row_counts)
    if spec.startswith("C:"):
        return _coded_index_size(spec[2:], CODED[spec[2:]], row_counts)
    raise ValueError("bad column spec %r" % spec)


# ECMA-335 II.24.2.6 coded index tag tables.
CODED = {
    "TypeDefOrRef": (0x02, 0x01, 0x1B),
    "HasConstant": (0x04, 0x08, 0x17),
    "HasCustomAttribute": (0x06, 0x04, 0x01, 0x02, 0x08, 0x09, 0x0A, 0x00, 0x0E, 0x17, 0x14,
                           0x11, 0x1A, 0x1B, 0x20, 0x23, 0x26, 0x27, 0x28, 0x2A, 0x2C),
    "HasFieldMarshal": (0x04, 0x08),
    "HasDeclSecurity": (0x02, 0x06, 0x20),
    "MemberRefParent": (0x02, 0x01, 0x1A, 0x06, 0x1B),
    "HasSemantics": (0x14, 0x17),
    "MethodDefOrRef": (0x06, 0x0A),
    "MemberForwarded": (0x04, 0x06),
    "Implementation": (0x26, 0x23, 0x27),
    "CustomAttributeType": (0x00, 0x00, 0x06, 0x0A, 0x00),
    "ResolutionScope": (0x00, 0x1A, 0x23, 0x01),
    "TypeOrMethodDef": (0x02, 0x06),
}

# ECMA-335 II.22 table column layouts, as (name, spec) pairs. `fixed:N` is an opaque N-byte field.
TABLE_LAYOUTS = {
    0x00: [("Generation", "fixed:2"), ("Name", "str"), ("Mvid", "guid"), ("EncId", "guid"),
           ("EncBaseId", "guid")],
    0x01: [("ResolutionScope", "C:ResolutionScope"), ("Name", "str"), ("Namespace", "str")],
    0x02: [("Flags", "fixed:4"), ("Name", "str"), ("Namespace", "str"),
           ("Extends", "C:TypeDefOrRef"), ("FieldList", "T04"), ("MethodList", "T06")],
    0x03: [("Field", "T04")],
    0x04: [("Flags", "fixed:2"), ("Name", "str"), ("Signature", "blob")],
    0x05: [("Method", "T06")],
    0x06: [("RVA", "fixed:4"), ("ImplFlags", "fixed:2"), ("Flags", "fixed:2"), ("Name", "str"),
           ("Signature", "blob"), ("ParamList", "T08")],
    0x07: [("Param", "T08")],
    0x08: [("Flags", "fixed:2"), ("Sequence", "fixed:2"), ("Name", "str")],
    0x09: [("Class", "T02"), ("Interface", "C:TypeDefOrRef")],
    0x0A: [("Class", "C:MemberRefParent"), ("Name", "str"), ("Signature", "blob")],
    0x0B: [("Type", "fixed:1"), ("Padding", "fixed:1"), ("Parent", "C:HasConstant"),
           ("Value", "blob")],
    0x0C: [("Parent", "C:HasCustomAttribute"), ("Type", "C:CustomAttributeType"), ("Value", "blob")],
    0x0D: [("Parent", "C:HasFieldMarshal"), ("NativeType", "blob")],
    0x0E: [("Action", "fixed:2"), ("Parent", "C:HasDeclSecurity"), ("PermissionSet", "blob")],
    0x0F: [("PackingSize", "fixed:2"), ("ClassSize", "fixed:4"), ("Parent", "T02")],
    0x10: [("Offset", "fixed:4"), ("Field", "T04")],
    0x11: [("Signature", "blob")],
    0x12: [("Parent", "T02"), ("EventList", "T14")],
    0x13: [("EventFlags", "fixed:2"), ("Name", "str"), ("EventType", "C:TypeDefOrRef")],
    0x14: [("Parent", "T02"), ("PropertyList", "T17")],
    0x15: [("Flags", "fixed:2"), ("Name", "str"), ("Type", "blob")],
    0x16: [("Semantics", "fixed:2"), ("Method", "T06"), ("Association", "C:HasSemantics")],
    0x17: [("Class", "T02"), ("MethodBody", "T06"), ("MethodDeclaration", "T06")],
    0x18: [("Name", "str")],
    0x19: [("Signature", "blob")],
    0x1A: [("MappingFlags", "fixed:2"), ("MemberForwarded", "C:MemberForwarded"),
           ("ImportName", "str"), ("ImportScope", "T18")],
    0x1B: [("RVA", "fixed:4"), ("Field", "T04")],
    0x1C: [("Token", "fixed:4")],
    0x1D: [("Token", "fixed:4")],
    0x1E: [("HashAlgId", "fixed:4"), ("MajorVersion", "fixed:2"), ("MinorVersion", "fixed:2"),
           ("BuildNumber", "fixed:2"), ("RevisionNumber", "fixed:2"), ("Flags", "fixed:4"),
           ("PublicKey", "blob"), ("Name", "str"), ("Culture", "str")],
    0x1F: [("Processor", "fixed:4")],
    0x20: [("OSPlatformId", "fixed:4"), ("OSMajorVersion", "fixed:4"), ("OSMinorVersion", "fixed:4")],
    0x21: [("MajorVersion", "fixed:2"), ("MinorVersion", "fixed:2"), ("BuildNumber", "fixed:2"),
           ("RevisionNumber", "fixed:2"), ("Flags", "fixed:4"), ("PublicKeyOrToken", "blob"),
           ("Name", "str"), ("Culture", "str"), ("HashValue", "blob")],
    0x22: [("Processor", "fixed:4"), ("AssemblyRef", "T21")],
    0x23: [("OSPlatformId", "fixed:4"), ("OSMajorVersion", "fixed:4"), ("OSMinorVersion", "fixed:4"),
           ("AssemblyRef", "T21")],
    0x24: [("Flags", "fixed:4"), ("Name", "str"), ("HashValue", "blob")],
    0x25: [("Flags", "fixed:4"), ("TypeDefId", "fixed:4"), ("TypeName", "str"),
           ("TypeNamespace", "str"), ("Implementation", "C:Implementation")],
    0x26: [("Offset", "fixed:4"), ("Flags", "fixed:4"), ("Name", "str"),
           ("Implementation", "C:Implementation")],
    0x27: [("NestedClass", "T02"), ("EnclosingClass", "T02")],
    0x28: [("Number", "fixed:2"), ("Flags", "fixed:2"), ("Owner", "C:TypeOrMethodDef"),
           ("Name", "str")],
    0x29: [("Method", "C:MethodDefOrRef"), ("Instantiation", "blob")],
    0x2A: [("Owner", "T28"), ("Constraint", "C:TypeDefOrRef")],
}


def full_row_layout(tid, row_counts, heap_sizes):
    """Row size and column widths for table `tid`, honouring ECMA-335 coded indices."""
    cols = TABLE_LAYOUTS.get(tid)
    if cols is None:
        return None
    out = []
    for name, spec in cols:
        if spec.startswith("fixed:"):
            out.append(("fixed", int(spec[6:])))
        else:
            out.append(("idx", _simple_or_coded(tid, spec, row_counts, heap_sizes)))
    return out


def row_size_of(tid, rows, row_counts, heap_sizes):
    layout = full_row_layout(tid, row_counts, heap_sizes)
    if layout is None:
        raise NotImplementedError("table 0x%02x has no layout" % tid)
    return sum(w for _, w in layout)


def read_string(streams, index):
    s = streams["#Strings"]
    b = s.data[s.off:s.off + s.size]
    end = b.index(b"\0", index)
    return b[index:end].decode("utf-8", "replace")


def read_blob(streams, index):
    s = streams["#Blob"]
    b = s.data[s.off:s.off + s.size]
    p = index
    # compressed length
    first = b[p]
    if first & 0x80 == 0:
        n = first
        p += 1
    elif first & 0xC0 == 0x80:
        n = ((first & 0x3F) << 8) | b[p + 1]
        p += 2
    else:
        n = ((first & 0x1F) << 24) | (b[p + 1] << 16) | (b[p + 2] << 8) | b[p + 3]
        p += 4
    return b[p:p + n]


def format_guid(raw16):
    d1 = struct.unpack_from("<I", raw16, 0)[0]
    d2 = struct.unpack_from("<H", raw16, 4)[0]
    d3 = struct.unpack_from("<H", raw16, 6)[0]
    d4 = raw16[8:16]
    return "%08x-%04x-%04x-%s-%s" % (
        d1, d2, d3,
        d4[:2].hex(), d4[2:].hex())


def main():
    winmd = sys.argv[1] if len(sys.argv) > 1 else \
        r"C:\Program Files (x86)\Windows Phone Kits\8.1\References\CommonConfiguration\Neutral\Windows.winmd"
    want = set(sys.argv[2:])

    data = open(winmd, "rb").read()
    streams = parse_metadata(data)
    tables, heap_sizes, tb = decode_tables(streams)

    rows_td, size_td, off_td = tables[TYPEDEF]
    rows_ca, size_ca, off_ca = tables[CUSTOMATTRIBUTE]

    # Build a list of (owner_row, ctor_table, ctor_row, blob_off) for CustomAttribute.
    def read_row(tid, i):
        rows, size, off = tables[tid]
        row_counts_only = {k: v[0] for k, v in tables.items()}
        layout = full_row_layout(tid, row_counts_only, heap_sizes)
        p = off + i * size
        vals = []
        for kind, w in layout:
            if kind == "fixed":
                if w == 1:
                    vals.append(tb[p])
                elif w == 2:
                    vals.append(struct.unpack_from("<H", tb, p)[0])
                elif w == 4:
                    vals.append(struct.unpack_from("<I", tb, p)[0])
                else:
                    vals.append(tb[p:p + w])
            else:
                fmt = "<H" if w == 2 else "<I"
                vals.append(struct.unpack_from(fmt, tb, p)[0])
            p += w
        return vals

    # Map: TypeDef row -> guid
    guid_attr_ctor = None
    # Find the GuidAttribute MemberRef ctor signature: it is a MemberRef on
    # Windows.Foundation.Metadata.GuidAttribute whose name is ".ctor".
    # Cheaper and robust: scan all CustomAttributes; for each, the blob for a GuidAttribute is
    # prolog 0x0001 then a 4-byte-length name string then the 11 + 1 + 1 + 8 bytes of GUID-ish data.
    # We key on TypeDef name instead.

    type_names = {}
    for i in range(rows_td):
        vals = read_row(TYPEDEF, i)
        # TypeDef: [flags, name(str idx), namespace(str idx), extends(coded), fieldList, methodList]
        nm = read_string(streams, vals[1])
        ns = read_string(streams, vals[2])
        type_names[i + 1] = (ns + "." + nm) if ns else nm

    results = {}
    for i in range(rows_ca):
        vals = read_row(CUSTOMATTRIBUTE, i)
        parent = vals[0]
        tag = parent & 0x1F
        # HasCustomAttribute tag 0 = TypeDef, 1 = Field, 2 = MethodDef, ... The WinRT metadata
        # generator puts `GuidAttribute` on the *interface* TypeDef, so tag 0 is the one we want.
        if tag != 0:
            continue
        owner_row = parent >> 5
        raw = read_blob(streams, vals[2])
        # GuidAttribute blob: prolog(2)=0x0001, then a SerString name, then the GUID.
        if len(raw) < 3 + 32:
            continue
        if raw[0] != 1 or raw[1] != 0:
            continue
        # SerString: compressed length then bytes
        p = 2
        first = raw[p]
        if first & 0x80 == 0:
            n = first
            p += 1
        elif first & 0xC0 == 0x80:
            n = ((first & 0x3F) << 8) | raw[p + 1]
            p += 2
        else:
            n = ((first & 0x1F) << 24) | (raw[p + 1] << 16) | (raw[p + 2] << 8) | raw[p + 3]
            p += 4
        name = raw[p:p + n].decode("utf-8", "replace")
        p += n
        if name != "GuidAttribute":
            continue
        # number of named args
        n_named = struct.unpack_from("<H", raw, p)[0]
        p += 2
        # value: fixed arg = the 16-byte GUID, serialized as 11 bytes + 2 + 1 + 8
        g = raw[p:p + 16]
        if len(g) < 16:
            continue
        results[owner_row] = format_guid(g)

    targets = want if want else sorted(type_names.values())
    for row, guid in sorted(results.items()):
        nm = type_names.get(row, "?row%d" % row)
        if want and nm not in want:
            continue
        print("%-70s %s" % (nm, guid))

    if want:
        found = set()
        for row, guid in results.items():
            found.add(type_names.get(row))
        for w in want:
            if w not in found:
                print("MISSING  %s" % w)


if __name__ == "__main__":
    main()
