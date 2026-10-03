"""Ask csc.exe directly which WP8.1 APIs exist.

Metadata probes are unreliable here: the #Strings heap uses suffix sharing, so a
plain heap walk reports "WindowActivatedEventArgs" as missing even when it is a
suffix of "IWindowActivatedEventArgs". The compiler is the ground truth, so each
candidate snippet is compiled on its own with the exact reference set the real
build uses (see the XamlPreCompile Csc task in the detailed MSBuild log).
"""
import glob
import os
import subprocess
import sys

CSC = r"C:\Program Files (x86)\MSBuild\12.0\bin\Csc.exe"
PHONE_WINMD = (r"C:\Program Files (x86)\Windows Phone Kits\8.1\References"
               r"\CommonConfiguration\Neutral\Windows.winmd")
PLATFORM_WINMD = (r"C:\Program Files (x86)\Microsoft SDKs\WindowsPhoneApp\v8.1"
                  r"\ExtensionSDKs\Microsoft.VCLibs\12.0\References"
                  r"\CommonConfiguration\neutral\platform.winmd")
FX_DIR = (r"C:\Program Files (x86)\Reference Assemblies\Microsoft\Framework"
          r"\WindowsPhoneApp\v8.1")
WIN2D = (r"E:\Users\admin\Documents\visual studio 2013\Phirura\Phirura\packages"
         r"\Win2D.win81.1.21.0\bin\Phone\ARM")

OUT = r"E:\_phirura_probe"
SRC = os.path.join(OUT, "probe.cs")


def references():
    refs = [PHONE_WINMD, PLATFORM_WINMD]
    refs += sorted(glob.glob(os.path.join(FX_DIR, "*.dll")))
    refs += [os.path.join(WIN2D, "Microsoft.Graphics.Canvas.winmd"),
             os.path.join(WIN2D, "Microsoft.Graphics.Canvas.Numerics.dll")]
    return [r for r in refs if os.path.exists(r)]


def compile_snippet(body):
    os.makedirs(OUT, exist_ok=True)
    with open(SRC, "w", encoding="utf-8") as f:
        f.write("using System;\nusing Windows.UI.Xaml;\nusing Windows.UI.Xaml.Controls;\n"
                "using Windows.ApplicationModel;\n"
                "namespace Probe { public static class P { %s } }\n" % body)
    args = [CSC, "/noconfig", "/nostdlib+", "/nologo", "/warn:0", "/target:library",
            "/platform:arm", "/define:DEBUG;TRACE;NETFX_CORE;WINDOWS_PHONE_APP",
            "/out:" + os.path.join(OUT, "probe.dll"), SRC]
    args += ["/reference:" + r for r in references()]
    p = subprocess.run(args, capture_output=True, text=True, errors="replace")
    out = (p.stdout or "") + (p.stderr or "")
    lines = [l.strip() for l in out.splitlines() if "error" in l or "warning" in l]
    return p.returncode, lines


CANDIDATES = [
    ("WindowActivatedEventArgs type",
     "static void H(object s, WindowActivatedEventArgs e) { }"),
    ("Window.Activated += lambda(s,e)",
     "static void M() { Window.Current.Activated += (s, e) => { }; }"),
    ("Window.Activated += method(object,object)",
     "static void H(object s, object e) { }\nstatic void M() { Window.Current.Activated += H; }"),
    ("WindowActivatedEventHandler delegate named",
     "static void M() { WindowActivatedEventHandler h = OnA; Window.Current.Activated += h; }\n"
     "static void OnA(object s, WindowActivatedEventArgs e) { }"),
    ("Window.VisibilityChanged += lambda(s,e)",
     "static void M() { Window.Current.VisibilityChanged += (s, e) => { }; }"),
    ("VisibilityChangedEventArgs type",
     "static void H(object s, VisibilityChangedEventArgs e) { }"),
    ("Application.Resuming += (object,object)",
     "static void H(object s, object e) { }\nstatic void M() { Application.Current.Resuming += H; }"),
    ("Application.Resuming += (object,ResumingEventArgs)",
     "static void H(object s, ResumingEventArgs e) { }\nstatic void M() { Application.Current.Resuming += H; }"),
    ("ResumingEventHandler named",
     "static void M() { ResumingEventHandler h = OnR; Application.Current.Resuming += h; }\n"
     "static void OnR(object s, object e) { }"),
    ("Application.Suspending += SuspendingEventArgs",
     "static void H(object s, SuspendingEventArgs e) { }\nstatic void M() { Application.Current.Suspending += H; }"),
    ("Window.Current.CoreWindow.Activated (object,object)",
     "static void H(object s, object e) { }\n"
     "static void M() { Window.Current.CoreWindow.Activated += H; }"),
    ("CoreWindowActivationState enum",
     "static void H(object s, object e) { var st = (Windows.UI.Core.CoreWindowActivationState)e; }"),
    ("StatusBar.HideAsync",
     "static void M() { var t = Windows.UI.ViewManagement.StatusBar.GetForCurrentView().HideAsync(); }"),
    ("Window.SizeChanged",
     "static void M() { Window.Current.SizeChanged += (s, e) => { }; }"),
]


def compile_file(path):
    """Compile a standalone .cs file with the WP8.1 reference set."""
    args = [CSC, "/noconfig", "/nostdlib+", "/nologo", "/warn:0", "/target:library",
            "/platform:arm", "/define:DEBUG;TRACE;NETFX_CORE;WINDOWS_PHONE_APP",
            "/out:" + os.path.join(OUT, "file.dll"), path]
    args += ["/reference:" + r for r in references()]
    p = subprocess.run(args, capture_output=True, text=True, errors="replace")
    out = (p.stdout or "") + (p.stderr or "")
    return p.returncode, [l.strip() for l in out.splitlines()
                          if "error" in l or "warning" in l]


def main():
    if len(sys.argv) > 2 and sys.argv[1] == "--file":
        # Accept either a full path or a bare name; MSYS path translation mangles
        # drive-letter paths on the way in, so resolving against OUT is the safe route.
        path = sys.argv[2]
        if not os.path.exists(path):
            path = os.path.join(OUT, os.path.basename(path))
        code, lines = compile_file(path)
        print("%-5s %s" % ("OK" if code == 0 else "FAIL", path))
        for l in lines[:12]:
            print("   ", l.encode("latin-1", "replace").decode("gbk", "replace"))
        return

    only = sys.argv[1] if len(sys.argv) > 1 else None
    print("references:", len(references()))
    for name, body in CANDIDATES:
        if only and only not in name:
            continue
        code, lines = compile_snippet(body)
        print("%-5s %s" % ("OK" if code == 0 else "FAIL", name))
        for l in lines[:3]:
            print("        " + l)


if __name__ == "__main__":
    main()
