#!/usr/bin/env python3
r"""Retarget Rust's `std` Windows PAL for Windows Phone 8.1.

The approach
------------
`std` declares every Win32 entry point with
`windows_link::link!("<dll>" "system" fn NAME(...) -> RET);`, which expands to an
`#[link(kind = "raw-dylib")]` extern block. That bakes the DLL name into the import table,
and WP8.1 has no `ntdll.dll` and no UCRT.

Rather than fork the PAL, this script rewrites exactly the 20 declarations whose DLLs WP8.1
does not expose into **plain externs with no link attribute**. They then become ordinary
undefined symbols, which `compat/` (a small Rust crate in this directory) implements on top
of the API set that WP8.1 *does* have — see `Windows Phone Kits\8.1\lib\ARM\`.

Everything else in `std` is left untouched, so the patch stays mechanical and reviewable.

Usage
-----
    python wp81_std_patch.py --check      # report which bindings would be rewritten
    python wp81_std_patch.py --apply      # copy library/ to work/ and rewrite it
"""

import argparse
import os
import re
import shutil
import sys

RUSTUP_HOME = os.environ.get(
    "RUSTUP_HOME", r"C:\Users\admin\.workbuddy-ai\binaries\rust\rustup")
TOOLCHAIN = "nightly-x86_64-pc-windows-msvc"

HERE = os.path.dirname(os.path.abspath(__file__))
_SRC_ROOT = os.path.join(
    RUSTUP_HOME, "toolchains", TOOLCHAIN, "lib", "rustlib", "src", "rust")
# build-std.ps1 swaps `library` for a junction onto work/library while it builds, so the
# pristine sources have to be read from `library.orig` when that swap is in place - otherwise
# the patcher would read its own output and find nothing left to do.
SRC_LIBRARY = os.path.join(_SRC_ROOT, "library.orig")
if not os.path.isdir(SRC_LIBRARY):
    SRC_LIBRARY = os.path.join(_SRC_ROOT, "library")
WORK_LIBRARY = os.path.join(HERE, "work", "library")

# ---------------------------------------------------------------------------
# The 20 symbols WP8.1 does not provide, and why.
#
# Measured by comparing a `-Z build-std=std` staticlib's 181 `__imp_*` imports against every
# symbol in `Windows Phone Kits\8.1\lib\ARM\*.lib` plus
# `Microsoft Visual Studio 11.0\VC\lib\arm\*.lib`.
# ---------------------------------------------------------------------------
UNSUPPORTED = {
    # api-ms-win-core-console-* is not in the WP8.1 API set. WP8.1 has no console at all.
    "GetConsoleMode", "GetConsoleOutputCP", "ReadConsoleW", "WriteConsoleW",
    "GetStdHandle", "SetStdHandle",
    # api-ms-win-core-processenv-* is not in the WP8.1 API set.
    "GetCommandLineW", "GetEnvironmentStringsW", "FreeEnvironmentStringsW",
    # ntdll is not importable from a WP8.1 app. The Win32 file API is
    # (api-ms-win-core-file-l1-2-1), so these translate onto it.
    "NtCreateFile", "NtOpenFile", "NtReadFile", "NtWriteFile",
    "NtSetInformationFile", "NtCreateNamedPipeFile", "RtlNtStatusToDosError",
    # Absent / different name on WP8.1.
    "ProcessPrng", "WaitForMultipleObjects", "GetUserProfileDirectoryW",
    # Not exported by any WP8.1 API set, so compat supplies them instead of the remap.
    "LoadLibraryA", "lstrlenW",
    # Confirmed missing on WP8.1 by wp81_probe_api_sets(): the set exists but does not
    # export it. Calling the resulting null pointer is what aborted the process.
    "GetActiveProcessorCount",
}

