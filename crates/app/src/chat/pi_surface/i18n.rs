#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    En,
    ZhCn,
    ZhTw,
    Ja,
    Ru,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PiCopy {
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
    CommandDeckLabelExit,
    CommandDeckDescExit,
    CommandDeckEmpty,
    FooterQueueHint,
    FooterQueueShort,
    FooterRestoreQueued,
    FooterRestoreShort,
    FooterFollowHint,
    FooterFollowShort,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct I18nService {
    current_lang: Language,
}

impl I18nService {
    pub fn new(lang: Language) -> Self {
        Self { current_lang: lang }
    }

    pub fn text(&self, key: PiCopy) -> &'static str {
        text_for(self.current_lang, key)
    }
}

pub fn resolve_default_language() -> Language {
    let locale = std::env::var("LC_ALL")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| std::env::var("LANG").ok())
        .unwrap_or_default()
        .to_ascii_lowercase();

    if locale.contains("zh_tw") || locale.contains("zh-hant") || locale.contains("zh_hk") {
        Language::ZhTw
    } else if locale.contains("zh_cn") || locale.contains("zh-hans") || locale.contains("zh_sg") {
        Language::ZhCn
    } else if locale.contains("ja") {
        Language::Ja
    } else if locale.contains("ru") {
        Language::Ru
    } else {
        Language::En
    }
}

fn text_for(lang: Language, key: PiCopy) -> &'static str {
    match lang {
        Language::En => en_text(key),
        Language::ZhCn => zh_cn_text(key),
        Language::ZhTw => zh_tw_text(key),
        Language::Ja => ja_text(key),
        Language::Ru => ru_text(key),
    }
}

fn en_text(key: PiCopy) -> &'static str {
    match key {
        PiCopy::Tutorial => "ctrl+c exit · :/ commands · type $skill directly · ctrl+o compaction",
        PiCopy::StartupSectionMcp => "MCP",
        PiCopy::StartupSectionSkills => "Repo skills",
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
        PiCopy::CommandDeckLabelExit => "exit",
        PiCopy::CommandDeckDescExit => "Leave interactive chat",
        PiCopy::CommandDeckEmpty => "no matching commands",
        PiCopy::FooterQueueHint => "Tab to queue message",
        PiCopy::FooterQueueShort => "Tab to queue",
        PiCopy::FooterRestoreQueued => "to restore queued message",
        PiCopy::FooterRestoreShort => "restore queued",
        PiCopy::FooterFollowHint => "PgDn / End to latest reply",
        PiCopy::FooterFollowShort => "End to latest",
    }
}

fn zh_cn_text(key: PiCopy) -> &'static str {
    match key {
        PiCopy::Tutorial => "ctrl+c 退出 · :/ 命令 · 直接输入 $skill · ctrl+o 压缩",
        PiCopy::StartupSectionMcp => "MCP",
        PiCopy::StartupSectionSkills => "仓库技能",
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
        PiCopy::CommandDeckLabelExit => "退出",
        PiCopy::CommandDeckDescExit => "离开交互聊天",
        PiCopy::CommandDeckEmpty => "没有匹配的命令",
        PiCopy::FooterQueueHint => "按 Tab 将消息加入队列",
        PiCopy::FooterQueueShort => "Tab 加入队列",
        PiCopy::FooterRestoreQueued => "可恢复排队消息",
        PiCopy::FooterRestoreShort => "恢复队列",
        PiCopy::FooterFollowHint => "PgDn / End 跳到最新回复",
        PiCopy::FooterFollowShort => "End 到最新",
    }
}

fn zh_tw_text(key: PiCopy) -> &'static str {
    match key {
        PiCopy::Tutorial => "ctrl+c 離開 · :/ 命令 · 直接輸入 $skill · ctrl+o 壓縮",
        PiCopy::StartupSectionMcp => "MCP",
        PiCopy::StartupSectionSkills => "倉庫技能",
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
        PiCopy::CommandDeckLabelExit => "退出",
        PiCopy::CommandDeckDescExit => "離開互動聊天",
        PiCopy::CommandDeckEmpty => "沒有符合的命令",
        PiCopy::FooterQueueHint => "按 Tab 將訊息加入佇列",
        PiCopy::FooterQueueShort => "Tab 加入佇列",
        PiCopy::FooterRestoreQueued => "可還原排隊訊息",
        PiCopy::FooterRestoreShort => "還原佇列",
        PiCopy::FooterFollowHint => "PgDn / End 跳到最新回覆",
        PiCopy::FooterFollowShort => "End 到最新",
    }
}

