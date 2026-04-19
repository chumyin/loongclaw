#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Language {
    #[default]
    En,
    ZhCn,
    ZhTw,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PiCopy {
    ThinkingTitle,
    ThinkingLive,
    Tutorial,
    StartupSectionMcp,
    StartupSectionSkills,
    StartupSectionAcp,
    CommandDeckLabelHelp,
    CommandDeckDescHelp,
    CommandDeckLabelStatus,
    CommandDeckDescStatus,
    CommandDeckLabelHistory,
    CommandDeckDescHistory,
    CommandDeckLabelCompact,
    CommandDeckDescCompact,
    CommandDeckLabelSessions,
    CommandDeckDescSessions,
    CommandDeckLabelWorkers,
    CommandDeckDescWorkers,
    CommandDeckLabelReview,
    CommandDeckDescReview,
    CommandDeckLabelMission,
    CommandDeckDescMission,
    CommandDeckLabelFastLane,
    CommandDeckDescFastLane,
    CommandDeckLabelSafeLane,
    CommandDeckDescSafeLane,
    CommandDeckLabelCheckpoint,
    CommandDeckDescCheckpoint,
    CommandDeckLabelRepair,
    CommandDeckDescRepair,
    CommandDeckLabelExit,
    CommandDeckDescExit,
    CommandDeckEmpty,
}

#[derive(Debug, Clone, Default)]
pub struct I18nService {
    current_lang: Language,
}

impl I18nService {
    pub fn new(lang: Language) -> Self {
        Self { current_lang: lang }
    }

    pub fn current_lang(&self) -> Language {
        self.current_lang
    }

    pub fn set_lang(&mut self, lang: Language) {
        self.current_lang = lang;
    }

    pub fn text(&self, key: PiCopy) -> &'static str {
        match self.current_lang {
            Language::En => en_text(key),
            Language::ZhCn => zh_cn_text(key),
            Language::ZhTw => zh_tw_text(key),
        }
    }
}

fn en_text(key: PiCopy) -> &'static str {
    match key {
        PiCopy::ThinkingTitle => "thinking",
        PiCopy::ThinkingLive => "streaming reasoning + tool state",
        PiCopy::Tutorial => "escape interrupt · : deck · / commands · ctrl+o compaction",
        PiCopy::StartupSectionMcp => "MCP",
        PiCopy::StartupSectionSkills => "Skills",
        PiCopy::StartupSectionAcp => "ACP",
        PiCopy::CommandDeckLabelHelp => "help",
        PiCopy::CommandDeckDescHelp => "Show keyboard shortcuts and control-surface commands",
        PiCopy::CommandDeckLabelStatus => "status",
        PiCopy::CommandDeckDescStatus => "Inspect runtime posture and continuity settings",
        PiCopy::CommandDeckLabelHistory => "history",
        PiCopy::CommandDeckDescHistory => "Show the current transcript window",
        PiCopy::CommandDeckLabelCompact => "compact",
        PiCopy::CommandDeckDescCompact => "Create a manual continuity checkpoint",
        PiCopy::CommandDeckLabelSessions => "sessions",
        PiCopy::CommandDeckDescSessions => "Inspect visible sessions rooted at the current scope",
        PiCopy::CommandDeckLabelWorkers => "workers",
        PiCopy::CommandDeckDescWorkers => "Inspect visible delegate worker sessions",
        PiCopy::CommandDeckLabelReview => "review",
        PiCopy::CommandDeckDescReview => "Inspect the latest approval and review queue",
        PiCopy::CommandDeckLabelMission => "mission",
        PiCopy::CommandDeckDescMission => "Inspect mission-control lane counts and phase state",
        PiCopy::CommandDeckLabelFastLane => "fast lane",
        PiCopy::CommandDeckDescFastLane => "Summarize recent fast-lane execution batches",
        PiCopy::CommandDeckLabelSafeLane => "safe lane",
        PiCopy::CommandDeckDescSafeLane => "Summarize safe-lane runtime events",
        PiCopy::CommandDeckLabelCheckpoint => "checkpoint",
        PiCopy::CommandDeckDescCheckpoint => "Summarize durable turn finalization state",
        PiCopy::CommandDeckLabelRepair => "repair tail",
        PiCopy::CommandDeckDescRepair => "Repair durable turn finalization when safe",
        PiCopy::CommandDeckLabelExit => "exit",
        PiCopy::CommandDeckDescExit => "Leave interactive chat",
        PiCopy::CommandDeckEmpty => "no matching commands",
    }
}

