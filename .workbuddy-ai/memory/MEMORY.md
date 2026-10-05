# Phirura — Phira → Windows Phone 8.1

Deploy log: `phirura.log`. **`REFERENCE.md` is the deep reference — vtable slots, IIDs, constants,
the full eliminated-hypothesis log, tool/env catalogues, and the R54 additions. Read it before
touching any slot number, GUID, or Win2D/thread-affinity rule.** Daily logs: `.workbuddy-ai/memory/`.

`Phirura/Phirura.sln` → `Phirura.Windows/` (Win8.1, AnyCPU), `Phirura.WindowsPhone/` (WP8.1, **ARM**;
real dir `Phirura/Phirura/Phirura.WindowsPhone/`), source-shared `Phirura.Shared` (shproj). Read-only
ref: `phira-main/phira-main/` (`prpr`, `prpr-*`, `prpr-port`). 2nd workstream: `rust-wp81/`.

**User-mandated direction: the real Rust game must run AND render on WP8.1.** Device: **Lumia 925 /
RM-910, Adreno 225, WDDM 1.2, FL 9.3 (`0x9300`)**, product id `fa2c9634-5ad8-487e-be3c-9893871fda9f`.
Test chart = `Phirura\190` (Transcendence), staged at `E:\_phirura_m1\charts\190`; that dir is a push
mirror of `phira-main/phira-main/assets` and **must stay in sync** (UI images come from there).

---

## Current state (R54)

**Stage 1 is DONE and on the phone** (R51–R52): textured quad renders, touch follows the finger,
~60 fps idle / 57–59 under load, ARM DLL 32,256 B, 6 shippable imports, `check_imports` exit 0.
Desktop gate `=== ALL STAGE 1 CHECKS PASSED ===` stayed green. **Stage 3's gate is open.**

**R53–R54: the real game boots on the phone.** `prpr` parses the 8.8 MB RPE chart, loads the resource
pack, registers every texture, and `GameScene` runs. Three reported symptoms, all root-caused:

1. **Gameplay drew nothing — the camera was wiped every frame.** `reset_frame()` in
   `rust-wp81/shims/macroquad/src/lib.rs` overwrote `CAM = CAM_OFF` and cleared the stacks, but prpr
   sets the camera **once** in `GameScene::enter` (`game.rs:851`) and only re-sets it in `render`
   when `update_size` sees the viewport change (`game.rs:1162`). From frame 2 on, `project` took the
   `!cam.active` branch and returned world coords — `vertBbox` logged `(-1,-0.6)-(1,0.6)` instead of
   pixels, so everything landed in a half-DIP box top-left. **Fix: `reset_frame` now drains the
   stacks by *popping* (`while … pop_camera()` / `pop_model()`)** — still undoes a leaked bracket but
   restores the value in force below it, matching `pop_camera`'s contract. **Never go back to
   overwriting CAM/MODEL there.**
2. **Loading-screen glyphs were solid white blocks.** `Win2DChartRenderer.cs` `PaintPortBand` fed
   Rust's straight alpha into `CanvasBitmap.CreateFromColors`, which stores **premultiplied** bytes
   with no conversion. **Fix: premultiply RGB by A on the way in.** (Same defect as the earlier
   "some images render white".)
