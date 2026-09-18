//! 呈现：把 Host 的状态翻译成 ibus 的 UI 信号（preedit、候选表、上屏）。
//!
//! 分两步：锁内构帧（[`Frame`]，纯数据），锁外发送（await）。preedit 内联在应用里显示拼音
//! （带 `'` 分隔与光标，与 mac 壳的 marked text 同一内容），候选表交给 ibus 原生候选窗渲染；
//! 辅助行 MVP 不用。收窗 = 空候选 + 隐藏 preedit。

use librush::ibus::{IBusEngineBackend, IBusPreeditFocusMode, LookupTable};
use zbus::object_server::SignalEmitter;

use super::engine::QingjianEngine;
use crate::host::Host;

/// 一帧要发的 UI 状态。
#[derive(Debug)]
pub struct Frame {
    /// 内联 preedit 文本（拼音）。
    pub preedit: String,

    /// preedit 光标（字符位）。
    pub cursor: u32,

    /// preedit 是否可见。
    pub preedit_visible: bool,

    /// 候选表。
    pub table: LookupTable,

    /// 候选表是否可见。
    pub table_visible: bool,
}

/// 重查候选后构帧（按键改了缓冲区之后用这个）。
///
/// 一次查询同时拿候选与 preedit（查询是这里最贵的活，不能每键跑两遍，对齐 mac 壳 refresh 的做法）；
/// 查询失败（整段切不动）退回显示原始字母、不出候选。
pub fn frame_after_change(host: &mut Host) -> Frame {
    match host.engine.query() {
        Ok(query) => {
            let preedit = query.marked_text();
            let cursor = query.marked_cursor();
            let candidates = query.candidates.items;
            host.session.reset(preedit, cursor, candidates);
        }
        Err(_) => {
            let text = host.engine.composition().text().to_owned();
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
    let table = lookup_table(host);
    Frame {
        preedit_visible: composing && !preedit.is_empty(),
        preedit: preedit.to_owned(),
        // marked_cursor 是字符位、ibus 的 cursor_pos 按约定是字节位：本壳的 preedit 全是 ASCII
        // （拼音、' 分隔、直输段），两者一致；哪天 preedit 出非 ASCII（如注音）要在这里换算
        cursor: cursor as u32,
        table_visible: composing && !table.candidates().is_empty(),
        table,
    }
}

/// 把一帧发给 ibus。
pub async fn send_frame(se: &SignalEmitter<'_>, frame: &Frame) -> bool {
    let mut all_ok = <QingjianEngine as IBusEngineBackend>::update_preedit_text(
        se,
        frame.preedit.clone(),
        frame.cursor,
        frame.preedit_visible,
        IBusPreeditFocusMode::Clear,
    )
    .await
    .is_ok();
    all_ok &= <QingjianEngine as IBusEngineBackend>::update_lookup_table(
        se,
        &frame.table,
        frame.table_visible,
    )
    .await
    .is_ok();
    all_ok
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

/// 由会话建候选表：全部候选 + 每页大小 + 高亮光标，分页交给 ibus 候选窗。
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
    table
}
