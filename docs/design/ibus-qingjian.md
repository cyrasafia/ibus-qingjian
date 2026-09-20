# ibus 版青简

## 背景

本项目 fork 自`https://github.com/qingjian-team/qingjian` 

原项目的开发顺序是 Mac->Windows->Linux，暂时没有现成的 linux 版；且有明确的设计理念，其中一些和我的需求是冲突的，如：

- 以“顺便学一门语言”为理念，在候选词旁同时展示翻译，但我完全不需要
- 因为上述理由必须自绘选词界面，但我更倾向于原生选词界面（视觉风格应用服从系统）
- 考虑支持其他语言输入（如日语），我同样不需要

归根到底的原因是，Mac/Windows 用户需要一个功能差异点来作为“切换的理由”，而 Linux 用户只需要一个 “好用的输入法”

所以我决定先按照自己的需求做一个 Linux 壳，确定他是否适合作为我的主力输入法，然后再决定是否把代码贡献给原项目

## 目标

自用，先实现MVP

## Dos and Don'ts

- 只实现 Gnome+wayland+ibus， 最小可用
- 只考虑拼音+双拼（小鹤方案），不做辅助码
- 不做图形化配置界面，走配置文件
- 使用 ibus 原生选词界面
- 要实现个人词库导入导出，可以是脚本+API形式

## 调研：ibus engine 怎么接（2026-09-18）

### ibus 的架构

四个角色，全部走 ibus 专属 D-Bus socket（不是 session bus）：

| 角色 | 谁在做 |
|---|---|
| 输入上下文 | 应用侧（GTK/Qt 的 IM 模块；GNOME Wayland 下是 mutter/gnome-shell 统一建） |
| 总线 | `ibus-daemon`，转发按键、管理引擎与面板 |
| 引擎 | 独立进程，被 daemon 按需拉起，只通过 D-Bus 说话，不碰显示协议 |
| 面板（候选窗） | GNOME 下由 gnome-shell 自己实现（不是 ibus-ui-gtk3） |

对我们的关键结论：

- **「原生选词界面」零成本拿到。** 引擎只发 `UpdateLookupTable` 信号，候选窗、辅助行、内联 preedit 全由 gnome-shell 按系统主题渲染，Wayland 下定位跟随 `SetCursorLocation`。
- socket 地址发现：先看 `$IBUS_ADDRESS`，否则读 `~/.config/ibus/bus/<machine-id>-unix-wayland-0`（X11 是 `<machine-id>-unix-0`）。
- 引擎进程不需要 DISPLAY/Wayland 权限，纯后台进程，与 Windows 的 Server 进程模型同构。

### 生命周期与注册

- 引擎用一份 component XML 注册，字段见 ibus-libpinyin 的样例（`<component>` 包 `<engines><engine>`，engine 有 name / language / layout / longname / symbol / rank）。`<exec>` 是 daemon 拉起引擎的命令行，惯例带 `--ibus` 参数。
- **组件搜索目录只有 `/usr/share/ibus/component/`。** 用户级目录（`~/.local/share/ibus/component`）在 ibus 源码 `ibusregistry.c` 里被 `#if 0` 注释掉了——不要指望它；开发时用 `$IBUS_COMPONENT_PATH` 给 daemon 加搜索路径，或一次性 sudo 放进系统目录。
- 启动时序：用户切到引擎 → daemon 执行 `<exec> --ibus` → 进程连 socket、`Hello` → 在 `/org/freedesktop/IBus/Factory` 暴露 `CreateEngine` 方法 → request 一个 well-known name → daemon 调 `CreateEngine(name)` → 进程在 `/org/freedesktop/IBus/Engine/<n>` 暴露 `org.freedesktop.IBus.Engine` 对象 → 此后每个按键都是对这个对象的方法调用。
- component XML 里 `<observed-paths>` 指向引擎二进制或配置文件，文件变了 daemon 自动重启引擎——开发时重编译即生效，可省掉手动 `ibus restart`。

