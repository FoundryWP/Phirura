# Phirura — handover

Running the real **phira** (Rust rhythm game) on a **Windows Phone 8.1** device.
Device: **Lumia 925 / RM-910**, Adreno 225, WDDM 1.2, FL 9.3 (`0x9300`),
product id `fa2c9634-5ad8-487e-be3c-9893871fda9f`.

> **Read `REFERENCE.md` (project root) first** — vtable slots, IIDs, constants, the eliminated-hypothesis
> log, tool/env catalogues. Then `.workbuddy-ai/memory/2026-10-05.md`, which has the blow-by-blow
> evidence for everything below. This file is the summary; those two are the detail.

---

## 1. Layout

| Path | What |
|---|---|
| `Phirura/Phirura.sln` | the app |
| `Phirura/Phirura/Phirura.WindowsPhone/` | **WP8.1 head (ARM)** — the one that runs on the device |
| `Phirura/Phirura/Phirura.Windows/` | Win8.1 head (AnyCPU) |
| `Phirura/Phirura/Phirura.Shared/` | shared C# (App.xaml.cs, AppLog.cs) |
| `phira-main/phira-main/` | the Rust game. **`prpr`, `prpr-port`, `phira` are build inputs, not read-only** — we modify all three |
| `rust-wp81/` | the port workstream: shims, build scripts, tools |
| `rust-wp81/shims/{macroquad,miniquad,sasa,rfd}/` | our replacements for those crates |
| `E:\_phirura_m1\charts` | asset mirror — **packaging source**, keep in sync with `Assets/charts` |

**Do not delete `.workbuddy-ai/`** (project data, not cache).
**Never call `os.remove` in this workspace** — a safe-delete guard once moved 13 source files into
`<ws>/trash/`. Use `os.replace()`/`mv`.

---

## 2. Build & deploy — the exact commands

All via the **PowerShell** tool (direct MSBuild is sandbox-blocked).

```powershell
# Rust DLL (only when Rust changed)
& "E:\Users\admin\Documents\visual studio 2013\Phirura\rust-wp81\build-port.ps1"
# C# appx (when C# changed, or after the DLL changed)
& "E:\Users\admin\Documents\visual studio 2013\Phirura\build_wp.ps1" -Configuration Debug -Target wp
# Deploy + launch
& "E:\Users\admin\Documents\visual studio 2013\Phirura\rust-wp81\stage-chart.ps1" -Deploy
```

* `build-port.ps1` gates: 7 imports all API sets, TLS directory OK, prints the DLL size.
  `PORT_EXIT=0` is the pass condition. **`check_imports.py` is the real gate — a passing link proves
  nothing** (`rust-wp81/tools/check_imports.py`, exit 0 = OK).
* `-Deploy` **only** works if `<dir>` does not exist for the log pull; the deploy script handles it.
* `stage-chart.ps1 -Push` is **no longer needed** (see §4). Keep it around for emergencies.
* Launch often fails with **`请确保设备屏幕已解锁`** — the phone is locked. Retry in a loop; it
  eventually goes. Nothing is wrong when you see that.
* **`-Isolated`** on `build_wp.ps1` repoints the obj dir when a pdb handle is stuck.

**Reading the log** (this is the only debugger you have):
```powershell
& "E:\Users\admin\Documents\visual studio 2013\Phirura\rust-wp81\tail-log.ps1" -Seconds 5 -MaxPolls 2 -Filter "PATTERN"
```
`tail-log.ps1` reads **`appxfolder:Temp`**, not Local — `AppLog` mirrors every line into the temp
store. (`ts` on Local copies the whole 22 MB store and aborts on the chart's mp3 held by the
`MediaElement`, which is why pulls used to fail.) Each attempt needs a **fresh** destination dir.

---

## 3. THE OPEN BUG — UI touches arrive with position (0, 0)

**This is where the next agent starts. It is measured, not guessed.**

Symptoms the user sees: **on `home.rs` almost every button is dead** (one, the resource-pack button,
used to work — that was a coincidence of its rect containing the origin). During gameplay, **drag
notes register but taps/flicks/holds are missed**.

The decisive measurement (the per-second `rust: input:` line, see §5):

```
rust: input: ... gate=15493 thr=0 tx=0 ty=0 px=719 py=519 pn=29 sw=1280 sh=768
```

