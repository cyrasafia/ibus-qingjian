//! ibus 接入：librush 的 D-Bus 协议实现与本仓 Core 的对接。

pub mod engine;
pub mod factory;
pub mod keymap;
pub mod present;

/// 组件与引擎的 D-Bus 名：进程内 request 的 well-known 名必须与组件 XML 的 `<name>` 一致，
/// ibus-daemon 靠 NameOwnerChanged 把拉起的进程绑到组件上。
pub const BUS_NAME: &str = "app.qingjian.ibus";

/// 引擎名（组件 XML `<engine><name>`，也是 `ibus engine` 命令用的那个）。
pub const ENGINE_NAME: &str = "qingjian";