# DLLs whose `raw-dylib` binding must be dropped so the symbol resolves from compat/ instead.
# `bcryptprimitives.dll` does exist on WP8.1, but the SDK ships no import library for it and
# `raw-dylib` would bake the name in, so it is handled the same way as the rest.
#
# Deliberately NOT including advapi32: the only reference is inside windows_link's macro
# definition, guarded by `target_vendor = "win7"`, which our target never matches. Leaving it
# alone keeps the patch to the files that matter.
STRIP_LINK_DLLS = ("bcryptprimitives", "ntdll", "userenv")

# ---------------------------------------------------------------------------
# kernel32.dll -> the WP8.1 API set that actually exports the function.
#
# `std` binds ~107 functions to kernel32.dll because that is what `windows-link` chose. WP8.1
# apps must not import KERNEL32.dll (Win2D, which loads on the same device, imports it zero
# times), but rustc's `raw-dylib` **emits the import descriptor itself and needs no import
# library** - which is why std's GetModuleHandleA already lands in
# api-ms-win-core-libraryloader-l1-2-0.dll with no .lib anywhere. So the fix is purely textual:
# rewrite the DLL name inside the declaration.
#
# Only sets confirmed present on WP8.1 are used here. The list comes from
# WindowsPhoneCore.lib's forward table, which is the platform's own definition of what exists.
# Anything NOT in this table has no API set at all and is stubbed by compat/ instead.
# ---------------------------------------------------------------------------
API_SETS = {
    "libraryloader": "api-ms-win-core-libraryloader-l1-2-0.dll",
    "string":        "api-ms-win-core-string-l1-1-0.dll",
    "errorhandling": "api-ms-win-core-errorhandling-l1-1-2.dll",
    "handle":        "api-ms-win-core-handle-l1-1-0.dll",
    "heap":          "api-ms-win-core-heap-l1-2-0.dll",
    "file":          "api-ms-win-core-file-l1-2-1.dll",
    "file2":         "api-ms-win-core-file-l2-1-1.dll",
    "processthreads": "api-ms-win-core-processthreads-l1-1-2.dll",
    "synch":         "api-ms-win-core-synch-l1-2-0.dll",
    "profile":       "api-ms-win-core-profile-l1-1-0.dll",
    "localization":  "api-ms-win-core-localization-l1-2-1.dll",
    "sysinfo":       "api-ms-win-core-sysinfo-l1-2-1.dll",
    "fibers":        "api-ms-win-core-fibers-l1-1-1.dll",
    "rtlsupport":    "api-ms-win-core-rtlsupport-l1-2-0.dll",
    "util":          "api-ms-win-core-util-l1-1-0.dll",
    "debug":         "api-ms-win-core-debug-l1-1-1.dll",
    "memory":        "api-ms-win-core-memory-l1-1-2.dll",
    "interlocked":   "api-ms-win-core-interlocked-l1-2-0.dll",
    "io":            "api-ms-win-core-io-l1-1-1.dll",
}

