use super::command_palette::CommandPalette;
use super::i18n::{I18nService, Language, SurfaceCopy, resolve_default_language};
use super::message_list::{
    MessageList, StartupEyeAnimation, StartupEyeFocus, StartupPanel, StartupPanelOption,
};
use crate::config::{
    InitiativeLevel, LoongConfig, PersonalizationConfig, PersonalizationPromptState,
    ProviderConfig, ProviderKind, ProviderProfileConfig, ResponseDensity,
    service_channel_descriptors,
};
use crate::prompt::PromptPersonality;
use std::fs;
use std::path::{Path, PathBuf};

pub(crate) const ONBOARDING_SOUL_MANAGED_START: &str = "<!-- LOONG:ONBOARDING:START -->";
pub(crate) const ONBOARDING_SOUL_MANAGED_END: &str = "<!-- LOONG:ONBOARDING:END -->";
pub(crate) const ONBOARDING_IDENTITY_MANAGED_START: &str = "<!-- LOONG:IDENTITY:START -->";
pub(crate) const ONBOARDING_IDENTITY_MANAGED_END: &str = "<!-- LOONG:IDENTITY:END -->";
pub(crate) const ONBOARDING_USER_MANAGED_START: &str = "<!-- LOONG:USER:START -->";
pub(crate) const ONBOARDING_USER_MANAGED_END: &str = "<!-- LOONG:USER:END -->";

#[derive(Debug, Clone)]
pub(crate) struct RepoOptionalSkill {
    pub(crate) name: String,
    pub(crate) description: String,
    pub(crate) source_dir: PathBuf,
    pub(crate) install_relative_path: PathBuf,
}

#[derive(Clone)]
pub(crate) struct StartupOnboardingController {
    state: StartupOnboardingState,
    first_turn_calibration_stage: FirstTurnCalibrationStage,
    config_path: Option<PathBuf>,
    workspace_root: Option<PathBuf>,
    optional_repo_skills: Vec<RepoOptionalSkill>,
    mcp_count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FirstTurnCalibrationStage {
    Idle,
    PromptPending,
    ReplyPending,
}

#[derive(Clone, Copy)]
pub(crate) struct StartupOnboardingKeyOutcome {
    pub(crate) handled: bool,
    pub(crate) animation: Option<StartupEyeAnimation>,
    pub(crate) language_applied: bool,
    pub(crate) finished: bool,
}

#[derive(Clone, Copy)]
pub(crate) enum StartupQuickstartFocus {
    Chat,
    Commands,
    Skills,
    Transcript,
}

#[derive(Clone, Copy)]
pub(crate) struct StartupSurfaceSnapshot {
    pub(crate) pending_turn: bool,
    pub(crate) following_tail: bool,
    pub(crate) has_conversation_messages: bool,
    pub(crate) composer_empty: bool,
    pub(crate) command_palette_active: bool,
    pub(crate) inline_skill_popup_active: bool,
    pub(crate) quickstart_focus: StartupQuickstartFocus,
}

pub(crate) fn startup_palette_eye_focus(selected: usize, total: usize) -> StartupEyeFocus {
    if total <= 1 {
        return StartupEyeFocus::DownCenter;
    }

    let ratio = selected as f32 / total.saturating_sub(1) as f32;
    if ratio < 0.34 {
        StartupEyeFocus::DownLeft
    } else if ratio > 0.66 {
        StartupEyeFocus::DownRight
    } else {
        StartupEyeFocus::DownCenter
    }
}

impl StartupOnboardingController {
    pub(crate) fn new(
        config_path: Option<PathBuf>,
        workspace_root: Option<PathBuf>,
        optional_repo_skills: Vec<RepoOptionalSkill>,
        mcp_count: usize,
    ) -> Self {
        let detected_provider_kind = detect_onboarding_provider_kind(config_path.as_deref());
        Self {
            state: StartupOnboardingState::new(optional_repo_skills.len(), detected_provider_kind),
            first_turn_calibration_stage: FirstTurnCalibrationStage::Idle,
            config_path,
            workspace_root,
            optional_repo_skills,
            mcp_count,
        }
    }

    pub(crate) fn startup_content(
        &self,
        i18n: &I18nService,
    ) -> (String, String, Vec<(String, Vec<String>)>, Vec<String>) {
        build_chat_startup_content(self.mcp_count, self.optional_repo_skills.len(), i18n)
    }
    pub(crate) fn eye_animation(&self) -> StartupEyeAnimation {
        self.state.eye_animation()
    }

    pub(crate) fn should_show_panel(&self, surface: StartupSurfaceSnapshot) -> bool {
        !self.state.completed
            && !surface.pending_turn
            && surface.following_tail
            && !surface.has_conversation_messages
    }

    pub(crate) fn active_in_surface(&self, surface: StartupSurfaceSnapshot) -> bool {
        self.should_show_panel(surface)
            && surface.composer_empty
            && !surface.command_palette_active
            && !surface.inline_skill_popup_active
    }

    pub(crate) fn panel_for_surface(
        &self,
        surface: StartupSurfaceSnapshot,
    ) -> Option<StartupPanel> {
        let language = resolve_startup_copy_language(self);
        if !self.should_show_panel(surface) {
            return None;
        }
        if self.active_in_surface(surface) {
            return Some(self.state.panel(&self.optional_repo_skills, language));
        }
        Some(build_quickstart_panel(surface.quickstart_focus, language))
    }

    pub(crate) fn apply_language_selection(
        &mut self,
        i18n: &mut I18nService,
        command_palette: &mut CommandPalette,
        message_list: &mut MessageList,
    ) {
        let language = self
            .state
            .selected_language
            .unwrap_or_else(|| startup_onboarding_language(self.state.selection));
        i18n.set_language(language);
        command_palette.set_language(language);
        let (_version, tutorial, sections, tips) = self.startup_content(i18n);
        message_list.set_latest_startup_header_content(tutorial, sections, tips);
    }

    pub(crate) fn handle_key(
        &mut self,
        key: crossterm::event::KeyEvent,
    ) -> StartupOnboardingKeyOutcome {
        let mut outcome = StartupOnboardingKeyOutcome {
            handled: true,
            animation: None,
            language_applied: false,
            finished: false,
        };

        match key.code {
            crossterm::event::KeyCode::Up | crossterm::event::KeyCode::Char('k') => {
                self.state.move_selection(-1);
            }
            crossterm::event::KeyCode::Down | crossterm::event::KeyCode::Char('j') => {
                self.state.move_selection(1);
            }
            crossterm::event::KeyCode::Char(' ') => {
                if self.state.toggle_current_option() {
                    outcome.animation = Some(StartupEyeAnimation::Confirm(self.state.focus()));
                } else {
                    outcome.handled = false;
                }
            }
            crossterm::event::KeyCode::Enter => {
                let finishing = self.state.stage == StartupOnboardingStage::Finish;
                outcome.language_applied = self.state.stage == StartupOnboardingStage::Language;
                self.state.confirm_current();
                outcome.finished = finishing && self.state.completed;
                if self.state.completed {
                    outcome.animation = Some(StartupEyeAnimation::Celebrate);
                }
            }
            crossterm::event::KeyCode::Esc => {
                if self.state.go_back() {
                    outcome.animation = Some(StartupEyeAnimation::Focus(self.state.focus()));
                } else {
                    outcome.handled = false;
                }
            }
            crossterm::event::KeyCode::Backspace
            | crossterm::event::KeyCode::Left
            | crossterm::event::KeyCode::Right
            | crossterm::event::KeyCode::Home
            | crossterm::event::KeyCode::End
            | crossterm::event::KeyCode::PageUp
            | crossterm::event::KeyCode::PageDown
            | crossterm::event::KeyCode::Tab
            | crossterm::event::KeyCode::BackTab
            | crossterm::event::KeyCode::Delete
            | crossterm::event::KeyCode::Insert
            | crossterm::event::KeyCode::F(_)
            | crossterm::event::KeyCode::Char(_)
            | crossterm::event::KeyCode::Null
            | crossterm::event::KeyCode::CapsLock
            | crossterm::event::KeyCode::ScrollLock
            | crossterm::event::KeyCode::NumLock
            | crossterm::event::KeyCode::PrintScreen
            | crossterm::event::KeyCode::Pause
            | crossterm::event::KeyCode::Menu
            | crossterm::event::KeyCode::KeypadBegin
            | crossterm::event::KeyCode::Media(_)
            | crossterm::event::KeyCode::Modifier(_) => {
                outcome.handled = false;
            }
        }

        outcome
    }

    pub(crate) fn finish(&mut self, message_list: &mut MessageList) {
        let follow_up_lines = render_startup_onboarding_follow_up_lines(&self.state);
        append_rendered_lines_if_any(message_list, follow_up_lines);
        append_persistence_result(
            message_list,
            self.install_selected_optional_repo_skills(),
            "Onboarding skill install error",
        );
        self.persist_seed_updates(
            message_list,
            "Onboarding config seed error",
            "Onboarding workspace seed error",
        );
        self.first_turn_calibration_stage = if self
            .state
            .selected_personalization
            .is_some_and(|choice| !choice.skips_calibration())
        {
            FirstTurnCalibrationStage::PromptPending
        } else {
            FirstTurnCalibrationStage::Idle
        };
    }

    pub(crate) fn take_first_turn_calibration_addendum(&mut self) -> Option<String> {
        if self.first_turn_calibration_stage != FirstTurnCalibrationStage::PromptPending {
            return None;
        }
        let addendum = render_first_turn_calibration_system_addendum(&self.state)?;
        self.first_turn_calibration_stage = FirstTurnCalibrationStage::ReplyPending;
        Some(addendum)
    }

    pub(crate) fn absorb_first_turn_calibration_if_needed(
        &mut self,
        message: &str,
        message_list: &mut MessageList,
    ) -> bool {
        if self.first_turn_calibration_stage != FirstTurnCalibrationStage::ReplyPending {
            return false;
        }
        self.first_turn_calibration_stage = FirstTurnCalibrationStage::Idle;

        let fallback = self.state.selected_personalization;
        let update = infer_first_turn_calibration_update(message, fallback);
        let identity_update = infer_first_turn_identity_update(message);
        let Some(choice) = update.choice else {
            return false;
        };
        self.state.selected_personalization = Some(choice);
        self.state.calibrated_density = update.density;
        self.state.calibrated_initiative = update.initiative;
        if let Some(preferred_address) = identity_update.preferred_address {
            self.state.preferred_address = Some(preferred_address);
        }
        if let Some(identity_name) = identity_update.identity_name {
            self.state.identity_name = Some(identity_name);
        }
        if let Some(identity_creature) = identity_update.identity_creature {
            self.state.identity_creature = Some(identity_creature);
        }
        if let Some(identity_vibe) = identity_update.identity_vibe {
            self.state.identity_vibe = Some(identity_vibe);
        }
        if let Some(identity_emoji) = identity_update.identity_emoji {
            self.state.identity_emoji = Some(identity_emoji);
        }
        append_rendered_lines_if_any(
            message_list,
            render_first_turn_calibration_update_lines(update, &self.state),
        );
        self.persist_seed_updates(
            message_list,
            "Calibration config update error",
            "Calibration workspace update error",
        );
        true
    }

    fn persist_seed_updates(
        &self,
        message_list: &mut MessageList,
        config_error_prefix: &str,
        workspace_error_prefix: &str,
    ) {
        append_persistence_result(
            message_list,
            persist_onboarding_config_seed(self.config_path.as_deref(), &self.state),
            config_error_prefix,
        );
        append_persistence_result(
            message_list,
            persist_onboarding_workspace_seed(self.workspace_root.as_deref(), &self.state),
            workspace_error_prefix,
        );
    }

    fn install_selected_optional_repo_skills(&self) -> Result<Vec<String>, String> {
        let selected_skills = self.selected_optional_repo_skills();
        install_repo_optional_skills(&selected_skills, self.workspace_root.as_deref())
    }

    fn selected_optional_repo_skills(&self) -> Vec<RepoOptionalSkill> {
        self.optional_repo_skills
            .iter()
            .zip(self.state.skill_enabled.iter())
            .filter_map(|(skill, enabled)| (*enabled).then_some(skill.clone()))
            .collect::<Vec<_>>()
    }
}

fn build_quickstart_panel(focus: StartupQuickstartFocus, language: Language) -> StartupPanel {
    let (chat_selected, commands_selected, skills_selected, transcript_selected) = match focus {
        StartupQuickstartFocus::Chat => (true, false, false, false),
        StartupQuickstartFocus::Commands => (false, true, false, false),
        StartupQuickstartFocus::Skills => (false, false, true, false),
        StartupQuickstartFocus::Transcript => (false, false, false, true),
    };

    let mut options = vec![
        StartupPanelOption {
            label: localized_quickstart_label(StartupQuickstartFocus::Chat, language).to_owned(),
            detail: localized_quickstart_detail(StartupQuickstartFocus::Chat, language).to_owned(),
            selected: chat_selected,
        },
        StartupPanelOption {
            label: localized_quickstart_label(StartupQuickstartFocus::Commands, language)
                .to_owned(),
            detail: localized_quickstart_detail(StartupQuickstartFocus::Commands, language)
                .to_owned(),
            selected: commands_selected,
        },
        StartupPanelOption {
            label: localized_quickstart_label(StartupQuickstartFocus::Skills, language).to_owned(),
            detail: localized_quickstart_detail(StartupQuickstartFocus::Skills, language)
                .to_owned(),
            selected: skills_selected,
        },
    ];

    if transcript_selected {
        options.push(StartupPanelOption {
            label: localized_quickstart_label(StartupQuickstartFocus::Transcript, language)
                .to_owned(),
            detail: localized_quickstart_detail(StartupQuickstartFocus::Transcript, language)
                .to_owned(),
            selected: true,
        });
    }

    StartupPanel {
        title: localized_quickstart_title(language).to_owned(),
        hint: localized_quickstart_hint(language).to_owned(),
        options,
    }
}

fn append_rendered_lines_if_any(message_list: &mut MessageList, lines: Vec<String>) {
    if !lines.is_empty() {
        message_list.add_rendered_lines(lines);
    }
}

fn append_persistence_result(
    message_list: &mut MessageList,
    result: Result<Vec<String>, String>,
    error_prefix: &str,
) {
    match result {
        Ok(lines) => append_rendered_lines_if_any(message_list, lines),
        Err(error) => {
            message_list.add_rendered_lines(vec![format!("{error_prefix}: {error}")]);
        }
    }
}

fn install_repo_optional_skills(
    skills: &[RepoOptionalSkill],
    workspace_root: Option<&Path>,
) -> Result<Vec<String>, String> {
    if skills.is_empty() {
        return Ok(Vec::new());
    }

    let install_root = resolve_repo_optional_skill_install_root(workspace_root)?;
    fs::create_dir_all(&install_root).map_err(|error| {
        format!(
            "failed to create optional skill install root {}: {error}",
            install_root.display()
        )
    })?;

    let mut lines = vec![format!("Installed optional repo skills: {}", skills.len())];

    for skill in skills {
        let target_dir = install_root.join(&skill.install_relative_path);
        if target_dir.exists() {
            fs::remove_dir_all(&target_dir).map_err(|error| {
                format!(
                    "failed to replace installed skill {} at {}: {error}",
                    skill.name,
                    target_dir.display()
                )
            })?;
        }
        if let Some(parent) = target_dir.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                format!(
                    "failed to create optional skill parent {}: {error}",
                    parent.display()
                )
            })?;
        }
        copy_directory_recursive(skill.source_dir.as_path(), target_dir.as_path())?;
        lines.push(format!("- {} -> {}", skill.name, target_dir.display()));
    }

    Ok(lines)
}

fn resolve_repo_optional_skill_install_root(
    workspace_root: Option<&Path>,
) -> Result<PathBuf, String> {
    let base_root = if let Some(workspace_root) = workspace_root {
        workspace_root.to_path_buf()
    } else {
        std::env::current_dir().map_err(|error| {
            format!("cannot resolve repo skill install root without a workspace root: {error}")
        })?
    };
    Ok(base_root.join(".loong").join("skills"))
}

fn copy_directory_recursive(source: &Path, target: &Path) -> Result<(), String> {
    fs::create_dir_all(target)
        .map_err(|error| format!("failed to create {}: {error}", target.display()))?;
    let entries = fs::read_dir(source)
        .map_err(|error| format!("failed to read {}: {error}", source.display()))?;
    for entry in entries {
        let entry = entry.map_err(|error| {
            format!("failed to iterate directory {}: {error}", source.display())
        })?;
        let file_type = entry.file_type().map_err(|error| {
            format!(
                "failed to read file type for {}: {error}",
                entry.path().display()
            )
        })?;
        let destination = target.join(entry.file_name());
        if file_type.is_dir() {
            copy_directory_recursive(entry.path().as_path(), destination.as_path())?;
        } else if file_type.is_file() {
            fs::copy(entry.path(), &destination).map_err(|error| {
                format!(
                    "failed to copy {} to {}: {error}",
                    entry.path().display(),
                    destination.display()
                )
            })?;
        }
    }
    Ok(())
}

