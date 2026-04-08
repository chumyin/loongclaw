use std::fs;

use clap::{Args, Subcommand, ValueEnum};
use loongclaw_contracts::{
    WORK_UNIT_SPLIT_MIN_CHILDREN, WorkSourceKind, WorkUnitKind, WorkUnitPriority,
    WorkUnitRetryPolicy, WorkUnitSnapshot, WorkUnitSourceRef, WorkUnitStatus,
};
use loongclaw_spec::CliResult;
use serde::{Deserialize, Serialize};

use crate::mvp;

#[derive(Subcommand, Debug, Clone, PartialEq, Eq)]
pub enum WorkUnitCommands {
    /// Create one durable work unit record
    Create(WorkUnitCreateCommandOptions),
    /// Show one durable work unit record
    Show(WorkUnitShowCommandOptions),
    /// List durable work units
    List(WorkUnitListCommandOptions),
    /// List recent events for one durable work unit
    Events(WorkUnitEventsCommandOptions),
    /// Claim the next ready work unit for a worker lease
    Claim(WorkUnitClaimCommandOptions),
    /// Transition one leased work unit into the running state
    Start(WorkUnitStartCommandOptions),
    /// Refresh heartbeat and lease expiry for one leased work unit
    Heartbeat(WorkUnitHeartbeatCommandOptions),
    /// Mark one leased work unit as completed, retried, failed, or cancelled
    Complete(WorkUnitCompleteCommandOptions),
    /// Recover expired leases back into retry or terminal failure states
    Recover(WorkUnitRecoverCommandOptions),
    /// Archive one terminal work unit
    Archive(WorkUnitArchiveCommandOptions),
    /// Create a child work unit beneath an existing parent work unit
    SpawnChild(WorkUnitSpawnChildCommandOptions),
    /// Split one parent work unit into multiple child work units
    Split(WorkUnitSplitCommandOptions),
    /// Update one parent plan and optionally reorder its child work units
    Replan(WorkUnitReplanCommandOptions),
    /// Reorder the existing child work units beneath one parent work unit
    Resequence(WorkUnitResequenceCommandOptions),
    /// Replace one obsolete work unit with another durable work unit
    Supersede(WorkUnitSupersedeCommandOptions),
    /// Assign or clear a durable work-unit owner without taking a runtime lease
    Assign(WorkUnitAssignCommandOptions),
    /// Update mutable orchestration fields on a durable work unit
    Update(WorkUnitUpdateCommandOptions),
    /// Request review on a durable work unit and move it into review-wait state
    RequestReview(WorkUnitRequestReviewCommandOptions),
    /// Record a review decision for a durable work unit
    Review(WorkUnitReviewDecisionCommandOptions),
    /// Add one blocking dependency edge between two durable work units
    Depend(WorkUnitDependCommandOptions),
    /// Remove one blocking dependency edge between two durable work units
    Undepend(WorkUnitUndependCommandOptions),
    /// Append one orchestration note event to a durable work unit
    Note(WorkUnitNoteCommandOptions),
    /// Summarize durable runtime queue health
    Health(WorkUnitHealthCommandOptions),
}

#[derive(Args, Debug, Clone, PartialEq, Eq)]
pub struct WorkUnitCreateCommandOptions {
    #[arg(long)]
    pub config: Option<String>,
    #[arg(long)]
    pub id: Option<String>,
    #[arg(long, value_enum)]
    pub kind: WorkUnitKindArg,
    #[arg(long)]
    pub title: String,
    #[arg(long)]
    pub description: String,
    #[arg(long, value_enum, default_value_t = WorkUnitStatusArg::Ready)]
    pub status: WorkUnitStatusArg,
    #[arg(long, value_enum, default_value_t = WorkUnitPriorityArg::Normal)]
    pub priority: WorkUnitPriorityArg,
    #[arg(long, default_value_t = 3)]
    pub max_attempts: u32,
    #[arg(long, default_value_t = 1000)]
    pub initial_backoff_ms: u64,
    #[arg(long, default_value_t = 60_000)]
    pub max_backoff_ms: u64,
    #[arg(long)]
    pub next_run_at_ms: Option<i64>,
    #[arg(long)]
    pub actor: Option<String>,
    #[arg(long, value_enum, default_value_t = WorkSourceKindArg::Manual)]
    pub source_kind: WorkSourceKindArg,
    #[arg(long)]
    pub project_id: Option<String>,
    #[arg(long)]
    pub channel_id: Option<String>,
    #[arg(long)]
    pub thread_id: Option<String>,
    #[arg(long)]
    pub message_id: Option<String>,
    #[arg(long)]
    pub external_ref: Option<String>,
    #[arg(long)]
    pub source_url: Option<String>,
    #[arg(long)]
    pub parent_work_unit_id: Option<String>,
    #[arg(long, default_value_t = false)]
    pub json: bool,
}

#[derive(Args, Debug, Clone, PartialEq, Eq)]
pub struct WorkUnitShowCommandOptions {
    #[arg(long)]
    pub config: Option<String>,
    #[arg(long)]
    pub id: String,
    #[arg(long, default_value_t = false)]
    pub json: bool,
}

#[derive(Args, Debug, Clone, PartialEq, Eq)]
pub struct WorkUnitListCommandOptions {
    #[arg(long)]
    pub config: Option<String>,
    #[arg(long, value_enum)]
    pub status: Option<WorkUnitStatusArg>,
    #[arg(long, default_value_t = false)]
    pub include_archived: bool,
    #[arg(long, default_value_t = 50)]
    pub limit: usize,
    #[arg(long, default_value_t = false)]
    pub json: bool,
}

#[derive(Args, Debug, Clone, PartialEq, Eq)]
pub struct WorkUnitEventsCommandOptions {
    #[arg(long)]
    pub config: Option<String>,
    #[arg(long)]
    pub id: String,
    #[arg(long, default_value_t = 20)]
    pub limit: usize,
    #[arg(long, default_value_t = false)]
    pub json: bool,
}

#[derive(Args, Debug, Clone, PartialEq, Eq)]
pub struct WorkUnitClaimCommandOptions {
    #[arg(long)]
    pub config: Option<String>,
    #[arg(long)]
    pub owner: String,
    #[arg(long, default_value_t = 300_000)]
    pub ttl_ms: u64,
    #[arg(long)]
    pub actor: Option<String>,
    #[arg(long)]
    pub now_ms: Option<i64>,
    #[arg(long, default_value_t = false)]
    pub json: bool,
}

#[derive(Args, Debug, Clone, PartialEq, Eq)]
pub struct WorkUnitStartCommandOptions {
    #[arg(long)]
    pub config: Option<String>,
    #[arg(long)]
    pub id: String,
    #[arg(long)]
    pub owner: String,
    #[arg(long)]
    pub actor: Option<String>,
    #[arg(long)]
    pub now_ms: Option<i64>,
    #[arg(long, default_value_t = false)]
    pub json: bool,
}

#[derive(Args, Debug, Clone, PartialEq, Eq)]
pub struct WorkUnitHeartbeatCommandOptions {
    #[arg(long)]
    pub config: Option<String>,
    #[arg(long)]
    pub id: String,
    #[arg(long)]
    pub owner: String,
    #[arg(long, default_value_t = 300_000)]
    pub ttl_ms: u64,
    #[arg(long)]
    pub actor: Option<String>,
    #[arg(long)]
    pub now_ms: Option<i64>,
    #[arg(long, default_value_t = false)]
    pub json: bool,
}

