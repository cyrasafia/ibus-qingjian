#!/usr/bin/env python3
"""headless ibus 集成测试台：起独立 ibus-daemon + 青简引擎，模拟 GTK 客户端逐键打字，
校验 preedit / 辅助行 / 候选表（含排布方向）/ 上屏都真的流过真实 daemon。

用法：python3 apps/linux/tests/ibus_harness.py
环境变量：HARNESS_LAYOUT=vertical|horizontal（写入引擎配置的 [general] layout）
          HARNESS_CLIENT=<path>（换自定义客户端脚本）
依赖：ibus-daemon、python-gobject（gi + IBus gir）、构建好的 target/debug 引擎。

注意：客户端必须让 GLib 主循环真正空转（逐键用 timeout 异步发），在回调里 time.sleep
会把信号饿死——历史教训见 docs/design/ibus-qingjian.md。
"""
import os
import subprocess
import sys
import time
import xml.etree.ElementTree as ET

TEST = "/tmp/opencode/qingjian-ibus-test"
ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", "..", ".."))
HOME = f"{TEST}/home"
SOCK = f"{TEST}/socket"
COMP_DIR = f"{TEST}/component"
BIN_DIR = f"{ROOT}/target/debug"

os.makedirs(f"{HOME}/.config", exist_ok=True)
os.makedirs(f"{HOME}/.cache", exist_ok=True)
os.makedirs(COMP_DIR, exist_ok=True)

config_dir = f"{HOME}/.config/qingjian"
os.makedirs(config_dir, exist_ok=True)
layout = os.environ.get("HARNESS_LAYOUT", "vertical")
with open(f"{config_dir}/config.toml", "w") as f:
    f.write(f'[general]\nlog_level = "debug"\nlayout = "{layout}"\n')

# 组件 XML：替换路径占位
xml_path = f"{ROOT}/apps/linux/data/app.qingjian.ibus.xml"
tree = ET.parse(xml_path)
tree.getroot().find("exec").text = f"{BIN_DIR}/ibus-engine-qingjian --ibus"
observed = tree.getroot().find("observed-paths")
observed.find("path").text = f"{BIN_DIR}/ibus-engine-qingjian"
tree.write(f"{COMP_DIR}/app.qingjian.ibus.xml", encoding="utf-8", xml_declaration=True)

env = dict(
    os.environ,
    HOME=HOME,
    XDG_CONFIG_HOME=f"{HOME}/.config",
    XDG_CACHE_HOME=f"{HOME}/.cache",
    XDG_DATA_HOME=f"{HOME}/.local/share",
    QINGJIAN_DATA_DIR=f"{ROOT}/assets/lexicon",
    IBUS_COMPONENT_PATH=COMP_DIR,
    IBUS_ADDRESS=f"unix:path={SOCK}",
    IBUS_DISABLE_SNOOPER="1",
)

# 清掉连着本测试 socket 的残留引擎（不动真实会话）
for pid in [p for p in os.listdir("/proc") if p.isdigit()]:
    try:
        with open(f"/proc/{pid}/environ", "rb") as f:
            blob = f.read()
        if f"IBUS_ADDRESS=unix:path={SOCK}".encode() in blob:
            os.kill(int(pid), 9)
    except OSError:
        pass
try:
    os.unlink(SOCK)
except FileNotFoundError:
    pass

log = open(f"{TEST}/ibus-daemon.log", "w")
daemon = subprocess.Popen(
    ["ibus-daemon", "--replace", "--single", "--panel", "disable",
     "--address", f"unix:path={SOCK}", "--verbose"],
    env=env, stdout=log, stderr=subprocess.STDOUT,
)
for _ in range(100):
    if os.path.exists(SOCK) or daemon.poll() is not None:
        break
    time.sleep(0.1)
time.sleep(0.5)
if daemon.poll() is not None:
    print("ibus-daemon 退出：")
    print(open(f"{TEST}/ibus-daemon.log").read())
    sys.exit(1)

client = subprocess.Popen(
    [sys.executable, os.environ.get("HARNESS_CLIENT",
                                    f"{ROOT}/apps/linux/tests/ibus_client.py")],
    env=env,
    stdout=sys.stdout, stderr=sys.stderr,
)
try:
    rc = client.wait(timeout=60)
finally:
    client.kill()
    daemon.terminate()
    daemon.wait()
print("client rc =", rc)
sys.exit(rc)
