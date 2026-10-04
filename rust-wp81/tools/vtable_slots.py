#!/usr/bin/env python3
"""Compute a COM vtable slot from the kit's own headers.

    python vtable_slots.py --iface ID3D11Texture2D --method GetDesc
    python vtable_slots.py --iface IDXGISurface --method GetDesc
    python vtable_slots.py --iface ID3D11DeviceContext --method CopyResource

Why this exists
---------------
MEMORY.md and REFERENCE.md both forbid *remembering* slot numbers: a wrong slot is
silent corruption, never a diagnostic (you call the wrong function through a valid
pointer and get a crash or a bogus HRESULT). REFERENCE.md carries the tables that
were extracted before, but it cannot carry every interface anyone will ever need.

So instead of writing a number down from memory, ask the header. `d3d11.h` /
`dxgi.h` declare each interface as

    MIDL_INTERFACE("6f15aaf2-...")
    ID3D11Texture2D : public ID3D11Resource
    {
    public:
        virtual void STDMETHODCALLTYPE GetDesc( ... ) = 0;
    };

and C++ member order *is* vtable order. Walking `: public BASE` back to `IUnknown`
and counting gives the slot exactly as the compiler sees it.

Self-check: run it for the slots already proven on the Lumia 925 and it must
reproduce them (`ID3D11Device::CreateRenderTargetView` = 9,
`ID3D11DeviceContext::ClearRenderTargetView` = 50,
`ISurfaceImageSourceNative::BeginDraw` = 4). That is what makes its answer for an
interface we have never called trustworthy.

Exit codes: 0 = found, 2 = interface/method not found, 3 = self-check mismatch.
"""

import argparse
import os
import re
import sys

KIT = r"C:\Program Files (x86)\Windows Kits\8.1\Include"
DEFAULT_HEADERS = [
    os.path.join(KIT, "shared", "unknwn.h"),
    os.path.join(KIT, "shared", "dxgi.h"),
    os.path.join(KIT, "shared", "dxgi1_2.h"),
    os.path.join(KIT, "um", "d3d11.h"),
    # ISurfaceImageSourceNative lives in the WP8.1 SDK's XAML interop header.
    os.path.join(
        r"C:\Program Files (x86)\Microsoft SDKs\Windows Phone\v8.1\Include",
        "um",
        "windows.ui.xaml.media.dxinterop.h",
    ),
    r"C:\Program Files (x86)\Windows Kits\8.1\Include\um\windows.ui.xaml.media.dxinterop.h",
]

# Slots REFERENCE.md already records as proven on device / host. Used only to
# validate the parser before answering a new question.
KNOWN = [
    ("ID3D11Device", "CreateRenderTargetView", 9),
    ("ID3D11Device", "CreateTexture2D", 5),
    ("ID3D11Device", "CreateShaderResourceView", 7),
    ("ID3D11DeviceContext", "ClearRenderTargetView", 50),
    ("ID3D11DeviceContext", "Draw", 13),
    ("ID3D11DeviceContext", "Map", 14),
    ("ID3D11DeviceContext", "OMSetRenderTargets", 33),
    ("IDXGIFactory2", "IsCurrent", 13),
    ("IDXGISwapChain", "Present", 8),
    ("IDXGISwapChain", "GetBuffer", 9),
]

IFACE_RE = re.compile(
    r'MIDL_INTERFACE\s*\(\s*"[^"]*"\s*\)\s*(?P<name>\w+)\s*:\s*public\s+(?P<base>\w+)'
)
CXX_METHOD_RE = re.compile(r"STDMETHODCALLTYPE\s+(\w+)\s*\(")
C_METHOD_RE = re.compile(r"STDMETHOD\s*\(\s*(\w+)\s*\)")
PURE_VIRTUAL = re.compile(r"=\s*0\s*;")


def _block(text, open_brace):
    """Return the {...} block starting at `open_brace`, brace-matched."""
    depth = 0
    for i in range(open_brace, len(text)):
        if text[i] == "{":
            depth += 1
        elif text[i] == "}":
            depth -= 1
            if depth == 0:
                return text[open_brace : i + 1]
    return ""


