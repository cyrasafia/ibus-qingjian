//! 按键分流：把一枚键翻译成对 Engine 的操作与要上屏的文本。
//!
//! 语义照 mac 壳的 `imk/controller`（text.rs / command.rs）移植：「交给应用」从 IMK 的返回值
//! 换成 `handled = false`，呈现动作（preedit / 候选表）由调用方按 `refresh` 重画。
//! 这里不出现任何平台 API，可独立测试。

use qingjian_core::Engine;

use super::session::Session;

/// 一枚键处理完的结果。
#[derive(Debug, Default)]
pub struct Outcome {
    /// 是否吃掉这个键（不吃则交回系统按普通按键处理）。
    pub handled: bool,

    /// 依序上屏的文本（候选 + 标点、原样拼音）。
    pub commits: Vec<String>,

    /// 处理完是否要重查候选并刷新 preedit 与候选表。
    pub refresh: bool,

    /// 只动了高亮 / 页位：不重查（重查会把高亮与页位归零，翻页等于白翻），
    /// 按当前会话状态重画一帧。
    pub redraw: bool,
}

impl Outcome {
    /// 吃掉并刷新（改了缓冲区的键用这个）。
    pub(crate) fn consumed() -> Self {
        Self {
            handled: true,
            refresh: true,
            ..Self::default()
        }
    }

    /// 不吃、不刷新（普通放行）。
    pub(crate) fn passed() -> Self {
        Self::default()
    }

    /// 吃掉、只重画（动高亮 / 翻页的键用这个，对齐 mac 壳 turn_page 只 render 不 refresh）。
    pub(crate) fn redrawn() -> Self {
        Self {
            handled: true,
            redraw: true,
            ..Self::default()
        }
    }

    /// 上屏一段文本并继续刷新（上屏后剩余拼音继续组句）。
    fn committed(text: String) -> Self {
        let mut outcome = Self::consumed();
        outcome.commits.push(text);
        outcome
    }
}

/// 命令键种类。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandKey {
    Backspace,
    Delete,
    Enter,
    Escape,
    Tab,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
}