#[derive(Args, Debug, Clone, PartialEq, Eq)]
pub struct WorkUnitCompleteCommandOptions {
    #[arg(long)]
    pub config: Option<String>,
    #[arg(long)]
    pub id: String,
    #[arg(long)]
    pub owner: String,
    #[arg(long, value_enum)]
    pub disposition: WorkUnitDispositionArg,
    #[arg(long)]
    pub actor: Option<String>,
    #[arg(long)]
    pub now_ms: Option<i64>,
    #[arg(long)]
    pub next_run_at_ms: Option<i64>,
    #[arg(long)]
    pub result_payload_json: Option<String>,
    #[arg(long)]
    pub error: Option<String>,
    #[arg(long, default_value_t = false)]
    pub json: bool,
}

#[derive(Args, Debug, Clone, PartialEq, Eq)]
pub struct WorkUnitRecoverCommandOptions {
    #[arg(long)]
    pub config: Option<String>,
    #[arg(long)]
    pub actor: Option<String>,
    #[arg(long)]
    pub now_ms: Option<i64>,
    #[arg(long, default_value_t = false)]
    pub json: bool,
}

#[derive(Args, Debug, Clone, PartialEq, Eq)]
pub struct WorkUnitArchiveCommandOptions {
    #[arg(long)]
    pub config: Option<String>,
    #[arg(long)]
    pub id: String,
    #[arg(long)]
    pub actor: Option<String>,
    #[arg(long)]
    pub now_ms: Option<i64>,
    #[arg(long, default_value_t = false)]
    pub json: bool,
}

#[derive(Args, Debug, Clone, PartialEq, Eq)]
pub struct WorkUnitSpawnChildCommandOptions {
    #[arg(long)]
    pub config: Option<String>,
    #[arg(long)]
    pub parent_id: String,
    #[arg(long)]
    pub id: Option<String>,
    #[arg(long, value_enum)]
    pub kind: WorkUnitKindArg,
    #[arg(long)]
    pub title: String,
    #[arg(long)]
    pub description: String,
    #[arg(long, value_enum, default_value_t = WorkUnitStatusArg::Ready)]
    pub status: WorkUnitStatusArg,
    #[arg(long, value_enum)]
    pub priority: Option<WorkUnitPriorityArg>,
    #[arg(long)]
    pub max_attempts: Option<u32>,
    #[arg(long)]
    pub initial_backoff_ms: Option<u64>,
    #[arg(long)]
    pub max_backoff_ms: Option<u64>,
    #[arg(long)]
    pub next_run_at_ms: Option<i64>,
    #[arg(long)]
    pub actor: Option<String>,
    #[arg(
        long,
        default_value_t = true,
        action = clap::ArgAction::Set,
        num_args = 0..=1,
        default_missing_value = "true"
    )]
    pub block_parent: bool,
    #[arg(long, value_enum)]
    pub source_kind: Option<WorkSourceKindArg>,
    #[arg(long)]
    pub project_id: Option<String>,
    #[arg(long)]
    pub channel_id: Option<String>,
    #[arg(long)]
    pub thread_id: Option<String>,
    #[arg(long)]
    pub message_id: Option<String>,
    #[arg(long)]
    pub external_ref: Option<String>,
    #[arg(long)]
    pub source_url: Option<String>,
    #[arg(long, default_value_t = false)]
    pub json: bool,
}

#[derive(Args, Debug, Clone, PartialEq, Eq)]
pub struct WorkUnitSplitCommandOptions {
    #[arg(long)]
    pub config: Option<String>,
    #[arg(long)]
    pub parent_id: String,
    #[arg(long)]
    pub children_json: Option<String>,
    #[arg(long)]
    pub children_path: Option<String>,
    #[arg(
        long,
        default_value_t = true,
        action = clap::ArgAction::Set,
        num_args = 0..=1,
        default_missing_value = "true"
    )]
    pub block_parent: bool,
    #[arg(long)]
    pub actor: Option<String>,
    #[arg(long, default_value_t = false)]
    pub json: bool,
}

#[derive(Args, Debug, Clone, PartialEq, Eq)]
pub struct WorkUnitSupersedeCommandOptions {
    #[arg(long)]
    pub config: Option<String>,
    #[arg(long)]
    pub obsolete_id: String,
    #[arg(long)]
    pub replacement_id: String,
    #[arg(long)]
    pub actor: Option<String>,
    #[arg(long)]
    pub now_ms: Option<i64>,
    #[arg(long, default_value_t = false)]
    pub json: bool,
}

#[derive(Args, Debug, Clone, PartialEq, Eq)]
pub struct WorkUnitResequenceCommandOptions {
    #[arg(long)]
    pub config: Option<String>,
    #[arg(long)]
    pub parent_id: String,
    #[arg(long)]
    pub ordered_child_ids_json: Option<String>,
    #[arg(long)]
    pub ordered_child_ids_path: Option<String>,
    #[arg(long)]
    pub actor: Option<String>,
    #[arg(long)]
    pub now_ms: Option<i64>,
    #[arg(long, default_value_t = false)]
    pub json: bool,
}

#[derive(Args, Debug, Clone, PartialEq, Eq)]
pub struct WorkUnitReplanCommandOptions {
    #[arg(long)]
    pub config: Option<String>,
    #[arg(long)]
    pub id: String,
    #[arg(long)]
    pub title: Option<String>,
    #[arg(long)]
    pub description: Option<String>,
    #[arg(long, value_enum)]
    pub status: Option<WorkUnitStatusArg>,
    #[arg(long, value_enum)]
    pub priority: Option<WorkUnitPriorityArg>,
    #[arg(long)]
    pub next_run_at_ms: Option<i64>,
    #[arg(long)]
    pub blocking_reason: Option<String>,
    #[arg(long, default_value_t = false)]
    pub clear_blocking_reason: bool,
    #[arg(long)]
    pub ordered_child_ids_json: Option<String>,
    #[arg(long)]
    pub ordered_child_ids_path: Option<String>,
    #[arg(long)]
    pub actor: Option<String>,
    #[arg(long)]
    pub now_ms: Option<i64>,
    #[arg(long, default_value_t = false)]
    pub json: bool,
}

#[derive(Args, Debug, Clone, PartialEq, Eq)]
pub struct WorkUnitAssignCommandOptions {
    #[arg(long)]
    pub config: Option<String>,
    #[arg(long)]
    pub id: String,
    #[arg(long)]
    pub assigned_to: Option<String>,
    #[arg(long)]
    pub actor: Option<String>,
    #[arg(long)]
    pub now_ms: Option<i64>,
    #[arg(long, default_value_t = false)]
    pub json: bool,
}

#[derive(Args, Debug, Clone, PartialEq, Eq)]
pub struct WorkUnitUpdateCommandOptions {
    #[arg(long)]
    pub config: Option<String>,
    #[arg(long)]
    pub id: String,
    #[arg(long)]
    pub title: Option<String>,
    #[arg(long)]
    pub description: Option<String>,
    #[arg(long, value_enum)]
    pub status: Option<WorkUnitStatusArg>,
    #[arg(long, value_enum)]
    pub priority: Option<WorkUnitPriorityArg>,
    #[arg(long)]
    pub next_run_at_ms: Option<i64>,
    #[arg(long)]
    pub blocking_reason: Option<String>,
    #[arg(long, default_value_t = false)]
    pub clear_blocking_reason: bool,
    #[arg(long)]
    pub actor: Option<String>,
    #[arg(long)]
    pub now_ms: Option<i64>,
    #[arg(long, default_value_t = false)]
    pub json: bool,
}

