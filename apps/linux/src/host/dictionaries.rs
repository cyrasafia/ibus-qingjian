//! 附加词库装配：随包领域词库与用户 `dicts/` 目录，启动装一次，之后两路触发重装——
//! 配置里 `[dictionaries]` 开关变化（[`Host::apply_config`]）、目录里文件增删 / 更新（轮询快照比对）。
//! 与 mac 壳 `reload_dictionaries`、Windows Server 的 `ConfigReload` 同一语义。

use qingjian_platform::extra_dictionaries;

use super::{Host, paths};

impl Host {
    /// 按当前配置重新装配附加词库，并记下已应用的配置与用户目录文件快照。
    pub fn reload_dictionaries(&mut self) {
        let config = self.settings.config().dictionaries.clone();
        let bundled = paths::bundled_dicts_dir();
        let user = paths::user_dicts_dir();
        let loaded = extra_dictionaries::load(bundled.as_deref(), user.as_deref(), &config);
        tracing::info!(count = loaded.len(), "附加词库已装配");
        self.engine.set_extra_dictionaries(loaded);
        self.applied_dictionaries = config;
        self.dictionary_files = user
            .as_deref()
            .map(extra_dictionaries::snapshot)
            .unwrap_or_default();
    }

    /// 轮询：用户 `dicts/` 目录的文件快照变了（新增、移除、同名更新）才重装；
    /// 配置变化走 [`Host::apply_config`]（60 秒配置轮询的节奏）。
    ///
    /// 装配在锁内解析词库正文（与 Windows Server 同一语义）：快照比对只看元数据、每秒都便宜，
    /// 只有目录真变了才会花一次解析；拖进几十万条的大 TSV 时下一次 tick 会持锁到解析完，
    /// 按键最多停顿这一个解析时长。
    pub fn poll_dictionaries(&mut self) -> bool {
        let files = paths::user_dicts_dir()
            .as_deref()
            .map(extra_dictionaries::snapshot)
            .unwrap_or_default();
        if files == self.dictionary_files {
            return false;
        }
        tracing::info!("用户词库目录有变化，重新装配");
        self.reload_dictionaries();
        true
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use qingjian_platform::{Config, extra_dictionaries};

    /// 临时目录：带进程号隔离，用完清掉。
    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("qingjian-dicts-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_user_dict(dir: &Path, name: &str, text: &str) {
        std::fs::write(dir.join("dicts").join(name), text).unwrap();
    }

    #[test]
    fn load_respects_config_switches() {
        let dir = temp_dir("load");
        std::fs::create_dir_all(dir.join("dicts")).unwrap();
        write_user_dict(&dir, "law.tsv", "合同法\the tong fa\t100\n");

        let mut config = Config::default();
        let dicts = dir.join("dicts");
        let loaded = extra_dictionaries::load(None, Some(&dicts), &config.dictionaries);
        assert_eq!(loaded.len(), 1);
        assert_eq!(
            loaded[0].lookup(&["he", "tong", "fa"], false)[0].text,
            "合同法"
        );

        // 关掉后不加载
        config.dictionaries.disabled = vec!["law".to_owned()];
        assert!(extra_dictionaries::load(None, Some(&dicts), &config.dictionaries).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn snapshot_detects_add_update_and_removal() {
        let dir = temp_dir("snapshot");
        std::fs::create_dir_all(dir.join("dicts")).unwrap();
        let dicts = dir.join("dicts");
        assert!(extra_dictionaries::snapshot(&dicts).is_empty());

        write_user_dict(&dir, "a.tsv", "词一\tci yi\t10\n");
        let first = extra_dictionaries::snapshot(&dicts);
        assert_eq!(first.len(), 1);

        // 同名更新：mtime 与长度进快照
        std::fs::write(dicts.join("a.tsv"), "词一\tci yi\t99\n词二\tci er\t1\n").unwrap();
        assert_ne!(extra_dictionaries::snapshot(&dicts), first);

        // 没再变就稳定
        let second = extra_dictionaries::snapshot(&dicts);
        assert_eq!(extra_dictionaries::snapshot(&dicts), second);

        // 移除也能发现
        std::fs::remove_file(dicts.join("a.tsv")).unwrap();
        assert_ne!(extra_dictionaries::snapshot(&dicts), second);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
