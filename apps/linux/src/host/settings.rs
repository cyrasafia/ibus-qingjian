//! 配置状态：加载 `config.toml`、按 mtime 热重载。
//!
//! 解析失败沿用上一份生效的配置并记日志——半保存 / 写坏的文件不能让输入法的方案、
//! 每页候选数这些行为突然回默认值。

use std::path::Path;
use std::time::SystemTime;

use qingjian_platform::Config;

/// 当前配置与它的来源 mtime。
#[derive(Debug)]
pub struct Settings {
    /// 当前生效的配置。
    config: Config,

    /// 上一轮成功读取的配置文件修改时间；文件不存在为 `None`。
    modified: Option<SystemTime>,
}

impl Settings {
    /// 首次加载：不存在就先写带说明的模板；读不了按默认值。
    pub fn load(path: &Path) -> Self {
        if let Err(error) = Config::write_template_if_missing(path) {
            tracing::warn!(path = %path.display(), %error, "配置模板写不了");
        }
        let modified = file_mtime(path);
        match Config::load(path) {
            Ok(config) => {
                tracing::info!(path = %path.display(), "配置已加载");
                Self { config, modified }
            }
            Err(error) => {
                tracing::warn!(path = %path.display(), %error, "配置解析失败，用默认值");
                Self {
                    config: Config::default(),
                    modified,
                }
            }
        }
    }

    /// 直接用给定配置（测试用，不碰文件）。
    #[cfg(test)]
    pub fn with_config(config: Config) -> Self {
        Self {
            config,
            modified: None,
        }
    }

    /// 文件改了（mtime 变化）才整份重读。解析失败沿用当前配置，只记下 mtime 免得每轮重试。
    /// 返回是否拿到了新配置（调用方据此重新套用）。
    pub fn reload_if_changed(&mut self, path: &Path) -> bool {
        let modified = file_mtime(path);
        if modified == self.modified {
            return false;
        }
        match Config::load(path) {
            Ok(config) => {
                self.config = config;
                self.modified = modified;
                tracing::info!(path = %path.display(), "配置已热加载");
                true
            }
            Err(error) => {
                self.modified = modified;
                tracing::warn!(path = %path.display(), %error, "配置解析失败，沿用上一份");
                false
            }
        }
    }

    /// 当前生效的配置。
    pub fn config(&self) -> &Config {
        &self.config
    }
}

fn file_mtime(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_config(tag: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "qingjian-settings-test-{tag}-{}.toml",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        path
    }

    #[test]
    fn parse_failure_keeps_the_previous_config() {
        let path = temp_config("keep");
        std::fs::write(&path, "[general]\npage_size = 5\n").unwrap();
        let mut settings = Settings::load(&path);
        assert_eq!(settings.config().general.page_size(), 5);
        // 写坏的 TOML：mtime 变了、读失败了，生效的配置要还是上一份
        std::fs::write(&path, "[general]\npage_size = ??").unwrap();
        assert!(!settings.reload_if_changed(&path));
        assert_eq!(
            settings.config().general.page_size(),
            5,
            "解析失败不该回默认值"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn valid_change_is_reloaded() {
        let path = temp_config("reload");
        std::fs::write(&path, "[general]\npage_size = 5\n").unwrap();
        let mut settings = Settings::load(&path);
        std::fs::write(&path, "[general]\npage_size = 7\n").unwrap();
        assert!(settings.reload_if_changed(&path));
        assert_eq!(settings.config().general.page_size(), 7);
        // mtime 没再变就不重读
        assert!(!settings.reload_if_changed(&path));
        let _ = std::fs::remove_file(&path);
    }
}