# function -> API set key. Anything absent here and absent from UNSUPPORTED is left alone.
KERNEL32_REMAP = {
    # --- libraryloader ---
    "LoadLibraryW": "libraryloader",
    "LoadLibraryExW": "libraryloader", "LoadLibraryExA": "libraryloader",
    "FreeLibrary": "libraryloader", "GetProcAddress": "libraryloader",
    "GetModuleHandleW": "libraryloader", "GetModuleHandleA": "libraryloader",
    "GetModuleHandleExW": "libraryloader", "GetModuleFileNameW": "libraryloader",
    "GetModuleFileNameA": "libraryloader", "DisableThreadLibraryCalls": "libraryloader",
    # --- string ---
    # --- errorhandling ---
    "GetLastError": "errorhandling", "SetLastError": "errorhandling",
    "FormatMessageW": "errorhandling", "FormatMessageA": "errorhandling",
    # --- handle ---
    "CloseHandle": "handle", "DuplicateHandle": "handle",
    "SetHandleInformation": "handle", "GetHandleInformation": "handle",
    # --- heap ---
    "HeapAlloc": "heap", "HeapFree": "heap", "HeapReAlloc": "heap",
    "HeapSize": "heap", "HeapCreate": "heap", "HeapDestroy": "heap",
    "GetProcessHeap": "heap",
    # --- file ---
    "CreateFileW": "file", "ReadFile": "file", "WriteFile": "file",
    "ReadFileEx": "file", "WriteFileEx": "file", "FlushFileBuffers": "file",
    "SetFilePointerEx": "file", "SetEndOfFile": "file", "GetFileType": "file",
    "GetFileSizeEx": "file", "SetFileTime": "file", "GetFileAttributesW": "file",
    "SetFileAttributesW": "file", "DeleteFileW": "file", "MoveFileExW": "file",
    "CopyFileExW": "file", "CreateDirectoryW": "file", "RemoveDirectoryW": "file",
    "FindFirstFileExW": "file", "FindNextFileW": "file", "FindClose": "file",
    "GetFullPathNameW": "file", "GetFinalPathNameByHandleW": "file",
    "GetFileInformationByHandle": "file", "LockFileEx": "file", "UnlockFile": "file",
    "GetOverlappedResult": "file", "CancelIo": "io", "DeviceIoControl": "io",
    "GetTempPathW": "file", "SetFileInformationByHandle": "file",
    "CreateHardLinkW": "file", "CreateSymbolicLinkW": "file",
    "AreFileApisANSI": "file", "GetLogicalDrives": "file",
    "GetFileInformationByHandleEx": "file2",
    # --- processthreads ---
    "CreateThread": "processthreads", "ExitProcess": "processthreads",
    "ExitThread": "processthreads", "GetCurrentProcess": "processthreads",
    "GetCurrentProcessId": "processthreads", "GetCurrentThread": "processthreads",
    "GetCurrentThreadId": "processthreads", "GetExitCodeProcess": "processthreads",
    "TerminateProcess": "processthreads", "GetProcessId": "processthreads",
    "SetThreadStackGuarantee": "processthreads",
    "InitializeProcThreadAttributeList": "processthreads",
    "UpdateProcThreadAttribute": "processthreads",
    "DeleteProcThreadAttributeList": "processthreads",
    "TlsAlloc": "processthreads", "TlsFree": "processthreads",
    "TlsGetValue": "processthreads", "TlsSetValue": "processthreads",
    # --- synch ---
    "WaitForSingleObject": "synch", "WaitForSingleObjectEx": "synch",
    "WaitForMultipleObjects": "synch", "Sleep": "synch", "SleepEx": "synch",
    "SwitchToThread": "synch", "CreateEventW": "synch", "CreateEventA": "synch",
    "CreateMutexW": "synch", "CreateMutexA": "synch", "ReleaseMutex": "synch",
    "SetWaitableTimer": "synch", "CreateWaitableTimerExW": "synch",
    "InitializeCriticalSectionAndSpinCount": "synch",
    "EnterCriticalSection": "synch", "LeaveCriticalSection": "synch",
    "DeleteCriticalSection": "synch",
    "AcquireSRWLockExclusive": "synch", "AcquireSRWLockShared": "synch",
    "ReleaseSRWLockExclusive": "synch", "ReleaseSRWLockShared": "synch",
    "TryAcquireSRWLockExclusive": "synch", "TryAcquireSRWLockShared": "synch",
    "SleepConditionVariableSRW": "synch", "WakeConditionVariable": "synch",
    "WakeAllConditionVariable": "synch",
    "InitOnceBeginInitialize": "synch", "InitOnceComplete": "synch",
    # --- profile / sysinfo / localization ---
    "QueryPerformanceCounter": "profile", "QueryPerformanceFrequency": "profile",
    "GetSystemTimeAsFileTime": "sysinfo", "GetSystemTimePreciseAsFileTime": "sysinfo",
    "GetTickCount64": "sysinfo", "GetSystemInfo": "sysinfo",
    "GetSystemDirectoryW": "sysinfo", "GetWindowsDirectoryW": "sysinfo",
    "GetACP": "localization", "GetOEMCP": "localization", "GetCPInfo": "localization",
    "IsValidCodePage": "localization", "LCMapStringEx": "localization",
    "MultiByteToWideChar": "string", "WideCharToMultiByte": "string",
    "GetStringTypeW": "string", "CompareStringOrdinal": "string",
    # --- fibers / rtl / util / debug / memory ---
    "FlsAlloc": "fibers", "FlsFree": "fibers",
    "FlsGetValue": "fibers", "FlsSetValue": "fibers", "IsThreadAFiber": "fibers",
    "RaiseException": "debug", "RtlUnwindEx": "rtlsupport",
    "RtlCaptureContext": "rtlsupport",
    "DecodePointer": "util", "EncodePointer": "util",
    "IsDebuggerPresent": "debug", "OutputDebugStringW": "debug",
    "OutputDebugStringA": "debug",
    "VirtualAlloc": "memory", "VirtualFree": "memory",
    "VirtualProtect": "memory", "VirtualQuery": "memory",
}

