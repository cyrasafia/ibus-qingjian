//! XDG 路径约定：配置、用户数据、随包数据。
//!
//! - **配置** `~/.config/qingjian/config.toml`（`$XDG_CONFIG_HOME` 优先）
//! - **用户数据** `~/.local/share/qingjian/`（`$XDG_DATA_HOME` 优先）：学习数据六张表、输入日志
//! - **随包数据** `/usr/share/qingjian/`（`$QINGJIAN_DATA_DIR` 优先）：词库、语言模型、英文词表。
//!   开发时把 `QINGJIAN_DATA_DIR` 指到仓库的 `data/generated/` 即可用生成数据跑。

use std::path::PathBuf;

/// 配置文件 `~/.config/qingjian/config.toml`。
pub fn config_path() -> PathBuf {
    xdg_config_home().join("qingjian").join("config.toml")
}

/// 用户数据目录 `~/.local/share/qingjian/`，不存在则创建（学习数据要往里写）。
pub fn user_data_dir() -> PathBuf {
    let dir = xdg_data_home().join("qingjian");
    if let Err(error) = std::fs::create_dir_all(&dir) {
        tracing::warn!(path = %dir.display(), %error, "用户数据目录建不了，学习只在内存里");
    }
    dir
}

/// 随包数据文件：`$QINGJIAN_DATA_DIR` 优先，否则 `/usr/share/qingjian/`；不存在为 `None`。
pub fn bundled_file(name: &str) -> Option<PathBuf> {
    let candidates = match std::env::var_os("QINGJIAN_DATA_DIR") {
        Some(dir) => vec![PathBuf::from(dir).join(name)],
        None => vec![
            PathBuf::from("/usr/share/qingjian").join(name),
            PathBuf::from("/usr/lib/qingjian").join(name),
        ],
    };
    candidates.into_iter().find(|path| path.is_file())
}

fn xdg_config_home() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home_dir().join(".config"))
}

fn xdg_data_home() -> PathBuf {
    std::env::var_os("XDG_DATA_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home_dir().join(".local").join("share"))
}

fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_default()
}
