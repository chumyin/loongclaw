use super::*;
use loongclaw_daemon::work_unit_cli as daemon_work_unit_cli;
use loongclaw_daemon::work_unit_cli as work_unit_runtime;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

fn write_work_unit_config(root: &Path) -> PathBuf {
    fs::create_dir_all(root).expect("create fixture root");

    let sqlite_path = root.join("memory.sqlite3");
    let mut config = mvp::config::LoongClawConfig::default();
    config.memory.sqlite_path = sqlite_path.display().to_string();

    let config_path = root.join("loongclaw.toml");
    mvp::config::write(Some(config_path.to_string_lossy().as_ref()), &config, true)
        .expect("write config fixture");
    config_path
}

fn load_work_unit_repository(config_path: &Path) -> mvp::work::repository::WorkUnitRepository {
    let (_, config) = mvp::config::load(Some(
        config_path.to_str().expect("config path should be utf-8"),
    ))
    .expect("load work-unit config");
    let memory_config =
        mvp::memory::runtime_config::MemoryRuntimeConfig::from_memory_config(&config.memory);
    mvp::work::repository::WorkUnitRepository::new(&memory_config).expect("work unit repository")
}

fn work_unit_environment_guard() -> super::tasks_cli::TasksCliEnvironmentGuard {
    super::tasks_cli::TasksCliEnvironmentGuard::set(&[])
}

