#!/usr/bin/env bash
# Fedora RPM 本地构建：从 HEAD 打源码 tarball，丢给 rpmbuild。产物收集到 target/rpm/。
# 依赖：rpm-build、cargo、git。用法：apps/linux/packaging/fedora/build.sh
set -euo pipefail

root="$(cd "$(dirname "$0")/../../../.." && pwd)"
cd "$root"

version="$(sed -n 's/^version = "\(.*\)"$/\1/p' apps/linux/Cargo.toml | head -1 | sed 's/-dev$//')"
[[ -n "$version" ]] || { echo "读不到 apps/linux/Cargo.toml 的版本号" >&2; exit 1; }
# spec 读不了 Cargo.toml、只能写死：版本对不上就报清楚，别让 rpmbuild 报「找不到源码包」
spec_version="$(sed -n 's/^Version:[[:space:]]*//p' "$(dirname "$0")/ibus-qingjian.spec" | head -1)"
[[ "$version" == "$spec_version" ]] || {
    echo "版本不一致：apps/linux/Cargo.toml 是 $version，spec 是 $spec_version；升版本时两处要一起改" >&2
    exit 1
}
# 打包取 HEAD：未提交的改动进不了包，提醒而不是静默漏掉
git diff --quiet || echo "警告：工作区有未提交的改动，它们不会进包（打包取 HEAD）" >&2

topdir="$root/target/rpmbuild"
rm -rf "$topdir"
mkdir -p "$topdir"/{BUILD,RPMS,SOURCES,SPECS,SRPMS}

echo "打源码包 ibus-qingjian-$version.tar.gz（取 HEAD）"
git archive --prefix="ibus-qingjian-$version/" -o "$topdir/SOURCES/ibus-qingjian-$version.tar.gz" HEAD

cp "$(dirname "$0")/ibus-qingjian.spec" "$topdir/SPECS/"
rpmbuild -bb --define "_topdir $topdir" "$topdir/SPECS/ibus-qingjian.spec"

outdir="$root/target/rpm"
mkdir -p "$outdir"
find "$topdir/RPMS" -name "*.rpm" -exec cp -v {} "$outdir/" \;
echo "完成：$(ls "$outdir"/*.rpm)，安装：sudo dnf install '$outdir'/ibus-qingjian-*.rpm"
