//! keysym 到壳内按键的翻译。ibus 的 keyval 就是 X keysym，修饰位是 GDK 风格。
//!
//! 纯函数，可独立测试。

use xkeysym::{KeyCode, Keysym};

use crate::host::keys::CommandKey;

/// 一枚翻译好的键。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    /// 可打印字符（大小写已按修饰键算好）。
    Char(char),

    /// 命令键。
    Command(CommandKey),

    /// Shift 的按下 / 抬起。
    Shift,

    /// Ctrl 的按下 / 抬起。
    Control,

    /// 其他修饰键（Alt / Super 等）。
    Modifier,
}

/// 把 (keyval, 按下与否) 翻成壳内按键。
///
/// 带特殊修饰（Ctrl / Alt / Super）的字符键原样交给应用（输入法不抢快捷键），
/// 调用方在进入分流前用 [`special_modifiers`] 判断。
pub fn translate(keyval: Keysym) -> Option<Key> {
    let key = match keyval {
        Keysym::space | Keysym::KP_Space => return Some(Key::Char(' ')),
        Keysym::BackSpace => Key::Command(CommandKey::Backspace),
        Keysym::Delete | Keysym::KP_Delete => Key::Command(CommandKey::Delete),
        Keysym::Return | Keysym::KP_Enter | Keysym::Linefeed => Key::Command(CommandKey::Enter),
        Keysym::Escape => Key::Command(CommandKey::Escape),
        // GTK 客户端的 Shift+Tab 送来的是 ISO_Left_Tab 而不是 Tab + Shift
        Keysym::Tab | Keysym::KP_Tab | Keysym::ISO_Left_Tab => Key::Command(CommandKey::Tab),
        Keysym::Up | Keysym::KP_Up => Key::Command(CommandKey::Up),
        Keysym::Down | Keysym::KP_Down => Key::Command(CommandKey::Down),
        Keysym::Left | Keysym::KP_Left => Key::Command(CommandKey::Left),
        Keysym::Right | Keysym::KP_Right => Key::Command(CommandKey::Right),
        Keysym::Home | Keysym::KP_Home => Key::Command(CommandKey::Home),
        Keysym::End | Keysym::KP_End => Key::Command(CommandKey::End),
        Keysym::Prior | Keysym::KP_Prior => Key::Command(CommandKey::PageUp),
        Keysym::Next | Keysym::KP_Next => Key::Command(CommandKey::PageDown),
        Keysym::Shift_L | Keysym::Shift_R | Keysym::Shift_Lock => Key::Shift,
        Keysym::Control_L | Keysym::Control_R => Key::Control,
        Keysym::Alt_L
        | Keysym::Alt_R
        | Keysym::Meta_L
        | Keysym::Meta_R
        | Keysym::Super_L
        | Keysym::Super_R
        | Keysym::Hyper_L
        | Keysym::Hyper_R
        | Keysym::Caps_Lock
        | Keysym::Num_Lock
        | Keysym::ISO_Level3_Shift
        | Keysym::ISO_Level5_Shift => Key::Modifier,
        // 小键盘数字当普通数字（组句里能选词、非组句能进直输段）：KP_0..KP_9 是 0xffb0..0xffb9
        Keysym::KP_Multiply => return Some(Key::Char('*')),
        Keysym::KP_Add => return Some(Key::Char('+')),
        Keysym::KP_Subtract => return Some(Key::Char('-')),
        Keysym::KP_Divide => return Some(Key::Char('/')),
        Keysym::KP_Decimal | Keysym::KP_Separator => return Some(Key::Char('.')),
        _ => {
            let raw = u32::from(keyval);
            if (0xffb0..=0xffb9).contains(&raw) {
                return Some(Key::Char(char::from(b'0' + (raw - 0xffb0) as u8)));
            }
            // ASCII 可打印范围直接转字符；其余（功能键、非拉丁布局）交给应用
            if (0x20..=0x7e).contains(&raw) {
                return Some(Key::Char(char::from_u32(raw)?));
            }
            return None;
        }
    };
    Some(key)
}