3. **The result page replayed the chart song.** `AudioClip` returns `id: 0` for *every* clip, so
   `music_new` could not say *which* audio it wanted and the shell's single player always pointed
   back at `PhiraMusicUri`. **Fix: `music_new` carries `clip_secs`; C# classifies by duration** —
   `Math.Abs(clipSecs - PhiraMusicSeconds) > 1.0` ⇒ not the chart ⇒ play the res-pack `ending` clip.
   `PhiraMusicSeconds` is recorded in `OnMediaOpened`'s phira branch **only when `ActiveMusicUri ==
   PhiraMusicUri`**, so the later `ending.ogg` open cannot overwrite the chart length. The 1.0 s
   tolerance is deliberate: `NaturalDuration` and Rust's `frame_count/sample_rate` disagree by
   encoder padding, a *miss* costs the whole soundtrack, a false hit only plays the wrong BGM.

Also fixed: `stage-chart.ps1` used to `| Out-Null` ISETool's output, so all four log-pull failure
modes printed the same "no log"; it now captures and prints on failure.

**Standing decisions:** network / login / online charts are **implemented in C# and called from Rust**
(more `HostCallbacks` entries later — never `tokio`/`reqwest` in the Rust half; `std::thread` aborts
`0xC0000409` on device anyway). Playable chart = `190`, not the bundled demo.

---

## Rules that break again easily (full versions + rationale in `REFERENCE.md`)

**Build env (`build-prpr.ps1` / `build-port.ps1`):** `VC\bin` first on PATH (Git Bash's coreutils
`link` shadows the linker), `Common7\IDE` on PATH (`0xc0000135` without `mspdb110.dll`), `LIB` =
`VC\lib\amd64;winv6.3\um\x64`, `workdir = phira-main\phira-main`. **Never `$ErrorActionPreference =
"Stop"` with `2>&1`** — it turns cargo warnings into a `NativeCommandError` that hides the exit code.

**Rust↔shell ABI:** new `#[no_mangle]` entry points **silently do not appear** in the DLL —
`build.ps1`/`build-port.ps1` carry an explicit `/EXPORT:` list; verify by parsing the export table.
**Link ≠ emitted** (an unreferenced `extern` produces no import descriptor). **Never treat a passing
link as proof** — `rust-wp81/tools/check_imports.py` is the gate (0 = OK). `hostcheck.rs` compiles
the same `direct.rs`/`renderer.rs` for x64 against the real desktop D3D11 runtime — **anything
touching those files must pass `hostcheck` before deploy.**

**Build/deploy:** `build_wp.ps1 -Configuration Debug -Target wp` (WP head is **ARM, not AnyCPU** —
Win2D ships Phone arm/x86 only) — invoke **via the PowerShell tool** (direct MSBuild is
sandbox-blocked); `-Isolated` repoints the obj dir when a pdb handle is stuck. **Always build
`-Target win` after touching `Phirura.Shared/`** (without `-Isolated`). Rust DLL → copy to
`Phirura.WindowsPhone/`, then rebuild the WP head. Deploy `AppDeployCmd.exe /installlaunch <appx>
/targetdevice:de`; log `ISETool.exe ts de <productId> <dir>` **works iff `<dir>` does not exist**.

**C# app:** gameplay core = line-by-line C# port of `prpr` in `Phirura.Shared/`; WP head renders via
**vendored** Win2D (`Phirura/packages/Win2D.win81.1.21.0/`) — **referencing a Win2D type is what
packages the native DLL**. `CanvasControl`, **not** `CanvasAnimatedControl` (judge + HUD must stay on
the UI thread). **`edit` is flaky on `GamePage.xaml.cs`** — retry 1–4 times, keep `oldString` ASCII.

**Workspace safety: never call `os.remove` here** — the safe-delete guard intercepts it and once
**moved all 13 `Phirura.Shared/{Core,Parse,Game}` sources into `<ws>/trash/`**. Use `os.replace()`/
`mv`; if a build reports missing files, **check `<ws>/trash/` first**.

## R55 — the four symptoms after the first gameplay round

1. **Texture registry cap.** `TEX_SLOTS` was 128 and the chart needs 194; the log said
   `128 registered, 66 dropped`. **A dropped texture is invisible, never an error** — that is what
   "the images don't display" was. Now 512 slots (14 KB) and `TEX_MAX_BYTES` 64 MiB (the 32 MiB
   ceiling was also binding: ~37 MiB of unique content). Verified on device: `119 registered,
   0 dropped`. `wp81_texture_drop_info(index)` + `PrprPort.TextureSummary()` print the reason.
2. **Record strides are a pair.** Adding a field to `PortVertex` (alpha) without moving
   `PortVertex`/`VertexStride`/`ReadVertex` made every geometry triangle read `a = 0`, so the
   renderer's transparent-guard skipped *all* of it — notes, HUD and text gone while judge lines
   (a different command kind) drew fine. `WireLayoutOk()` existed but was only reachable from the
   unused `DrainSelfTest`. **It is now asserted at startup and printed every run**
   (`wire layout ok=True cmdStride=56/56 vertexStride=36/36`) — check that line first when geometry
   disappears.
3. **Touch: `_started` is a chart-path flag.** `if (_paused || _finished || !_started) return;` in
   `OnPointerPressed` dropped every tap in phira mode, where `StartPlayback` never runs. Now an
   `InputLive` property (`_rustCreated` on the Rust path, `_started` on the chart path), and the
   press is logged *before* the guard so a dropped press is visible.
