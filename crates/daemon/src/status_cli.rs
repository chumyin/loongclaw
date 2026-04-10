use loongclaw_contracts::WorkRuntimeHealthSnapshot;
use loongclaw_spec::CliResult;
use serde::Serialize;

use crate::gateway::read_models::{
    GatewayAcpObservabilityReadModel, GatewayOperatorSummaryReadModel,
    build_acp_observability_read_model, build_operator_summary_read_model,
    build_runtime_snapshot_read_model,
};
use crate::gateway::service::default_gateway_owner_status;
use crate::gateway::state::{default_gateway_runtime_state_dir, load_gateway_owner_status};
use crate::mvp;
use crate::operator_runtime_diagnostics::RuntimeOperatorDiagnosticsState;
use crate::supervisor::LoadedSupervisorConfig;

const STATUS_CLI_JSON_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize)]
pub struct StatusCliJsonSchema {
    pub version: u32,
    pub surface: &'static str,
    pub purpose: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct StatusCliAcpReadModel {
    pub enabled: bool,
    pub availability: String,
    pub error: Option<String>,
    pub persisted_session_count: Option<usize>,
    pub observability: Option<GatewayAcpObservabilityReadModel>,
}

#[derive(Debug, Clone, Serialize)]
pub struct StatusCliWorkUnitReadModel {
    pub availability: String,
    pub error: Option<String>,
    pub health: Option<WorkRuntimeHealthSnapshot>,
}

#[derive(Debug, Clone, Serialize)]
pub struct StatusCliReadModel {
    pub config: String,
    pub schema: StatusCliJsonSchema,
    pub gateway: GatewayOperatorSummaryReadModel,
    pub runtime_diagnostics: RuntimeOperatorDiagnosticsState,
    pub acp: StatusCliAcpReadModel,
    pub work_units: StatusCliWorkUnitReadModel,
    pub recipes: Vec<String>,
}

pub async fn run_status_cli(config_path: Option<&str>, as_json: bool) -> CliResult<()> {
    let status = collect_status_cli_read_model(config_path).await?;

    if as_json {
        let pretty_result = serde_json::to_string_pretty(&status);
        let pretty =
            pretty_result.map_err(|error| format!("serialize status output failed: {error}"))?;
        println!("{pretty}");
        return Ok(());
    }

    let rendered = render_status_cli_text(&status);
    println!("{rendered}");
    Ok(())
}

pub async fn collect_status_cli_read_model(
    config_path: Option<&str>,
) -> CliResult<StatusCliReadModel> {
    let load_result = mvp::config::load(config_path);
    let (resolved_path, config) = load_result?;
    let resolved_path_ref = resolved_path.as_path();
    mvp::runtime_env::initialize_runtime_environment(&config, Some(resolved_path_ref));

    let loaded_config = LoadedSupervisorConfig {
        resolved_path: resolved_path.clone(),
        config: config.clone(),
    };
    let snapshot_result =
        crate::collect_runtime_snapshot_cli_state_from_loaded_config(&loaded_config);
    let snapshot = snapshot_result?;
    let config_path_display = resolved_path.display().to_string();
    let config_path_text = config_path_display.as_str();
    let channel_inventory =
        crate::build_channels_cli_json_payload(config_path_text, &snapshot.channels);
    let runtime_snapshot = build_runtime_snapshot_read_model(&snapshot);
    let runtime_dir = default_gateway_runtime_state_dir();
    let owner_status_option = load_gateway_owner_status(runtime_dir.as_path());
    let owner_status = match owner_status_option {
        Some(owner_status) => owner_status,
        None => default_gateway_owner_status(runtime_dir.as_path()),
    };
    let gateway =
        build_operator_summary_read_model(&owner_status, &channel_inventory, &runtime_snapshot);
    let runtime_diagnostics = snapshot.runtime_diagnostics.clone();
    let acp = collect_status_cli_acp_read_model(config_path_text, &config).await;
    let work_units = collect_status_cli_work_unit_read_model(&config);
    let recipes = build_status_cli_recipes(config_path_text);
    let schema = StatusCliJsonSchema {
        version: STATUS_CLI_JSON_SCHEMA_VERSION,
        surface: "status",
        purpose: "operator_runtime_summary",
    };

    Ok(StatusCliReadModel {
        config: config_path_display,
        schema,
        gateway,
        runtime_diagnostics,
        acp,
        work_units,
        recipes,
    })
}

async fn collect_status_cli_acp_read_model(
    config_path: &str,
    config: &mvp::config::LoongClawConfig,
) -> StatusCliAcpReadModel {
    let enabled = config.acp.enabled;
    let persisted_session_count = load_persisted_acp_session_count(config);

    if !enabled {
        return StatusCliAcpReadModel {
            enabled,
            availability: "disabled".to_owned(),
            error: None,
            persisted_session_count,
            observability: None,
        };
    }

    let manager_result = mvp::acp::shared_acp_session_manager(config);
    let manager = match manager_result {
        Ok(manager) => manager,
        Err(error) => {
            return build_unavailable_acp_read_model(enabled, error, persisted_session_count);
        }
    };

    let snapshot_result = manager.observability_snapshot(config).await;
    let snapshot = match snapshot_result {
        Ok(snapshot) => snapshot,
        Err(error) => {
            return build_unavailable_acp_read_model(enabled, error, persisted_session_count);
        }
    };

    let observability = build_acp_observability_read_model(config_path, &snapshot);

    StatusCliAcpReadModel {
        enabled,
        availability: "available".to_owned(),
        error: None,
        persisted_session_count,
        observability: Some(observability),
    }
}

fn build_unavailable_acp_read_model(
    enabled: bool,
    error: String,
    persisted_session_count: Option<usize>,
) -> StatusCliAcpReadModel {
    StatusCliAcpReadModel {
        enabled,
        availability: "unavailable".to_owned(),
        error: Some(error),
        persisted_session_count,
        observability: None,
    }
}

fn collect_status_cli_work_unit_read_model(
    config: &mvp::config::LoongClawConfig,
) -> StatusCliWorkUnitReadModel {
    #[cfg(not(feature = "memory-sqlite"))]
    {
        let _ = config;
        StatusCliWorkUnitReadModel {
            availability: "unavailable".to_owned(),
            error: Some("work unit runtime requires feature `memory-sqlite`".to_owned()),
            health: None,
        }
    }

    #[cfg(feature = "memory-sqlite")]
    {
        let memory_config =
            mvp::memory::runtime_config::MemoryRuntimeConfig::from_memory_config(&config.memory);
        let repository_result = mvp::work::repository::WorkUnitRepository::new(&memory_config);
        let repository = match repository_result {
            Ok(repository) => repository,
            Err(error) => {
                return StatusCliWorkUnitReadModel {
                    availability: "unavailable".to_owned(),
                    error: Some(error),
                    health: None,
                };
            }
        };

        let health_result = repository.load_runtime_health(None);
        let health = match health_result {
            Ok(health) => health,
            Err(error) => {
                return StatusCliWorkUnitReadModel {
                    availability: "unavailable".to_owned(),
                    error: Some(error),
                    health: None,
                };
            }
        };

        StatusCliWorkUnitReadModel {
            availability: "available".to_owned(),
            error: None,
            health: Some(health),
        }
    }
}

fn load_persisted_acp_session_count(config: &mvp::config::LoongClawConfig) -> Option<usize> {
    #[cfg(not(any(feature = "memory-sqlite", feature = "mvp")))]
    {
        let _ = config;
        None
    }

    #[cfg(any(feature = "memory-sqlite", feature = "mvp"))]
    {
        let sqlite_path = config.memory.resolved_sqlite_path();
        let store = mvp::acp::AcpSqliteSessionStore::new(Some(sqlite_path));
        let sessions_result = mvp::acp::AcpSessionStore::list(&store);
        let sessions = match sessions_result {
            Ok(sessions) => sessions,
            Err(_) => {
                return None;
            }
        };
        Some(sessions.len())
    }
}

fn build_status_cli_recipes(config_path: &str) -> Vec<String> {
    let command_name = crate::active_cli_command_name();
    let config_arg = crate::cli_handoff::shell_quote_argument(config_path);
    let doctor_recipe = format!("{command_name} doctor --config {config_arg} --json");
    let audit_verify_recipe = format!("{command_name} audit verify --config {config_arg}");
    let gateway_recipe = format!("{command_name} gateway status");
    let channels_recipe = format!("{command_name} channels --config {config_arg} --json");
    let acp_observability_recipe =
        format!("{command_name} acp-observability --config {config_arg} --json");
    let acp_sessions_recipe =
        format!("{command_name} list-acp-sessions --config {config_arg} --json");
    let work_units_recipe = format!("{command_name} work-unit health --config {config_arg} --json");

    vec![
        doctor_recipe,
        audit_verify_recipe,
        gateway_recipe,
        channels_recipe,
        acp_observability_recipe,
        acp_sessions_recipe,
        work_units_recipe,
    ]
}

fn render_status_cli_text(status: &StatusCliReadModel) -> String {
    let gateway = &status.gateway;
    let owner = &gateway.owner;
    let control_surface = &gateway.control_surface;
    let channels = &gateway.channels;
    let runtime = &gateway.runtime;
    let base_url_option = control_surface.base_url.as_deref();
    let base_url = base_url_option.unwrap_or("-");
    let owner_pid = render_optional_u32(owner.pid);
    let owner_session_option = owner.attached_cli_session.as_deref();
    let owner_session = owner_session_option.unwrap_or("-");
    let owner_error_option = owner.last_error.as_deref();
    let owner_error = owner_error_option.unwrap_or("-");
    let owner_shutdown_reason_option = owner.shutdown_reason.as_deref();
    let owner_shutdown_reason = owner_shutdown_reason_option.unwrap_or("-");
    let active_provider_profile_id_option = runtime.active_provider_profile_id.as_deref();
    let active_provider_profile_id = active_provider_profile_id_option.unwrap_or("-");
    let active_provider_label_option = runtime.active_provider_label.as_deref();
    let active_provider_label = active_provider_label_option.unwrap_or("-");
    let capability_snapshot_sha256 = runtime.capability_snapshot_sha256.as_str();
    let tool_calling = &runtime.tool_calling;
    let runtime_diagnostics = &status.runtime_diagnostics;
    let tool_workspace = &runtime_diagnostics.tool_workspace;
    let audit_integrity = &runtime_diagnostics.audit_integrity;
    let current_working_directory =
        render_optional_text(tool_workspace.current_working_directory.as_deref());
    let configured_file_root = render_optional_text(tool_workspace.configured_file_root.as_deref());
    let tool_calling_remediation = render_optional_text(tool_calling.remediation.as_deref());
    let tool_workspace_remediation = render_optional_text(tool_workspace.remediation.as_deref());
    let audit_integrity_remediation = render_optional_text(audit_integrity.remediation.as_deref());
    let verdict = &runtime_diagnostics.verdict;
    let recommended_actions = if verdict.recommended_actions.is_empty() {
        "-".to_owned()
    } else {
        verdict.recommended_actions.join(" | ")
    };

    let mut lines = Vec::new();
    lines.push(format!("config={}", status.config));
    lines.push(format!(
        "gateway phase={} running={} stale={} mode={} pid={} session={} control_base_url={} owner_config={} loopback_only={} surfaces_configured={} surfaces_running={}",
        owner.phase,
        owner.running,
        owner.stale,
        owner.mode.as_str(),
        owner_pid,
        owner_session,
        base_url,
        owner.config_path,
        control_surface.loopback_only,
        owner.configured_surface_count,
        owner.running_surface_count,
    ));
    lines.push(format!(
        "gateway_shutdown_reason={} gateway_last_error={}",
        owner_shutdown_reason, owner_error,
    ));
    lines.push(format!(
        "channels catalog={} configured={} enabled_accounts={} misconfigured_accounts={} runtime_backed={} enabled_service_channels={} ready_service_channels={}",
        channels.catalog_channel_count,
        channels.configured_account_count,
        channels.enabled_account_count,
        channels.misconfigured_account_count,
        channels.runtime_backed_channel_count,
        channels.enabled_service_channel_count,
        channels.ready_service_channel_count,
    ));
    lines.push(format!(
        "runtime provider_profile={} provider_label={} visible_tool_count={} capability_snapshot_sha256={}",
        active_provider_profile_id,
        active_provider_label,
        runtime.visible_tool_count,
        capability_snapshot_sha256,
    ));
    lines.push(format!(
        "operator_verdict level={} summary={} recommended_actions={}",
        verdict.level, verdict.summary, recommended_actions,
    ));
    lines.push(format!(
        "tool_calling availability={} level={} structured_tool_schema_enabled={} mode={} active_model={} reason={} remediation={}",
        tool_calling.availability,
        tool_calling.level,
        tool_calling.structured_tool_schema_enabled,
        tool_calling.effective_tool_schema_mode,
        tool_calling.active_model,
        tool_calling.reason,
        tool_calling_remediation,
    ));
    lines.push(format!(
        "tool_workspace binding={} level={} configured_file_root={} effective_file_root={} current_working_directory={} reason={} remediation={}",
        tool_workspace.binding,
        tool_workspace.level,
        configured_file_root,
        tool_workspace.effective_file_root,
        current_working_directory,
        tool_workspace.reason,
        tool_workspace_remediation,
    ));
    lines.push(format!(
        "audit_integrity availability={} level={} mode={} journal_path={} reason={} remediation={}",
        audit_integrity.availability,
        audit_integrity.level,
        audit_integrity.mode,
        audit_integrity.journal_path,
        audit_integrity.reason,
        audit_integrity_remediation,
    ));
    lines.push(render_status_cli_acp_text(&status.acp));
    lines.push(render_status_cli_work_units_text(&status.work_units));

    if !status.recipes.is_empty() {
        lines.push("recipes:".to_owned());
        for recipe in &status.recipes {
            lines.push(format!("- {recipe}"));
        }
    }

    lines.join("\n")
}

fn render_status_cli_acp_text(acp: &StatusCliAcpReadModel) -> String {
    let persisted_session_count = render_optional_usize(acp.persisted_session_count);
    let availability = acp.availability.as_str();

    if let Some(observability) = &acp.observability {
        let snapshot = &observability.snapshot;
        let error_values = snapshot.errors_by_code.values();
        let error_values = error_values.copied();
        let error_total = error_values.sum::<usize>();
        let line = format!(
            "acp enabled={} availability={} persisted_sessions={} runtime_active_sessions={} bound_sessions={} unbound_sessions={} actor_queue_depth={} turn_queue_depth={} turn_failures={} error_total={}",
            acp.enabled,
            availability,
            persisted_session_count,
            snapshot.runtime_cache.active_sessions,
            snapshot.sessions.bound,
            snapshot.sessions.unbound,
            snapshot.actors.queue_depth,
            snapshot.turns.queue_depth,
            snapshot.turns.failed,
            error_total,
        );
        return line;
    }

    let error_option = acp.error.as_deref();
    let error = error_option.unwrap_or("-");
    format!(
        "acp enabled={} availability={} persisted_sessions={} error={}",
        acp.enabled, availability, persisted_session_count, error,
    )
}

fn render_status_cli_work_units_text(work_units: &StatusCliWorkUnitReadModel) -> String {
    let availability = work_units.availability.as_str();

    if let Some(health) = &work_units.health {
        let line = format!(
            "work_units availability={} total_count={} ready_count={} leased_count={} running_count={} blocked_count={} retry_pending_count={} terminal_count={} archived_count={} expired_lease_count={}",
            availability,
            health.total_count,
            health.ready_count,
            health.leased_count,
            health.running_count,
            health.blocked_count,
            health.retry_pending_count,
            health.terminal_count,
            health.archived_count,
            health.expired_lease_count,
        );
        return line;
    }

    let error_option = work_units.error.as_deref();
    let error = error_option.unwrap_or("-");
    format!("work_units availability={} error={}", availability, error)
}

fn render_optional_u32(value: Option<u32>) -> String {
    let value = value.map(|value| value.to_string());
    value.unwrap_or_else(|| "-".to_owned())
}

fn render_optional_usize(value: Option<usize>) -> String {
    let value = value.map(|value| value.to_string());
    value.unwrap_or_else(|| "-".to_owned())
}

fn render_optional_text(value: Option<&str>) -> String {
    value.unwrap_or("-").to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::read_models::{
        GatewayOperatorChannelsSummaryReadModel, GatewayOperatorControlSurfaceReadModel,
        GatewayOperatorRuntimeSummaryReadModel,
    };
    use crate::gateway::state::{GatewayOwnerMode, GatewayOwnerStatus};
    use kernel::AuditSink;
    use kernel::JsonlAuditSink;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn status_cli_temp_dir(prefix: &str) -> PathBuf {
        static NEXT_STATUS_CLI_TEMP_DIR_SEED: AtomicUsize = AtomicUsize::new(1);
        let seed = NEXT_STATUS_CLI_TEMP_DIR_SEED.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be after epoch")
            .as_nanos();
        let process_id = std::process::id();

        std::env::temp_dir().join(format!("{prefix}-{process_id}-{seed}-{nanos}"))
    }

    fn sample_audit_event(
        event_id: &str,
        occurred_at_ms: u64,
        agent_id: Option<&str>,
    ) -> kernel::AuditEvent {
        kernel::AuditEvent {
            event_id: event_id.to_owned(),
            timestamp_epoch_s: occurred_at_ms,
            agent_id: agent_id.map(ToOwned::to_owned),
            kind: kernel::AuditEventKind::TokenRevoked {
                token_id: format!("token-{event_id}"),
            },
        }
    }

    #[test]
    fn collect_tool_workspace_binding_state_reports_external_root_when_file_root_differs() {
        let root = status_cli_temp_dir("status-cli-tool-workspace");
        fs::create_dir_all(&root).expect("create workspace root");
        let mut config = mvp::config::LoongClawConfig::default();
        config.tools.file_root = Some(root.display().to_string());

        let state =
            crate::operator_runtime_diagnostics::collect_tool_workspace_binding_state(&config);
        let root_text = root.display().to_string();

        assert_eq!(state.binding, "external");
        assert_eq!(state.level, "degraded");
        assert_eq!(
            state.configured_file_root.as_deref(),
            Some(root_text.as_str())
        );
        assert_eq!(state.effective_file_root, root_text);
        assert_eq!(
            state.reason,
            "runtime tools resolve outside the current working directory; file operations target the configured tool root instead"
        );

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn collect_audit_integrity_state_reports_failed_for_tampered_chain() {
        let root = status_cli_temp_dir("status-cli-audit-integrity");
        let journal_path = root.join("events.jsonl");
        fs::create_dir_all(&root).expect("create audit root");
        let sink = JsonlAuditSink::new(journal_path.clone()).expect("create jsonl sink");

        sink.record(sample_audit_event("evt-1", 1_700_010_400, Some("agent-a")))
            .expect("record first event");
        sink.record(sample_audit_event("evt-2", 1_700_010_401, Some("agent-b")))
            .expect("record second event");

        let contents = fs::read_to_string(&journal_path).expect("read audit journal");
        let tampered = contents.replacen("token-evt-2", "token-evt-x", 1);
        fs::write(&journal_path, tampered).expect("rewrite tampered journal");

        let state = crate::operator_runtime_diagnostics::collect_audit_integrity_state(
            &mvp::config::AuditConfig {
                mode: mvp::config::AuditMode::Jsonl,
                path: journal_path.display().to_string(),
                retain_in_memory: false,
            },
        );

        assert_eq!(state.availability, "failed");
        assert_eq!(state.level, "blocked");
        assert_eq!(state.mode, "jsonl");
        assert!(state.reason.contains("failed at line 2"));
        assert!(state.remediation.is_some());

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn render_status_cli_text_surfaces_drill_down_recipes() {
        let gateway = GatewayOperatorSummaryReadModel {
            owner: GatewayOwnerStatus {
                runtime_dir: "/tmp/runtime".to_owned(),
                phase: "running".to_owned(),
                running: true,
                stale: false,
                pid: Some(42),
                mode: GatewayOwnerMode::GatewayHeadless,
                version: "0.0.0-test".to_owned(),
                config_path: "/tmp/config.toml".to_owned(),
                attached_cli_session: None,
                started_at_ms: 1,
                last_heartbeat_at: 2,
                stopped_at_ms: None,
                shutdown_reason: None,
                last_error: None,
                configured_surface_count: 1,
                running_surface_count: 1,
                bind_address: Some("127.0.0.1".to_owned()),
                port: Some(7777),
                token_path: Some("/tmp/token".to_owned()),
            },
            control_surface: GatewayOperatorControlSurfaceReadModel {
                base_url: Some("http://127.0.0.1:7777".to_owned()),
                loopback_only: true,
            },
            channels: GatewayOperatorChannelsSummaryReadModel {
                catalog_channel_count: 1,
                configured_channel_count: 1,
                configured_account_count: 1,
                enabled_account_count: 1,
                misconfigured_account_count: 0,
                runtime_backed_channel_count: 1,
                enabled_service_channel_count: 1,
                ready_service_channel_count: 1,
                surfaces: Vec::new(),
            },
            runtime: GatewayOperatorRuntimeSummaryReadModel {
                enabled_channel_ids: vec!["telegram".to_owned()],
                enabled_service_channel_ids: vec!["telegram".to_owned()],
                visible_tool_count: 4,
                capability_snapshot_sha256: "abc123".to_owned(),
                active_provider_profile_id: Some("demo".to_owned()),
                active_provider_label: Some("Demo".to_owned()),
                tool_calling: crate::gateway::read_models::GatewayToolCallingReadModel {
                    availability: "ready".to_owned(),
                    level: "healthy".to_owned(),
                    structured_tool_schema_enabled: true,
                    effective_tool_schema_mode: "enabled_with_downgrade".to_owned(),
                    active_model: "gpt-4.1-mini".to_owned(),
                    reason:
                        "provider turns include structured tool definitions for the active model"
                            .to_owned(),
                    remediation: None,
                },
                runtime_diagnostics: crate::gateway::read_models::GatewayRuntimeDiagnosticsReadModel {
                    verdict: crate::RuntimeOperatorVerdictState {
                        level: "degraded".to_owned(),
                        summary:
                            "operator diagnostics found degraded runtime conditions that can make the local agent appear unreliable"
                                .to_owned(),
                        recommended_actions: vec![
                            "Run from the configured tool workspace or update tools.file_root to the intended working tree"
                                .to_owned(),
                            "Perform a durable write and re-run verification before treating audit integrity as established"
                                .to_owned(),
                        ],
                    },
                    tool_calling: crate::RuntimeSnapshotToolCallingState {
                        availability: "ready".to_owned(),
                        level: "healthy".to_owned(),
                        structured_tool_schema_enabled: true,
                        effective_tool_schema_mode: "enabled_with_downgrade".to_owned(),
                        active_model: "gpt-4.1-mini".to_owned(),
                        reason:
                            "provider turns include structured tool definitions for the active model"
                                .to_owned(),
                        remediation: None,
                    },
                    tool_workspace: crate::ToolWorkspaceBindingState {
                        binding: "external".to_owned(),
                        level: "degraded".to_owned(),
                        configured_file_root: Some("/tmp/workspace".to_owned()),
                        effective_file_root: "/tmp/workspace".to_owned(),
                        current_working_directory: Some("/Users/chum/loongclaw".to_owned()),
                        reason:
                            "runtime tools resolve outside the current working directory; file operations target the configured tool root instead"
                                .to_owned(),
                        remediation: Some(
                            "Run from the configured tool workspace or update tools.file_root to the intended working tree"
                                .to_owned(),
                        ),
                    },
                    audit_integrity: crate::AuditIntegrityState {
                        availability: "missing".to_owned(),
                        level: "advisory".to_owned(),
                        mode: "jsonl".to_owned(),
                        journal_path: "/tmp/audit/events.jsonl".to_owned(),
                        reason:
                            "audit journal has not been created yet, so integrity verification is unavailable until the first durable write"
                                .to_owned(),
                        remediation: Some(
                            "Perform a durable write and re-run verification before treating audit integrity as established"
                                .to_owned(),
                        ),
                    },
                },
            },
        };
        let status = StatusCliReadModel {
            config: "/tmp/config.toml".to_owned(),
            schema: StatusCliJsonSchema {
                version: STATUS_CLI_JSON_SCHEMA_VERSION,
                surface: "status",
                purpose: "operator_runtime_summary",
            },
            gateway,
            runtime_diagnostics: RuntimeOperatorDiagnosticsState {
                verdict: crate::operator_runtime_diagnostics::RuntimeOperatorVerdictState {
                    level: "degraded".to_owned(),
                    summary:
                        "operator diagnostics found degraded runtime conditions that can make the local agent appear unreliable"
                            .to_owned(),
                    recommended_actions: vec![
                        "Run from the configured tool workspace or update tools.file_root to the intended working tree"
                            .to_owned(),
                        "Perform a durable write and re-run verification before treating audit integrity as established"
                            .to_owned(),
                    ],
                },
                tool_calling: crate::tool_calling_readiness::RuntimeSnapshotToolCallingState {
                    availability: "ready".to_owned(),
                    level: "healthy".to_owned(),
                    structured_tool_schema_enabled: true,
                    effective_tool_schema_mode: "enabled_with_downgrade".to_owned(),
                    active_model: "gpt-4.1-mini".to_owned(),
                    reason:
                        "provider turns include structured tool definitions for the active model"
                            .to_owned(),
                    remediation: None,
                },
                tool_workspace: crate::operator_runtime_diagnostics::ToolWorkspaceBindingState {
                    binding: "external".to_owned(),
                    level: "degraded".to_owned(),
                    configured_file_root: Some("/tmp/workspace".to_owned()),
                    effective_file_root: "/tmp/workspace".to_owned(),
                    current_working_directory: Some("/Users/chum/loongclaw".to_owned()),
                    reason:
                        "runtime tools resolve outside the current working directory; file operations target the configured tool root instead"
                            .to_owned(),
                    remediation: Some(
                        "Run from the configured tool workspace or update tools.file_root to the intended working tree"
                            .to_owned(),
                    ),
                },
                audit_integrity: crate::operator_runtime_diagnostics::AuditIntegrityState {
                    availability: "missing".to_owned(),
                    level: "advisory".to_owned(),
                    mode: "jsonl".to_owned(),
                    journal_path: "/tmp/audit/events.jsonl".to_owned(),
                    reason:
                        "audit journal has not been created yet, so integrity verification is unavailable until the first durable write"
                            .to_owned(),
                    remediation: Some(
                        "Perform a durable write and re-run verification before treating audit integrity as established"
                            .to_owned(),
                    ),
                },
            },
            acp: StatusCliAcpReadModel {
                enabled: false,
                availability: "disabled".to_owned(),
                error: None,
                persisted_session_count: Some(0),
                observability: None,
            },
            work_units: StatusCliWorkUnitReadModel {
                availability: "available".to_owned(),
                error: None,
                health: Some(WorkRuntimeHealthSnapshot {
                    total_count: 0,
                    ready_count: 0,
                    leased_count: 0,
                    running_count: 0,
                    blocked_count: 0,
                    retry_pending_count: 0,
                    terminal_count: 0,
                    archived_count: 0,
                    expired_lease_count: 0,
                }),
            },
            recipes: vec![
                "loong doctor --config '/tmp/config.toml' --json".to_owned(),
                "loong audit verify --config '/tmp/config.toml'".to_owned(),
                "loong gateway status".to_owned(),
            ],
        };

        let rendered = render_status_cli_text(&status);

        assert!(rendered.contains("gateway phase=running"));
        assert!(rendered.contains("operator_verdict level=degraded"));
        assert!(rendered.contains("tool_calling availability=ready"));
        assert!(rendered.contains("level=healthy"));
        assert!(rendered.contains("tool_workspace binding=external"));
        assert!(rendered.contains("audit_integrity availability=missing"));
        assert!(rendered.contains("acp enabled=false availability=disabled"));
        assert!(rendered.contains("work_units availability=available total_count=0"));
        assert!(rendered.contains("recipes:"));
        assert!(rendered.contains("gateway status"));
        assert!(rendered.contains("audit verify --config"));
    }
}