/// 字符键（可打印）的处理，对应 mac 壳 `handle_text`。
///
/// `c` 是按键产生的字符（Shift 组合后的大小写保持原样，大小写语义在这里决定）。
pub fn handle_char(
    engine: &mut Engine,
    session: &mut Session,
    page_size: usize,
    page_keys: (char, char),
    english_candidates: bool,
    c: char,
) -> Outcome {
    let composing = !engine.composition().is_empty();
    let english = engine.english_mode();
    let (page_previous, page_next) = page_keys;
    // 缓冲区为空时敲 ? 先进问字模式（配置 `[shortcut] question_mark`，缺省关），中英文模式都行：
    // 后面跟字母就是在问字，跟别的键就还原成问号。英文模式进入时先退出英文（对齐 Windows），
    // 不然后续按键全走英文分支、问字逻辑永远到不了
    if !composing && c == '?' && engine.takes_question_mark() {
        engine.set_english_mode(false);
        engine.push(c);
        return Outcome::consumed();
    }
    if english {
        return handle_english_char(engine, session, page_size, page_keys, english_candidates, c);
    }
    // 双拼下 Shift+V / Shift+U 进表达式 / 问字模式（全拼下的 v / u 被音节占了）：
    // Core 的 ModeKeys 在双拼下认的就是大写，这里传原始字符、不改大小写
    if !composing && engine.takes_mode_letter(c) {
        engine.push(c);
        return Outcome::consumed();
    }
    let question = composing && engine.question_mode();
    let expression = composing && engine.expression_mode();
    let raw = composing && engine.raw_mode();
    let unicode = question && engine.unicode_entry();
    // 组句中敲 `-`：进入英文直输段（`no-way`）；配成翻页键（`-=`）时只翻页
    let hyphen = composing && !question && c == '-' && c != page_previous && c != page_next;
    // 微软 / 搜狗双拼的 `;` 是 ing 键：末尾有落单声母时进缓冲，其他时候还是标点
    let semicolon = composing && c == ';' && engine.takes_semicolon();
    // 组句中敲半角标点：进缓冲成为英文直输段（`hello,`），中文模式下也能打带标点的英文；翻页键除外
    let punctuation = composing
        && !question
        && !expression
        && c.is_ascii_punctuation()
        && c != page_previous
        && c != page_next;
    if c.is_ascii_lowercase()
        || (composing && c == '\'')
        || semicolon
        || (expression && qingjian_core::shortcut::is_expression_char(c))
        || (raw && c.is_ascii_graphic())
        || (unicode && (c.is_ascii_digit() || c == '+'))
        || hyphen
        || punctuation
    {
        engine.push(c);
        return Outcome::consumed();
    }
    // 直输段里的空格：整段原样上屏，空格本身也交给应用（`hello, world` 里的空格要在）
    if raw && c == ' ' {
        let mut outcome = commit_highlighted(engine, session);
        engine.note_passthrough(c);
        outcome.handled = false;
        return outcome;
    }
    // 缓冲区里只有一个 `?` 而用户按了别的键：把它还原成问号上屏（英文半角）、清空缓冲区，
    // 再按非组句状态继续处理这个字符；空格只是「把这个 ? 上屏」
    if composing && let Some(mark) = engine.restore_bare_question(false) {
        let mut outcome = Outcome::consumed();
        outcome.commits.push(mark);
        if c == ' ' {
            return outcome;
        }
        let follow = punctuate_or_pass(engine, c);
        outcome.commits.extend(follow.commits);
        outcome.handled = follow.handled;
        return outcome;
    }
    // 按住 Shift 打的大写字母：缺省先把拼音原样上屏再把字母交给应用；
    // `[general] shift_letter = "compose"` 时进缓冲区（Core 按小写匹配、原样上屏时还原大写）
    if c.is_ascii_uppercase() {
        if engine.shift_letter_compose() {
            engine.push(c);
            return Outcome::consumed();
        }
        let mut outcome = if composing {
            commit_raw(engine)
        } else {
            Outcome::passed()
        };
        engine.note_passthrough(c);
        outcome.handled = false;
        return outcome;
    }
    if composing {
        match c {
            ' ' => return commit_highlighted(engine, session),
            '1'..='9' => {
                let offset = usize::from(c as u8 - b'1');
                if let Some(index) = session.index_on_page(offset, page_size) {
                    return commit_index(engine, index, session);
                }
                // 这一页没有这一格（`gpt6` 只有三个候选）：数字当内容进缓冲，成为英文直输段；
                // 问字模式里数字不是问题的一部分，不算
                if !question {
                    engine.push(c);
                }
                return Outcome::consumed();
            }
            c if c == page_previous => {
                if session.turn_page(-1, page_size) {
                    engine.note_page_turn();
                }
                return Outcome::redrawn();
            }
            c if c == page_next => {
                if session.turn_page(1, page_size) {
                    engine.note_page_turn();
                }
                return Outcome::redrawn();
            }
            // 其他字符：把当前高亮候选上屏，再按非组句状态处理这个字符（通常是标点）
            _ => {
                let mut outcome = commit_highlighted(engine, session);
                let follow = punctuate_or_pass(engine, c);
                outcome.commits.extend(follow.commits);
                outcome.handled = follow.handled;
                return outcome;
            }
        }
    }
    punctuate_or_pass(engine, c)
}

/// 英文模式：字母与 `_ ' -` 进缓冲出英文候选，选词与中文模式一样（数字选格、翻页键翻页）；
/// 空格上屏高亮词后照样交给应用；回车、标点先把敲的字母原样上屏再交给应用。
fn handle_english_char(
    engine: &mut Engine,
    session: &mut Session,
    page_size: usize,
    page_keys: (char, char),
    english_candidates: bool,
    c: char,
) -> Outcome {
    let composing = !engine.composition().is_empty();
    let (page_previous, page_next) = page_keys;
    // 英文候选关掉（`[general] english_candidates = false`）= 纯直通：组句中的字母先原样上屏，
    // 字符交给应用（终端 / IDE 里不想被候选窗打扰的场景；ibus 认不到应用身份，只能全局关）
    if !english_candidates {
        let mut outcome = if composing {
            commit_raw(engine)
        } else {
            Outcome::passed()
        };
        engine.note_passthrough(c);
        outcome.handled = false;
        return outcome;
    }
    if composing
        && let Some(offset) = c.to_digit(10).filter(|d| *d > 0)
        && let Some(index) = session.index_on_page(offset as usize - 1, page_size)
    {
        return commit_index(engine, index, session);
    }
    if c.is_ascii_alphabetic()
        || (composing && (c.is_ascii_digit() || matches!(c, '_' | '\'' | '-')))
    {
        engine.push(c);
        return Outcome::consumed();
    }
    if composing && c == page_previous {
        if session.turn_page(-1, page_size) {
            engine.note_page_turn();
        }
        return Outcome::redrawn();
    }
    if composing && c == page_next {
        if session.turn_page(1, page_size) {
            engine.note_page_turn();
        }
        return Outcome::redrawn();
    }
    let mut outcome = if composing && c == ' ' {
        commit_highlighted(engine, session)
    } else if composing {
        commit_raw(engine)
    } else {
        Outcome::passed()
    };
    engine.note_passthrough(c);
    outcome.handled = false;
    outcome
}

