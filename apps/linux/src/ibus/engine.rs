//! ibus 引擎对象：daemon 逐键调这里，把按键翻译成 Core 的输入、把结果发回 ibus。
//!
//! 每个方法都是「锁内同步算，锁外发信号」：Core 的按键处理在毫秒级，但 D-Bus 信号的
//! await 不该握着锁。panic 边界包住分流逻辑：崩了恢复成清空状态、放行当键，进程不死。

use std::panic::AssertUnwindSafe;
use std::sync::{Arc, Mutex, MutexGuard};

use librush::ibus::{IBusEngine, IBusModifierState};
use qingjian_platform::{Modifiers, SwitchKey};
use xkeysym::{KeyCode, Keysym};
use zbus::fdo;
use zbus::object_server::{ObjectServer, SignalEmitter};

use super::keymap::{self, Key};
use super::present;
use crate::host::Host;
use crate::host::keys::{self, CommandKey, Outcome};

/// ibus 引擎实现：全部状态在共享的 Host 里，自己只留一个句柄。
pub struct QingjianEngine {
    /// 进程级状态。
    host: Arc<Mutex<Host>>,
}

impl QingjianEngine {
    /// 拿着共享 Host 建引擎。
    pub fn new(host: Arc<Mutex<Host>>) -> Self {
        Self { host }
    }
}

/// 拿锁；毒化（有回调 panic 过）时恢复数据继续，输入法不能死。
fn lock(host: &Arc<Mutex<Host>>) -> MutexGuard<'_, Host> {
    host.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// 一枚键的完整处理：分流 + 依结果构帧，两步都在 panic 边界内
/// （构帧要跑 `query()`，是按键路径上最重的 Core 调用，不能漏在边界外）。
/// 崩了恢复成清空状态、放行当键、发收窗帧，进程不死。
fn dispatch(host: &mut Host, key: PressedKey) -> (Outcome, Option<present::Frame>) {
    let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
        let outcome = dispatch_inner(host, key);
        // 改了缓冲区才重查；只动高亮 / 页位的键按当前会话重画（重查会把它们归零）
        let frame = if outcome.refresh {
            Some(present::frame_after_change(host))
        } else if outcome.redraw {
            Some(present::frame_of(host))
        } else {
            None
        };
        (outcome, frame)
    }));
    match result {
        Ok(pair) => pair,
        Err(panic) => {
            tracing::error!(panic = ?panic, "按键处理 panic，清空组句放行");
            host.engine.clear();
            host.session.clear();
            let frame = present::frame_after_change(host);
            (Outcome::passed(), Some(frame))
        }
    }
}

/// 进入分流前翻译好的按键。
struct PressedKey {
    /// 翻译结果。
    key: Option<Key>,

    /// 物理键码（X 键码 = evdev + 8）：「修饰键 + 数字」按它认，keyval 会被修饰键变掉。
    keycode: KeyCode,

    /// 修饰状态。
    state: IBusModifierState,
}

/// 从修饰位取配置里的组合键（Alt→option、Super→command）；带着配置表达不了的修饰
/// （Meta / Hyper / mod5 即 AltGr）按不配处理，走各自的原有分支。
fn chord_of(state: IBusModifierState) -> Option<Modifiers> {
    (!state.meta() && !state.hyper() && !state.mod5()).then_some(Modifiers {
        shift: state.shift(),
        control: state.control(),
        option: state.mod1(),
        command: state.mod4() || state.super_(),
    })
}

