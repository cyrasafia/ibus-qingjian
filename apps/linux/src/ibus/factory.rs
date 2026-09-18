//! 引擎工厂：daemon 要引擎时造一个，全部指向同一个进程级 Host。

use std::sync::{Arc, Mutex};

use librush::ibus::IBusFactory;

use super::engine::QingjianEngine;
use crate::host::Host;

/// 造 [`QingjianEngine`] 的工厂。
pub struct QingjianFactory {
    /// 进程级状态。
    host: Arc<Mutex<Host>>,
}

impl QingjianFactory {
    /// 拿着共享的 Host 造工厂。
    pub fn new(host: Arc<Mutex<Host>>) -> Self {
        Self { host }
    }
}

impl IBusFactory<QingjianEngine> for QingjianFactory {
    fn create_engine(&mut self, name: String) -> Result<QingjianEngine, String> {
        if name != super::ENGINE_NAME {
            return Err(format!("不认识的引擎名: {name}"));
        }
        Ok(QingjianEngine::new(self.host.clone()))
    }
}