fn provider_onboarding_brief(kind: ProviderKind) -> String {
    if kind == ProviderKind::Custom {
        "custom endpoint · env CUSTOM_PROVIDER_API_KEY".to_owned()
    } else if kind == ProviderKind::Bedrock {
        "AWS region + SigV4".to_owned()
    } else if kind == ProviderKind::GithubCopilot {
        "OAuth bridge".to_owned()
    } else if matches!(
        kind,
        ProviderKind::Llamacpp
            | ProviderKind::LmStudio
            | ProviderKind::Ollama
            | ProviderKind::Sglang
            | ProviderKind::Vllm
    ) {
        "local runtime".to_owned()
    } else {
        kind.default_api_key_env()
            .map(|env| format!("env {env}"))
            .unwrap_or_else(|| "provider profile".to_owned())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PersonalizationChoice {
    Balanced,
    Concise,
    Thorough,
    Skip,
    TurnOff,
}

impl PersonalizationChoice {
    const ALL: [Self; 5] = [
        Self::Balanced,
        Self::Concise,
        Self::Thorough,
        Self::Skip,
        Self::TurnOff,
    ];

    fn from_selection(selection: usize) -> Self {
        Self::ALL.get(selection).copied().unwrap_or(Self::TurnOff)
    }

    fn label(self) -> &'static str {
        match self {
            Self::Balanced => "balanced",
            Self::Concise => "concise",
            Self::Thorough => "thorough",
            Self::Skip => "skip",
            Self::TurnOff => "turn off",
        }
    }

    fn summary_label(self) -> &'static str {
        match self {
            Self::TurnOff => "disabled",
            Self::Balanced | Self::Concise | Self::Thorough | Self::Skip => self.label(),
        }
    }

    fn display_label(self, language: Language) -> &'static str {
        match language {
            Language::En => self.label(),
            Language::ZhCn => match self {
                Self::Balanced => "平衡",
                Self::Concise => "简洁",
                Self::Thorough => "深入",
                Self::Skip => "先跳过",
                Self::TurnOff => "关闭",
            },
            Language::ZhTw => match self {
                Self::Balanced => "平衡",
                Self::Concise => "精簡",
                Self::Thorough => "深入",
                Self::Skip => "先略過",
                Self::TurnOff => "關閉",
            },
            Language::Ja => match self {
                Self::Balanced => "標準",
                Self::Concise => "簡潔",
                Self::Thorough => "詳細",
                Self::Skip => "後で",
                Self::TurnOff => "無効化",
            },
            Language::Ru => match self {
                Self::Balanced => "сбалансировано",
                Self::Concise => "кратко",
                Self::Thorough => "подробно",
                Self::Skip => "позже",
                Self::TurnOff => "выключить",
            },
        }
    }

    fn display_panel_detail(self, language: Language) -> &'static str {
        match language {
            Language::En => self.panel_detail(),
            Language::ZhCn => match self {
                Self::Balanced => "默认平衡深度与主动性。",
                Self::Concise => "默认更短更直接，需要时再展开。",
                Self::Thorough => "默认更深入，并主动补充上下文。",
                Self::Skip => "先跳过个性化，后续对话里再调整。",
                Self::TurnOff => "关闭个性化，保持纯 coding agent 模式。",
            },
            Language::ZhTw => match self {
                Self::Balanced => "預設平衡深度與主動性。",
                Self::Concise => "預設更短更直接，需要時再展開。",
                Self::Thorough => "預設更深入，並主動補充上下文。",
                Self::Skip => "先略過個性化，之後再調整。",
                Self::TurnOff => "關閉個性化，保持純 coding agent 模式。",
            },
            Language::Ja => match self {
                Self::Balanced => "深さと主体性の標準設定です。",
                Self::Concise => "短めに返し、必要な時だけ広げます。",
                Self::Thorough => "詳しく返し、文脈も先回りします。",
                Self::Skip => "今は設定せず、後で調整します。",
                Self::TurnOff => "個性化を無効化し、純粋な coding agent にします。",
            },
            Language::Ru => match self {
                Self::Balanced => "Стандартный баланс глубины и инициативы.",
                Self::Concise => "Отвечать короче и раскрывать только по запросу.",
                Self::Thorough => "Отвечать глубже и проактивнее добавлять контекст.",
                Self::Skip => "Пока пропустить и настроить позже.",
                Self::TurnOff => "Отключить персонализацию и оставить чистый coding agent режим.",
            },
        }
    }

    fn panel_detail(self) -> &'static str {
        match self {
            Self::Balanced => "default depth and initiative for everyday use.",
            Self::Concise => "shorter replies with less expansion unless you ask for more.",
            Self::Thorough => "deeper explanations and more proactive context by default.",
            Self::Skip => "skip personalization for now and let loong learn from future turns.",
            Self::TurnOff => {
                "fully disable personalization and keep loong in plain coding-agent mode."
            }
        }
    }

    fn skips_calibration(self) -> bool {
        matches!(self, Self::Skip | Self::TurnOff)
    }

    fn default_seed(self) -> Option<(ResponseDensity, InitiativeLevel)> {
        match self {
            Self::Balanced => Some((ResponseDensity::Balanced, InitiativeLevel::Balanced)),
            Self::Concise => Some((ResponseDensity::Concise, InitiativeLevel::AskBeforeActing)),
            Self::Thorough => Some((ResponseDensity::Thorough, InitiativeLevel::HighInitiative)),
            Self::Skip | Self::TurnOff => None,
        }
    }

    fn prompt_personality(self) -> Option<PromptPersonality> {
        match self {
            Self::Balanced => Some(PromptPersonality::Classicist),
            Self::Concise => Some(PromptPersonality::Pragmatist),
            Self::Thorough => Some(PromptPersonality::Idealist),
            Self::Skip | Self::TurnOff => None,
        }
    }

    fn suppresses_personalization(self) -> bool {
        matches!(self, Self::TurnOff)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ContinueSetupChoice {
    WebProviders,
    Channels,
    Mcp,
    Personalization,
}

impl ContinueSetupChoice {
    fn label(self) -> &'static str {
        match self {
            Self::WebProviders => "web providers",
            Self::Channels => "channels",
            Self::Mcp => "MCP",
            Self::Personalization => "personalization",
        }
    }

    fn panel_detail(self) -> &'static str {
        match self {
            Self::WebProviders => "bring browser-facing or remote provider capabilities online.",
            Self::Channels => {
                "connect external chat surfaces and delivery channels after first run."
            }
            Self::Mcp => "continue capability/tool server setup inside the same shell.",
            Self::Personalization => "refine loong tone, depth, and first-turn behavior.",
        }
    }

    fn display_label(self, language: Language) -> &'static str {
        match language {
            Language::En => self.label(),
            Language::ZhCn => match self {
                Self::WebProviders => "网页 providers",
                Self::Channels => "渠道",
                Self::Mcp => "MCP",
                Self::Personalization => "个性化",
            },
            Language::ZhTw => match self {
                Self::WebProviders => "網頁 providers",
                Self::Channels => "渠道",
                Self::Mcp => "MCP",
                Self::Personalization => "個性化",
            },
            Language::Ja => match self {
                Self::WebProviders => "web providers",
                Self::Channels => "channels",
                Self::Mcp => "MCP",
                Self::Personalization => "personalization",
            },
            Language::Ru => match self {
                Self::WebProviders => "web providers",
                Self::Channels => "channels",
                Self::Mcp => "MCP",
                Self::Personalization => "personalization",
            },
        }
    }

    fn display_panel_detail(self, language: Language) -> &'static str {
        match language {
            Language::En => self.panel_detail(),
            Language::ZhCn => match self {
                Self::WebProviders => "把浏览器侧或远程 provider 能力接起来。",
                Self::Channels => "首轮后继续接外部聊天渠道与投递面。",
                Self::Mcp => "继续配置能力 / 工具服务器。",
                Self::Personalization => "继续调整语气、深度与首轮行为。",
            },
            Language::ZhTw => match self {
                Self::WebProviders => "把瀏覽器側或遠端 provider 能力接起來。",
                Self::Channels => "首輪後繼續接外部聊天渠道與投遞面。",
                Self::Mcp => "繼續配置能力 / 工具伺服器。",
                Self::Personalization => "繼續調整語氣、深度與首輪行為。",
            },
            Language::Ja => self.panel_detail(),
            Language::Ru => self.panel_detail(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum McpChoice {
    Filesystem,
    Fetch,
    Github,
}

impl McpChoice {
    fn label(self) -> &'static str {
        match self {
            Self::Filesystem => "filesystem",
            Self::Fetch => "fetch",
            Self::Github => "github",
        }
    }
}

const STARTUP_LANGUAGE_CHOICES: [Language; 5] = [
    Language::En,
    Language::ZhCn,
    Language::ZhTw,
    Language::Ja,
    Language::Ru,
];

fn default_startup_provider_choices(
    detected_provider_kind: Option<ProviderKind>,
) -> Vec<(ProviderKind, bool)> {
    let default_kind = detected_provider_kind.unwrap_or(ProviderKind::Openai);
    ProviderKind::all_sorted()
        .iter()
        .copied()
        .map(|kind| (kind, kind == default_kind))
        .collect()
}

#[derive(Clone)]
struct StartupOnboardingState {
    stage: StartupOnboardingStage,
    selection: usize,
    selected_language: Option<Language>,
    selected_provider: Option<ProviderKind>,
    provider_choices: Vec<(ProviderKind, bool)>,
    selected_personalization: Option<PersonalizationChoice>,
    calibrated_density: Option<ResponseDensity>,
    calibrated_initiative: Option<InitiativeLevel>,
    preferred_address: Option<String>,
    identity_name: Option<String>,
    identity_creature: Option<String>,
    identity_vibe: Option<String>,
    identity_emoji: Option<String>,
    skill_enabled: Vec<bool>,
    continue_setup_choices: Vec<(ContinueSetupChoice, bool)>,
    mcp_choices: Vec<(McpChoice, bool)>,
    completed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StartupOnboardingStage {
    Language,
    Provider,
    Skills,
    ContinueSetup,
    Mcp,
    Personalization,
    Finish,
}

impl StartupOnboardingState {
    fn new(optional_repo_skill_count: usize, detected_provider_kind: Option<ProviderKind>) -> Self {
        let provider_choices = default_startup_provider_choices(detected_provider_kind);
        Self {
            stage: StartupOnboardingStage::Language,
            selection: 0,
            selected_language: None,
            selected_provider: None,
            provider_choices,
            selected_personalization: None,
            calibrated_density: None,
            calibrated_initiative: None,
            preferred_address: None,
            identity_name: None,
            identity_creature: None,
            identity_vibe: None,
            identity_emoji: None,
            skill_enabled: vec![false; optional_repo_skill_count],
            continue_setup_choices: vec![
                (ContinueSetupChoice::WebProviders, false),
                (ContinueSetupChoice::Channels, false),
                (ContinueSetupChoice::Mcp, false),
                (ContinueSetupChoice::Personalization, true),
            ],
            mcp_choices: vec![
                (McpChoice::Filesystem, true),
                (McpChoice::Fetch, false),
                (McpChoice::Github, false),
            ],
            completed: false,
        }
    }

    pub(crate) fn option_count(&self) -> usize {
        match self.stage {
            StartupOnboardingStage::Language => STARTUP_LANGUAGE_CHOICES.len(),
            StartupOnboardingStage::Provider => self.provider_choices.len(),
            StartupOnboardingStage::Skills => {
                if self.skill_enabled.is_empty() {
                    1
                } else {
                    self.skill_enabled.len() + 2
                }
            }
            StartupOnboardingStage::ContinueSetup => self.continue_setup_choices.len(),
            StartupOnboardingStage::Mcp => self.mcp_choices.len() + 2,
            StartupOnboardingStage::Personalization => PersonalizationChoice::ALL.len(),
            StartupOnboardingStage::Finish => 2,
        }
    }

    fn move_selection(&mut self, delta: isize) {
        let total = self.option_count();
        if total == 0 {
            self.selection = 0;
            return;
        }
        let current = self.selection.min(total - 1) as isize;
        let wrapped = (current + delta).rem_euclid(total as isize);
        self.selection = wrapped as usize;
    }

    fn toggle_current_option(&mut self) -> bool {
        match self.stage {
            StartupOnboardingStage::Provider => {
                let Some((_, enabled)) = self.provider_choices.get_mut(self.selection) else {
                    return false;
                };
                *enabled = !*enabled;
                true
            }
            StartupOnboardingStage::Skills => {
                if self.skill_enabled.is_empty() {
                    false
                } else {
                    match self.selection {
                        0 => {
                            for enabled in &mut self.skill_enabled {
                                *enabled = true;
                            }
                            true
                        }
                        1 => {
                            for enabled in &mut self.skill_enabled {
                                *enabled = false;
                            }
                            true
                        }
                        index => {
                            let Some(enabled) = self.skill_enabled.get_mut(index.saturating_sub(2))
                            else {
                                return false;
                            };
                            *enabled = !*enabled;
                            true
                        }
                    }
                }
            }
            StartupOnboardingStage::ContinueSetup => {
                let Some((_, selected)) = self.continue_setup_choices.get_mut(self.selection)
                else {
                    return false;
                };
                *selected = !*selected;
                true
            }
            StartupOnboardingStage::Mcp => {
                toggle_meta_or_item_selection(&mut self.mcp_choices, self.selection)
            }
            StartupOnboardingStage::Language
            | StartupOnboardingStage::Personalization
            | StartupOnboardingStage::Finish => false,
        }
    }

    fn confirm_current(&mut self) {
        match self.stage {
            StartupOnboardingStage::Language => {
                self.selected_language = Some(startup_onboarding_language(self.selection));
                self.stage = StartupOnboardingStage::Provider;
                self.selection = self.preferred_provider_selection();
            }
            StartupOnboardingStage::Provider => {
                self.ensure_provider_selection();
                self.selected_provider = self.active_provider_choice();
                self.stage = if self.skill_enabled.is_empty() {
                    StartupOnboardingStage::ContinueSetup
                } else {
                    StartupOnboardingStage::Skills
                };
                self.selection = 0;
            }
            StartupOnboardingStage::Skills => {
                self.stage = StartupOnboardingStage::ContinueSetup;
                self.selection = 0;
            }
            StartupOnboardingStage::ContinueSetup => {
                self.stage = if self.is_continue_setup_enabled(ContinueSetupChoice::Mcp) {
                    StartupOnboardingStage::Mcp
                } else if self.is_continue_setup_enabled(ContinueSetupChoice::Personalization) {
                    StartupOnboardingStage::Personalization
                } else {
                    StartupOnboardingStage::Finish
                };
                self.selection = 0;
            }
            StartupOnboardingStage::Mcp => {
                self.stage = if self.is_continue_setup_enabled(ContinueSetupChoice::Personalization)
                {
                    StartupOnboardingStage::Personalization
                } else {
                    StartupOnboardingStage::Finish
                };
                self.selection = 0;
            }
            StartupOnboardingStage::Personalization => {
                self.selected_personalization =
                    Some(PersonalizationChoice::from_selection(self.selection));
                self.stage = StartupOnboardingStage::Finish;
                self.selection = 0;
            }
            StartupOnboardingStage::Finish => {
                self.completed = true;
            }
        }
    }

    fn go_back(&mut self) -> bool {
        match self.stage {
            StartupOnboardingStage::Language => false,
            StartupOnboardingStage::Provider => {
                self.stage = StartupOnboardingStage::Language;
                self.selection = self
                    .selected_language
                    .and_then(|language| {
                        STARTUP_LANGUAGE_CHOICES
                            .iter()
                            .position(|candidate| *candidate == language)
                    })
                    .unwrap_or(0);
                true
            }
            StartupOnboardingStage::Skills => {
                self.stage = StartupOnboardingStage::Provider;
                self.selection = self.preferred_provider_selection();
                true
            }
            StartupOnboardingStage::ContinueSetup => {
                self.stage = if self.skill_enabled.is_empty() {
                    StartupOnboardingStage::Provider
                } else {
                    StartupOnboardingStage::Skills
                };
                self.selection = match self.stage {
                    StartupOnboardingStage::Provider => self.preferred_provider_selection(),
                    StartupOnboardingStage::Skills => 0,
                    StartupOnboardingStage::Language
                    | StartupOnboardingStage::ContinueSetup
                    | StartupOnboardingStage::Mcp
                    | StartupOnboardingStage::Personalization
                    | StartupOnboardingStage::Finish => 0,
                };
                true
            }
            StartupOnboardingStage::Mcp => {
                self.stage = StartupOnboardingStage::ContinueSetup;
                self.selection = self
                    .continue_setup_choices
                    .iter()
                    .position(|(choice, _)| *choice == ContinueSetupChoice::Mcp)
                    .unwrap_or(0);
                true
            }
            StartupOnboardingStage::Personalization => {
                self.stage = if self.is_continue_setup_enabled(ContinueSetupChoice::Mcp) {
                    StartupOnboardingStage::Mcp
                } else {
                    StartupOnboardingStage::ContinueSetup
                };
                self.selection = match self.stage {
                    StartupOnboardingStage::Mcp => 0,
                    StartupOnboardingStage::ContinueSetup => self
                        .continue_setup_choices
                        .iter()
                        .position(|(choice, _)| *choice == ContinueSetupChoice::Personalization)
                        .unwrap_or(0),
                    StartupOnboardingStage::Language
                    | StartupOnboardingStage::Provider
                    | StartupOnboardingStage::Skills
                    | StartupOnboardingStage::Personalization
                    | StartupOnboardingStage::Finish => 0,
                };
                true
            }
            StartupOnboardingStage::Finish => {
                self.stage = if self.is_continue_setup_enabled(ContinueSetupChoice::Personalization)
                {
                    StartupOnboardingStage::Personalization
                } else if self.is_continue_setup_enabled(ContinueSetupChoice::Mcp) {
                    StartupOnboardingStage::Mcp
                } else {
                    StartupOnboardingStage::ContinueSetup
                };
                self.selection = match self.stage {
                    StartupOnboardingStage::Personalization => self
                        .selected_personalization
                        .map(|choice| {
                            PersonalizationChoice::ALL
                                .iter()
                                .position(|candidate| *candidate == choice)
                                .unwrap_or(0)
                        })
                        .unwrap_or(0),
                    StartupOnboardingStage::Mcp => 0,
                    StartupOnboardingStage::ContinueSetup => 0,
                    StartupOnboardingStage::Language
                    | StartupOnboardingStage::Provider
                    | StartupOnboardingStage::Skills
                    | StartupOnboardingStage::Finish => 0,
                };
                self.completed = false;
                true
            }
        }
    }

    fn focus(&self) -> StartupEyeFocus {
        match self.stage {
            StartupOnboardingStage::ContinueSetup => match self
                .continue_setup_choices
                .get(self.selection)
                .map(|(name, _)| *name)
            {
                Some(ContinueSetupChoice::WebProviders) => StartupEyeFocus::Right,
                Some(ContinueSetupChoice::Channels) => StartupEyeFocus::DownCenter,
                Some(ContinueSetupChoice::Mcp) => StartupEyeFocus::Left,
                Some(ContinueSetupChoice::Personalization) => StartupEyeFocus::Up,
                _ => StartupEyeFocus::DownCenter,
            },
            StartupOnboardingStage::Mcp => match self.option_count() {
                0 | 1 => StartupEyeFocus::Left,
                total => startup_palette_eye_focus(self.selection.min(total - 1), total),
            },
            StartupOnboardingStage::Personalization => match self.selection {
                0 => StartupEyeFocus::DownCenter,
                1 => StartupEyeFocus::Left,
                2 => StartupEyeFocus::Right,
                _ => StartupEyeFocus::Up,
            },
            StartupOnboardingStage::Language
            | StartupOnboardingStage::Provider
            | StartupOnboardingStage::Skills
            | StartupOnboardingStage::Finish => match self.option_count() {
                0 | 1 => StartupEyeFocus::DownCenter,
                total => startup_palette_eye_focus(self.selection.min(total - 1), total),
            },
        }
    }

    fn eye_animation(&self) -> StartupEyeAnimation {
        match self.stage {
            StartupOnboardingStage::Language
            | StartupOnboardingStage::Provider
            | StartupOnboardingStage::Skills
            | StartupOnboardingStage::ContinueSetup
            | StartupOnboardingStage::Mcp
            | StartupOnboardingStage::Personalization => StartupEyeAnimation::Focus(self.focus()),
            StartupOnboardingStage::Finish => StartupEyeAnimation::Confirm(self.focus()),
        }
    }

    fn preferred_provider_selection(&self) -> usize {
        self.provider_choices
            .iter()
            .position(|(_, enabled)| *enabled)
            .unwrap_or(0)
    }

    fn ensure_provider_selection(&mut self) {
        if self.provider_choices.iter().any(|(_, enabled)| *enabled) {
            return;
        }
        if let Some((_, enabled)) = self.provider_choices.get_mut(self.selection) {
            *enabled = true;
        }
    }

    fn active_provider_choice(&self) -> Option<ProviderKind> {
        self.provider_choices
            .get(self.selection)
            .filter(|(_, enabled)| *enabled)
            .map(|(kind, _)| *kind)
            .or_else(|| {
                self.provider_choices
                    .iter()
                    .find_map(|(kind, enabled)| (*enabled).then_some(*kind))
            })
    }

    fn panel(
        &self,
        optional_repo_skills: &[RepoOptionalSkill],
        language: Language,
    ) -> StartupPanel {
        match self.stage {
            StartupOnboardingStage::Language => self.language_panel(language),
            StartupOnboardingStage::Provider => self.provider_panel(language),
            StartupOnboardingStage::Skills => self.skills_panel(optional_repo_skills, language),
            StartupOnboardingStage::ContinueSetup => self.continue_setup_panel(language),
            StartupOnboardingStage::Mcp => self.mcp_panel(language),
            StartupOnboardingStage::Personalization => self.personalization_panel(language),
            StartupOnboardingStage::Finish => self.finish_panel(language),
        }
    }

    fn language_panel(&self, language: Language) -> StartupPanel {
        StartupPanel {
            title: localized_panel_title("language", language).to_owned(),
            hint: localized_panel_hint("language", language).to_owned(),
            options: STARTUP_LANGUAGE_CHOICES
                .iter()
                .enumerate()
                .map(|(index, language)| StartupPanelOption {
                    label: onboarding_language_label(*language).to_owned(),
                    detail: onboarding_language_panel_detail(*language).to_owned(),
                    selected: self.selection == index,
                })
                .collect(),
        }
    }

    fn provider_panel(&self, language: Language) -> StartupPanel {
        StartupPanel {
            title: localized_panel_title("provider", language).to_owned(),
            hint: localized_panel_hint("provider", language).to_owned(),
            options: self
                .provider_choices
                .iter()
                .enumerate()
                .map(|(index, (kind, enabled))| StartupPanelOption {
                    label: format!(
                        "{} {}",
                        if *enabled { "[x]" } else { "[ ]" },
                        kind.display_name()
                    ),
                    detail: provider_onboarding_brief(*kind),
                    selected: self.selection == index,
                })
                .collect(),
        }
    }

    fn skills_panel(
        &self,
        optional_repo_skills: &[RepoOptionalSkill],
        language: Language,
    ) -> StartupPanel {
        if self.skill_enabled.is_empty() {
            return StartupPanel {
                title: localized_panel_title("skills", language).to_owned(),
                hint: localized_skills_empty_hint(language).to_owned(),
                options: vec![StartupPanelOption {
                    label: localized_skills_empty_label(language).to_owned(),
                    detail: localized_skills_empty_detail(language).to_owned(),
                    selected: true,
                }],
            };
        }

        let all_selected = self.skill_enabled.iter().all(|enabled| *enabled);
        let none_selected = self.skill_enabled.iter().all(|enabled| !*enabled);
        let selected_count = self
            .skill_enabled
            .iter()
            .filter(|enabled| **enabled)
            .count();
        let mut options = vec![
            StartupPanelOption {
                label: format!(
                    "{} {}",
                    if all_selected { "[x]" } else { "[ ]" },
                    localized_meta_label("all", language)
                ),
                detail: localized_meta_detail("skills_all", language).to_owned(),
                selected: self.selection == 0,
            },
            StartupPanelOption {
                label: format!(
                    "{} {}",
                    if none_selected { "[x]" } else { "[ ]" },
                    localized_meta_label("skip_all", language)
                ),
                detail: localized_meta_detail("skills_skip_all", language).to_owned(),
                selected: self.selection == 1,
            },
        ];
        options.extend(
            optional_repo_skills
                .iter()
                .zip(self.skill_enabled.iter())
                .enumerate()
                .map(|(index, (skill, enabled))| {
                    let detail = if skill.description.trim().is_empty() {
                        localized_optional_skill_detail(language).to_owned()
                    } else {
                        compact_startup_detail(skill.description.as_str())
                    };
                    StartupPanelOption {
                        label: format!("{} {}", if *enabled { "[x]" } else { "[ ]" }, skill.name),
                        detail,
                        selected: self.selection == index + 2,
                    }
                }),
        );

        StartupPanel {
            title: localized_panel_title("skills", language).to_owned(),
            hint: localized_skills_hint(language, selected_count, self.skill_enabled.len()),
            options,
        }
    }

    fn continue_setup_panel(&self, language: Language) -> StartupPanel {
        StartupPanel {
            title: localized_panel_title("continue_setup", language).to_owned(),
            hint: localized_panel_hint("continue_setup", language).to_owned(),
            options: self
                .continue_setup_choices
                .iter()
                .enumerate()
                .map(|(index, (name, enabled))| StartupPanelOption {
                    label: format!(
                        "{} {}",
                        if *enabled { "[x]" } else { "[ ]" },
                        name.display_label(language)
                    ),
                    detail: name.display_panel_detail(language).to_owned(),
                    selected: self.selection == index,
                })
                .collect(),
        }
    }

    fn mcp_panel(&self, language: Language) -> StartupPanel {
        StartupPanel {
            title: localized_panel_title("mcp", language).to_owned(),
            hint: localized_panel_hint("mcp", language).to_owned(),
            options: render_meta_select_options(
                &self.mcp_choices,
                self.selection,
                |name| name.label().to_owned(),
                localized_meta_detail("mcp_all", language),
                localized_meta_detail("mcp_skip_all", language),
                language,
            ),
        }
    }

    fn personalization_panel(&self, language: Language) -> StartupPanel {
        StartupPanel {
            title: localized_panel_title("personalization", language).to_owned(),
            hint: localized_panel_hint("personalization", language).to_owned(),
            options: PersonalizationChoice::ALL
                .iter()
                .enumerate()
                .map(|(index, choice)| StartupPanelOption {
                    label: choice.display_label(language).to_owned(),
                    detail: choice.display_panel_detail(language).to_owned(),
                    selected: self.selection == index,
                })
                .collect(),
        }
    }

    fn finish_panel(&self, language: Language) -> StartupPanel {
        StartupPanel {
            title: localized_panel_title("finish", language).to_owned(),
            hint: localized_panel_hint("finish", language).to_owned(),
            options: vec![
                StartupPanelOption {
                    label: localized_finish_label("start_chatting", language).to_owned(),
                    detail: format!(
                        "{}: {} · {}: {} · {}: {} · MCP: {} · {}: {}",
                        localized_summary_label("language", language),
                        self.language_label_or_default(),
                        localized_summary_label("provider", language),
                        self.provider_label_or_default(),
                        localized_summary_label("continue_setup", language),
                        self.continue_setup_summary(),
                        self.mcp_summary(),
                        localized_summary_label("personalization", language),
                        self.personalization_summary_label()
                    ),
                    selected: self.selection == 0,
                },
                StartupPanelOption {
                    label: localized_finish_label("skip_rest", language).to_owned(),
                    detail: localized_finish_detail(language).to_owned(),
                    selected: self.selection == 1,
                },
            ],
        }
    }

    fn continue_setup_summary(&self) -> String {
        selection_summary(&self.continue_setup_choices, |choice| choice.label())
    }

    fn selected_continue_setup(&self) -> Vec<ContinueSetupChoice> {
        self.continue_setup_choices
            .iter()
            .filter_map(|(name, enabled)| (*enabled).then_some(*name))
            .collect()
    }

    pub(crate) fn is_continue_setup_enabled(&self, target: ContinueSetupChoice) -> bool {
        self.continue_setup_choices
            .iter()
            .any(|(name, enabled)| *name == target && *enabled)
    }

    fn mcp_summary(&self) -> String {
        selection_summary(&self.mcp_choices, |choice| choice.label())
    }

    fn language_label_or_default(&self) -> &'static str {
        self.selected_language
            .map(onboarding_language_label)
            .unwrap_or("default")
    }

    fn language_locale(&self) -> Option<&'static str> {
        self.selected_language.map(onboarding_language_locale)
    }

    fn selected_provider_kinds(&self) -> Vec<ProviderKind> {
        self.provider_choices
            .iter()
            .filter_map(|(kind, enabled)| (*enabled).then_some(*kind))
            .collect()
    }

    fn configured_provider_kinds(&self) -> Vec<ProviderKind> {
        let mut selected = self.selected_provider_kinds();
        if let Some(active) = self.selected_provider
            && !selected.contains(&active)
        {
            selected.insert(0, active);
        }
        selected
    }

    fn provider_label_or_default(&self) -> String {
        let selected = self.configured_provider_kinds();
        let Some(active) = self.selected_provider.or_else(|| selected.first().copied()) else {
            return "none".to_owned();
        };
        if selected.len() <= 1 {
            active.display_name().to_owned()
        } else {
            format!("{} +{}", active.display_name(), selected.len() - 1)
        }
    }

    fn personalization_summary_label(&self) -> &'static str {
        self.selected_personalization
            .map(PersonalizationChoice::summary_label)
            .unwrap_or("skip")
    }

    fn personalization_seed(&self) -> Option<PersonalizationConfig> {
        let selected = self.selected_personalization?;
        let (default_density, default_initiative) = selected.default_seed()?;
        Some(PersonalizationConfig {
            preferred_name: self.preferred_address.clone(),
            response_density: self.calibrated_density.or(Some(default_density)),
            initiative_level: self.calibrated_initiative.or(Some(default_initiative)),
            prompt_state: PersonalizationPromptState::Configured,
            locale: self.language_locale().map(str::to_owned),
            ..Default::default()
        })
    }
}