4. **Lag had three causes:** `PushRustTouches` duplicated drain+draw+present (2× per frame);
   `FillPortTriangle` leaked a `CanvasGeometry` per untextured triangle per frame (78→26 fps over
   10 min); `ReadF` allocated a byte[] per field (tens of thousands per frame). All fixed, plus the
   drain buffers are reused and the two triangles of a quad now draw once.
5. **AppLog rotation** used to delete and recreate past 256 KB, losing the *beginning* of a long
   run. It keeps the tail now.

**Logging on device (R55).** `rust-wp81/tail-log.ps1` follows the log: poll `ISETool ts`, print the
delta. It reads **`appxfolder:Temp`**, not Local — `AppLog` mirrors every line into the temp store —
because `ts` on Local copies the whole 22 MB store and **aborts on the first unreadable file, which is
the chart's mp3 held by the `MediaElement`**, so the log was never reached and pulls failed with a
misleading sharing violation. Each attempt also needs a **fresh** destination directory. `AppLog` now
keeps the tail on rotation instead of starting over, and there is an **on-screen log panel** in
`GamePage` (a `LOG` button beside pause; `AppLog.Remember`/`RecentText` ring buffer) so no pull is
needed at all. Verified: `fps=46…52` in gameplay, `touch down … live=True`, `wire layout ok=True
vertexStride=36/36`, `119 registered, 0 dropped`, `noCarrier=0`.

## R56 — assets packaged; two open items

**The app is self-contained now: `stage-chart.ps1 -Push` is not needed.** `Assets/charts/` (114
files, 22 MB, copied from `E:\_phirura_m1\charts`) is packaged via a **recursive glob**
`<Content Include="Assets\charts\**\*" />`, and `EnsureBundledAssetsAsync()`/`CopyTreeAsync()` seed
`LocalFolder\charts` from `ms-appx:` behind a `.seeded` marker before the chart bootstrap runs.
Verified with `-Deploy -Launch` and **no push**: `seeded 114 packaged assets`, `phira boot -> 0`.
**Keep `E:\_phirura_m1\charts` and `Assets/charts` in sync** — that dir is now the packaging source,
not just the push mirror.

**Sprite drawing is exact for rotated quads now.** `FillPortTriangle`'s axis-aligned bounding box
stretched rotated notes (a vertical note on a tilted line). `TryDrawPortQuad` takes the 6 vertices of
a triangle pair, finds the 4 uv corners, requires a parallelogram (prpr sprites always are), and
draws once via an affine `Matrix3x2` from the source rect onto the quad.

**Hit sounds are stubs — not a regression.** `PrprPort.OnSfxNew` returns 0 and `OnSfxPlay` only logs;
there is no `ISfxSink`. Rust requests them (`audio sfx_play id=0`), so the fix is C#-side and has the
same shape as the music one: carry `clip.length()` across `wp81_audio_sfx_new`, identify by duration,
implement an `ISfxSink` in `GamePage` over `SfxPlayer`.

**The hit *effect* cannot appear at all — architectural, not a regression.** `prpr::particle` drives
miniquad's `Context` directly (`Emitter::draw` → `apply_bindings`/`apply_uniforms`/`draw`, instanced
quads uploaded via `Buffer::update`), and **every one of those is a no-op in the shim** — the file
says so. So particle geometry, its texture and its MVP are all discarded and never reach the port's
`Cmd` arena. Fix: have the shim *record* that pass (`Buffer::update` keeps the bytes,
`apply_bindings` keeps the texture, `Context::draw` expands instances into `Cmd::Geometry`). The same
change unlocks `JudgeLineKind::Paint`, the other raw-render-pass path.

## Next steps
Stage 3: port prpr scene draw calls — vertex stride **36 B** (`pos3(12)+uv2(8)+rgba4(16)`, `z` kept 0),
`MAX_VERTICES=65536` → ~2.3 MB dynamic buffer (stride 32 is wrong). Scene is all macroquad
`draw_texture_ex`; sprites are already batched into `res.note_buffer`. Stage 4: on-device self-test,
then swap `GamePage` off `Win2DChartRenderer` onto the Rust renderer.
