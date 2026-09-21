//! Host：进程级单例状态——Core Engine、配置、候选会话。
//!
//! ibus 会为每个 input context 各调一次 `CreateEngine`，而 librush 给所有引擎对象用同一个
//! D-Bus 对象路径，多个引擎对象实际共享一个接口实现；反正 Core 的缓冲区也是全进程一份，
//! 会话状态跟着它走（与 mac 壳 `host` 的进程单例同一思路）。
//! 并发由 `Arc<Mutex<Host>>` 保证：方法回调都在 zbus 的 tokio 线程池里进来。

pub mod dictionaries;
pub mod init;
pub mod keys;
pub mod paths;
pub mod session;
pub mod settings;

use qingjian_core::Engine;
use qingjian_platform::{
    DEFAULT_PAGE_KEYS, DictionariesConfig, LayoutMode, Modifiers, Scheme, SwitchKey,
};

use self::session::Session;
use self::settings::Settings;

/// Host 装配错误。
#[derive(Debug, thiserror::Error)]
pub enum HostError {
    /// 随包数据目录里找不到词库，起不来。
    #[error("找不到词库数据（dict.qj / dict.tsv），检查 QINGJIAN_DATA_DIR 或 /usr/share/qingjian")]
    Dictionary,

    /// 词库文件读不了。
    #[error(transparent)]
    DictionaryIo(#[from] qingjian_dictionary::DictionaryError),
}

/// 进程状态。
pub struct Host {
    /// Core 引擎：拼音缓冲、候选、上屏、学习都在里面。
    pub engine: Engine,

    /// 配置（含热重载状态）。
    pub settings: Settings,

    /// 本轮候选与高亮。
    pub session: Session,

    /// 每页候选数（配置 `[general] page_size`）。
    pub page_size: usize,

    /// 候选窗排布（配置 `[general] layout`）：竖排 / 横排，随 LookupTable 下发给面板。
    pub layout: LayoutMode,

    /// 拼音行显示原始键（配置 `[general] raw_preedit`）：开着时 preedit 与辅助行显示敲的键
    /// 而不是解出的全拼；只影响显示，查询与学习不变。
    pub raw_preedit: bool,

    /// 翻页键对（配置 `[general] page_keys`）。
    pub page_keys: (char, char),

    /// 中英切换键（配置 `[shortcut] switch_mode`）：`shift` / `control` 单击切换。
    /// `ctrl+space` 与 GNOME 自己的输入法切换冲突，按不切换处理。
    pub switch_key: SwitchKey,

    /// 内置英文模式总开关（配置 `[general] english_mode`）：关掉后切换键不再切过去。
    english_mode_enabled: bool,

    /// 英文模式给不给候选（配置 `[general] english_candidates`）：关掉就是纯直通。
    pub english_candidates: bool,

    /// 删候选的修饰键（配置 `[shortcut] delete_candidate`，缺省 Shift；与译词键撞了退回缺省）。
    pub delete_keys: Modifiers,

    /// 删候选后给用户看的提示（「已删除用户词「X」」）：随辅助行发在拼音右侧，敲下一键就没。
    pub notice: Option<String>,

    /// 当前客户端的能力位掩码（ibus `SetCapabilities`，位义见
    /// [`CAP_PREEDIT_TEXT`](crate::ibus::engine::CAP_PREEDIT_TEXT)）。
    /// 决定拼音要不要再发一遍辅助行：客户端不认内联 preedit 时 daemon 把 preedit 转给面板画，
    /// 此时再发辅助行，候选窗上就是两行同样的拼音。缺省 0（还没收到能力）按面板兜底算。
    pub client_caps: u32,

    /// 附加词库是按哪份 `[dictionaries]` 装的；开关变了才重新加载。
    pub(super) applied_dictionaries: DictionariesConfig,

    /// 用户导入词库目录最近的文件快照（路径、修改时间、长度）；变了才重新加载。
    pub(super) dictionary_files: Vec<(std::path::PathBuf, Option<std::time::SystemTime>, u64)>,

    /// 切换键单击检测：按下后到抬起之间没插进别的键就是一次单击。
    switch_tap_pending: bool,
}

impl Host {
    /// 组装：建 Engine、读配置文件并应用、装一次附加词库。
    pub fn new() -> Result<Self, HostError> {
        let engine = init::build_engine()?;
        let settings = Settings::load(&paths::config_path());
        let mut host = Self::with_settings(engine, settings);
        // 附加词库启动装一次：基线已在 with_settings 预记为当前配置，apply_config 没装过，这里无条件装
        host.reload_dictionaries();
        Ok(host)
    }