| Field | Meaning | Verdict |
|---|---|---|
| `px=719 py=519` | position **at the DLL boundary** (`wp81_game_touch`), after the shell's DIP→px | **CORRECT** — the shell is fine |
| `tx=0 ty=0` | position **as `prpr::ui` sees it** (`home.rs`'s `touch`) | **WRONG — (0,0)** |
| `sw=1280 sh=768` | `screen_width()`/`screen_height()` | **fine** |

**So the position is good entering the DLL and (0,0) by the time the UI gets it.** Everything on the
C# side is proven correct; **do not go back there.**

**Where to look, in order:**

1. **`Judge::get_touches()` → `touch_transform`** — `prpr/src/judge.rs:382`. This is the **only** place
   a touch position is rewritten:
   ```rust
   touch.position = vec2(
       (p.x - vp.0 as f32) / vp.2 as f32 * 2. - 1.,
       ((p.y - (screen_height() - (vp.1 + vp.3) as f32)) / vp.3 as f32 * 2. - 1.) / (vp.2 as f32 / vp.3 as f32),
   );
   ```
   `vp` comes from `prpr::ext::get_viewport()` (`prpr/src/ext.rs:194`), which chains
   `quad_gl.get_viewport()` → `quad_gl.get_active_render_pass()` → `screen_width()/height()`.
   **The shim returns `None` from the first two**, so it should land on `(0, 0, 1280, 768)` — which
   would be correct. **Verify that, because a `vp.2` of 0 makes every position NaN, and a NaN printed
   through an integer log reads as `0`.** `tx=0` *exactly* (not "small") is the tell.
2. **`Judge::on_new_frame()`** — `prpr/src/judge.rs:367`, called from `prpr/src/scene.rs:475`. It is
   the only thing that fills `TOUCHES` (via `repeat_all_miniquad_input` → `for_each_pending`).
3. **`Handler::touch_event`** — `judge.rs`, pushes `position: vec2(x, y)` raw. Verify it receives the
   good value.

**The cheapest next probe** (follow the existing pattern in §5): log `p.x/p.y` **inside
`touch_transform`** next to `vp`. That is one edit and it settles whether the transform or the store
is at fault. **Add probes; do not change behaviour until the numbers say which line is wrong.**

**Proven-good upstream of this** (all measured): touches reach the shell and the DLL (`pn` counts up),
the C# finger table drains (`remaining` returns to 1), `PointerReleased` is the only release event that
fires (`PointerCaptureLost` never does), and the `home.rs` gate is **open** (`thr=0`).

---

## 4. What is DONE and verified on the device

| Item | Evidence |
|---|---|
| **Boot succeeds** | `GamePage: phira boot -> 0 in 8961 ms, stage 7 (phira ready)` |
| **Lands on `home.rs`** | `scenes=1` steady for 36 s, no `SongScene`, no `auto-open requested` |
| **Assets self-contained** | `seeded 114 packaged assets`; deploys with **no `-Push`**; appx 7.4 → 19.7 MB |
| **Hit sounds** | `sfx_new name='click' secs=0.24 -> ms-appx:///Assets/game/click.wav`, then `sfx_play id=3` (click) / `id=4` (drag) |
| **Hit effects (particles)** | `pushed=882` — particle quads reach the command stream, positioned on the judge line |
| **Clock runs** | `now=3 → 5 → 7` (was frozen at 0 forever) |
| **No startup white blocks** | `noCarrier=0` on the very first frame |
| **Texture registry** | `119 registered, 0 dropped` (cap is 512 slots / 64 MiB) |
| **Autoplay off** | `PhiraAutoPlay=false`, `PhiraAutoOpen=false` |
| **Rust→log is non-blocking** | `AppLog.Write`, not `WriteSync` (the latter stalled the render thread 500 ms/sec) |

---

## 5. Instrumentation already in the tree — use it, don't rebuild it

**Per-second diagnostic line** (from `wp81_game_frame`, `prpr-port/src/lib.rs`):
```
rust: input: pending= replayed= draws= pushed= quads= texNew= texRebuilt= gate= thr= tx= ty= px= py= pn= sw= sh= now=
```
Gated on a **frame counter**, deliberately — gating it on a particle counter once made it silent on
`home.rs`, exactly where the bugs were.

**`macroquad::dbg`** (`shims/macroquad/src/lib.rs`): a 4-slot `f32` channel for values only the game
crate can see. `phira` cannot log directly (`prpr_port` depends on it, not the reverse; `tracing` has
no subscriber here) — so park floats here and the frame entry point prints them.
Currently `home.rs`'s `touch` parks `(rt, char_screen_p.now(rt), touch.position.x, touch.position.y)`.

**Other probes:** `wp81_sprite_quads`, `wp81_particle_diag(index)`,
`wp81_texture_drop_info(index)`, `macroquad::input::last_push()`, `pending_len()`, `replayed_count()`.

**On-screen log panel**: a `LOG` button beside pause in `GamePage` (`AppLog.Remember`/`RecentText`).

**`home.rs` has bracketing lines** around the boot's file load
(`load_info -> entering` / `-> returned`) — keep them, they localised the worst bug in this project.

---

## 6. Rules that bite — each one cost real time

1. **`tokio::task::spawn_blocking` never completes on WP8.1.** The blocking pool creates threads on
   demand and this target will not give the DLL working threads. The task silently never runs and the
   await **never resolves** — a hang, not an error. `std::thread` is worse: it aborts `0xC0000409`.
   **Any thread-based primitive is a landmine.** (`prpr/src/fs.rs:90` was the offender; fixed.)
2. **`std::sync::Mutex` in the shim broke the boot.** Reverted. Same class as (1). Do not reintroduce
   a lock without a device test that shows the boot still reaches its first frame.
3. **`QuadGl::get_projection_matrix()` returning `Mat4::IDENTITY` is load-bearing for the boot.**
   "Correcting" it to the screen ortho crashed the boot (`-2`, never reached its first frame). It is
   back to `IDENTITY` with a comment. The boot is an await chain, so a bad value **parks** instead of
   erroring.
4. **The touch queue `PENDING` is `thread_local!` and must stay so.** (See 2.)
5. **Rust→log must use `AppLog.Write`, never `WriteSync`** — it runs on the render thread.
6. **New `#[no_mangle]` entry points silently do not appear in the DLL** — `build-port.ps1` carries an
   explicit `/EXPORT:` list. **Link ≠ emitted.** Verify with `check_imports.py`.
7. **`Ordering`/`AtomicU32` are not in scope in the shim's `input` module** — fully qualify
   (`core::sync::atomic::Ordering`). This bit twice.
8. **A texture with no carrier falls back to a flat fill — a white block.** So never gate the texture
   poll while the registry is still growing.
9. **`prpr/src/particle.rs` is ours to modify.** It drives miniquad's `Context` directly, and every
   method of that shim is a no-op, so nothing it draws can reach the renderer — the hit effect had to
   be re-expressed as ordinary geometry. Same class: `JudgeLineKind::Paint`, and `ThreeD`
   (`phira/src/ui/threed.rs`) which is why the **play button renders as a flat white shape**.
10. **Never `$ErrorActionPreference = "Stop"` with `2>&1`** in the build scripts — it turns cargo
    warnings into a `NativeCommandError` that hides the exit code.
11. **`edit` is flaky on `GamePage.xaml.cs`** — retry 1–4 times, keep `oldString` ASCII.

---

## 7. Remaining work, in priority order

1. **UI touch position (0,0)** — §3. This blocks every menu. Highest value: it is also why
   `chart_info.rs`'s buttons and the song list are dead.
2. **SFX cuts the music off.** Almost certainly WP8.1's **single media session** — `SfxPlayer`'s
   `MediaElement` pool and the music's `MediaElement` compete for it. **Do not fix this by adding
   another `MediaElement`.** Needs a different audio path.
3. **`chart_info.rs` cover** — may already be fixed by the `spawn_blocking` fix (same function). Verify.
4. **`ThreeD` shader effect** (white play button) — reproduce as geometry, same technique as the
   particles.
5. **Intermittent freeze** — reduced (rate-gated touch log, `WriteSync` 3 s → 500 ms, poll gate), not
   eliminated. Nothing in the log points at it yet.
6. **Home BGM is not obtainable** — `phira/src/lib.rs:65` `load_res` is `#[cfg(closed)]` and there is
   no `res/` directory anywhere in the tree. It belongs to phira's commercial resource set. **Not a
   packaging miss** — the three asset trees are identical in listing.

---

## 8. Working style that worked here

* **Measure, then change.** Every wrong turn in this project came from reasoning about code instead of
  reading a number. Every fix that stuck came from a probe.
* **One change per device round.** Mixing caused the white blocks, the skewed hold, the boot crash and
  a `Mutex` regression — all self-inflicted.
* **"The button reacts but does nothing" is a RELEASE problem; "nothing reacts at all" is a hit-test
  problem.** They need completely different investigations. The one detail that separated them was the
  user noticing the pressed feedback.
* **The boot is an await chain.** A bad value parks it silently — no panic, no error, no stack. Read
  the bracketing log lines to find which await stopped.
