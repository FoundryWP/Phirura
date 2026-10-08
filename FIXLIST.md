# Phirura 修复清单

状态：`待办` / `进行中` / `已修复待编译` / `已上机验证`

| # | 状态 | 问题 | 位置 |
|---|------|------|------|
| 1 | 已修复待上机 | 结算页背景音频概率播错，播成了 chart/190 预置音频 | `GamePage.xaml.cs` `MusicNew` |
| 2 | 已修复待上机 | 选歌页音频播的是 190 预置音频，不是所选谱面的预览；且应读 json 里指定的预览秒数，不是整首 | `song.rs` + `sasa` shim + `GamePage.xaml.cs` |
| 3 | 已修复待上机 | 选歌页背景图黑屏（home.rs 实时轮换能显示封面 ⇒ 问题在 song.rs 自身） | `phira/src/images.rs` + `page.rs` |
| 4 | 已修复待上机 | 本地铺面列表封面黑屏（已确认下载资源齐全） | `phira/src/images.rs` + `page.rs` |
| 5 | 已修复待上机 | 删除铺面按钮一点就闪退 | `charts_view.rs` + `library.rs` |
| 6 | **已修复已上机验证** | 资源为什么全在 `Assets/charts`，应该在 `Assets/games` | `Assets\charts`+`Assets\respack`、`PrprPort.AssetPackRoot`、`stage-chart.ps1` |
| 7 | 待办 | 结算页不渲染分数等，但点对应位置按钮有响应 | `prpr/src/scene/ending.rs` |
| 8 | 已修复待上机 | 阴影渲染成格子横线（可见三角形拼缝） | `Win2DChartRenderer.cs` `FillPortTriangle` |
| 9 | 已修复+已加诊断，待上机 | 铺面读取导致闪退 | `phira/src/lib.rs` + `song.rs` |
| 10 | 部分修复（负长度已修，跳变待上机） | 游玩时 hold 键有时多出一截 | `prpr/src/core/note.rs` Hold 分支 |
| 11 | 待办 | 长按 Hold 时可能出现重复的 hold 键 | prpr 判定 |
| 12 | 已修复待上机（`fake` 淡出留观察） | 虚假 hold 体积大小不定 | `prpr/src/core/note.rs` Hold 分支 |

---

## 1. 结算页背景音频播错

**现象**：`ending.rs` 页的背景音乐有概率播成 `chart/190` 里预置的音频。
**应该是**：`E:\Users\admin\Documents\visual studio 2013\Phirura\Phirura\Phirura\Phirura.WindowsPhone\Assets\charts\ending.mp3`

**已知**：
- 音频分类：music 走 `GameMedia`→`Other`，sfx 走 `GameEffects`→`Other`（`AudioCategory` 不是变量，四种配对都上机测过，music 全被切）。
- `MediaElement.Source` 只在 `SfxPlayer` 构造时赋值一次。
- 结算页重复播放音频曾经是已知问题，已修过一次（不要重开那条线）。

**根因（已定位）**：Rust 侧是对的 —— `game.rs:1008` 传进去的就是 `self.res.res_pack.ending.clone()`，
`resource.rs:296` 的 `load_clip!("ending")` 还给它起了名字 `"ending"`。

错在 C# `GamePage.xaml.cs` 的 `MusicNew`，它用两条规则区分"谱面自己的音乐"和"结算页 BGM"：

```csharp
string uri = PhiraMusicUri;                       // ← 默认就是谱面音乐
bool isChart = true;
if (IsChartMusicPath(source))  { ... }            // 有 "/" 且有扩展名 → 谱面
else if (|clipSecs - PhiraMusicSeconds| > 1.0)    // 否则靠时长猜 → 结算 BGM
```

`"ending"` 既没有 `/` 也没有扩展名，`IsChartMusicPath` 判否，于是**落到"靠时长猜"**：
只要 `ending.mp3` 的时长和谱面音乐相差不到 1 秒，`else if` 就不成立，`uri` 保持初值
`PhiraMusicUri` —— **播成了 chart/190 的预置音频**。这就是"概率错误"的来源。

**改法（已落地，`GamePage.xaml.cs`）**：
1. 新增 `IsResPackEndingName(source)`：取 basename 去扩展名后比 `"ending"`，
   **放在 `IsChartMusicPath` 之后**，这样谱面自己的文件万一真叫 `ending.mp3`（带目录）仍归谱面。
