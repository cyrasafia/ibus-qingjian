//! 日志：tracing 写 stderr。ibus-daemon 拉起的进程 stderr 进 journal（`journalctl --user`）。
//! 级别优先级：`QINGJIAN_LOG` > `RUST_LOG` > 配置里的 `[general] log_level`（启动时读一次）> info。

use tracing_subscriber::EnvFilter;

/// 装全局 subscriber。
pub fn init(level: &str) {
    let _ = tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            EnvFilter::try_from_env("QINGJIAN_LOG")
                .or_else(|_| EnvFilter::try_from_default_env())
                .or_else(|_| EnvFilter::try_new(level))
                .unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_target(false)
        .try_init();
}