# `windows_link::link!("dll" "system" fn NAME(args) -> RET);`
#
# NOTE: deliberately NOT parsed with a regex. The declarations contain nested parentheses,
# span several lines, and may or may not carry a return type; every regex tried here either
# stopped at a nested `)` or swallowed the declaration's own closing paren. `rewrite()` scans
# for the balanced argument group instead.

# Minimum number of declarations a successful --apply must rewrite. If fewer are found the
# patcher aborts rather than reporting success on an unpatched tree - a false green here is
# worse than a red build, because it silently invalidates whatever is built afterwards.
MIN_EXPECTED_REWRITES = 17


def _force_rmtree(func, path, _exc):
    """shutil.rmtree onerror hook: clear the read-only bit and retry."""
    try:
        os.chmod(path, 0o666)
        func(path)
    except OSError:
        pass


def strip_link_attrs(text, dlls=STRIP_LINK_DLLS):
    """Remove `#[cfg_attr(..., link(name = "<dll>", kind = "raw-dylib", ...))]` attributes.

    Scans for balanced parentheses rather than regexing, because the attribute body contains
    nested brackets and spans several lines.
    """
    out = text
    removed = 0
    i = 0
    while True:
        i = out.find("#[cfg_attr(", i)
        if i < 0:
            break
        j = out.find(")]", i)
        if j < 0:
            break
        body = out[i:j + 2]
        if any('link(name = "%s"' % d in body for d in dlls):
            end = j + 2
            while end < len(out) and out[end] in "\r\n":
                end += 1
            out = out[:i] + out[end:]
            removed += 1
            continue
        i = j + 2
    return out, removed


def _skip_ws(text, i):
    while i < len(text) and text[i] in " \t\r\n":
        i += 1
    return i


def _read_str(text, i):
    """Read a Rust string literal at `i`. Returns (value, index_after) or (None, i)."""
    if i >= len(text) or text[i] != '"':
        return None, i
    j = text.find('"', i + 1)
    if j < 0:
        return None, i
    return text[i + 1:j], j + 1


def _matching_paren(text, i):
    """`text[i]` must be '('. Returns the index just past the matching ')', or -1."""
    if i >= len(text) or text[i] != "(":
        return -1
    depth = 0
    while i < len(text):
        c = text[i]
        if c == "(":
            depth += 1
        elif c == ")":
            depth -= 1
            if depth == 0:
                return i + 1
        i += 1
    return -1