2. `MusicNew` 里在时长猜测**之前**插入 `else if (IsResPackEndingName(source))`，
   由名字直接决定，不再看时长。
3. `ResolvePhiraEndingMusicAsync` 末尾补上打包内那份兜底
   `ms-appx:///Assets/charts/ending.mp3`（此前只找 `LocalFolder\charts`，仓库里真正存在的
   `Assets\charts\ending.mp3` 从没被试过）。

**状态**：编译 `EXITCODE: 0`，仅 3 条既有警告。**待上机验证**（看
`GamePage: music_new -> ... (res-pack ending)` 与 `phira ending music = ... (packaged)` 两行）。

**注意**：结算页重复播放音频是另一条已经修好的线，不要重开。

---

## 2. 选歌页音频：预置音频 vs 谱面预览

**现象**：`song.rs` 播的是 190 的预置音频，不是所选谱面对应的音频预览。
**应该是**：
- 播所选谱面自己的预览音频；
- 并且要读铺面 json 里指定的**预览起始秒数**，从那一秒开始播，而不是把整首播完。

**Rust 侧本来就做对了一半**：`song.rs` 读的是 `info.preview_start` / `info.preview_end`
（缺省 `preview_start + 15`），`with_effects` 用 `frames.drain(..)` 把帧**裁到这个窗口**。
问题是**裁好的 clip 没有名字** —— 走 `AudioClip::from_raw(...)`，`name_len = 0`。

**根因**：C# `MusicNew` 拿到 `source = ""`，`IsChartMusicPath("")` 判否、`IsResPackEndingName("")`
判否，又因为此刻 `PhiraMusicSeconds == 0`（歌单页还没开过任何曲子）**时长判断被整个跳过**，
`isChart` 保持初值 `true`，`uri` 保持 `PhiraMusicUri` = **chart 190 的预置音频**。
于是每首歌的预览都播 190 那首、而且从头播到尾。

**改法（已落地）**：
1. `rust-wp81/shims/sasa`：新增 `AudioClip::from_raw_named(name, frames, rate)`，
   `from_raw` 改为委托它；抽 `copy_name()` 供 `new_named` / `from_raw_named` 共用；
   `NAME_CAP` 96 → **192**（新名字形如
   `preview|999.9|999.9|download/79814/4347843821313567.mp3`）。
2. `song.rs::with_effects` 增加首参 `name`，三个调用点分别传
   - `load_local_tuple`：`preview|{from:.1}|{to:.1}|{local_path}/{info.music}`
   - `preview_task` 本地：`preview|{from:.1}|{to:.1}|{path}/{info.music}`
   - `preview_task` 服务端：`preview|0|0|`（**空路径** ⇒ 壳层无文件可开，明确静默并打日志，
     而不是再去播启动前解析好的那首）
3. `GamePage.xaml.cs::MusicNew`：解析 `preview|from|to|path`，`path` 进 `IsChartMusicPath` ⇒
   `uri = ms-appdata:///local/data/charts/{path}`；同时记 `_previewFrom/_previewTo`。
4. `OnMediaOpened(phira)`：打开后 `MusicSeek(_previewFrom)` 跳到预览起点。
5. 新增 `GuardPreview()`，由 `GuardMusic()` 每次先调：`Player.Position >= _previewTo` 时
   `MusicSeek(_previewFrom)` + `Play()` **循环**；250 ms 节流，防止元素拒绝对同一跳重试。
   走 `MusicSeek` 而不是直接写 `Player.Position`，是因为它同时动 clock 和元素并清掉
   停滞检测历史（否则这个倒退跳会被读成卡顿）。

**状态**：`build_wp.ps1 -Debug -Target wp` → `EXITCODE: 0`。**待上机**看
`music_new -> ...(chart) src='preview|...'`、`MediaOpened(phira): preview window`、
`preview loop ->` 三行。

---

## 3. 选歌页背景图黑屏

**现象**：`song.rs` 页背景是黑屏，不显示封面。
**对照**：`home.rs` 的实时轮换**能**显示封面 ⇒ 上传/注册/解码链路是好的，问题出在 `song.rs` 自身。

**调用链**（`song.rs:2717` 就是背景那一行）：
```
ui.fill_rect(ui.screen_rect(), (*self.illu.texture.1, ui.screen_rect()));
```
`self.illu` 由 `local_illustration(path, def, full)`（`page.rs:86`）建出，任务是 `illustration_task`
（`page.rs:62`）→ `Images::local_or_else(thumbnail_path(&path)?, ...)`。

