//! 呈现：把 Host 的状态翻译成 ibus 的 UI 信号（preedit、候选表、上屏）。
//!
//! 分两步：锁内构帧（[`Frame`]，纯数据），锁外发送（await）。preedit 内联在应用里显示拼音
//! （带 `'` 分隔与光标，与 mac 壳的 marked text 同一内容）；不支持内联 preedit 的客户端
//! （XIM、未声明能力的 text-input 路径）由面板兜底显示，但那条路并非所有客户端都走得到，
//! 所以同帧再把拼音发一遍辅助行（`UpdateAuxiliaryText`）——主流 ibus 引擎都这么做，
//! 候选窗里始终有拼音落点。收窗 = 空候选 + 隐藏 preedit。

use librush::ibus::{IBusEngineBackend, IBusOrientation, IBusPreeditFocusMode, LookupTable};
use zbus::object_server::SignalEmitter;

use super::engine::QingjianEngine;
use crate::host::Host;
use qingjian_platform::LayoutMode;

/// 一帧要发的 UI 状态。
#[derive(Debug)]
pub struct Frame {
    /// 内联 preedit 文本（拼音）。
    pub preedit: String,

    /// preedit 光标（字符位）。
    pub cursor: u32,

    /// preedit 是否可见。
    pub preedit_visible: bool,

    /// 辅助行文本：拼音；删候选后右侧带一句提示（拼音行右侧，敲下一键就没）。
    pub aux: String,

    /// 候选表。
    pub table: LookupTable,

    /// 候选表是否可见。
    pub table_visible: bool,
}

/// 查询成功后构 preedit 的显示文本与字符光标。
///
/// 缺省显示解出的全拼（`Query::marked_text`，双拼 `nihc` → `ni'hao`）；
/// `[general] raw_preedit` 开着时显示敲的原始键（`nihc`）——`typed_text` 把中文模式下
/// Shift 敲的大写还原，缓冲全 ASCII、字节位即字符位（组句中的 `'` 也原样保留）。
/// 两条路都不掺删候选提示（内联 preedit 是应用里的 marked text，混进去光标换算全乱）。
pub fn preedit_of(host: &Host, query: &qingjian_core::Query) -> (String, usize) {
    if host.raw_preedit {
        let text = host.engine.composition().typed_text();
        let cursor = host.engine.composition().cursor();
        (text, cursor)
    } else {
        (query.marked_text(), query.marked_cursor())
    }
}

/// 重查候选后构帧（按键改了缓冲区之后用这个）。
///
/// 一次查询同时拿候选与 preedit（查询是这里最贵的活，不能每键跑两遍，对齐 mac 壳 refresh 的做法）；
/// 查询失败（整段切不动）退回显示原始字母、不出候选。
pub fn frame_after_change(host: &mut Host) -> Frame {
    match host.engine.query() {
        Ok(query) => {
            let (preedit, cursor) = preedit_of(host, &query);
            let candidates = query.candidates.items;
            host.session.reset(preedit, cursor, candidates);
        }
        Err(_) => {
            // 查询失败（整段切不动）：回退显示敲的原始键——没有解出的全拼可显示，
            // 大写还原（typed_text）与原样上屏（take_raw）走同一个口径
            let text = host.engine.composition().typed_text();
            let cursor = host.engine.composition().cursor();
            host.session.reset(text, cursor, Vec::new());
        }
    }
    frame_of(host)
}

/// 不重查、按当前状态构帧（只是动了高亮 / 翻页时用这个）。
pub fn frame_of(host: &Host) -> Frame {
    let composing = !host.engine.composition().is_empty();
    let (preedit, cursor) = host.session.preedit();
    // 辅助行 = 拼音 + 删候选提示：ibustext 不带样式，「灰字在拼音行右侧」只能并成一行；
    // 内联 preedit 不掺提示——那是应用里的 marked text，混进去光标换算全乱
    let aux = host
        .notice
        .as_ref()
        .filter(|_| composing)
        .map(|notice| format!("{preedit}  {notice}"))
        .unwrap_or_else(|| preedit.to_owned());
    let table = lookup_table(host);
    Frame {
        preedit_visible: composing && !preedit.is_empty(),
        preedit: preedit.to_owned(),
        // marked_cursor 是字符位、ibus 的 cursor_pos 按约定是字节位：本壳的 preedit 全是 ASCII
        // （拼音、' 分隔、直输段），两者一致；哪天 preedit 出非 ASCII（如注音）要在这里换算
        cursor: cursor as u32,
        aux,
        table_visible: composing && !table.candidates().is_empty(),
        table,
    }
}