fn build_chat_startup_content(
    mcp_count: usize,
    skill_count: usize,
    i18n: &I18nService,
) -> (String, String, Vec<(String, Vec<String>)>, Vec<String>) {
    let version = format!("v{}", env!("CARGO_PKG_VERSION"));

    let tutorial = i18n.text(SurfaceCopy::Tutorial).to_owned();
    let sections = vec![
        (
            i18n.text(SurfaceCopy::StartupSectionSkills).to_owned(),
            vec![skill_count.to_string()],
        ),
        (
            i18n.text(SurfaceCopy::StartupSectionMcp).to_owned(),
            vec![mcp_count.to_string()],
        ),
    ];

    let tips = vec![
        tutorial.clone(),
        i18n.text(SurfaceCopy::StartupTipCommands).to_owned(),
        i18n.text(SurfaceCopy::StartupTipSkills).to_owned(),
        i18n.text(SurfaceCopy::StartupTipQueue).to_owned(),
        i18n.text(SurfaceCopy::StartupTipHistory).to_owned(),
    ];

    (version, tutorial, sections, tips)
}

fn startup_onboarding_language(selection: usize) -> Language {
    STARTUP_LANGUAGE_CHOICES
        .get(selection)
        .copied()
        .unwrap_or(Language::En)
}

fn onboarding_language_label(language: Language) -> &'static str {
    match language {
        Language::ZhCn => "中文",
        Language::En => "English",
        Language::ZhTw => "繁體中文",
        Language::Ja => "日本語",
        Language::Ru => "Русский",
    }
}

fn onboarding_language_panel_detail(language: Language) -> &'static str {
    match language {
        Language::ZhCn => "保留中文界面与引导。",
        Language::En => "keep the shell and guidance in English.",
        Language::ZhTw => "保留繁體中文介面與引導。",
        Language::Ja => "シェルとガイダンスを日本語で表示します。",
        Language::Ru => "оставить shell и подсказки на русском.",
    }
}

fn onboarding_language_locale(language: Language) -> &'static str {
    match language {
        Language::ZhCn => "zh-CN",
        Language::En => "en",
        Language::ZhTw => "zh-TW",
        Language::Ja => "ja",
        Language::Ru => "ru",
    }
}

fn compact_startup_detail(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn resolve_startup_copy_language(controller: &StartupOnboardingController) -> Language {
    controller
        .state
        .selected_language
        .unwrap_or_else(resolve_default_language)
}

fn localized_quickstart_title(language: Language) -> &'static str {
    match language {
        Language::En => "quick start",
        Language::ZhCn => "快速开始",
        Language::ZhTw => "快速開始",
        Language::Ja => "クイックスタート",
        Language::Ru => "быстрый старт",
    }
}

fn localized_quickstart_hint(language: Language) -> &'static str {
    match language {
        Language::En => "minimal onboard · one shell · loong reacts to the surface you are using.",
        Language::ZhCn => "极简上手 · 单一 shell · loong 会跟随你当前的操作面。",
        Language::ZhTw => "極簡上手 · 單一 shell · loong 會跟隨你目前的操作面。",
        Language::Ja => "最小オンボード · 単一シェル · 今使っている面に loong が追従します。",
        Language::Ru => {
            "минимальный старт · одна shell-среда · loong подстраивается под текущую поверхность."
        }
    }
}

fn localized_quickstart_label(focus: StartupQuickstartFocus, language: Language) -> &'static str {
    match language {
        Language::En => match focus {
            StartupQuickstartFocus::Chat => "start chatting",
            StartupQuickstartFocus::Commands => "browse commands",
            StartupQuickstartFocus::Skills => "pick a skill",
            StartupQuickstartFocus::Transcript => "inspect startup guidance",
        },
        Language::ZhCn => match focus {
            StartupQuickstartFocus::Chat => "开始聊天",
            StartupQuickstartFocus::Commands => "看命令",
            StartupQuickstartFocus::Skills => "选技能",
            StartupQuickstartFocus::Transcript => "看启动指引",
        },
        Language::ZhTw => match focus {
            StartupQuickstartFocus::Chat => "開始聊天",
            StartupQuickstartFocus::Commands => "看命令",
            StartupQuickstartFocus::Skills => "選技能",
            StartupQuickstartFocus::Transcript => "看啟動指引",
        },
        Language::Ja => match focus {
            StartupQuickstartFocus::Chat => "会話を始める",
            StartupQuickstartFocus::Commands => "コマンドを見る",
            StartupQuickstartFocus::Skills => "スキルを選ぶ",
            StartupQuickstartFocus::Transcript => "起動ガイドを見る",
        },
        Language::Ru => match focus {
            StartupQuickstartFocus::Chat => "начать чат",
            StartupQuickstartFocus::Commands => "посмотреть команды",
            StartupQuickstartFocus::Skills => "выбрать навык",
            StartupQuickstartFocus::Transcript => "посмотреть стартовую подсказку",
        },
    }
}

fn localized_quickstart_detail(focus: StartupQuickstartFocus, language: Language) -> &'static str {
    match language {
        Language::En => match focus {
            StartupQuickstartFocus::Chat => {
                "type directly in the composer and press enter when ready."
            }
            StartupQuickstartFocus::Commands => {
                "open / or : to inspect startup actions and operational entrypoints."
            }
            StartupQuickstartFocus::Skills => {
                "type $skill in the composer to expand installed skills inline."
            }
            StartupQuickstartFocus::Transcript => {
                "use j / k or pgup / pgdn to review the startup transcript and tips."
            }
        },
        Language::ZhCn => match focus {
            StartupQuickstartFocus::Chat => "直接输入，准备好后按 enter。",
            StartupQuickstartFocus::Commands => "用 / 或 : 看启动动作与入口。",
            StartupQuickstartFocus::Skills => "在输入框里输入 $skill 展开已装技能。",
            StartupQuickstartFocus::Transcript => "用 j / k 或 pgup / pgdn 回看启动提示。",
        },
        Language::ZhTw => match focus {
            StartupQuickstartFocus::Chat => "直接輸入，準備好後按 enter。",
            StartupQuickstartFocus::Commands => "用 / 或 : 看啟動動作與入口。",
            StartupQuickstartFocus::Skills => "在輸入框輸入 $skill 展開已裝技能。",
            StartupQuickstartFocus::Transcript => "用 j / k 或 pgup / pgdn 回看啟動提示。",
        },
        Language::Ja => match focus {
            StartupQuickstartFocus::Chat => "そのまま入力して、準備できたら enter。",
            StartupQuickstartFocus::Commands => "/ または : で起動コマンドを確認。",
            StartupQuickstartFocus::Skills => "composer で $skill を打ってインライン起動。",
            StartupQuickstartFocus::Transcript => "j / k または pgup / pgdn で起動ガイドを確認。",
        },
        Language::Ru => match focus {
            StartupQuickstartFocus::Chat => "печатайте сразу и нажмите enter, когда готовы.",
            StartupQuickstartFocus::Commands => {
                "откройте / или : чтобы посмотреть стартовые действия."
            }
            StartupQuickstartFocus::Skills => {
                "введите $skill в composer, чтобы раскрыть установленный навык."
            }
            StartupQuickstartFocus::Transcript => {
                "используйте j / k или pgup / pgdn для просмотра стартовых подсказок."
            }
        },
    }
}

fn localized_panel_title(panel: &str, language: Language) -> &'static str {
    match language {
        Language::En => match panel {
            "language" => "onboard · language",
            "provider" => "onboard · provider",
            "skills" => "onboard · skills",
            "continue_setup" => "onboard · continue setup",
            "mcp" => "onboard · MCP",
            "personalization" => "onboard · personalization",
            "finish" => "onboard · finish",
            _ => "onboard",
        },
        Language::ZhCn => match panel {
            "language" => "引导 · 语言",
            "provider" => "引导 · provider",
            "skills" => "引导 · 技能",
            "continue_setup" => "引导 · 后续配置",
            "mcp" => "引导 · MCP",
            "personalization" => "引导 · 个性化",
            "finish" => "引导 · 完成",
            _ => "引导",
        },
        Language::ZhTw => match panel {
            "language" => "引導 · 語言",
            "provider" => "引導 · provider",
            "skills" => "引導 · 技能",
            "continue_setup" => "引導 · 後續配置",
            "mcp" => "引導 · MCP",
            "personalization" => "引導 · 個性化",
            "finish" => "引導 · 完成",
            _ => "引導",
        },
        Language::Ja => match panel {
            "language" => "onboard · language",
            "provider" => "onboard · provider",
            "skills" => "onboard · skills",
            "continue_setup" => "onboard · continue setup",
            "mcp" => "onboard · MCP",
            "personalization" => "onboard · personalization",
            "finish" => "onboard · finish",
            _ => "onboard",
        },
        Language::Ru => match panel {
            "language" => "onboard · language",
            "provider" => "onboard · provider",
            "skills" => "onboard · skills",
            "continue_setup" => "onboard · continue setup",
            "mcp" => "onboard · MCP",
            "personalization" => "onboard · personalization",
            "finish" => "onboard · finish",
            _ => "onboard",
        },
    }
}

