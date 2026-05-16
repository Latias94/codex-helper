#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    Zh,
    En,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TextKey(&'static str);

impl TextKey {
    pub(crate) const fn new(id: &'static str) -> Self {
        Self(id)
    }

    pub(crate) const fn id(self) -> &'static str {
        self.0
    }
}

macro_rules! define_messages {
    ($($name:ident => { zh: $zh:literal, en: $en:literal },)*) => {
        pub(crate) mod msg {
            use super::TextKey;

            $(
                pub(crate) const $name: TextKey = TextKey::new(stringify!($name));
            )*
        }

        static ZH_MESSAGES: &[(&str, &str)] = &[
            $((stringify!($name), $zh),)*
        ];

        static EN_MESSAGES: &[(&str, &str)] = &[
            $((stringify!($name), $en),)*
        ];

        #[cfg(test)]
        fn all_message_ids() -> &'static [&'static str] {
            &[$(stringify!($name),)*]
        }
    };
}

define_messages! {
    LANGUAGE_LABEL => { zh: "语言：", en: "language: " },
    LANGUAGE_SAVED => { zh: "（已保存）", en: " (saved)" },
    LANGUAGE_SAVE_FAILED_PREFIX => { zh: "（保存失败：", en: " (save failed: " },
    LANGUAGE_SAVE_FAILED_SUFFIX => { zh: "）", en: ")" },
    LANGUAGE_TOGGLE_HINT => { zh: "  (按 L 切换，并落盘到 ui.language)", en: "  (press L to cycle and persist to ui.language)" },

    LANGUAGE_NAME_ZH => { zh: "中文", en: "Chinese" },
    LANGUAGE_NAME_EN => { zh: "English", en: "English" },

    PAGE_DASHBOARD => { zh: "1 总览", en: "1 Dashboard" },
    PAGE_ROUTING => { zh: "2 路由", en: "2 Routing" },
    PAGE_STATIONS => { zh: "2 站点", en: "2 Stations" },
    PAGE_SESSIONS => { zh: "3 会话", en: "3 Sessions" },
    PAGE_REQUESTS => { zh: "4 请求", en: "4 Requests" },
    PAGE_STATS => { zh: "5 提供商", en: "5 Providers" },
    PAGE_SETTINGS => { zh: "6 设置", en: "6 Settings" },
    PAGE_HISTORY => { zh: "7 历史", en: "7 History" },
    PAGE_RECENT => { zh: "8 最近", en: "8 Recent" },

    FOCUS_SESSIONS => { zh: "会话", en: "Sessions" },
    FOCUS_REQUESTS => { zh: "请求", en: "Requests" },
    FOCUS_ROUTING => { zh: "路由", en: "Routing" },
    FOCUS_STATIONS => { zh: "站点", en: "Stations" },
    FOCUS_LABEL => { zh: "焦点：", en: "focus: " },

    STATUS_ACTIVE_SHORT => { zh: "活跃 ", en: "active " },
    STATUS_ERRORS_SHORT => { zh: "错误(80) ", en: "errors(80) " },
    STATUS_CURRENT_SHORT => { zh: "当前 ", en: "cur " },
    STATUS_HEALTH_CHECK_SHORT => { zh: "健康检查 ", en: "hc " },
    STATUS_OVERRIDES_SHORT => { zh: "覆盖(M/E/C/T) ", en: "overrides(M/E/C/T) " },
    STATUS_OVERRIDES_ROUTE_SHORT => { zh: "覆盖(M/E/R/T) ", en: "overrides(M/E/R/T) " },
    STATUS_GLOBAL_STATION_OVERRIDE_SHORT => { zh: "覆盖(全局站点) ", en: "override(global station) " },
    STATUS_GLOBAL_ROUTE_TARGET_SHORT => { zh: "覆盖(全局路由) ", en: "override(global route) " },
    STATUS_UPDATED_SHORT => { zh: "刷新 ", en: "updated " },
    STATUS_ACTIVE_TINY => { zh: "活 ", en: "act " },
    STATUS_ERRORS_TINY => { zh: "错 ", en: "err " },
    STATUS_UPDATED_TINY => { zh: "刷 ", en: "upd " },

    OVERLAY_SESSION_PROVIDER_OVERRIDE => { zh: "会话 Provider 覆盖", en: "Session provider override" },
    OVERLAY_GLOBAL_STATION_PIN => { zh: "全局站点 pin", en: "Global station pin" },
    OVERLAY_HELP_TITLE => { zh: "帮助", en: "Help" },
    OVERLAY_STATION_DETAILS => { zh: "站点详情", en: "Station details" },
    OVERLAY_SESSION_TRANSCRIPT => { zh: "会话对话记录", en: "Session transcript" },
    OVERLAY_SET_SESSION_MODEL => { zh: "设置 Session Model", en: "Set session model" },
    OVERLAY_SET_SERVICE_TIER => { zh: "设置 Fast / Service Tier", en: "Set Fast / Service Tier" },
    OVERLAY_INPUT_SESSION_MODEL => { zh: "输入自定义 Session Model", en: "Enter custom session model" },
    OVERLAY_INPUT_SERVICE_TIER => { zh: "输入自定义 Service Tier", en: "Enter custom service tier" },
    OVERLAY_MANAGE_RUNTIME_PROFILE => { zh: "管理运行时默认 Profile", en: "Manage runtime default profile" },
    OVERLAY_MANAGE_CONFIGURED_PROFILE => { zh: "管理配置默认 Profile", en: "Manage configured default profile" },
    OVERLAY_MANAGE_SESSION_PROFILE => { zh: "管理 Session Profile Binding", en: "Manage session profile binding" },

    FOOTER_DASHBOARD => { zh: "1-8 页面  q 退出  L 语言  Tab 焦点  ↑/↓ 移动  b/M/f 覆盖  p/P 站点  ? 帮助", en: "1-8 pages  q quit  L language  Tab focus  ↑/↓ move  b/M/f overrides  p/P station  ? help" },
    FOOTER_DASHBOARD_ROUTE_GRAPH => { zh: "1-8 页面  q 退出  L 语言  Tab 焦点  ↑/↓ 移动  b/M/f 覆盖  p/P 路由  ? 帮助", en: "1-8 pages  q quit  L language  Tab focus  ↑/↓ move  b/M/f overrides  p/P route  ? help" },
    FOOTER_ROUTING => { zh: "1-8 页面  q 退出  L 语言  ↑/↓ provider  r/Enter 编辑  g 刷新余额  ? 帮助", en: "1-8 pages  q quit  L language  ↑/↓ provider  r/Enter edit  g refresh balances  ? help" },
    FOOTER_STATIONS => { zh: "1-8 页面  q 退出  L 语言  ↑/↓ 选择  Enter pin  i 详情  h/H 检查  ? 帮助", en: "1-8 pages  q quit  L language  ↑/↓ select  Enter pin  i details  h/H check  ? help" },
    FOOTER_REQUESTS => { zh: "1-8 页面  q 退出  L 语言  ↑/↓ 选择  e 错误  s scope  o/h 跳转  ? 帮助", en: "1-8 pages  q quit  L language  ↑/↓ select  e errors  s scope  o/h navigate  ? help" },
    FOOTER_SESSIONS => { zh: "1-8 页面  q 退出  L 语言  ↑/↓ 选择  b/M/f 覆盖  a/e/v 筛选  t 记录  ? 帮助", en: "1-8 pages  q quit  L language  ↑/↓ select  b/M/f overrides  a/e/v filters  t transcript  ? help" },
    FOOTER_STATS => { zh: "1-8 页面  q 退出  L 语言  Tab 站点/提供商  ↑/↓ 选择  a 关注  g 刷新余额  PgUp/PgDn 详情  ? 帮助", en: "1-8 pages  q quit  L language  Tab station/provider  ↑/↓ select  a attention  g refresh balances  PgUp/PgDn detail  ? help" },
    FOOTER_SETTINGS_CODEX => { zh: "1-8 页面  q 退出  L 语言  p/P profile  R 重载  O 导入 ~/.codex  ? 帮助", en: "1-8 pages  q quit  L language  p/P profile  R reload  O import ~/.codex  ? help" },
    FOOTER_SETTINGS_OTHER => { zh: "1-8 页面  q 退出  L 语言  p/P profile  R 重载  ? 帮助", en: "1-8 pages  q quit  L language  p/P profile  R reload  ? help" },
    FOOTER_HISTORY => { zh: "1-8 页面  q 退出  L 语言  ↑/↓ 选择  r 刷新  t/Enter 记录  s/f 跳转  ? 帮助", en: "1-8 pages  q quit  L language  ↑/↓ select  r refresh  t/Enter transcript  s/f navigate  ? help" },
    FOOTER_RECENT => { zh: "1-8 页面  q 退出  L 语言  ↑/↓ 选择  [] 时间  Enter/y 复制  t 记录  ? 帮助", en: "1-8 pages  q quit  L language  ↑/↓ select  [] window  Enter/y copy  t transcript  ? help" },
    FOOTER_HELP => { zh: "Esc 关闭帮助  L 语言", en: "Esc close help  L language" },
    FOOTER_SELECT_APPLY => { zh: "↑/↓ 选择  Enter 应用  Esc 取消", en: "↑/↓ select  Enter apply  Esc cancel" },
    FOOTER_MODEL_MENU => { zh: "↑/↓ 选择 model  Enter 应用  Esc 取消", en: "↑/↓ select model  Enter apply  Esc cancel" },
    FOOTER_MODEL_INPUT => { zh: "输入 model  Enter 应用  Esc 返回菜单  Backspace 删除  Delete/Ctrl+U 清空", en: "type model  Enter apply  Esc back to menu  Backspace delete  Delete/Ctrl+U clear" },
    FOOTER_SERVICE_TIER_MENU => { zh: "↑/↓ 选择 service tier  Enter 应用  Esc 取消", en: "↑/↓ select service tier  Enter apply  Esc cancel" },
    FOOTER_SERVICE_TIER_INPUT => { zh: "输入 service_tier  Enter 应用  Esc 返回菜单  Backspace 删除  Delete/Ctrl+U 清空", en: "type service_tier  Enter apply  Esc back to menu  Backspace delete  Delete/Ctrl+U clear" },
    FOOTER_PROFILE_SESSION => { zh: "↑/↓ 选择 profile 操作  Enter 应用/清除绑定  Esc 取消", en: "↑/↓ select profile action  Enter apply/clear binding  Esc cancel" },
    FOOTER_PROFILE_RUNTIME => { zh: "↑/↓ 选择运行时默认 profile  Enter 应用/清除覆盖  Esc 取消", en: "↑/↓ select runtime default profile  Enter apply/clear override  Esc cancel" },
    FOOTER_PROFILE_CONFIGURED => { zh: "↑/↓ 选择配置默认 profile  Enter 应用/清除默认值  Esc 取消", en: "↑/↓ select configured default profile  Enter apply/clear default  Esc cancel" },
    FOOTER_ROUTING_MENU => { zh: "↑/↓ 选择  Enter pin  a 顺序  e/f/s 策略  1/2/0 billing  []/u/d 重排  g 刷新  Esc 关闭", en: "↑/↓ select  Enter pin  a order  e/f/s policy  1/2/0 billing  []/u/d reorder  g refresh  Esc close" },
    FOOTER_STATION_INFO => { zh: "↑/↓ 滚动  PgUp/PgDn 翻页  Esc 关闭  L 语言", en: "↑/↓ scroll  PgUp/PgDn page  Esc close  L language" },
    FOOTER_SESSION_TRANSCRIPT => { zh: "↑/↓ 滚动  PgUp/PgDn 翻页  g/G 顶/底  A 全量/尾部  y 复制  t/Esc 关闭  L 语言", en: "↑/↓ scroll  PgUp/PgDn page  g/G top/bottom  A all/tail  y copy  t/Esc close  L language" },

    DECLARED_LABEL => { zh: "声明：", en: "declared:" },
    RESOLVED_LABEL => { zh: "生效：", en: "resolved:" },
    RESOLVE_FAILED_LABEL => { zh: "解析失败：", en: "resolve failed:" },
    SESSION_LABEL => { zh: "会话：", en: "session: " },
    PINNED_LABEL => { zh: "固定：", en: "pinned: " },
    KEYS_LABEL => { zh: "按键：", en: "keys: " },
    ALIAS_LABEL => { zh: "别名：", en: "alias: " },
    STATUS_LABEL => { zh: "状态：", en: "status: " },
    ENABLED_LABEL => { zh: "启用", en: "enabled" },
    DISABLED_LABEL => { zh: "禁用", en: "disabled" },
    RUNTIME_HEALTH_TITLE => { zh: "运行态（可用性/体验）", en: "Runtime health / experience" },
    OK_PREFIX => { zh: "成功 ", en: "ok " },
    UPSTREAMS_TITLE => { zh: "上游（Providers）", en: "Upstreams (providers)" },
    NONE_PARENS => { zh: "（无）", en: "(none)" },
    NOT_CHECKED => { zh: "未检查", en: "not checked" },
    MODELS_ALL => { zh: "模型：全部", en: "models: all" },
    NO_STATION_SELECTED => { zh: "未选中任何站点。", en: "No station selected." },
    NO_TRANSCRIPT_MESSAGES => { zh: "未找到可展示的对话消息（可能该会话不在 ~/.codex/sessions，或格式发生变化）。", en: "No displayable transcript messages were found; the session may be outside ~/.codex/sessions or the format changed." },
    CURRENT_INPUT_LABEL => { zh: "当前输入: ", en: "current: " },
    SESSION_MODEL_HINT_LABEL => { zh: "当前会话模型: ", en: "session hint: " },
    SESSION_TIER_HINT_LABEL => { zh: "当前会话 tier: ", en: "session hint: " },
    CLEAR_MODEL_OVERRIDE => { zh: "Clear (清除会话 model 覆盖)", en: "Clear (remove session model override)" },
    RESTORE_DEFAULT_ROUTING => { zh: "恢复为 request / binding / runtime 的默认路由", en: "Restore the request / binding / runtime default route" },
    APPLY_SESSION_MODEL_OVERRIDE => { zh: "应用为当前会话的 model override", en: "Apply as the current session model override" },
    CUSTOM_MODEL => { zh: "Custom...（输入任意 model）", en: "Custom... (enter any model)" },
    CUSTOM_MODEL_HELP => { zh: "打开输入框，手动填写 model override", en: "Open an input box to enter a model override" },
    MODEL_INPUT_HELP => { zh: "输入任意 model 名称。Enter 应用，Esc 返回菜单，Backspace 删除，Delete / Ctrl+U 清空。空值会清除会话 model 覆盖。", en: "Enter any model name. Enter applies, Esc returns to the menu, Backspace deletes, Delete / Ctrl+U clears. Empty input clears the session model override." },
    CLEAR_SERVICE_TIER_OVERRIDE => { zh: "移除当前会话的 service tier 覆盖", en: "Remove the current session service tier override" },
    USE_DEFAULT_SERVICE_TIER => { zh: "显式使用 default", en: "Explicitly use default" },
    USE_PRIORITY_SERVICE_TIER => { zh: "通常可视为 fast mode", en: "Usually maps to fast mode" },
    USE_FLEX_SERVICE_TIER => { zh: "显式使用 flex", en: "Explicitly use flex" },
    CUSTOM_SERVICE_TIER => { zh: "Custom...（输入任意 service_tier）", en: "Custom... (enter any service_tier)" },
    CUSTOM_SERVICE_TIER_HELP => { zh: "打开输入框，手动填写 service_tier override", en: "Open an input box to enter a service_tier override" },
    SERVICE_TIER_INPUT_HELP => { zh: "输入任意 service_tier。Enter 应用，Esc 返回菜单，Backspace 删除，Delete / Ctrl+U 清空。空值会清除会话 service_tier 覆盖。", en: "Enter any service_tier. Enter applies, Esc returns to the menu, Backspace deletes, Delete / Ctrl+U clears. Empty input clears the session service_tier override." },
    CLEAR_RUNTIME_PROFILE => { zh: "Clear runtime override（回退到配置默认 profile）", en: "Clear runtime override (fall back to configured default profile)" },
    CLEAR_RUNTIME_PROFILE_HELP => { zh: "只清理运行时 default_profile 覆盖；保留配置文件里的 default_profile", en: "Only clears the runtime default_profile override; keeps the configured default_profile" },
    CLEAR_CONFIGURED_PROFILE => { zh: "Clear configured default（移除配置默认 profile）", en: "Clear configured default (remove configured default profile)" },
    CLEAR_CONFIGURED_PROFILE_HELP => { zh: "会修改并重载代理配置；新的会话将不再继承配置级 default_profile", en: "Edits and reloads proxy config; new sessions stop inheriting the configured default_profile" },
    CLEAR_SESSION_PROFILE_BINDING => { zh: "Clear binding（移除会话已存储的 profile 绑定）", en: "Clear binding (remove stored session profile binding)" },
    CLEAR_SESSION_PROFILE_BINDING_HELP => { zh: "只清理 profile binding；保留当前会话的 model / effort / provider / service_tier 覆盖", en: "Only clears the profile binding; keeps the current session model / effort / provider / service_tier overrides" },

    HISTORY_TITLE => { zh: "历史会话 (Codex)", en: "History sessions (Codex)" },
    RECENT_TITLE => { zh: "最近会话 (Codex)", en: "Recent sessions (Codex)" },
    DETAILS_TITLE => { zh: "详情", en: "Details" },
    FIRST_USER_MESSAGE => { zh: "首条用户消息", en: "First user message" },
    BULLET_DASH => { zh: "  -", en: "  -" },
    HISTORY_EMPTY => { zh: "未找到历史会话。按 r 刷新；或确认 ~/.codex/sessions 存在。", en: "No history sessions found. Press r to refresh, or check that ~/.codex/sessions exists." },
    HISTORY_KEYS => { zh: "  ↑/↓ 选择  r 刷新  t/Enter 打开对话记录  s 打开到 Sessions  f 打开到 Requests", en: "  ↑/↓ select  r refresh  t/Enter transcript  s open Sessions  f open Requests" },
    HISTORY_EXTERNAL_NO_TRANSCRIPT => { zh: "  当前条目来自外部桥接，没有本地 transcript 文件。", en: "  This entry came from an external bridge and has no local transcript file." },
    RECENT_EMPTY => { zh: "未加载最近会话。按 r 刷新；或确认 ~/.codex/sessions 存在。", en: "Recent sessions are not loaded. Press r to refresh, or check that ~/.codex/sessions exists." },
    RECENT_KEYS_PRIMARY => { zh: "  Enter 复制条目  y 复制可见列表  t 打开 transcript", en: "  Enter copy entry  y copy visible list  t open transcript" },
    RECENT_KEYS_NAV => { zh: "  s 打开到 Sessions  f 打开到 Requests  h 打开到 History", en: "  s open Sessions  f open Requests  h open History" },
    NO_SELECTION => { zh: "未选中任何条目。", en: "No selection." },

    SETTINGS_TITLE => { zh: "设置", en: "Settings" },
    RUNTIME_OVERVIEW_TITLE => { zh: "运行态概览", en: "Runtime overview" },
    BALANCE_OVERVIEW_TITLE => { zh: "余额 / 配额概览", en: "Balance / quota overview" },
    PRICING_CATALOG_TITLE => { zh: "价格目录", en: "Pricing catalog" },
    TUI_OPTIONS_TITLE => { zh: "TUI 选项", en: "TUI options" },
    REFRESH_LABEL => { zh: "刷新间隔：", en: "refresh: " },
    WINDOW_SAMPLES_LABEL => { zh: "窗口采样：", en: "window samples: " },
    PROFILE_CONTROL_TITLE => { zh: "Profile 控制", en: "Profile control" },
    CONFIGURED_DEFAULT_LABEL => { zh: "配置默认：", en: "configured default: " },
    PRESS_P_MANAGE => { zh: "  (按 p 管理)", en: "  (press p to manage)" },
    RUNTIME_OVERRIDE_LABEL => { zh: "运行时覆盖：", en: "runtime override: " },
    PRESS_CAPITAL_P_MANAGE => { zh: "  (按 P 管理)", en: "  (press P to manage)" },
    EFFECTIVE_DEFAULT_LABEL => { zh: "当前生效：", en: "effective default: " },
    NO_PROFILES => { zh: "<no profiles>", en: "<no profiles>" },
    AVAILABLE_PROFILES_LABEL => { zh: "可用 profile：", en: "available profiles: " },
    HEALTH_CHECK_TITLE => { zh: "Health Check", en: "Health Check" },
    PATHS_TITLE => { zh: "路径", en: "Paths" },
    RUNTIME_CONFIG_TITLE => { zh: "运行态配置", en: "Runtime config" },
    PRESS_R_RELOAD => { zh: "  (按 R 立即重载)", en: "  (press R to reload)" },
    COMMON_KEYS_TITLE => { zh: "常用快捷键", en: "Common keys" },
    COMMON_KEYS_CODEX => { zh: "  1-8 切页  ? 帮助  q 退出  L 语言  (Stations: i 详情  Stats: y 导出/复制  Settings: R 重载配置  O 覆盖导入(二次确认))", en: "  1-8 pages  ? help  q quit  L language  (Stations: i details  Stats: y export/copy  Settings: R reload  O overwrite(confirm))" },
    COMMON_KEYS_OTHER => { zh: "  1-8 切页  ? 帮助  q 退出  L 语言  (Stations: i 详情  Stats: y 导出/复制)", en: "  1-8 pages  ? help  q quit  L language  (Stations: i details  Stats: y export/copy)" },

    CONFIRM_OVERWRITE => { zh: "再次按 O 确认覆盖导入（3s 内）", en: "Press O again to confirm overwrite (within 3s)" },
    CONFIG_RELOADED => { zh: "已重载配置（", en: "Config reloaded (" },
    CONFIG_CHANGED => { zh: "检测到变更", en: "changed" },
    CONFIG_NO_CHANGE => { zh: "无变更", en: "no change" },
    CONFIG_RELOADED_SUFFIX => { zh: "）", en: ")" },
    HISTORY_REFRESHING => { zh: "history: 刷新中…", en: "history: refreshing…" },
    RECENT_REFRESHING => { zh: "recent: 刷新中…", en: "recent: refreshing…" },
    PROFILE_NO_OPTIONS => { zh: "profile: 当前服务没有可用 profile", en: "profile: no profiles available for this service" },
    DEFAULT_PROFILE_NO_OPTIONS => { zh: "default profile: 当前服务没有可用 profile", en: "default profile: no profiles available for this service" },
    DEFAULT_PROFILE_MANAGE_CONFIGURED => { zh: "default profile: 管理配置默认值", en: "default profile: manage configured default" },
    RUNTIME_DEFAULT_PROFILE_NO_OPTIONS => { zh: "runtime default profile: 当前服务没有可用 profile", en: "runtime default profile: no profiles available for this service" },
    RUNTIME_DEFAULT_PROFILE_MANAGE => { zh: "runtime default profile: 管理运行时默认值", en: "runtime default profile: manage runtime default" },
    MODEL_NO_CATALOG => { zh: "model: 当前服务没有可用模型目录", en: "model: no model catalog available for this service" },
    RECENT_COPIED_SELECTED => { zh: "recent: 已复制选中条目", en: "recent: copied selected entry" },
    RECENT_SESSION_NOT_OBSERVED => { zh: "sessions: 当前 runtime 未观测到这个 recent session", en: "sessions: this recent session is not observed by the current runtime" },
    RECENT_COPIED_VISIBLE => { zh: "recent: 已复制可见列表", en: "recent: copied visible list" },
    SESSION_NOT_OBSERVED => { zh: "sessions: 当前 runtime 未观测到这个 session", en: "sessions: this session is not observed by the current runtime" },
    HISTORY_NO_TRANSCRIPT_FILE => { zh: "history: 当前选中项没有本地 transcript 文件", en: "history: selected item has no local transcript file" },

    ROUTING_ACTION_PROVIDER_DETAILS => { zh: "  i            Provider 详情（可滚动）", en: "  i            provider details (scrollable)" },
}

