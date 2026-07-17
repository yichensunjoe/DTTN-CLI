//! Chinese translations for user-facing TUI labels.

use std::borrow::Cow;

/// Translate a short shortcut-bar label while preserving unknown/plugin text.
pub fn hint_label(label: &str) -> Cow<'_, str> {
    if let Some(name) = label.strip_prefix("Reply to ") {
        return Cow::Owned(format!("回复 {name}"));
    }
    let translated = match label {
        "nav" => "导航",
        "turn" => "回合",
        "response" => "回复",
        "top/btm" => "顶部/底部",
        "bottom" => "底部",
        "scroll up" => "向上滚动",
        "scroll down" => "向下滚动",
        "half page up" => "上翻半页",
        "half page down" => "下翻半页",
        "page up" => "上翻一页",
        "page down" => "下翻一页",
        "fold" => "折叠",
        "all" => "全部",
        "expand/collapse thinking" => "展开/折叠思考",
        "raw" => "原始",
        "copy" => "复制",
        "copy cmd" => "复制命令",
        "view" => "查看",
        "link" => "链接",
        "rewind" => "回退",
        "kill" => "终止",
        "send" => "发送",
        "prompt" => "输入",
        "scrollback" => "对话记录",
        "cancel" => "取消",
        "mode" => "模式",
        "todos" => "待办",
        "tasks" => "任务",
        "queue" => "队列",
        "sessions" => "会话",
        "extensions" => "扩展",
        "send to bg" => "后台运行",
        "send now" => "立即发送",
        "voice mode" => "语音模式",
        "mic" => "麦克风",
        "multiline" => "多行",
        "shell" => "Shell",
        "yolo" | "always-approve" => "始终批准",
        "new" => "新建",
        "quit" | "exit" => "退出",
        "commands" => "命令",
        "shortcuts" => "快捷键",
        "model" => "模型",
        "settings" => "设置",
        "mouse reporting" => "鼠标报告",
        "dashboard" => "控制台",
        "next" => "下一个",
        "prev" => "上一个",
        "pin" => "置顶",
        "rename" => "重命名",
        "stop" => "停止",
        "close" => "关闭",
        "group" => "分组",
        "reorder up" => "上移",
        "reorder down" => "下移",
        "location" => "位置",
        "worktree" => "工作树",
        "close overlay" => "关闭浮层",
        "prev session" => "上一会话",
        "next session" => "下一会话",
        "New Agent" => "新建智能体",
        "back" => "返回",
        "list" => "列表",
        "input" => "输入",
        "select" => "选择",
        "scope" => "范围",
        "collapse" => "折叠",
        "expand" => "展开",
        "apply" => "应用",
        "open" => "打开",
        "show all" => "显示全部",
        "show fewer" => "显示更少",
        "plan" => "计划",
        "plan approval" => "计划审批",
        "comment" => "评论",
        "fullscreen" => "全屏",
        "save comment" => "保存评论",
        "request changes" => "请求修改",
        "approve" => "批准",
        "dismiss" => "忽略",
        "unselect" => "取消选择",
        "confirm" => "确认",
        "keep running" => "继续运行",
        "edit" => "编辑",
        "delete" => "删除",
        "clear" => "清空",
        "filename" => "文件名",
        "goto" => "跳转",
        "submit" => "提交",
        "save" => "保存",
        "switch tab" => "切换标签",
        "search" => "搜索",
        "details" => "详情",
        _ => return Cow::Borrowed(label),
    };
    Cow::Borrowed(translated)
}

/// Translate a mode name used by the transient Shift+Tab banner.
pub fn mode_name(mode: &str) -> Cow<'_, str> {
    match mode {
        "Normal" => Cow::Borrowed("普通"),
        "Plan" => Cow::Borrowed("计划"),
        "Always-Approve" | "Always-approve" => Cow::Borrowed("始终批准"),
        "Auto" => Cow::Borrowed("自动"),
        _ => Cow::Borrowed(mode),
    }
}

/// Translate the standard confirmation prefix used by the shortcuts bar.
pub fn confirmation(label: &str) -> String {
    format!("再次按下以{}", hint_label(label))
}

/// Translate the bundled development changelog entries shown on the welcome
/// screen. Unknown server-provided entries remain untouched.
pub fn changelog_bullet(text: &str) -> Cow<'_, str> {
    match text {
        "This is a dummy changelog entry for testing purposes." => {
            Cow::Borrowed("这是用于测试的示例更新日志条目。")
        }
        "Another dummy feature to verify the welcome screen renders correctly." => {
            Cow::Borrowed("另一项用于验证欢迎界面正确显示的示例功能。")
        }
        "Dummy bug fix entry for layout testing." => Cow::Borrowed("用于布局测试的示例问题修复。"),
        _ => Cow::Borrowed(text),
    }
}