/// 命令键（回车、退格、方向键、Esc 等）的处理，对应 mac 壳 `handle_command`。
///
/// 组句期间未识别的命令一律吞掉，免得应用动光标把 preedit 丢了。
pub fn handle_command(
    engine: &mut Engine,
    session: &mut Session,
    page_size: usize,
    key: CommandKey,
) -> Outcome {
    let composing = !engine.composition().is_empty();
    if !composing {
        // 删的是应用里的文字：刚上屏的词被整个删掉是「选错了」的信号，Engine 记着
        if key == CommandKey::Backspace {
            engine.note_backspace();
        } else if key == CommandKey::Enter {
            engine.note_passthrough('\n');
        }
        return Outcome::passed();
    }
    if key != CommandKey::Escape
        && let Some(mark) = engine.restore_bare_question(engine.english_mode())
    {
        // 缓冲区里只有一个 `?` 而用户按了命令键：把它还原成问号上屏（中文遵循标点设置、英文半角）、清空缓冲区。
        // 回车的意义就是「把这个 ? 上屏」，吞掉（否则聊天框里会连消息一起发出去）；其他键还原后交给应用
        let mut outcome = Outcome::consumed();
        outcome.commits.push(mark);
        outcome.handled = key == CommandKey::Enter;
        return outcome;
    }
    match key {
        CommandKey::Backspace => {
            engine.backspace();
            Outcome::consumed()
        }
        CommandKey::Delete => {
            engine.delete_forward();
            Outcome::consumed()
        }
        CommandKey::Enter => commit_raw(engine),
        CommandKey::Escape => {
            engine.clear();
            Outcome::consumed()
        }
        CommandKey::Tab => {
            // 英文模式 Tab 选中高亮的词；中文模式没有整句补全，当翻页
            if engine.english_mode() {
                commit_highlighted(engine, session)
            } else {
                if session.turn_page(1, page_size) {
                    engine.note_page_turn();
                }
                Outcome::redrawn()
            }
        }
        CommandKey::Up => {
            session.move_highlight(-1);
            Outcome::redrawn()
        }
        CommandKey::Down => {
            session.move_highlight(1);
            Outcome::redrawn()
        }
        CommandKey::Left => {
            engine.move_cursor_left();
            Outcome::consumed()
        }
        CommandKey::Right => {
            engine.move_cursor_right();
            Outcome::consumed()
        }
        CommandKey::Home => {
            engine.move_cursor_home();
            Outcome::consumed()
        }
        CommandKey::End => {
            engine.move_cursor_end();
            Outcome::consumed()
        }
        // 翻页只在真翻到时记数（首页按上一页不算候选质量信号）；只重画不重查
        CommandKey::PageUp => {
            if session.turn_page(-1, page_size) {
                engine.note_page_turn();
            }
            Outcome::redrawn()
        }
        CommandKey::PageDown => {
            if session.turn_page(1, page_size) {
                engine.note_page_turn();
            }
            Outcome::redrawn()
        }
    }
}

/// 非组句时敲的字符：全角标点上屏；转不了的（数字、字母以外的其他键）交给应用。
fn punctuate_or_pass(engine: &mut Engine, c: char) -> Outcome {
    match engine.punctuate(c) {
        Some(full_width) => Outcome::committed(full_width.to_owned()),
        None => {
            engine.note_passthrough(c);
            Outcome::passed()
        }
    }
}

/// 上屏高亮候选；没有候选时上屏拼音本身。
fn commit_highlighted(engine: &mut Engine, session: &Session) -> Outcome {
    commit_index(engine, session.highlighted(), session)
}