#[derive(Args, Debug, Clone, PartialEq, Eq)]
pub struct WorkUnitRequestReviewCommandOptions {
    #[arg(long)]
    pub config: Option<String>,
    #[arg(long)]
    pub id: String,
    #[arg(long)]
    pub requested_by: Option<String>,
    #[arg(long)]
    pub reviewer: Option<String>,
    #[arg(long)]
    pub summary: Option<String>,
    #[arg(long)]
    pub now_ms: Option<i64>,
    #[arg(long, default_value_t = false)]
    pub json: bool,
}

#[derive(Args, Debug, Clone, PartialEq, Eq)]
pub struct WorkUnitReviewDecisionCommandOptions {
    #[arg(long)]
    pub config: Option<String>,
    #[arg(long)]
    pub id: String,
    #[arg(long, value_enum)]
    pub decision: WorkUnitReviewDecisionArg,
    #[arg(long)]
    pub reviewer: Option<String>,
    #[arg(long)]
    pub summary: Option<String>,
    #[arg(long)]
    pub now_ms: Option<i64>,
    #[arg(long, default_value_t = false)]
    pub json: bool,
}

#[derive(Args, Debug, Clone, PartialEq, Eq)]
pub struct WorkUnitDependCommandOptions {
    #[arg(long)]
    pub config: Option<String>,
    #[arg(long)]
    pub blocking_id: String,
    #[arg(long)]
    pub blocked_id: String,
    #[arg(long)]
    pub actor: Option<String>,
    #[arg(long)]
    pub now_ms: Option<i64>,
    #[arg(long, default_value_t = false)]
    pub json: bool,
}

#[derive(Args, Debug, Clone, PartialEq, Eq)]
pub struct WorkUnitUndependCommandOptions {
    #[arg(long)]
    pub config: Option<String>,
    #[arg(long)]
    pub blocking_id: String,
    #[arg(long)]
    pub blocked_id: String,
    #[arg(long)]
    pub actor: Option<String>,
    #[arg(long)]
    pub now_ms: Option<i64>,
    #[arg(long, default_value_t = false)]
    pub json: bool,
}

#[derive(Args, Debug, Clone, PartialEq, Eq)]
pub struct WorkUnitNoteCommandOptions {
    #[arg(long)]
    pub config: Option<String>,
    #[arg(long)]
    pub id: String,
    #[arg(long)]
    pub actor: Option<String>,
    #[arg(long)]
    pub note: String,
    #[arg(long)]
    pub now_ms: Option<i64>,
    #[arg(long, default_value_t = false)]
    pub json: bool,
}

