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
    pub configured_file_root: Option<String>,
    pub effective_file_root: String,
    pub current_working_directory: Option<String>,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AuditIntegrityState {
    pub availability: String,
    pub mode: String,
    pub journal_path: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimeOperatorDiagnosticsState {
    pub tool_calling: RuntimeSnapshotToolCallingState,
    pub tool_workspace: ToolWorkspaceBindingState,
    pub audit_integrity: AuditIntegrityState,
}

pub fn collect_runtime_operator_diagnostics_state(
    config: &mvp::config::LoongClawConfig,
    visible_tool_count: usize,
) -> RuntimeOperatorDiagnosticsState {
    let tool_calling = collect_runtime_snapshot_tool_calling_state(config, visible_tool_count);
    let tool_workspace = collect_tool_workspace_binding_state(config);
    let audit_integrity = collect_audit_integrity_state(&config.audit);

    RuntimeOperatorDiagnosticsState {
        tool_calling,
        tool_workspace,
        audit_integrity,
    }
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
            configured_file_root: None,
            effective_file_root,
            current_working_directory: current_directory,
            reason:
                "runtime tools resolve relative to the current working directory because tools.file_root is unset"
                    .to_owned(),
        };
    }

    let current_directory_path = match current_directory_result {
        Ok(path) => path,
        Err(error) => {
            return ToolWorkspaceBindingState {
                binding: "unknown".to_owned(),
                configured_file_root,
                effective_file_root,
                current_working_directory: None,
                reason: format!("failed to resolve current working directory: {error}"),
            };
        }
    };
    let canonical_effective_file_root = canonicalize_path_for_comparison(&effective_file_root_path);
    let canonical_current_directory = canonicalize_path_for_comparison(&current_directory_path);

    if canonical_effective_file_root == canonical_current_directory {
        return ToolWorkspaceBindingState {
            binding: "aligned".to_owned(),
            configured_file_root,
            effective_file_root,
            current_working_directory: Some(current_directory_path.display().to_string()),
            reason: "runtime tools resolve under the current working directory".to_owned(),
        };
    }

    ToolWorkspaceBindingState {
        binding: "external".to_owned(),
        configured_file_root,
        effective_file_root,
        current_working_directory: Some(current_directory_path.display().to_string()),
        reason:
            "runtime tools resolve outside the current working directory; file operations target the configured tool root instead"
                .to_owned(),
    }
}

pub fn collect_audit_integrity_state(audit: &mvp::config::AuditConfig) -> AuditIntegrityState {
    let mode = audit.mode.as_str();
    let journal_path = audit.resolved_path();
    let journal_path_text = journal_path.display().to_string();

    if matches!(audit.mode, mvp::config::AuditMode::InMemory) {
        return AuditIntegrityState {
            availability: "in_memory".to_owned(),
            mode: mode.to_owned(),
            journal_path: journal_path_text,
            reason: "audit integrity verification is unavailable while audit.mode=in_memory"
                .to_owned(),
        };
    }

    if !journal_path.exists() {
        return AuditIntegrityState {
            availability: "missing".to_owned(),
            mode: mode.to_owned(),
            journal_path: journal_path_text,
            reason:
                "audit journal has not been created yet, so integrity verification is unavailable until the first durable write"
                    .to_owned(),
        };
    }

    match verify_jsonl_audit_journal(&journal_path) {
        Ok(report) if report.valid => AuditIntegrityState {
            availability: "verified".to_owned(),
            mode: mode.to_owned(),
            journal_path: journal_path_text,
            reason: format!(
                "verified {} of {} audit events (last_entry_hash={})",
                report.verified_events,
                report.total_events,
                report.last_entry_hash.as_deref().unwrap_or("-")
            ),
        },
        Ok(report) => AuditIntegrityState {
            availability: "failed".to_owned(),
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
        },
        Err(error) => AuditIntegrityState {
            availability: "unavailable".to_owned(),
            mode: mode.to_owned(),
            journal_path: journal_path_text,
            reason: format!("audit integrity verification failed: {error}"),
        },
    }
}

fn canonicalize_path_for_comparison(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}