**根因（已修）**：`images.rs` 的 `local_or_else` 原来是
```rust
Ok(if path.exists() {
    image::load_from_memory(&tokio::fs::read(path).await.context(...)?)?
} else {
    let image = task.await?;
    image.save_with_format(path, ImageFormat::Jpeg).context("Failed to save image")?;   // ← 一个 '?'
    image
})
```
**缩略图缓存写失败（或缓存文件损坏读失败）就让整个插图任务返回 `Err`**，而 `page.rs:177` 的
错误分支只有 `warn!(...)` —— **本目标 `warn!` 一条都不打**。于是：图好好地躺在盘上，页面纯黑，
日志里也找不到任何痕迹。

**改法（已落地）**：
- [x] `images.rs::local_or_else`：缓存读改成"只是一次尝试"（读不到/解不开就重新生成，
      并 `log_line("images: cache unreadable at ...")`）；缓存写改成尽力而为
      （失败 `log_line`，**照常返回图片**）。
- [x] `page.rs::Illustration::settle` 错误分支在 `warn!` 之外补
      `crate::log_line(&format!("illustration failed: {err:?}"))`。

**状态**：`build_wp.ps1 -Debug -Target wp` → `EXITCODE: 0`。**待上机**确认
`illustration failed:` / `images: cache ...` 两行为空（= 修复生效）或给出真正的原因。

---

## 4. 本地铺面列表封面黑屏

**现象**：`library.rs` 的本地铺面列表封面黑屏。
**已确认**：下载的资源都是全的（图确实存在）。

**待查**：
- [x] 与 #3 **是同一个根因** —— `library.rs` 的封面也走 `Illustration::settle` → `Images::into_texture`，
      同样落在 `images.rs::local_or_else` 那两个 `?` 上。
- [x] `Illustration::settle` 的失败现在会打 `illustration failed: ...`（原来只有静默的 `warn!`）。
- [ ] 若上机后该行仍报错，再看是不是 `library.rs:390` 的 `Illustration::from_file_thumbnail`
      走的 `file.load_thumbnail()`（服务端缩略图）在离线时失败。

---

## 5. 删除铺面按钮闪退

**现象**：点"删除铺面"按钮立即闪退。

**根因（已定位）**：删除这段代码里有 **两个 `unwrap()` + 三个 `?`**，任何一个失败都会让
`update()` 返回 `Err`，往上一路冒到主循环 —— 在本目标表现就是**闪退**。

单个删除（`charts_view.rs:413-441`，删除在转场动画结束后才真正执行）：
```rust
let item = &self.charts.as_ref().unwrap()[transit.id as usize];   // 解包 + 下标越界
let path = if let Some(path) = &item.chart.as_ref().unwrap().local_path { ... }  // 解包
    else { format!("download/{}", item.chart.as_ref().unwrap().info.id.unwrap()) }; // 解包
std::fs::remove_dir_all(format!("{}/{path}", dir::charts()?))?;    // 目录已不存在 → Err
save_data()?;                                                      // → 整个进程退出
```

批量删除（`library.rs:1040-1060`）：
```rust
let selected = self.tabs.selected_mut().view.multi_select.take().unwrap();  // 选中已被清空 → panic
if let Some(path) = chart.find_local_path()? { ... }
match std::fs::remove_dir_all(...) { Err(err) => return Err(err.into()), ... }
```
确认对话框是异步的，`delete_multi` 置位时原选中可能已经没了 → `.take().unwrap()` 直接 panic。

**改法（已落地）**：
- [x] `charts_view.rs`：改用 `transit.chart`（本就持有该谱面的克隆）替代下标 + `unwrap`；
      `remove_dir_all` / `dir::charts()` / `save_data()` 全部改为"失败只 `log_line`，不外抛"；
      谱面既无 `local_path` 又无 `info.id` 时打日志并保留文件夹，而不是 `unwrap`。
- [x] `library.rs`：`multi_select.take()` 改 `match`，无选中时打日志直接跳过；
      逐项 `find_local_path` / `dir::charts` / `remove_dir_all` 的 `?` 与 `return Err`
      全部改成 `log_line`，`NotFound` 仍视为正常。

