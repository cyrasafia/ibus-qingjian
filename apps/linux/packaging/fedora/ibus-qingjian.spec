# Fedora RPM 的 spec。本地构建走同目录的 build.sh（git archive 出源码包再 rpmbuild）：
#   apps/linux/packaging/fedora/build.sh
# 也可以自己：
#   git archive --prefix=ibus-qingjian-0.1.4/ -o ibus-qingjian-0.1.4.tar.gz HEAD
#   rpmbuild -bb ibus-qingjian.spec
#
# 版本号与 apps/linux/Cargo.toml 同步维护（0.1.4-dev 打成 0.1.4）。

%global debug_package %{nil}

Name:           ibus-qingjian
Version:        0.1.4
Release:        1%{?dist}
Summary:        Qingjian pinyin input method engine for IBus

License:        GPL-3.0-or-later
# 上面的 License 只算了代码。随包数据自带许可（pack 命令把元数据打进了 .qj）：
# dict.qj 是 MIT AND Unicode-3.0、english.tsv 见 assets/lexicon/README.md。
# 本地 dnf install 装着用没关系；要是哪天提交进 Fedora 官方仓库，License 字段要写成
# GPL-3.0-or-later AND MIT AND Unicode-3.0（英文词表的许可核对后并入）。
URL:            https://github.com/cyrasafia/ibus-qingjian
Source0:        %{name}-%{version}.tar.gz

BuildRequires:  cargo
BuildRequires:  gcc
# 运行期只要 ibus-daemon 在（组件缓存）；写缓存的调用在脚本里做了容错
Requires:       ibus

%description
Qingjian (青简) pinyin engine for the IBus input framework: full pinyin and
shuangpin, native IBus candidate window. Candidates, ranking and learning all
run locally; the engine process talks to ibus-daemon over D-Bus only.

%prep
%autosetup -n %{name}-%{version}

%build
# 只构建要装的成员（workspace 里的 macOS 壳在 Linux 上编译不过）
cargo build --release --locked -p qingjian-linux -p qingjian-dict-convert
# 词库在构建期从仓库自带的词表快照打成 .qj（mmap 加载）；语言模型语料不在源码树里，不带
cargo run --release -q -p qingjian-dict-convert -- pack dict \
    --input assets/lexicon/dict.tsv \
    --name "青简基础词库" \
    --license "MIT AND Unicode-3.0" \
    --attribution "通用规范汉字表；现代汉语常用词表（liuxilu 校对版）；THUOCL（清华大学自然语言处理实验室，MIT）；读音 Unihan（Unicode）" \
    --source https://github.com/qingjian-team/qingjian/tree/main/assets/lexicon
# 组件 XML 的占位符换成实际路径与版本
sed -e "s|@BIN_DIR@|%{_libexecdir}|g" -e "s|@VERSION@|%{version}|g" \
    apps/linux/data/app.qingjian.ibus.xml > app.qingjian.ibus.xml

%install
install -Dm755 target/release/ibus-engine-qingjian \
    %{buildroot}%{_libexecdir}/ibus-engine-qingjian
install -Dm644 app.qingjian.ibus.xml \
    %{buildroot}%{_datadir}/ibus/component/app.qingjian.ibus.xml
install -Dm644 data/generated/dict.qj \
    %{buildroot}%{_datadir}/qingjian/dict.qj
install -Dm644 assets/lexicon/english.tsv \
    %{buildroot}%{_datadir}/qingjian/english.tsv

%post
# 刷新 ibus 组件缓存；ibus 还没跑起来时失败也无妨（下次 ibus restart 会再刷）
ibus write-cache >/dev/null 2>&1 || :

%postun
ibus write-cache >/dev/null 2>&1 || :

%files
%license LICENSE
%doc README.md
%{_libexecdir}/ibus-engine-qingjian
%{_datadir}/ibus/component/app.qingjian.ibus.xml
%dir %{_datadir}/qingjian
%{_datadir}/qingjian/dict.qj
%{_datadir}/qingjian/english.tsv

%changelog
* Fri Sep 18 2026 ibus-qingjian packager <noreply@example.com> - 0.1.4-1
- 初次打包：ibus 引擎 MVP（全拼 / 双拼、原生候选窗）