def rewrite(text, only_names=None):
    """Turn selected `windows_link::link!()` declarations into plain externs.

    Deliberately a hand-written scanner rather than a regex. The declarations contain nested
    parentheses, span multiple lines, and may or may not have a return type, and a regex that
    handles all three turned out to be both hard to read and wrong. Scanning for the balanced
    argument group is exact.
    """
    changed = []
    remapped = []
    sigs = []
    out = []
    marker = "windows_link::link!("
    i = 0

    while True:
        j = text.find(marker, i)
        if j < 0:
            out.append(text[i:])
            break
        out.append(text[i:j])

        k = j + len(marker)
        dll, k = _read_str(text, k)
        k = _skip_ws(text, k)
        abi, k = _read_str(text, k) if dll is not None else (None, k)
        k = _skip_ws(text, k)

        if dll is None or abi != "system" or not text.startswith("fn", k):
            # Not a shape we understand - emit it verbatim and carry on.
            out.append(text[j:k])
            i = k
            continue

        k = _skip_ws(text, k + 2)
        name_start = k
        while k < len(text) and (text[k].isalnum() or text[k] == "_"):
            k += 1
        name = text[name_start:k]

        open_paren = _skip_ws(text, k)
        end_args = _matching_paren(text, open_paren)
        if not name or end_args < 0:
            out.append(text[j:end_args if end_args > 0 else k])
            i = end_args if end_args > 0 else k
            continue

        # Optional `-> RET`, then the terminating ';'.
        k = _skip_ws(text, end_args)
        ret = ""
        if text.startswith("->", k):
            k = _skip_ws(text, k + 2)
            rs = k
            # Read to the closing ')' or ';'. NOT an alphanumeric-only loop: return types can
            # contain '*' and spaces, e.g. `*mut c_void`. Restricting the character class meant
            # every pointer-returning declaration was silently rejected - which is exactly how
            # HeapAlloc and HeapReAlloc stayed bound to kernel32.dll and sank the device load.
            while k < len(text) and text[k] not in ");":
                k += 1
            ret = " -> " + text[rs:k].strip()
            k = _skip_ws(text, k)
        # The macro wraps the whole declaration, so the shape is
        #     link!( "dll" "system" fn NAME(args) [-> RET] ) ;
        # - note the closing ')' of the macro invocation before the ';'. Missing that was why
        # every declaration was rejected and the patcher silently reported zero rewrites.
        k = _skip_ws(text, k)
        if k < len(text) and text[k] == ")":
            k = _skip_ws(text, k + 1)
        if k >= len(text) or text[k] != ";":
            out.append(text[j:k])
            i = k
            continue
        k += 1  # consume ';'

        if name in UNSUPPORTED and (only_names is None or name in only_names):
            args = text[open_paren:end_args]
            changed.append((name, dll))
            out.append('unsafe extern "system" {{ pub fn {}{}{}; }}'.format(name, args, ret))
        elif (dll == "kernel32.dll" and name in KERNEL32_REMAP) \
                or dll.startswith("api-ms-win-"):
            # Emit a PLAIN extern, not a raw-dylib import.
            #
            # Pointing the declaration at an API set works right up until one of the ~143
            # hand-written set names is wrong for the target platform - and then the DLL fails
            # to load with ERROR_PROC_NOT_FOUND, naming no symbol. Desktop Windows exports more
            # than WP8.1 does, so local verification cannot rule that out.
            #
            # A plain extern moves the resolution to run time: compat supplies a trampoline that
            # does LoadLibraryW + GetProcAddress, so a bad name becomes a logged null instead of
            # a load-time failure. gen_api_trampolines.py emits those trampolines from the same
            # signatures, so the two sides cannot drift apart.
            args = text[open_paren:end_args]
            newdll = API_SETS[KERNEL32_REMAP[name]] if dll == "kernel32.dll" else dll
            remapped.append((name, newdll))
            sigs.append((name, newdll, args, ret))
            out.append('unsafe extern "system" {{ pub fn {}{}{}; }}'.format(name, args, ret))
        else:
            out.append(text[j:k])
        i = k

    return "".join(out), changed, remapped, sigs