### D-Bus 接口面

daemon → 引擎，方法调用（引擎要实现）：

| 方法 | 说明 |
|---|---|
| `ProcessKeyEvent(keyval, keycode, state) → b` | 逐键到达；返回 true 表示吃掉。keyval 是 X keysym，state 是 GDK 风格修饰位（Shift=1<<0、Ctrl=1<<2、Alt=1<<3、Super=1<<6，释放事件 1<<30） |
| `FocusIn` / `FocusOut` / `Enable` / `Disable` / `Reset` | 焦点与生命周期 |
| `SetCapabilities` | 客户端声明支持内联 preedit / 候选表 / 环绕文本与否 |
| `SetCursorLocation` | 光标矩形，转发即可（面板自己定位候选窗） |
| `PageUp` / `PageDown` / `CursorUp` / `CursorDown` / `CandidateClicked` | 来自候选窗的鼠标交互（滚轮、点击） |
| `SetSurroundingText` | 光标前文（GTK 应用才有）；MVP 不接 |
| `PropertyActivate` 等 | 引擎菜单属性；MVP 不做 |

引擎 → daemon，信号（驱动 UI）：

| 信号 | 说明 |
|---|---|
| `CommitText` | 上屏 |
| `UpdatePreeditText(text, cursor_pos, visible)` | 内联在应用里的 preedit（带下划线），cursor 按字符数计 |
| `UpdateLookupTable(table, visible)` | 候选表：纯文本候选数组 + labels + 每页大小（1–16）+ 光标 + 是否循环 |
| `UpdateAuxiliaryText` | 候选窗旁的辅助行——放拼音串 |
| `RegisterProperties` / `UpdateProperty` | 菜单与状态图标；MVP 不做 |

两个要意识到的限制：

- LookupTable 的候选是**纯字符串**，没有「右侧注解列」这类东西——本项目本来就不展示翻译，正好；拼音放辅助行。
- 键盘翻页 / 数字选词仍然作为普通按键进 `ProcessKeyEvent`，由引擎处理（读 Core 的 `[shortcut]`）；`PageUp` / `CandidateClicked` 这些方法调用只来自鼠标。

### 候选路线与结论

| 路线 | 结论 |
|---|---|
| libibus C API + GObject FFI | 无维护中的 Rust 绑定；自己绑要 GLib 主循环 + GObject 子类化，unsafe 面大；否 |
| crates.io `ibus`（ArturKovacs/ibus-rs） | 是**客户端**库（造 InputContext、发假键），不做引擎侧；2022 年起停更，依赖旧 `dbus` crate；否 |
| Python 壳 + PyO3 桥 Core | 两种运行时、两套打包，部署翻倍；否 |
| **zbus 直连 ibus 协议** | 已有先例验证：librush（pmim-ibus 在用）、Bonolith（日语输入法）；**选定** |

zbus 直连再分两步走：