fn zh_cn_text(key: PiCopy) -> &'static str {
    match key {
        PiCopy::ThinkingTitle => "思考中",
        PiCopy::ThinkingLive => "实时显示推理与工具状态",
        PiCopy::Tutorial => "esc 中断 · : 命令台 · / 命令 · ctrl+o 压缩",
        PiCopy::StartupSectionMcp => "MCP",
        PiCopy::StartupSectionSkills => "技能",
        PiCopy::StartupSectionAcp => "ACP",
        PiCopy::CommandDeckLabelHelp => "帮助",
        PiCopy::CommandDeckDescHelp => "查看快捷键与控制面命令",
        PiCopy::CommandDeckLabelStatus => "状态",
        PiCopy::CommandDeckDescStatus => "查看运行姿态与连续性设置",
        PiCopy::CommandDeckLabelHistory => "历史",
        PiCopy::CommandDeckDescHistory => "查看当前转录窗口",
        PiCopy::CommandDeckLabelCompact => "压缩",
        PiCopy::CommandDeckDescCompact => "创建手动连续性检查点",
        PiCopy::CommandDeckLabelSessions => "会话",
        PiCopy::CommandDeckDescSessions => "查看当前作用域下可见会话",
        PiCopy::CommandDeckLabelWorkers => "工作者",
        PiCopy::CommandDeckDescWorkers => "查看可见的委派工作者会话",
        PiCopy::CommandDeckLabelReview => "审阅",
        PiCopy::CommandDeckDescReview => "查看最新审批与审阅队列",
        PiCopy::CommandDeckLabelMission => "任务态势",
        PiCopy::CommandDeckDescMission => "查看 mission-control 分支数量与阶段状态",
        PiCopy::CommandDeckLabelFastLane => "快速通道",
        PiCopy::CommandDeckDescFastLane => "汇总最近的快速通道执行批次",
        PiCopy::CommandDeckLabelSafeLane => "安全通道",
        PiCopy::CommandDeckDescSafeLane => "汇总安全通道运行事件",
        PiCopy::CommandDeckLabelCheckpoint => "检查点",
        PiCopy::CommandDeckDescCheckpoint => "汇总持久化 turn 完成状态",
        PiCopy::CommandDeckLabelRepair => "修复尾部",
        PiCopy::CommandDeckDescRepair => "在安全时修复持久化 turn 尾部",
        PiCopy::CommandDeckLabelExit => "退出",
        PiCopy::CommandDeckDescExit => "离开交互聊天",
        PiCopy::CommandDeckEmpty => "没有匹配的命令",
    }
}

fn zh_tw_text(key: PiCopy) -> &'static str {
    match key {
        PiCopy::ThinkingTitle => "思考中",
        PiCopy::ThinkingLive => "即時顯示推理與工具狀態",
        PiCopy::Tutorial => "esc 中斷 · : 命令台 · / 命令 · ctrl+o 壓縮",
        PiCopy::StartupSectionMcp => "MCP",
        PiCopy::StartupSectionSkills => "技能",
        PiCopy::StartupSectionAcp => "ACP",
        PiCopy::CommandDeckLabelHelp => "幫助",
        PiCopy::CommandDeckDescHelp => "查看快捷鍵與控制面命令",
        PiCopy::CommandDeckLabelStatus => "狀態",
        PiCopy::CommandDeckDescStatus => "查看執行姿態與連續性設定",
        PiCopy::CommandDeckLabelHistory => "歷史",
        PiCopy::CommandDeckDescHistory => "查看目前逐字稿視窗",
        PiCopy::CommandDeckLabelCompact => "壓縮",
        PiCopy::CommandDeckDescCompact => "建立手動連續性檢查點",
        PiCopy::CommandDeckLabelSessions => "工作階段",
        PiCopy::CommandDeckDescSessions => "查看目前範圍下可見工作階段",
        PiCopy::CommandDeckLabelWorkers => "工作者",
        PiCopy::CommandDeckDescWorkers => "查看可見的委派工作者工作階段",
        PiCopy::CommandDeckLabelReview => "審閱",
        PiCopy::CommandDeckDescReview => "查看最新核准與審閱佇列",
        PiCopy::CommandDeckLabelMission => "任務態勢",
        PiCopy::CommandDeckDescMission => "查看 mission-control 分支數量與階段狀態",
        PiCopy::CommandDeckLabelFastLane => "快速通道",
        PiCopy::CommandDeckDescFastLane => "彙總最近快速通道執行批次",
        PiCopy::CommandDeckLabelSafeLane => "安全通道",
        PiCopy::CommandDeckDescSafeLane => "彙總安全通道執行事件",
        PiCopy::CommandDeckLabelCheckpoint => "檢查點",
        PiCopy::CommandDeckDescCheckpoint => "彙總持久化 turn 完成狀態",
        PiCopy::CommandDeckLabelRepair => "修復尾端",
        PiCopy::CommandDeckDescRepair => "在安全時修復持久化 turn 尾端",
        PiCopy::CommandDeckLabelExit => "退出",
        PiCopy::CommandDeckDescExit => "離開互動聊天",
        PiCopy::CommandDeckEmpty => "沒有符合的命令",
    }
}