fn lookup(table: &'static [(&'static str, &'static str)], key: TextKey) -> Option<&'static str> {
    table
        .iter()
        .find_map(|(id, value)| (*id == key.id()).then_some(*value))
}

pub(crate) fn text(lang: Language, key: TextKey) -> &'static str {
    match lang {
        Language::Zh => lookup(ZH_MESSAGES, key)
            .or_else(|| lookup(EN_MESSAGES, key))
            .unwrap_or_else(|| key.id()),
        Language::En => lookup(EN_MESSAGES, key).unwrap_or_else(|| key.id()),
    }
}

pub(crate) fn label(lang: Language, en: &'static str) -> &'static str {
    match lang {
        Language::En => en,
        Language::Zh => zh_label(en).unwrap_or(en),
    }
}

fn zh_label(en: &'static str) -> Option<&'static str> {
    Some(match en {
        "Actions" => "操作",
        "A" => "活",
        "Age" => "耗时",
        "<auto>" => "<自动>",
        "<clear>" => "<清除>",
        "<configured fallback>" => "<配置回退>",
        "<none>" => "<无>",
        "Balance / quota" => "余额 / 配额",
        "Balance/Quota" => "余额/配额",
        "Cache & speed" => "缓存 / 速度",
        "Clear override" => "清除覆盖",
        "Clear (use request value)" => "清除（使用请求值）",
        "Clear (use request/binding value)" => "清除（使用请求/绑定值）",
        "CNew" => "新缓存",
        "CRead" => "读缓存",
        "CWD" => "目录",
        "Details" => "详情",
        "Dur" => "耗时",
        "Effective route" => "生效路由",
        "First user message" => "第一条用户消息",
        "Health" => "健康",
        "In" => "输入",
        "Keys" => "按键",
        "Last" => "最近",
        "Live health" => "实时健康",
        "Lvl" => "级别",
        "Model" => "模型",
        "Name" => "名称",
        "No data in this window." => "当前窗口没有数据。",
        "No requests match the current filters." => "没有请求匹配当前筛选。",
        "No sessions match the current filters." => "没有会话匹配当前筛选。",
        "No stations available." => "没有可用站点。",
        "Observed route" => "观测路由",
        "On" => "启用",
        "Out" => "输出",
        "Path" => "路径",
        "Provider" => "提供商",
        "Provider routing" => "Provider 路由",
        "Recent sample" => "最近样本",
        "Requests" => "请求",
        "Requests page" => "请求页",
        "Retry / route chain" => "重试 / 路由链",
        "Routing" => "路由",
        "Routing page" => "路由页",
        "Route" => "路由",
        "Session" => "会话",
        "Session details" => "会话详情",
        "Sessions" => "会话",
        "Sessions page" => "会话页",
        "Set reasoning effort" => "设置推理强度",
        "Spend & tokens" => "花费 / Token",
        "St" => "状态",
        "Station details" => "站点详情",
        "Stations" => "站点",
        "Stats page" => "提供商页",
        "TTFB" => "首包",
        "Tok" => "Tok",
        "Tokens / day" => "每日 Token",
        "Tokens / day (window, zero-filled)" => "每日 Token（窗口，补零）",
        "Total" => "总耗时",
        "Up" => "上游",
        "Upstreams" => "上游",
        "Updated" => "更新时间",
        "ΣTok" => "总Tok",
        "active" => "活跃",
        "activity" => "活动",
        "active_only" => "仅活跃",
        "age" => "耗时",
        "alias" => "别名",
        "all_exhausted" => "全部耗尽",
        "all" => "全部",
        "all requests" => "全部请求",
        "allow" => "允许",
        "attempts" => "尝试",
        "assistant" => "助手",
        "auth" => "认证",
        "auto" => "自动",
        "auto(level fallback)" => "自动（按级别回退）",
        "auto(single-level fallback)" => "自动（同级回退）",
        "attention only" => "仅关注项",
        "avg" => "平均",
        "balance" => "余额",
        "balance refresh already requested" => "余额刷新已在进行中",
        "balance lookup failed" => "余额查询失败",
        "balance refresh started" => "余额刷新已开始",
        "balance/quota" => "余额/配额",
        "breaker_open_blocks_pin" => "熔断阻断 pin",
        "branch" => "分支",
        "budget" => "预算",
        "binding" => "绑定",
        "cache" => "缓存",
        "cache read/create" => "缓存 读/新建",
        "cache hit" => "缓存命中",
        "cache hit rate" => "缓存命中率",
        "Hit%" => "命中率",
        "client" => "客户端",
        "client(last)" => "最近客户端",
        "clipboard failed" => "剪贴板失败",
        "config file" => "配置文件",
        "configured default profile" => "配置默认 profile",
        "configured default profile apply failed" => "配置默认 profile 应用失败",
        "configured default profile refresh failed" => "配置默认 profile 刷新失败",
        "context" => "上下文",
        "control" => "控制",
        "cooldown" => "冷却",
        "copy" => "复制",
        "cost" => "成本",
        "cost_parts" => "成本拆分",
        "coverage" => "覆盖",
        "coverage warning" => "覆盖提醒",
        "cancel" => "取消",
        "canceled" => "已取消",
        "cwd" => "目录",
        "dashboard: no request selected" => "dashboard: 未选择请求",
        "dashboard: no session selected" => "dashboard: 未选择会话",
        "dashboard: selected request has no session id" => "dashboard: 所选请求没有 session id",
        "dashboard: selected row has no session id" => "dashboard: 所选行没有 session id",
        "disabled" => "已禁用",
        "done" => "完成",
        "effort" => "推理强度",
        "effort override" => "推理强度覆盖",
        "effort override cleared" => "推理强度覆盖已清除",
        "effort set" => "推理强度已设置",
        "enabled" => "启用",
        "err" => "错误",
        "error" => "错误",
        "errors" => "错误",
        "errors_only" => "仅错误",
        "estimated" => "估算",
        "exact" => "精确",
        "exh" => "耗尽",
        "exhausted" => "已耗尽",
        "explain" => "说明",
        "explicit session focus" => "显式会话聚焦",
        "failed to load transcript" => "加载 transcript 失败",
        "follow selected session" => "跟随选中会话",
        "focus" => "焦点",
        "focused from" => "聚焦来源",
        "generation" => "生成",
        "global" => "全局",
        "global override" => "全局覆盖",
        "global station pin" => "全局站点 pin",
        "global route target" => "全局 route target",
        "global_station" => "全局站点",
        "health_check" => "健康检查",
        "health" => "健康",
        "health check already running" => "健康检查已在运行",
        "health check cancel requested" => "已请求取消健康检查",
        "health check load failed" => "健康检查加载失败",
        "health check not running" => "健康检查未运行",
        "health check queued" => "健康检查已排队",
        "history: failed to prepare request focus" => "history: 准备请求聚焦失败",
        "history: failed to prepare session focus" => "history: 准备会话聚焦失败",
        "history: focused session" => "history: 已聚焦会话",
        "history: no selection" => "history: 未选择条目",
        "history: resolve session file failed" => "history: 解析会话文件失败",
        "host-local enriched" => "本机 transcript 增强",
        "home" => "主目录",
        "identity" => "身份",
        "in/out" => "输入/输出",
        "last_response" => "最近响应",
        "lazy" => "不降级",
        "lazy reset" => "不降级耗尽",
        "lb" => "负载",
        "left" => "剩余",
        "limit" => "上限",
        "linked under ~/.codex/sessions" => "已链接到 ~/.codex/sessions",
        "loaded" => "已加载",
        "loaded days with data" => "有数据的已加载天数",
        "loaded total req" => "已加载请求总数",
        "local transcript" => "本地对话记录",
        "logs" => "日志",
        "lookup_failed" => "查询失败",
        "map" => "映射",
        "messages" => "消息",
        "meta" => "元数据",
        "method" => "方法",
        "missing_pinned_station" => "缺少固定站点",
        "mode" => "模式",
        "model" => "模型",
        "model override" => "模型覆盖",
        "models" => "模型",
        "mtime" => "修改时间",
        "name" => "名称",
        "nested route graph: edit route nodes in TOML for grouped reorder" => {
            "嵌套路由图：请在 TOML 中编辑路由节点来分组排序"
        }
        "no" => "否",
        "no balance/quota data" => "没有余额/配额数据",
        "no Codex session file found for this session id" => {
            "没有找到该 session id 对应的 Codex 会话文件"
        }
        "no price rows" => "没有价格行",
        "no session selected" => "未选择会话",
        "no transcript file loaded" => "未加载对话记录文件",
        "no host-local transcript detected" => "未检测到本机 transcript",
        "no stored binding or session override" => "没有已存储绑定或会话覆盖",
        "no_upstreams" => "没有可路由上游",
        "now" => "当前",
        "observed bridge" => "观测桥接",
        "observed only" => "仅运行时观测",
        "ok" => "成功",
        "on" => "开",
        "on_exhausted" => "耗尽策略",
        "off" => "关",
        "order" => "顺序",
        "order_rule" => "排序规则",
        "out_tok/s" => "输出 tok/s",
        "overwrite-from-codex is only supported for Codex service" => {
            "overwrite-from-codex 仅支持 Codex 服务"
        }
        "override" => "覆盖",
        "overrides_only" => "仅覆盖",
        "partial" => "部分",
        "partial_exhausted" => "部分耗尽",
        "path" => "路径",
        "paygo" => "按量",
        "pinned" => "固定",
        "policy" => "策略",
        "prefer_tags" => "偏好标签",
        "profile applied" => "profile 已应用",
        "profile apply failed" => "profile 应用失败",
        "profile binding cleared" => "profile 绑定已清除",
        "profile default" => "profile 默认值",
        "provider" => "提供商",
        "provider enable failed" => "provider 启用失败",
        "provider tag failed" => "provider 标签失败",
        "providers" => "提供商",
        "provider is referenced by the route graph but missing from catalog" => {
            "路由图引用了该 provider，但目录中缺失"
        }
        "quota" => "配额",
        "raw_cwd" => "原始 cwd",
        "read/create" => "读取/创建",
        "recent filter" => "recent 筛选",
        "recent window" => "recent 窗口",
        "recent: no local transcript file found for this session" => {
            "recent: 没有找到该会话的本地对话记录文件"
        }
        "recent: no selection" => "recent: 未选择条目",
        "recent: nothing to copy" => "recent: 没有可复制内容",
        "recent: resolve session file failed" => "recent: 解析会话文件失败",
        "requests" => "请求",
        "requests filter" => "requests 筛选",
        "requests scope" => "requests 范围",
        "requests: cleared explicit session focus" => "requests: 已清除显式会话聚焦",
        "requests: focused session" => "requests: 已聚焦会话",
        "requests: no selection" => "requests: 未选择条目",
        "requests: selected request has no session id" => "requests: 所选请求没有 session id",
        "request payload" => "请求负载",
        "req" => "请求",
        "Rounds" => "轮次",
        "reports" => "报告",
        "resolved policy unavailable" => "未解析到策略",
        "retry" => "重试",
        "retry policy" => "重试策略",
        "root" => "根目录",
        "rounds" => "轮次",
        "rt" => "运行态",
        "route graph" => "路由图",
        "routing" => "路由",
        "routing update failed" => "routing 更新失败",
        "routing: apply failed" => "routing: 应用失败",
        "routing spec not loaded" => "routing 规格未加载",
        "routing: edit persisted policy/order" => "routing: 编辑持久化策略/顺序",
        "routing: edit provider policy/order/tags" => "routing: 编辑 provider 策略/顺序/标签",
        "routing: load failed" => "routing: 加载失败",
        "routing: move failed" => "routing: 移动失败",
        "routing: moved down" => "routing: 已下移",
        "routing: moved up" => "routing: 已上移",
        "routing: ordered-failover" => "routing: ordered-failover",
        "routing: pin failed" => "routing: pin 失败",
        "routing: pinned" => "routing: 已 pin",
        "routing: prefer billing=monthly" => "routing: 优先 billing=monthly",
        "routing: provider details/edit" => "routing: provider 详情/编辑",
        "routing: refresh failed" => "routing: 刷新失败",
        "routing: refreshed" => "routing: 已刷新",
        "run" => "运行",
        "running" => "运行中",
        "runtime default profile" => "运行时默认 profile",
        "runtime default profile apply failed" => "运行时默认 profile 应用失败",
        "runtime default profile refresh failed" => "运行时默认 profile 刷新失败",
        "runtime fallback" => "运行时回退",
        "scope" => "范围",
        "selected session" => "选中会话",
        "session_affinity" => "会话粘性",
        "selected window starts before loaded log data" => "所选窗口早于已加载日志数据",
        "selected session has no session id" => "所选会话没有 session id",
        "set global pin failed" => "设置全局 pin 失败",
        "set global route target failed" => "设置全局 route target 失败",
        "session" => "会话",
        "session-controlled route" => "会话控制路由",
        "session manual overrides reset" => "会话手动覆盖已重置",
        "session override" => "会话覆盖",
        "session overrides already clear" => "会话覆盖已为空",
        "session station override" => "会话站点覆盖",
        "session station override: <clear>" => "会话站点覆盖：<清除>",
        "session station override: <no session>" => "会话站点覆盖：<无会话>",
        "session route target" => "会话 route target",
        "session route target: <no session>" => "会话 route target：<无会话>",
        "route_target" => "路由目标",
        "sessions filter" => "sessions 筛选",
        "sessions filter: reset" => "sessions 筛选：已重置",
        "sessions: focused" => "sessions: 已聚焦",
        "sessions: no session selected" => "sessions: 未选择会话",
        "sessions: selected row has no session id" => "sessions: 所选行没有 session id",
        "service_tier" => "服务层级",
        "service_tier set" => "service_tier 已设置",
        "sid" => "会话 ID",
        "skipped" => "跳过",
        "source" => "来源",
        "stale" => "过期",
        "station" => "站点",
        "station mapping" => "站点映射",
        "status" => "状态",
        "strategy" => "策略",
        "stream" => "流式",
        "success" => "成功",
        "subscription" => "订阅",
        "tags" => "标签",
        "tail" => "尾部",
        "target" => "目标",
        "tier" => "服务层级",
        "not in catalog" => "不在目录中",
        "today" => "今天",
        "today used" => "今日已用",
        "tok" => "tok",
        "tok in/out/rsn" => "tok 输入/输出/推理",
        "tok in/out/rsn/ttl" => "tok 输入/输出/推理/总计",
        "tok/s" => "tok/s",
        "tokens" => "Token",
        "top models by tokens" => "按 Token 排名前列模型",
        "top paths by tokens" => "按 Token 排名前列路径",
        "top status" => "状态排名",
        "total" => "总数",
        "trace" => "追踪",
        "transcript" => "对话记录",
        "file" => "文件",
        "transcript: copied to clipboard" => "transcript: 已复制到剪贴板",
        "transcript: copy failed" => "transcript: 复制失败",
        "transcript: loaded all" => "transcript: 已加载全部",
        "transcript: loaded tail" => "transcript: 已加载尾部",
        "transcript: reload failed" => "transcript: 重新加载失败",
        "ttfb" => "首包",
        "turns" => "轮次",
        "usage" => "用量",
        "updated" => "已更新",
        "upstream" => "上游",
        "upstreams" => "上游",
        "user" => "用户",
        "unknown" => "未知",
        "unlimited" => "不限量",
        "used" => "已用",
        "usd" => "USD",
        "v4 routing owns provider choice; press r to edit routing" => {
            "v4 routing 接管 provider 选择；按 r 编辑路由"
        }
        "v4 routing is global; editing persisted routing" => {
            "v4 routing 是全局配置；正在编辑持久化 routing"
        }
        "window" => "窗口",
        "yes" => "是",
        "pinned target only; breaker_open / empty upstreams block." => {
            "仅固定目标；breaker_open 或空上游会阻断。"
        }
        "known fully exhausted stations are demoted by default; provider-level exceptions only show balance/quota." => {
            "默认降低已知完全耗尽站点的优先级；provider 级例外只展示余额/配额。"
        }
        "known fully exhausted stations are demoted by default unless a provider opts out of routing trust." => {
            "默认降低已知完全耗尽站点的优先级，除非 provider 不信任余额参与路由。"
        }
        "This session keeps its stored binding until another profile or override rewrites it." => {
            "该会话会保留已存储绑定，直到其他 profile 或覆盖写入新值。"
        }
        "Session overrides currently take priority over the bound profile:" => {
            "当前优先生效的是会话覆盖字段："
        }
        "This session is currently pinned by overrides on:" => "该会话当前由这些覆盖字段固定：",
        "Without a stored profile or session override, runtime/global routing explains the effective route." => {
            "没有已存储 profile 或会话覆盖时，生效路由由运行态/全局路由解释。"
        }
        "Effective route comes from request payloads, station defaults, and runtime fallback." => {
            "生效路由来自请求负载、站点默认值和运行时回退。"
        }
        "Effective route comes from request payloads, route graph defaults, route target overrides, and runtime fallback." => {
            "生效路由来自请求负载、路由图默认值、route target 覆盖和运行时回退。"
        }
        _ => return None,
    })
}

pub(crate) fn language_name(lang: Language) -> &'static str {
    match lang {
        Language::Zh => text(Language::Zh, msg::LANGUAGE_NAME_ZH),
        Language::En => text(Language::En, msg::LANGUAGE_NAME_EN),
    }
}

pub(crate) fn next_language(lang: Language) -> Language {
    match lang {
        Language::Zh => Language::En,
        Language::En => Language::Zh,
    }
}

pub(crate) fn storage_code(lang: Language) -> &'static str {
    match lang {
        Language::Zh => "zh-CN",
        Language::En => "en-US",
    }
}