def main(argv):
    ap = argparse.ArgumentParser()
    ap.add_argument("--check", action="store_true", help="report without writing")
    ap.add_argument("--apply", action="store_true", help="copy library/ and rewrite it")
    args = ap.parse_args(argv[1:])

    if not os.path.isdir(SRC_LIBRARY):
        print("rust-src library not found at:\n  " + SRC_LIBRARY, file=sys.stderr)
        print("Install it with:  rustup component add rust-src --toolchain nightly", file=sys.stderr)
        return 1

    targets = []
    for root, _dirs, files in os.walk(SRC_LIBRARY):
        for f in files:
            if f.endswith(".rs"):
                targets.append(os.path.join(root, f))

    hits = {}
    remap_hits = {}
    attr_hits = 0
    for p in targets:
        try:
            with open(p, "r", encoding="utf-8") as fh:
                text = fh.read()
        except (UnicodeDecodeError, OSError):
            continue
        if "windows_link::link!" not in text and "raw-dylib" not in text:
            continue
        _, changed, remapped, _sigs = rewrite(text)
        for name, dll in changed:
            hits.setdefault(name, set()).add(dll)
        for name, newdll in remapped:
            remap_hits.setdefault(name, set()).add(newdll)
        _, n = strip_link_attrs(text)
        attr_hits += n

    print("bindings that would be rewritten: %d of the %d unsupported symbols"
          % (len(hits), len(UNSUPPORTED)))
    for name in sorted(hits):
        print("  %-26s from %s" % (name, ", ".join(sorted(hits[name]))))
    print("kernel32 bindings that would be re-pointed at a WP8.1 API set: %d"
          % len(remap_hits))
    print("raw-dylib attributes that would be stripped: %d" % attr_hits)
    missing = sorted(UNSUPPORTED - set(hits))
    if missing:
        print("\nNOT FOUND in the bindings (investigate before trusting the patch):")
        for name in missing:
            print("  " + name)

    if not args.apply:
        return 0

    if not os.path.isdir(WORK_LIBRARY):
        # First run: clone the whole library tree. Subsequent runs refresh only the files we
        # touch, which keeps this idempotent without ever deleting a large tree.
        shutil.copytree(SRC_LIBRARY, WORK_LIBRARY)
        for root, _dirs, files in os.walk(WORK_LIBRARY):
            for f in files:
                try:
                    os.chmod(os.path.join(root, f), 0o666)
                except OSError:
                    pass

    total = 0
    total_remapped = 0
    total_attrs = 0
    for p in targets:
        rel = os.path.relpath(p, SRC_LIBRARY)
        dst = os.path.join(WORK_LIBRARY, rel)
        with open(p, "r", encoding="utf-8") as fh:
            text = fh.read()
        if "windows_link::link!" not in text and "raw-dylib" not in text:
            continue
        new_text, changed, remapped, _sigs = rewrite(text)
        new_text, n_attrs = strip_link_attrs(new_text)
        # Always write the candidate file, even when nothing changed: the output is then a pure
        # function of the pristine source, so the work tree is self-healing after a bad run
        # instead of accumulating a mixture of old and new transformations.
        if os.path.exists(dst):
            try:
                os.chmod(dst, 0o666)
            except OSError:
                pass
        else:
            os.makedirs(os.path.dirname(dst), exist_ok=True)
        with open(dst, "w", encoding="utf-8", newline="") as fh:
            fh.write(new_text)
        total += len(changed)
        total_remapped += len(remapped)
        total_attrs += n_attrs
        print("patched %-46s %d freed, %d remapped, %d attr(s)"
              % (rel, len(changed), len(remapped), n_attrs))

    print("\nrewrote %d binding(s), re-pointed %d kernel32 binding(s) at WP8.1 API sets,\n"
          "stripped %d attribute(s) into %s"
          % (total, total_remapped, total_attrs, WORK_LIBRARY))

    if total < MIN_EXPECTED_REWRITES:
        print("\nERROR: only %d declaration(s) rewritten, expected at least %d.\n"
              "       The tree at %s is probably NOT patched, so anything built from it is\n"
              "       unpatched std. Check that the sources were read from the pristine tree\n"
              "       (a `library.orig` swap left in place by `build-std.ps1 -KeepSwapped` can\n"
              "       make the patcher read its own output)."
              % (total, MIN_EXPECTED_REWRITES, WORK_LIBRARY), file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
