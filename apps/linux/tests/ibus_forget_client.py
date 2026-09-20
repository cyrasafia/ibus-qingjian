#!/usr/bin/env python3
"""模拟 GTK 的 ibus 客户端（删候选场景）：打 nihao，Shift+1 删第一格候选，
校验提示真的流过真实 daemon——辅助行拼音右侧多出一句、内联 preedit 不掺、不上屏。

用法：HARNESS_CLIENT=apps/linux/tests/ibus_forget_client.py python3 apps/linux/tests/ibus_harness.py
逐键同样必须 process_key_event_async + 主循环空转（教训见 ibus_client.py 头注）。
"""
import sys

import gi

gi.require_version("IBus", "1.0")
from gi.repository import IBus, GLib

events = []
SHIFT = 1 << 0
# (keyval, keycode, state)：修饰键 + 数字按物理键码认（X 键码 10 是数字行 1），
# keyval 给 Shift 变出来的符号（US 布局）
keys = [(ord(c), 0, 0) for c in "nihao"] + [(ord("!"), 10, SHIFT)]

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

ic.set_capabilities(IBus.Capabilite.PREEDIT_TEXT | IBus.Capabilite.FOCUS |
                    IBus.Capabilite.AUXILIARY_TEXT | IBus.Capabilite.LOOKUP_TABLE |
                    IBus.Capabilite.PROPERTY)
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
if not preedit or preedit[-1][1] != "ni'hao":
    print("删候选后 preedit 应仍是纯拼音 ni'hao：", preedit[-1:] or "无")
    ok = False
notices = [e[1] for e in aux if "「" in e[1] and "」" in e[1]]
if not notices:
    print("辅助行应出现删候选提示（「…」）：", aux[-3:] or "无")
    ok = False
elif not notices[-1].startswith("ni'hao  "):
    print("提示应并排在拼音右侧：", notices[-1])
    ok = False
print("preedit:", preedit[-1][1] if preedit else "无", " 辅助行:",
      notices[-1] if notices else "无", " 上屏:", commit)
sys.exit(0 if ok else 2)