**状态**：`EXITCODE: 0`。**待上机**验证点删除不再退，并看 `charts_view: delete folder failed for ...`
/ `library: ...` 是否给出之前没看见的真因（怀疑就是那个 **os error 123**）。

---

## 6. 资源目录：`Assets/charts` vs `Assets/games`

**查清了，其实有三个互不相干的目录，名字撞车了：**

| 目录 | 里面是什么 | 运行时去哪 | 谁在用 |
|---|---|---|---|
| `Assets\game\`（**单数**） | 我们这个 port 自己的判定音 `click/drag/flick/cali_hit/button/switch.wav` + HUD 用的 `click/drag/flick/hold/hit_fx.png` | 不搬，直接 `ms-appx:///Assets/game/...` | `SfxPlayer.cs:50-56`、`GamePage:1150/1925/3185`、`GamePage:657-663` |
| `Assets\charts\`（拆分前是引擎资源根） | **只剩演示谱面 `190\` `demo\`**（28 项） | `LocalFolder\charts`（`stage-chart.ps1` 按内容分类推） | `PrprPort.AssetRoot`（`PrprPort.cs:1914`） |
| `Assets\respack\`（**C 拆出来的新增目录**） | **引擎自己的资源根**：`font.ttf` `bold.ttf` `background.jpg` `ending.mp3` `respack\` `rank\` `mod\` + 全部 UI 贴图 / 判定音（87 项） | `LocalFolder\respack` | `PrprPort.AssetPackRoot`（`PrprPort.cs:1945`） |
| `Assets\demo\` | 旧的 C# demo 谱面 | 不用（`UseRustEngine = true` 已绕开） | `ChartLibrary.cs:26` |

**为什么当初叫 `charts`**：跟 phira 无关，是**桌面版的目录布局**带来的 ——
`stage-chart.ps1` 的老注释原话："The engine reads from `<LocalFolder>\charts` … which is also the
desktop layout, since `E:\_phirura_m1` holds nothing but `charts`"。脚本 `$Source` 默认
`E:\_phirura_m1\charts`（**至今仍是平铺的一层，没动**），
`rs` 把 `$Out` 的内容推到 `LocalFolder` ⇒ 原先 `LocalFolder\charts` 就是引擎资源根。
C 拆分后 `stage-chart.ps1` 把 `$Source` 按内容分成 `$Out\charts` + `$Out\respack` 两份再推，
`LocalFolder\charts` 退回成**只装谱面**，资源根改叫 `LocalFolder\respack`。

**真正的铺面不在这里**：下载的谱面走 `dir::charts()` = `ensure("data/charts")`
（`phira/src/lib.rs:255`）⇒ **`LocalFolder\data\charts`**，而且 `stage-chart.ps1` 部署时
专门保留 `LocalState\data`（第 134-180 行）。

**⇒ 所以 `Assets\charts` 里"像谱面"的 `190\` 只是随包附带的演示谱面（源），
不是铺面库；铺面库在 `LocalState\data\charts`。**

**⇒ 而 `Assets\game` 与 `Assets\games` 无关**，它是本 port 自己的 HUD 资源，
和 phira 的 `games/` 目录没有对应关系。

**✅ 你选了 C，已实施（待上机验证）** —— 谱面和引擎资源不再混在一个叫 `charts` 的目录里：

| 新位置 | 内容 | 运行时根 |
|---|---|---|
| `Assets\charts\` | 只剩演示谱面 `190\` `demo\`（28 项） | `LocalFolder\charts` → `PrprPort.AssetRoot` |
| `Assets\respack\` | 引擎自己的文件：字体、UI 贴图、判定音、`respack\` `rank\` `mod\`、`ending.mp3`（87 项） | `LocalFolder\respack` → `PrprPort.AssetPackRoot`（新增） |

**改动清单：**

1. **`PrprPort.cs`** —— 新增 `AssetPackRoot`；`ReadAsset(relative)` 拆成 `ReadAsset`（双根分派）
   + `ReadAssetFrom(root, normalised)`（单根读取，内部 `try/catch` 返回 null）。
   **顺序是 `AssetRoot` 先、`AssetPackRoot` 后**：前者正是拆分前的布局，所以没重推过的 store
   行为逐字节不变，只有 miss 才多走一趟。两根名字集合不相交（谱面名带自己的目录前缀
   `190/info.yml`，引擎名是裸的 `font.ttf` / `rank/phi.png`），顺序只决定走几次，不会取错文件。
   第二根命中时打一行 `PrprPort: asset resolved from pack root: <name>`。
2. **`GamePage.xaml.cs`** 和 **`RustStage1Page.xaml.cs`** —— 两处设根都同时设 `AssetPackRoot`，
   日志变成 `GamePage: phira asset root = …\charts, pack root = …\respack`。
3. **`GamePage.xaml.cs` `EnsureBundledAssetsAsync`** —— 拆成 `SeedRootAsync(local,"charts")` +
   `SeedRootAsync(local,"respack")`，**各自带 `.seeded` 标记**。这是必须改的：不改的话
   `LocalFolder\respack` 永远不会被种进去，下游症状是 `asset not found: font.ttf`。
   包里没有 `respack` 时打 `GamePage: seed 'respack' absent from the package`。
4. **`ResolvePhiraEndingMusicAsync`** —— 原来硬编码 `LocalFolder\charts`，拆分后必断。现在是
   `folderRoots = { "charts", "respack" }` × `subs = { "", "respack\\" }` × `exts = { ".mp3", ".wav" }`，
   最后才回落到包内 `ms-appx:///Assets/respack/ending.mp3`。
