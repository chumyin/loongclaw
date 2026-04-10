use std::fs;
use std::path::{Path, PathBuf};

use kernel::verify_jsonl_audit_journal;
use serde::{Deserialize, Serialize};

use crate::mvp;
use crate::tool_calling_readiness::RuntimeSnapshotToolCallingState;
use crate::tool_calling_readiness::collect_runtime_snapshot_tool_calling_state;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolWorkspaceBindingState {
    pub binding: String,
    pub level: String,
    pub configured_file_root: Option<String>,
    pub effective_file_root: String,
    pub current_working_directory: Option<String>,
    pub reason: String,
    pub remediation: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AuditIntegrityState {
    pub availability: String,
    pub level: String,
    pub mode: String,
    pub journal_path: String,
    pub reason: String,
    pub remediation: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimeOperatorDiagnosticsState {
    pub verdict: RuntimeOperatorVerdictState,
    pub tool_calling: RuntimeSnapshotToolCallingState,
    pub tool_workspace: ToolWorkspaceBindingState,
    pub audit_integrity: AuditIntegrityState,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimeOperatorVerdictState {
    pub level: String,
    pub summary: String,
    pub recommended_actions: Vec<String>,
}

pub fn collect_runtime_operator_diagnostics_state(
    config: &mvp::config::LoongClawConfig,
    visible_tool_count: usize,
) -> RuntimeOperatorDiagnosticsState {
    let tool_calling = collect_runtime_snapshot_tool_calling_state(config, visible_tool_count);
    let tool_workspace = collect_tool_workspace_binding_state(config);
    let audit_integrity = collect_audit_integrity_state(&config.audit);
    let verdict =
        collect_runtime_operator_verdict_state(&tool_calling, &tool_workspace, &audit_integrity);

    RuntimeOperatorDiagnosticsState {
        verdict,
        tool_calling,
        tool_workspace,
        audit_integrity,
    }
}

fn collect_runtime_operator_verdict_state(
    tool_calling: &RuntimeSnapshotToolCallingState,
    tool_workspace: &ToolWorkspaceBindingState,
    audit_integrity: &AuditIntegrityState,
) -> RuntimeOperatorVerdictState {
    let levels = [
        tool_calling.level.as_str(),
        tool_workspace.level.as_str(),
        audit_integrity.level.as_str(),
    ];
    let level = if levels.contains(&"blocked") {
        "blocked"
    } else if levels.contains(&"degraded") {
        "degraded"
    } else if levels.contains(&"advisory") {
        "advisory"
    } else {
        "healthy"
    };
    let summary = match level {
        "blocked" => {
            "operator diagnostics found at least one blocking runtime issue that should be repaired before trusting the local agent"
                .to_owned()
        }
        "degraded" => {
            "operator diagnostics found degraded runtime conditions that can make the local agent appear unreliable"
                .to_owned()
        }
        "advisory" => {
            "operator diagnostics found advisory runtime drift worth addressing for a more predictable operator setup"
                .to_owned()
        }
        _ => "operator diagnostics are healthy across tool calling, workspace binding, and audit integrity"
            .to_owned(),
    };
    let mut recommended_actions = Vec::new();

    push_unique_action(
        &mut recommended_actions,
        tool_calling.remediation.as_deref(),
    );
    push_unique_action(
        &mut recommended_actions,
        tool_workspace.remediation.as_deref(),
    );
    push_unique_action(
        &mut recommended_actions,
        audit_integrity.remediation.as_deref(),
    );

    RuntimeOperatorVerdictState {
        level: level.to_owned(),
        summary,
        recommended_actions,
    }
}

fn push_unique_action(actions: &mut Vec<String>, action: Option<&str>) {
    let Some(action) = action else {
        return;
    };
    let trimmed_action = action.trim();
    if trimmed_action.is_empty() {
        return;
    }
    let already_present = actions.iter().any(|existing| existing == trimmed_action);
    if already_present {
        return;
    }
    actions.push(trimmed_action.to_owned());
}

pub fn collect_tool_workspace_binding_state(
    config: &mvp::config::LoongClawConfig,
) -> ToolWorkspaceBindingState {
    let configured_file_root = config
        .tools
        .file_root
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);
    let effective_file_root_path = config.tools.resolved_file_root();
    let effective_file_root = effective_file_root_path.display().to_string();
    let current_directory_result = std::env::current_dir();
    let current_directory = current_directory_result
        .as_ref()
        .ok()
        .map(|path| path.display().to_string());

    if configured_file_root.is_none() {
        return ToolWorkspaceBindingState {
            binding: "cwd_fallback".to_owned(),
            level: "advisory".to_owned(),
            configured_file_root: None,
            effective_file_root,
            current_working_directory: current_directory,
            reason:
                "runtime tools resolve relative to the current working directory because tools.file_root is unset"
                    .to_owned(),
            remediation: Some(
                "Set tools.file_root explicitly if you want a stable workspace binding across shells and launches"
                    .to_owned(),
            ),
        };
    }

    let current_directory_path = match current_directory_result {
        Ok(path) => path,
        Err(error) => {
            return ToolWorkspaceBindingState {
                binding: "unknown".to_owned(),
                level: "degraded".to_owned(),
                configured_file_root,
                effective_file_root,
                current_working_directory: None,
                reason: format!("failed to resolve current working directory: {error}"),
                remediation: Some(
                    "Restore current working directory access or set tools.file_root explicitly before relying on file tools"
                        .to_owned(),
                ),
            };
        }
    };
    let canonical_effective_file_root = canonicalize_path_for_comparison(&effective_file_root_path);
    let canonical_current_directory = canonicalize_path_for_comparison(&current_directory_path);

    if canonical_effective_file_root == canonical_current_directory {
        return ToolWorkspaceBindingState {
            binding: "aligned".to_owned(),
            level: "healthy".to_owned(),
            configured_file_root,
            effective_file_root,
            current_working_directory: Some(current_directory_path.display().to_string()),
            reason: "runtime tools resolve under the current working directory".to_owned(),
            remediation: None,
        };
    }

    ToolWorkspaceBindingState {
        binding: "external".to_owned(),
        level: "degraded".to_owned(),
        configured_file_root,
        effective_file_root,
        current_working_directory: Some(current_directory_path.display().to_string()),
        reason:
            "runtime tools resolve outside the current working directory; file operations target the configured tool root instead"
                .to_owned(),
        remediation: Some(
            "Run from the configured tool workspace or update tools.file_root to the intended working tree"
                .to_owned(),
        ),
    }
}

pub fn collect_audit_integrity_state(audit: &mvp::config::AuditConfig) -> AuditIntegrityState {
    let mode = audit.mode.as_str();
    let journal_path = audit.resolved_path();
    let journal_path_text = journal_path.display().to_string();

    if matches!(audit.mode, mvp::config::AuditMode::InMemory) {
        return AuditIntegrityState {
            availability: "in_memory".to_owned(),
            level: "advisory".to_owned(),
            mode: mode.to_owned(),
            journal_path: journal_path_text,
            reason: "audit integrity verification is unavailable while audit.mode=in_memory"
                .to_owned(),
            remediation: Some(
                "Use audit.mode = \"fanout\" or \"jsonl\" if durable audit verification is required"
                    .to_owned(),
            ),
        };
    }

    if !journal_path.exists() {
        return AuditIntegrityState {
            availability: "missing".to_owned(),
            level: "advisory".to_owned(),
            mode: mode.to_owned(),
            journal_path: journal_path_text,
            reason:
                "audit journal has not been created yet, so integrity verification is unavailable until the first durable write"
                    .to_owned(),
            remediation: Some(
                "Perform a durable write and re-run verification before treating audit integrity as established"
                    .to_owned(),
            ),
        };
    }

    match verify_jsonl_audit_journal(&journal_path) {
        Ok(report) if report.valid => AuditIntegrityState {
            availability: "verified".to_owned(),
            level: "healthy".to_owned(),
            mode: mode.to_owned(),
            journal_path: journal_path_text,
            reason: format!(
                "verified {} of {} audit events (last_entry_hash={})",
                report.verified_events,
                report.total_events,
                report.last_entry_hash.as_deref().unwrap_or("-")
            ),
            remediation: None,
        },
        Ok(report) => AuditIntegrityState {
            availability: "failed".to_owned(),
            level: "blocked".to_owned(),
            mode: mode.to_owned(),
            journal_path: journal_path_text,
            reason: format!(
                "audit journal integrity failed at line {} ({})",
                report
                    .first_invalid_line
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "-".to_owned()),
                report.reason.as_deref().unwrap_or("unknown reason")
            ),
            remediation: Some(
                "Inspect or repair the durable audit journal before trusting retained runtime evidence"
                    .to_owned(),
            ),
        },
        Err(error) => AuditIntegrityState {
            availability: "unavailable".to_owned(),
            level: "blocked".to_owned(),
            mode: mode.to_owned(),
            journal_path: journal_path_text,
            reason: format!("audit integrity verification failed: {error}"),
            remediation: Some(
                "Repair the audit journal path or permissions before relying on durable audit evidence"
                    .to_owned(),
            ),
        },
    }
}

fn canonicalize_path_for_comparison(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::collect_runtime_operator_diagnostics_state;
    use crate::mvp;

    #[test]
    fn runtime_operator_diagnostics_verdict_uses_strongest_present_level() {
        let mut config = mvp::config::LoongClawConfig::default();
        config.audit.mode = mvp::config::AuditMode::Jsonl;
        config.audit.path = "/tmp/loongclaw-missing-audit-journal/does-not-exist.jsonl".to_owned();

        let diagnostics = collect_runtime_operator_diagnostics_state(&config, 0);

        assert_eq!(diagnostics.verdict.level, "degraded");
        assert!(!diagnostics.verdict.recommended_actions.is_empty());
    }
}
