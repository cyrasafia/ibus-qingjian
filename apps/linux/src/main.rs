//! ibus 引擎入口：被 ibus-daemon 按需拉起（组件 XML 的 `<exec>`），连上 ibus 的 D-Bus socket
//! 注册引擎，然后挂着等按键。日志走 stderr（journal 可查），学习数据每 60 秒落一次盘。

mod host;
mod ibus;
mod logging;

use std::process::ExitCode;
use std::sync::{Arc, Mutex};

use crate::host::Host;
use crate::ibus::factory::QingjianFactory;
use crate::ibus::{BUS_NAME, engine::QingjianEngine};

fn main() -> ExitCode {
    // 组件 XML 的 <exec> 带 --ibus（ibus 惯例）；这里不区分参数，唯一入口
    let args = std::env::args().skip(1);
    for arg in args {
        match arg.as_str() {
            "--version" => {
                println!("ibus-engine-qingjian {}", env!("CARGO_PKG_VERSION"));
                return ExitCode::SUCCESS;
            }
            "--ibus" => {}
            "--help" | "-h" => {
                println!(
                    "ibus-engine-qingjian {} —— 青简输入法的 ibus 引擎\n\
                     由 ibus-daemon 按组件 XML 拉起，手动运行只用于诊断。\n\
                     用法：ibus-engine-qingjian [--ibus] [--version]",
                    env!("CARGO_PKG_VERSION")
                );
                return ExitCode::SUCCESS;
            }
            other => {
                eprintln!("不认识的参数: {other}");
                return ExitCode::FAILURE;
            }
        }
    }
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            tracing::error!(%error, "引擎退出");
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

/// 连上 ibus、注册引擎、起定时事务，然后永久挂起。
fn run() -> Result<(), String> {
    // 日志级别先看配置（读不了用 info），再装日志——装配阶段的日志也要按级别来；
    // 运行中改配置里的 log_level 要重启才生效
    let level = qingjian_platform::Config::load(&host::paths::config_path())
        .map(|config| config.general.log_level.key().to_owned())
        .unwrap_or_else(|_| "info".to_owned());
    logging::init(&level);
    let host = Arc::new(Mutex::new(Host::new().map_err(|error| error.to_string())?));
    let addr = librush::ibus::get_ibus_addr().map_err(|error| error.to_string())?;
    tracing::info!(addr = %addr, version = env!("CARGO_PKG_VERSION"), "青简 ibus 引擎启动");
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .map_err(|error| error.to_string())?;
    runtime.block_on(async move {
        // 引擎对象全指向同一个 Host；librush 给所有引擎对象用同一个 D-Bus 路径，
        // 多个 input context 实际共享一个实现，正好匹配进程单例的设计
        let factory = QingjianFactory::new(host.clone());
        let _bus = librush::ibus::IBus::<QingjianEngine, QingjianFactory>::new(
            addr,
            factory,
            BUS_NAME.to_owned(),
        )
        .await
        .map_err(|error| error.to_string())?;
        tracing::info!("已注册到 ibus，等待按键");
        periodic_tasks(host);
        // 永久挂起：daemon 断开会话时进程由 ibus 收回
        std::future::pending::<()>().await;
        #[allow(unreachable_code)]
        Ok::<(), String>(())
    })
}

/// 激活期间的定时事务：每 60 秒把学习数据落盘（有新数据时才是真活），顺带热加载配置。
fn periodic_tasks(host: Arc<Mutex<Host>>) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
        loop {
            interval.tick().await;
            let refreshed = {
                let Ok(mut host) = host.lock() else {
                    continue;
                };
                host.reload_config_if_changed()
            };
            if refreshed {
                tracing::info!("配置已热加载");
            }
            let Ok(mut host) = host.lock() else {
                continue;
            };
            host.engine.flush_learning();
        }
    });
}
