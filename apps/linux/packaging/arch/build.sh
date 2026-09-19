#!/usr/bin/env bash
# Arch 包本地构建：从 HEAD 打源码 tarball，与 PKGBUILD 一起放进临时目录跑 makepkg。
# 依赖：base-devel（makepkg）、cargo、git。用法：apps/linux/packaging/arch/build.sh
set -euo pipefail

root="$(cd "$(dirname "$0")/../../../.." && pwd)"
cd "$root"

version="$(sed -n 's/^version = "\(.*\)"$/\1/p' apps/linux/Cargo.toml | head -1 | sed 's/-dev$//')"
[[ -n "$version" ]] || { echo "读不到 apps/linux/Cargo.toml 的版本号" >&2; exit 1; }
# PKGBUILD 读不了 Cargo.toml、只能写死：版本对不上就报清楚，别让 makepkg 报「找不到源码包」
pkgver_declared="$(sed -n 's/^pkgver=//p' "$(dirname "$0")/PKGBUILD" | head -1)"
[[ "$version" == "$pkgver_declared" ]] || {
    echo "版本不一致：apps/linux/Cargo.toml 是 $version，PKGBUILD 是 $pkgver_declared；升版本时两处要一起改" >&2
    exit 1
}
# 打包取 HEAD：未提交的改动进不了包，提醒而不是静默漏掉
git diff --quiet || echo "警告：工作区有未提交的改动，它们不会进包（打包取 HEAD）" >&2

stage="$root/target/archbuild"
rm -rf "$stage"
mkdir -p "$stage"

echo "打源码包 ibus-qingjian-$version.tar.gz（取 HEAD）"
git archive --prefix="ibus-qingjian-$version/" -o "$stage/ibus-qingjian-$version.tar.gz" HEAD

cp "$(dirname "$0")/PKGBUILD" "$(dirname "$0")/ibus-qingjian.install" "$stage/"
cd "$stage"
# cargo 来自 rustup 而不是系统包时 pacman 看不到它，makepkg 会误报缺依赖；这种环境跳过依赖检查
deps=()
pacman -Q cargo >/dev/null 2>&1 || deps+=(--nodeps)
makepkg -sf --noconfirm "${deps[@]}"

outdir="$root/target/Arch"
mkdir -p "$outdir"
cp -v ./*.pkg.tar.zst "$outdir/" 2>/dev/null || cp -v ./*.pkg.tar.xz "$outdir/"
echo "完成：$(ls "$outdir")，安装：sudo pacman -U '$outdir'/ibus-qingjian-*.pkg.tar.*"