5. **`stage-chart.ps1`** —— `$destCharts` + `$destPack` 两个目标，**按内容分类而不是按名字**：
   顶层目录里有 `*.json`/`*.pec` 就是谱面，否则是引擎资源。所以 `190\` `demo\` → charts，
   `respack\` `rank\` `mod\` + 平铺的字体/贴图/音效 → pack。
   （只认 `info.yml` 会把资源包误判成谱面 —— `respack\info.yml` 是资源包自己的描述文件，
   必须同时认谱面文件。）每次 `-Push` 打 `-> charts : …` / `-> pack : …` 报告分类结果。
6. **csproj** —— `Assets\charts\**\*` 之外新增 `Assets\respack\**\*`（已验证打进 appx：87 + 28 项）。
7. **`GamePage.xaml.cs` 注释里的 `ms-appx:///Assets/charts/190` 不用改** —— `190` 本来就留在 `charts`。

**桌面源目录 `E:\_phirura_m1\charts` 保持原样**（还是平铺的一层），拆分只发生在 stage 时，
所以 `build-m1.ps1` 的布局不用动。

**没动的**：`Assets\game\`（本 port 自己的 HUD 资源，与 phira 无关）、`Assets\demo\`（死掉的 C# demo）。
`SfxUri` 的 `default` 分支（`ms-appdata:///local/charts/<name>.ogg`）也没动 —— 它是给"谱面自带判定音"
留的，而谱面自带的音效本该在 `<chartfolder>/<name>.ogg`，这个分支拆分前后都一样取不到；
等日志里出现 `sfx_new` 的非 switch 名字再说。

---

## 7. 结算页不渲染分数，但按钮位置有响应

**现象**：`ending.rs` 页的结算分数等**不渲染**，但点上面对应位置的按钮**有响应**。
（与"最右边「详细」按钮的字看不见"、"半块背景不显示"是同一页。）

**已有证据**（本页每个形状都已用"变了就记"清单核过一遍，全部正常）：
- 半块背景**就是** `br`：`untext dst=(0,76.8,1280,217.6) a=0.7 rgb=0.16,0.2,0.24 clp=none`
  - 世界坐标验算：`zoom=(1,-1.667)` ⇒ 世界 1 单位 = 640px，世界 y=0 在屏幕 384，`top=0.6`，`br.y = 0.6−0.12 = 0.48` → py = `384−0.48×640` = **76.8** ✓；`br.bottom = 0.48−0.34 = 0.14` → **294.4** ✓
  - `a=0.7` = 水平渐变 alpha(1→0.1) 三顶点平均 `(1+1+0.1)/3` ✓
- 绘制顺序 `ord=t0 r1 g2..129 c16 last=4` ⇒ 封面第 0 条、全屏黑 30% 底第 1 条、**几何 2..129 全在其后**，没有东西盖住
- 文字 `texdraw id=2147483650 dst=(917.4,434.7,9,4) src=(117,34,9,4) a=1 atlas=1024x1024 clp=none` ⇒ 位置在屏上、src 落在字图内、alpha=1、未裁剪
- `clp=none`，唯一一条被裁的字体 `dst=(603.2,141.4,41,56)` 完整落在 `clp=(0,134,1280,68)` 内
- 无异常：`port draw failed` / `device lost` / `UNHANDLED` **一条都没有**
- `noCarrier=0`、`geoUv=0,0-1,1`、`geoNaN=0`、`registry miss : 0`