fn dispatch_inner(host: &mut Host, pressed: PressedKey) -> Outcome {
    let PressedKey {
        key,
        keycode,
        state,
    } = pressed;
    // 删候选的提示只活到下一次按键；修饰键自己的按下 / 抬起不算「敲键」（对齐 mac 只在 KeyDown 收）。
    // 记下这次是否真清掉了：吞键这类不发帧的分支要用它补一帧，不然提示在面板上多留一拍
    let notice_cleared = state.is_keydown()
        && !matches!(
            key,
            Some(Key::Shift) | Some(Key::Control) | Some(Key::Modifier)
        )
        && host.notice.take().is_some();
    // 配置的切换键（缺省 Shift；`[shortcut] switch_mode`）从按下到抬起之间没插别的键 = 单击切中英
    let is_switch_key = |key: Option<Key>, switch: SwitchKey| {
        matches!(
            (switch, key),
            (SwitchKey::Shift, Some(Key::Shift)) | (SwitchKey::Control, Some(Key::Control))
        )
    };
    // 同一时刻还按着别的显著修饰键（Shift / Ctrl / Alt / Super / Meta / Hyper）= 在组和弦，不是单击；
    // 与 librush `has_special_modifiers` 的判定面保持一致
    let chorded = |state: IBusModifierState, switch: SwitchKey| match switch {
        SwitchKey::Shift => {
            state.control()
                || state.mod1()
                || state.mod4()
                || state.super_()
                || state.meta()
                || state.hyper()
        }
        SwitchKey::Control => {
            state.shift()
                || state.mod1()
                || state.mod4()
                || state.super_()
                || state.meta()
                || state.hyper()
        }
        _ => false,
    };
    if state.is_keyup() {
        // 切模式时把没敲完的组句先原样上屏
        if is_switch_key(key, host.switch_key)
            && !chorded(state, host.switch_key)
            && host.switch_released().is_some()
        {
            return keys::commit_raw(&mut host.engine);
        }
        return Outcome::passed();
    }
    match key {
        Some(Key::Shift) | Some(Key::Control) => {
            if is_switch_key(key, host.switch_key) && !chorded(state, host.switch_key) {
                host.switch_pressed();
            } else {
                // 非切换修饰键按下，或按下时已组和弦：正在等的单击不算了
                host.break_switch_tap();
            }
            Outcome::passed()
        }
        Some(Key::Modifier) => {
            host.break_switch_tap();
            Outcome::passed()
        }
        Some(Key::Char(c)) if state.lock() => {
            // Caps Lock 亮着 = 纯直通：组句中的字母先原样上屏，字符交给应用
            host.break_switch_tap();
            let mut outcome = keys::commit_raw(&mut host.engine);
            host.engine.note_passthrough(c);
            outcome.handled = false;
            outcome
        }
        // 修饰键 + 数字（缺省 ⇧，配置 `[shortcut] delete_candidate`）：删掉当前页第 N 个候选，
        // 只在组句中认。数字按物理键码认——Shift 会把数字行的 keyval 变成 `!@#$…`；
        // 表达式模式里 ⇧+数字是运算符（`v2^3`），不截。不在组句、没配到的组合照旧走各自分支：
        // 组句外的 ⇧4 还是 `$` 走标点转换出 ￥，Ctrl / Alt / Super 组合按应用快捷键放行
        Some(Key::Char(_))
            if !host.engine.composition().is_empty()
                && !host.engine.expression_mode()
                && let Some(digit) = keymap::digit_key(keycode)
                && chord_of(state) == Some(host.delete_keys) =>
        {
            host.break_switch_tap();
            if let Some(message) =
                keys::forget_on_page(&mut host.engine, &host.session, host.page_size, digit)
            {
                tracing::info!(%message, "删候选");
                host.notice = Some(message);
                return Outcome::consumed();
            }
            // 那一格没有候选：吞掉按键；刚清掉的删候选提示补一帧收掉（只重画、不重查）
            if notice_cleared {
                Outcome::redrawn()
            } else {
                Outcome {
                    handled: true,
                    ..Outcome::default()
                }
            }
        }
        Some(Key::Char(_)) | Some(Key::Command(_)) if state.has_special_modifiers() => {
            // Ctrl / Alt / Super 组合是应用快捷键：先把组句收掉，再把键放行给应用，缓冲不丢
            host.break_switch_tap();
            let mut outcome = keys::commit_raw(&mut host.engine);
            outcome.handled = false;
            outcome.refresh = true;
            outcome
        }
        Some(Key::Char(c)) => {
            host.break_switch_tap();
            keys::handle_char(
                &mut host.engine,
                &mut host.session,
                host.page_size,
                host.page_keys,
                host.english_candidates,
                c,
            )
        }
        Some(Key::Command(command)) => {
            host.break_switch_tap();
            // 组句中的 Shift+Tab 是上一页（对齐 mac 的 Backtab）；组句外是应用的反向 Tab，照常放行。
            // 翻完直接出「只重画」的结果，不再进 handle_command（那边按未知命令处理会触发重查、白翻）
            if command == CommandKey::Tab && state.shift() && !host.engine.composition().is_empty()
            {
                if host.session.turn_page(-1, host.page_size) {
                    host.engine.note_page_turn();
                }
                return Outcome::redrawn();
            }
            keys::handle_command(&mut host.engine, &mut host.session, host.page_size, command)
        }
        None => {
            // 组句中不认识的键吞掉（防止应用动状态丢 preedit），非组句放行
            host.break_switch_tap();
            if host.engine.composition().is_empty() {
                Outcome::passed()
            } else {
                Outcome::consumed()
            }
        }
    }
}