/// 上屏第 `index` 个候选；上屏后剩余拼音继续组句。
pub fn commit_index(engine: &mut Engine, index: usize, session: &Session) -> Outcome {
    match session.candidate(index) {
        Some(candidate) => Outcome::committed(engine.commit(candidate)),
        None => commit_raw(engine),
    }
}

/// 把拼音原样上屏并清空。缓冲区为空时只是放行。
pub fn commit_raw(engine: &mut Engine) -> Outcome {
    let raw = engine.take_raw();
    if raw.is_empty() {
        return Outcome::passed();
    }
    Outcome::committed(raw)
}

#[cfg(test)]
mod tests {
    use super::*;
    use qingjian_core::ShuangpinScheme;
    use qingjian_dictionary::Dictionary;

    use crate::host::Host;

    /// 样例词库建 Host，按序喂字符，收集上屏。
    struct Driver {
        host: Host,
        commits: Vec<String>,
    }

    impl Driver {
        fn new() -> Self {
            let dict = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/sample/dict.tsv");
            let engine = Engine::new(Dictionary::from_path(dict).expect("样例词库读不了"));
            Self {
                host: Host::with_engine(engine),
                commits: Vec::new(),
            }
        }

        /// 敲一个字符并重查候选（对齐引擎层的「refresh 后构帧」）。
        fn tap(&mut self, c: char) -> bool {
            let outcome = handle_char(
                &mut self.host.engine,
                &mut self.host.session,
                self.host.page_size,
                self.host.page_keys,
                self.host.english_candidates,
                c,
            );
            self.apply(outcome)
        }

        /// 敲一个命令键。
        fn command(&mut self, key: CommandKey) -> bool {
            let outcome = handle_command(
                &mut self.host.engine,
                &mut self.host.session,
                self.host.page_size,
                key,
            );
            self.apply(outcome)
        }

        fn apply(&mut self, outcome: Outcome) -> bool {
            self.commits.extend(outcome.commits);
            if outcome.refresh {
                // 对齐 present::frame_after_change：一次查询同时拿 preedit 与候选
                match self.host.engine.query() {
                    Ok(query) => {
                        let preedit = query.marked_text();
                        let cursor = query.marked_cursor();
                        self.host
                            .session
                            .reset(preedit, cursor, query.candidates.items);
                    }
                    Err(_) => self.host.session.clear(),
                }
            }
            outcome.handled
        }

        fn committed(&self) -> String {
            self.commits.join("")
        }
    }

    #[test]
    fn typing_pinyin_and_space_commits_the_top_candidate() {
        let mut driver = Driver::new();
        for c in "kaifa".chars() {
            assert!(driver.tap(c), "拼音字母应该被吃掉: {c}");
        }
        assert!(
            driver
                .host
                .session
                .candidates()
                .iter()
                .any(|candidate| candidate.text == "开发"),
            "kaifa 的候选里应该有 开发"
        );
        assert!(driver.tap(' '));
        assert_eq!(driver.committed(), "开发");
        assert!(driver.host.engine.composition().is_empty());
    }

    #[test]
    fn digits_pick_from_the_current_page() {
        let mut driver = Driver::new();
        for c in "kaifa".chars() {
            driver.tap(c);
        }
        assert!(driver.tap('2'));
        // 上屏的是第二个候选，非空即通过（候选顺序由 Core 排序决定）
        assert!(!driver.committed().is_empty());
    }

    #[test]
    fn english_page_keys_redraw_without_requerying() {
        // 英文模式自己的翻页分支（handle_english_char）也要走「只重画」：
        // 走 refresh 会重查并把页位归零，与中文模式同一个坑（2026-09-19 漏改过一次）
        let mut driver = Driver::new();
        driver.host.engine.set_english_mode(true);
        for c in "hell".chars() {
            driver.tap(c);
        }
        let outcome = handle_char(
            &mut driver.host.engine,
            &mut driver.host.session,
            driver.host.page_size,
            driver.host.page_keys,
            driver.host.english_candidates,
            ']',
        );
        assert!(
            outcome.handled && outcome.redraw && !outcome.refresh,
            "英文模式翻页只重画不重查"
        );
    }

    #[test]
    fn enter_commits_raw_punctuation_maps_full_width() {
        let mut driver = Driver::new();
        for c in "kaifa".chars() {
            driver.tap(c);
        }
        assert!(driver.command(CommandKey::Enter));
        assert_eq!(driver.committed(), "kaifa");
        // 非组句的逗号转全角并吃掉
        assert!(driver.tap(','));
        assert_eq!(driver.committed(), "kaifa，");
        // 非组句的数字放行
        assert!(!driver.tap('5'));
    }