#[derive(Args, Debug, Clone, PartialEq, Eq)]
pub struct WorkUnitHealthCommandOptions {
    #[arg(long)]
    pub config: Option<String>,
    #[arg(long)]
    pub now_ms: Option<i64>,
    #[arg(long, default_value_t = false)]
    pub json: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum WorkUnitKindArg {
    Feature,
    Issue,
    Review,
    Ops,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum WorkSourceKindArg {
    Manual,
    Discord,
    Github,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
#[value(rename_all = "snake_case")]
pub enum WorkUnitStatusArg {
    Captured,
    Triaged,
    Ready,
    Leased,
    Running,
    WaitingExternal,
    WaitingReview,
    RetryPending,
    Completed,
    FailedTerminal,
    Cancelled,
    Archived,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
#[value(rename_all = "snake_case")]
pub enum WorkUnitPriorityArg {
    Low,
    Normal,
    High,
    Critical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
#[value(rename_all = "snake_case")]
pub enum WorkUnitDispositionArg {
    Completed,
    RetryPending,
    FailedTerminal,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum WorkUnitReviewDecisionArg {
    Approve,
    RequestChanges,
    Reject,
}

impl From<WorkUnitKindArg> for WorkUnitKind {
    fn from(value: WorkUnitKindArg) -> Self {
        match value {
            WorkUnitKindArg::Feature => Self::Feature,
            WorkUnitKindArg::Issue => Self::Issue,
            WorkUnitKindArg::Review => Self::Review,
            WorkUnitKindArg::Ops => Self::Ops,
        }
    }
}

impl From<WorkSourceKindArg> for WorkSourceKind {
    fn from(value: WorkSourceKindArg) -> Self {
        match value {
            WorkSourceKindArg::Manual => Self::Manual,
            WorkSourceKindArg::Discord => Self::Discord,
            WorkSourceKindArg::Github => Self::Github,
        }
    }
}

impl From<WorkUnitStatusArg> for WorkUnitStatus {
    fn from(value: WorkUnitStatusArg) -> Self {
        match value {
            WorkUnitStatusArg::Captured => Self::Captured,
            WorkUnitStatusArg::Triaged => Self::Triaged,
            WorkUnitStatusArg::Ready => Self::Ready,
            WorkUnitStatusArg::Leased => Self::Leased,
            WorkUnitStatusArg::Running => Self::Running,
            WorkUnitStatusArg::WaitingExternal => Self::WaitingExternal,
            WorkUnitStatusArg::WaitingReview => Self::WaitingReview,
            WorkUnitStatusArg::RetryPending => Self::RetryPending,
            WorkUnitStatusArg::Completed => Self::Completed,
            WorkUnitStatusArg::FailedTerminal => Self::FailedTerminal,
            WorkUnitStatusArg::Cancelled => Self::Cancelled,
            WorkUnitStatusArg::Archived => Self::Archived,
        }
    }
}

impl From<WorkUnitPriorityArg> for WorkUnitPriority {
    fn from(value: WorkUnitPriorityArg) -> Self {
        match value {
            WorkUnitPriorityArg::Low => Self::Low,
            WorkUnitPriorityArg::Normal => Self::Normal,
            WorkUnitPriorityArg::High => Self::High,
            WorkUnitPriorityArg::Critical => Self::Critical,
        }
    }
}

impl From<WorkUnitDispositionArg> for mvp::work::repository::WorkUnitCompletionDisposition {
    fn from(value: WorkUnitDispositionArg) -> Self {
        match value {
            WorkUnitDispositionArg::Completed => Self::Completed,
            WorkUnitDispositionArg::RetryPending => Self::RetryPending,
            WorkUnitDispositionArg::FailedTerminal => Self::FailedTerminal,
            WorkUnitDispositionArg::Cancelled => Self::Cancelled,
        }
    }
}

impl From<WorkUnitReviewDecisionArg> for mvp::work::repository::WorkUnitReviewDecision {
    fn from(value: WorkUnitReviewDecisionArg) -> Self {
        match value {
            WorkUnitReviewDecisionArg::Approve => Self::Approve,
            WorkUnitReviewDecisionArg::RequestChanges => Self::RequestChanges,
            WorkUnitReviewDecisionArg::Reject => Self::Reject,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkUnitSplitChildInput {
    #[serde(default)]
    id: Option<String>,
    kind: WorkUnitKind,
    title: String,
    description: String,
    #[serde(default)]
    status: Option<WorkUnitStatus>,
    #[serde(default)]
    priority: Option<WorkUnitPriority>,
    #[serde(default)]
    retry_policy: Option<WorkUnitRetryPolicy>,
    #[serde(default)]
    source_ref: Option<WorkUnitSourceRef>,
    #[serde(default)]
    next_run_at_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
struct WorkUnitSplitView {
    parent: WorkUnitSnapshot,
    children: Vec<WorkUnitSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
struct WorkUnitReplanView {
    parent: WorkUnitSnapshot,
    children: Vec<WorkUnitSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
struct WorkUnitResequenceView {
    parent: WorkUnitSnapshot,
    children: Vec<WorkUnitSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
struct WorkUnitSupersedeView {
    obsolete: WorkUnitSnapshot,
    replacement: WorkUnitSnapshot,
}

pub fn run_work_unit_cli(command: WorkUnitCommands) -> CliResult<()> {
    match command {
        WorkUnitCommands::Create(options) => run_create_command(options),
        WorkUnitCommands::Show(options) => run_show_command(options),
        WorkUnitCommands::List(options) => run_list_command(options),
        WorkUnitCommands::Events(options) => run_events_command(options),
        WorkUnitCommands::Claim(options) => run_claim_command(options),
        WorkUnitCommands::Start(options) => run_start_command(options),
        WorkUnitCommands::Heartbeat(options) => run_heartbeat_command(options),
        WorkUnitCommands::Complete(options) => run_complete_command(options),
        WorkUnitCommands::Recover(options) => run_recover_command(options),
        WorkUnitCommands::Archive(options) => run_archive_command(options),
        WorkUnitCommands::SpawnChild(options) => run_spawn_child_command(options),
        WorkUnitCommands::Split(options) => run_split_command(options),
        WorkUnitCommands::Replan(options) => run_replan_command(options),
        WorkUnitCommands::Resequence(options) => run_resequence_command(options),
        WorkUnitCommands::Supersede(options) => run_supersede_command(options),
        WorkUnitCommands::Assign(options) => run_assign_command(options),
        WorkUnitCommands::Update(options) => run_update_command(options),
        WorkUnitCommands::RequestReview(options) => run_request_review_command(options),
        WorkUnitCommands::Review(options) => run_review_decision_command(options),
        WorkUnitCommands::Depend(options) => run_depend_command(options),
        WorkUnitCommands::Undepend(options) => run_undepend_command(options),
        WorkUnitCommands::Note(options) => run_note_command(options),
        WorkUnitCommands::Health(options) => run_health_command(options),
    }
}

fn run_create_command(options: WorkUnitCreateCommandOptions) -> CliResult<()> {
    let repository = load_work_unit_repository(options.config.as_deref())?;
    let retry_policy = WorkUnitRetryPolicy {
        max_attempts: options.max_attempts,
        initial_backoff_ms: options.initial_backoff_ms,
        max_backoff_ms: options.max_backoff_ms,
    };
    let source_ref = WorkUnitSourceRef {
        source_kind: options.source_kind.into(),
        project_id: options.project_id,
        channel_id: options.channel_id,
        thread_id: options.thread_id,
        message_id: options.message_id,
        external_ref: options.external_ref,
        source_url: options.source_url,
    };
    let new_work_unit = mvp::work::repository::NewWorkUnitRecord {
        work_unit_id: options.id,
        kind: options.kind.into(),
        title: options.title,
        description: options.description,
        source_ref,
        status: options.status.into(),
        priority: options.priority.into(),
        retry_policy,
        parent_work_unit_id: options.parent_work_unit_id,
        plan_position: None,
        next_run_at_ms: options.next_run_at_ms,
    };
    let snapshot = repository.create_work_unit(new_work_unit, options.actor.as_deref())?;
    render_json_or_text(&snapshot, options.json, render_work_unit_snapshot_text)
}

fn run_show_command(options: WorkUnitShowCommandOptions) -> CliResult<()> {
    let repository = load_work_unit_repository(options.config.as_deref())?;
    let snapshot = repository.load_work_unit_snapshot(options.id.as_str())?;
    let Some(snapshot) = snapshot else {
        return Err(format!("work unit `{}` not found", options.id));
    };
    render_json_or_text(&snapshot, options.json, render_work_unit_snapshot_text)
}

fn run_list_command(options: WorkUnitListCommandOptions) -> CliResult<()> {
    let repository = load_work_unit_repository(options.config.as_deref())?;
    let query = mvp::work::repository::WorkUnitListQuery {
        status: options.status.map(WorkUnitStatus::from),
        include_archived: options.include_archived,
        limit: options.limit,
    };
    let snapshots = repository.list_work_units(query)?;
    render_json_or_text(&snapshots, options.json, |value| {
        render_work_unit_list_text(value.as_slice())
    })
}

fn run_events_command(options: WorkUnitEventsCommandOptions) -> CliResult<()> {
    let repository = load_work_unit_repository(options.config.as_deref())?;
    let events = repository.list_work_unit_events(options.id.as_str(), options.limit)?;
    render_json_or_text(&events, options.json, |value| {
        render_work_unit_events_text(value.as_slice())
    })
}

fn run_claim_command(options: WorkUnitClaimCommandOptions) -> CliResult<()> {
    let repository = load_work_unit_repository(options.config.as_deref())?;
    let request = mvp::work::repository::AcquireWorkUnitLeaseRequest {
        owner: options.owner,
        ttl_ms: options.ttl_ms,
        actor: options.actor,
        now_ms: options.now_ms,
    };
    let snapshot = repository.acquire_next_ready_lease(request)?;
    if options.json {
        let payload = serde_json::json!({ "claimed": snapshot.is_some(), "snapshot": snapshot });
        return print_json(payload);
    }
    let Some(snapshot) = snapshot else {
        println!("claimed=false");
        return Ok(());
    };
    println!("claimed=true");
    print!("{}", render_work_unit_snapshot_text(&snapshot));
    Ok(())
}

fn run_start_command(options: WorkUnitStartCommandOptions) -> CliResult<()> {
    let repository = load_work_unit_repository(options.config.as_deref())?;
    let request = mvp::work::repository::StartWorkUnitLeaseRequest {
        work_unit_id: options.id,
        owner: options.owner,
        actor: options.actor,
        now_ms: options.now_ms,
    };
    let snapshot = repository.mark_leased_running(request)?;
    let missing_message = "start did not find a matching active lease";
    render_optional_snapshot(snapshot, options.json, missing_message)
}

fn run_heartbeat_command(options: WorkUnitHeartbeatCommandOptions) -> CliResult<()> {
    let repository = load_work_unit_repository(options.config.as_deref())?;
    let request = mvp::work::repository::WorkUnitHeartbeatRequest {
        work_unit_id: options.id,
        owner: options.owner,
        ttl_ms: options.ttl_ms,
        actor: options.actor,
        now_ms: options.now_ms,
    };
    let snapshot = repository.heartbeat_lease(request)?;
    let missing_message = "heartbeat did not find a matching active lease";
    render_optional_snapshot(snapshot, options.json, missing_message)
}

fn run_complete_command(options: WorkUnitCompleteCommandOptions) -> CliResult<()> {
    let repository = load_work_unit_repository(options.config.as_deref())?;
    let result_payload_json = options
        .result_payload_json
        .as_deref()
        .map(parse_json_value)
        .transpose()?;
    let request = mvp::work::repository::CompleteWorkUnitRequest {
        work_unit_id: options.id,
        owner: options.owner,
        disposition: options.disposition.into(),
        actor: options.actor,
        now_ms: options.now_ms,
        next_run_at_ms: options.next_run_at_ms,
        result_payload_json,
        error: options.error,
    };
    let snapshot = repository.complete_work_unit(request)?;
    let missing_message = "complete did not find a matching active lease";
    render_optional_snapshot(snapshot, options.json, missing_message)
}

fn run_recover_command(options: WorkUnitRecoverCommandOptions) -> CliResult<()> {
    let repository = load_work_unit_repository(options.config.as_deref())?;
    let snapshots = repository.recover_expired_leases(options.actor.as_deref(), options.now_ms)?;
    render_json_or_text(&snapshots, options.json, |value| {
        render_work_unit_list_text(value.as_slice())
    })
}

fn run_archive_command(options: WorkUnitArchiveCommandOptions) -> CliResult<()> {
    let repository = load_work_unit_repository(options.config.as_deref())?;
    let request = mvp::work::repository::ArchiveWorkUnitRequest {
        work_unit_id: options.id,
        actor: options.actor,
        now_ms: options.now_ms,
    };
    let snapshot = repository.archive_work_unit(request)?;
    let missing_message =
        "archive failed: work unit not found, already archived, or not in a terminal state";
    render_optional_snapshot(snapshot, options.json, missing_message)
}

fn run_spawn_child_command(options: WorkUnitSpawnChildCommandOptions) -> CliResult<()> {
    let repository = load_work_unit_repository(options.config.as_deref())?;
    let priority_is_overridden = options.priority.is_some();
    let retry_policy_is_overridden = options.max_attempts.is_some()
        || options.initial_backoff_ms.is_some()
        || options.max_backoff_ms.is_some();
    let source_ref_is_overridden = options.source_kind.is_some()
        || options.project_id.is_some()
        || options.channel_id.is_some()
        || options.thread_id.is_some()
        || options.message_id.is_some()
        || options.external_ref.is_some()
        || options.source_url.is_some();
    let retry_policy = build_retry_policy_for_child(
        options.max_attempts,
        options.initial_backoff_ms,
        options.max_backoff_ms,
    );
    let source_ref = build_source_ref_for_child(
        options.source_kind,
        options.project_id,
        options.channel_id,
        options.thread_id,
        options.message_id,
        options.external_ref,
        options.source_url,
    );
    let child = mvp::work::repository::NewWorkUnitRecord {
        work_unit_id: options.id,
        kind: options.kind.into(),
        title: options.title,
        description: options.description,
        source_ref,
        status: options.status.into(),
        priority: options
            .priority
            .map(WorkUnitPriority::from)
            .unwrap_or(WorkUnitPriority::Normal),
        retry_policy,
        parent_work_unit_id: None,
        plan_position: None,
        next_run_at_ms: options.next_run_at_ms,
    };
    let request = mvp::work::repository::CreateChildWorkUnitRequest {
        parent_work_unit_id: options.parent_id,
        child,
        inherit_parent_source_ref: !source_ref_is_overridden,
        inherit_parent_retry_policy: !retry_policy_is_overridden,
        inherit_parent_priority: !priority_is_overridden,
        block_parent: options.block_parent,
        actor: options.actor,
    };
    let snapshot = repository.create_child_work_unit(request)?;
    render_json_or_text(&snapshot, options.json, render_work_unit_snapshot_text)
}

fn run_split_command(options: WorkUnitSplitCommandOptions) -> CliResult<()> {
    let child_inputs = parse_split_children_inputs(
        options.children_json.as_deref(),
        options.children_path.as_deref(),
    )?;
    let repository = load_work_unit_repository(options.config.as_deref())?;
    let mut child_requests = Vec::with_capacity(child_inputs.len());

    for child_input in child_inputs {
        let source_ref_is_overridden = child_input.source_ref.is_some();
        let retry_policy_is_overridden = child_input.retry_policy.is_some();
        let priority_is_overridden = child_input.priority.is_some();
        let child = mvp::work::repository::NewWorkUnitRecord {
            work_unit_id: child_input.id,
            kind: child_input.kind,
            title: child_input.title,
            description: child_input.description,
            source_ref: child_input.source_ref.unwrap_or_default(),
            status: child_input.status.unwrap_or(WorkUnitStatus::Ready),
            priority: child_input.priority.unwrap_or(WorkUnitPriority::Normal),
            retry_policy: child_input.retry_policy.unwrap_or_default(),
            parent_work_unit_id: None,
            plan_position: None,
            next_run_at_ms: child_input.next_run_at_ms,
        };
        let child_request = mvp::work::repository::SplitWorkUnitChildRequest {
            child,
            inherit_parent_source_ref: !source_ref_is_overridden,
            inherit_parent_retry_policy: !retry_policy_is_overridden,
            inherit_parent_priority: !priority_is_overridden,
        };
        child_requests.push(child_request);
    }

    let result = repository.split_work_unit(mvp::work::repository::SplitWorkUnitRequest {
        parent_work_unit_id: options.parent_id,
        children: child_requests,
        block_parent: options.block_parent,
        actor: options.actor,
    })?;
    let view = WorkUnitSplitView {
        parent: result.parent,
        children: result.children,
    };
    render_json_or_text(&view, options.json, render_work_unit_split_text)
}

fn run_replan_command(options: WorkUnitReplanCommandOptions) -> CliResult<()> {
    let ordered_child_work_unit_ids = match (
        options.ordered_child_ids_json.as_deref(),
        options.ordered_child_ids_path.as_deref(),
    ) {
        (None, None) => None,
        _ => Some(parse_resequence_child_ids(
            options.ordered_child_ids_json.as_deref(),
            options.ordered_child_ids_path.as_deref(),
        )?),
    };
    let repository = load_work_unit_repository(options.config.as_deref())?;
    let request = mvp::work::repository::ReplanWorkUnitRequest {
        work_unit_id: options.id,
        title: options.title,
        description: options.description,
        status: options.status.map(WorkUnitStatus::from),
        priority: options.priority.map(WorkUnitPriority::from),
        next_run_at_ms: options.next_run_at_ms,
        blocking_reason: options.blocking_reason,
        clear_blocking_reason: options.clear_blocking_reason,
        ordered_child_work_unit_ids,
        actor: options.actor,
        now_ms: options.now_ms,
    };
    let result = repository.replan_work_unit(request)?;
    let Some(result) = result else {
        return Err("replan failed: work unit not found or archived".to_owned());
    };
    let view = WorkUnitReplanView {
        parent: result.parent,
        children: result.children,
    };
    render_json_or_text(&view, options.json, render_work_unit_replan_text)
}

fn run_resequence_command(options: WorkUnitResequenceCommandOptions) -> CliResult<()> {
    let ordered_child_work_unit_ids = parse_resequence_child_ids(
        options.ordered_child_ids_json.as_deref(),
        options.ordered_child_ids_path.as_deref(),
    )?;
    let repository = load_work_unit_repository(options.config.as_deref())?;
    let request = mvp::work::repository::ResequenceChildWorkUnitsRequest {
        parent_work_unit_id: options.parent_id,
        ordered_child_work_unit_ids,
        actor: options.actor,
        now_ms: options.now_ms,
    };
    let result = repository.resequence_child_work_units(request)?;
    let view = WorkUnitResequenceView {
        parent: result.parent,
        children: result.children,
    };
    render_json_or_text(&view, options.json, render_work_unit_resequence_text)
}

fn run_supersede_command(options: WorkUnitSupersedeCommandOptions) -> CliResult<()> {
    let repository = load_work_unit_repository(options.config.as_deref())?;
    let request = mvp::work::repository::SupersedeWorkUnitRequest {
        obsolete_work_unit_id: options.obsolete_id,
        replacement_work_unit_id: options.replacement_id,
        actor: options.actor,
        now_ms: options.now_ms,
    };
    let result = repository.supersede_work_unit(request)?;
    let view = WorkUnitSupersedeView {
        obsolete: result.obsolete,
        replacement: result.replacement,
    };
    render_json_or_text(&view, options.json, render_work_unit_supersede_text)
}

fn run_assign_command(options: WorkUnitAssignCommandOptions) -> CliResult<()> {
    let repository = load_work_unit_repository(options.config.as_deref())?;
    let request = mvp::work::repository::AssignWorkUnitRequest {
        work_unit_id: options.id,
        assigned_to: options.assigned_to,
        actor: options.actor,
        now_ms: options.now_ms,
    };
    let snapshot = repository.assign_work_unit(request)?;
    let missing_message = "assign failed: work unit not found or archived";
    render_optional_snapshot(snapshot, options.json, missing_message)
}

fn run_update_command(options: WorkUnitUpdateCommandOptions) -> CliResult<()> {
    let repository = load_work_unit_repository(options.config.as_deref())?;
    let request = mvp::work::repository::UpdateWorkUnitRequest {
        work_unit_id: options.id,
        title: options.title,
        description: options.description,
        status: options.status.map(WorkUnitStatus::from),
        priority: options.priority.map(WorkUnitPriority::from),
        next_run_at_ms: options.next_run_at_ms,
        blocking_reason: options.blocking_reason,
        clear_blocking_reason: options.clear_blocking_reason,
        actor: options.actor,
        now_ms: options.now_ms,
    };
    let snapshot = repository.update_work_unit(request)?;
    let missing_message = "update failed: work unit not found or archived";
    render_optional_snapshot(snapshot, options.json, missing_message)
}

fn run_request_review_command(options: WorkUnitRequestReviewCommandOptions) -> CliResult<()> {
    let repository = load_work_unit_repository(options.config.as_deref())?;
    let request = mvp::work::repository::RequestWorkUnitReviewRequest {
        work_unit_id: options.id,
        requested_by: options.requested_by,
        reviewer: options.reviewer,
        summary: options.summary,
        now_ms: options.now_ms,
    };
    let snapshot = repository.request_work_unit_review(request)?;
    render_optional_snapshot(snapshot, options.json, "request-review")
}

fn run_review_decision_command(options: WorkUnitReviewDecisionCommandOptions) -> CliResult<()> {
    let repository = load_work_unit_repository(options.config.as_deref())?;
    let request = mvp::work::repository::RecordWorkUnitReviewDecisionRequest {
        work_unit_id: options.id,
        reviewer: options.reviewer,
        decision: options.decision.into(),
        summary: options.summary,
        now_ms: options.now_ms,
    };
    let snapshot = repository.record_work_unit_review_decision(request)?;
    render_optional_snapshot(snapshot, options.json, "review")
}

fn run_depend_command(options: WorkUnitDependCommandOptions) -> CliResult<()> {
    let repository = load_work_unit_repository(options.config.as_deref())?;
    let request = mvp::work::repository::AddWorkUnitDependencyRequest {
        blocking_work_unit_id: options.blocking_id,
        blocked_work_unit_id: options.blocked_id,
        actor: options.actor,
        now_ms: options.now_ms,
    };
    let snapshot = repository.add_dependency(request)?;
    let missing_message = "depend failed: blocked work unit not found after dependency update";
    render_optional_snapshot(snapshot, options.json, missing_message)
}

fn run_undepend_command(options: WorkUnitUndependCommandOptions) -> CliResult<()> {
    let repository = load_work_unit_repository(options.config.as_deref())?;
    let request = mvp::work::repository::RemoveWorkUnitDependencyRequest {
        blocking_work_unit_id: options.blocking_id,
        blocked_work_unit_id: options.blocked_id,
        actor: options.actor,
        now_ms: options.now_ms,
    };
    let snapshot = repository.remove_dependency(request)?;
    let missing_message = "undepend failed: blocked work unit not found";
    render_optional_snapshot(snapshot, options.json, missing_message)
}

fn run_note_command(options: WorkUnitNoteCommandOptions) -> CliResult<()> {
    let repository = load_work_unit_repository(options.config.as_deref())?;
    let request = mvp::work::repository::AppendWorkUnitNoteRequest {
        work_unit_id: options.id,
        actor: options.actor,
        note: options.note,
        now_ms: options.now_ms,
    };
    let event = repository.append_note(request)?;
    let Some(event) = event else {
        return Err("note failed: work unit not found or archived".to_owned());
    };
    render_json_or_text(&event, options.json, render_single_work_unit_event_text)
}

fn run_health_command(options: WorkUnitHealthCommandOptions) -> CliResult<()> {
    let repository = load_work_unit_repository(options.config.as_deref())?;
    let health = repository.load_runtime_health(options.now_ms)?;
    render_json_or_text(&health, options.json, render_work_unit_health_text)
}

fn load_work_unit_repository(
    config_path: Option<&str>,
) -> CliResult<mvp::work::repository::WorkUnitRepository> {
    #[cfg(not(feature = "memory-sqlite"))]
    {
        let _ = config_path;
        Err("work unit runtime requires feature `memory-sqlite`".to_owned())
    }

    #[cfg(feature = "memory-sqlite")]
    {
        let (_, config) = mvp::config::load(config_path)?;
        let memory_config =
            mvp::memory::runtime_config::MemoryRuntimeConfig::from_memory_config(&config.memory);
        mvp::work::repository::WorkUnitRepository::new(&memory_config)
    }
}

fn build_retry_policy_for_child(
    max_attempts: Option<u32>,
    initial_backoff_ms: Option<u64>,
    max_backoff_ms: Option<u64>,
) -> WorkUnitRetryPolicy {
    let mut retry_policy = WorkUnitRetryPolicy::default();
    if let Some(max_attempts) = max_attempts {
        retry_policy.max_attempts = max_attempts;
    }
    if let Some(initial_backoff_ms) = initial_backoff_ms {
        retry_policy.initial_backoff_ms = initial_backoff_ms;
    }
    if let Some(max_backoff_ms) = max_backoff_ms {
        retry_policy.max_backoff_ms = max_backoff_ms;
    }
    retry_policy
}

fn build_source_ref_for_child(
    source_kind: Option<WorkSourceKindArg>,
    project_id: Option<String>,
    channel_id: Option<String>,
    thread_id: Option<String>,
    message_id: Option<String>,
    external_ref: Option<String>,
    source_url: Option<String>,
) -> WorkUnitSourceRef {
    let mut source_ref = WorkUnitSourceRef::default();
    if let Some(source_kind) = source_kind {
        source_ref.source_kind = source_kind.into();
    }
    source_ref.project_id = project_id;
    source_ref.channel_id = channel_id;
    source_ref.thread_id = thread_id;
    source_ref.message_id = message_id;
    source_ref.external_ref = external_ref;
    source_ref.source_url = source_url;
    source_ref
}

fn render_optional_snapshot(
    snapshot: Option<WorkUnitSnapshot>,
    as_json: bool,
    missing_message: &str,
) -> CliResult<()> {
    let Some(snapshot) = snapshot else {
        return Err(missing_message.to_owned());
    };
    render_json_or_text(&snapshot, as_json, render_work_unit_snapshot_text)
}

fn render_json_or_text<T>(
    value: &T,
    as_json: bool,
    render_text: impl FnOnce(&T) -> String,
) -> CliResult<()>
where
    T: serde::Serialize,
{
    if as_json {
        let payload = serde_json::to_string_pretty(value)
            .map_err(|error| format!("serialize work unit output failed: {error}"))?;
        println!("{payload}");
        return Ok(());
    }
    print!("{}", render_text(value));
    Ok(())
}

fn print_json(value: serde_json::Value) -> CliResult<()> {
    let payload = serde_json::to_string_pretty(&value)
        .map_err(|error| format!("serialize work unit json payload failed: {error}"))?;
    println!("{payload}");
    Ok(())
}

fn parse_json_value(raw: &str) -> CliResult<serde_json::Value> {
    serde_json::from_str(raw).map_err(|error| format!("parse result payload json failed: {error}"))
}

fn parse_split_children_inputs(
    children_json: Option<&str>,
    children_path: Option<&str>,
) -> CliResult<Vec<WorkUnitSplitChildInput>> {
    match (children_json, children_path) {
        (Some(_), Some(_)) => {
            Err("provide either --children-json or --children-path, but not both".to_owned())
        }
        (None, None) => Err("split requires either --children-json or --children-path".to_owned()),
        (Some(children_json), None) => {
            decode_split_children_inputs(children_json, "parse split children json failed")
        }
        (None, Some(children_path)) => {
            let raw_children = fs::read_to_string(children_path)
                .map_err(|error| format!("read split children path failed: {error}"))?;
            decode_split_children_inputs(
                raw_children.as_str(),
                "parse split children path json failed",
            )
        }
    }
}

fn decode_split_children_inputs(
    raw_children: &str,
    context: &str,
) -> CliResult<Vec<WorkUnitSplitChildInput>> {
    let children = serde_json::from_str::<Vec<WorkUnitSplitChildInput>>(raw_children)
        .map_err(|error| format!("{context}: {error}"))?;
    let child_count = children.len();
    if child_count < WORK_UNIT_SPLIT_MIN_CHILDREN {
        return Err(format!(
            "split children payload must contain at least {WORK_UNIT_SPLIT_MIN_CHILDREN} child items"
        ));
    }
    Ok(children)
}

fn parse_resequence_child_ids(
    ordered_child_ids_json: Option<&str>,
    ordered_child_ids_path: Option<&str>,
) -> CliResult<Vec<String>> {
    match (ordered_child_ids_json, ordered_child_ids_path) {
        (Some(_), Some(_)) => Err(
            "provide either --ordered-child-ids-json or --ordered-child-ids-path, but not both"
                .to_owned(),
        ),
        (None, None) => Err(
            "resequence requires either --ordered-child-ids-json or --ordered-child-ids-path"
                .to_owned(),
        ),
        (Some(ordered_child_ids_json), None) => decode_resequence_child_ids(
            ordered_child_ids_json,
            "parse resequence child ids json failed",
        ),
        (None, Some(ordered_child_ids_path)) => {
            let raw_child_ids = fs::read_to_string(ordered_child_ids_path)
                .map_err(|error| format!("read resequence child ids path failed: {error}"))?;
            decode_resequence_child_ids(
                raw_child_ids.as_str(),
                "parse resequence child ids path json failed",
            )
        }
    }
}

fn decode_resequence_child_ids(raw_child_ids: &str, context: &str) -> CliResult<Vec<String>> {
    let ordered_child_ids = serde_json::from_str::<Vec<String>>(raw_child_ids)
        .map_err(|error| format!("{context}: {error}"))?;
    let child_count = ordered_child_ids.len();
    if child_count < WORK_UNIT_SPLIT_MIN_CHILDREN {
        return Err(format!(
            "resequence child ids payload must contain at least {WORK_UNIT_SPLIT_MIN_CHILDREN} child ids"
        ));
    }
    Ok(ordered_child_ids)
}

fn render_work_unit_snapshot_text(snapshot: &WorkUnitSnapshot) -> String {
    let work_unit = &snapshot.work_unit;
    let lease_text = snapshot
        .lease
        .as_ref()
        .map(render_lease_text)
        .unwrap_or_else(|| "lease: (none)".to_owned());
    let review_text = work_unit
        .review
        .as_ref()
        .map(render_review_text)
        .unwrap_or_else(|| "review: (none)".to_owned());
    let result_payload = work_unit
        .result_payload_json
        .as_ref()
        .and_then(|value| serde_json::to_string(value).ok())
        .unwrap_or_else(|| "-".to_owned());
    let last_error = work_unit.last_error.as_deref().unwrap_or("-");
    let blocking_reason = work_unit.blocking_reason.as_deref().unwrap_or("-");
    let parent = work_unit.parent_work_unit_id.as_deref().unwrap_or("-");
    let plan_position = render_optional_i64(work_unit.plan_position);
    let superseded_by = work_unit
        .superseded_by_work_unit_id
        .as_deref()
        .unwrap_or("-");
    let assigned_to = work_unit.assigned_to.as_deref().unwrap_or("-");
    let children = render_string_list(work_unit.child_work_unit_ids.as_slice());
    let supersedes = render_string_list(work_unit.supersedes_work_unit_ids.as_slice());
    let blocks = render_string_list(work_unit.blocks_work_unit_ids.as_slice());
    let blocked_by = render_string_list(work_unit.blocked_by_work_unit_ids.as_slice());
    let source = render_source_ref(&work_unit.source_ref);
    let retry = render_retry_policy(&work_unit.retry_policy);
    format!(
        "id={} kind={} status={} priority={} attempts={} next_run_at_ms={} archived_at_ms={}\nsource={}\nretry={}\nparent_work_unit_id={}\nplan_position={}\nsuperseded_by_work_unit_id={}\nchild_work_unit_ids={}\nsupersedes_work_unit_ids={}\nassigned_to={}\nblocks_work_unit_ids={}\nblocked_by_work_unit_ids={}\ntitle={}\ndescription={}\nlast_error={}\nblocking_reason={}\nresult_payload_json={}\n{}\n{}\n",
        work_unit.work_unit_id,
        work_unit.kind.as_str(),
        work_unit.status.as_str(),
        work_unit.priority.as_str(),
        work_unit.attempt_count,
        work_unit.next_run_at_ms,
        render_optional_i64(work_unit.archived_at_ms),
        source,
        retry,
        parent,
        plan_position,
        superseded_by,
        children,
        supersedes,
        assigned_to,
        blocks,
        blocked_by,
        work_unit.title,
        work_unit.description,
        last_error,
        blocking_reason,
        result_payload,
        review_text,
        lease_text,
    )
}

fn render_work_unit_split_text(view: &WorkUnitSplitView) -> String {
    let child_work_unit_ids = view
        .children
        .iter()
        .map(|child| child.work_unit.work_unit_id.as_str())
        .collect::<Vec<_>>();
    let child_work_unit_ids = child_work_unit_ids.join(",");
    let child_work_unit_ids = if child_work_unit_ids.is_empty() {
        "-".to_owned()
    } else {
        child_work_unit_ids
    };
    format!(
        "split_parent_id={} child_count={} child_work_unit_ids={}\n{}{}",
        view.parent.work_unit.work_unit_id,
        view.children.len(),
        child_work_unit_ids,
        render_work_unit_snapshot_text(&view.parent),
        render_work_unit_list_text(view.children.as_slice()),
    )
}

fn render_work_unit_supersede_text(view: &WorkUnitSupersedeView) -> String {
    format!(
        "superseded_obsolete_id={} superseded_replacement_id={}\n{}{}",
        view.obsolete.work_unit.work_unit_id,
        view.replacement.work_unit.work_unit_id,
        render_work_unit_snapshot_text(&view.obsolete),
        render_work_unit_snapshot_text(&view.replacement),
    )
}

fn render_work_unit_replan_text(view: &WorkUnitReplanView) -> String {
    format!(
        "replan_parent_id={} child_count={}\n{}{}",
        view.parent.work_unit.work_unit_id,
        view.children.len(),
        render_work_unit_snapshot_text(&view.parent),
        render_work_unit_list_text(view.children.as_slice()),
    )
}

fn render_work_unit_resequence_text(view: &WorkUnitResequenceView) -> String {
    let ordered_child_work_unit_ids = view
        .children
        .iter()
        .map(|child| child.work_unit.work_unit_id.as_str())
        .collect::<Vec<_>>();
    let ordered_child_work_unit_ids = ordered_child_work_unit_ids.join(",");
    format!(
        "resequence_parent_id={} ordered_child_work_unit_ids={}\n{}{}",
        view.parent.work_unit.work_unit_id,
        ordered_child_work_unit_ids,
        render_work_unit_snapshot_text(&view.parent),
        render_work_unit_list_text(view.children.as_slice()),
    )
}

fn render_work_unit_list_text(snapshots: &[WorkUnitSnapshot]) -> String {
    if snapshots.is_empty() {
        return "work_units: (none)\n".to_owned();
    }
    let mut lines = Vec::new();
    lines.push("work_units:".to_owned());
    for snapshot in snapshots {
        let work_unit = &snapshot.work_unit;
        let lease_owner = snapshot
            .lease
            .as_ref()
            .map(|lease| lease.owner.as_str())
            .unwrap_or("-");
        let assigned_to = work_unit.assigned_to.as_deref().unwrap_or("-");
        let blocked_by_count = work_unit.blocked_by_work_unit_ids.len();
        let child_count = work_unit.child_work_unit_ids.len();
        let plan_position = render_optional_i64(work_unit.plan_position);
        let line = format!(
            "- id={} kind={} status={} priority={} attempts={} next_run_at_ms={} plan_position={} lease_owner={} assigned_to={} child_count={} blocked_by_count={}",
            work_unit.work_unit_id,
            work_unit.kind.as_str(),
            work_unit.status.as_str(),
            work_unit.priority.as_str(),
            work_unit.attempt_count,
            work_unit.next_run_at_ms,
            plan_position,
            lease_owner,
            assigned_to,
            child_count,
            blocked_by_count,
        );
        lines.push(line);
    }
    lines.push(String::new());
    lines.join("\n")
}

fn render_work_unit_events_text(events: &[loongclaw_contracts::WorkUnitEventRecord]) -> String {
    if events.is_empty() {
        return "work_unit_events: (none)\n".to_owned();
    }
    let mut lines = Vec::new();
    lines.push("work_unit_events:".to_owned());
    for event in events {
        let payload =
            serde_json::to_string(&event.payload_json).unwrap_or_else(|_| "{}".to_owned());
        let line = format!(
            "- sequence_id={} event_kind={} actor={} recorded_at_ms={} payload={}",
            event.sequence_id,
            event.event_kind,
            event.actor.as_deref().unwrap_or("-"),
            event.recorded_at_ms,
            payload,
        );
        lines.push(line);
    }
    lines.push(String::new());
    lines.join("\n")
}

fn render_single_work_unit_event_text(event: &loongclaw_contracts::WorkUnitEventRecord) -> String {
    render_work_unit_events_text(std::slice::from_ref(event))
}

fn render_work_unit_health_text(health: &loongclaw_contracts::WorkRuntimeHealthSnapshot) -> String {
    format!(
        "total_count={} ready_count={} leased_count={} running_count={} blocked_count={} retry_pending_count={} terminal_count={} archived_count={} expired_lease_count={}\n",
        health.total_count,
        health.ready_count,
        health.leased_count,
        health.running_count,
        health.blocked_count,
        health.retry_pending_count,
        health.terminal_count,
        health.archived_count,
        health.expired_lease_count,
    )
}

fn render_review_text(review: &loongclaw_contracts::WorkUnitReviewRecord) -> String {
    let requested_by = review.requested_by.as_deref().unwrap_or("-");
    let reviewer = review.reviewer.as_deref().unwrap_or("-");
    let decided_at_ms = render_optional_i64(review.decided_at_ms);
    let summary = review.summary.as_deref().unwrap_or("-");
    format!(
        "review: status={} requested_by={} reviewer={} requested_at_ms={} decided_at_ms={} summary={}",
        review.status.as_str(),
        requested_by,
        reviewer,
        review.requested_at_ms,
        decided_at_ms,
        summary,
    )
}

fn render_source_ref(source_ref: &WorkUnitSourceRef) -> String {
    let project_id = source_ref.project_id.as_deref().unwrap_or("-");
    let channel_id = source_ref.channel_id.as_deref().unwrap_or("-");
    let thread_id = source_ref.thread_id.as_deref().unwrap_or("-");
    let message_id = source_ref.message_id.as_deref().unwrap_or("-");
    let external_ref = source_ref.external_ref.as_deref().unwrap_or("-");
    let source_url = source_ref.source_url.as_deref().unwrap_or("-");
    format!(
        "source_kind={} project_id={} channel_id={} thread_id={} message_id={} external_ref={} source_url={}",
        source_ref.source_kind.as_str(),
        project_id,
        channel_id,
        thread_id,
        message_id,
        external_ref,
        source_url,
    )
}

fn render_retry_policy(retry_policy: &WorkUnitRetryPolicy) -> String {
    format!(
        "max_attempts={} initial_backoff_ms={} max_backoff_ms={}",
        retry_policy.max_attempts, retry_policy.initial_backoff_ms, retry_policy.max_backoff_ms,
    )
}

fn render_lease_text(lease: &loongclaw_contracts::WorkUnitLeaseRecord) -> String {
    format!(
        "lease: owner={} lease_version={} acquired_at_ms={} heartbeat_at_ms={} expires_at_ms={}",
        lease.owner,
        lease.lease_version,
        lease.acquired_at_ms,
        lease.heartbeat_at_ms,
        lease.expires_at_ms,
    )
}

fn render_optional_i64(value: Option<i64>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "-".to_owned())
}

fn render_string_list(values: &[String]) -> String {
    if values.is_empty() {
        return "-".to_owned();
    }
    values.join(",")
}