impl IBusEngine for QingjianEngine {
    async fn process_key_event(
        &mut self,
        se: SignalEmitter<'_>,
        _server: &ObjectServer,
        keyval: Keysym,
        keycode: KeyCode,
        state: IBusModifierState,
    ) -> fdo::Result<bool> {
        let pressed = PressedKey {
            key: keymap::translate(keyval),
            keycode,
            state,
        };
        let (outcome, frame) = {
            let mut host = lock(&self.host);
            dispatch(&mut host, pressed)
        };
        if !outcome.commits.is_empty() {
            present::commit_texts(&se, &outcome.commits).await;
        }
        if let Some(frame) = frame {
            present::send_frame(&se, &frame).await;
        }
        Ok(outcome.handled)
    }

    async fn focus_out(
        &mut self,
        se: SignalEmitter<'_>,
        _server: &ObjectServer,
    ) -> fdo::Result<()> {
        // 失焦上屏：拼音原样交出，链断开，收窗
        let raw = {
            let mut host = lock(&self.host);
            host.break_switch_tap();
            host.notice = None;
            host.engine.break_chain();
            present::take_raw(&mut host)
        };
        if let Some(raw) = raw {
            present::commit_texts(&se, &[raw]).await;
        }
        let frame = {
            let mut host = lock(&self.host);
            host.session.clear();
            present::frame_after_change(&mut host)
        };
        present::send_frame(&se, &frame).await;
        Ok(())
    }

    async fn reset(&mut self, se: SignalEmitter<'_>, _server: &ObjectServer) -> fdo::Result<()> {
        let frame = {
            let mut host = lock(&self.host);
            host.break_switch_tap();
            host.notice = None;
            host.engine.clear();
            present::frame_after_change(&mut host)
        };
        present::send_frame(&se, &frame).await;
        Ok(())
    }

    async fn disable(&mut self, se: SignalEmitter<'_>, server: &ObjectServer) -> fdo::Result<()> {
        // 被切走（对齐 mac 的 deactivate）：先把学习数据落盘，别等 60 秒的定时；其余同失焦
        lock(&self.host).engine.flush_learning();
        self.focus_out(se, server).await
    }

    async fn candidate_clicked(
        &mut self,
        se: SignalEmitter<'_>,
        _server: &ObjectServer,
        index: u32,
        _button: u32,
        _state: u32,
    ) -> fdo::Result<()> {
        // index 是当前页内的下标，换算成整个候选表的下标；换算不出（面板页态滞后于最新一帧）
        // 就当没点——不上屏任何东西，只按当前状态重发一帧
        let (commits, frame) = {
            let mut guard = lock(&self.host);
            let host: &mut Host = &mut guard;
            let commits = host
                .session
                .index_on_page(index as usize, host.page_size)
                .map(|index| keys::commit_index(&mut host.engine, index, &host.session))
                .unwrap_or_default();
            let frame = present::frame_after_change(host);
            (commits, frame)
        };
        present::commit_texts(&se, &commits.commits).await;
        present::send_frame(&se, &frame).await;
        Ok(())
    }

    async fn page_up(&mut self, se: SignalEmitter<'_>, _server: &ObjectServer) -> fdo::Result<()> {
        turn_page(self, se, -1).await;
        Ok(())
    }

    async fn page_down(
        &mut self,
        se: SignalEmitter<'_>,
        _server: &ObjectServer,
    ) -> fdo::Result<()> {
        turn_page(self, se, 1).await;
        Ok(())
    }

    async fn cursor_up(
        &mut self,
        se: SignalEmitter<'_>,
        _server: &ObjectServer,
    ) -> fdo::Result<()> {
        move_highlight(self, se, -1).await;
        Ok(())
    }

    async fn cursor_down(
        &mut self,
        se: SignalEmitter<'_>,
        _server: &ObjectServer,
    ) -> fdo::Result<()> {
        move_highlight(self, se, 1).await;
        Ok(())
    }
}

