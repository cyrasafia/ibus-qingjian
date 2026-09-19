# 用户词库的导入、更新与删除（2026-09-19）

设置页的「词库」页背后是文件与三层 API：`qingjian_dictionary::import`（转换落盘）、`extra_dictionaries`（列目录、加载）、
壳的移除（挪文件）。本文写给不走设置页、用脚本或直接操作文件的人：从其他输入法迁移、批量导入、自动化清理。
格式与 UI 措辞见 [user/settings/dictionaries.md](../user/settings/dictionaries.md)，架构定位见 [design/architecture.md](../design/architecture.md) 的「多词库」。

## 三个动作都怎么落

**导入**：`qingjian_core::dictionary::import::import(source, dest_dir)`（`crates/qingjian-dictionary/src/import/mod.rs`）。
自由函数，不依赖 Engine，脚本直接调即可。接受青简 TSV（`词\t拼音\t词频`）、Rime `.dict.yaml`、现成 `.qj` 三种，
统一转成 `dest_dir/<源文件主干>.qj` 并返回路径、名称、条数。Windows 设置页（`apps/windows/settings/src/panel/pages/dictionaries.rs`）就是这么用的。

**更新**：没有单独的更新 API——同名重新导入即覆盖（`law.dict.yaml` 两次导入都写 `law.qj`）。
目录监控（mac `Host::reload_dictionaries`；Windows Server 每次轮询比对 `dicts/` 的路径 / mtime / 长度快照，
`extra_dictionaries::snapshot`）发现同名更新、新增、移除都自动重载，不用重启。

**删除**：core 没有 API，是壳层文件操作，且**挪不真删**：文件移到 `dicts/removed/`（可手工找回），
随包词库不能移除。mac 在 `apps/macos/src/host/dictionaries/mod.rs` 的 `remove_dictionary`（顺带清 `[dictionaries] disabled` 里的项），
Windows 在设置页 `remove_user_dict`。脚本等价操作：把 `dicts/<名>.qj` 挪出目录即可，引擎自动发现；
`disabled` 里残留该文件名无害（`is_enabled` 只在文件存在时才起作用），但配置会脏，建议一并清。

## 推荐放哪

用户导入词库的目录是用户数据目录下的 `dicts/`，三平台：

| 平台 | 目录 |
|---|---|
| macOS | `~/Library/Application Support/Qingjian/dicts/` |
| Windows | `%APPDATA%\Qingjian\dicts\` |
| Linux | `~/.local/share/qingjian/dicts/`（XDG 约定见 `apps/linux/src/host/paths.rs`；**壳尚未接线**，见下「坑」） |

推荐走 `import()` 落 `.qj`（mmap 启动快、带名称与许可证元数据）；目录也直接认裸 `.tsv`（`EXTENSIONS = ["qj", "tsv"]`，
同名 `.qj` 优先），小词库丢 TSV 省一步转换。Rime `.dict.yaml` **不能**直接丢进去——目录加载不认，必须经 `import()` 转换。

别放错层：随包领域词库（`Resources/dicts/`）是打包固定的，`[dictionaries] domains` 管开关；学习数据
（`user-words.tsv` 等，见 [crate-notes.md](crate-notes.md) `qingjian-learning` 节）没有导入 API（个人词库导入导出 API 化待做），
手工合并属黑箱操作，迁移词频请导成用户词库。

## 容易踩的坑

按从转换到加载的顺序：

1. **文件名就是词库名**。目标 `.qj` 取源文件主干（`law.dict.yaml` → `law.qj`），同名覆盖。两个不同来源的同义词库
   若文件名不同（`rime-词库.yaml` 与 `my-dict.yaml`）会变成两本叠加，权重翻倍；先删旧再导新。
2. **空词库拒绝**：一条可用词条都没有（全空 / 全坏行）报错，不产出半空的 `.qj`。别拿纯表头 YAML 导。
3. **Rime 的坑**：YAML 头只取 `name:`；不支持 `import_tables` 引别的表、自定义 `columns` 列顺序、`.schema.yaml`；
   权重非整数（`1e5` 这类科学计数）当 1。判断是不是 Rime 文件的依据是首条非注释行为 `---` 或 `name:`——
   所以没有 YAML 头的纯 TSV 会按青简 TSV 直接解析，恰好是快照迁移想要的路径。
4. **音节写法**：词库用 `v` 表示 ü；`lue` / `nue` 在导入时自动归一为 `lve` / `nve`。双拼方案的 Rime 用户词典（userdb）
   存的是**双拼码**不是全拼，迁移要转写；全拼方案（luna_pinyin 等）的 userdb 导出快照（每行 `词\t编码\t次数`）就是青简 TSV 格式。
   导入不做拼音校验：音节不合法的词只是查不到，不报错、也不影响别的词——大词库导入后拿 `apps/cli` 抽查几条。
5. **量纲**：userdb 快照的第三列是上屏次数（小整数），青简词频是语料次数（千级量纲）。直接用词只会「有但偏后」；
   想让它像学过的词排位，脚本里乘个系数（如 ×100）再导。
6. **同名 `.qj` 压过 `.tsv`**：目录里两样并存只加载 `.qj`。更新了 `.tsv` 却没见生效，先看有没有同名的 `.qj` 挡着。
7. **移除不是删除**：`removed/` 里躺着的是能找回的备份；真要清就把 `removed/` 一起删。反向坑：手工恢复词库
   从 `removed/` 挪回 `dicts/` 即可，引擎自动发现。
8. **坏文件不拖垮输入法**：目录里解析失败的文件只记日志跳过。导入「成功」不代表每条都能打——见第 4 条音节校验。
9. **配置坏时词库沿用上次有效开关**：`[dictionaries]` 解析失败时已加载的词库不卸载；修好配置自动恢复。脚本改配置
   注意保留 TOML 结构（`Config::set_value` / `set_array` 而不是整个重写）。
10. **Linux 壳还没接线**（本 fork 待办）：`apps/linux` 未调 `Engine::set_extra_dictionaries`，`[dictionaries]` 写了不生效、
    `dicts/` 放了不加载。脚本可以先把文件放到位，接线完成后即全部生效。接线照 `apps/windows/server/src/dispatch/reload` 抄即可
    （`extra_dictionaries::load` + 轮询 `snapshot`）。
11. **导入词库不带语言模型**：附加词库的词在整句里按 `sentence::fallback_log_prob` 兜底打分，只影响「有没有」与词频，
    不改变整句权重尺度。几十万条的巨型词库导进来不会让整句变聪明，但会增内存与查询面，按需拆分。