fn localized_panel_hint(panel: &str, language: Language) -> &'static str {
    match language {
        Language::En => match panel {
            "language" => "pick the shell language. enter continues.",
            "provider" => {
                "space toggles providers. enter seeds all checked providers and keeps the current row active."
            }
            "continue_setup" => "space toggles follow-up lanes. enter continues.",
            "mcp" => "space toggles MCP capabilities. enter continues.",
            "personalization" => "pick the default feel after the first turn. enter continues.",
            "finish" => "loong is ready. chat now, or continue the deeper setup surfaces later.",
            _ => "",
        },
        Language::ZhCn => match panel {
            "language" => "先选 shell 语言，按 enter 继续。",
            "provider" => "space 勾选 provider，enter 会写入所有勾选项，并把当前行设为 active。",
            "continue_setup" => "space 勾选后续配置项，enter 继续。",
            "mcp" => "space 勾选 MCP 能力，enter 继续。",
            "personalization" => "选择首轮之后的默认风格，enter 继续。",
            "finish" => "loong 已就绪，现在开聊，或稍后继续更深配置。",
            _ => "",
        },
        Language::ZhTw => match panel {
            "language" => "先選 shell 語言，按 enter 繼續。",
            "provider" => "space 勾選 provider，enter 會寫入所有勾選項，並把目前列設為 active。",
            "continue_setup" => "space 勾選後續配置項，enter 繼續。",
            "mcp" => "space 勾選 MCP 能力，enter 繼續。",
            "personalization" => "選擇首輪之後的預設風格，enter 繼續。",
            "finish" => "loong 已就緒，現在開聊，或稍後繼續更深配置。",
            _ => "",
        },
        Language::Ja => match panel {
            "language" => "shell 言語を選び、enter で続行します。",
            "provider" => "space で provider を切り替え、enter で選択済みを seed します。",
            "continue_setup" => "space で後続セットアップを選び、enter で続行します。",
            "mcp" => "space で MCP 能力を切り替え、enter で続行します。",
            "personalization" => "最初の会話後の既定スタイルを選び、enter で続行します。",
            "finish" => "loong の準備ができました。今すぐ会話するか、後で深い設定を続けられます。",
            _ => "",
        },
        Language::Ru => match panel {
            "language" => "выберите язык shell и нажмите enter.",
            "provider" => {
                "space переключает provider, enter сохраняет отмеченные и делает текущую строку active."
            }
            "continue_setup" => "space отмечает следующие шаги, enter продолжает.",
            "mcp" => "space переключает MCP возможности, enter продолжает.",
            "personalization" => "выберите стиль после первого хода и нажмите enter.",
            "finish" => {
                "loong готов. можно сразу начать чат или позже продолжить глубокую настройку."
            }
            _ => "",
        },
    }
}

fn localized_meta_label(kind: &str, language: Language) -> &'static str {
    match language {
        Language::En => match kind {
            "all" => "all",
            "skip_all" => "skip all",
            _ => "",
        },
        Language::ZhCn => match kind {
            "all" => "全选",
            "skip_all" => "全不选",
            _ => "",
        },
        Language::ZhTw => match kind {
            "all" => "全選",
            "skip_all" => "全不選",
            _ => "",
        },
        Language::Ja => match kind {
            "all" => "all",
            "skip_all" => "skip all",
            _ => "",
        },
        Language::Ru => match kind {
            "all" => "all",
            "skip_all" => "skip all",
            _ => "",
        },
    }
}

fn localized_meta_detail(kind: &str, language: Language) -> &'static str {
    match language {
        Language::En => match kind {
            "skills_all" => "select every optional repo skill.",
            "skills_skip_all" => "leave optional repo skills uninstalled for now.",
            "mcp_all" => "continue capability/tool server setup inside the same shell.",
            "mcp_skip_all" => "let loong set that for you later",
            _ => "",
        },
        Language::ZhCn => match kind {
            "skills_all" => "选中当前区块里的所有可选 repo skill。",
            "skills_skip_all" => "先不安装这些可选 repo skill。",
            "mcp_all" => "继续在当前 shell 里接能力 / 工具服务器。",
            "mcp_skip_all" => "稍后再让 loong 帮你设置",
            _ => "",
        },
        Language::ZhTw => match kind {
            "skills_all" => "選中目前區塊裡的所有可選 repo skill。",
            "skills_skip_all" => "先不安裝這些可選 repo skill。",
            "mcp_all" => "繼續在目前 shell 裡接能力 / 工具伺服器。",
            "mcp_skip_all" => "稍後再讓 loong 幫你設定",
            _ => "",
        },
        Language::Ja => localized_meta_detail(kind, Language::En),
        Language::Ru => localized_meta_detail(kind, Language::En),
    }
}

fn localized_optional_skill_detail(language: Language) -> &'static str {
    match language {
        Language::En => "optional repo skill",
        Language::ZhCn => "可选 repo skill",
        Language::ZhTw => "可選 repo skill",
        Language::Ja => "optional repo skill",
        Language::Ru => "optional repo skill",
    }
}

fn localized_skills_empty_hint(language: Language) -> &'static str {
    match language {
        Language::En => "no optional repo skills are available in this workspace right now.",
        Language::ZhCn => "当前 workspace 里没有可选 repo skill。",
        Language::ZhTw => "目前 workspace 裡沒有可選 repo skill。",
        Language::Ja => "この workspace には optional repo skill がありません。",
        Language::Ru => "в этом workspace сейчас нет optional repo skill.",
    }
}

fn localized_skills_empty_label(language: Language) -> &'static str {
    match language {
        Language::En => "no optional repo skills detected",
        Language::ZhCn => "未发现可选 repo skill",
        Language::ZhTw => "未發現可選 repo skill",
        Language::Ja => "optional repo skill は見つかりません",
        Language::Ru => "optional repo skill не найден",
    }
}

fn localized_skills_empty_detail(language: Language) -> &'static str {
    match language {
        Language::En => {
            "bundled mandatory skills stay available without appearing in this selector."
        }
        Language::ZhCn => "内置 mandatory skill 仍然可用，只是不出现在这里。",
        Language::ZhTw => "內建 mandatory skill 仍然可用，只是不會出現在這裡。",
        Language::Ja => "bundled mandatory skill は利用可能ですが、ここには表示されません。",
        Language::Ru => "bundled mandatory skill остаются доступными, но не показываются здесь.",
    }
}

fn localized_skills_hint(language: Language, selected_count: usize, total_count: usize) -> String {
    match language {
        Language::En => {
            format!("space toggles. {selected_count} of {total_count} selected. enter continues.")
        }
        Language::ZhCn => format!("space 勾选。已选 {selected_count}/{total_count}。enter 继续。"),
        Language::ZhTw => format!("space 勾選。已選 {selected_count}/{total_count}。enter 繼續。"),
        Language::Ja => format!(
            "space で切り替え。{total_count} 件中 {selected_count} 件を選択。enter で続行。"
        ),
        Language::Ru => format!(
            "space переключает. выбрано {selected_count} из {total_count}. enter продолжает."
        ),
    }
}

fn localized_finish_label(kind: &str, language: Language) -> &'static str {
    match language {
        Language::En => match kind {
            "start_chatting" => "start chatting",
            "skip_rest" => "skip the rest for now",
            _ => "",
        },
        Language::ZhCn => match kind {
            "start_chatting" => "开始聊天",
            "skip_rest" => "先跳过剩下这些",
            _ => "",
        },
        Language::ZhTw => match kind {
            "start_chatting" => "開始聊天",
            "skip_rest" => "先跳過剩下這些",
            _ => "",
        },
        Language::Ja => match kind {
            "start_chatting" => "会話を始める",
            "skip_rest" => "残りは後で",
            _ => "",
        },
        Language::Ru => match kind {
            "start_chatting" => "начать чат",
            "skip_rest" => "остальное позже",
            _ => "",
        },
    }
}

fn localized_finish_detail(language: Language) -> &'static str {
    match language {
        Language::En => {
            "web providers, channels, MCP, and personalization can continue after the first turn."
        }
        Language::ZhCn => "web providers、channels、MCP 和个性化都可以在首轮之后继续。",
        Language::ZhTw => "web providers、channels、MCP 與個性化都可以在首輪之後繼續。",
        Language::Ja => {
            "web providers、channels、MCP、personalization は最初の会話後でも続けられます。"
        }
        Language::Ru => {
            "web providers, channels, MCP и personalization можно продолжить после первого хода."
        }
    }
}

fn localized_summary_label(kind: &str, language: Language) -> &'static str {
    match language {
        Language::En => match kind {
            "language" => "language",
            "provider" => "provider",
            "continue_setup" => "continue",
            "personalization" => "personalization",
            _ => "",
        },
        Language::ZhCn => match kind {
            "language" => "语言",
            "provider" => "provider",
            "continue_setup" => "后续",
            "personalization" => "个性化",
            _ => "",
        },
        Language::ZhTw => match kind {
            "language" => "語言",
            "provider" => "provider",
            "continue_setup" => "後續",
            "personalization" => "個性化",
            _ => "",
        },
        Language::Ja => match kind {
            "language" => "language",
            "provider" => "provider",
            "continue_setup" => "continue",
            "personalization" => "personalization",
            _ => "",
        },
        Language::Ru => match kind {
            "language" => "language",
            "provider" => "provider",
            "continue_setup" => "continue",
            "personalization" => "personalization",
            _ => "",
        },
    }
}

fn localized_channel_label(id: &str, fallback_label: &'static str, language: Language) -> String {
    match language {
        Language::ZhCn => match id {
            "feishu" => "飞书".to_owned(),
            "dingtalk" => "钉钉".to_owned(),
            "wecom" => "企业微信".to_owned(),
            "weixin" => "微信".to_owned(),
            _ => fallback_label.to_owned(),
        },
        Language::ZhTw => match id {
            "feishu" => "飛書".to_owned(),
            "dingtalk" => "釘釘".to_owned(),
            "wecom" => "企業微信".to_owned(),
            "weixin" => "微信".to_owned(),
            _ => fallback_label.to_owned(),
        },
        Language::En | Language::Ja | Language::Ru => fallback_label.to_owned(),
    }
}

fn localized_follow_up_heading(kind: &str, language: Language) -> &'static str {
    match language {
        Language::En => match kind {
            "onboarding_complete" => "Onboarding complete",
            "language" => "Language",
            "provider" => "Provider",
            "personalization" => "Personalization",
            "continue_setup" => "Continue setup",
            "suggested_next_asks" => "Suggested next asks:",
            "later" => "later",
            "calibration_updated" => "First-turn calibration updated",
            "response_density" => "response density",
            "initiative" => "initiative",
            "preferred_address" => "preferred address",
            "agent_name" => "agent name",
            "creature" => "creature",
            "vibe" => "vibe",
            "emoji" => "emoji",
            _ => "",
        },
        Language::ZhCn => match kind {
            "onboarding_complete" => "引导已完成",
            "language" => "语言",
            "provider" => "provider",
            "personalization" => "个性化",
            "continue_setup" => "后续配置",
            "suggested_next_asks" => "建议下一步可以这样说：",
            "later" => "稍后",
            "calibration_updated" => "首轮校准已更新",
            "response_density" => "响应密度",
            "initiative" => "主动性",
            "preferred_address" => "称呼偏好",
            "agent_name" => "agent 名字",
            "creature" => "身份设定",
            "vibe" => "气质",
            "emoji" => "emoji",
            _ => "",
        },
        Language::ZhTw => match kind {
            "onboarding_complete" => "引導已完成",
            "language" => "語言",
            "provider" => "provider",
            "personalization" => "個性化",
            "continue_setup" => "後續配置",
            "suggested_next_asks" => "建議下一步可以這樣說：",
            "later" => "稍後",
            "calibration_updated" => "首輪校準已更新",
            "response_density" => "回應密度",
            "initiative" => "主動性",
            "preferred_address" => "稱呼偏好",
            "agent_name" => "agent 名字",
            "creature" => "身份設定",
            "vibe" => "氣質",
            "emoji" => "emoji",
            _ => "",
        },
        Language::Ja => match kind {
            "onboarding_complete" => "セットアップ完了",
            "language" => "言語",
            "provider" => "provider",
            "personalization" => "personalization",
            "continue_setup" => "続きの設定",
            "suggested_next_asks" => "次にこんな聞き方ができます：",
            "later" => "あとで",
            "calibration_updated" => "初回キャリブレーションを更新しました",
            "response_density" => "応答の密度",
            "initiative" => "主体性",
            "preferred_address" => "呼び方",
            "agent_name" => "agent 名",
            "creature" => "キャラクター",
            "vibe" => "雰囲気",
            "emoji" => "emoji",
            _ => "",
        },
        Language::Ru => match kind {
            "onboarding_complete" => "Онбординг завершён",
            "language" => "Язык",
            "provider" => "provider",
            "personalization" => "personalization",
            "continue_setup" => "Дальше настроить",
            "suggested_next_asks" => "Дальше можно сказать так:",
            "later" => "позже",
            "calibration_updated" => "Калибровка первого хода обновлена",
            "response_density" => "плотность ответа",
            "initiative" => "инициативность",
            "preferred_address" => "как обращаться",
            "agent_name" => "имя agent",
            "creature" => "образ",
            "vibe" => "вайб",
            "emoji" => "emoji",
            _ => "",
        },
    }
}

fn detect_onboarding_provider_kind(config_path: Option<&Path>) -> Option<ProviderKind> {
    let config_path = config_path?;
    let config_path_string = config_path.display().to_string();
    let (_, config) = crate::config::load(Some(config_path_string.as_str())).ok()?;
    Some(config.provider.kind)
}

fn toggle_meta_or_item_selection<T>(options: &mut [(T, bool)], selection: usize) -> bool {
    match selection {
        0 => {
            for (_, enabled) in options.iter_mut() {
                *enabled = true;
            }
            true
        }
        1 => {
            for (_, enabled) in options.iter_mut() {
                *enabled = false;
            }
            true
        }
        index => {
            let Some((_, enabled)) = options.get_mut(index.saturating_sub(2)) else {
                return false;
            };
            *enabled = !*enabled;
            true
        }
    }
}

fn render_meta_select_options<T>(
    options: &[(T, bool)],
    selection: usize,
    label: impl Fn(&T) -> String,
    item_detail: &str,
    skip_all_detail: &str,
    language: Language,
) -> Vec<StartupPanelOption> {
    let all_selected = options.iter().all(|(_, enabled)| *enabled);
    let none_selected = options.iter().all(|(_, enabled)| !*enabled);
    let mut rendered = vec![
        StartupPanelOption {
            label: format!(
                "{} {}",
                if all_selected { "[x]" } else { "[ ]" },
                localized_meta_label("all", language)
            ),
            detail: item_detail.to_owned(),
            selected: selection == 0,
        },
        StartupPanelOption {
            label: format!(
                "{} {}",
                if none_selected { "[x]" } else { "[ ]" },
                localized_meta_label("skip_all", language)
            ),
            detail: skip_all_detail.to_owned(),
            selected: selection == 1,
        },
    ];
    rendered.extend(options.iter().enumerate().map(|(index, (name, enabled))| {
        StartupPanelOption {
            label: format!("{} {}", if *enabled { "[x]" } else { "[ ]" }, label(name)),
            detail: item_detail.to_owned(),
            selected: selection == index + 2,
        }
    }));
    rendered
}

fn selection_summary<T>(options: &[(T, bool)], label: impl Fn(&T) -> &'static str) -> String {
    let selected = options
        .iter()
        .filter_map(|(name, enabled)| (*enabled).then_some(label(name)))
        .collect::<Vec<_>>();
    if selected.is_empty() {
        "later".to_owned()
    } else if selected.len() == options.len() {
        "all".to_owned()
    } else {
        selected.join(", ")
    }
}

fn render_startup_onboarding_follow_up_lines(state: &StartupOnboardingState) -> Vec<String> {
    let language = state.selected_language.unwrap_or(Language::En);
    let mut lines = vec![
        localized_follow_up_heading("onboarding_complete", language).to_owned(),
        format!(
            "{}: {}",
            localized_follow_up_heading("language", language),
            state.language_label_or_default()
        ),
        format!(
            "{}: {}",
            localized_follow_up_heading("provider", language),
            state.provider_label_or_default()
        ),
        format!(
            "{}: {}",
            localized_follow_up_heading("personalization", language),
            state.personalization_summary_label()
        ),
    ];

    let selected_continue_setup = state.selected_continue_setup();
    if selected_continue_setup.is_empty() {
        lines.push(format!(
            "{}: {}",
            localized_follow_up_heading("continue_setup", language),
            localized_follow_up_heading("later", language)
        ));
        return lines;
    }

    lines.push(format!(
        "{}: {}",
        localized_follow_up_heading("continue_setup", language),
        selected_continue_setup
            .iter()
            .map(|choice| choice.label())
            .collect::<Vec<_>>()
            .join(", ")
    ));
    lines.push(localized_follow_up_heading("suggested_next_asks", language).to_owned());

    for item in selected_continue_setup {
        lines.extend(item.follow_up_lines(state));
    }

    if let Some(personalization) = state.personalization_seed() {
        lines.extend(render_personalization_seed_lines(&personalization));
    }

    lines
}

fn startup_channel_examples_and_command(language: Language) -> (String, Option<&'static str>) {
    let descriptors = service_channel_descriptors()
        .into_iter()
        .filter(|descriptor| descriptor.id != "cli")
        .collect::<Vec<_>>();
    let labels = descriptors
        .iter()
        .map(|descriptor| localized_channel_label(descriptor.id, descriptor.label, language))
        .take(4)
        .collect::<Vec<_>>();
    let command = descriptors
        .iter()
        .find_map(|descriptor| descriptor.serve_subcommand);
    if labels.is_empty() {
        ("telegram, discord, slack".to_owned(), None)
    } else {
        (labels.join(", "), command)
    }
}