/// 「修饰键 + 数字」的快捷键要按**物理键**认：Shift 会把数字行的 keyval 变成 `!@#$…`
/// （mac 壳按 keyCode 认是同一件事）。
///
/// 键码在 Linux 上有两种惯例并存，都不能只认一种：
/// - GTK 直连 ibus 的客户端送 X 码（数字行 10–18）——GDK Wayland 对 evdev 键码 + 8
///   （`gdkseat-wayland.c` 的 `deliver_key_event(data, time, key + 8, …)`）；
/// - gnome-shell 的 text-input 路径（Electron / Chromium / Firefox 等 zwp_text_input 客户端）
///   送 evdev 裸码（数字行 2–10）——`inputMethod.js` 的 `process_key_event_async(…,
///   event.get_key_code() - 8, …)`（注释原话「Convert XKB keycodes to evcodes」）。
///
/// 所以 keyval 优先：keyval 本身是数字（Ctrl+数字不改 keyval；AZERTY 这类要 Shift 才出数字的
/// 布局）或 US 系布局 Shift 出的符号，直接得到数字、与键码无关；键码只做非 US 符号布局的兜底
/// （2–9 只有 evdev 会用——X 键码 8 以下空缺；11–18 按 X 数字行认，但要先排除 evdev 语义下
/// 落在这段的字符 0 - = q w e，否则 evdev 路径的 Shift+0 / Shift+减号 / Shift+Q 会误删候选；
/// 键码 10 两边都是数字行——X 的 1、evdev 的 9——没法分辨，只认 keyval）。
pub fn digit_for_event(c: char, keycode: KeyCode) -> Option<usize> {
    if let Some(digit) = c.to_digit(10) {
        return (1..=9).contains(&digit).then_some(digit as usize);
    }
    const SHIFTED_SYMBOLS: [char; 9] = ['!', '@', '#', '$', '%', '^', '&', '*', '('];
    if let Some(index) = SHIFTED_SYMBOLS.iter().position(|&symbol| symbol == c) {
        return Some(index + 1);
    }
    let code = u32::from(keycode);
    if (2..=9).contains(&code) {
        return Some((code - 1) as usize);
    }
    // evdev 键码 11–18 是 KEY_0 KEY_MINUS KEY_EQUAL Backspace Tab Q W E
    // （Backspace / Tab 走命令键，到不了这里）：这些字符说明这不是数字行
    const EVDEV_11_18: [char; 12] = ['0', ')', '-', '_', '=', '+', 'q', 'Q', 'w', 'W', 'e', 'E'];
    if (11..=18).contains(&code) && !EVDEV_11_18.contains(&c) {
        return Some((code - 9) as usize);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letters_and_digits_become_chars() {
        assert_eq!(translate(Keysym::a), Some(Key::Char('a')));
        assert_eq!(translate(Keysym::A), Some(Key::Char('A')));
        assert_eq!(translate(Keysym::_1), Some(Key::Char('1')));
        assert_eq!(translate(Keysym::KP_5), Some(Key::Char('5')));
        assert_eq!(translate(Keysym::space), Some(Key::Char(' ')));
    }

    #[test]
    fn commands_map_to_the_right_kind() {
        assert_eq!(
            translate(Keysym::Return),
            Some(Key::Command(CommandKey::Enter))
        );
        assert_eq!(
            translate(Keysym::BackSpace),
            Some(Key::Command(CommandKey::Backspace))
        );
        assert_eq!(
            translate(Keysym::Prior),
            Some(Key::Command(CommandKey::PageUp))
        );
        assert_eq!(
            translate(Keysym::Left),
            Some(Key::Command(CommandKey::Left))
        );
    }

    #[test]
    fn modifiers_are_recognized_and_f_keys_pass() {
        assert_eq!(translate(Keysym::Shift_L), Some(Key::Shift));
        assert_eq!(translate(Keysym::Control_L), Some(Key::Control));
        assert_eq!(translate(Keysym::Caps_Lock), Some(Key::Modifier));
        assert_eq!(translate(Keysym::Alt_L), Some(Key::Modifier));
        assert_eq!(translate(Keysym::F1), None);
    }

    #[test]
    fn shifted_tab_and_numpad_space_map_like_their_plain_forms() {
        // GTK 客户端的 Shift+Tab
        assert_eq!(
            translate(Keysym::ISO_Left_Tab),
            Some(Key::Command(CommandKey::Tab))
        );
        assert_eq!(translate(Keysym::KP_Space), Some(Key::Char(' ')));
    }

    #[test]
    fn digit_events_are_recognized_across_keyval_and_both_keycode_conventions() {
        // keyval 本身是数字：Ctrl+数字（修饰键不改 keyval）、AZERTY（Shift 才出数字）
        assert_eq!(digit_for_event('4', KeyCode::new(0)), Some(4));
        assert_eq!(digit_for_event('0', KeyCode::new(0)), None, "0 不是选格");
        // US 系布局 Shift 出的符号，与键码无关
        assert_eq!(digit_for_event('!', KeyCode::new(0)), Some(1));
        assert_eq!(digit_for_event('@', KeyCode::new(0)), Some(2));
        assert_eq!(digit_for_event('(', KeyCode::new(0)), Some(9));
        // evdev 裸码（gnome-shell text-input 路径）：数字行 2–10
        assert_eq!(digit_for_event('!', KeyCode::new(2)), Some(1));
        assert_eq!(digit_for_event('&', KeyCode::new(8)), Some(7));
        // 键码兜底服务非 US 符号布局：UK 的 Shift+2 是 '"'，靠键码认出数字 2
        assert_eq!(digit_for_event('"', KeyCode::new(3)), Some(2));
        // X 码（GTK 直连）：数字行 10–18
        assert_eq!(digit_for_event('@', KeyCode::new(11)), Some(2));
        assert_eq!(digit_for_event('*', KeyCode::new(17)), Some(8));
        // 键码 10 两边都是数字行（X 的 1、evdev 的 9），只认 keyval
        assert_eq!(digit_for_event('!', KeyCode::new(10)), Some(1));
        assert_eq!(digit_for_event('(', KeyCode::new(10)), Some(9));
        assert_eq!(digit_for_event('"', KeyCode::new(10)), None);
    }

    #[test]
    fn non_digit_keys_never_become_digits() {
        // Shift+0（US ')'）两条路都不认：符号表没有；evdev 11 语义是 KEY_0
        assert_eq!(digit_for_event(')', KeyCode::new(19)), None); // X 的 0 键
        assert_eq!(digit_for_event(')', KeyCode::new(11)), None); // evdev KEY_0
        // Shift+减号（US '_'）、Shift+等号（'+'）、Shift+Q/W/E：evdev 语义落在 11–18 区间，要排除
        assert_eq!(digit_for_event('_', KeyCode::new(13)), None);
        assert_eq!(digit_for_event('+', KeyCode::new(13)), None);
        assert_eq!(digit_for_event('q', KeyCode::new(16)), None);
        assert_eq!(digit_for_event('Q', KeyCode::new(16)), None);
        assert_eq!(digit_for_event('e', KeyCode::new(18)), None);
        // 同一些键在 X 码下不在数字行区间，天然不认
        assert_eq!(digit_for_event('_', KeyCode::new(20)), None);
        assert_eq!(digit_for_event('Q', KeyCode::new(24)), None);
        // 客户端没给键码、keyval 也不是数字 / US 符号
        assert_eq!(digit_for_event('"', KeyCode::new(0)), None);
    }
}
