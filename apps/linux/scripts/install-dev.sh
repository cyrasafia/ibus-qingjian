#!/usr/bin/env bash
# 开发安装：把组件 XML 指到 target/debug 的二进制并装进系统组件目录，让 ibus 认到这个引擎。
# 用法：apps/linux/scripts/install-dev.sh [--release]
# 卸载：sudo rm /usr/share/ibus/component/app.qingjian.ibus.xml && ibus restart
set -euo pipefail

root="$(cd "$(dirname "$0")/../../.." && pwd)"
profile="debug"
if [[ "${1:-}" == "--release" ]]; then
    profile="release"
elif [[ -n "${1:-}" ]]; then
    echo "用法: $0 [--release]" >&2
    exit 1
fi
bin_dir="$root/target/$profile"
bin="$bin_dir/ibus-engine-qingjian"

if [[ ! -x "$bin" ]]; then
    echo "先构建：cargo build -p qingjian-linux${1:+ --release}" >&2
    exit 1
fi

component_dir="${IBUS_COMPONENT_DIR:-/usr/share/ibus/component}"
tmp="$(mktemp)"
sed -e "s|@BIN_DIR@|$bin_dir|g" -e "s|@VERSION@|dev|" \
    "$root/apps/linux/data/app.qingjian.ibus.xml" > "$tmp"

echo "装组件到 $component_dir/app.qingjian.ibus.xml（要 sudo）"
sudo mkdir -p "$component_dir"
sudo install -m 644 "$tmp" "$component_dir/app.qingjian.ibus.xml"
rm -f "$tmp"

# 数据：开发时用仓库的生成数据（没有生成数据就先跑 tools 生成，或指向已有安装）
if [[ -z "${QINGJIAN_DATA_DIR:-}" ]]; then
    if [[ -d "$root/data/generated" ]]; then
        echo "提示：export QINGJIAN_DATA_DIR=$root/data/generated 用生成数据"
    fi
fi

ibus write-cache
ibus restart
echo "完成。GNOME 设置 → 键盘 → 输入源添加「青简」，或直接：ibus engine qingjian"
