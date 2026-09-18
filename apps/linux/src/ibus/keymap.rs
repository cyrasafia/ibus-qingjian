//! keysym 到壳内按键的翻译。ibus 的 keyval 就是 X keysym，修饰位是 GDK 风格。
//!
//! 纯函数，可独立测试。

use xkeysym::Keysym;

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
}
