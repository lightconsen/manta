//! Keyword heuristics for routing an incoming message to the right
//! execution strategy.
//!
//! These are intentionally cheap, allocation-light substring checks run
//! on every inbound message before any LLM call. They decide whether a
//! request should engage the [`ComputerUseLoop`](crate::computer) (GUI /
//! desktop work) or the [`GoalPlanner`](crate::planner) (multi-step
//! orchestration).
//!
//! Keyword lists are grouped by language (English first, then Chinese)
//! so the i18n surface is obvious and easy to extend. Matching is
//! case-insensitive via [`str::to_lowercase`]; note that Chinese has no
//! case, so the lowercase pass is a no-op for those entries but keeps a
//! single code path.

/// English keywords indicating GUI / desktop interaction.
const DESKTOP_KEYWORDS_EN: &[&str] = &[
    "click",
    "screenshot",
    "screen shot",
    "take a screenshot",
    "open app",
    "open application",
    "launch app",
    "type in",
    "type text",
    "type ",
    "press",
    "press key",
    "keyboard",
    "mouse",
    "scroll",
    "drag",
    "right-click",
    "double-click",
    "desktop",
    "gui",
    "window",
    "browser",
    "chrome",
    "safari",
    "firefox",
    "edge",
    "file explorer",
    "finder",
    "spotlight",
    "menu bar",
    "taskbar",
    "dock",
    "notification",
];

/// Chinese keywords indicating GUI / desktop interaction.
const DESKTOP_KEYWORDS_ZH: &[&str] = &[
    "对话框",
    "点击",
    "截图",
    "屏幕",
    "打开应用",
    "打开软件",
    "输入",
    "键盘",
    "鼠标",
    "滚动",
    "拖拽",
    "桌面",
    "窗口",
    "浏览器",
];

/// English keywords indicating a complex, multi-step orchestration task.
const COMPLEX_KEYWORDS_EN: &[&str] = &[
    "deploy",
    "install",
    "setup",
    "configure",
    "build",
    "compile",
    "migrate",
    "backup",
    "restore",
    "pipeline",
    "orchestrate",
    "setup environment",
    "deploy to",
    "configure ssl",
    "configure https",
    "install and",
    "build and",
    "clone and",
    "docker compose",
    // Device / sensor orchestration
    "read sensor",
    "capture waveform",
    "oscilloscope",
    "multimeter",
    "motor",
    "actuator",
];

/// Chinese keywords indicating a complex, multi-step orchestration task.
const COMPLEX_KEYWORDS_ZH: &[&str] = &[
    "部署",
    "安装",
    "配置",
    "编译",
    "构建",
    "迁移",
    "备份",
    "恢复",
    "流水线",
    "读取传感器",
    "采集波形",
    "示波器",
    "万用表",
    "电机",
];

/// Fast check for desktop-operation tasks that should use
/// [`ComputerUseLoop`](crate::computer).
pub fn is_desktop_task(message: &str) -> bool {
    let lower = message.to_lowercase();
    DESKTOP_KEYWORDS_EN
        .iter()
        .chain(DESKTOP_KEYWORDS_ZH.iter())
        .any(|kw| lower.contains(kw))
}

/// Fast check for complex multi-step tasks that should use
/// [`GoalPlanner`](crate::planner).
pub fn is_complex_task(message: &str) -> bool {
    let lower = message.to_lowercase();
    COMPLEX_KEYWORDS_EN
        .iter()
        .chain(COMPLEX_KEYWORDS_ZH.iter())
        .any(|kw| lower.contains(kw))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_desktop_task_positive() {
        assert!(is_desktop_task("click the button"));
        assert!(is_desktop_task("take a screenshot"));
        assert!(is_desktop_task("open Chrome"));
        assert!(is_desktop_task("type hello in the search box"));
        assert!(is_desktop_task("press cmd+space"));
        assert!(is_desktop_task("截图"));
        assert!(is_desktop_task("点击确认按钮"));
        assert!(is_desktop_task("打开浏览器"));
    }

    #[test]
    fn test_is_desktop_task_negative() {
        assert!(!is_desktop_task("hello"));
        assert!(!is_desktop_task("what is the weather"));
        assert!(!is_desktop_task("explain quantum computing"));
        assert!(!is_desktop_task("write a poem"));
    }

    #[test]
    fn test_is_complex_task_positive() {
        assert!(is_complex_task("deploy the service to staging"));
        assert!(is_complex_task("install and configure nginx"));
        assert!(is_complex_task("read sensor data from the bus"));
        assert!(is_complex_task("部署到生产环境"));
        assert!(is_complex_task("配置 SSL 证书"));
    }

    #[test]
    fn test_is_complex_task_negative() {
        assert!(!is_complex_task("hello there"));
        assert!(!is_complex_task("what is 2 + 2"));
        assert!(!is_complex_task("tell me a joke"));
    }
}