/// 把一帧发给 ibus。
///
/// 发送失败只记日志不重试：ibus 侧下一帧会整体覆盖，丢一帧的观感代价是闪一下旧内容。
pub async fn send_frame(se: &SignalEmitter<'_>, frame: &Frame) -> bool {
    let preedit = <QingjianEngine as IBusEngineBackend>::update_preedit_text(
        se,
        frame.preedit.clone(),
        frame.cursor,
        frame.preedit_visible,
        IBusPreeditFocusMode::Clear,
    )
    .await;
    if let Err(error) = &preedit {
        tracing::warn!(%error, preedit = %frame.preedit, "UpdatePreeditText 发不出去");
    }
    let aux = <QingjianEngine as IBusEngineBackend>::update_auxiliary_text(
        se,
        frame.aux.clone(),
        frame.preedit_visible,
    )
    .await;
    if let Err(error) = &aux {
        tracing::warn!(%error, text = %frame.aux, "UpdateAuxiliaryText 发不出去");
    }
    let table = <QingjianEngine as IBusEngineBackend>::update_lookup_table(
        se,
        &frame.table,
        frame.table_visible,
    )
    .await;
    if let Err(error) = &table {
        tracing::warn!(%error, visible = frame.table_visible, "UpdateLookupTable 发不出去");
    }
    preedit.is_ok() && aux.is_ok() && table.is_ok()
}

/// 依序上屏几段文本。
pub async fn commit_texts(se: &SignalEmitter<'_>, texts: &[String]) -> bool {
    let mut all_ok = true;
    for text in texts {
        all_ok &= <QingjianEngine as IBusEngineBackend>::commit_text(se, text.clone())
            .await
            .is_ok();
    }
    all_ok
}

/// 失焦 / 停用时把拼音原样交出：有就取走（返回给调用方上屏）。
pub fn take_raw(host: &mut Host) -> Option<String> {
    let raw = host.engine.take_raw();
    (!raw.is_empty()).then_some(raw)
}

/// 由会话建候选表：全部候选 + 每页大小 + 高亮光标 + 排布方向，分页交给 ibus 候选窗。
///
/// 方向必须显式下发：gnome-shell 把 `System`（librush 的缺省）当竖排处理、不再回退
/// 系统设置，不写死用户配置的横排就永远立不起来。
fn lookup_table(host: &Host) -> LookupTable {
    let texts = host
        .session
        .candidates()
        .iter()
        .map(|candidate| candidate.text.clone())
        .collect::<Vec<_>>();
    let page_size = host.page_size.clamp(1, 16) as u32;
    let mut table = LookupTable::new(texts, page_size, true, false)
        .unwrap_or_else(|_| LookupTable::new(Vec::new(), 1, true, false).expect("页大小 1 合法"));
    table.set_cursor_pos(host.session.highlighted() as i64);
    table.orientation = match host.layout {
        LayoutMode::Vertical => IBusOrientation::Vertical,
        LayoutMode::Horizontal => IBusOrientation::Horizontal,
    };
    table
}

#[cfg(test)]
mod tests {
    use super::*;
    use qingjian_core::{Candidate, CandidateKind};
    use qingjian_platform::Config;

    fn host_with_layout(layout: LayoutMode) -> Host {
        let dict = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/sample/dict.tsv");
        let mut config = Config::default();
        config.general.layout = layout;
        Host::with_engine_and_config(
            qingjian_core::Engine::new(
                qingjian_dictionary::Dictionary::from_path(dict).expect("样例词库读不了"),
            ),
            config,
        )
    }

    fn candidates() -> Vec<Candidate> {
        (0..3)
            .map(|i| Candidate {
                text: format!("词{i}"),
                kind: CandidateKind::Chinese,
                syllables: vec!["a".into()],
                reading: None,
                translation: None,
                aux_code: None,
            })
            .collect()
    }