/// 翻页（候选窗上的鼠标交互）后重发一帧。
async fn turn_page(engine: &QingjianEngine, se: SignalEmitter<'_>, delta: isize) {
    let frame = {
        let mut host = lock(&engine.host);
        let page_size = host.page_size;
        if host.session.turn_page(delta, page_size) {
            host.engine.note_page_turn();
        }
        present::frame_of(&host)
    };
    present::send_frame(&se, &frame).await;
}

/// 高亮移动（滚轮）后重发一帧。
async fn move_highlight(engine: &QingjianEngine, se: SignalEmitter<'_>, delta: isize) {
    let frame = {
        let mut host = lock(&engine.host);
        host.session.move_highlight(delta);
        present::frame_of(&host)
    };
    present::send_frame(&se, &frame).await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use qingjian_core::Engine;
    use qingjian_dictionary::Dictionary;

    fn test_host() -> Host {
        let dict = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/sample/dict.tsv");
        Host::with_engine(Engine::new(
            Dictionary::from_path(dict).expect("样例词库读不了"),
        ))
    }

    /// 按下（非抬起）一枚键；键码缺省 0（数字行键码才参与分流）。
    fn press(key: Key) -> PressedKey {
        PressedKey {
            key: Some(key),
            keycode: KeyCode::new(0),
            state: IBusModifierState::new_with_raw_value(0),
        }
    }

    /// Shift 按着按下数字行第 `digit` 个键：keyval 是 Shift 变出来的符号（US 布局 `!@#$%^&*(`）。
    fn shift_digit(digit: usize) -> PressedKey {
        const SHIFTED: [char; 9] = ['!', '@', '#', '$', '%', '^', '&', '*', '('];
        PressedKey {
            key: Some(Key::Char(SHIFTED[digit - 1])),
            keycode: KeyCode::new(9 + digit as u32),
            state: IBusModifierState::new_with_raw_value(1),
        }
    }

    /// Ctrl 按着按下数字行第 `digit` 个键（keyval 仍是数字本身）。
    fn ctrl_digit(digit: usize) -> PressedKey {
        PressedKey {
            key: Some(Key::Char(char::from(b'0' + digit as u8))),
            keycode: KeyCode::new(9 + digit as u32),
            state: IBusModifierState::new_with_raw_value(1 << 2),
        }
    }

    fn type_pinyin(host: &mut Host, letters: &str) {
        for c in letters.chars() {
            let (outcome, _) = dispatch(host, press(Key::Char(c)));
            assert!(outcome.handled, "拼音字母 {c} 应该被吃掉");
        }
    }

    #[test]
    fn switch_key_defaults_to_a_shift_tap() {
        let mut host = test_host();
        let outcome = dispatch_inner(
            &mut host,
            PressedKey {
                key: Some(Key::Shift),
                keycode: KeyCode::new(0),
                state: IBusModifierState::new_with_raw_value(0),
            },
        );
        assert!(!outcome.handled);
        // 中间没插别的键：抬起 = 单击，切进英文模式
        let outcome = dispatch_inner(
            &mut host,
            PressedKey {
                key: Some(Key::Shift),
                keycode: KeyCode::new(0),
                state: IBusModifierState::new_with_raw_value(1 << 30),
            },
        );
        assert!(!outcome.handled);
        assert!(host.engine.english_mode());
    }

    #[test]
    fn intervening_key_breaks_the_tap() {
        let mut host = test_host();
        dispatch_inner(
            &mut host,
            PressedKey {
                key: Some(Key::Shift),
                keycode: KeyCode::new(0),
                state: IBusModifierState::new_with_raw_value(0),
            },
        );
        // Shift 按住时敲了别的键（大写字母走临时英文）
        dispatch_inner(
            &mut host,
            PressedKey {
                key: Some(Key::Char('A')),
                keycode: KeyCode::new(0),
                state: IBusModifierState::new_with_raw_value(1),
            },
        );
        dispatch_inner(
            &mut host,
            PressedKey {
                key: Some(Key::Shift),
                keycode: KeyCode::new(0),
                state: IBusModifierState::new_with_raw_value(1 << 30),
            },
        );
        assert!(!host.engine.english_mode(), "Shift+字母 不是单击");
    }

    #[test]
    fn switch_mode_none_never_toggles() {
        let mut host = test_host();
        host.switch_key = qingjian_platform::SwitchKey::None;
        dispatch_inner(
            &mut host,
            PressedKey {
                key: Some(Key::Shift),
                keycode: KeyCode::new(0),
                state: IBusModifierState::new_with_raw_value(0),
            },
        );
        dispatch_inner(
            &mut host,
            PressedKey {
                key: Some(Key::Shift),
                keycode: KeyCode::new(0),
                state: IBusModifierState::new_with_raw_value(1 << 30),
            },
        );
        assert!(!host.engine.english_mode());
    }

    #[test]
    fn special_modifier_combo_commits_raw_then_passes_through() {
        let mut host = test_host();
        for c in "kaifa".chars() {
            keys::handle_char(
                &mut host.engine,
                &mut host.session,
                host.page_size,
                host.page_keys,
                host.english_candidates,
                c,
            );
        }
        // 组句中的 Ctrl+Enter（发送消息）：拼音先原样上屏，键要放行给应用
        let outcome = dispatch_inner(
            &mut host,
            PressedKey {
                key: Some(Key::Command(CommandKey::Enter)),
                keycode: KeyCode::new(0),
                state: IBusModifierState::new_with_raw_value(1 << 2),
            },
        );
        assert!(!outcome.handled, "应用快捷键组合必须放行");
        assert!(outcome.refresh);
        assert_eq!(outcome.commits, vec!["kaifa".to_owned()]);
    }

    #[test]
    fn modifier_chords_are_not_taps_in_either_order() {
        // 顺序一：Shift↓ Ctrl↓ Ctrl↑ Shift↑（Ctrl 插进来断了单击）
        let mut host = test_host();
        for (key, state) in [
            (Some(Key::Shift), 0),
            (Some(Key::Control), 1 << 2),
            (Some(Key::Control), (1 << 2) | (1 << 30)),
            (Some(Key::Shift), 1 << 30),
        ] {
            dispatch_inner(
                &mut host,
                PressedKey {
                    key,
                    keycode: KeyCode::new(0),
                    state: IBusModifierState::new_with_raw_value(state),
                },
            );
        }
        assert!(!host.engine.english_mode(), "Shift+Ctrl 和弦不该切模式");
        // 顺序二：Ctrl↓ Shift↓ Shift↑ Ctrl↑（Shift 一起按着时 Ctrl 抬起）
        let mut host = Host::with_engine(test_host().engine);
        host.switch_key = qingjian_platform::SwitchKey::Control;
        for (key, state) in [
            (Some(Key::Control), 1 << 2),
            (Some(Key::Shift), (1 << 2) | 1),
            (Some(Key::Shift), (1 << 2) | (1 << 30)),
            (Some(Key::Control), (1 << 2) | (1 << 30)),
        ] {
            dispatch_inner(
                &mut host,
                PressedKey {
                    key,
                    keycode: KeyCode::new(0),
                    state: IBusModifierState::new_with_raw_value(state),
                },
            );
        }
        assert!(!host.engine.english_mode(), "按着 Shift 时抬 Ctrl 不算单击");
    }

    #[test]
    fn shifted_tab_outside_composition_passes_through() {
        let host = test_host();
        let outcome = dispatch_inner(
            &mut { host },
            PressedKey {
                key: Some(Key::Command(CommandKey::Tab)),
                keycode: KeyCode::new(0),
                state: IBusModifierState::new_with_raw_value(1),
            },
        );
        assert!(!outcome.handled, "组句外的 Shift+Tab 是应用的反向 Tab");
    }

    #[test]
    fn page_keys_redraw_without_requerying_so_the_page_survives() {
        // 真词库下 nihao 的候选远超一页；翻页后高亮必须落到新页第一格，
        // 而不是被「重查 + session 归零」打回第一页（2026-09-19 真机翻页失效的根因）
        let dict = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/lexicon/dict.tsv");
        let mut host = Host::with_engine(Engine::new(
            Dictionary::from_path(dict).expect("词库读不了"),
        ));
        for c in "nihao".chars() {
            let (outcome, _) = dispatch(&mut host, press(Key::Char(c)));
            assert!(outcome.handled);
        }
        assert!(
            host.session.candidates().len() > host.page_size,
            "样例要能翻页：候选得多于一页"
        );

        let (outcome, frame) = dispatch(&mut host, press(Key::Char(']')));
        assert!(
            outcome.handled && !outcome.refresh && outcome.redraw,
            "翻页只重画不重查"
        );
        let frame = frame.expect("翻页要重画一帧");
        assert!(frame.table_visible);
        assert_eq!(
            frame.table.cursor_pos(),
            host.page_size as u32,
            "翻页后高亮落到第二页第一格"
        );

        // 组句中的 Shift+Tab 是上一页：应回到第一页第一格
        let (outcome, frame) = dispatch(
            &mut host,
            PressedKey {
                key: Some(Key::Command(CommandKey::Tab)),
                keycode: KeyCode::new(0),
                state: IBusModifierState::new_with_raw_value(1),
            },
        );
        assert!(outcome.handled && outcome.redraw && !outcome.refresh);
        assert_eq!(frame.expect("上一页也要重画").table.cursor_pos(), 0);
    }

    #[test]
    fn control_tap_toggles_when_configured() {
        let mut host = test_host();
        host.switch_key = qingjian_platform::SwitchKey::Control;
        dispatch_inner(
            &mut host,
            PressedKey {
                key: Some(Key::Control),
                keycode: KeyCode::new(0),
                state: IBusModifierState::new_with_raw_value(1 << 2),
            },
        );
        dispatch_inner(
            &mut host,
            PressedKey {
                key: Some(Key::Control),
                keycode: KeyCode::new(0),
                state: IBusModifierState::new_with_raw_value((1 << 2) | (1 << 30)),
            },
        );
        assert!(host.engine.english_mode());
    }

    #[test]
    fn shift_digit_forgets_candidate_requeries_and_shows_notice() {
        let mut host = test_host();
        type_pinyin(&mut host, "kaifa");
        let slot = host
            .session
            .candidates()
            .iter()
            .position(|candidate| candidate.text == "开发")
            .expect("kaifa 的候选里应该有 开发");
        // 缺省 Shift + 数字（按物理键码认，keyval 已被 Shift 变成符号）：删候选、重查、不上屏
        let (outcome, frame) = dispatch(&mut host, shift_digit(slot + 1));
        assert!(outcome.handled && outcome.refresh && outcome.commits.is_empty());
        assert!(!host.engine.composition().is_empty(), "删候选不动组句");
        let notice = host.notice.as_deref().expect("删候选后应有提示");
        assert!(notice.contains("开发"), "提示要提到候选词：{notice}");
        let frame = frame.expect("重查后要发一帧");
        assert!(
            frame.aux.contains(notice),
            "提示随辅助行下发：{}",
            frame.aux
        );
        assert_eq!(frame.preedit, "kai'fa", "内联 preedit 不掺提示");
        // 敲下一键：提示收掉，拼音照常进缓冲
        let (outcome, frame) = dispatch(&mut host, press(Key::Char('n')));
        assert!(outcome.handled);
        assert!(host.notice.is_none(), "提示只活到下一次按键");
        assert_eq!(
            frame.expect("改缓冲要重查发帧").aux,
            "kai'fan",
            "下一帧辅助行回到纯拼音"
        );
    }

    #[test]
    fn shift_digit_without_a_candidate_in_that_slot_is_swallowed() {
        let mut host = test_host();
        type_pinyin(&mut host, "kaifa");
        let count = host.session.candidates().len();
        let digit = count + 1;
        assert!(digit <= 9, "样例词库 kaifa 的候选应不足一页：{count}");
        let (outcome, frame) = dispatch(&mut host, shift_digit(digit));
        assert!(outcome.handled, "那格没有候选也要吞掉按键");
        assert!(!outcome.refresh && !outcome.redraw && outcome.commits.is_empty());
        assert!(frame.is_none(), "什么都不动就不发帧");
        assert!(host.notice.is_none());
    }

    #[test]
    fn swallowing_after_a_notice_retires_it_with_a_frame() {
        // 提示还挂在面板上时吞了一键：要补一帧把提示收掉，不能多留一拍
        let mut host = test_host();
        type_pinyin(&mut host, "kaifa");
        let slot = host
            .session
            .candidates()
            .iter()
            .position(|candidate| candidate.text == "开发")
            .expect("kaifa 的候选里应该有 开发");
        dispatch(&mut host, shift_digit(slot + 1));
        assert!(host.notice.is_some());
        let count = host.session.candidates().len();
        let digit = count + 1;
        assert!(digit <= 9, "样例词库 kaifa 的候选应不足一页：{count}");
        let (outcome, frame) = dispatch(&mut host, shift_digit(digit));
        assert!(outcome.handled && outcome.redraw && !outcome.refresh);
        let frame = frame.expect("刚清掉提示要补一帧");
        assert_eq!(frame.aux, "kai'fa", "提示随这一帧从辅助行消失");
        assert!(host.notice.is_none());
    }

    #[test]
    fn altgr_shift_digit_is_not_a_delete_chord() {
        // mod5（AltGr / ISO_Level3）表达不进配置的 Modifiers，与 Meta / Hyper 一样按不配处理：
        // Shift+AltGr+数字落回普通字符分支（组句中进直输段），不当删候选
        let mut host = test_host();
        type_pinyin(&mut host, "kaifa");
        let (outcome, _) = dispatch(
            &mut host,
            PressedKey {
                key: Some(Key::Char('!')),
                keycode: KeyCode::new(10),
                state: IBusModifierState::new_with_raw_value((1 << 0) | (1 << 7)),
            },
        );
        assert!(outcome.handled && outcome.refresh);
        assert!(host.engine.composition().text().contains('!'));
        assert!(host.notice.is_none());
    }

    #[test]
    fn shift_digit_outside_composition_is_full_width_punctuation() {
        let mut host = test_host();
        // 不在组句中：Shift+1 就是 `!`，走中文标点规则出全角 ！（不能被快捷键截走）
        let (outcome, _) = dispatch(&mut host, shift_digit(1));
        assert!(outcome.handled);
        assert_eq!(outcome.commits, vec!["！".to_owned()]);
        assert!(host.notice.is_none());
    }

    #[test]
    fn shift_digit_in_expression_mode_is_an_operator_not_a_shortcut() {
        let mut host = test_host();
        type_pinyin(&mut host, "v2");
        assert!(host.engine.expression_mode());
        // 表达式模式里 ⇧+数字打的是运算符（`v2^3` 的 ^、`v1*8` 的 *）：不当删候选快捷键
        let (outcome, _) = dispatch(&mut host, shift_digit(8));
        assert!(outcome.handled, "* 是表达式字符，照常进缓冲");
        assert!(host.notice.is_none(), "表达式模式不截修饰键 + 数字");
        assert!(host.engine.expression_mode(), "表达式模式不被打断");
        assert_eq!(host.engine.composition().text(), "v2*");
    }

    #[test]
    fn configured_chord_intercepts_and_unmatched_shift_falls_through() {
        // 删候选改成 Ctrl+数字：Shift+2 回到原行为（`@` 进直输段），Ctrl+2 才删
        let mut host = test_host();
        host.delete_keys = Modifiers::CONTROL;
        type_pinyin(&mut host, "kaifa");
        let (outcome, _) = dispatch(&mut host, shift_digit(2));
        assert!(outcome.handled && outcome.refresh, "Shift+2 走普通字符分支");
        assert!(
            host.engine.composition().text().contains('@'),
            "组句中 Shift+2 是直输段 @"
        );
        assert!(host.notice.is_none());
        let (outcome, _) = dispatch(&mut host, ctrl_digit(1));
        assert!(outcome.handled && outcome.refresh && outcome.commits.is_empty());
        assert!(host.notice.is_some(), "配到的 Ctrl+数字删候选");
    }

    #[test]
    fn shift_digit_does_not_toggle_english_mode() {
        let mut host = test_host();
        type_pinyin(&mut host, "kaifa");
        // Shift↓ 数字↓ 数字↑ Shift↑ 整套下来：既删了候选，也不算 Shift 单击切中英
        dispatch_inner(&mut host, press(Key::Shift));
        let (outcome, _) = dispatch(&mut host, shift_digit(1));
        assert!(outcome.handled);
        dispatch_inner(
            &mut host,
            PressedKey {
                key: Some(Key::Char('!')),
                keycode: KeyCode::new(10),
                state: IBusModifierState::new_with_raw_value(1 | (1 << 30)),
            },
        );
        dispatch_inner(
            &mut host,
            PressedKey {
                key: Some(Key::Shift),
                keycode: KeyCode::new(0),
                state: IBusModifierState::new_with_raw_value(1 << 30),
            },
        );
        assert!(!host.engine.english_mode(), "Shift+数字不是切换键单击");
    }
}
