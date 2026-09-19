# ibus-qingjian 的发行版打包

Fedora（RPM）与 Arch（Pacman）各一套，产物都装三样东西：

| 内容 | Fedora | Arch |
|---|---|---|
| 引擎二进制 | `/usr/libexec/ibus-engine-qingjian` | `/usr/lib/ibus/ibus-engine-qingjian` |
| 组件 XML | `/usr/share/ibus/component/app.qingjian.ibus.xml` | 同左 |
| 随包数据 | `/usr/share/qingjian/`（dict.qj、english.tsv） | 同左 |

引擎按 `$QINGJIAN_DATA_DIR` → `/usr/share/qingjian` 找数据（`apps/linux/src/host/paths.rs`），
两个发行版落在同一个目录，组件 XML 的 `<exec>` 各自指自己的二进制路径（`@BIN_DIR@` 替换）。

## 数据从哪来

源码 tarball 是纯 git 源，随包数据在**构建期**生成：

- `dict.qj`：`dict-convert pack dict` 从仓库自带的 `assets/lexicon/dict.tsv` 快照现打（mmap 加载）。
- `english.tsv`：`assets/lexicon/english.tsv` 直接拷。
- `lm.qj` **不带**：语言模型要中文维基 + LCCC 语料统计，语料不在 git 里（`data/` 是 gitignore）。
  没有它引擎退化成一元词频整句（功能完整、整句质量降）。正式数据包发布后（`tools/release/data-bundle.sh`
  的 data Release 流程），可改成从锁文件取——到时候把 `lm.qj` 补进两边的安装清单即可。

## 构建

要求：仓库干净（打包取 `HEAD`）、本机有 cargo 与 git。

```sh
# Fedora（还要 rpm-build 包）
apps/linux/packaging/fedora/build.sh          # 产物在 target/rpm/

# Arch（还要 base-devel 的 makepkg）
apps/linux/packaging/arch/build.sh            # 产物在 target/Arch/，pacman -U 安装
```

版本号从 `apps/linux/Cargo.toml` 读（`0.1.4-dev` → 打包版本 `0.1.4`）。装完执行 `ibus restart`
并注销重登一次，输入源列表里加「青简」。

## 安装后

两个包的 post-install 都会尝试 `ibus write-cache` 刷新组件缓存（ibus 未装时跳过）。
卸载后输入源里如果残留「青简」，再跑一次 `ibus restart` 即可清掉。