**⇒ 发出侧、顺序、裁剪、alpha 全部排除。剩下的唯一未验证假设：采样的字图区域内容是空的（atlas 该处透明）。**

**待办**：
- [ ] 跑 `patch_rustread.py`（`TAIL_OLD` 锚点已因 `ClipTag()` 变更而失效，需先修；且依赖并不存在的 `PrprPort.TexRect` / `ReadTextureRect`，要加 Rust `#[no_mangle]` + `/EXPORT:` 到 `rust-wp81/build-port.ps1`）
- [ ] 或者直接对比 home.rs / song.rs 成功显示时的同一段 atlas

---

## 8. 阴影渲染成格子横线（可见三角形拼缝）

**现象**：阴影渲染全是一片格子横线，能看出是三角形拼出来的。

**根因（已定位，未修）**：`Win2DChartRenderer.cs` 的 `FillPortTriangle` 给每个无贴图三角形只填**一个纯色**：

```csharp
ds.FillGeometry(tri, FloatColor(a.r, a.g, a.b, alpha));
//                          ^首顶点色        ^三顶点 alpha 平均
```

而 `prpr/src/ext.rs` 的 `rect_shadow(r, radius, alpha)` 是"外圈透明 → 内圈黑"的**顶点色渐变**，靠 8 个三角形表达。拍平之后：

| 三角形 | 顶点 alpha | 拍平后 |
|---|---|---|
| (0,1,2) 外,外,内 | 0,0,1 | 0.33 |
| (1,2,3) 外,内,内 | 0,1,1 | 0.67 |
| (0,2,4) 外,内,内 | 0,1,1 | 0.67 |
| (4,0,6) 内,外,外 | 1,0,0 | 0.33 |

上机清单实测正是 `a=0.7` / `a=0.4` 成对出现 ⇒ **渐变完全丢失，只剩 8 块错开的纯色**，顶/底带是横条、左右带是竖条，合起来就是"格子横线框"，且能看见拼缝。

**同一处也影响所有渐变**（结算页 `br` 的水平渐变同样被拍成一个色）。

**主菜单阴影环实测**（健康，几何没问题）：
- 卡片 `(638.7,161.1) → (1163,467)`
- 顶带 `(648.1,166,521.7,9.5)`、底带 `(641.4,458.1,521.7,9.5)`
- 左带 `(638.7,161.1,11.9,297)`、右带 `(1160.6,166,9.2,296.8)`

**改法（已落地，`Win2DChartRenderer.cs`）—— 实际选了 D：线性渐变刷 + 按颜色对缓存**

- [x] 新增 `PortGradientBrush(ds, c0, c1)`：`CanvasLinearGradientBrush` 按**两个端点色**缓存在
      `_portGradients`（`Dictionary<ulong, …>`，key = `(k0<<32)|k1`，有序、无碰撞）。
      阴影 8 个三角形**共用同一对颜色** ⇒ 整个环只创建 **1 个**原生刷子，之后每次只改
      `StartPoint`/`EndPoint`（这两个是可写属性；`EdgeBehavior` 只读，已去掉那行）。
- [x] 端点取**颜色差最大的那对顶点**。依据：重心插值的顶点色在三角形上是一个**仿射场**，
      仿射场沿任意线段都是线性的 ⇒ 两端点色确定的线性渐变，对 prpr 实际发出的
      **一维渐变**（`rect_shadow` 两个外顶点同色、`br` 左右两端定色）是**精确**的。
- [x] 三顶点同色时**仍走原来的单次 `FillGeometry`** —— 纯色形状（面板、note 体、判定线）
      成本和 1:1 通信号都不变，census 仍然一条不差。
- [x] `DisposePortGradients()` 挂进 `DisposePortTextures()` ⇒ 游戏销毁 / 设备丢失
      （`0x8899000C`）两条路径都会释放，不会把 D2D 刷子留给终结器。
- [x] `untext` 的 census 日志**没动**（仍记 `alpha` 平均值 + 首顶点 `rgb`），已核过的日志格式保持可比。