1. **MVP 直接依赖 [librush](https://github.com/fm-elpac/librush)**（crates.io `librush`）：纯 Rust、`deny(unsafe_code)`、zbus 5、把 ibus 源码逐文件对着注释；地址发现、factory、`IBusEngine` trait、LookupTable 序列化都齐。许可证 LGPL-2.1-or-later **或** GPL-3.0-or-later 双选，取 GPL-3.0-or-later 与本仓库一致。
2. 它只覆盖最小接口（没暴露 `SetSurroundingText` / `SetCapabilities` / Property 系列），MVP 够用；要扩时给它提 PR，或把这几百行协议层内化进自己的壳（协议面就 factory + engine 两份 XML，可控）。

### 与 Core 的对接

壳只做架构约束允许的两件事：

- **按键**：keysym + state → Core 的按键输入；Core 的帧 → `UpdatePreeditText` + `UpdateAuxiliaryText` + `UpdateLookupTable`；上屏 → `CommitText`。观感落点（真机验证过，2026-09-19）：内联 preedit 显示拼音 marked text（带 `'` 分隔与字符光标，同 mac 壳、光标编辑直接可见），**辅助行同帧再发一遍拼音**——主流 ibus 引擎（libpinyin 等）都发辅助行，不支持内联 preedit 的客户端（XIM、未声明能力的 text-input 路径）只有这条路能看到拼音；代价是内联可见的客户端会看到应用内与候选窗各一份拼音，观感能否接受真机继续用下来再定。候选表只放候选文本，排布方向按 `[general] layout` 显式下发（gnome-shell 把 `System` 当竖排处理、不回退系统设置，缺省发 `System` 等于横排永不生效）。
- **进程模型**：ibus 会为每个 input context 各调一次 `CreateEngine`，多个 engine 对象全部转发到**进程级单例 Core `Engine`**（同 mac 壳 `host.rs` 的模式），`focus_in` 只切活跃对象。
- 双拼小鹤：`[general] shuangpin = "flypy"`，Core 已有，无壳侧逻辑。
- 依赖树只带 core / dictionary / lm / learning / format / platform；**translate / predict / neural / render 全部不进**——Core 的 `Translator` / `Predictor` trait 留空实现，候选照常出（mac/win 壳做不到这么干净，Linux 壳反而最贴「平台只是壳」）。
- panic 边界：方法回调包 `catch_unwind`（对齐 mac 壳），学习数据沿用 Core 既有的 60 秒落盘。

### 与 architecture.md「Linux：Fcitx5 / IBus」规划的对照（2026-09-18 补）

原仓库 `docs/design/architecture.md`（2026-09-15 定，PR #90 在做）对 Linux 的既有规划是「与 Windows 同构」：
Core 跑独立 Server 进程（Unix socket 复用 `qingjian-platform` 协议），Fcitx5 壳是薄 C++ 插件只做转发，
候选窗由 Server 自绘，IBus 壳「以后用 zbus 写成纯 Rust，共用同一个 Server」。核对结果：

**一致**：IBus 壳用 zbus 纯 Rust 实现——与该规划原话相同，路线互相印证。

**分歧（本 fork 有意为之）**：

- **不共用 Server，ibus 引擎进程单进程直接嵌 Core**（同 mac 壳模式）。upstream 拆 Server 的理由是
  Fcitx5 的 addon 跑在所有输入法共用的 fcitx5 进程里，Core 崩了会带倒 Fcitx5；而 ibus 的引擎本来就是
  ibus-daemon 拉起的独立进程，崩溃隔离免费拿到（daemon 会按需重新拉起）。本 fork 又用不上 Server 里的
  自绘候选窗与翻译装配，双进程只剩「多一跳逐键 IPC + 多一套协议编解码 + 多一个常驻进程」的成本。
  `qingjian-platform` 在 Linux 壳只取 `Config`，`protocol` 类型不需要。
- **候选窗用 ibus 原生 LookupTable 而非 Server 自绘**——fork 的既定取舍（见 Dos and Don'ts），代价是观感完全交给 GNOME。
- **顺序倒过来**：upstream 先 Fcitx5 后 IBus（排在 Fcitx5 真机验收之后）；本 fork 只做 GNOME+Wayland+ibus，Fcitx5 不在范围。

将来若把代码贡献回原项目：architecture.md 已计划「三个平台的按键分流抽成公共 crate」，单进程 ibus 壳的
按键分流逻辑同样能搬进那个公共层，分歧不构成合流障碍。

### 目录与路径

| 什么 | 放哪 |
|---|---|
| 代码 | `apps/linux`（package 名 `qingjian-linux`，二进制名 `ibus-engine-qingjian`） |
| 组件 XML | `apps/linux/data/app.qingjian.ibus.xml`；`<name>` 与进程内 request 的 D-Bus 名一致（`app.qingjian.ibus`，daemon 按 NameOwnerChanged 绑进程）；安装到 `/usr/share/ibus/component/`（`scripts/install-dev.sh` 替换 `@BIN_DIR@` 后装入） |
| 发行版打包 | `apps/linux/packaging/`：Fedora spec（`/usr/libexec`）与 Arch PKGBUILD（`/usr/lib/ibus`）+ 各自 build.sh，数据构建期从 `assets/lexicon` 生成（`lm.qj` 暂不带），见该目录 README |
| 配置 | `~/.config/qingjian/config.toml`（60 秒 mtime 热加载，随学习数据落盘任务一起） |
| 学习数据 | `~/.local/share/qingjian/`（六张 TSV 原样） |
| 随包数据 | `$QINGJIAN_DATA_DIR` → `/usr/share/qingjian/`（dict.qj / lm.qj / english.tsv；开发时把环境变量指到仓库 `assets/lexicon/` 即可用现有 TSV） |

### 开发与调试

1. `cargo build -p qingjian-linux`。
2. 组件 XML 的 `<exec>` 指到 `target/debug/ibus-engine-qingjian`，放进 `/usr/share/ibus/component/`（sudo，一次性），或给 daemon 设 `IBUS_COMPONENT_PATH`。
3. `ibus write-cache && ibus restart`；`ibus engine qingjian` 可直接从命令行切引擎。
4. GNOME 设置 → 键盘 → 输入源加「青简」。
5. 日志看 `journalctl --user -f`（daemon 拉起的进程输出进 journal）。

Core 侧改动照旧先跑 `qingjian-cli`；壳层手测用一个 GTK 应用（gnome-text-editor）即可。

### 个人词库导入导出

- 学习数据本就是 TSV：导出 = `user.tsv` + `user-words.tsv`（可带 `user-ngram.tsv`），导入 = 合并去重后原子写回。
- 形式：`qingjian-cli` 加子命令（如 `--export-user-dict <dir>` / `--import-user-dict <dir>`），走 `qingjian-learning` 的公开 API，不手工拼文本。

### 风险与开放问题

- **多 input context 并发**：两个应用交替打字时共享同一个 Core 组合态的行为要定。倾向 `focus_out` 清组合（ibus 引擎惯例），mac 壳是切输入源才收窗——两者取舍实现时验证。
- **引擎收不到应用身份**：ibus 不把 app id 告诉引擎（X11 下还能自己查焦点窗口，Wayland 下没有途径），`[apps] english_candidates_off` 这类按应用配置在 ibus 壳没有数据来源；MVP 不做按应用行为，需要时再议（如仅 X11 支持或砍掉该功能）。
- **ibus 版本差异**：1.5.x 之间有 `FocusInId` 这类新增方法；目标只认 1.5.29 一线（Arch 当前版本）。
- **客户端不支持内联 preedit**（`SetCapabilities` 没给 PREEDIT_TEXT）：daemon 把 preedit 转给面板显示（gnome-shell 的候选窗有 preedit 行），引擎侧的兜底是辅助行拼音（已发）。
- **候选窗观感完全交给 GNOME**：不能自定义字体间距主题（既定取舍，换主题是系统的事）。
- **librush 维护度**：个人项目、更新不勤；好在协议层小，出问题就内化，不构成架构风险。

## 实现状态（2026-09-19）

MVP 已落地（`apps/linux`，实现要点见 `docs/notes/crate-notes.md`「apps/linux」）：

- [x] `apps/linux` crate：librush 协议层 + `QingjianEngine` / `QingjianFactory`，进程级单例 `Host`（`Arc<Mutex<…>>`）
- [x] 按键分流 `host/keys.rs`（照 mac 壳语义移植：字母 / 数字选词 / 空格上屏 / 回车原样 / 退格 / Esc / 方向键
      （←→ 拼音光标、↑↓ 高亮）/ Home End / PageUp PageDown / 翻页键 `[` `]` / Tab 翻页、Shift+Tab 上一页 / 标点全角 / 大写字母临时英文 / 直输段 / 问字与表达式模式）
- [x] 中英切换按配置：`[shortcut] switch_mode` 认 `shift`（缺省）/ `control` / `none`（`ctrl+space` 与 GNOME 系统切换冲突，
      按不切换处理并告警）；`[general] english_mode = false` 时切换键停用（对齐 Windows，issue #81）；Caps Lock 亮 = 纯直通
- [x] Ctrl / Alt / Super 组合放行（组句中先原样上屏再放行，缓冲不丢）
- [x] 候选窗交互：`CandidateClicked`（页内下标换算）、`PageUp` / `PageDown` / `CursorUp` / `CursorDown`（鼠标滚轮）
- [x] 失焦 / 停用 = 拼音原样上屏 + 断链；`Reset` = 清空
- [x] 排布方向按 `[general] layout` 显式下发（2026-09-19）：gnome-shell 把 `System` 当竖排、不回退 gsettings，
      librush 缺省恰是 `System`——不显式下发横排永远立不起来。librush 0.2.3 没导出 `IBusOrientation` 类型，
      走 `vendor/librush` 补一行 re-export（根 Cargo.toml 的 `[patch.crates-io]`，上游收了就撤）
- [x] 辅助行拼音兜底（2026-09-19）：同帧把拼音发 `UpdateAuxiliaryText`，无内联 preedit 能力的客户端
      （XIM / 部分 text-input 路径）在候选窗里也有拼音可看——同日定位真机上「preedit 无法显示」
- [x] headless 集成测试台 `apps/linux/tests/`（2026-09-19）：独立 socket + 独立 HOME 起真 ibus-daemon 与引擎，
      python GI 模拟 GTK 客户端逐键打字断言 preedit / 辅助行 / 候选方向 / 上屏。**教训：测试客户端逐键必须
      异步 + 主循环空转，同步调用夹 `sleep` 会把 GDBus 信号分发饿死，看起来像引擎丢信号**
- [x] 数据与路径：XDG 配置 / 学习数据、`$QINGJIAN_DATA_DIR` 随包数据（开发指 `assets/lexicon`）、60 秒落盘 + 配置热加载
- [x] panic 边界（`catch_unwind` + 锁毒化恢复）、组件 XML + `install-dev.sh`
- [x] 测试：keymap 翻译、会话分页 / 高亮 / 数字选格、带真实 Engine 的按键流（样例词库）、排布方向映射
- [x] 真机验证（2026-09-19，GNOME+Wayland）：横排 / 竖排随 `[general] layout` 生效，preedit 与辅助行拼音可见
- [ ] 真机验收余项：候选窗观感细节、光标定位跟随、journal 日志、各客户端类型（GTK3 / XIM / Electron）覆盖
- [ ] 个人词库导入导出（学习数据是 TSV，先能手工拷贝；CLI 子命令 API 化待做）
- [ ] SetSurroundingText（前文，联想 / 重排要用时再接）、SetCapabilities 探测、Property 菜单
- [x] 附加词库接线（2026-09-19）：随包领域词库 + 用户 `dicts/` 目录（`host/dictionaries.rs`），
      `[dictionaries]` 开关热加载、目录文件增删 / 更新 1 秒轮询发现；打包带 `assets/lexicon/dicts/*.tsv`
- [x] 删候选（2026-09-20）：Shift+数字（`[shortcut] delete_candidate`，缺省 ⇧）删当前页第 N 个候选——
      数字按 keyval + 物理键码认：键码在 GTK 直连（X 码 10–18）与 gnome-shell 的 text-input 路径
      （evdev 裸码 2–10）两种惯例下不一样，只认一种就是当天真机失灵的根因；
      删完重查，提示并排在辅助行拼音右侧、敲下一键就没（内联 preedit 不掺，那是应用里的 marked text）；
      那格没候选吞键；e2e 场景 `HARNESS_CLIENT=apps/linux/tests/ibus_forget_client.py`
- [ ] `[general] input_log` 输入日志未接（配置里写了不生效，见 crate-notes）；
      `docs/user/` 用户文档未动（fork 尚未建 Linux 用户文档，真机验收后再写）
