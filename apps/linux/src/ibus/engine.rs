//! ibus 引擎对象：daemon 逐键调这里，把按键翻译成 Core 的输入、把结果发回 ibus。
//!
//! 每个方法都是「锁内同步算，锁外发信号」：Core 的按键处理在毫秒级，但 D-Bus 信号的
//! await 不该握着锁。panic 边界包住分流逻辑：崩了恢复成清空状态、放行当键，进程不死。

use std::panic::AssertUnwindSafe;
use std::sync::{Arc, Mutex, MutexGuard};

use librush::ibus::{IBusEngine, IBusModifierState};
use qingjian_platform::SwitchKey;
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
        let frame = outcome.refresh.then(|| present::frame_after_change(host));
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

    /// 修饰状态。
    state: IBusModifierState,
}

fn dispatch_inner(host: &mut Host, pressed: PressedKey) -> Outcome {
    let PressedKey { key, state } = pressed;
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
            // 组句中的 Shift+Tab 是上一页（对齐 mac 的 Backtab）；组句外是应用的反向 Tab，照常放行
            let command = if command == CommandKey::Tab
                && state.shift()
                && !host.engine.composition().is_empty()
            {
                host.session.turn_page(-1, host.page_size);
                host.engine.note_page_turn();
                CommandKey::Unknown
            } else {
                command
            };
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
        _keycode: KeyCode,
        state: IBusModifierState,
    ) -> fdo::Result<bool> {
        let pressed = PressedKey {
            key: keymap::translate(keyval),
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
        host.session.turn_page(delta, page_size);
        host.engine.note_page_turn();
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

    #[test]
    fn switch_key_defaults_to_a_shift_tap() {
        let mut host = test_host();
        let outcome = dispatch_inner(
            &mut host,
            PressedKey {
                key: Some(Key::Shift),
                state: IBusModifierState::new_with_raw_value(0),
            },
        );
        assert!(!outcome.handled);
        // 中间没插别的键：抬起 = 单击，切进英文模式
        let outcome = dispatch_inner(
            &mut host,
            PressedKey {
                key: Some(Key::Shift),
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
                state: IBusModifierState::new_with_raw_value(0),
            },
        );
        // Shift 按住时敲了别的键（大写字母走临时英文）
        dispatch_inner(
            &mut host,
            PressedKey {
                key: Some(Key::Char('A')),
                state: IBusModifierState::new_with_raw_value(1),
            },
        );
        dispatch_inner(
            &mut host,
            PressedKey {
                key: Some(Key::Shift),
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
                state: IBusModifierState::new_with_raw_value(0),
            },
        );
        dispatch_inner(
            &mut host,
            PressedKey {
                key: Some(Key::Shift),
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
                state: IBusModifierState::new_with_raw_value(1),
            },
        );
        assert!(!outcome.handled, "组句外的 Shift+Tab 是应用的反向 Tab");
    }

    #[test]
    fn control_tap_toggles_when_configured() {
        let mut host = test_host();
        host.switch_key = qingjian_platform::SwitchKey::Control;
        dispatch_inner(
            &mut host,
            PressedKey {
                key: Some(Key::Control),
                state: IBusModifierState::new_with_raw_value(1 << 2),
            },
        );
        dispatch_inner(
            &mut host,
            PressedKey {
                key: Some(Key::Control),
                state: IBusModifierState::new_with_raw_value((1 << 2) | (1 << 30)),
            },
        );
        assert!(host.engine.english_mode());
    }
}
