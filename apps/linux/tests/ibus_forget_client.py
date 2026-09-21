#!/usr/bin/env python3
"""模拟 ibus 客户端（删候选场景）：打 nihao，分别用 evdev 裸码与 X 码两种惯例
Shift+数字删候选（gnome-shell 的 text-input 路径送 evdev 裸码、GTK 直连送 X 码，
2026-09-20 真机失灵的根因就是只认了 X 码），校验两条路都出提示、不上屏、preedit 不掺。

用法：HARNESS_CLIENT=apps/linux/tests/ibus_forget_client.py python3 apps/linux/tests/ibus_harness.py
HARNESS_CAPS=panel 换成不声明 preedit 能力的客户端（Sublime / XIM 那类）：拼音由面板兜底画，
提示仍要走辅助行、但不能带拼音前缀（否则候选窗上两行拼音）。
逐键同样必须 process_key_event_async + 主循环空转（教训见 ibus_client.py 头注）。
"""
import os
import sys

import gi

gi.require_version("IBus", "1.0")
from gi.repository import IBus, GLib

events = []
SHIFT = 1 << 0
caps_mode = os.environ.get("HARNESS_CAPS", "gtk")
# (keyval, keycode, state)：修饰键 + 数字按 keyval + 物理键码认，keyval 是 Shift 变出来的符号
keys = (
    [(ord(c), 0, 0) for c in "nihao"]
    + [(ord("!"), 2, SHIFT)]    # Shift+1，evdev 键码（gnome-shell text-input 路径）
    + [(ord("h"), 0, 0)]        # 敲下一键收掉提示
    + [(ord("@"), 11, SHIFT)]   # Shift+2，X 键码（GTK 直连路径）
)

bus = IBus.Bus()
if not bus.is_connected():
    print("连不上 ibus-daemon")
    sys.exit(1)
assert "qingjian" in [e.get_name() for e in bus.list_engines()], "组件没注册"

ic = bus.create_input_context("qingjian-forget-harness")
ic.connect("update-preedit-text",
           lambda ic, t, c, v: events.append(("preedit", t.get_text(), v)))
ic.connect("update-auxiliary-text",
           lambda ic, t, v: events.append(("aux", t.get_text(), v)))
ic.connect("commit-text", lambda ic, t: events.append(("commit", t.get_text())))

caps = (IBus.Capabilite.FOCUS | IBus.Capabilite.AUXILIARY_TEXT |
        IBus.Capabilite.LOOKUP_TABLE | IBus.Capabilite.PROPERTY)
if caps_mode == "gtk":
    caps |= IBus.Capabilite.PREEDIT_TEXT
ic.set_capabilities(caps)
assert bus.set_global_engine("qingjian")
ic.focus_in()

state = {"i": 0}
loop = GLib.MainLoop()


def step():
    i = state["i"]
    if i >= len(keys):
        loop.quit()
        return False
    state["i"] = i + 1
    keyval, keycode, modifiers = keys[i]
    ic.process_key_event_async(keyval, keycode, modifiers, 10000, None,
                               lambda *a: None)
    return True


GLib.timeout_add(300, step)
GLib.timeout_add_seconds(10, loop.quit)
loop.run()

for e in events:
    print(e)
preedit = [e for e in events if e[0] == "preedit" and e[2]]
aux = [e for e in events if e[0] == "aux" and e[2]]
commit = [e for e in events if e[0] == "commit"]

ok = True
if commit:
    print("删候选不该上屏任何东西：", commit)
    ok = False
if caps_mode == "panel":
    # 不认 preedit 的客户端：daemon 把 preedit 转给面板画，客户端这边收不到
    if preedit:
        print("该由面板兜底画拼音，客户端不该收到 preedit：", preedit[-1:])
        ok = False
elif not preedit or "ni'hao" not in preedit[-1][1]:
    print("删候选后 preedit 应仍是纯拼音：", preedit[-1:] or "无")
    ok = False
notices = [e[1] for e in aux if "「" in e[1] and "」" in e[1]]
if len(notices) < 2:
    print("两种键码惯例各应出一次删候选提示：", aux or "无")
    ok = False
else:
    for notice in notices[:2]:
        if caps_mode == "panel":
            if notice.startswith("ni'hao"):
                print("面板已兜底画拼音，辅助行不该再带一遍：", notice)
                ok = False
        elif not notice.startswith("ni'hao"):
            print("提示应并排在拼音右侧：", notice)
            ok = False
print("能力模式:", caps_mode, " preedit:", preedit[-1][1] if preedit else "无",
      " 提示:", notices[:2], " 上屏:", commit)
sys.exit(0 if ok else 2)
