#!/usr/bin/env python3
"""模拟 ibus 客户端：建 InputContext、声明能力、异步逐键打字，收全 UI 信号。

跑在 ibus_harness.py 起好的测试 daemon 上（IBUS_ADDRESS 指过去）。
HARNESS_CAPS=gtk（缺省）声明 PREEDIT_TEXT，模拟 GTK：preedit 内联、辅助行带拼音；
HARNESS_CAPS=panel 不声明 PREEDIT_TEXT，模拟 Sublime / XIM：daemon 把 preedit 转给面板画，
此时引擎不该再把拼音发一遍辅助行（候选窗会出现两行一样的拼音）；
HARNESS_CAPS=flip 组句中途换两次能力（模拟换客户端；Wayland 下 daemon 会吞掉一部分
FocusOut），引擎每次都要立刻补一帧，辅助行按新客户端的口径来。
退出码 0 = 该模式下该有的信号都逐键到位、不该有的没出现。
逐键必须走 process_key_event_async + 主循环空转：同步调用加 sleep 会把
GDBus 的信号分发饿死（那不是引擎丢信号，是客户端不消费）。
"""
import os
import sys

import gi

gi.require_version("IBus", "1.0")
from gi.repository import IBus, GLib

events = []
# 期望的 LookupTable 方向：0 横排 / 1 竖排 / 2 跟随系统
want_orientation = int(os.environ.get("WANT_ORIENTATION", "1"))
# gtk = 自己画内联 preedit；panel = 不认 preedit，由面板兜底画；flip = 组句中来回换
caps_mode = os.environ.get("HARNESS_CAPS", "gtk")
# 逐键动作。flip 模式在组句中插两次换能力（模拟换客户端）：换能力独占一个 tick，
# 不与按键同 tick——daemon 侧能力立刻生效、引擎晚一拍才收到 SetCapabilities，
# 同 tick 发键会让两帧的顺序取决于投递时序，断言就没法写死。
actions = [("key", c) for c in "nihao "]
if caps_mode == "flip":
    actions = ([("key", "n"), ("key", "i"), ("flip", "panel"),
                ("key", "h"), ("key", "a"), ("flip", "gtk"),
                ("key", "o"), ("key", " ")])

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

BASE_CAPS = (IBus.Capabilite.FOCUS | IBus.Capabilite.AUXILIARY_TEXT |
             IBus.Capabilite.LOOKUP_TABLE | IBus.Capabilite.PROPERTY)


def set_caps(mode, mark=True):
    caps = BASE_CAPS
    if mode == "gtk":
        caps |= IBus.Capabilite.PREEDIT_TEXT
    ic.set_capabilities(caps)
    if mark:
        # 标记在调用返回后立刻记：引擎补发的那帧要等主循环空转才到，顺序稳
        events.append(("flip", mode))


set_caps("gtk" if caps_mode in ("gtk", "flip") else "panel", mark=False)
assert bus.set_global_engine("qingjian")
ic.focus_in()

state = {"i": 0}
loop = GLib.MainLoop()


def step():
    i = state["i"]
    if i >= len(actions):
        loop.quit()
        return False
    state["i"] = i + 1
    kind, payload = actions[i]
    if kind == "flip":
        set_caps(payload)
        return True
    ic.process_key_event_async(ord(payload), 0, 0, 10000, None, lambda *a: None)
    # 按键标记记在异步调用之后：这一键引发的 UI 信号都要等主循环空转才到
    events.append(("key", payload))
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
print(f"能力模式: {caps_mode}  可见 preedit: {len(preedit)}  可见候选: {len(lookup)}  "
      f"辅助行: {len(aux)}  上屏: {commit}  方向(期望 {want_orientation}): "
      f"{'对' if orient_ok else '错'}")
if caps_mode == "panel":
    # 拼音由面板兜底画（preedit 信号不会到客户端），辅助行必须空着，否则面板上两行拼音
    ok = lookup and commit and orient_ok and not aux and not preedit
elif caps_mode == "flip":
    ok = bool(lookup) and bool(commit) and orient_ok
    flips = [i for i, e in enumerate(events) if e[0] == "flip"]
    if len(flips) < 2:
        print("flip 模式应换两次能力：", flips)
        ok = False
    for idx in flips:
        mode = events[idx][1]
        # 到下一次按键为止，这段里只可能有引擎因能力变化补发的那一帧
        nxt_key = next((j for j in range(idx + 1, len(events))
                        if events[j][0] == "key"), len(events))
        resent = [e for e in events[idx + 1:nxt_key] if e[0] == "aux"]
        if not resent:
            print(f"换成 {mode} 后没补发帧（要等下一键才纠正）：", events[idx + 1:nxt_key])
            ok = False
            continue
        if mode == "panel" and (resent[0][1] or resent[0][2]):
            print("面板兜底画拼音，补发的辅助行不该带拼音：", resent[0])
            ok = False
        elif mode == "gtk" and not (resent[0][1] and resent[0][2]):
            print("内联 preedit 客户端，补发的辅助行应带拼音：", resent[0])
            ok = False
else:
    ok = preedit and lookup and aux and commit and orient_ok
sys.exit(0 if ok else 2)