**为什么不再用"切条/细分"**：那会让一个源三角形变成几十次 `CanvasGeometry` 分配，
正好撞上本文件 1881-1886 行记的那个坑（每帧上千次原生资源，10 分钟内从 78 fps 掉到 26）。
刷子是**按颜色对复用**的，不受三角形数量影响。

**踩过的两个编译错（已修）**：
- `CS0200` `CanvasLinearGradientBrush.EdgeBehavior` **只读** → 删掉赋值。
- `CS0136` 我的 `v0/v1` 与上面贴图分支的 UV 局部量 `v0/v1` 撞名（C# 同一方法体内不得重名）
  → 改成 `cv0/cv1/cv2`。

**状态**：`build_wp.ps1 -Debug -Target wp` → `EXITCODE: 0`（仍只有 3 条既有警告）。
**待上机**看主菜单卡片的阴影环是否变成连续的软边，并顺带看结算页 `br` 面板的水平渐变。

---

## 9. 铺面读取导致闪退

**现象**：读取铺面会导致游戏闪退。

**根因 A（已修）：`os error 123` 是 `dir::cache_image_local()` 自己造出来的**

`phira/src/lib.rs` 的 `dir` 模块里，`ensure(s)` 会**前缀 `DATA_PATH`** 再返回：

```rust
fn ensure(s: &str) -> Result<String> {
    let s = format!("{}/{}", DATA_PATH..., s);      // ← 加根
    ...
}
pub fn cache() -> Result<String>            { ensure("cache") }   // ⇒ {DATA_PATH}/cache
pub fn cache_image_local() -> Result<String>{ ensure(&format!("{}/image", cache()?)) }  // ← 再加一次！
```

`cache()?` **已经带根了**，又被塞回 `ensure` ⇒ `{DATA_PATH}/{DATA_PATH}/cache/image`。
`DATA_PATH` 是绝对路径（`lib.rs:547` 由 host 设置），中间于是出现第二个 `C:` ——
Windows 判定路径非法 ⇒ **`os error 123`（ERROR_INVALID_NAME）**。

**而且它同时就是封面黑屏的真凶**：
`page.rs::thumbnail_path` 建在这个错路径上 → `Path::exists()` 恒 false →
`image.save_with_format` 用同一个错路径 → 123 → `illustration_task` 返回 `Err` →
`Illustration::settle` 只有静默的 `warn!` → 纯黑封面、日志一片空白。

**改法**：`cache_image_local()` 不再回进 `ensure`，自己 `create_dir_all` 一次。
（#3/#4 里 `images.rs` 把"写缓存失败"降级成只打日志，是这条的**兜底**；这条是**真修**。）

**根因 B（已修）：panic 在本目标 = 无痕闪退**

`std::panic::set_hook` 装在 `set_log_sink()` 里（sink 一存在就装），
任何 panic 都会写 `PANIC <file>:<line>: <message>` —— 否则本目标没有控制台、`log` crate
被编译掉，shell 只知道回调没返回，**看谱闪退和别的闪退长得一模一样**。

**根因 C（已修）：两处 `chart.id.unwrap()`**
- `song.rs:724` 下载成功但 `id` 为 `None` → panic（"下载正常却报错/闪退"的形状）
- `song.rs:501` 预览任务里 `Ptr::<Chart>::new(id.unwrap())`
  两处都改成 `match`/`ok_or_else` ⇒ 返回 `Err`，由上层显示，不再杀进程。

**另外**：`dir::ensure` 的 `create_dir_all` 失败现在会打
`dir: create '<完整路径>' failed: <错误>` —— 下次再见到 123 就知道是哪个目录。

**状态**：`EXITCODE: 0`。**待上机**看 `PANIC …` / `dir: create … failed` 是否出现；
若读谱仍退，日志会直接给出文件:行号。

---

## 10. 游玩时 hold 键多出一截

**现象**：游玩时 hold 键渲染有时会多出一截。

**已定位（`prpr/src/core/note.rs` 的 `Note::draw` → `NoteKind::Hold` 分支）**：

```rust
let h          = if self.time <= res.time { line_height } else { height };
let bottom     = (h - line_height) as f32;   // 主体下沿
let top        = (end_height - line_height) as f32;   // 主体上沿
```

主体画在 `[bottom, top]`：

- **还没到点**：`bottom = height - line_height`
- **到了点**：`bottom = 0`（判定线）