    /// 拿现成 Engine 与配置状态建 Host。
    ///
    /// 附加词库的基线按当前配置预记，首次 apply_config 因而不装配：
    /// 生产路径由 [`Self::new`] 装一次，测试构造（with_engine*）不碰词库目录。
    fn with_settings(engine: Engine, settings: Settings) -> Self {
        let applied_dictionaries = settings.config().dictionaries.clone();
        let mut host = Self {
            engine,
            settings,
            session: Session::default(),
            page_size: 9,
            layout: LayoutMode::default(),
            raw_preedit: false,
            page_keys: DEFAULT_PAGE_KEYS,
            switch_key: SwitchKey::default(),
            english_mode_enabled: true,
            english_candidates: true,
            delete_keys: Modifiers::SHIFT,
            notice: None,
            client_caps: 0,
            applied_dictionaries,
            dictionary_files: Vec::new(),
            switch_tap_pending: false,
        };
        host.apply_config();
        host
    }

    /// 拿现成 Engine 建 Host（测试用）：默认配置、不碰配置文件。
    ///
    /// 默认配置的 `[dictionaries]` 与基线相等，附加词库不装配；注意传非默认 `[dictionaries]`
    /// 的测试会让 apply_config 读写真实的随包与用户词库目录，测试里别这么配。
    #[cfg(test)]
    pub fn with_engine(engine: Engine) -> Self {
        Self::with_settings(
            engine,
            Settings::with_config(qingjian_platform::Config::default()),
        )
    }

    /// 拿现成 Engine 与指定配置建 Host（测试用）：不碰配置文件。
    ///
    /// 隔离边界同 [`Self::with_engine`]：不要在配置里改 `[dictionaries]`。
    #[cfg(test)]
    pub fn with_engine_and_config(engine: Engine, config: qingjian_platform::Config) -> Self {
        Self::with_settings(engine, Settings::with_config(config))
    }

    /// 把当前配置推给 Engine 与会话参数。启动、热重载都走这一条路。
    pub fn apply_config(&mut self) {
        let config = self.settings.config().clone();
        self.engine.set_fuzzy(config.fuzzy);
        self.engine.set_traditional_mode(config.general.traditional);
        self.engine
            .set_full_width_punctuation(config.general.full_width_punctuation);
        self.engine.set_mode_keys(config.shortcut.mode);
        self.engine.set_chinese_first(config.general.chinese_first);
        self.engine
            .set_shift_letter_compose(config.general.shift_letter.compose());
        self.engine.set_learning(config.general.learning);
        self.apply_scheme(config.general.scheme());
        self.page_size = config.general.page_size();
        self.layout = config.general.layout;
        self.raw_preedit = config.general.raw_preedit;
        self.page_keys = config.general.page_keys();
        self.english_mode_enabled = config.general.english_mode;
        self.english_candidates = config.general.english_candidates;
        self.delete_keys = config.shortcut.delete_keys();
        self.switch_key = match config.shortcut.switch_mode {
            // GNOME 把「输入法/非输入法切换」也绑在 Ctrl+Space 上，系统那条路会抢先，按不切换处理
            SwitchKey::CtrlSpace => {
                tracing::warn!(
                    "[shortcut] switch_mode = \"ctrl+space\" 在 ibus 壳下不生效（与系统热键冲突），按不切换处理"
                );
                SwitchKey::None
            }
            other => other,
        };
        // 词库开关变了才重新装配；文件增删走轮询的 poll_dictionaries
        if config.dictionaries != self.applied_dictionaries {
            self.reload_dictionaries();
        }
    }

    /// 拼音侧方案：这个壳只做全拼与双拼，注音 / 形码不在范围。
    fn apply_scheme(&mut self, scheme: Scheme) {
        let shuangpin = match scheme {
            Scheme::Pinyin => None,
            Scheme::Shuangpin(scheme) => Some(scheme),
            _ => {
                tracing::warn!(?scheme, "Linux 壳暂只支持全拼与双拼，按全拼处理");
                None
            }
        };
        self.engine.set_shuangpin(shuangpin);
    }

    /// 切换键按下：开始等一次单击。
    pub fn switch_pressed(&mut self) {
        self.switch_tap_pending = true;
    }

    /// 别的键到了：正在等的单击不算了。
    pub fn break_switch_tap(&mut self) {
        self.switch_tap_pending = false;
    }

    /// 切换键抬起：之前没插进别的键就是一次单击，翻转中英模式。返回切换后的模式。
    /// 切换键配成 `none`、或内置英文模式被 `[general] english_mode` 关掉时不切。
    pub fn switch_released(&mut self) -> Option<bool> {
        if !self.switch_tap_pending {
            return None;
        }
        self.switch_tap_pending = false;
        if self.switch_key == SwitchKey::None || !self.english_mode_enabled {
            return None;
        }
        let english = !self.engine.english_mode();
        self.engine.set_english_mode(english);
        Some(english)
    }

    /// 配置改了（mtime 变化）就整份重读并应用。返回是否重新应用。
    pub fn reload_config_if_changed(&mut self) -> bool {
        if self.settings.reload_if_changed(&paths::config_path()) {
            self.apply_config();
            true
        } else {
            false
        }
    }
}