    #[test]
    fn layout_config_maps_to_lookup_table_orientation() {
        let mut host = host_with_layout(LayoutMode::Vertical);
        host.session.reset("nihao".to_owned(), 5, candidates());
        assert_eq!(lookup_table(&host).orientation, IBusOrientation::Vertical);

        let mut host = host_with_layout(LayoutMode::Horizontal);
        host.session.reset("nihao".to_owned(), 5, candidates());
        assert_eq!(lookup_table(&host).orientation, IBusOrientation::Horizontal);
    }

    #[test]
    fn frame_hides_everything_when_not_composing() {
        let mut host = host_with_layout(LayoutMode::Vertical);
        host.session.clear();
        let frame = frame_of(&host);
        assert!(!frame.preedit_visible);
        assert!(!frame.table_visible);
    }

    #[test]
    fn session_reset_feeds_the_frame() {
        let mut host = host_with_layout(LayoutMode::Horizontal);
        for c in "ni".chars() {
            host.engine.push(c);
        }
        host.session.reset("ni".to_owned(), 2, candidates());
        let frame = frame_of(&host);
        assert!(frame.preedit_visible);
        assert_eq!(frame.preedit, "ni");
        assert_eq!(frame.aux, "ni", "没提示时辅助行就是拼音");
        assert_eq!(frame.table.candidates().len(), 3);
        assert!(frame.table_visible);
    }

    #[test]
    fn raw_preedit_shows_typed_keys_under_shuangpin() {
        // 双拼开着时缺省显示解出的全拼（nihc → ni'hao）；raw_preedit 开着则显示敲的键。
        // 两条路都要发一帧（候选照常、辅助行同内容）。
        let dict = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/sample/dict.tsv");
        let mut host = Host::with_engine_and_config(
            qingjian_core::Engine::new(
                qingjian_dictionary::Dictionary::from_path(dict).expect("样例词库读不了"),
            ),
            Config::default(),
        );
        host.engine
            .set_shuangpin(Some(qingjian_core::ShuangpinScheme::Xiaohe));
        for c in "nihc".chars() {
            host.engine.push(c);
        }
        let frame = frame_after_change(&mut host);
        assert_eq!(frame.preedit, "ni'hao", "缺省显示解出的全拼");
        assert!(frame.table_visible, "候选照常");
        assert_eq!(frame.aux, "ni'hao");

        host.raw_preedit = true;
        let frame = frame_after_change(&mut host);
        assert_eq!(frame.preedit, "nihc", "raw_preedit 显示敲的原始键");
        assert_eq!(frame.aux, "nihc", "辅助行跟着显示原始键");
        assert!(frame.table_visible, "候选不受显示模式影响");
        assert_eq!(frame.cursor, 4);
    }

    #[test]
    fn query_failure_fallback_shows_typed_keys() {
        // 整段切不动（`Ii` 的头一个 I 是大写直进，i 起不了音节）：回退显示敲的原始键，
        // Shift 大写还原与原样上屏（take_raw）同一个口径——不能显示成小写
        let dict = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/sample/dict.tsv");
        let mut host = Host::with_engine_and_config(
            qingjian_core::Engine::new(
                qingjian_dictionary::Dictionary::from_path(dict).expect("样例词库读不了"),
            ),
            Config::default(),
        );
        host.engine.set_shift_letter_compose(true);
        for c in "Ii".chars() {
            host.engine.push(c);
        }
        let frame = frame_after_change(&mut host);
        assert!(!frame.table_visible, "切不动不出候选");
        assert_eq!(frame.preedit, "Ii", "回退显示敲的键（大写还原）");
        assert_eq!(frame.aux, "Ii");
    }

    #[test]
    fn delete_notice_rides_the_auxiliary_line_beside_the_pinyin() {
        let mut host = host_with_layout(LayoutMode::Vertical);
        for c in "ni".chars() {
            host.engine.push(c);
        }
        host.session.reset("ni".to_owned(), 2, candidates());
        host.notice = Some("「你」是词库里的词，也没有学习记录，没什么可删".to_owned());
        let frame = frame_of(&host);
        assert_eq!(frame.preedit, "ni", "内联 preedit 不掺提示");
        assert!(
            frame.aux.starts_with("ni  「你」"),
            "提示并排在拼音右侧：{}",
            frame.aux
        );
        // 收窗（失焦 / Reset 会同时清 engine 与 session）：提示不再挂在辅助行上
        host.engine.clear();
        host.session.clear();
        host.notice = None;
        let frame = frame_of(&host);
        assert_eq!(frame.aux, "");
    }
}