fn ja_text(key: PiCopy) -> &'static str {
    match key {
        PiCopy::Tutorial => "ctrl+c で終了 · :/ コマンド · $skill を直接入力 · ctrl+o 圧縮",
        PiCopy::StartupSectionMcp => "MCP",
        PiCopy::StartupSectionSkills => "リポジトリスキル",
        PiCopy::StartupSectionAcp => "ACP",
        PiCopy::CommandDeckLabelHelp => "ヘルプ",
        PiCopy::CommandDeckDescHelp => "ショートカットと制御面コマンドを表示",
        PiCopy::CommandDeckLabelStatus => "状態",
        PiCopy::CommandDeckDescStatus => "実行姿勢と連続性設定を確認",
        PiCopy::CommandDeckLabelHistory => "履歴",
        PiCopy::CommandDeckDescHistory => "現在の転写ウィンドウを表示",
        PiCopy::CommandDeckLabelCompact => "圧縮",
        PiCopy::CommandDeckDescCompact => "手動の連続性チェックポイントを作成",
        PiCopy::CommandDeckLabelSessions => "セッション",
        PiCopy::CommandDeckDescSessions => "現在のスコープにぶら下がる可視セッションを確認",
        PiCopy::CommandDeckLabelWorkers => "ワーカー",
        PiCopy::CommandDeckDescWorkers => "可視の委譲ワーカーセッションを確認",
        PiCopy::CommandDeckLabelReview => "レビュー",
        PiCopy::CommandDeckDescReview => "最新の承認・レビューキューを確認",
        PiCopy::CommandDeckLabelMission => "ミッション",
        PiCopy::CommandDeckDescMission => "mission-control の分岐数と段階を確認",
        PiCopy::CommandDeckLabelExit => "終了",
        PiCopy::CommandDeckDescExit => "インタラクティブチャットを終了",
        PiCopy::CommandDeckEmpty => "一致するコマンドがありません",
        PiCopy::FooterQueueHint => "Tab でメッセージをキューへ",
        PiCopy::FooterQueueShort => "Tab でキューへ",
        PiCopy::FooterRestoreQueued => "でキュー済みメッセージを復元",
        PiCopy::FooterRestoreShort => "キュー復元",
        PiCopy::FooterFollowHint => "PgDn / End で最新返信へ",
        PiCopy::FooterFollowShort => "End で最新へ",
    }
}

fn ru_text(key: PiCopy) -> &'static str {
    match key {
        PiCopy::Tutorial => "ctrl+c выйти · :/ команды · вводите $skill прямо · ctrl+o сжатие",
        PiCopy::StartupSectionMcp => "MCP",
        PiCopy::StartupSectionSkills => "Навыки репозитория",
        PiCopy::StartupSectionAcp => "ACP",
        PiCopy::CommandDeckLabelHelp => "помощь",
        PiCopy::CommandDeckDescHelp => "Показать сочетания клавиш и команды панели",
        PiCopy::CommandDeckLabelStatus => "статус",
        PiCopy::CommandDeckDescStatus => "Проверить состояние рантайма и непрерывности",
        PiCopy::CommandDeckLabelHistory => "история",
        PiCopy::CommandDeckDescHistory => "Показать текущее окно транскрипта",
        PiCopy::CommandDeckLabelCompact => "сжать",
        PiCopy::CommandDeckDescCompact => "Создать ручную точку непрерывности",
        PiCopy::CommandDeckLabelSessions => "сессии",
        PiCopy::CommandDeckDescSessions => "Показать видимые сессии в текущей области",
        PiCopy::CommandDeckLabelWorkers => "воркеры",
        PiCopy::CommandDeckDescWorkers => "Показать видимые сессии делегированных воркеров",
        PiCopy::CommandDeckLabelReview => "ревью",
        PiCopy::CommandDeckDescReview => "Показать очередь последних одобрений и ревью",
        PiCopy::CommandDeckLabelMission => "миссия",
        PiCopy::CommandDeckDescMission => "Показать количество веток mission-control и фазу",
        PiCopy::CommandDeckLabelExit => "выход",
        PiCopy::CommandDeckDescExit => "Выйти из чата",
        PiCopy::CommandDeckEmpty => "нет подходящих команд",
        PiCopy::FooterQueueHint => "Tab — поставить сообщение в очередь",
        PiCopy::FooterQueueShort => "Tab — в очередь",
        PiCopy::FooterRestoreQueued => "чтобы вернуть сообщение из очереди",
        PiCopy::FooterRestoreShort => "вернуть очередь",
        PiCopy::FooterFollowHint => "PgDn / End к последнему ответу",
        PiCopy::FooterFollowShort => "End к последнему",
    }
}
