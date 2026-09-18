//! 候选会话状态：一轮查询的候选与高亮光标。
//!
//! 分页由呈现层（ibus LookupTable）按 `page_size` 切，这里只记候选列表与绝对下标的光标，
//! 与 mac 壳 `Session` 同一套语义（没有云端槽位）：数字选当前页第 N 格、上下键动高亮、翻页落到新页第一格。

use qingjian_core::Candidate;

/// 一轮查询后的候选与高亮。
#[derive(Debug, Default)]
pub struct Session {
    /// 本轮候选（整个候选表）。
    candidates: Vec<Candidate>,

    /// 高亮下标（绝对）。
    highlighted: usize,

    /// 本轮内联 preedit（拼音 marked text；查询失败时是原始字母）。
    preedit: String,

    /// preedit 光标（字符位）。
    preedit_cursor: usize,
}

impl Session {
    /// 新一轮查询：换候选与 preedit，高亮第一个。
    pub fn reset(&mut self, preedit: String, preedit_cursor: usize, candidates: Vec<Candidate>) {
        self.candidates = candidates;
        self.highlighted = 0;
        self.preedit = preedit;
        self.preedit_cursor = preedit_cursor;
    }

    /// 清空（收窗）。
    pub fn clear(&mut self) {
        self.candidates.clear();
        self.highlighted = 0;
        self.preedit.clear();
        self.preedit_cursor = 0;
    }

    /// 本轮候选。
    pub fn candidates(&self) -> &[Candidate] {
        &self.candidates
    }

    /// 高亮下标。
    pub fn highlighted(&self) -> usize {
        self.highlighted
    }

    /// 本轮内联 preedit 与字符光标。
    pub fn preedit(&self) -> (&str, usize) {
        (&self.preedit, self.preedit_cursor)
    }

    /// 第 `index` 个候选。
    pub fn candidate(&self, index: usize) -> Option<&Candidate> {
        self.candidates.get(index)
    }

    /// 当前页第 `offset` 格的绝对下标；越界返回 `None`。
    pub fn index_on_page(&self, offset: usize, page_size: usize) -> Option<usize> {
        let index = self.page(page_size) * page_size + offset;
        (offset < page_size && index < self.candidates.len()).then_some(index)
    }

    /// 光标所在的页。
    pub fn page(&self, page_size: usize) -> usize {
        self.highlighted / page_size.max(1)
    }

    /// 高亮上下移动，越界不动。返回是否变化。
    pub fn move_highlight(&mut self, delta: isize) -> bool {
        let Some(last) = self.candidates.len().checked_sub(1) else {
            return false;
        };
        let next = (self.highlighted as isize + delta).clamp(0, last as isize) as usize;
        if next == self.highlighted {
            return false;
        }
        self.highlighted = next;
        true
    }

    /// 翻页，高亮落到新页第一格；已在首页 / 末页不动。返回是否变化。
    pub fn turn_page(&mut self, delta: isize, page_size: usize) -> bool {
        let pages = self.candidates.len().div_ceil(page_size.max(1)).max(1);
        let current = self.page(page_size) as isize;
        let next = (current + delta).clamp(0, pages as isize - 1);
        if next == current {
            return false;
        }
        self.highlighted = next as usize * page_size;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use qingjian_core::CandidateKind;

    fn candidates(count: usize) -> Vec<Candidate> {
        (0..count)
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
    fn digits_map_to_the_current_page() {
        let mut session = Session::default();
        session.reset(String::new(), 0, candidates(12));
        assert_eq!(session.index_on_page(3, 9), Some(3));
        assert_eq!(session.index_on_page(9, 9), None);
        assert!(session.turn_page(1, 9));
        assert_eq!(session.highlighted(), 9);
        assert_eq!(session.index_on_page(0, 9), Some(9));
        assert_eq!(session.index_on_page(2, 9), Some(11));
        assert_eq!(session.index_on_page(3, 9), None);
    }

    #[test]
    fn highlight_moves_and_clamps() {
        let mut session = Session::default();
        session.reset(String::new(), 0, candidates(3));
        assert!(session.move_highlight(1));
        assert_eq!(session.highlighted(), 1);
        assert!(session.move_highlight(5));
        assert_eq!(session.highlighted(), 2);
        assert!(!session.move_highlight(1));
        assert!(session.move_highlight(-5));
        assert_eq!(session.highlighted(), 0);
        assert!(!session.move_highlight(-1));
    }

    #[test]
    fn turning_past_the_edges_does_nothing() {
        let mut session = Session::default();
        session.reset(String::new(), 0, candidates(9));
        assert!(!session.turn_page(-1, 9));
        assert!(!session.turn_page(1, 9));
        session.reset(String::new(), 0, candidates(1));
        assert!(!session.turn_page(1, 9));
        session.clear();
        assert!(!session.turn_page(1, 9));
    }
}