impl ContinueSetupChoice {
    fn follow_up_lines(self, state: &StartupOnboardingState) -> Vec<String> {
        let language = state.language_label_or_default();
        let copy_language = state.selected_language.unwrap_or(Language::En);
        match self {
            Self::WebProviders => {
                let selected_kinds = state.configured_provider_kinds();
                let active_kind = state
                    .selected_provider
                    .or_else(|| selected_kinds.first().copied());
                let mut lines = vec![
                    "• web providers".to_owned(),
                    "  surface: /model".to_owned(),
                    format!("  provider seed: {}", state.provider_label_or_default()),
                ];
                if let Some(kind) = active_kind {
                    lines.push(format!("  active row: {}", kind.display_name()));
                    if let Some(env_name) = kind.default_api_key_env() {
                        lines.push(format!("  env: {env_name}"));
                    }
                    if let Some(hint) = kind.configuration_hint() {
                        lines.push(format!("  hint: {hint}"));
                    }
                    lines.push(format!(
                        "  ask: help me finish {} provider setup in this shell",
                        kind.display_name()
                    ));
                } else {
                    lines.push(
                        "  ask: help me continue web provider setup in this shell".to_owned(),
                    );
                }
                lines
            }
            Self::Channels => {
                let (labels, command) = startup_channel_examples_and_command(copy_language);
                let mut lines = vec![
                    "• channels".to_owned(),
                    format!("  examples: {labels}"),
                    "  ask: help me continue channel setup after first run".to_owned(),
                ];
                if let Some(command) = command {
                    lines.push(format!("  runtime: {command}"));
                }
                lines
            }
            Self::Mcp => vec![
                "• MCP".to_owned(),
                "  surface: /mcp".to_owned(),
                format!(
                    "  current provider choice: {}",
                    state.provider_label_or_default()
                ),
                format!("  selected MCP scope: {}", state.mcp_summary()),
                "  ask: help me continue MCP setup from this shell".to_owned(),
            ],
            Self::Personalization => vec![
                "• personalization".to_owned(),
                "  surfaces: /language, /model".to_owned(),
                format!("  current language choice: {language}"),
                format!(
                    "  chosen personality: {}",
                    state.personalization_summary_label()
                ),
                if state
                    .selected_personalization
                    .is_some_and(PersonalizationChoice::suppresses_personalization)
                {
                    "  ask: help me re-enable personalization later if I want style guidance"
                        .to_owned()
                } else {
                    "  ask: help me personalize loong for future conversations".to_owned()
                },
            ],
        }
    }
}

fn render_first_turn_calibration_prompt(state: &StartupOnboardingState) -> Option<String> {
    let personalization = state.selected_personalization?;
    if personalization.skips_calibration() {
        return None;
    }

    let language = state.selected_language.unwrap_or(Language::En);
    Some(match language {
        Language::En => match personalization {
            PersonalizationChoice::Concise => "Before we get going, I’d love one quick calibration so I can fit you better: how should I address you, should I stay concise by default, and when setup is incomplete would you rather I ask first or keep moving until there’s a real blocker? If you want, you can also give me a name, creature, vibe, and emoji. Reply once and I’ll adapt from the next turn.".to_owned(),
            PersonalizationChoice::Thorough => "Before we get going, I’d love one quick calibration so I can fit you better: how should I address you, should I stay thorough by default, and when there are several good paths would you rather see a brief comparison first or have me choose directly? If you want, you can also give me a name, creature, vibe, and emoji. Reply once and I’ll adapt from the next turn.".to_owned(),
            PersonalizationChoice::Balanced | PersonalizationChoice::Skip | PersonalizationChoice::TurnOff => "Before we get going, I’d love one quick calibration so I can fit you better: how should I address you, should I stay balanced by default or lean a bit more concise / more thorough, and when setup is incomplete would you rather I ask first or keep moving until there’s a real blocker? If you want, you can also give me a name, creature, vibe, and emoji. Reply once and I’ll adapt from the next turn.".to_owned(),
        },
        Language::ZhCn => match personalization {
            PersonalizationChoice::Concise => "正式开始前，我先和你对齐一下：我应该怎么称呼你？你更希望我默认简洁一点吗？如果配置还没补完，你更希望我先问你，还是先继续推进到真的卡住为止？如果你愿意，也可以顺手给我一个名字、身份设定、气质和 emoji。你回我一条，我就会从下一轮开始适配。".to_owned(),
            PersonalizationChoice::Thorough => "正式开始前，我先和你对齐一下：我应该怎么称呼你？你更希望我默认深入一点吗？如果有多条可行路径，你更想先看一个简短对比，还是让我直接选一个最合适的方案？如果你愿意，也可以顺手给我一个名字、身份设定、气质和 emoji。你回我一条，我就会从下一轮开始适配。".to_owned(),
            PersonalizationChoice::Balanced | PersonalizationChoice::Skip | PersonalizationChoice::TurnOff => "正式开始前，我先和你对齐一下：我应该怎么称呼你？你更希望我默认保持平衡，还是稍微偏简洁 / 偏深入？如果配置还没补完，你更希望我先问你，还是先继续推进到真的卡住为止？如果你愿意，也可以顺手给我一个名字、身份设定、气质和 emoji。你回我一条，我就会从下一轮开始适配。".to_owned(),
        },
        Language::ZhTw => match personalization {
            PersonalizationChoice::Concise => "正式開始前，我先和你對齊一下：我應該怎麼稱呼你？你希望我預設更精簡一點嗎？如果配置還沒補完，你比較希望我先問你，還是先推進到真的卡住為止？如果你願意，也可以順手給我一個名字、身份設定、氣質和 emoji。你回我一則，我就會從下一輪開始適配。".to_owned(),
            PersonalizationChoice::Thorough => "正式開始前，我先和你對齊一下：我應該怎麼稱呼你？你希望我預設更深入一點嗎？如果有多條可行路徑，你想先看一個簡短比較，還是讓我直接選一個最合適的方案？如果你願意，也可以順手給我一個名字、身份設定、氣質和 emoji。你回我一則，我就會從下一輪開始適配。".to_owned(),
            PersonalizationChoice::Balanced | PersonalizationChoice::Skip | PersonalizationChoice::TurnOff => "正式開始前，我先和你對齊一下：我應該怎麼稱呼你？你希望我預設保持平衡，還是稍微偏精簡 / 偏深入？如果配置還沒補完，你比較希望我先問你，還是先推進到真的卡住為止？如果你願意，也可以順手給我一個名字、身份設定、氣質和 emoji。你回我一則，我就會從下一輪開始適配。".to_owned(),
        },
        Language::Ja => match personalization {
            PersonalizationChoice::Concise => "始める前に、一つだけ合わせさせてください。どう呼べばよいですか？ 普段は簡潔寄りのほうがいいですか？ それと、設定がまだ途中の時は先に確認したほうがいいですか、それとも本当に詰まるまで進めたほうがいいですか。よければ私の名前や雰囲気、絵文字も決めてもらえるとうれしいです。ひとこと返してくれれば、次のターンから合わせます。".to_owned(),
            PersonalizationChoice::Thorough => "始める前に、一つだけ合わせさせてください。どう呼べばよいですか？ 普段は詳しめのほうがいいですか？ それと、選択肢が複数ある時は短い比較を先に見たいですか、それとも私がそのまま選んだほうがいいですか。よければ私の名前や雰囲気、絵文字も決めてもらえるとうれしいです。ひとこと返してくれれば、次のターンから合わせます。".to_owned(),
            PersonalizationChoice::Balanced | PersonalizationChoice::Skip | PersonalizationChoice::TurnOff => "始める前に、一つだけ合わせさせてください。どう呼べばよいですか？ 普段はバランスのままがいいですか、それとも少し簡潔寄り / 詳細寄りにしたいですか。設定がまだ途中の時は先に確認したほうがいいですか、それとも本当に詰まるまで進めたほうがいいですか。よければ私の名前や雰囲気、絵文字も決めてもらえるとうれしいです。ひとこと返してくれれば、次のターンから合わせます。".to_owned(),
        },
        Language::Ru => match personalization {
            PersonalizationChoice::Concise => "Перед началом хочу уточнить один момент, чтобы лучше подстроиться под тебя: как мне к тебе обращаться, оставить меня по умолчанию более кратким, и если настройка ещё не закончена — мне лучше сначала спросить тебя или спокойно двигаться дальше до реального блокера? Если хочешь, можно заодно выбрать мне имя, образ, вайб и emoji. Ответь одним сообщением, и со следующего хода я подстроюсь.".to_owned(),
            PersonalizationChoice::Thorough => "Перед началом хочу уточнить один момент, чтобы лучше подстроиться под тебя: как мне к тебе обращаться, оставить меня по умолчанию более подробным, и если есть несколько хороших путей — тебе приятнее сначала видеть короткое сравнение или мне сразу выбрать самый уместный вариант? Если хочешь, можно заодно выбрать мне имя, образ, вайб и emoji. Ответь одним сообщением, и со следующего хода я подстроюсь.".to_owned(),
            PersonalizationChoice::Balanced | PersonalizationChoice::Skip | PersonalizationChoice::TurnOff => "Перед началом хочу уточнить один момент, чтобы лучше подстроиться под тебя: как мне к тебе обращаться, оставить меня по умолчанию сбалансированным или чуть сместить в сторону краткости / подробности, и если настройка ещё не закончена — мне лучше сначала спросить тебя или спокойно двигаться дальше до реального блокера? Если хочешь, можно заодно выбрать мне имя, образ, вайб и emoji. Ответь одним сообщением, и со следующего хода я подстроюсь.".to_owned(),
        },
    })
}

fn render_first_turn_calibration_system_addendum(state: &StartupOnboardingState) -> Option<String> {
    let language = state.selected_language.unwrap_or(Language::En);
    let user_prompt = render_first_turn_calibration_prompt(state)?;
    Some(match language {
        Language::En => format!(
            "For your next reply only, do not start the main task yet. First send one short, warm calibration message that asks about address preference plus response style. Cover: how to address the user, whether to stay balanced / concise / thorough, and whether to ask first or keep moving until a real blocker. If it feels natural, also invite a quick self-identity choice for your name, creature, vibe, and emoji. Keep it conversational, not checklist-like, and stay under 120 words.\n\nUse this content as the target shape:\n{user_prompt}"
        ),
        Language::ZhCn => format!(
            "仅限你的下一条回复：先不要进入主任务，先发一条简短、友善、自然的校准消息。要覆盖：怎么称呼用户、默认风格更平衡 / 更简洁 / 更深入、以及遇到未完成配置时是先问还是先推进到真实阻塞。如果自然的话，也可以顺带邀请用户给你一个名字、身份设定、气质、emoji。不要写成 checklist，也不要提什么“语气风格要求”。控制在 120 字以内。\n\n目标内容参考：\n{user_prompt}"
        ),
        Language::ZhTw => format!(
            "僅限你的下一條回覆：先不要進入主任務，先發一條簡短、友善、自然的校準訊息。要涵蓋：怎麼稱呼使用者、預設風格更平衡 / 更精簡 / 更深入、以及遇到未完成配置時是先問還是先推進到真實阻塞。如果自然的話，也可以順帶邀請使用者給你一個名字、身份設定、氣質、emoji。不要寫成 checklist，也不要直接講什麼「語氣風格要求」。控制在 120 字內。\n\n目標內容參考：\n{user_prompt}"
        ),
        Language::Ja => format!(
            "次の返信だけは、まだ本題に入らず、短く親しみのある調整メッセージを返してください。内容は、どう呼べばよいか、既定の応答スタイル（balanced / concise / thorough）、未完の設定にぶつかった時は先に聞くか本当の blocker まで進めるか、を自然に聞いてください。余裕があれば自分の name / creature / vibe / emoji も軽く尋ねてください。チェックリスト調にはせず、120語以内にしてください。\n\n狙いの内容:\n{user_prompt}"
        ),
        Language::Ru => format!(
            "Только для следующего ответа: пока не переходи к основной задаче. Сначала отправь короткое, дружелюбное и естественное сообщение для калибровки. Уточни, как обращаться к пользователю, какой стиль ответа держать по умолчанию (balanced / concise / thorough), и при незавершённой настройке лучше сначала спрашивать или двигаться до реального blocker. Если уместно, мягко предложи выбрать тебе name / creature / vibe / emoji. Не превращай это в чеклист и уложись примерно в 120 слов.\n\nЦелевой смысл:\n{user_prompt}"
        ),
    })
}

#[derive(Clone, Copy)]
struct FirstTurnCalibrationUpdate {
    pub(crate) choice: Option<PersonalizationChoice>,
    pub(crate) density: Option<ResponseDensity>,
    pub(crate) initiative: Option<InitiativeLevel>,
}

#[derive(Default)]
struct FirstTurnIdentityUpdate {
    preferred_address: Option<String>,
    identity_name: Option<String>,
    identity_creature: Option<String>,
    identity_vibe: Option<String>,
    identity_emoji: Option<String>,
}

fn infer_first_turn_calibration_update(
    message: &str,
    fallback_choice: Option<PersonalizationChoice>,
) -> FirstTurnCalibrationUpdate {
    let normalized = message.to_ascii_lowercase();
    let matches_any =
        |patterns: &[&str]| patterns.iter().any(|pattern| normalized.contains(pattern));

    let density = if matches_any(&["concise", "brief", "short answer", "shorter", "terse"]) {
        Some(ResponseDensity::Concise)
    } else if matches_any(&[
        "thorough",
        "detailed",
        "detail-heavy",
        "deep",
        "deeper",
        "tradeoff",
        "comprehensive",
    ]) {
        Some(ResponseDensity::Thorough)
    } else if matches_any(&["balanced", "default", "normal"]) {
        Some(ResponseDensity::Balanced)
    } else {
        None
    };

    let initiative = if matches_any(&[
        "ask first",
        "ask before acting",
        "ask before act",
        "check with me first",
    ]) {
        Some(InitiativeLevel::AskBeforeActing)
    } else if matches_any(&[
        "make a reasonable assumption",
        "make reasonable assumptions",
        "move fast",
        "just move",
        "go ahead",
        "choose directly",
        "decide directly",
    ]) {
        Some(InitiativeLevel::HighInitiative)
    } else if matches_any(&["balanced", "default", "normal"]) {
        Some(InitiativeLevel::Balanced)
    } else {
        None
    };

    let choice = match density {
        Some(ResponseDensity::Concise) => Some(PersonalizationChoice::Concise),
        Some(ResponseDensity::Thorough) => Some(PersonalizationChoice::Thorough),
        Some(ResponseDensity::Balanced) => Some(PersonalizationChoice::Balanced),
        None => fallback_choice,
    };

    FirstTurnCalibrationUpdate {
        choice,
        density,
        initiative,
    }
}

fn infer_first_turn_identity_update(message: &str) -> FirstTurnIdentityUpdate {
    let trimmed = message.trim();
    let mut update = FirstTurnIdentityUpdate::default();

    for segment in trimmed.split([';', '\n', '·']) {
        let part = segment.trim();
        if part.is_empty() {
            continue;
        }
        let Some((raw_key, raw_value)) = part.split_once('=') else {
            continue;
        };
        let key = raw_key.trim().to_ascii_lowercase();
        let value = raw_value.trim();
        if value.is_empty() {
            continue;
        }

        match key.as_str() {
            "call_me" | "preferred_address" | "address" | "称呼" | "稱呼" => {
                update.preferred_address = Some(value.to_owned());
            }
            "agent_name" | "identity_name" | "name" | "名字" => {
                update.identity_name = Some(value.to_owned());
            }
            "creature" | "物种" | "物種" => {
                update.identity_creature = Some(value.to_owned());
            }
            "vibe" | "气质" | "氣質" | "风格" | "風格" => {
                update.identity_vibe = Some(value.to_owned());
            }
            "emoji" => {
                update.identity_emoji = Some(value.to_owned());
            }
            _ => {}
        }
    }

    if update.preferred_address.is_none() {
        update.preferred_address = extract_after_any_keyword(
            trimmed,
            &[
                "call me ",
                "you can call me ",
                "address me as ",
                "叫我",
                "你可以叫我",
                "称呼我",
                "稱呼我",
            ],
        );
    }
    if update.identity_name.is_none() {
        update.identity_name = extract_after_any_keyword(
            trimmed,
            &[
                "your name is ",
                "you are ",
                "you can be ",
                "你叫",
                "你的名字是",
                "你可以叫",
            ],
        );
    }
    if update.identity_creature.is_none() {
        update.identity_creature = extract_after_any_keyword(
            trimmed,
            &[
                "creature is ",
                "creature: ",
                "物种是",
                "物種是",
                "设定是",
                "設定是",
            ],
        );
    }
    if update.identity_vibe.is_none() {
        update.identity_vibe = extract_after_any_keyword(
            trimmed,
            &["vibe is ", "vibe: ", "风格是", "風格是", "气质是", "氣質是"],
        );
    }
    if update.identity_emoji.is_none() {
        update.identity_emoji =
            extract_after_any_keyword(trimmed, &["emoji is ", "emoji: ", "emoji 就用", "emoji 用"])
                .or_else(|| extract_first_emoji(trimmed));
    }

    update
}

fn extract_after_any_keyword(text: &str, keywords: &[&str]) -> Option<String> {
    let lower = text.to_lowercase();
    for keyword in keywords {
        let keyword_lower = keyword.to_lowercase();
        let Some(start) = lower.find(keyword_lower.as_str()) else {
            continue;
        };
        let value = &text[start + keyword.len()..];
        let extracted = value
            .split([';', '\n', ',', '，', '.', '。', '！', '!', '?', '？'])
            .next()
            .map(str::trim)
            .filter(|candidate| !candidate.is_empty())
            .map(|candidate| {
                candidate
                    .trim_matches(['"', '\'', '“', '”', '「', '」'])
                    .to_owned()
            });
        if extracted
            .as_deref()
            .is_some_and(|candidate| !candidate.is_empty())
        {
            return extracted;
        }
    }
    None
}

fn extract_first_emoji(text: &str) -> Option<String> {
    text.chars().find_map(|ch| {
        let value = ch as u32;
        ((0x1F300..=0x1FAFF).contains(&value) || (0x2600..=0x27BF).contains(&value))
            .then(|| ch.to_string())
    })
}