于是 `self.time == res.time` 那一帧，`bottom` **一次性往下跳 `height - line_height`**，
主体瞬间变长 —— 就是"多出一截"。而且 `height` / `end_height` 都被
`spd`（`self.speed * ctrl_obj.y`，含 combo 拉镜等动画）整体缩放，跳变幅度**每首歌、每个
combo 段都不一样**，正好对上"有时"。

**已先做的安全修（#12 同一处）**：
- [x] `let body = (top - bottom).max(0.);` 且 `body <= 0` **直接不画主体**。
      原来负高度会**同时**进 `dest_size` **和**贴图 `source`，`source` 的 v 高度为负 ⇒
      采样区间翻转，画出来的体积可以是任何值。这条是纯防御，不改变任何正常渲染。

**待查（必须上机 + 截图，`bottom` 一改动所有 hold，不敢盲改）**：
- [ ] 上机确认跳变是否就是这一处（重点看**不在主判定线上**的 hold，`spd` 有动画时最明显）
- [ ] `height` 对 hold 到底该是"笔记静止高度"还是"判定线高度" —— 现在这两帧不连续，
      但把它接上等于重定义 hold 的几何
- [ ] `judge.rs` 的 `note_enter()` / `note_events()`（本轮已改过，注意别回归）

---

## 11. 长按 Hold 出现重复的 hold 键

**现象**：游玩时长按 Hold 时可能会出现重复的 hold 键。

**当前最可能的三条**（都要真机日志/截图才能定，先不动代码）：
- [ ] **渲染侧**：`note.rs` Hold 分支里 `hold_keep_head` 为真时头不隐藏（`draw_below` 场景），
      而主体照画 ⇒ 一长一短两条并排。
- [ ] **fake note 顶上来**：见 #12 —— fake hold 在 `self.time` 之后**照样满不透明地画**，
      长按期间它一直在，很容易被当成"多出来的那条 hold"。
- [ ] **判定侧**：`judge.rs:593` / `:693` 对同一 touch 遍历 note，长按时会不会对同一个
      `NoteKind::Hold` 二次落 `NoteStatus::Hold`（要看 `judge:` 那行 `jc=` 才能证）。

**待查**：
- [ ] 长按触发了 note 的再次进入（`note_enter()` 被重复调用）
- [ ] touch id 复用导致同一根手指被当成两次
- [ ] `pull-log.ps1` 抓一段长按，看 `judge: ... jc=` 在长按窗口里是否同一条 note 出现两次

---

## 12. 虚假 hold 体积大小不定

**现象**：虚假 hold 的体积大小不定（渲染问题）。

**"虚假 hold" 就是 `Note::fake`** —— 谱面里的 fake note（渲染但不计入判定）：
`judge.rs:307` / `judge.rs:319` 构造 `idx` 时显式 `.filter(|it| !it.fake)`，
所以 fake note 永远停在 `JudgeStatus::NotJudged`，`auto_play_update` 也拿不到它。

**根因（已修，与 #10 是同一处）**：主体高度 `top - bottom` 可以为负 —— 上游
`// TODO (end_height - height) is not always total height` 自己承认了。
负值**同时**污染 `dest_size` 和贴图 `source`（v 高度为负 ⇒ 采样翻转），于是体积可以是任何值。
现在 `body = (top - bottom).max(0.)`，`body <= 0` 直接不画。

**留着没动的风险点（要上机确认 fake hold 的预期行为再改）**：
- [ ] Hold 分支**不走** `draw` 闭包，因此 `draw` 里那句
      `color.a *= if self.fake && res.time >= self.time { 0. } else { alpha }`
      对 hold **不生效** ⇒ fake hold 从 `self.time` 到 `end_time` 全程满不透明，
      到 `end_time` 再突然消失（普通 fake note 是淡出）。
      但 hold 在 `self.time` 之后**本来就得看得见**，所以不能照抄那句，得先确认预期。

---

## 附：抓日志

```powershell
rust-wp81\pull-log.ps1
```

拉全量 + 按消息去时间戳归组，输出 `ord=` / `untext` / `texdraw` 三张清单，末尾打印：
- `ALIVE=no` —— 手机/app 没了
- `ALIVE=yes but chars=STATIC` —— app 真的停在写日志
- `ALIVE=yes and chars=MOVING` —— app 正常，日志就是完整的

日志服务器：`http://192.168.1.4:8765/health`、`/log`、`/log?tail=N`、`/log?from=N`

改文件**只用 `edit`**，要执行的命令写进脚本。