fn render_output(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[test]
fn cli_work_unit_help_mentions_durable_runtime_commands() {
    let help = render_cli_help(["work-unit"]);

    assert!(
        help.contains("Create one durable work unit record"),
        "work-unit help should describe the create flow: {help}"
    );
    assert!(
        help.contains("claim"),
        "work-unit help should expose lease claiming: {help}"
    );
    assert!(
        help.contains("recover"),
        "work-unit help should expose lease recovery: {help}"
    );
    assert!(
        help.contains("assign"),
        "work-unit help should expose orchestration assignment: {help}"
    );
    assert!(
        help.contains("update"),
        "work-unit help should expose general work-unit mutation: {help}"
    );
    assert!(
        help.contains("request-review"),
        "work-unit help should expose review orchestration: {help}"
    );
    assert!(
        help.contains("spawn-child"),
        "work-unit help should expose child-work decomposition: {help}"
    );
}

#[test]
fn cli_work_unit_parse_accepts_full_complete_command_shape() {
    let cli = try_parse_cli([
        "loongclaw",
        "work-unit",
        "complete",
        "--config",
        "/tmp/loongclaw.toml",
        "--id",
        "wu-demo",
        "--owner",
        "worker-a",
        "--disposition",
        "retry_pending",
        "--actor",
        "scheduler",
        "--now-ms",
        "1500",
        "--next-run-at-ms",
        "2500",
        "--result-payload-json",
        "{\"summary\":\"retry\"}",
        "--error",
        "transient",
        "--json",
    ])
    .expect("work-unit complete CLI should parse");

    let command = cli.command.expect("CLI should parse a subcommand");
    let Commands::WorkUnit { command } = command else {
        panic!("unexpected CLI parse result: {command:?}");
    };
    let work_unit_runtime::WorkUnitCommands::Complete(options) = command else {
        panic!("unexpected work-unit subcommand parse result: {command:?}");
    };

    assert_eq!(options.id, "wu-demo");
    assert_eq!(options.owner, "worker-a");
    assert_eq!(
        options.disposition,
        work_unit_runtime::WorkUnitDispositionArg::RetryPending
    );
    assert_eq!(options.actor.as_deref(), Some("scheduler"));
    assert_eq!(options.now_ms, Some(1500));
    assert_eq!(options.next_run_at_ms, Some(2500));
    assert!(options.json);
}

#[test]
fn cli_work_unit_parse_accepts_update_command_shape() {
    let cli = try_parse_cli([
        "loongclaw",
        "work-unit",
        "update",
        "--config",
        "/tmp/loongclaw.toml",
        "--id",
        "wu-demo",
        "--title",
        "Refined title",
        "--description",
        "Refined description",
        "--status",
        "waiting_review",
        "--priority",
        "critical",
        "--next-run-at-ms",
        "3333",
        "--blocking-reason",
        "awaiting review",
        "--actor",
        "planner",
        "--now-ms",
        "2222",
        "--json",
    ])
    .expect("work-unit update CLI should parse");

    let command = cli.command.expect("CLI should parse a subcommand");
    let Commands::WorkUnit { command } = command else {
        panic!("unexpected CLI parse result: {command:?}");
    };
    let work_unit_runtime::WorkUnitCommands::Update(options) = command else {
        panic!("unexpected work-unit update parse result: {command:?}");
    };

    assert_eq!(options.id, "wu-demo");
    assert_eq!(options.title.as_deref(), Some("Refined title"));
    assert_eq!(options.description.as_deref(), Some("Refined description"));
    assert_eq!(
        options.status,
        Some(work_unit_runtime::WorkUnitStatusArg::WaitingReview)
    );
    assert_eq!(
        options.priority,
        Some(work_unit_runtime::WorkUnitPriorityArg::Critical)
    );
    assert_eq!(options.next_run_at_ms, Some(3333));
    assert_eq!(options.blocking_reason.as_deref(), Some("awaiting review"));
    assert_eq!(options.actor.as_deref(), Some("planner"));
    assert_eq!(options.now_ms, Some(2222));
    assert!(options.json);
}

#[test]
fn cli_work_unit_parse_accepts_review_command_shapes() {
    let request_cli = try_parse_cli([
        "loongclaw",
        "work-unit",
        "request-review",
        "--config",
        "/tmp/loongclaw.toml",
        "--id",
        "wu-demo",
        "--requested-by",
        "planner",
        "--reviewer",
        "reviewer-a",
        "--summary",
        "please review",
        "--now-ms",
        "2100",
        "--json",
    ])
    .expect("work-unit request-review CLI should parse");
    let request_command = request_cli.command.expect("request-review command");
    let Commands::WorkUnit {
        command: work_unit_runtime::WorkUnitCommands::RequestReview(options),
    } = request_command
    else {
        panic!("unexpected request-review parse result: {request_command:?}");
    };
    assert_eq!(options.id, "wu-demo");
    assert_eq!(options.requested_by.as_deref(), Some("planner"));
    assert_eq!(options.reviewer.as_deref(), Some("reviewer-a"));
    assert_eq!(options.summary.as_deref(), Some("please review"));
    assert_eq!(options.now_ms, Some(2100));
    assert!(options.json);

    let decision_cli = try_parse_cli([
        "loongclaw",
        "work-unit",
        "review",
        "--config",
        "/tmp/loongclaw.toml",
        "--id",
        "wu-demo",
        "--decision",
        "request-changes",
        "--reviewer",
        "reviewer-a",
        "--summary",
        "tighten the plan",
        "--now-ms",
        "2200",
    ])
    .expect("work-unit review CLI should parse");
    let decision_command = decision_cli.command.expect("review command");
    let Commands::WorkUnit {
        command: work_unit_runtime::WorkUnitCommands::Review(options),
    } = decision_command
    else {
        panic!("unexpected review parse result: {decision_command:?}");
    };
    assert_eq!(options.id, "wu-demo");
    assert_eq!(
        options.decision,
        work_unit_runtime::WorkUnitReviewDecisionArg::RequestChanges
    );
    assert_eq!(options.reviewer.as_deref(), Some("reviewer-a"));
    assert_eq!(options.summary.as_deref(), Some("tighten the plan"));
    assert_eq!(options.now_ms, Some(2200));
}

#[test]
fn cli_work_unit_parse_accepts_spawn_child_shape() {
    let cli = try_parse_cli([
        "loongclaw",
        "work-unit",
        "spawn-child",
        "--config",
        "/tmp/loongclaw.toml",
        "--parent-id",
        "wu-parent",
        "--id",
        "wu-child",
        "--kind",
        "issue",
        "--title",
        "Child work",
        "--description",
        "Follow-up item",
        "--block-parent",
        "false",
        "--json",
    ])
    .expect("work-unit spawn-child CLI should parse");

    let command = cli.command.expect("CLI should parse a subcommand");
    let Commands::WorkUnit { command } = command else {
        panic!("unexpected CLI parse result: {command:?}");
    };
    let work_unit_runtime::WorkUnitCommands::SpawnChild(options) = command else {
        panic!("unexpected spawn-child parse result: {command:?}");
    };

    assert_eq!(options.parent_id, "wu-parent");
    assert_eq!(options.id.as_deref(), Some("wu-child"));
    assert_eq!(options.kind, work_unit_runtime::WorkUnitKindArg::Issue);
    assert_eq!(options.title, "Child work");
    assert!(!options.block_parent);
    assert!(options.json);
}

#[test]
fn work_unit_cli_create_claim_complete_and_archive_round_trip() {
    let _env = work_unit_environment_guard();
    let root = unique_temp_dir("loongclaw-work-unit-cli");
    let config_path = write_work_unit_config(&root);
    let config_path_string = config_path.display().to_string();

    work_unit_runtime::run_work_unit_cli(work_unit_runtime::WorkUnitCommands::Create(
        work_unit_runtime::WorkUnitCreateCommandOptions {
            config: Some(config_path_string.clone()),
            id: Some("wu-blocker".to_owned()),
            kind: work_unit_runtime::WorkUnitKindArg::Ops,
            title: "Prerequisite".to_owned(),
            description: "Finish prerequisite work".to_owned(),
            status: work_unit_runtime::WorkUnitStatusArg::Ready,
            priority: work_unit_runtime::WorkUnitPriorityArg::Low,
            max_attempts: 1,
            initial_backoff_ms: 1_000,
            max_backoff_ms: 1_000,
            next_run_at_ms: Some(1_000),
            actor: Some("operator".to_owned()),
            source_kind: work_unit_runtime::WorkSourceKindArg::Manual,
            project_id: None,
            channel_id: None,
            thread_id: None,
            message_id: None,
            external_ref: None,
            source_url: None,
            parent_work_unit_id: None,
            json: true,
        },
    ))
    .expect("create blocker work unit via CLI");

    work_unit_runtime::run_work_unit_cli(work_unit_runtime::WorkUnitCommands::Create(
        work_unit_runtime::WorkUnitCreateCommandOptions {
            config: Some(config_path_string.clone()),
            id: Some("wu-cli".to_owned()),
            kind: work_unit_runtime::WorkUnitKindArg::Feature,
            title: "Durable runtime slice".to_owned(),
            description: "Create the first work-unit runtime slice".to_owned(),
            status: work_unit_runtime::WorkUnitStatusArg::Ready,
            priority: work_unit_runtime::WorkUnitPriorityArg::High,
            max_attempts: 3,
            initial_backoff_ms: 1_000,
            max_backoff_ms: 8_000,
            next_run_at_ms: Some(1_000),
            actor: Some("operator".to_owned()),
            source_kind: work_unit_runtime::WorkSourceKindArg::Discord,
            project_id: Some("loongclaw-ai/server".to_owned()),
            channel_id: Some("feature".to_owned()),
            thread_id: Some("thread-1".to_owned()),
            message_id: Some("message-1".to_owned()),
            external_ref: Some("feature-thread".to_owned()),
            source_url: None,
            parent_work_unit_id: None,
            json: true,
        },
    ))
    .expect("create work unit via CLI");

    work_unit_runtime::run_work_unit_cli(work_unit_runtime::WorkUnitCommands::Assign(
        work_unit_runtime::WorkUnitAssignCommandOptions {
            config: Some(config_path_string.clone()),
            id: "wu-cli".to_owned(),
            assigned_to: Some("designer".to_owned()),
            actor: Some("operator".to_owned()),
            now_ms: Some(1_050),
            json: true,
        },
    ))
    .expect("assign work unit via CLI");

    work_unit_runtime::run_work_unit_cli(work_unit_runtime::WorkUnitCommands::Update(
        work_unit_runtime::WorkUnitUpdateCommandOptions {
            config: Some(config_path_string.clone()),
            id: "wu-cli".to_owned(),
            title: Some("Durable runtime slice v2".to_owned()),
            description: Some("Refine the orchestration-ready slice".to_owned()),
            status: None,
            priority: Some(work_unit_runtime::WorkUnitPriorityArg::Critical),
            next_run_at_ms: Some(1_060),
            blocking_reason: None,
            clear_blocking_reason: false,
            actor: Some("planner".to_owned()),
            now_ms: Some(1_055),
            json: true,
        },
    ))
    .expect("update work unit via CLI");

    work_unit_runtime::run_work_unit_cli(work_unit_runtime::WorkUnitCommands::RequestReview(
        work_unit_runtime::WorkUnitRequestReviewCommandOptions {
            config: Some(config_path_string.clone()),
            id: "wu-cli".to_owned(),
            requested_by: Some("planner".to_owned()),
            reviewer: Some("reviewer-a".to_owned()),
            summary: Some("review the revised slice".to_owned()),
            now_ms: Some(1_057),
            json: true,
        },
    ))
    .expect("request review via CLI");

    work_unit_runtime::run_work_unit_cli(work_unit_runtime::WorkUnitCommands::Depend(
        work_unit_runtime::WorkUnitDependCommandOptions {
            config: Some(config_path_string.clone()),
            blocking_id: "wu-blocker".to_owned(),
            blocked_id: "wu-cli".to_owned(),
            actor: Some("operator".to_owned()),
            now_ms: Some(1_075),
            json: true,
        },
    ))
    .expect("add dependency via CLI");

    work_unit_runtime::run_work_unit_cli(work_unit_runtime::WorkUnitCommands::Note(
        work_unit_runtime::WorkUnitNoteCommandOptions {
            config: Some(config_path_string.clone()),
            id: "wu-cli".to_owned(),
            actor: Some("operator".to_owned()),
            note: "waiting on prerequisite".to_owned(),
            now_ms: Some(1_090),
            json: true,
        },
    ))
    .expect("append note via CLI");

    work_unit_runtime::run_work_unit_cli(work_unit_runtime::WorkUnitCommands::Claim(
        work_unit_runtime::WorkUnitClaimCommandOptions {
            config: Some(config_path_string.clone()),
            owner: "worker-a".to_owned(),
            ttl_ms: 5_000,
            actor: Some("scheduler".to_owned()),
            now_ms: Some(1_000),
            json: true,
        },
    ))
    .expect("claim work unit via CLI");

    let repository = load_work_unit_repository(&config_path);
    let updated_snapshot = repository
        .load_work_unit_snapshot("wu-cli")
        .expect("load updated snapshot")
        .expect("updated snapshot");
    assert_eq!(updated_snapshot.work_unit.title, "Durable runtime slice v2");
    assert_eq!(
        updated_snapshot.work_unit.status,
        loongclaw_contracts::WorkUnitStatus::WaitingReview
    );
    assert_eq!(
        updated_snapshot.work_unit.blocking_reason.as_deref(),
        Some("review the revised slice")
    );
    let review = updated_snapshot.work_unit.review.expect("pending review");
    assert_eq!(
        review.status,
        loongclaw_contracts::WorkUnitReviewStatus::Pending
    );
    assert_eq!(review.requested_by.as_deref(), Some("planner"));
    assert_eq!(review.reviewer.as_deref(), Some("reviewer-a"));

    let blocker_snapshot = repository
        .load_work_unit_snapshot("wu-blocker")
        .expect("load blocker snapshot")
        .expect("blocker snapshot");
    assert_eq!(
        blocker_snapshot
            .lease
            .as_ref()
            .map(|lease| lease.owner.as_str()),
        Some("worker-a")
    );

    work_unit_runtime::run_work_unit_cli(work_unit_runtime::WorkUnitCommands::Complete(
        work_unit_runtime::WorkUnitCompleteCommandOptions {
            config: Some(config_path_string.clone()),
            id: "wu-blocker".to_owned(),
            owner: "worker-a".to_owned(),
            disposition: work_unit_runtime::WorkUnitDispositionArg::Completed,
            actor: Some("worker-a".to_owned()),
            now_ms: Some(1_050),
            next_run_at_ms: None,
            result_payload_json: None,
            error: None,
            json: true,
        },
    ))
    .expect("complete blocker work unit via CLI");

    work_unit_runtime::run_work_unit_cli(work_unit_runtime::WorkUnitCommands::Undepend(
        work_unit_runtime::WorkUnitUndependCommandOptions {
            config: Some(config_path_string.clone()),
            blocking_id: "wu-blocker".to_owned(),
            blocked_id: "wu-cli".to_owned(),
            actor: Some("operator".to_owned()),
            now_ms: Some(1_100),
            json: true,
        },
    ))
    .expect("remove dependency via CLI");

    work_unit_runtime::run_work_unit_cli(work_unit_runtime::WorkUnitCommands::Review(
        work_unit_runtime::WorkUnitReviewDecisionCommandOptions {
            config: Some(config_path_string.clone()),
            id: "wu-cli".to_owned(),
            decision: work_unit_runtime::WorkUnitReviewDecisionArg::Approve,
            reviewer: Some("reviewer-a".to_owned()),
            summary: Some("approved".to_owned()),
            now_ms: Some(1_095),
            json: true,
        },
    ))
    .expect("approve review via CLI");

    work_unit_runtime::run_work_unit_cli(work_unit_runtime::WorkUnitCommands::Claim(
        work_unit_runtime::WorkUnitClaimCommandOptions {
            config: Some(config_path_string.clone()),
            owner: "worker-a".to_owned(),
            ttl_ms: 5_000,
            actor: Some("scheduler".to_owned()),
            now_ms: Some(1_100),
            json: true,
        },
    ))
    .expect("claim unblocked work unit via CLI");

    work_unit_runtime::run_work_unit_cli(work_unit_runtime::WorkUnitCommands::Start(
        work_unit_runtime::WorkUnitStartCommandOptions {
            config: Some(config_path_string.clone()),
            id: "wu-cli".to_owned(),
            owner: "worker-a".to_owned(),
            actor: Some("worker-a".to_owned()),
            now_ms: Some(1_100),
            json: true,
        },
    ))
    .expect("start work unit via CLI");

    work_unit_runtime::run_work_unit_cli(work_unit_runtime::WorkUnitCommands::Complete(
        work_unit_runtime::WorkUnitCompleteCommandOptions {
            config: Some(config_path_string.clone()),
            id: "wu-cli".to_owned(),
            owner: "worker-a".to_owned(),
            disposition: work_unit_runtime::WorkUnitDispositionArg::Completed,
            actor: Some("worker-a".to_owned()),
            now_ms: Some(1_200),
            next_run_at_ms: None,
            result_payload_json: Some("{\"summary\":\"done\"}".to_owned()),
            error: None,
            json: true,
        },
    ))
    .expect("complete work unit via CLI");

    work_unit_runtime::run_work_unit_cli(work_unit_runtime::WorkUnitCommands::Archive(
        work_unit_runtime::WorkUnitArchiveCommandOptions {
            config: Some(config_path_string),
            id: "wu-cli".to_owned(),
            actor: Some("operator".to_owned()),
            now_ms: Some(1_300),
            json: true,
        },
    ))
    .expect("archive work unit via CLI");

    let snapshot = repository
        .load_work_unit_snapshot("wu-cli")
        .expect("load work unit snapshot")
        .expect("work unit snapshot");
    let events = repository
        .list_work_unit_events("wu-cli", 20)
        .expect("load work unit events");

    assert_eq!(
        snapshot.work_unit.status,
        loongclaw_contracts::WorkUnitStatus::Archived
    );
    assert_eq!(
        snapshot.work_unit.result_payload_json,
        Some(json!({"summary": "done"}))
    );
    assert_eq!(snapshot.work_unit.assigned_to.as_deref(), Some("designer"));
    assert!(snapshot.work_unit.blocked_by_work_unit_ids.is_empty());
    assert!(snapshot.lease.is_none());
    assert!(
        events
            .iter()
            .any(|event| event.event_kind == "work_unit_created"),
        "expected create event in work-unit ledger"
    );
    assert!(
        events
            .iter()
            .any(|event| event.event_kind == "work_unit_updated"),
        "expected update event in work-unit ledger"
    );
    assert!(
        events
            .iter()
            .any(|event| event.event_kind == "work_unit_review_requested"),
        "expected review request event in work-unit ledger"
    );
    assert!(
        events
            .iter()
            .any(|event| event.event_kind == "work_unit_review_recorded"),
        "expected review decision event in work-unit ledger"
    );
    assert!(
        events
            .iter()
            .any(|event| event.event_kind == "work_unit_archived"),
        "expected archive event in work-unit ledger"
    );
    assert!(
        events
            .iter()
            .any(|event| event.event_kind == "work_unit_assigned"),
        "expected assignment event in work-unit ledger"
    );
    assert!(
        events
            .iter()
            .any(|event| event.event_kind == "work_unit_note_added"),
        "expected note event in work-unit ledger"
    );
}

#[test]
fn work_unit_cli_spawn_child_blocks_parent_until_child_completes() {
    let _env = work_unit_environment_guard();
    let root = unique_temp_dir("loongclaw-work-unit-child-cli");
    let config_path = write_work_unit_config(&root);
    let config_path_string = config_path.display().to_string();

    work_unit_runtime::run_work_unit_cli(work_unit_runtime::WorkUnitCommands::Create(
        work_unit_runtime::WorkUnitCreateCommandOptions {
            config: Some(config_path_string.clone()),
            id: Some("wu-parent".to_owned()),
            kind: work_unit_runtime::WorkUnitKindArg::Feature,
            title: "Parent work".to_owned(),
            description: "Coordinate child items".to_owned(),
            status: work_unit_runtime::WorkUnitStatusArg::Ready,
            priority: work_unit_runtime::WorkUnitPriorityArg::High,
            max_attempts: 3,
            initial_backoff_ms: 1_000,
            max_backoff_ms: 8_000,
            next_run_at_ms: Some(1_000),
            actor: Some("operator".to_owned()),
            source_kind: work_unit_runtime::WorkSourceKindArg::Discord,
            project_id: Some("loongclaw-ai/server".to_owned()),
            channel_id: Some("feature".to_owned()),
            thread_id: Some("thread-parent".to_owned()),
            message_id: Some("message-parent".to_owned()),
            external_ref: Some("parent-thread".to_owned()),
            source_url: None,
            parent_work_unit_id: None,
            json: true,
        },
    ))
    .expect("create parent work unit via CLI");

    work_unit_runtime::run_work_unit_cli(work_unit_runtime::WorkUnitCommands::SpawnChild(
        work_unit_runtime::WorkUnitSpawnChildCommandOptions {
            config: Some(config_path_string.clone()),
            parent_id: "wu-parent".to_owned(),
            id: Some("wu-child".to_owned()),
            kind: work_unit_runtime::WorkUnitKindArg::Issue,
            title: "Child work".to_owned(),
            description: "Complete child task".to_owned(),
            status: work_unit_runtime::WorkUnitStatusArg::Ready,
            priority: None,
            max_attempts: None,
            initial_backoff_ms: None,
            max_backoff_ms: None,
            next_run_at_ms: Some(1_010),
            actor: Some("planner".to_owned()),
            block_parent: true,
            source_kind: None,
            project_id: None,
            channel_id: None,
            thread_id: None,
            message_id: None,
            external_ref: None,
            source_url: None,
            json: true,
        },
    ))
    .expect("spawn child work unit via CLI");

    let repository = load_work_unit_repository(&config_path);
    let parent_snapshot = repository
        .load_work_unit_snapshot("wu-parent")
        .expect("load parent snapshot")
        .expect("parent snapshot");
    assert_eq!(
        parent_snapshot.work_unit.child_work_unit_ids,
        vec!["wu-child".to_owned()]
    );
    assert_eq!(
        parent_snapshot.work_unit.blocked_by_work_unit_ids,
        vec!["wu-child".to_owned()]
    );

    work_unit_runtime::run_work_unit_cli(work_unit_runtime::WorkUnitCommands::Claim(
        work_unit_runtime::WorkUnitClaimCommandOptions {
            config: Some(config_path_string.clone()),
            owner: "worker-a".to_owned(),
            ttl_ms: 5_000,
            actor: Some("scheduler".to_owned()),
            now_ms: Some(1_020),
            json: true,
        },
    ))
    .expect("claim next ready work via CLI");

    let child_snapshot = repository
        .load_work_unit_snapshot("wu-child")
        .expect("load child snapshot")
        .expect("child snapshot");
    assert_eq!(
        child_snapshot
            .lease
            .as_ref()
            .map(|lease| lease.owner.as_str()),
        Some("worker-a")
    );

    work_unit_runtime::run_work_unit_cli(work_unit_runtime::WorkUnitCommands::Complete(
        work_unit_runtime::WorkUnitCompleteCommandOptions {
            config: Some(config_path_string.clone()),
            id: "wu-child".to_owned(),
            owner: "worker-a".to_owned(),
            disposition: work_unit_runtime::WorkUnitDispositionArg::Completed,
            actor: Some("worker-a".to_owned()),
            now_ms: Some(1_030),
            next_run_at_ms: None,
            result_payload_json: None,
            error: None,
            json: true,
        },
    ))
    .expect("complete child work unit via CLI");

    work_unit_runtime::run_work_unit_cli(work_unit_runtime::WorkUnitCommands::Claim(
        work_unit_runtime::WorkUnitClaimCommandOptions {
            config: Some(config_path_string),
            owner: "worker-b".to_owned(),
            ttl_ms: 5_000,
            actor: Some("scheduler".to_owned()),
            now_ms: Some(1_040),
            json: true,
        },
    ))
    .expect("claim parent after child completion");

    let ready_parent = repository
        .load_work_unit_snapshot("wu-parent")
        .expect("reload parent snapshot")
        .expect("ready parent snapshot");
    assert_eq!(
        ready_parent
            .lease
            .as_ref()
            .map(|lease| lease.owner.as_str()),
        Some("worker-b")
    );
}

#[test]
fn work_unit_cli_spawn_child_preserves_explicit_default_overrides() {
    let _env = work_unit_environment_guard();
    let root = unique_temp_dir("loongclaw-work-unit-child-cli-explicit-overrides");
    let config_path = write_work_unit_config(&root);
    let config_path_string = config_path.display().to_string();

    work_unit_runtime::run_work_unit_cli(work_unit_runtime::WorkUnitCommands::Create(
        work_unit_runtime::WorkUnitCreateCommandOptions {
            config: Some(config_path_string.clone()),
            id: Some("wu-parent".to_owned()),
            kind: work_unit_runtime::WorkUnitKindArg::Feature,
            title: "Parent work".to_owned(),
            description: "Coordinate child items".to_owned(),
            status: work_unit_runtime::WorkUnitStatusArg::Ready,
            priority: work_unit_runtime::WorkUnitPriorityArg::High,
            max_attempts: 7,
            initial_backoff_ms: 2_000,
            max_backoff_ms: 8_000,
            next_run_at_ms: Some(1_000),
            actor: Some("operator".to_owned()),
            source_kind: work_unit_runtime::WorkSourceKindArg::Discord,
            project_id: Some("loongclaw-ai/server".to_owned()),
            channel_id: Some("feature".to_owned()),
            thread_id: Some("thread-parent".to_owned()),
            message_id: Some("message-parent".to_owned()),
            external_ref: Some("parent-thread".to_owned()),
            source_url: None,
            parent_work_unit_id: None,
            json: true,
        },
    ))
    .expect("create parent work unit via CLI");

    work_unit_runtime::run_work_unit_cli(work_unit_runtime::WorkUnitCommands::SpawnChild(
        work_unit_runtime::WorkUnitSpawnChildCommandOptions {
            config: Some(config_path_string),
            parent_id: "wu-parent".to_owned(),
            id: Some("wu-child-manual".to_owned()),
            kind: work_unit_runtime::WorkUnitKindArg::Issue,
            title: "Child work".to_owned(),
            description: "Stay manual and normal".to_owned(),
            status: work_unit_runtime::WorkUnitStatusArg::Ready,
            priority: Some(work_unit_runtime::WorkUnitPriorityArg::Normal),
            max_attempts: Some(3),
            initial_backoff_ms: Some(1_000),
            max_backoff_ms: Some(60_000),
            next_run_at_ms: Some(1_010),
            actor: Some("planner".to_owned()),
            block_parent: false,
            source_kind: Some(work_unit_runtime::WorkSourceKindArg::Manual),
            project_id: None,
            channel_id: None,
            thread_id: None,
            message_id: None,
            external_ref: None,
            source_url: None,
            json: true,
        },
    ))
    .expect("spawn child work unit via CLI");

    let repository = load_work_unit_repository(&config_path);
    let child_snapshot = repository
        .load_work_unit_snapshot("wu-child-manual")
        .expect("load child snapshot")
        .expect("child snapshot");

    assert_eq!(
        child_snapshot.work_unit.priority,
        loongclaw_contracts::WorkUnitPriority::Normal
    );
    assert_eq!(
        child_snapshot.work_unit.retry_policy,
        loongclaw_contracts::WorkUnitRetryPolicy::default()
    );
    assert_eq!(
        child_snapshot.work_unit.source_ref.source_kind,
        loongclaw_contracts::WorkSourceKind::Manual
    );
    assert!(child_snapshot.work_unit.blocks_work_unit_ids.is_empty());
}

#[test]
fn work_unit_cli_update_text_output_uses_snake_case_status_labels() {
    let _env = work_unit_environment_guard();
    let root = unique_temp_dir("loongclaw-work-unit-cli-text");
    let config_path = write_work_unit_config(&root);
    let repository = load_work_unit_repository(&config_path);
    let retry_policy = loongclaw_contracts::WorkUnitRetryPolicy {
        max_attempts: 2,
        initial_backoff_ms: 1_000,
        max_backoff_ms: 8_000,
    };
    let source_ref = loongclaw_contracts::WorkUnitSourceRef {
        source_kind: loongclaw_contracts::WorkSourceKind::Manual,
        project_id: None,
        channel_id: None,
        thread_id: None,
        message_id: None,
        external_ref: None,
        source_url: None,
    };
    let new_work_unit = mvp::work::repository::NewWorkUnitRecord {
        work_unit_id: Some("wu-text".to_owned()),
        kind: loongclaw_contracts::WorkUnitKind::Feature,
        title: "text renderer".to_owned(),
        description: "verify non-json output".to_owned(),
        source_ref,
        status: loongclaw_contracts::WorkUnitStatus::Ready,
        priority: loongclaw_contracts::WorkUnitPriority::Normal,
        retry_policy,
        parent_work_unit_id: None,
        next_run_at_ms: Some(1_000),
    };
    repository
        .create_work_unit(new_work_unit, Some("operator"))
        .expect("create work unit fixture");

    let config_path_string = config_path.display().to_string();
    let output = Command::new(env!("CARGO_BIN_EXE_loongclaw"))
        .args([
            "work-unit",
            "update",
            "--config",
            config_path_string.as_str(),
            "--id",
            "wu-text",
            "--status",
            "waiting_review",
            "--actor",
            "planner",
            "--now-ms",
            "1234",
        ])
        .output()
        .expect("run work-unit update text command");

    let stdout = render_output(&output.stdout);
    let stderr = render_output(&output.stderr);

    assert!(
        output.status.success(),
        "work-unit update text output should succeed, stdout={stdout:?}, stderr={stderr:?}"
    );
    assert!(
        stdout.contains("status=waiting_review"),
        "text output should preserve snake_case status labels, stdout={stdout:?}"
    );
}
