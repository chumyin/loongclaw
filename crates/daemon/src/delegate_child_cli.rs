use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::time::sleep;
use wait_timeout::ChildExt;

use crate::CliResult;
use crate::mvp;

const DETACHED_DELEGATE_CHILD_COMMAND: &str = "delegate-child-run";
const DETACHED_DELEGATE_CHILD_CONFIG_ARG: &str = "--config-path";
const DETACHED_DELEGATE_CHILD_PAYLOAD_ARG: &str = "--payload-file";
const DETACHED_DELEGATE_CHILD_EXECUTABLE_ENV: &str = "CARGO_BIN_EXE_loong";
const DETACHED_DELEGATE_CHILD_KERNEL_SCOPE: &str = "delegate-child-worker";
const DETACHED_DELEGATE_CANCEL_POLL_INTERVAL: Duration = Duration::from_millis(100);
const DETACHED_DELEGATE_OWNER_BOUND_EVENT_KIND: &str = "delegate_runtime_owner_bound";
const DETACHED_DELEGATE_OWNER_KIND: &str = "detached_process";
const DETACHED_DELEGATE_CHILD_PASSTHROUGH_ENV_KEYS: &[&str] =
    &["LOONGCLAW_CONFIG_PATH", "LOONG_HOME", "LOONGCLAW_HOME"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum DetachedDelegateChildBinding {
    Kernel,
    Direct,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct DetachedDelegateChildPayload {
    child_session_id: String,
    parent_session_id: String,
    task: String,
    label: Option<String>,
    profile: Option<mvp::conversation::DelegateBuiltinProfile>,
    execution: mvp::conversation::ConstrainedSubagentExecution,
    runtime_self_continuity: Option<serde_json::Value>,
    timeout_seconds: u64,
    binding: DetachedDelegateChildBinding,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DetachedDelegateCancellationRequest {
    reference: String,
    reason: String,
    requested_at: i64,
    actor_session_id: Option<String>,
}

impl DetachedDelegateChildPayload {
    fn from_request(request: &mvp::conversation::AsyncDelegateSpawnRequest) -> Self {
        let binding = if request.binding.is_kernel_bound() {
            DetachedDelegateChildBinding::Kernel
        } else {
            DetachedDelegateChildBinding::Direct
        };

        Self {
            child_session_id: request.child_session_id.clone(),
            parent_session_id: request.parent_session_id.clone(),
            task: request.task.clone(),
            label: request.label.clone(),
            profile: request.profile,
            execution: request.execution.clone(),
            runtime_self_continuity: request
                .runtime_self_continuity_json()
                .expect("delegate payload serialization should succeed"),
            timeout_seconds: request.timeout_seconds,
            binding,
        }
    }

    fn into_spawn_request(
        self,
        binding: mvp::conversation::OwnedConversationRuntimeBinding,
    ) -> CliResult<mvp::conversation::AsyncDelegateSpawnRequest> {
        mvp::conversation::async_delegate_spawn_request_from_serialized_parts(
            self.child_session_id,
            self.parent_session_id,
            self.task,
            self.label,
            self.profile,
            self.execution,
            self.runtime_self_continuity,
            self.timeout_seconds,
            binding,
        )
    }
}

pub(crate) fn spawn_detached_delegate_child_process(
    request: &mvp::conversation::AsyncDelegateSpawnRequest,
) -> CliResult<()> {
    let executable_path = resolve_detached_delegate_child_executable_path()?;
    let config_path = resolve_detached_delegate_child_config_path()?;
    let payload = DetachedDelegateChildPayload::from_request(request);
    let payload_path = materialize_detached_delegate_child_payload_file(&payload)?;

    let mut command = std::process::Command::new(&executable_path);
    command.arg(DETACHED_DELEGATE_CHILD_COMMAND);
    command.arg(DETACHED_DELEGATE_CHILD_CONFIG_ARG);
    command.arg(config_path.as_os_str());
    command.arg(DETACHED_DELEGATE_CHILD_PAYLOAD_ARG);
    command.arg(payload_path.as_os_str());
    command.stdin(Stdio::null());
    command.stdout(Stdio::null());
    command.stderr(Stdio::piped());
    propagate_detached_delegate_child_environment(&mut command);

    let spawn_result = command.spawn();

    match spawn_result {
        Ok(mut child) => {
            let startup_timeout = std::time::Duration::from_millis(500);
            let startup_status = child.wait_timeout(startup_timeout).map_err(|error| {
                format!("wait for detached delegate child startup failed: {error}")
            })?;

            if let Some(exit_status) = startup_status {
                let stderr = read_detached_delegate_child_stderr(&mut child);
                remove_detached_delegate_child_payload_file(payload_path.as_path());
                let startup_failure =
                    detached_delegate_child_startup_failure(&exit_status, stderr.as_str());

                if let Some(startup_failure) = startup_failure {
                    return Err(startup_failure);
                }

                return Ok(());
            }

            Ok(())
        }
        Err(error) => {
            remove_detached_delegate_child_payload_file(payload_path.as_path());
            Err(format!(
                "delegate_async_process_spawn_failed: could not launch detached delegate child via `{}`: {error}",
                executable_path.display()
            ))
        }
    }
}

fn read_detached_delegate_child_stderr(child: &mut std::process::Child) -> String {
    let Some(mut stderr) = child.stderr.take() else {
        return String::new();
    };

    let mut buffer = String::new();
    let _ = stderr.read_to_string(&mut buffer);

    buffer
}

fn detached_delegate_child_startup_failure(
    exit_status: &std::process::ExitStatus,
    stderr: &str,
) -> Option<String> {
    let exited_successfully = exit_status.success();

    if exited_successfully {
        return None;
    }

    let status_code = exit_status
        .code()
        .map(|code| code.to_string())
        .unwrap_or_else(|| "signal".to_owned());
    let trimmed_stderr = stderr.trim();
    let detail = if trimmed_stderr.is_empty() {
        "(empty stderr)".to_owned()
    } else {
        trimmed_stderr.to_owned()
    };
    let failure = format!(
        "delegate_async_process_spawn_failed: detached delegate child exited during startup with status {status_code}: {detail}"
    );

    Some(failure)
}

fn append_detached_delegate_owner_binding_event(
    memory_config: &mvp::memory::runtime_config::MemoryRuntimeConfig,
    request: &mvp::conversation::AsyncDelegateSpawnRequest,
    process_id: u32,
) -> CliResult<()> {
    let repo = mvp::session::repository::SessionRepository::new(memory_config)?;
    let owner_payload = json!({
        "owner_kind": DETACHED_DELEGATE_OWNER_KIND,
        "process_id": process_id,
    });
    let owner_event = mvp::session::repository::NewSessionEvent {
        session_id: request.child_session_id.clone(),
        event_kind: DETACHED_DELEGATE_OWNER_BOUND_EVENT_KIND.to_owned(),
        actor_session_id: Some(request.parent_session_id.clone()),
        payload_json: owner_payload,
    };

    repo.append_event(owner_event)?;

    Ok(())
}

async fn wait_for_detached_delegate_cancellation(
    memory_config: &mvp::memory::runtime_config::MemoryRuntimeConfig,
    child_session_id: &str,
) -> CliResult<DetachedDelegateCancellationRequest> {
    loop {
        let repo = mvp::session::repository::SessionRepository::new(memory_config)?;
        let delegate_events = repo.list_delegate_lifecycle_events(child_session_id)?;
        let cancellation =
            latest_detached_delegate_cancellation_request(delegate_events.as_slice());

        if let Some(cancellation) = cancellation {
            return Ok(cancellation);
        }

        sleep(DETACHED_DELEGATE_CANCEL_POLL_INTERVAL).await;
    }
}

fn latest_detached_delegate_cancellation_request(
    delegate_events: &[mvp::session::repository::SessionEventRecord],
) -> Option<DetachedDelegateCancellationRequest> {
    for event in delegate_events.iter().rev() {
        let event_kind = event.event_kind.as_str();

        if event_kind != mvp::session::DELEGATE_CANCEL_REQUESTED_EVENT_KIND {
            continue;
        }

        let reason = event
            .payload_json
            .get("cancel_reason")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(mvp::session::DELEGATE_CANCEL_REASON_OPERATOR_REQUESTED)
            .to_owned();
        let reference = event
            .payload_json
            .get("reference")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("running")
            .to_owned();
        let cancellation = DetachedDelegateCancellationRequest {
            reference,
            reason,
            requested_at: event.ts,
            actor_session_id: event.actor_session_id.clone(),
        };

        return Some(cancellation);
    }

    None
}

fn finalize_detached_delegate_child_cancellation(
    memory_config: &mvp::memory::runtime_config::MemoryRuntimeConfig,
    request: &mvp::conversation::AsyncDelegateSpawnRequest,
    cancellation: &DetachedDelegateCancellationRequest,
    max_frozen_bytes: usize,
) -> CliResult<()> {
    let repo = mvp::session::repository::SessionRepository::new(memory_config)?;
    let cancel_error = mvp::session::delegate_cancelled_error(cancellation.reason.as_str());
    let outcome = detached_delegate_cancelled_outcome(
        &request.child_session_id,
        request.label.clone(),
        request.profile,
        cancel_error.clone(),
    );
    let frozen_result = detached_delegate_cancelled_frozen_result(&cancel_error, max_frozen_bytes);
    let cancellation_actor_session_id = cancellation
        .actor_session_id
        .clone()
        .or(Some(request.parent_session_id.clone()));
    let outcome_status = outcome.status;
    let outcome_payload_json = outcome.payload;
    let running_request = mvp::session::repository::FinalizeSessionTerminalRequest {
        state: mvp::session::repository::SessionState::Failed,
        last_error: Some(cancel_error.clone()),
        event_kind: mvp::session::DELEGATE_CANCELLED_EVENT_KIND.to_owned(),
        actor_session_id: cancellation_actor_session_id.clone(),
        event_payload_json: detached_delegate_cancelled_event_payload(cancellation, "running"),
        outcome_status: outcome_status.clone(),
        outcome_payload_json: outcome_payload_json.clone(),
        frozen_result: Some(frozen_result.clone()),
    };
    let running_result = repo.finalize_session_terminal_if_current(
        &request.child_session_id,
        mvp::session::repository::SessionState::Running,
        running_request,
    )?;

    if running_result.is_some() {
        return Ok(());
    }

    let queued_request = mvp::session::repository::FinalizeSessionTerminalRequest {
        state: mvp::session::repository::SessionState::Failed,
        last_error: Some(cancel_error),
        event_kind: mvp::session::DELEGATE_CANCELLED_EVENT_KIND.to_owned(),
        actor_session_id: cancellation_actor_session_id,
        event_payload_json: detached_delegate_cancelled_event_payload(cancellation, "queued"),
        outcome_status,
        outcome_payload_json,
        frozen_result: Some(frozen_result),
    };
    let _ = repo.finalize_session_terminal_if_current(
        &request.child_session_id,
        mvp::session::repository::SessionState::Ready,
        queued_request,
    )?;

    Ok(())
}

fn detached_delegate_cancelled_event_payload(
    cancellation: &DetachedDelegateCancellationRequest,
    fallback_reference: &str,
) -> serde_json::Value {
    let reference = if cancellation.reference.is_empty() {
        fallback_reference.to_owned()
    } else {
        cancellation.reference.clone()
    };
    json!({
        "reference": reference,
        "cancel_reason": cancellation.reason,
        "requested_at": cancellation.requested_at,
        "owner_kind": DETACHED_DELEGATE_OWNER_KIND,
    })
}

fn detached_delegate_cancelled_outcome(
    child_session_id: &str,
    label: Option<String>,
    profile: Option<mvp::conversation::DelegateBuiltinProfile>,
    error: String,
) -> loongclaw_contracts::ToolCoreOutcome {
    let mut payload = json!({
        "child_session_id": child_session_id,
        "label": label,
        "duration_ms": 0,
        "error": error,
    });

    if let Some(profile) = profile
        && let Some(payload_object) = payload.as_object_mut()
    {
        let profile_value = json!(profile.as_str());
        payload_object.insert("profile".to_owned(), profile_value);
    }

    loongclaw_contracts::ToolCoreOutcome {
        status: "error".to_owned(),
        payload,
    }
}

fn detached_delegate_cancelled_frozen_result(
    cancel_error: &str,
    max_frozen_bytes: usize,
) -> mvp::session::frozen_result::FrozenResult {
    let effective_max_frozen_bytes = max_frozen_bytes.max(1);
    let truncated = cancel_error.len() > effective_max_frozen_bytes;
    let truncated_message = if truncated {
        cancel_error
            .chars()
            .take(effective_max_frozen_bytes)
            .collect::<String>()
    } else {
        cancel_error.to_owned()
    };
    let byte_len = truncated_message.len();
    let frozen_content = mvp::session::frozen_result::FrozenContent::Error {
        code: truncated_message.clone(),
        message: truncated_message,
    };
    mvp::session::frozen_result::FrozenResult {
        content: frozen_content,
        captured_at: SystemTime::now(),
        byte_len,
        truncated,
    }
}

pub async fn run_detached_delegate_child_cli(
    config_path: &str,
    payload_file: &str,
) -> CliResult<()> {
    let payload_path = PathBuf::from(payload_file);
    let payload = read_detached_delegate_child_payload_file(payload_path.as_path())?;
    remove_detached_delegate_child_payload_file(payload_path.as_path());

    let (resolved_path, config) = mvp::config::load(Some(config_path))?;
    mvp::runtime_env::initialize_runtime_environment(&config, Some(&resolved_path));

    let binding = owned_binding_from_detached_payload(payload.binding, &config)?;
    let spawn_request = payload.into_spawn_request(binding)?;
    let memory_config =
        mvp::memory::runtime_config::MemoryRuntimeConfig::from_memory_config(&config.memory);
    let execute_request = spawn_request.clone();

    append_detached_delegate_owner_binding_event(
        &memory_config,
        &spawn_request,
        std::process::id(),
    )?;

    let execute_future =
        mvp::conversation::execute_async_delegate_spawn_request(&config, execute_request);
    let cancel_future =
        wait_for_detached_delegate_cancellation(&memory_config, &spawn_request.child_session_id);
    tokio::pin!(execute_future);
    tokio::pin!(cancel_future);

    tokio::select! {
        execute_result = &mut execute_future => {
            execute_result?;
        }
        cancellation = &mut cancel_future => {
            let cancellation = cancellation?;
            finalize_detached_delegate_child_cancellation(
                &memory_config,
                &spawn_request,
                &cancellation,
                config.tools.delegate.max_frozen_bytes,
            )?;
        }
    }

    Ok(())
}

fn resolve_detached_delegate_child_executable_path() -> CliResult<PathBuf> {
    let env_path = std::env::var_os(DETACHED_DELEGATE_CHILD_EXECUTABLE_ENV);

    if let Some(env_path) = env_path {
        let candidate_path = PathBuf::from(env_path);
        return Ok(candidate_path);
    }

    let executable_path = std::env::current_exe()
        .map_err(|error| format!("resolve detached delegate executable failed: {error}"))?;

    Ok(executable_path)
}

fn resolve_detached_delegate_child_config_path() -> CliResult<PathBuf> {
    let config_path = std::env::var_os("LOONGCLAW_CONFIG_PATH")
        .map(PathBuf::from)
        .ok_or_else(|| {
            "delegate_async_process_spawn_failed: LOONGCLAW_CONFIG_PATH is not set for detached delegate child startup"
                .to_owned()
        })?;

    Ok(config_path)
}

fn materialize_detached_delegate_child_payload_file(
    payload: &DetachedDelegateChildPayload,
) -> CliResult<PathBuf> {
    let payload_directory = std::env::temp_dir()
        .join("loongclaw")
        .join("delegate-child-payloads");
    std::fs::create_dir_all(&payload_directory)
        .map_err(|error| format!("create detached delegate payload directory failed: {error}"))?;

    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| {
            format!("read system clock for detached delegate payload failed: {error}")
        })?;
    let payload_file_name = format!(
        "delegate-child-{}-{}.json",
        std::process::id(),
        timestamp.as_nanos()
    );
    let payload_path = payload_directory.join(payload_file_name);
    let payload_bytes = serde_json::to_vec(payload)
        .map_err(|error| format!("serialize detached delegate payload failed: {error}"))?;
    std::fs::write(&payload_path, payload_bytes)
        .map_err(|error| format!("write detached delegate payload failed: {error}"))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let metadata = std::fs::metadata(&payload_path)
            .map_err(|error| format!("stat detached delegate payload failed: {error}"))?;
        let mut permissions = metadata.permissions();
        permissions.set_mode(0o600);
        std::fs::set_permissions(&payload_path, permissions)
            .map_err(|error| format!("chmod detached delegate payload failed: {error}"))?;
    }

    Ok(payload_path)
}

fn read_detached_delegate_child_payload_file(
    payload_path: &Path,
) -> CliResult<DetachedDelegateChildPayload> {
    let payload_bytes = std::fs::read(payload_path)
        .map_err(|error| format!("read detached delegate payload failed: {error}"))?;
    let payload = serde_json::from_slice::<DetachedDelegateChildPayload>(&payload_bytes)
        .map_err(|error| format!("parse detached delegate payload failed: {error}"))?;

    Ok(payload)
}

fn remove_detached_delegate_child_payload_file(payload_path: &Path) {
    let _ = std::fs::remove_file(payload_path);
}

fn propagate_detached_delegate_child_environment(command: &mut std::process::Command) {
    for env_key in DETACHED_DELEGATE_CHILD_PASSTHROUGH_ENV_KEYS {
        let env_value = std::env::var_os(env_key);

        if let Some(env_value) = env_value {
            command.env(env_key, env_value);
        }
    }
}

fn owned_binding_from_detached_payload(
    binding: DetachedDelegateChildBinding,
    config: &mvp::config::LoongClawConfig,
) -> CliResult<mvp::conversation::OwnedConversationRuntimeBinding> {
    match binding {
        DetachedDelegateChildBinding::Kernel => {
            let kernel_context = mvp::context::bootstrap_kernel_context_with_config(
                DETACHED_DELEGATE_CHILD_KERNEL_SCOPE,
                mvp::context::DEFAULT_TOKEN_TTL_S,
                config,
            )?;
            let owned_binding =
                mvp::conversation::OwnedConversationRuntimeBinding::kernel(kernel_context);
            Ok(owned_binding)
        }
        DetachedDelegateChildBinding::Direct => {
            let owned_binding = mvp::conversation::OwnedConversationRuntimeBinding::direct();
            Ok(owned_binding)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DetachedDelegateCancellationRequest, detached_delegate_child_startup_failure,
        finalize_detached_delegate_child_cancellation,
        latest_detached_delegate_cancellation_request,
    };
    use crate::mvp;
    use serde_json::json;

    #[cfg(unix)]
    fn exit_status_for(command: &str) -> std::process::ExitStatus {
        std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(command)
            .status()
            .expect("spawn shell command")
    }

    #[cfg(windows)]
    fn exit_status_for(command: &str) -> std::process::ExitStatus {
        std::process::Command::new("cmd")
            .args(["/C", command])
            .status()
            .expect("spawn cmd command")
    }

    #[test]
    fn detached_delegate_child_startup_failure_ignores_fast_success_with_warning_stderr() {
        #[cfg(unix)]
        let exit_status = exit_status_for("exit 0");
        #[cfg(windows)]
        let exit_status = exit_status_for("exit 0");

        let warning_output = "WARN optional runtime-self source missing";
        let failure = detached_delegate_child_startup_failure(&exit_status, warning_output);

        assert_eq!(failure, None);
    }

    #[test]
    fn detached_delegate_child_startup_failure_surfaces_non_zero_exit() {
        #[cfg(unix)]
        let exit_status = exit_status_for("exit 7");
        #[cfg(windows)]
        let exit_status = exit_status_for("exit 7");

        let failure = detached_delegate_child_startup_failure(&exit_status, "spawn failure");

        let failure = failure.expect("non-zero exit should be reported");
        assert!(failure.contains("status 7"), "failure={failure}");
        assert!(failure.contains("spawn failure"), "failure={failure}");
    }

    fn detached_delegate_memory_config(
        label: &str,
    ) -> mvp::memory::runtime_config::MemoryRuntimeConfig {
        let temp_root = std::env::temp_dir().join(format!(
            "loongclaw-detached-delegate-{label}-{}",
            std::process::id()
        ));
        let _ = std::fs::create_dir_all(&temp_root);
        let sqlite_path = temp_root.join("memory.sqlite3");
        let _ = std::fs::remove_file(&sqlite_path);
        mvp::memory::runtime_config::MemoryRuntimeConfig {
            sqlite_path: Some(sqlite_path),
            ..mvp::memory::runtime_config::MemoryRuntimeConfig::default()
        }
    }

    fn detached_delegate_execution() -> mvp::conversation::ConstrainedSubagentExecution {
        mvp::conversation::ConstrainedSubagentExecution {
            mode: mvp::conversation::ConstrainedSubagentMode::Async,
            isolation: mvp::conversation::ConstrainedSubagentIsolation::Shared,
            depth: 1,
            max_depth: 1,
            active_children: 0,
            max_active_children: 5,
            timeout_seconds: 60,
            allow_shell_in_child: false,
            child_tool_allowlist: vec![
                "file.read".to_owned(),
                "file.write".to_owned(),
                "file.edit".to_owned(),
            ],
            workspace_root: None,
            runtime_narrowing: mvp::tools::runtime_config::ToolRuntimeNarrowing::default(),
            kernel_bound: true,
            identity: None,
            profile: None,
        }
    }

    #[test]
    fn latest_detached_delegate_cancellation_request_prefers_latest_cancel_event() {
        let delegate_events = vec![
            mvp::session::repository::SessionEventRecord {
                id: 1,
                session_id: "child-session".to_owned(),
                event_kind: "delegate_started".to_owned(),
                actor_session_id: Some("root-session".to_owned()),
                payload_json: json!({}),
                ts: 10,
            },
            mvp::session::repository::SessionEventRecord {
                id: 2,
                session_id: "child-session".to_owned(),
                event_kind: mvp::session::DELEGATE_CANCEL_REQUESTED_EVENT_KIND.to_owned(),
                actor_session_id: Some("root-session".to_owned()),
                payload_json: json!({
                    "reference": "running",
                    "cancel_reason": "operator_requested",
                }),
                ts: 20,
            },
        ];

        let cancellation =
            latest_detached_delegate_cancellation_request(delegate_events.as_slice())
                .expect("cancellation record");

        assert_eq!(
            cancellation,
            DetachedDelegateCancellationRequest {
                reference: "running".to_owned(),
                reason: "operator_requested".to_owned(),
                requested_at: 20,
                actor_session_id: Some("root-session".to_owned()),
            }
        );
    }

    #[test]
    fn finalize_detached_delegate_child_cancellation_marks_running_child_failed() {
        let memory_config = detached_delegate_memory_config("cancel-finalize");
        let repo = mvp::session::repository::SessionRepository::new(&memory_config)
            .expect("session repository");
        let root_session = mvp::session::repository::NewSessionRecord {
            session_id: "root-session".to_owned(),
            kind: mvp::session::repository::SessionKind::Root,
            parent_session_id: None,
            label: Some("Root".to_owned()),
            state: mvp::session::repository::SessionState::Ready,
        };
        repo.create_session(root_session)
            .expect("create root session");
        let child_session = mvp::session::repository::NewSessionRecord {
            session_id: "child-session".to_owned(),
            kind: mvp::session::repository::SessionKind::DelegateChild,
            parent_session_id: Some("root-session".to_owned()),
            label: Some("Child".to_owned()),
            state: mvp::session::repository::SessionState::Running,
        };
        repo.create_session(child_session)
            .expect("create child session");
        let request = mvp::conversation::async_delegate_spawn_request_from_serialized_parts(
            "child-session".to_owned(),
            "root-session".to_owned(),
            "sleep".to_owned(),
            Some("Child".to_owned()),
            None,
            detached_delegate_execution(),
            None,
            60,
            mvp::conversation::OwnedConversationRuntimeBinding::direct(),
        )
        .expect("async delegate spawn request");
        let cancellation = DetachedDelegateCancellationRequest {
            reference: "running".to_owned(),
            reason: "operator_requested".to_owned(),
            requested_at: 30,
            actor_session_id: Some("root-session".to_owned()),
        };

        finalize_detached_delegate_child_cancellation(
            &memory_config,
            &request,
            &cancellation,
            1024,
        )
        .expect("finalize cancellation");

        let summary = repo
            .load_session_summary_with_legacy_fallback("child-session")
            .expect("load session summary")
            .expect("child summary");
        let terminal_outcome = repo
            .load_terminal_outcome("child-session")
            .expect("load terminal outcome")
            .expect("terminal outcome");
        let recent_events = repo
            .list_recent_events("child-session", 10)
            .expect("recent events");
        let latest_event = recent_events.last().expect("latest event");

        assert_eq!(
            summary.state,
            mvp::session::repository::SessionState::Failed
        );
        assert_eq!(
            latest_event.event_kind,
            mvp::session::DELEGATE_CANCELLED_EVENT_KIND
        );
        assert_eq!(
            latest_event.actor_session_id.as_deref(),
            Some("root-session")
        );
        assert_eq!(terminal_outcome.status, "error");
        assert!(
            terminal_outcome.payload_json["error"]
                .as_str()
                .expect("cancel error")
                .contains("delegate_cancelled"),
            "expected delegate_cancelled last_error"
        );
    }
}
