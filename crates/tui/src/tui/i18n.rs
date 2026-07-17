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

    LANGUAGE_NAME_ZH => { zh: "中文", en: "Chinese" },
    LANGUAGE_NAME_EN => { zh: "English", en: "English" },

    PAGE_DASHBOARD => { zh: "1 总览", en: "1 Dashboard" },
    PAGE_ROUTING => { zh: "2 路由", en: "2 Routing" },
    PAGE_SESSIONS => { zh: "3 会话", en: "3 Sessions" },
    PAGE_REQUESTS => { zh: "4 请求", en: "4 Requests" },
    PAGE_SERVICE_STATUS => { zh: "6 状态", en: "6 Status" },
    PAGE_STATS => { zh: "5 用量", en: "5 Usage" },
    PAGE_SETTINGS => { zh: "7 设置", en: "7 Settings" },
    PAGE_HISTORY => { zh: "8 历史", en: "8 History" },
    PAGE_RECENT => { zh: "9 最近", en: "9 Recent" },
    PAGE_FLEET => { zh: "0 Fleet", en: "0 Fleet" },

    FOCUS_SESSIONS => { zh: "会话", en: "Sessions" },
    FOCUS_REQUESTS => { zh: "请求", en: "Requests" },
    FOCUS_PROVIDERS => { zh: "提供商", en: "Providers" },
    FOCUS_LABEL => { zh: "焦点：", en: "focus: " },
    STATS_FOCUS_POOLS => { zh: "额度池", en: "pools" },
    STATS_FOCUS_PROJECTS => { zh: "项目", en: "projects" },
    STATS_FOCUS_PROVIDERS => { zh: "提供商", en: "providers" },
    STATS_FOCUS_ENDPOINTS => { zh: "端点", en: "endpoints" },

    STATUS_ACTIVE_SHORT => { zh: "活跃 ", en: "active " },
    STATUS_ERRORS_SHORT => { zh: "错误(80) ", en: "errors(80) " },
    STATUS_CURRENT_SHORT => { zh: "当前 ", en: "cur " },
    STATUS_UPDATED_SHORT => { zh: "刷新 ", en: "updated " },
    STATUS_ACTIVE_TINY => { zh: "活 ", en: "act " },
    STATUS_ERRORS_TINY => { zh: "错 ", en: "err " },
    STATUS_UPDATED_TINY => { zh: "刷 ", en: "upd " },

    OVERLAY_HELP_TITLE => { zh: "帮助", en: "Help" },
    OVERLAY_SESSION_TRANSCRIPT => { zh: "会话对话记录", en: "Session transcript" },
    OVERLAY_STARTUP_GUARDRAIL => { zh: "启动检查", en: "Startup guardrail" },

    FOOTER_DASHBOARD => { zh: "1-9/0 页面  q 退出  L 语言  Tab 焦点  ↑/↓ 移动  o/h 跳转  ? 帮助", en: "1-9/0 pages  q quit  L language  Tab focus  ↑/↓ move  o/h navigate  ? help" },
    FOOTER_ROUTING => { zh: "1-9/0 页面  q 退出  ↑/↓/Pg 端点  Enter 操作  a 自动  m 模式  g 刷新  i 详情  ? 帮助", en: "1-9/0 pages  q quit  ↑/↓/Pg endpoint  Enter actions  a auto  m mode  g refresh  i details  ? help" },
    FOOTER_REQUESTS => { zh: "1-9/0 页面  q 退出  L 语言  ↑/↓ 选择  e 错误  s scope  o/h 跳转  ? 帮助", en: "1-9/0 pages  q quit  L language  ↑/↓ select  e errors  s scope  o/h navigate  ? help" },
    FOOTER_SESSIONS => { zh: "1-9/0 页面  q 退出  L 语言  ↑/↓ 选择  a/e/v 筛选  t 记录  ? 帮助", en: "1-9/0 pages  q quit  L language  ↑/↓ select  a/e/v filters  t transcript  ? help" },
    FOOTER_STATS => { zh: "1-9/0 页面  q 退出  L 语言  Tab 池/项目/提供商/端点  ↑/↓ 选择  g 刷新  y 报告  ? 帮助", en: "1-9/0 pages  q quit  L language  Tab pool/project/provider/endpoint  ↑/↓ select  g refresh  y report  ? help" },
    FOOTER_SETTINGS_CODEX => { zh: "1-9/0 页面  q 退出  L 语言  n/o 本地 Codex switch  ? 帮助", en: "1-9/0 pages  q quit  L language  n/o local Codex switch  ? help" },
    FOOTER_SETTINGS_OTHER => { zh: "1-9/0 页面  q 退出  L 语言  ? 帮助", en: "1-9/0 pages  q quit  L language  ? help" },
    FOOTER_HISTORY => { zh: "1-9/0 页面  q 退出  L 语言  ↑/↓ 选择  r 刷新  t/Enter 记录  s/f 跳转  ? 帮助", en: "1-9/0 pages  q quit  L language  ↑/↓ select  r refresh  t/Enter transcript  s/f navigate  ? help" },
    FOOTER_RECENT => { zh: "1-9/0 页面  q 退出  L 语言  ↑/↓ 选择  [] 时间  Enter/y 复制  t 记录  s/f/h 跳转  ? 帮助", en: "1-9/0 pages  q quit  L language  ↑/↓ select  [] window  Enter/y copy  t transcript  s/f/h navigate  ? help" },
    FOOTER_FLEET => { zh: "1-9/0 页面  q 退出  L 语言  Tab 视图  ↑/↓ 选择  ? 帮助", en: "1-9/0 pages  q quit  L language  Tab view  ↑/↓ select  ? help" },
    FOOTER_SERVICE_STATUS => { zh: "1-9/0 页面  q 退出  L 语言  r 刷新状态  ? 帮助", en: "1-9/0 pages  q quit  L language  r refresh status  ? help" },
    FOOTER_HELP => { zh: "Esc 关闭帮助  L 语言", en: "Esc close help  L language" },
    FOOTER_PROVIDER_INFO => { zh: "↑/↓ 滚动  PgUp/PgDn 翻页  Esc 关闭  L 语言", en: "↑/↓ scroll  PgUp/PgDn page  Esc close  L language" },
    FOOTER_SESSION_TRANSCRIPT => { zh: "↑/↓ 滚动  PgUp/PgDn 翻页  g/G 顶/底  A 全量/尾部  y 复制  t/Esc 关闭  L 语言", en: "↑/↓ scroll  PgUp/PgDn page  g/G top/bottom  A all/tail  y copy  t/Esc close  L language" },
    FOOTER_STARTUP_GUARDRAIL => { zh: "Esc/Enter 关闭启动检查  L 语言", en: "Esc/Enter close startup guardrail  L language" },

    KEYS_LABEL => { zh: "按键：", en: "keys: " },
    NO_TRANSCRIPT_MESSAGES => { zh: "未找到可展示的对话消息（可能该会话不在 ~/.codex/sessions，或格式发生变化）。", en: "No displayable transcript messages were found; the session may be outside ~/.codex/sessions or the format changed." },

    HISTORY_TITLE => { zh: "历史会话 (Codex 全局)", en: "History sessions (Codex global)" },
    RECENT_TITLE => { zh: "最近会话 (Codex 全局)", en: "Recent sessions (Codex global)" },
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

    TUI_OPTIONS_TITLE => { zh: "TUI 选项", en: "TUI options" },
    PROFILE_CONTROL_TITLE => { zh: "Profile 控制", en: "Profile control" },
    CONFIGURED_DEFAULT_LABEL => { zh: "配置默认：", en: "configured default: " },
    EFFECTIVE_DEFAULT_LABEL => { zh: "当前生效：", en: "effective default: " },
    NO_PROFILES => { zh: "<no profiles>", en: "<no profiles>" },
    AVAILABLE_PROFILES_LABEL => { zh: "可用 profile：", en: "available profiles: " },
    HISTORY_REFRESHING => { zh: "history: 刷新中…", en: "history: refreshing…" },
    RECENT_REFRESHING => { zh: "recent: 刷新中…", en: "recent: refreshing…" },
    RECENT_COPIED_SELECTED => { zh: "recent: 已复制选中条目", en: "recent: copied selected entry" },
    RECENT_SESSION_NOT_OBSERVED => { zh: "sessions: 当前 runtime 未观测到这个 recent session", en: "sessions: this recent session is not observed by the current runtime" },
    RECENT_COPIED_VISIBLE => { zh: "recent: 已复制可见列表", en: "recent: copied visible list" },
    SESSION_NOT_OBSERVED => { zh: "sessions: 当前 runtime 未观测到这个 session", en: "sessions: this session is not observed by the current runtime" },
    HISTORY_NO_TRANSCRIPT_FILE => { zh: "history: 当前选中项没有本地 transcript 文件", en: "history: selected item has no local transcript file" },
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
        "No provider selected." => "未选择提供商。",
        "No providers available." => "没有可用提供商。",
        "No requests match the current filters." => "没有请求匹配当前筛选。",
        "No sessions match the current filters." => "没有会话匹配当前筛选。",
        "Observed route" => "观测路由",
        "On" => "启用",
        "Out" => "输出",
        "Path" => "路径",
        "Provider" => "提供商",
        "Provider details" => "提供商详情",
        "Provider routing" => "提供商路由",
        "Providers" => "提供商",
        "Recent sample" => "最近样本",
        "reasoning guard" => "推理保护",
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
        "Usage page" => "用量页",
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
        "configured" => "已配置",
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
        "failed" => "失败",
        "failed to load transcript" => "加载 transcript 失败",
        "follow selected session" => "跟随选中会话",
        "focus" => "焦点",
        "focused from" => "聚焦来源",
        "generation" => "生成",
        "global" => "全局",
        "global override" => "全局覆盖",
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
        "latency" => "延迟",
        "lazy" => "不降级",
        "lazy reset" => "不降级耗尽",
        "lb" => "负载",
        "left" => "剩余",
        "legend" => "图例",
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
        "mode" => "模式",
        "model" => "模型",
        "models" => "模型",
        "mtime" => "修改时间",
        "name" => "名称",
        "nested route graph: edit route nodes in TOML for grouped reorder" => {
            "嵌套路由图：请在 TOML 中编辑路由节点来分组排序"
        }
        "next" => "下步",
        "no" => "否",
        "no balance/quota data" => "没有余额/配额数据",
        "no Codex session file found for this session id" => {
            "没有找到该 session id 对应的 Codex 会话文件"
        }
        "No startup issues are currently recorded." => "当前没有启动检查项。",
        "no data" => "没有数据",
        "no price rows" => "没有价格行",
        "no session selected" => "未选择会话",
        "no transcript file loaded" => "未加载对话记录文件",
        "no host-local transcript detected" => "未检测到本机 transcript",
        "no stored profile binding" => "没有已存储的 profile 绑定",
        "no_upstreams" => "没有可路由上游",
        "now" => "当前",
        "observed bridge" => "观测桥接",
        "observed only" => "仅运行时观测",
        "ok" => "成功",
        "on" => "开",
        "on_exhausted" => "耗尽策略",
        "off" => "关",
        "order" => "顺序",
        "out_tok/s" => "输出 tok/s",
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
        "provider endpoint" => "提供商端点",
        "probe" => "探针",
        "probes" => "探针",
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
        "fleet" => "fleet",
        "fleet: load failed" => "fleet: 加载失败",
        "fleet: loaded" => "fleet: 已加载",
        "fleet: refreshing" => "fleet: 刷新中",
        "flat" => "平铺",
        "tree" => "树形",
        "node" => "节点",
        "nodes" => "节点",
        "work unit" => "工作单元",
        "work units" => "工作单元",
        "source/confidence" => "来源/置信度",
        "last activity" => "最近活动",
        "snapshot age" => "快照年龄",
        "current work" => "当前工作",
        "no fleet snapshot" => "未加载 Fleet 快照",
        "fleet empty" => "Fleet 为空",
        "last error" => "最近错误",
        "endpoint" => "端点",
        "processes" => "进程",
        "topology" => "拓扑",
        "fresh" => "新鲜",
        "auth_failed" => "认证失败",
        "rate_limited" => "限流",
        "unsupported" => "不支持",
        "unreachable" => "不可达",
        "parse_failed" => "解析失败",
        "waiting_input" => "等待输入",
        "waiting_approval" => "等待审批",
        "idle" => "空闲",
        "interrupted" => "已中断",
        "completed" => "已完成",
        "errored" => "错误",
        "exited" => "已退出",
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
        "refresh" => "刷新",
        "retry policy" => "重试策略",
        "root" => "根目录",
        "rounds" => "轮次",
        "rt" => "运行态",
        "route graph" => "路由图",
        "routing" => "路由",
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
        "session" => "会话",
        "session override" => "会话覆盖",
        "route_target" => "路由目标",
        "fleet view" => "fleet 视图",
        "sessions filter" => "sessions 筛选",
        "sessions filter: reset" => "sessions 筛选：已重置",
        "sessions: focused" => "sessions: 已聚焦",
        "sessions: no session selected" => "sessions: 未选择会话",
        "sessions: selected row has no session id" => "sessions: 所选行没有 session id",
        "service status" => "服务状态",
        "service status: refreshing" => "service status: 正在刷新",
        "service_tier" => "服务层级",
        "service_tier set" => "service_tier 已设置",
        "sid" => "会话 ID",
        "skipped" => "跳过",
        "slow" => "慢",
        "source" => "来源",
        "stale" => "过期",
        "provider mapping" => "提供商映射",
        "status" => "状态",
        "strategy" => "策略",
        "stream" => "流式",
        "success" => "成功",
        "summary" => "摘要",
        "subscription" => "订阅",
        "tags" => "标签",
        "tail" => "尾部",
        "target" => "目标",
        "tier" => "服务层级",
        "not in catalog" => "不在目录中",
        "not configured" => "未配置",
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
        "uptime" => "可用率",
        "updated" => "已更新",
        "upstream" => "上游",
        "upstreams" => "上游",
        "user" => "用户",
        "unknown" => "未知",
        "unlimited" => "不限量",
        "used" => "已用",
        "usd" => "USD",
        "window" => "窗口",
        "yes" => "是",
        "pinned target only; breaker_open / empty upstreams block." => {
            "仅固定目标；breaker_open 或空上游会阻断。"
        }
        "This session keeps its stored profile binding while runtime observations explain the effective route." => {
            "该会话保留已存储的 profile 绑定，生效路由由运行时观测解释。"
        }
        "Effective route comes from request payloads, route graph defaults, and runtime fallback." => {
            "生效路由来自请求负载、路由图默认值和运行时回退。"
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

pub(crate) fn format_language_changed(current_lang: Language, selected_lang: Language) -> String {
    match current_lang {
        Language::Zh => format!("语言：{}（仅当前 TUI 会话）", language_name(selected_lang)),
        Language::En => format!(
            "language: {} (current TUI session only)",
            language_name(selected_lang)
        ),
    }
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
    fn routing_and_provider_labels_are_canonical_in_both_languages() {
        assert_eq!(text(Language::En, msg::PAGE_ROUTING), "2 Routing");
        assert_eq!(text(Language::Zh, msg::PAGE_ROUTING), "2 路由");
        assert_eq!(text(Language::En, msg::FOCUS_PROVIDERS), "Providers");
        assert_eq!(text(Language::Zh, msg::FOCUS_PROVIDERS), "提供商");
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