fn render_first_turn_calibration_update_lines(
    update: FirstTurnCalibrationUpdate,
    state: &StartupOnboardingState,
) -> Vec<String> {
    let language = state.selected_language.unwrap_or(Language::En);
    let mut lines = vec![localized_follow_up_heading("calibration_updated", language).to_owned()];
    if let Some(choice) = update.choice {
        lines.push(format!(
            "  {}: {}",
            localized_follow_up_heading("personalization", language),
            choice.label()
        ));
    }
    if let Some(density) = update.density {
        lines.push(format!(
            "  {}: {}",
            localized_follow_up_heading("response_density", language),
            density.as_str()
        ));
    }
    if let Some(initiative) = update.initiative {
        lines.push(format!(
            "  {}: {}",
            localized_follow_up_heading("initiative", language),
            initiative.as_str()
        ));
    }
    if let Some(preferred_address) = state.preferred_address.as_deref() {
        lines.push(format!(
            "  {}: {preferred_address}",
            localized_follow_up_heading("preferred_address", language)
        ));
    }
    if let Some(identity_name) = state.identity_name.as_deref() {
        lines.push(format!(
            "  {}: {identity_name}",
            localized_follow_up_heading("agent_name", language)
        ));
    }
    if let Some(identity_creature) = state.identity_creature.as_deref() {
        lines.push(format!(
            "  {}: {identity_creature}",
            localized_follow_up_heading("creature", language)
        ));
    }
    if let Some(identity_vibe) = state.identity_vibe.as_deref() {
        lines.push(format!(
            "  {}: {identity_vibe}",
            localized_follow_up_heading("vibe", language)
        ));
    }
    if let Some(identity_emoji) = state.identity_emoji.as_deref() {
        lines.push(format!(
            "  {}: {identity_emoji}",
            localized_follow_up_heading("emoji", language)
        ));
    }
    lines
}

fn render_personalization_seed_lines(personalization: &PersonalizationConfig) -> Vec<String> {
    let seed_summary = personalization_seed_summary(personalization);

    vec![
        "Personalization seed".to_owned(),
        format!("  density: {}", seed_summary.density),
        format!("  initiative: {}", seed_summary.initiative),
        format!("  locale: {}", seed_summary.locale),
    ]
}

fn render_personalization_seed_block(personalization: &PersonalizationConfig) -> Vec<String> {
    let seed_summary = personalization_seed_summary(personalization);

    vec![
        format!("- response_density: {}", seed_summary.density),
        format!("- initiative_level: {}", seed_summary.initiative),
        format!("- locale: {}", seed_summary.locale),
    ]
}

fn render_soul_seed_content(state: &StartupOnboardingState) -> String {
    let mut lines = vec![ONBOARDING_SOUL_MANAGED_START.to_owned()];
    lines.extend(render_workspace_seed_summary_lines(
        state,
        "language",
        "provider_lane",
    ));
    if let Some(seed) = state.personalization_seed() {
        lines.push("- personalization_seed:".to_owned());
        lines.extend(render_personalization_seed_block(&seed));
    }
    lines.push(ONBOARDING_SOUL_MANAGED_END.to_owned());
    lines.join("\n")
}

fn render_identity_seed_content(state: &StartupOnboardingState) -> String {
    let mut lines = vec![ONBOARDING_IDENTITY_MANAGED_START.to_owned()];
    lines.extend(render_workspace_seed_summary_lines(
        state,
        "preferred_language",
        "preferred_provider_lane",
    ));
    if let Some(identity_name) = state.identity_name.as_deref() {
        lines.push(format!("- identity_name: {identity_name}"));
    }
    if let Some(identity_creature) = state.identity_creature.as_deref() {
        lines.push(format!("- identity_creature: {identity_creature}"));
    }
    if let Some(identity_vibe) = state.identity_vibe.as_deref() {
        lines.push(format!("- identity_vibe: {identity_vibe}"));
    }
    if let Some(identity_emoji) = state.identity_emoji.as_deref() {
        lines.push(format!("- identity_emoji: {identity_emoji}"));
    }
    lines.push(ONBOARDING_IDENTITY_MANAGED_END.to_owned());
    lines.join("\n")
}

fn render_user_seed_content(state: &StartupOnboardingState) -> String {
    let mut lines = vec![ONBOARDING_USER_MANAGED_START.to_owned()];
    if let Some(preferred_address) = state.preferred_address.as_deref() {
        lines.push(format!("- preferred_address: {preferred_address}"));
    }
    lines.push(format!(
        "- personalization_choice: {}",
        state.personalization_summary_label()
    ));
    lines.push(ONBOARDING_USER_MANAGED_END.to_owned());
    lines.join("\n")
}

struct PersonalizationSeedSummary<'a> {
    density: &'a str,
    initiative: &'a str,
    locale: &'a str,
}

fn personalization_seed_summary(
    personalization: &PersonalizationConfig,
) -> PersonalizationSeedSummary<'_> {
    PersonalizationSeedSummary {
        density: personalization
            .response_density
            .map(ResponseDensity::as_str)
            .unwrap_or("balanced"),
        initiative: personalization
            .initiative_level
            .map(InitiativeLevel::as_str)
            .unwrap_or("balanced"),
        locale: personalization.locale.as_deref().unwrap_or("default"),
    }
}

fn render_workspace_seed_summary_lines(
    state: &StartupOnboardingState,
    language_key: &str,
    provider_key: &str,
) -> Vec<String> {
    vec![
        format!("- {language_key}: {}", state.language_label_or_default()),
        format!("- {provider_key}: {}", state.provider_label_or_default()),
        format!("- continue_setup: {}", state.continue_setup_summary()),
        format!("- mcp_scope: {}", state.mcp_summary()),
        format!(
            "- personalization_choice: {}",
            state.personalization_summary_label()
        ),
    ]
}

fn default_soul_content() -> String {
    [
        "# SOUL.md - onboarding personality seed",
        "",
        "This file is the workspace-facing seed for how Loong should feel after the",
        "first-run onboarding flow.",
        "",
        "## First-turn calibration shape",
        "",
        "If onboarding selected a non-skip personality, Loong may ask a few lightweight",
        "calibration questions before or around the first substantive turn.",
        "",
        ONBOARDING_SOUL_MANAGED_START,
        ONBOARDING_SOUL_MANAGED_END,
    ]
    .join("\n")
}

fn default_identity_content() -> String {
    [
        "# IDENTITY.md - workspace identity",
        "",
        "- Name:",
        "- Creature:",
        "- Vibe:",
        "- Emoji:",
        "",
        "Keep the editable identity fields above human-authored; Loong only manages the",
        "marked block below them.",
        "",
        ONBOARDING_IDENTITY_MANAGED_START,
        ONBOARDING_IDENTITY_MANAGED_END,
    ]
    .join("\n")
}

fn default_user_content() -> String {
    [
        "# USER.md - workspace user context",
        "",
        "- Name:",
        "- Preferred address:",
        "- Pronouns:",
        "- Timezone:",
        "- Notes:",
        "",
        "Keep the editable user fields above human-authored; Loong only manages the",
        "marked block below them.",
        "",
        ONBOARDING_USER_MANAGED_START,
        ONBOARDING_USER_MANAGED_END,
    ]
    .join("\n")
}

fn merge_managed_block(
    existing: Option<&str>,
    default_content: &str,
    start_marker: &str,
    end_marker: &str,
    replacement_block: &str,
) -> String {
    let base = existing.unwrap_or(default_content);
    let Some(start_index) = base.find(start_marker) else {
        let mut content = base.trim_end().to_owned();
        if !content.is_empty() {
            content.push_str("\n\n");
        }
        content.push_str(replacement_block);
        return content;
    };
    let after_start = start_index + start_marker.len();
    let Some(relative_end_index) = base[after_start..].find(end_marker) else {
        let mut content = base[..start_index].trim_end().to_owned();
        if !content.is_empty() {
            content.push_str("\n\n");
        }
        content.push_str(replacement_block);
        return content;
    };
    let end_index = after_start + relative_end_index + end_marker.len();
    let prefix = &base[..start_index];
    let suffix = &base[end_index..];

    let mut merged = prefix.trim_end().to_owned();
    if !merged.is_empty() {
        merged.push('\n');
    }
    merged.push_str(replacement_block);
    let trimmed_suffix = suffix.trim_start_matches('\n');
    if !trimmed_suffix.is_empty() {
        merged.push('\n');
        merged.push_str(trimmed_suffix);
    }
    merged
}

fn update_markdown_bullet_field(content: String, field_label: &str, value: Option<&str>) -> String {
    let Some(value) = value.filter(|value| !value.trim().is_empty()) else {
        return content;
    };
    let prefix = format!("- {field_label}:");
    let mut lines = content.lines().map(str::to_owned).collect::<Vec<_>>();
    if let Some(index) = lines
        .iter()
        .position(|line| line.trim_start().starts_with(prefix.as_str()))
        && let Some(line) = lines.get_mut(index)
    {
        *line = format!("{prefix} {value}");
    }
    lines.join("\n")
}

fn persist_onboarding_workspace_seed(
    workspace_root: Option<&Path>,
    state: &StartupOnboardingState,
) -> Result<Vec<String>, String> {
    let Some(workspace_root) = workspace_root else {
        return Ok(Vec::new());
    };
    fs::create_dir_all(workspace_root).map_err(|error| {
        format!(
            "failed to ensure onboarding workspace exists at {}: {error}",
            workspace_root.display()
        )
    })?;

    let soul_path = workspace_root.join("SOUL.md");
    let identity_path = workspace_root.join("IDENTITY.md");
    let user_path = workspace_root.join("USER.md");
    let existing_soul = fs::read_to_string(&soul_path).ok();
    let existing_identity = fs::read_to_string(&identity_path).ok();
    let existing_user = fs::read_to_string(&user_path).ok();
    let soul_default = default_soul_content();
    let identity_default = default_identity_content();
    let user_default = default_user_content();
    let soul_seed = render_soul_seed_content(state);
    let identity_seed = render_identity_seed_content(state);
    let user_seed = render_user_seed_content(state);
    let soul_content = merge_managed_block(
        existing_soul.as_deref(),
        soul_default.as_str(),
        ONBOARDING_SOUL_MANAGED_START,
        ONBOARDING_SOUL_MANAGED_END,
        soul_seed.as_str(),
    );
    let identity_content = merge_managed_block(
        existing_identity.as_deref(),
        identity_default.as_str(),
        ONBOARDING_IDENTITY_MANAGED_START,
        ONBOARDING_IDENTITY_MANAGED_END,
        identity_seed.as_str(),
    );
    let user_content = merge_managed_block(
        existing_user.as_deref(),
        user_default.as_str(),
        ONBOARDING_USER_MANAGED_START,
        ONBOARDING_USER_MANAGED_END,
        user_seed.as_str(),
    );
    let identity_content =
        update_markdown_bullet_field(identity_content, "Name", state.identity_name.as_deref());
    let identity_content = update_markdown_bullet_field(
        identity_content,
        "Creature",
        state.identity_creature.as_deref(),
    );
    let identity_content =
        update_markdown_bullet_field(identity_content, "Vibe", state.identity_vibe.as_deref());
    let identity_content =
        update_markdown_bullet_field(identity_content, "Emoji", state.identity_emoji.as_deref());
    let user_content = update_markdown_bullet_field(
        user_content,
        "Preferred address",
        state.preferred_address.as_deref(),
    );
    fs::write(&soul_path, soul_content)
        .map_err(|error| format!("failed to write {}: {error}", soul_path.display()))?;
    fs::write(&identity_path, identity_content)
        .map_err(|error| format!("failed to write {}: {error}", identity_path.display()))?;
    fs::write(&user_path, user_content)
        .map_err(|error| format!("failed to write {}: {error}", user_path.display()))?;

    Ok(vec![
        "Workspace seed updated".to_owned(),
        format!("SOUL.md: {}", soul_path.display()),
        format!("IDENTITY.md: {}", identity_path.display()),
        format!("USER.md: {}", user_path.display()),
    ])
}

fn persist_onboarding_config_seed(
    config_path: Option<&Path>,
    state: &StartupOnboardingState,
) -> Result<Vec<String>, String> {
    let Some(config_path) = config_path else {
        return Ok(Vec::new());
    };

    let config_path_string = config_path.display().to_string();
    let (_, mut config) = crate::config::load(Some(config_path_string.as_str()))
        .map_err(|error| format!("failed to load config for onboarding seed: {error}"))?;
    let mcp_changed = apply_mcp_choice_to_config(&mut config, state);
    let provider_changed = apply_provider_choice_to_config(&mut config, state);
    let personalization_changed = apply_personalization_seed_to_config(&mut config, state);
    if !mcp_changed && !provider_changed && !personalization_changed {
        return Ok(Vec::new());
    }

    crate::config::write(Some(config_path_string.as_str()), &config, true)
        .map_err(|error| format!("failed to persist onboarding config seed: {error}"))?;

    Ok(vec![
        "Config seed updated".to_owned(),
        format!("config: {}", config_path.display()),
    ])
}

fn apply_mcp_choice_to_config(config: &mut LoongConfig, state: &StartupOnboardingState) -> bool {
    if !state.is_continue_setup_enabled(ContinueSetupChoice::Mcp) {
        return false;
    }

    let selected = state
        .mcp_choices
        .iter()
        .filter_map(|(choice, enabled)| (*enabled).then_some(choice.label().to_owned()))
        .collect::<Vec<_>>();
    config.acp.dispatch.bootstrap_mcp_servers = selected;
    true
}

fn apply_provider_choice_to_config(
    config: &mut LoongConfig,
    state: &StartupOnboardingState,
) -> bool {
    let selected_kinds = state.configured_provider_kinds();
    let Some(active_kind) = state
        .selected_provider
        .or_else(|| selected_kinds.first().copied())
    else {
        return false;
    };

    let active_profile = resolve_provider_profile_seed(config, active_kind);
    config.set_active_provider_profile(active_profile.0, active_profile.1);

    for kind in selected_kinds
        .into_iter()
        .filter(|kind| *kind != active_kind)
    {
        let (profile_id, profile) = resolve_provider_profile_seed(config, kind);
        config.providers.insert(profile_id, profile);
    }
    true
}

fn resolve_provider_profile_seed(
    config: &LoongConfig,
    kind: ProviderKind,
) -> (String, ProviderProfileConfig) {
    if config.provider.kind == kind {
        let profile_id = config
            .active_provider_id()
            .map(str::to_owned)
            .unwrap_or_else(|| config.provider.inferred_profile_id());
        let profile = config
            .providers
            .get(profile_id.as_str())
            .cloned()
            .unwrap_or_else(|| ProviderProfileConfig::from_provider(config.provider.clone()));
        return (profile_id, profile);
    }

    if let Some((profile_id, profile)) = config
        .providers
        .iter()
        .find(|(_, profile)| profile.provider.kind == kind && profile.default_for_kind)
        .or_else(|| {
            config
                .providers
                .iter()
                .find(|(_, profile)| profile.provider.kind == kind)
        })
    {
        return (profile_id.clone(), profile.clone());
    }

    let provider = ProviderConfig::fresh_for_kind(kind);
    let profile = ProviderProfileConfig::from_provider(provider);
    (kind.as_str().to_owned(), profile)
}

