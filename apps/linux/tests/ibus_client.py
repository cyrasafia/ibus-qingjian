#!/usr/bin/env python3
"""模拟 GTK 的 ibus 客户端：建 InputContext、声明能力、异步逐键打字，收全 UI 信号。

跑在 ibus_harness.py 起好的测试 daemon 上（IBUS_ADDRESS 指过去）。
退出码 0 = preedit / 辅助行 / 候选 / 上屏全部逐键到位。
逐键必须走 process_key_event_async + 主循环空转：同步调用加 sleep 会把
GDBus 的信号分发饿死（那不是引擎丢信号，是客户端不消费）。
"""
import os
import sys

import gi

gi.require_version("IBus", "1.0")
from gi.repository import IBus, GLib

events = []
keys = "nihao "
# 期望的 LookupTable 方向：0 横排 / 1 竖排 / 2 跟随系统
want_orientation = int(os.environ.get("WANT_ORIENTATION", "1"))

bus = IBus.Bus()
if not bus.is_connected():
    print("连不上 ibus-daemon")
    sys.exit(1)
assert "qingjian" in [e.get_name() for e in bus.list_engines()], "组件没注册"

ic = bus.create_input_context("qingjian-harness")
ic.connect("update-preedit-text",
           lambda ic, t, c, v: events.append(("preedit", t.get_text(), v)))
ic.connect("update-lookup-table",
           lambda ic, t, v: events.append(("lookup", t.get_number_of_candidates(), v,
                                           t.get_orientation())))
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
    ic.process_key_event_async(ord(keys[i]), 0, 0, 10000, None, lambda *a: None)
    return True


GLib.timeout_add(300, step)
GLib.timeout_add_seconds(10, loop.quit)
loop.run()

for e in events:
    print(e)
preedit = [e for e in events if e[0] == "preedit" and e[2]]
lookup = [e for e in events if e[0] == "lookup" and e[2]]
aux = [e for e in events if e[0] == "aux" and e[2]]
commit = [e for e in events if e[0] == "commit"]
orient_ok = all(e[3] == want_orientation for e in lookup)
print(f"可见 preedit: {len(preedit)}  可见候选: {len(lookup)}  辅助行: {len(aux)}  "
      f"上屏: {commit}  方向(期望 {want_orientation}): {'对' if orient_ok else '错'}")
sys.exit(0 if preedit and lookup and aux and commit and orient_ok else 2)