pub(crate) fn format_language_saved(current_lang: Language, selected_lang: Language) -> String {
    format!(
        "{}{}{}",
        text(current_lang, msg::LANGUAGE_LABEL),
        language_name(selected_lang),
        text(current_lang, msg::LANGUAGE_SAVED)
    )
}

pub(crate) fn format_language_save_failed(
    current_lang: Language,
    selected_lang: Language,
    err: &dyn std::fmt::Display,
) -> String {
    format!(
        "{}{}{}{}{}",
        text(current_lang, msg::LANGUAGE_LABEL),
        language_name(selected_lang),
        text(current_lang, msg::LANGUAGE_SAVE_FAILED_PREFIX),
        err,
        text(current_lang, msg::LANGUAGE_SAVE_FAILED_SUFFIX)
    )
}

pub(crate) fn format_history_loaded(lang: Language, count: usize) -> String {
    match lang {
        Language::Zh => format!("history: 已加载 {count} 个会话"),
        Language::En => format!("history: loaded {count} sessions"),
    }
}

pub(crate) fn format_history_load_failed(lang: Language, err: &dyn std::fmt::Display) -> String {
    match lang {
        Language::Zh => format!("history: 加载失败：{err}"),
        Language::En => format!("history: load failed: {err}"),
    }
}