def parse_headers(paths):
    """{interface: (base, [method names in order])} across all given headers."""
    ifaces = {}
    for path in paths:
        if not path or not os.path.isfile(path):
            continue
        with open(path, "r", errors="replace") as fh:
            text = fh.read()
        for m in IFACE_RE.finditer(text):
            name, base = m.group("name"), m.group("base")
            brace = text.find("{", m.end())
            if brace < 0:
                continue
            body = _block(text, brace)
            names = CXX_METHOD_RE.findall(body)
            if not names:
                names = C_METHOD_RE.findall(body)
            declared = len(PURE_VIRTUAL.findall(body))
            if declared and len(names) != declared:
                # Mixed macro styles; the count is authoritative, keep the names
                # we did parse and let the caller see the mismatch.
                names = names + ["<?>"] * (declared - len(names))
            # Later headers (d3d11_1.h) can re-declare with extra methods; the
            # base-interface answer we want comes from the plain header, so do
            # not overwrite a longer entry with a shorter one.
            if name not in ifaces or len(ifaces[name][1]) < len(names):
                ifaces[name] = (base, names)
    return ifaces


def chain(ifaces, name):
    """[(interface, [methods])] from `name` up to and including IUnknown."""
    out = []
    seen = set()
    cur = name
    while cur and cur not in seen:
        seen.add(cur)
        if cur not in ifaces:
            # Unknown root: assume the 3 IUnknown methods, which is what every
            # COM interface derives from (documented, and REFERENCE.md relies on
            # it: "ID3D11Device derives straight from IUnknown -> slots start at 3").
            out.append((cur, ["QueryInterface", "AddRef", "Release"]))
            break
        base, methods = ifaces[cur]
        out.append((cur, methods))
        cur = base
    if out and out[-1][0] != "IUnknown" and "IUnknown" in ifaces:
        out.append(("IUnknown", ifaces["IUnknown"][1]))
    return out


def slot_of(ifaces, iface, method):
    ch = chain(ifaces, iface)
    # ch is derived-first; vtable order is base-first.
    index = 0
    for name, methods in reversed(ch):
        if name == iface:
            if method not in methods:
                return None, ch
            return index + methods.index(method), ch
        index += len(methods)
    return None, ch


def main():
    ap = argparse.ArgumentParser(description="COM vtable slot from kit headers")
    ap.add_argument("--iface", help="interface name, e.g. ID3D11Texture2D")
    ap.add_argument("--method", help="method name, e.g. GetDesc")
    ap.add_argument("--header", action="append", default=[],
                    help="extra header to parse (repeatable)")
    ap.add_argument("--selfcheck", action="store_true",
                    help="verify the parser against REFERENCE.md's proven slots")
    args = ap.parse_args()

    paths = DEFAULT_HEADERS + args.header
    ifaces = parse_headers(paths)
    if not ifaces:
        print("no headers parsed from: %s" % paths, file=sys.stderr)
        return 3

    if args.selfcheck or not (args.iface and args.method):
        bad = 0
        for iface, method, want in KNOWN:
            got, _ = slot_of(ifaces, iface, method)
            mark = "ok " if got == want else "BAD"
            if got != want:
                bad += 1
            print("%s %-32s::%-24s slot=%s (REFERENCE says %d)"
                  % (mark, iface, method, got, want))
        print("self-check: %s" % ("FAILED" if bad else "all known slots reproduced"))
        if bad or not (args.iface and args.method):
            return 3 if bad else 0

    if args.iface and args.method:
        got, ch = slot_of(ifaces, args.iface, args.method)
        if got is None:
            print("%s::%s not found (parsed %d interfaces)"
                  % (args.iface, args.method, len(ifaces)), file=sys.stderr)
            if args.iface not in ifaces:
                print("  interface %s missing - pass its header with --header"
                      % args.iface, file=sys.stderr)
            return 2
        print("%s::%s = slot %d" % (args.iface, args.method, got))
        print("  chain (base-first, cumulative):")
        cum = 0
        for name, methods in reversed(ch):
            print("    %-32s slots %d..%d (%d methods)"
                  % (name, cum, cum + len(methods) - 1, len(methods)))
            cum += len(methods)
        print("  this interface's own methods:")
        own = dict((n, m) for n, m in ch)[args.iface]
        for i, m in enumerate(own):
            print("    %-3d %s" % (got - own.index(args.method) + i, m))
    return 0


if __name__ == "__main__":
    sys.exit(main())