fn apply_personalization_seed_to_config(
    config: &mut LoongConfig,
    state: &StartupOnboardingState,
) -> bool {
    if state
        .selected_personalization
        .is_some_and(PersonalizationChoice::suppresses_personalization)
    {
        config.memory.personalization = Some(PersonalizationConfig {
            prompt_state: PersonalizationPromptState::Suppressed,
            ..Default::default()
        });
        config.cli.personality = None;
        config.cli.personality_overlay_disabled = true;
        config.cli.refresh_native_system_prompt();
        return true;
    }

    let Some(seed) = state.personalization_seed() else {
        return false;
    };
    config.memory.personalization = Some(seed);
    config.cli.personality_overlay_disabled = false;
    if let Some(personality) = state
        .selected_personalization
        .and_then(PersonalizationChoice::prompt_personality)
    {
        config.cli.personality = Some(personality);
        config.cli.refresh_native_system_prompt();
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::chat_surface::command_palette::CommandPalette;
    use crate::chat::chat_surface::message_list::MessageList;

    fn blank_controller() -> StartupOnboardingController {
        StartupOnboardingController::new(None, None, Vec::new(), 0)
    }

    fn rendered_lines(list: &mut MessageList) -> String {
        list.get_rendered_lines(100)
            .into_iter()
            .map(|line| {
                line.spans
                    .into_iter()
                    .map(|span| span.content.to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn startup_panel_tracks_current_surface() {
        let mut controller = StartupOnboardingController::new(
            None,
            None,
            vec![RepoOptionalSkill {
                name: "alpha".to_owned(),
                description: "alpha description".to_owned(),
                source_dir: PathBuf::from("/tmp/alpha"),
                install_relative_path: PathBuf::from("alpha"),
            }],
            0,
        );
        let panel = controller
            .panel_for_surface(StartupSurfaceSnapshot {
                pending_turn: false,
                following_tail: true,
                has_conversation_messages: false,
                composer_empty: true,
                command_palette_active: false,
                inline_skill_popup_active: false,
                quickstart_focus: StartupQuickstartFocus::Chat,
            })
            .expect("startup panel");
        assert!(
            panel
                .options
                .iter()
                .any(|option| option.selected && option.label == "English")
        );

        controller.state.confirm_current();
        let panel = controller
            .panel_for_surface(StartupSurfaceSnapshot {
                pending_turn: false,
                following_tail: true,
                has_conversation_messages: false,
                composer_empty: true,
                command_palette_active: false,
                inline_skill_popup_active: false,
                quickstart_focus: StartupQuickstartFocus::Chat,
            })
            .expect("startup panel");
        assert!(
            panel
                .options
                .iter()
                .any(|option| option.selected && option.label == "[x] OpenAI")
        );

        controller.state.confirm_current();
        let panel = controller
            .panel_for_surface(StartupSurfaceSnapshot {
                pending_turn: false,
                following_tail: true,
                has_conversation_messages: false,
                composer_empty: true,
                command_palette_active: false,
                inline_skill_popup_active: false,
                quickstart_focus: StartupQuickstartFocus::Chat,
            })
            .expect("startup panel");
        assert!(
            panel
                .options
                .iter()
                .any(|option| option.selected && option.label.contains("all"))
        );
        assert!(panel.hint.contains("0 of 1 selected"));

        controller.state.confirm_current();
        let panel = controller
            .panel_for_surface(StartupSurfaceSnapshot {
                pending_turn: false,
                following_tail: true,
                has_conversation_messages: false,
                composer_empty: true,
                command_palette_active: false,
                inline_skill_popup_active: false,
                quickstart_focus: StartupQuickstartFocus::Chat,
            })
            .expect("startup panel");
        assert!(
            panel
                .options
                .iter()
                .any(|option| option.selected && option.label.contains("[ ] web providers"))
        );
    }

    #[test]
    fn provider_stage_skips_skills_when_no_optional_repo_skills_exist() {
        let mut controller = blank_controller();

        controller.state.confirm_current();
        controller.state.confirm_current();

        assert_eq!(
            controller.state.stage,
            StartupOnboardingStage::ContinueSetup
        );
    }

    #[test]
    fn startup_language_selection_refreshes_header_copy_and_palette_language() {
        let mut controller = blank_controller();
        controller.optional_repo_skills = vec![
            RepoOptionalSkill {
                name: "alpha".to_owned(),
                description: "alpha description".to_owned(),
                source_dir: PathBuf::from("/tmp/alpha"),
                install_relative_path: PathBuf::from("alpha"),
            },
            RepoOptionalSkill {
                name: "beta".to_owned(),
                description: "beta description".to_owned(),
                source_dir: PathBuf::from("/tmp/beta"),
                install_relative_path: PathBuf::from("beta"),
            },
        ];
        controller.state.skill_enabled = vec![false, false];
        controller.mcp_count = 1;
        let mut i18n = I18nService::new(Language::En);
        let mut command_palette = CommandPalette::new(Language::En, Vec::new());
        let (version, tutorial, sections, tips) = controller.startup_content(&i18n);
        let mut message_list = MessageList::new();
        message_list.add_startup_header_with_tips_and_eye(
            version,
            tutorial,
            sections,
            tips,
            StartupEyeAnimation::Ambient,
        );
        controller.state.selection = 1;

        controller.apply_language_selection(&mut i18n, &mut command_palette, &mut message_list);

        let rendered = rendered_lines(&mut message_list);
        assert!(rendered.contains("ctrl+c 退出"));
        assert_eq!(i18n.language(), Language::ZhCn);
        command_palette.show_commands("");
        let preview = command_palette.composer_preview_text().expect("preview");
        assert_eq!(preview, "/");
    }

    #[test]
    fn startup_onboarding_progresses_to_finish_and_can_complete() {
        let temp_root =
            std::env::temp_dir().join(format!("loong-onboarding-followup-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp_root);
        std::fs::create_dir_all(&temp_root).expect("mkdir temp root");
        let config_path = temp_root.join("loong.toml");
        let config_string = config_path.display().to_string();
        crate::config::write(Some(config_string.as_str()), &LoongConfig::default(), true)
            .expect("write config");
        let mut controller = StartupOnboardingController::new(
            Some(temp_root.join("loong.toml")),
            Some(temp_root.clone()),
            vec![RepoOptionalSkill {
                name: "alpha-skill".to_owned(),
                description: "alpha description".to_owned(),
                source_dir: PathBuf::from("/tmp/alpha"),
                install_relative_path: PathBuf::from("alpha"),
            }],
            0,
        );

        assert_eq!(controller.state.stage, StartupOnboardingStage::Language);
        controller.state.confirm_current();
        assert_eq!(controller.state.stage, StartupOnboardingStage::Provider);
        controller.state.confirm_current();
        assert_eq!(controller.state.stage, StartupOnboardingStage::Skills);
        assert!(controller.state.toggle_current_option());
        controller.state.confirm_current();
        assert_eq!(
            controller.state.stage,
            StartupOnboardingStage::ContinueSetup
        );
        controller.state.move_selection(1);
        assert!(controller.state.toggle_current_option());
        controller.state.move_selection(1);
        assert!(controller.state.toggle_current_option());
        controller.state.confirm_current();
        assert_eq!(controller.state.stage, StartupOnboardingStage::Mcp);
        controller.state.move_selection(1);
        assert!(controller.state.toggle_current_option());
        controller.state.confirm_current();
        assert_eq!(
            controller.state.stage,
            StartupOnboardingStage::Personalization
        );
        controller.state.move_selection(2);
        controller.state.confirm_current();
        assert_eq!(controller.state.stage, StartupOnboardingStage::Finish);
        controller.state.confirm_current();
        assert!(controller.state.completed);
        let mut message_list = MessageList::new();
        controller.finish(&mut message_list);
        let rendered = rendered_lines(&mut message_list);
        assert!(rendered.contains("Onboarding complete"));
        assert!(rendered.contains("Continue setup:"));
        assert!(rendered.contains("Suggested next asks:"));
        assert!(rendered.contains("examples:"));
        assert!(rendered.contains("Personalization: thorough"));
        assert!(rendered.contains("Config seed updated"));
        assert!(rendered.contains("Workspace seed updated"));

        let _ = std::fs::remove_dir_all(&temp_root);
    }

    #[test]
    fn finish_stages_first_turn_calibration_for_agent_reply() {
        let mut controller = StartupOnboardingController::new(None, None, Vec::new(), 0);
        controller.state.selected_language = Some(Language::En);
        controller.state.selected_personalization = Some(PersonalizationChoice::Balanced);
        let mut message_list = MessageList::new();

        controller.finish(&mut message_list);

        let addendum = controller
            .take_first_turn_calibration_addendum()
            .expect("agent calibration addendum");

        assert!(
            message_list
                .messages
                .iter()
                .all(|message| message.role != "Assistant")
        );
        assert!(addendum.contains("do not start the main task yet"));
        assert!(addendum.contains("how should I address you"));
        assert!(addendum.contains("name, creature, vibe, and emoji"));
    }

    #[test]
    fn turn_off_personalization_does_not_stage_first_turn_calibration() {
        let mut controller = StartupOnboardingController::new(None, None, Vec::new(), 0);
        controller.state.selected_language = Some(Language::En);
        controller.state.selected_personalization = Some(PersonalizationChoice::TurnOff);
        let mut message_list = MessageList::new();

        controller.finish(&mut message_list);

        assert_eq!(
            controller.first_turn_calibration_stage,
            FirstTurnCalibrationStage::Idle
        );
        assert!(controller.take_first_turn_calibration_addendum().is_none());
        assert!(
            message_list
                .messages
                .iter()
                .all(|message| message.role != "Assistant")
        );
    }

    #[test]
    fn startup_onboarding_escape_moves_back_one_stage() {
        let mut state = StartupOnboardingState::new(0, None);
        state.confirm_current();
        assert_eq!(state.stage, StartupOnboardingStage::Provider);
        assert!(state.go_back());
        assert_eq!(state.stage, StartupOnboardingStage::Language);

        state.confirm_current();
        state.confirm_current();
        assert_eq!(state.stage, StartupOnboardingStage::ContinueSetup);
        assert!(state.go_back());
        assert_eq!(state.stage, StartupOnboardingStage::Provider);
    }

    #[test]
    fn startup_onboarding_escape_key_uses_back_navigation_instead_of_finishing() {
        let mut controller = StartupOnboardingController::new(None, None, Vec::new(), 0);
        controller.state.confirm_current();
        assert_eq!(controller.state.stage, StartupOnboardingStage::Provider);

        let outcome = controller.handle_key(crossterm::event::KeyEvent::from(
            crossterm::event::KeyCode::Esc,
        ));

        assert!(outcome.handled);
        assert!(!outcome.finished);
        assert_eq!(controller.state.stage, StartupOnboardingStage::Language);
    }

    #[test]
    fn continue_setup_eye_focus_maps_channels_and_related_options_semantically() {
        let mut state = StartupOnboardingState::new(0, None);
        state.stage = StartupOnboardingStage::ContinueSetup;

        state.selection = 0;
        assert_eq!(state.focus(), StartupEyeFocus::Right);
        state.selection = 1;
        assert_eq!(state.focus(), StartupEyeFocus::DownCenter);
        state.selection = 2;
        assert_eq!(state.focus(), StartupEyeFocus::Left);
        state.selection = 3;
        assert_eq!(state.focus(), StartupEyeFocus::Up);
    }

    #[test]
    fn startup_follow_up_lines_for_channels_include_examples_and_runtime_hint() {
        let state = StartupOnboardingState::new(0, None);
        let lines = ContinueSetupChoice::Channels.follow_up_lines(&state);
        let rendered = lines.join("\n");

        assert!(rendered.contains("• channels"));
        assert!(rendered.contains("examples:"));
        assert!(rendered.contains("help me continue channel setup after first run"));
        assert!(rendered.contains("runtime:") || rendered.contains("examples: telegram"));
    }

    #[test]
    fn startup_follow_up_lines_for_web_providers_include_openai_compatible_hint() {
        let mut state = StartupOnboardingState::new(0, None);
        state.selected_provider = Some(ProviderKind::Custom);
        state.provider_choices = vec![(ProviderKind::Custom, true)];

        let rendered = ContinueSetupChoice::WebProviders
            .follow_up_lines(&state)
            .join("\n");

        assert!(rendered.contains("surface: /model"));
        assert!(rendered.contains("provider seed: Custom"));
        assert!(rendered.contains("CUSTOM_PROVIDER_API_KEY"));
        assert!(rendered.contains("<openai-compatible-host>"));
    }

    #[test]
    fn startup_follow_up_lines_for_mcp_and_personalization_reference_real_surfaces() {
        let mut state = StartupOnboardingState::new(0, None);
        state.selected_language = Some(Language::En);
        state.selected_provider = Some(ProviderKind::Custom);
        state.provider_choices = vec![(ProviderKind::Custom, true)];
        state.selected_personalization = Some(PersonalizationChoice::Balanced);
        state.mcp_choices = vec![
            (McpChoice::Filesystem, true),
            (McpChoice::Fetch, false),
            (McpChoice::Github, false),
        ];
        let mcp_lines = ContinueSetupChoice::Mcp.follow_up_lines(&state).join("\n");
        let personalization_lines = ContinueSetupChoice::Personalization
            .follow_up_lines(&state)
            .join("\n");

        assert!(mcp_lines.contains("surface: /mcp"));
        assert!(mcp_lines.contains("current provider choice: Custom"));
        assert!(mcp_lines.contains("selected MCP scope:"));
        assert!(mcp_lines.contains("continue MCP setup"));
        assert!(personalization_lines.contains("surfaces: /language, /model"));
        assert!(personalization_lines.contains("current language choice: English"));
        assert!(personalization_lines.contains("chosen personality: balanced"));
        assert!(personalization_lines.contains("personalize loong"));
    }

    #[test]
    fn personalization_follow_up_defaults_to_skip_not_later() {
        let state = StartupOnboardingState::new(0, None);
        let follow_up_lines = render_startup_onboarding_follow_up_lines(&state).join("\n");
        let personalization_lines = ContinueSetupChoice::Personalization
            .follow_up_lines(&state)
            .join("\n");

        assert!(follow_up_lines.contains("Personalization: skip"));
        assert!(personalization_lines.contains("chosen personality: skip"));
        assert!(!follow_up_lines.contains("Personalization: later"));
    }

    #[test]
    fn turn_off_personalization_follow_up_reports_disabled_and_reenable_hint() {
        let mut state = StartupOnboardingState::new(0, None);
        state.selected_personalization = Some(PersonalizationChoice::TurnOff);

        let follow_up_lines = render_startup_onboarding_follow_up_lines(&state).join("\n");
        let personalization_lines = ContinueSetupChoice::Personalization
            .follow_up_lines(&state)
            .join("\n");

        assert!(follow_up_lines.contains("Personalization: disabled"));
        assert!(personalization_lines.contains("chosen personality: disabled"));
        assert!(personalization_lines.contains("re-enable personalization"));
    }

    #[test]
    fn first_turn_calibration_prompt_follows_personalization_choice() {
        let mut concise = StartupOnboardingState::new(0, None);
        concise.selected_personalization = Some(PersonalizationChoice::Concise);
        let concise_prompt =
            render_first_turn_calibration_prompt(&concise).expect("concise prompt");
        assert!(concise_prompt.contains("concise"));
        assert!(concise_prompt.contains("Reply once"));

        let mut thorough = StartupOnboardingState::new(0, None);
        thorough.selected_personalization = Some(PersonalizationChoice::Thorough);
        let thorough_prompt =
            render_first_turn_calibration_prompt(&thorough).expect("thorough prompt");
        assert!(thorough_prompt.contains("thorough"));
        assert!(thorough_prompt.contains("comparison"));

        let mut skipped = StartupOnboardingState::new(0, None);
        skipped.selected_personalization = Some(PersonalizationChoice::Skip);
        assert!(render_first_turn_calibration_prompt(&skipped).is_none());

        let mut turned_off = StartupOnboardingState::new(0, None);
        turned_off.selected_personalization = Some(PersonalizationChoice::TurnOff);
        assert!(render_first_turn_calibration_prompt(&turned_off).is_none());
    }

    #[test]
    fn personalization_panel_keeps_turn_off_as_bottom_option() {
        let mut state = StartupOnboardingState::new(0, None);
        state.stage = StartupOnboardingStage::Personalization;

        let panel = state.panel(&[], Language::En);

        assert_eq!(panel.options.len(), 5);
        let last = panel.options.last().expect("turn off option");
        assert_eq!(last.label, "turn off");
        assert!(last.detail.contains("disable personalization"));
    }

    #[test]
    fn language_panel_uses_localized_secondary_copy() {
        let mut state = StartupOnboardingState::new(0, None);
        state.stage = StartupOnboardingStage::Language;

        let panel = state.panel(&[], Language::En);

        assert_eq!(panel.options.len(), 5);
        assert_eq!(
            panel.options[0].detail,
            "keep the shell and guidance in English."
        );
        assert_eq!(panel.options[1].detail, "保留中文界面与引导。");
        assert_eq!(panel.options[2].detail, "保留繁體中文介面與引導。");
        assert_eq!(
            panel.options[3].detail,
            "シェルとガイダンスを日本語で表示します。"
        );
        assert_eq!(
            panel.options[4].detail,
            "оставить shell и подсказки на русском."
        );
    }

    #[test]
    fn provider_panel_lists_real_provider_kinds_sorted_with_default_selection() {
        let mut state = StartupOnboardingState::new(0, Some(ProviderKind::Deepseek));
        state.stage = StartupOnboardingStage::Provider;
        state.selection = state.preferred_provider_selection();

        let panel = state.panel(&[], Language::En);

        assert!(panel.options.len() > 20);
        assert_eq!(panel.options[0].label, "[ ] Anthropic");
        assert!(
            panel
                .options
                .iter()
                .any(|option| option.label == "[x] DeepSeek")
        );
        assert!(panel.hint.contains("space toggles providers"));
    }

    #[test]
    fn infer_first_turn_calibration_update_extracts_density_and_initiative() {
        let update = infer_first_turn_calibration_update(
            "be concise and ask before acting",
            Some(PersonalizationChoice::Balanced),
        );
        assert_eq!(update.choice, Some(PersonalizationChoice::Concise));
        assert_eq!(update.density, Some(ResponseDensity::Concise));
        assert_eq!(update.initiative, Some(InitiativeLevel::AskBeforeActing));

        let update = infer_first_turn_calibration_update(
            "be thorough and make a reasonable assumption and move",
            Some(PersonalizationChoice::Balanced),
        );
        assert_eq!(update.choice, Some(PersonalizationChoice::Thorough));
        assert_eq!(update.density, Some(ResponseDensity::Thorough));
        assert_eq!(update.initiative, Some(InitiativeLevel::HighInitiative));
    }

    #[test]
    fn infer_first_turn_identity_update_supports_natural_language_reply() {
        let update = infer_first_turn_identity_update(
            "You can call me Operator. Your name is Aster, creature is fox, vibe is steady, emoji is ✨",
        );

        assert_eq!(update.preferred_address.as_deref(), Some("Operator"));
        assert_eq!(update.identity_name.as_deref(), Some("Aster"));
        assert_eq!(update.identity_creature.as_deref(), Some("fox"));
        assert_eq!(update.identity_vibe.as_deref(), Some("steady"));
        assert_eq!(update.identity_emoji.as_deref(), Some("✨"));
    }

    #[test]
    fn infer_first_turn_identity_update_supports_natural_language_reply_in_chinese() {
        let update = infer_first_turn_identity_update(
            "叫我伙伴。你可以叫 Aster，物种是狐，风格是沉稳，emoji 用✨。",
        );

        assert_eq!(update.preferred_address.as_deref(), Some("伙伴"));
        assert_eq!(update.identity_name.as_deref(), Some("Aster"));
        assert_eq!(update.identity_creature.as_deref(), Some("狐"));
        assert_eq!(update.identity_vibe.as_deref(), Some("沉稳"));
        assert_eq!(update.identity_emoji.as_deref(), Some("✨"));
    }

    #[test]
    fn personalization_seed_maps_selected_choice_to_config_shape() {
        let mut concise = StartupOnboardingState::new(0, None);
        concise.selected_language = Some(Language::En);
        concise.selected_personalization = Some(PersonalizationChoice::Concise);
        let concise_seed = concise.personalization_seed().expect("concise seed");
        assert_eq!(
            concise_seed.response_density,
            Some(ResponseDensity::Concise)
        );
        assert_eq!(
            concise_seed.initiative_level,
            Some(InitiativeLevel::AskBeforeActing)
        );
        assert_eq!(concise_seed.locale.as_deref(), Some("en"));

        let mut thorough = StartupOnboardingState::new(0, None);
        thorough.selected_language = Some(Language::ZhCn);
        thorough.selected_personalization = Some(PersonalizationChoice::Thorough);
        thorough.preferred_address = Some("Operator".to_owned());
        let thorough_seed = thorough.personalization_seed().expect("thorough seed");
        assert_eq!(
            thorough_seed.response_density,
            Some(ResponseDensity::Thorough)
        );
        assert_eq!(
            thorough_seed.initiative_level,
            Some(InitiativeLevel::HighInitiative)
        );
        assert_eq!(thorough_seed.locale.as_deref(), Some("zh-CN"));
        assert_eq!(thorough_seed.preferred_name.as_deref(), Some("Operator"));

        let mut skipped = StartupOnboardingState::new(0, None);
        skipped.selected_personalization = Some(PersonalizationChoice::Skip);
        assert!(skipped.personalization_seed().is_none());

        let mut turned_off = StartupOnboardingState::new(0, None);
        turned_off.selected_personalization = Some(PersonalizationChoice::TurnOff);
        assert!(turned_off.personalization_seed().is_none());
    }

    #[test]
    fn persist_onboarding_workspace_seed_writes_soul_and_identity_files() {
        let temp_root =
            std::env::temp_dir().join(format!("loong-onboarding-seed-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp_root);
        std::fs::create_dir_all(&temp_root).expect("mkdir temp root");

        let mut state = StartupOnboardingState::new(0, None);
        state.selected_language = Some(Language::En);
        state.selected_provider = Some(ProviderKind::Custom);
        state.provider_choices = vec![(ProviderKind::Custom, true)];
        state.selected_personalization = Some(PersonalizationChoice::Thorough);
        state.preferred_address = Some("Operator".to_owned());
        state.identity_name = Some("Aster".to_owned());
        state.identity_creature = Some("Fox".to_owned());
        state.identity_vibe = Some("Steady".to_owned());
        state.identity_emoji = Some("✨".to_owned());

        let lines =
            persist_onboarding_workspace_seed(Some(temp_root.as_path()), &state).expect("persist");
        assert!(lines.iter().any(|line| line.contains("SOUL.md")));
        assert!(lines.iter().any(|line| line.contains("IDENTITY.md")));
        assert!(lines.iter().any(|line| line.contains("USER.md")));

        let soul = std::fs::read_to_string(temp_root.join("SOUL.md")).expect("read soul");
        let identity =
            std::fs::read_to_string(temp_root.join("IDENTITY.md")).expect("read identity");
        let user = std::fs::read_to_string(temp_root.join("USER.md")).expect("read user");
        assert!(soul.contains(ONBOARDING_SOUL_MANAGED_START));
        assert!(soul.contains("personalization_seed"));
        assert!(identity.contains(ONBOARDING_IDENTITY_MANAGED_START));
        assert!(identity.contains("preferred_provider_lane"));
        assert!(user.contains(ONBOARDING_USER_MANAGED_START));
        assert!(identity.contains("- Name: Aster"));
        assert!(identity.contains("- Creature: Fox"));
        assert!(identity.contains("- Vibe: Steady"));
        assert!(identity.contains("- Emoji: ✨"));
        assert!(user.contains("- Preferred address: Operator"));

        let _ = std::fs::remove_dir_all(&temp_root);
    }

    #[test]
    fn persist_onboarding_workspace_seed_preserves_manual_identity_fields() {
        let temp_root = std::env::temp_dir().join(format!(
            "loong-onboarding-seed-preserve-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&temp_root);
        std::fs::create_dir_all(&temp_root).expect("mkdir temp root");
        let soul_path = temp_root.join("SOUL.md");
        let identity_path = temp_root.join("IDENTITY.md");
        let user_path = temp_root.join("USER.md");

        std::fs::write(
            &soul_path,
            "# SOUL.md - onboarding personality seed\n\nManual preface\n\n<!-- LOONG:ONBOARDING:START -->\nold\n<!-- LOONG:ONBOARDING:END -->\n\nManual suffix\n",
        )
        .expect("write soul fixture");
        std::fs::write(
            &identity_path,
            "# IDENTITY.md - workspace identity\n\n- Name: Nova\n- Creature: Fox\n\n<!-- LOONG:IDENTITY:START -->\nold\n<!-- LOONG:IDENTITY:END -->\n",
        )
        .expect("write identity fixture");
        std::fs::write(
            &user_path,
            "# USER.md - workspace user context\n\n- Preferred address: teammate\n\n<!-- LOONG:USER:START -->\nold\n<!-- LOONG:USER:END -->\n",
        )
        .expect("write user fixture");

        let mut state = StartupOnboardingState::new(0, None);
        state.selected_language = Some(Language::En);
        state.selected_personalization = Some(PersonalizationChoice::Balanced);

        persist_onboarding_workspace_seed(Some(temp_root.as_path()), &state)
            .expect("persist workspace seed");

        let soul = std::fs::read_to_string(&soul_path).expect("read soul");
        let identity = std::fs::read_to_string(&identity_path).expect("read identity");
        let user = std::fs::read_to_string(&user_path).expect("read user");
        assert!(soul.contains("Manual preface"));
        assert!(soul.contains("Manual suffix"));
        assert!(identity.contains("- Name: Nova"));
        assert!(identity.contains("- Creature: Fox"));
        assert!(user.contains("- Preferred address: teammate"));

        let _ = std::fs::remove_dir_all(&temp_root);
    }

    #[test]
    fn persist_onboarding_config_seed_writes_personalization_into_config() {
        let temp_root = std::env::temp_dir().join(format!(
            "loong-onboarding-config-seed-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&temp_root);
        std::fs::create_dir_all(&temp_root).expect("mkdir temp root");
        let config_path = temp_root.join("loong.toml");
        let config_string = config_path.display().to_string();

        crate::config::write(Some(config_string.as_str()), &LoongConfig::default(), true)
            .expect("write default config");

        let mut state = StartupOnboardingState::new(0, None);
        state.selected_language = Some(Language::En);
        state.selected_personalization = Some(PersonalizationChoice::Concise);
        state.preferred_address = Some("Operator".to_owned());

        let lines =
            persist_onboarding_config_seed(Some(config_path.as_path()), &state).expect("persist");
        assert!(
            lines
                .iter()
                .any(|line| line.contains("Config seed updated"))
        );

        let (_, loaded) = crate::config::load(Some(config_string.as_str())).expect("reload config");
        let personalization = loaded
            .memory
            .personalization
            .expect("personalization should persist");
        assert_eq!(
            personalization.response_density,
            Some(ResponseDensity::Concise)
        );
        assert_eq!(
            personalization.initiative_level,
            Some(InitiativeLevel::AskBeforeActing)
        );
        assert_eq!(personalization.preferred_name.as_deref(), Some("Operator"));
        assert_eq!(personalization.locale.as_deref(), Some("en"));
        assert_eq!(loaded.cli.personality, Some(PromptPersonality::Pragmatist));

        let _ = std::fs::remove_dir_all(&temp_root);
    }

    #[test]
    fn persist_onboarding_config_seed_writes_provider_choice_into_config() {
        let temp_root = std::env::temp_dir().join(format!(
            "loong-onboarding-provider-config-seed-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&temp_root);
        std::fs::create_dir_all(&temp_root).expect("mkdir temp root");
        let config_path = temp_root.join("loong.toml");
        let config_string = config_path.display().to_string();

        crate::config::write(Some(config_string.as_str()), &LoongConfig::default(), true)
            .expect("write default config");

        let mut state = StartupOnboardingState::new(0, None);
        state.selected_provider = Some(ProviderKind::Custom);
        state.provider_choices = vec![(ProviderKind::Custom, true)];

        let lines =
            persist_onboarding_config_seed(Some(config_path.as_path()), &state).expect("persist");
        assert!(
            lines
                .iter()
                .any(|line| line.contains("Config seed updated"))
        );

        let (_, loaded) = crate::config::load(Some(config_string.as_str())).expect("reload config");
        assert_eq!(loaded.active_provider_id(), Some("custom"));
        assert_eq!(loaded.provider.kind, ProviderKind::Custom);
        assert_eq!(
            loaded.provider.resolved_base_url(),
            "https://<openai-compatible-host>/v1"
        );
        assert_eq!(
            loaded.provider.default_api_key_env().as_deref(),
            Some("CUSTOM_PROVIDER_API_KEY")
        );

        let _ = std::fs::remove_dir_all(&temp_root);
    }

    #[test]
    fn finish_installs_selected_optional_repo_skills_into_loong_workspace_root() {
        let temp_root = std::env::temp_dir().join(format!(
            "loong-onboarding-skill-install-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&temp_root);

        let repo_root = temp_root.join("repo");
        let source_dir = repo_root
            .join("skills")
            .join("anthropic-office")
            .join("alpha");
        std::fs::create_dir_all(&source_dir).expect("mkdir source skill");
        std::fs::write(
            source_dir.join("SKILL.md"),
            "---\nname: alpha-skill\ndescription: alpha description\n---\n",
        )
        .expect("write skill");

        let optional_skill = RepoOptionalSkill {
            name: "alpha-skill".to_owned(),
            description: "alpha description".to_owned(),
            source_dir,
            install_relative_path: PathBuf::from("anthropic-office").join("alpha"),
        };
        let mut controller = StartupOnboardingController::new(
            None,
            Some(repo_root.clone()),
            vec![optional_skill],
            0,
        );
        controller.state.skill_enabled = vec![true];

        let mut message_list = MessageList::new();
        controller.finish(&mut message_list);

        let installed_skill = repo_root
            .join(".loong")
            .join("skills")
            .join("anthropic-office")
            .join("alpha")
            .join("SKILL.md");
        assert!(installed_skill.exists());

        let rendered = rendered_lines(&mut message_list);
        assert!(rendered.contains("Installed optional repo skills: 1"));
        assert!(rendered.contains("alpha-skill"));

        let _ = std::fs::remove_dir_all(&temp_root);
    }

    #[test]
    fn persist_onboarding_config_seed_reuses_detected_provider_profile() {
        let temp_root = std::env::temp_dir().join(format!(
            "loong-onboarding-provider-reuse-config-seed-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&temp_root);
        std::fs::create_dir_all(&temp_root).expect("mkdir temp root");
        let config_path = temp_root.join("loong.toml");
        let config_string = config_path.display().to_string();

        let mut config = LoongConfig::default();
        let provider = ProviderConfig::fresh_for_kind(ProviderKind::Deepseek);
        config.provider = provider;
        config.providers.clear();
        config.active_provider = None;
        crate::config::write(Some(config_string.as_str()), &config, true)
            .expect("write default config");

        let mut state = StartupOnboardingState::new(0, None);
        state.selected_provider = Some(ProviderKind::Deepseek);
        state.provider_choices = vec![(ProviderKind::Deepseek, true)];

        let lines =
            persist_onboarding_config_seed(Some(config_path.as_path()), &state).expect("persist");
        assert!(
            lines
                .iter()
                .any(|line| line.contains("Config seed updated"))
        );

        let (_, loaded) = crate::config::load(Some(config_string.as_str())).expect("reload config");
        assert_eq!(loaded.active_provider_id(), Some("deepseek"));
        assert!(loaded.providers.contains_key("deepseek"));
        assert_eq!(loaded.provider.kind, ProviderKind::Deepseek);
        assert_eq!(loaded.provider.model, "auto");

        let _ = std::fs::remove_dir_all(&temp_root);
    }

    #[test]
    fn persist_onboarding_config_seed_keeps_multiple_selected_provider_profiles() {
        let temp_root = std::env::temp_dir().join(format!(
            "loong-onboarding-provider-multi-config-seed-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&temp_root);
        std::fs::create_dir_all(&temp_root).expect("mkdir temp root");
        let config_path = temp_root.join("loong.toml");
        let config_string = config_path.display().to_string();

        crate::config::write(Some(config_string.as_str()), &LoongConfig::default(), true)
            .expect("write default config");

        let mut state = StartupOnboardingState::new(0, None);
        state.selected_provider = Some(ProviderKind::Anthropic);
        state.provider_choices = vec![
            (ProviderKind::Anthropic, true),
            (ProviderKind::Openai, true),
        ];

        persist_onboarding_config_seed(Some(config_path.as_path()), &state).expect("persist");

        let (_, loaded) = crate::config::load(Some(config_string.as_str())).expect("reload config");
        assert_eq!(loaded.active_provider_id(), Some("anthropic"));
        assert!(loaded.providers.contains_key("anthropic"));
        assert!(loaded.providers.contains_key("openai"));

        let _ = std::fs::remove_dir_all(&temp_root);
    }

    #[test]
    fn persist_onboarding_config_seed_writes_selected_mcp_servers_into_dispatch() {
        let temp_root = std::env::temp_dir().join(format!(
            "loong-onboarding-mcp-config-seed-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&temp_root);
        std::fs::create_dir_all(&temp_root).expect("mkdir temp root");
        let config_path = temp_root.join("loong.toml");
        let config_string = config_path.display().to_string();

        crate::config::write(Some(config_string.as_str()), &LoongConfig::default(), true)
            .expect("write default config");

        let mut state = StartupOnboardingState::new(0, None);
        state.continue_setup_choices = vec![
            (ContinueSetupChoice::WebProviders, false),
            (ContinueSetupChoice::Channels, false),
            (ContinueSetupChoice::Mcp, true),
            (ContinueSetupChoice::Personalization, false),
        ];
        state.mcp_choices = vec![
            (McpChoice::Filesystem, true),
            (McpChoice::Fetch, true),
            (McpChoice::Github, false),
        ];

        let lines =
            persist_onboarding_config_seed(Some(config_path.as_path()), &state).expect("persist");
        assert!(
            lines
                .iter()
                .any(|line| line.contains("Config seed updated"))
        );

        let (_, loaded) = crate::config::load(Some(config_string.as_str())).expect("reload config");
        assert_eq!(
            loaded
                .acp
                .dispatch
                .bootstrap_mcp_server_names()
                .expect("bootstrap names"),
            vec!["filesystem".to_owned(), "fetch".to_owned()]
        );

        let _ = std::fs::remove_dir_all(&temp_root);
    }

    #[test]
    fn persist_onboarding_config_seed_turn_off_suppresses_personalization() {
        let temp_root = std::env::temp_dir().join(format!(
            "loong-onboarding-config-turn-off-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&temp_root);
        std::fs::create_dir_all(&temp_root).expect("mkdir temp root");
        let config_path = temp_root.join("loong.toml");
        let config_string = config_path.display().to_string();

        crate::config::write(Some(config_string.as_str()), &LoongConfig::default(), true)
            .expect("write default config");

        let mut state = StartupOnboardingState::new(0, None);
        state.selected_personalization = Some(PersonalizationChoice::TurnOff);

        let lines =
            persist_onboarding_config_seed(Some(config_path.as_path()), &state).expect("persist");
        assert!(
            lines
                .iter()
                .any(|line| line.contains("Config seed updated"))
        );

        let (_, loaded) = crate::config::load(Some(config_string.as_str())).expect("reload config");
        let personalization = loaded
            .memory
            .personalization
            .expect("suppressed personalization should persist");
        assert_eq!(
            personalization.prompt_state,
            PersonalizationPromptState::Suppressed
        );
        assert_eq!(personalization.response_density, None);
        assert_eq!(personalization.initiative_level, None);
        assert!(loaded.cli.personality_overlay_disabled);
        assert!(!loaded.cli.system_prompt.contains("## Personality Overlay:"));

        let _ = std::fs::remove_dir_all(&temp_root);
    }

    #[test]
    fn first_turn_calibration_reply_updates_persisted_personalization_choice() {
        let temp_root = std::env::temp_dir().join(format!(
            "loong-first-turn-calibration-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&temp_root);
        std::fs::create_dir_all(&temp_root).expect("mkdir temp root");
        let config_path = temp_root.join("loong.toml");
        let config_string = config_path.display().to_string();
        crate::config::write(Some(config_string.as_str()), &LoongConfig::default(), true)
            .expect("write config");

        let mut controller = StartupOnboardingController::new(
            Some(config_path),
            Some(temp_root.clone()),
            Vec::new(),
            0,
        );
        controller.state.selected_language = Some(Language::En);
        controller.state.selected_personalization = Some(PersonalizationChoice::Balanced);
        let mut message_list = MessageList::new();
        controller.finish(&mut message_list);
        let _ = controller
            .take_first_turn_calibration_addendum()
            .expect("calibration addendum");

        assert!(controller.absorb_first_turn_calibration_if_needed(
            "please be concise and keep it short; call_me=Operator; agent_name=Aster; creature=fox; vibe=steady; emoji=✨",
            &mut message_list,
        ));

        assert_eq!(
            controller.state.selected_personalization,
            Some(PersonalizationChoice::Concise)
        );
        assert_eq!(
            controller.state.calibrated_density,
            Some(ResponseDensity::Concise)
        );
        assert_eq!(
            controller.state.preferred_address.as_deref(),
            Some("Operator")
        );
        assert_eq!(controller.state.identity_name.as_deref(), Some("Aster"));
        assert_eq!(controller.state.identity_creature.as_deref(), Some("fox"));
        assert_eq!(controller.state.identity_vibe.as_deref(), Some("steady"));
        assert_eq!(controller.state.identity_emoji.as_deref(), Some("✨"));
        assert_eq!(
            controller.first_turn_calibration_stage,
            FirstTurnCalibrationStage::Idle
        );

        let (_, loaded) = crate::config::load(Some(config_string.as_str())).expect("load config");
        let personalization = loaded
            .memory
            .personalization
            .expect("personalization should persist");
        assert_eq!(
            personalization.response_density,
            Some(ResponseDensity::Concise)
        );
        assert_eq!(personalization.preferred_name.as_deref(), Some("Operator"));
        assert_eq!(loaded.cli.personality, Some(PromptPersonality::Pragmatist));

        let identity =
            std::fs::read_to_string(temp_root.join("IDENTITY.md")).expect("read identity");
        let user = std::fs::read_to_string(temp_root.join("USER.md")).expect("read user");
        assert!(identity.contains("- Name: Aster"));
        assert!(identity.contains("- Creature: fox"));
        assert!(identity.contains("- Vibe: steady"));
        assert!(identity.contains("- Emoji: ✨"));
        assert!(user.contains("- Preferred address: Operator"));
        assert!(identity.contains("identity_name: Aster"));
        assert!(identity.contains("identity_creature: fox"));
        assert!(identity.contains("identity_vibe: steady"));
        assert!(identity.contains("identity_emoji: ✨"));
        assert!(user.contains("preferred_address: Operator"));

        let rendered = rendered_lines(&mut message_list);
        assert!(rendered.contains("First-turn calibration updated"));
        assert!(rendered.contains("Personalization: concise"));
        assert!(rendered.contains("response density: concise"));

        let _ = std::fs::remove_dir_all(&temp_root);
    }

    #[test]
    fn skills_and_mcp_meta_options_toggle_all_and_skip_all() {
        let mut state = StartupOnboardingState::new(2, None);
        state.stage = StartupOnboardingStage::Skills;
        state.selection = 1;
        assert!(state.toggle_current_option());
        assert!(state.skill_enabled.iter().all(|enabled| !*enabled));
        state.selection = 2;
        assert!(state.toggle_current_option());
        assert!(!state.skill_enabled.iter().all(|enabled| !*enabled));
        assert!(!state.skill_enabled.iter().all(|enabled| *enabled));

        state.stage = StartupOnboardingStage::Mcp;
        state.selection = 0;
        assert!(state.toggle_current_option());
        assert!(state.mcp_choices.iter().all(|(_, enabled)| *enabled));
        state.selection = 3;
        assert!(state.toggle_current_option());
        assert!(!state.mcp_choices.iter().all(|(_, enabled)| *enabled));
    }
}