pub(crate) fn format_recent_loaded(lang: Language, count: usize) -> String {
    match lang {
        Language::Zh => format!("recent: 已加载 {count} 个会话"),
        Language::En => format!("recent: loaded {count} sessions"),
    }
}

pub(crate) fn format_recent_load_failed(lang: Language, err: &dyn std::fmt::Display) -> String {
    match lang {
        Language::Zh => format!("recent: 加载失败：{err}"),
        Language::En => format!("recent: load failed: {err}"),
    }
}

pub(crate) fn format_config_reloaded(lang: Language, changed: bool) -> String {
    format!(
        "{}{}{}",
        text(lang, msg::CONFIG_RELOADED),
        if changed {
            text(lang, msg::CONFIG_CHANGED)
        } else {
            text(lang, msg::CONFIG_NO_CHANGE)
        },
        text(lang, msg::CONFIG_RELOADED_SUFFIX)
    )
}

pub fn parse_language(s: &str) -> Option<Language> {
    let s = normalize_language_tag(s)?;
    match s.as_str() {
        "zh" | "zh-cn" | "zh-hans" | "zh-hans-cn" | "cn" | "chinese" | "中文" => {
            Some(Language::Zh)
        }
        "en" | "en-us" | "en-gb" | "english" => Some(Language::En),
        _ if s.starts_with("zh-") => Some(Language::Zh),
        _ if s.starts_with("en-") => Some(Language::En),
        _ => None,
    }
}