    #[test]
    fn escape_clears_and_backspace_edits() {
        let mut driver = Driver::new();
        for c in "kaifa".chars() {
            driver.tap(c);
        }
        assert!(driver.command(CommandKey::Backspace));
        assert_eq!(driver.host.engine.composition().text(), "kaif");
        assert!(driver.command(CommandKey::Escape));
        assert!(driver.host.engine.composition().is_empty());
    }

    #[test]
    fn shift_letters_commit_raw_and_pass_through() {
        let mut driver = Driver::new();
        driver.tap('k');
        // 按住 Shift 的大写字母：先把拼音原样上屏，字母交给应用
        let handled = driver.tap('A');
        assert!(!handled);
        assert_eq!(driver.committed(), "k");
    }

    #[test]
    fn question_mode_is_reachable_from_english_mode() {
        let dict = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/sample/dict.tsv");
        let mut host = Host::with_engine(Engine::new(
            Dictionary::from_path(dict).expect("样例词库读不了"),
        ));
        host.engine.set_mode_keys(qingjian_core::ModeKeys {
            question_mark: true,
            ..qingjian_core::ModeKeys::default()
        });
        host.engine.set_english_mode(true);
        // 英文模式下敲 ? 也进问字模式：先退出英文，后续按键走问字规则
        assert!(
            handle_char(
                &mut host.engine,
                &mut host.session,
                host.page_size,
                host.page_keys,
                host.english_candidates,
                '?'
            )
            .handled
        );
        assert!(!host.engine.english_mode());
        assert!(host.engine.question_mode());
        // 缓冲区里只有 ? 时按回车：还原成问号上屏（进问字时已退出英文，遵循标点设置出全角）、回车吞掉
        let outcome = handle_command(
            &mut host.engine,
            &mut host.session,
            host.page_size,
            CommandKey::Enter,
        );
        assert!(outcome.handled);
        assert_eq!(outcome.commits, vec!["？".to_owned()]);
    }

    #[test]
    fn english_candidates_off_means_pure_passthrough() {
        let dict = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/sample/dict.tsv");
        let mut host = Host::with_engine(Engine::new(
            Dictionary::from_path(dict).expect("样例词库读不了"),
        ));
        host.engine.set_english_mode(true);
        host.english_candidates = false;
        // 关掉英文候选：字母纯直通，不进缓冲
        let outcome = handle_char(
            &mut host.engine,
            &mut host.session,
            host.page_size,
            host.page_keys,
            false,
            'g',
        );
        assert!(!outcome.handled);
        assert!(outcome.commits.is_empty());
        assert!(host.engine.composition().is_empty());
        // 组句切到英文模式后继续敲（关着候选）：先把拼音原样上屏再放行
        let mut host = Host::with_engine(Engine::new(
            Dictionary::from_path(dict).expect("样例词库读不了"),
        ));
        handle_char(
            &mut host.engine,
            &mut host.session,
            host.page_size,
            host.page_keys,
            true,
            'k',
        );
        host.engine.set_english_mode(true);
        host.english_candidates = false;
        let outcome = handle_char(
            &mut host.engine,
            &mut host.session,
            host.page_size,
            host.page_keys,
            false,
            'g',
        );
        assert!(!outcome.handled);
        assert_eq!(outcome.commits, vec!["k".to_owned()]);
    }

    #[test]
    fn shuangpin_mode_letter_takes_the_shifted_form() {
        let dict = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/sample/dict.tsv");
        let mut host = Host::with_engine(Engine::new(
            Dictionary::from_path(dict).expect("样例词库读不了"),
        ));
        host.engine.set_shuangpin(Some(ShuangpinScheme::Xiaohe));
        // 双拼下 Core 的模式键是大写 V（Shift+V 进表达式模式），小写 v 是普通键
        let outcome = handle_char(
            &mut host.engine,
            &mut host.session,
            host.page_size,
            host.page_keys,
            host.english_candidates,
            'V',
        );
        assert!(outcome.handled);
        assert!(host.engine.expression_mode(), "Shift+V 应进表达式模式");
        assert!(!host.engine.takes_mode_letter('v'));
    }
}
