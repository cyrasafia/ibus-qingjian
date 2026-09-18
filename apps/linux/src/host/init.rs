//! 装配 Engine：词库、语言模型、学习数据、英文词表，全部本机加载，不接翻译与联想。
//!
//! 词库必需要有（没有就报错退出，等不到候选）；语言模型与英文词表可选，缺了退化。

use std::time::Instant;

use qingjian_core::Engine;
use qingjian_dictionary::{Dictionary, WordList};
use qingjian_learning::FrequencyLearner;
use qingjian_lm::BigramModel;

use super::paths;
use crate::host::HostError;

/// 建好 Engine。装配是这个壳里唯一知道具体 Learner 类型的地方。
pub fn build_engine() -> Result<Engine, HostError> {
    let started = Instant::now();
    let dict_path = paths::bundled_file("dict.qj")
        .or_else(|| paths::bundled_file("dict.tsv"))
        .ok_or(HostError::Dictionary)?;
    let dictionary = Dictionary::from_path(&dict_path)?;
    let dict_ms = started.elapsed().as_millis();
    let data_dir = paths::user_data_dir();
    let learner = match FrequencyLearner::from_path(data_dir.join("user.tsv")) {
        Ok(learner) => learner,
        Err(error) => {
            tracing::error!(path = %data_dir.display(), %error, "学习数据读不了，本次只在内存里学");
            FrequencyLearner::default()
        }
    };
    tracing::info!(
        dict = %dict_path.display(),
        entries = dictionary.len(),
        learned = learner.len(),
        dict_ms,
        "数据加载完成"
    );
    let mut engine = Engine::new(dictionary).with_learner(Box::new(learner));
    // 英文词表（英文候选 / 中英混输）可选
    if let Some(path) = paths::bundled_file("english.tsv") {
        match WordList::from_path(&path) {
            Ok(words) => {
                tracing::info!(words = words.len(), "英文词表已加载");
                engine = engine.with_english(words);
            }
            Err(error) => tracing::warn!(%error, "英文词表加载失败，跳过"),
        }
    }
    // 语言模型可选：没有就退化成一元词频整句
    if let Some(path) = paths::bundled_file("lm.qj") {
        match BigramModel::from_path(&path) {
            Ok(model) => {
                tracing::info!(
                    words = model.word_count(),
                    bigrams = model.bigram_count(),
                    load_ms = started.elapsed().as_millis(),
                    "语言模型已加载"
                );
                engine = engine.with_language_model(Box::new(model));
            }
            Err(error) => tracing::warn!(%error, "语言模型加载失败，按一元退化"),
        }
    }
    Ok(engine)
}