fn normalize_language_tag(s: &str) -> Option<String> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let s = s
        .split('.')
        .next()
        .unwrap_or(s)
        .replace('_', "-")
        .to_ascii_lowercase();
    Some(s)
}

pub fn detect_system_language() -> Language {
    // Best-effort: prefer env vars to avoid platform-specific commands.
    // Common values:
    // - LANG=zh_CN.UTF-8
    // - LC_ALL=zh_CN.UTF-8
    // - LANGUAGE=zh_CN:en_US
    for key in ["LC_ALL", "LC_MESSAGES", "LANGUAGE", "LANG"] {
        if let Ok(v) = std::env::var(key) {
            for part in v.split(':') {
                if let Some(lang) = parse_language(part) {
                    return lang;
                }
            }
        }
    }
    Language::En
}

pub fn resolve_language_preference(value: Option<&str>) -> Language {
    match value.map(str::trim).filter(|s| !s.is_empty()) {
        Some(value) if value.eq_ignore_ascii_case("auto") => detect_system_language(),
        Some(value) => parse_language(value).unwrap_or_else(detect_system_language),
        None => detect_system_language(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_common_language_tags() {
        assert_eq!(parse_language("zh_CN.UTF-8"), Some(Language::Zh));
        assert_eq!(parse_language("zh-Hans-CN"), Some(Language::Zh));
        assert_eq!(parse_language("en_US"), Some(Language::En));
        assert_eq!(parse_language("en-GB"), Some(Language::En));
        assert_eq!(parse_language("auto"), None);
    }

    #[test]
    fn every_declared_message_is_available_in_all_locales() {
        for id in all_message_ids() {
            let key = TextKey::new(id);
            assert!(
                lookup(EN_MESSAGES, key).is_some(),
                "missing en message: {id}"
            );
            assert!(
                lookup(ZH_MESSAGES, key).is_some(),
                "missing zh message: {id}"
            );
        }
    }

    #[test]
    fn zh_messages_fallback_to_english_then_key() {
        assert_eq!(text(Language::Zh, msg::PAGE_DASHBOARD), "1 总览");
        assert_eq!(
            text(Language::En, TextKey::new("missing.test.key")),
            "missing.test.key"
        );
    }
}
