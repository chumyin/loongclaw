use std::collections::BTreeMap;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
#[cfg(unix)]
use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::sync::OnceLock;
#[cfg(unix)]
use std::sync::atomic::AtomicU64;
use std::sync::atomic::{AtomicUsize, Ordering};
#[cfg(unix)]
use tokio::sync::Mutex;

use super::*;
use crate::config::{AcpBackendProfilesConfig, AcpConfig, AcpxBackendConfig, LoongConfig};
use crate::test_support::ScopedEnv;

const ACPX_RUNTIME_TEST_TIMEOUT_SECONDS: f64 = 45.0;

#[cfg(unix)]
fn acpx_runtime_test_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

#[cfg(unix)]
async fn lock_acpx_runtime_tests() -> tokio::sync::MutexGuard<'static, ()> {
    acpx_runtime_test_lock().lock().await
}

#[cfg(unix)]
const ACPX_FAKE_RUNTIME_STARTUP_TIMEOUT_MS: u64 = 60_000;

#[cfg(unix)]
fn unique_temp_dir(prefix: &str) -> PathBuf {
    static NEXT_TEMP_DIR_SEED: AtomicU64 = AtomicU64::new(1);
    let seed = NEXT_TEMP_DIR_SEED.fetch_add(1, Ordering::Relaxed);
    let temp_dir = std::env::temp_dir().join(format!(
        "{prefix}-{}-{}-{seed}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    ));
    std::fs::create_dir_all(&temp_dir).expect("create temp dir");
    temp_dir
}

#[cfg(unix)]
fn write_executable_script_atomically(script_path: &Path, contents: &str) -> std::io::Result<()> {
    write_executable_script_atomically_with(script_path, |file| {
        std::io::Write::write_all(file, contents.as_bytes())
    })
}

#[cfg(unix)]
fn write_executable_script_atomically_with<F>(script_path: &Path, writer: F) -> std::io::Result<()>
where
    F: FnOnce(&mut std::fs::File) -> std::io::Result<()>,
{
    static NEXT_STAGING_FILE_SEED: AtomicU64 = AtomicU64::new(1);

    let Some(parent) = script_path.parent() else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!(
                "fake acpx script path `{}` has no parent directory",
                script_path.display()
            ),
        ));
    };
    let Some(file_name) = script_path.file_name().and_then(|name| name.to_str()) else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!(
                "fake acpx script path `{}` has no UTF-8 file name",
                script_path.display()
            ),
        ));
    };

    let seed = NEXT_STAGING_FILE_SEED.fetch_add(1, Ordering::Relaxed);
    let staged_path = parent.join(format!(".{file_name}.{}.{seed}.tmp", std::process::id()));
    let mut staged_file = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&staged_path)?;
    let write_result = writer(&mut staged_file).and_then(|()| staged_file.sync_all());
    drop(staged_file);

    if let Err(error) = write_result {
        let _ = std::fs::remove_file(&staged_path);
        return Err(error);
    }

    let mut permissions = std::fs::metadata(&staged_path)?.permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&staged_path, permissions)?;
    if let Err(error) = std::fs::rename(&staged_path, script_path) {
        let _ = std::fs::remove_file(&staged_path);
        return Err(error);
    }

    Ok(())
}

#[cfg(unix)]
fn write_fake_acpx_script(
    temp_dir: &Path,
    script_name: &str,
    log_path: &Path,
    body: &str,
) -> PathBuf {
    let script_path = temp_dir.join(script_name);
    let script_source = format!(
        "#!/bin/sh\nset -eu\n# Keep test helper scripts stable even when unrelated tests narrow PATH.\nPATH=\"$(command -p getconf PATH 2>/dev/null || printf '%s' '/usr/bin:/bin')\"\nexport PATH\nLOG_PATH=\"{}\"\nprintf '%s\\n' \"$*\" >> \"$LOG_PATH\"\nargs_contain() {{\n  case \"$1\" in\n    *\"$2\"*) return 0 ;;\n    *) return 1 ;;\n  esac\n}}\ndrain_stdin() {{\n  if [ ! -t 0 ]; then\n    cat >/dev/null\n  fi\n}}\n{}\n",
        log_path.display(),
        body
    );
    write_executable_script_atomically(&script_path, &script_source)
        .expect("write fake acpx script");
    script_path
}

#[cfg(unix)]
#[path = "acpx/tests/mcp_proxy_tests.rs"]
mod mcp_proxy_tests;
#[cfg(unix)]
#[path = "acpx/tests/path_tests.rs"]
mod path_tests;

#[test]
#[cfg(unix)]
fn write_executable_script_atomically_preserves_existing_script_when_write_fails() {
    let temp_dir = unique_temp_dir("loong-acpx-script-atomic");
    let script_path = temp_dir.join("fake-acpx");

    write_executable_script_atomically(&script_path, "#!/bin/sh\necho old\n")
        .expect("write baseline fake acpx script");

    let error = write_executable_script_atomically_with(&script_path, |file| {
        std::io::Write::write_all(file, b"#!/bin/sh\necho new\n")?;
        Err(std::io::Error::other("simulated staging failure"))
    })
    .expect_err("staging failure should surface");

    assert_eq!(error.kind(), std::io::ErrorKind::Other);
    assert_eq!(
        std::fs::read_to_string(&script_path).expect("read baseline fake acpx script"),
        "#!/bin/sh\necho old\n"
    );

    let staging_entries = std::fs::read_dir(&temp_dir)
        .expect("list temp dir")
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().contains(".tmp"))
        .count();
    assert_eq!(staging_entries, 0, "staging files should be cleaned up");
}

#[tokio::test]
async fn retry_executable_file_busy_retries_until_success() {
    let attempts = AtomicUsize::new(0);

    let result = retry_executable_file_busy(|| {
        let attempt = attempts.fetch_add(1, Ordering::Relaxed);
        if attempt < 2 {
            Err(std::io::Error::from(ErrorKind::ExecutableFileBusy))
        } else {
            Ok("spawned")
        }
    })
    .await
    .expect("retry should recover once the executable is no longer busy");

    assert_eq!(result, "spawned");
    assert_eq!(attempts.load(Ordering::Relaxed), 3);
}

#[tokio::test]
async fn retry_executable_file_busy_surfaces_non_retryable_error_immediately() {
    let attempts = AtomicUsize::new(0);

    let error = retry_executable_file_busy::<(), _>(|| {
        attempts.fetch_add(1, Ordering::Relaxed);
        Err(std::io::Error::other("boom"))
    })
    .await
    .expect_err("non-retryable spawn errors should surface immediately");

    assert_eq!(error.kind(), ErrorKind::Other);
    assert_eq!(attempts.load(Ordering::Relaxed), 1);
}

#[tokio::test]
async fn retry_executable_file_busy_stops_after_retry_budget() {
    let attempts = AtomicUsize::new(0);

    let error = retry_executable_file_busy::<(), _>(|| {
        attempts.fetch_add(1, Ordering::Relaxed);
        Err(std::io::Error::from(ErrorKind::ExecutableFileBusy))
    })
    .await
    .expect_err("persistent executable-file-busy errors should stop after the retry budget");

    assert_eq!(error.kind(), ErrorKind::ExecutableFileBusy);
    assert_eq!(attempts.load(Ordering::Relaxed), ACPX_SPAWN_RETRY_ATTEMPTS);
}

#[test]
fn retry_executable_file_busy_blocking_retries_until_success() {
    let attempts = AtomicUsize::new(0);

    let result = retry_executable_file_busy_blocking(|| {
        let attempt = attempts.fetch_add(1, Ordering::Relaxed);
        if attempt < 2 {
            Err(std::io::Error::from(ErrorKind::ExecutableFileBusy))
        } else {
            Ok("spawned")
        }
    })
    .expect("retry should recover once the executable is no longer busy");

    assert_eq!(result, "spawned");
    assert_eq!(attempts.load(Ordering::Relaxed), 3);
}

#[test]
fn retry_executable_file_busy_blocking_surfaces_non_retryable_error_immediately() {
    let attempts = AtomicUsize::new(0);

    let error = retry_executable_file_busy_blocking::<(), _>(|| {
        attempts.fetch_add(1, Ordering::Relaxed);
        Err(std::io::Error::other("boom"))
    })
    .expect_err("non-retryable spawn errors should surface immediately");

    assert_eq!(error.kind(), ErrorKind::Other);
    assert_eq!(attempts.load(Ordering::Relaxed), 1);
}

#[test]
fn retry_executable_file_busy_blocking_stops_after_retry_budget() {
    let attempts = AtomicUsize::new(0);

    let error = retry_executable_file_busy_blocking::<(), _>(|| {
        attempts.fetch_add(1, Ordering::Relaxed);
        Err(std::io::Error::from(ErrorKind::ExecutableFileBusy))
    })
    .expect_err("persistent executable-file-busy errors should stop after the retry budget");

    assert_eq!(error.kind(), ErrorKind::ExecutableFileBusy);
    assert_eq!(attempts.load(Ordering::Relaxed), ACPX_SPAWN_RETRY_ATTEMPTS);
}

#[cfg(unix)]
fn fake_acpx_config(script_path: &Path, cwd: &Path) -> LoongConfig {
    let startup_timeout_ms = ACPX_FAKE_RUNTIME_STARTUP_TIMEOUT_MS;

    LoongConfig {
        acp: AcpConfig {
            startup_timeout_ms: Some(startup_timeout_ms),
            allow_mcp_server_injection: false,
            backends: AcpBackendProfilesConfig {
                acpx: Some(AcpxBackendConfig {
                    command: Some(script_path.display().to_string()),
                    expected_version: Some("0.1.16".to_owned()),
                    cwd: Some(cwd.display().to_string()),
                    permission_mode: Some("approve-reads".to_owned()),
                    non_interactive_permissions: Some("fail".to_owned()),
                    timeout_seconds: Some(ACPX_RUNTIME_TEST_TIMEOUT_SECONDS),
                    queue_owner_ttl_seconds: Some(0.25),
                    ..AcpxBackendConfig::default()
                }),
            },
            ..AcpConfig::default()
        },
        ..LoongConfig::default()
    }
}

#[test]
#[cfg(unix)]
fn fake_acpx_config_uses_explicit_process_test_startup_timeout() {
    let temp_dir = unique_temp_dir("loong-acpx-config-timeout");
    let script_path = temp_dir.join("fake-acpx");

    let config = fake_acpx_config(&script_path, &temp_dir);
    let startup_timeout_ms = config.acp.startup_timeout_ms();

    assert_eq!(startup_timeout_ms, ACPX_FAKE_RUNTIME_STARTUP_TIMEOUT_MS);
}

#[tokio::test]
async fn doctor_reports_missing_command() {
    let backend = AcpxCliProbeBackend;
    let config = LoongConfig {
        acp: AcpConfig {
            backends: AcpBackendProfilesConfig {
                acpx: Some(AcpxBackendConfig {
                    command: Some("/definitely/not/a/real/acpx".to_owned()),
                    expected_version: Some("0.1.16".to_owned()),
                    ..AcpxBackendConfig::default()
                }),
            },
            ..AcpConfig::default()
        },
        ..LoongConfig::default()
    };

    let report = backend
        .doctor(&config)
        .await
        .expect("doctor should not fail")
        .expect("doctor report");
    assert!(!report.healthy);
    assert_eq!(
        report.diagnostics.get("status").map(String::as_str),
        Some("missing_command")
    );
}

#[test]
fn metadata_exposes_mcp_server_injection_capability() {
    let metadata = AcpxCliProbeBackend.metadata();

    assert!(
        metadata
            .capabilities
            .contains(&AcpCapability::McpServerInjection)
    );
}

#[test]
fn derive_agent_id_prefers_session_key_prefix() {
    let mut config = LoongConfig::default();
    config.acp.default_agent = Some("codex".to_owned());
    config.acp.allowed_agents = vec!["codex".to_owned(), "claude".to_owned()];
    let metadata = BTreeMap::from([("acp_agent".to_owned(), "claude".to_owned())]);

    let derived =
        derive_agent_id(&config, "agent:claude:session-1", &metadata).expect("derive agent");
    assert_eq!(derived, "claude");
}

#[test]
fn derive_agent_id_uses_configured_default_when_session_has_no_agent_prefix() {
    let mut config = LoongConfig::default();
    config.acp.default_agent = Some("gemini".to_owned());
    config.acp.allowed_agents = vec!["codex".to_owned(), "gemini".to_owned()];

    let derived = derive_agent_id(&config, "telegram:42", &BTreeMap::new()).expect("derive agent");
    assert_eq!(derived, "gemini");
}

#[test]
fn derive_agent_id_rejects_mismatched_metadata_agent() {
    let mut config = LoongConfig::default();
    config.acp.default_agent = Some("codex".to_owned());
    config.acp.allowed_agents = vec!["codex".to_owned(), "claude".to_owned()];
    let metadata = BTreeMap::from([("acp_agent".to_owned(), "codex".to_owned())]);

    let error = derive_agent_id(&config, "agent:claude:session-1", &metadata)
        .expect_err("mismatched ACP agent metadata must fail");
    assert!(error.contains("does not match"));
}

#[tokio::test]
#[cfg(unix)]
async fn doctor_accepts_fake_version_command() {
    let _env = crate::test_support::ScopedEnv::new();
    let temp_dir = unique_temp_dir("loong-acpx-probe");
    let script_path = temp_dir.join("fake-acpx");
    write_executable_script_atomically(&script_path, "#!/bin/sh\necho 'acpx 0.1.16'\n")
        .expect("write fake acpx script");

    let backend = AcpxCliProbeBackend;
    let config = LoongConfig {
        acp: AcpConfig {
            backends: AcpBackendProfilesConfig {
                acpx: Some(AcpxBackendConfig {
                    command: Some(script_path.display().to_string()),
                    expected_version: Some("0.1.16".to_owned()),
                    mcp_servers: BTreeMap::from([(
                        "filesystem".to_owned(),
                        crate::config::AcpxMcpServerConfig {
                            command: "npx".to_owned(),
                            args: vec!["@modelcontextprotocol/server-filesystem".to_owned()],
                            env: BTreeMap::new(),
                        },
                    )]),
                    ..AcpxBackendConfig::default()
                }),
            },
            ..AcpConfig::default()
        },
        ..LoongConfig::default()
    };

    let mut last_report = None;
    for attempt in 0..5 {
        let report = backend
            .doctor(&config)
            .await
            .expect("doctor should not fail")
            .expect("doctor report");
        if report.healthy {
            last_report = Some(report);
            break;
        }
        last_report = Some(report);
        if attempt < 4 {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    }

    let report = last_report.expect("doctor report");
    assert!(
        report.healthy,
        "doctor should accept fake version command: {:?}",
        report.diagnostics
    );
    assert_eq!(
        report.diagnostics.get("command"),
        Some(&script_path.display().to_string())
    );
    assert_eq!(
        report.diagnostics.get("mcp_server_count"),
        Some(&"1".to_owned())
    );
    assert_eq!(
        report.diagnostics.get("mcp_runtime_proxy"),
        Some(&"available_but_disabled_by_policy".to_owned())
    );

    let _ = std::fs::remove_file(&script_path);
    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
#[cfg(unix)]
async fn doctor_accepts_path_discovered_fake_version_command() {
    let _guard = lock_acpx_runtime_tests().await;
    let temp_dir = unique_temp_dir("loong-acpx-probe-path");
    let bin_dir = temp_dir.join("bin");
    let script_path = bin_dir.join("fake-acpx");
    std::fs::create_dir_all(&bin_dir).expect("create bin dir");
    write_executable_script_atomically(&script_path, "#!/bin/sh\necho 'acpx 0.1.16'\n")
        .expect("write fake acpx script");

    let mut env = ScopedEnv::new();
    let original_path = std::env::var_os("PATH").unwrap_or_default();
    let original_entries = std::env::split_paths(&original_path);
    let mut path_entries = vec![bin_dir.clone()];
    path_entries.extend(original_entries);
    let joined_path = std::env::join_paths(path_entries).expect("join PATH");
    env.set("PATH", joined_path);

    let backend = AcpxCliProbeBackend;
    let config = LoongConfig {
        acp: AcpConfig {
            backends: AcpBackendProfilesConfig {
                acpx: Some(AcpxBackendConfig {
                    command: Some("fake-acpx".to_owned()),
                    expected_version: Some("0.1.16".to_owned()),
                    cwd: Some(temp_dir.display().to_string()),
                    ..AcpxBackendConfig::default()
                }),
            },
            ..AcpConfig::default()
        },
        ..LoongConfig::default()
    };

    let report = backend
        .doctor(&config)
        .await
        .expect("doctor should not fail")
        .expect("doctor report");

    assert!(report.healthy, "doctor should use launcher path");
    assert_eq!(
        report.diagnostics.get("command"),
        Some(&"fake-acpx".to_owned())
    );
    assert_eq!(report.diagnostics.get("status"), Some(&"ready".to_owned()));
}

#[tokio::test]
#[cfg(unix)]
#[allow(clippy::await_holding_lock)]
async fn runtime_backend_uses_agent_proxy_when_mcp_servers_requested() {
    let _lock = lock_acpx_runtime_tests().await;
    let _env = crate::test_support::ScopedEnv::new();
    let temp_dir = unique_temp_dir("loong-acpx-mcp-proxy");
    let log_path = temp_dir.join("calls.log");
    let script_path = write_fake_acpx_script(
        &temp_dir,
        "fake-acpx",
        &log_path,
        r#"
case "$*" in
  "--version")
echo 'acpx 0.1.16'
exit 0
;;
esac

case "$*" in
  *"config show"*)
echo '{"agents":{"codex":{"command":"npx @zed-industries/codex-acp"}}}'
exit 0
;;
esac

case "$*" in
  *"sessions ensure --name"*)
echo '{"acpxSessionId":"sess-mcp","agentSessionId":"agent-mcp","acpxRecordId":"record-mcp"}'
exit 0
;;
esac

case "$*" in
  *"prompt --session"*)
drain_stdin
echo '{"type":"text","content":"proxy ok"}'
echo '{"type":"done"}'
exit 0
;;
esac

exit 0
"#,
    );
    let mut config = fake_acpx_config(&script_path, &temp_dir);
    config.acp.allow_mcp_server_injection = true;
    config.acp.backends.acpx = Some(AcpxBackendConfig {
        command: Some(script_path.display().to_string()),
        expected_version: Some("0.1.16".to_owned()),
        cwd: Some(temp_dir.display().to_string()),
        permission_mode: Some("approve-reads".to_owned()),
        non_interactive_permissions: Some("fail".to_owned()),
        timeout_seconds: Some(12.5),
        queue_owner_ttl_seconds: Some(0.25),
        mcp_servers: BTreeMap::from([(
            "filesystem".to_owned(),
            crate::config::AcpxMcpServerConfig {
                command: "npx".to_owned(),
                args: vec![
                    "-y".to_owned(),
                    "@modelcontextprotocol/server-filesystem".to_owned(),
                    temp_dir.display().to_string(),
                ],
                env: BTreeMap::from([("ROOT".to_owned(), temp_dir.display().to_string())]),
            },
        )]),
        ..AcpxBackendConfig::default()
    });

    let backend = AcpxCliProbeBackend;
    let bootstrap = AcpSessionBootstrap {
        session_key: "session-proxy".to_owned(),
        conversation_id: Some("telegram:mcp".to_owned()),
        binding: None,
        working_directory: Some(temp_dir.clone()),
        initial_prompt: None,
        mode: Some(AcpSessionMode::Interactive),
        mcp_servers: vec!["filesystem".to_owned()],
        metadata: BTreeMap::new(),
    };

    let handle = backend
        .ensure_session(&config, &bootstrap)
        .await
        .expect("ensure session with MCP proxy");
    let result = backend
        .run_turn(
            &config,
            &handle,
            &AcpTurnRequest {
                session_key: bootstrap.session_key.clone(),
                input: "test proxy path".to_owned(),
                working_directory: None,
                metadata: BTreeMap::new(),
            },
        )
        .await
        .expect("run proxied turn");
    assert_eq!(result.output_text, "proxy ok");

    let log = std::fs::read_to_string(&log_path).expect("read fake acpx log");
    assert!(
        log.contains("config show"),
        "expected agent override lookup in log: {log}"
    );
    assert!(
        log.contains("--agent"),
        "expected --agent proxy flag in log: {log}"
    );
    assert!(
        log.contains("--payload-file"),
        "expected MCP proxy payload-file flag in log: {log}"
    );
    assert!(
        log.contains("sessions ensure --name session-proxy"),
        "expected ensure command in log: {log}"
    );
    assert!(
        log.contains("prompt --session session-proxy --file -"),
        "expected prompt command in log: {log}"
    );
    assert!(
        !log.contains("codex sessions ensure --name session-proxy"),
        "expected raw agent positional form to be replaced by --agent proxy: {log}"
    );
}

#[tokio::test]
#[cfg(unix)]
async fn ensure_session_rejects_unknown_requested_mcp_server_names() {
    let temp_dir = unique_temp_dir("loong-acpx-mcp-unknown");
    let log_path = temp_dir.join("calls.log");
    let script_path = write_fake_acpx_script(
        &temp_dir,
        "fake-acpx",
        &log_path,
        "echo '{\"acpxSessionId\":\"unused\"}'\n",
    );
    let mut config = fake_acpx_config(&script_path, &temp_dir);
    config.acp.allow_mcp_server_injection = true;
    config.acp.backends.acpx = Some(AcpxBackendConfig {
        mcp_servers: BTreeMap::from([(
            "filesystem".to_owned(),
            crate::config::AcpxMcpServerConfig {
                command: "npx".to_owned(),
                args: vec!["@modelcontextprotocol/server-filesystem".to_owned()],
                env: BTreeMap::new(),
            },
        )]),
        ..AcpxBackendConfig::default()
    });

    let backend = AcpxCliProbeBackend;
    let error = backend
        .ensure_session(
            &config,
            &AcpSessionBootstrap {
                session_key: "session-unknown-mcp".to_owned(),
                conversation_id: None,
                binding: None,
                working_directory: Some(temp_dir),
                initial_prompt: None,
                mode: Some(AcpSessionMode::Interactive),
                mcp_servers: vec!["missing".to_owned()],
                metadata: BTreeMap::new(),
            },
        )
        .await
        .expect_err("unknown MCP server should fail");

    assert!(
        error.contains("missing") && error.contains("mcp_servers"),
        "expected missing MCP server validation error, got: {error}"
    );
}

#[tokio::test]
#[cfg(unix)]
#[allow(clippy::await_holding_lock)]
async fn runtime_backend_executes_session_turn_and_controls() {
    let _lock = lock_acpx_runtime_tests().await;
    let _env = crate::test_support::ScopedEnv::new();
    let temp_dir = unique_temp_dir("loong-acpx-runtime");
    let log_path = temp_dir.join("calls.log");
    let script_path = write_fake_acpx_script(
        &temp_dir,
        "fake-acpx",
        &log_path,
        r#"
case "$*" in
  "--version")
echo 'acpx 0.1.16'
exit 0
;;
esac

case "$*" in
  *"sessions ensure --name"*)
echo '{"acpxSessionId":"sess-42","agentSessionId":"agent-42","acpxRecordId":"record-42"}'
exit 0
;;
esac

case "$*" in
  *"prompt --session"*)
drain_stdin
echo '{"type":"text","content":"hello "}'
echo '{"type":"text","content":"world"}'
echo '{"type":"usage_update","used":7,"size":128}'
echo '{"type":"done"}'
exit 0
;;
esac

case "$*" in
  *"status --session"*)
echo '{"status":"ready","acpxSessionId":"sess-42","agentSessionId":"agent-42","acpxRecordId":"record-42"}'
exit 0
;;
esac

exit 0
"#,
    );
    let config = fake_acpx_config(&script_path, &temp_dir);
    let backend = AcpxCliProbeBackend;
    let bootstrap = AcpSessionBootstrap {
        session_key: "agent:codex:session-42".to_owned(),
        conversation_id: Some("telegram:42".to_owned()),
        binding: Some(crate::acp::AcpSessionBindingScope {
            route_session_id: "telegram:bot_123456:42".to_owned(),
            channel_id: Some("telegram".to_owned()),
            account_id: Some("bot_123456".to_owned()),
            conversation_id: Some("42".to_owned()),
            thread_id: Some("thread-42".to_owned()),
        }),
        working_directory: Some(temp_dir.clone()),
        initial_prompt: None,
        mode: Some(AcpSessionMode::Interactive),
        mcp_servers: Vec::new(),
        metadata: BTreeMap::new(),
    };

    let handle = backend
        .ensure_session(&config, &bootstrap)
        .await
        .expect("ensure session");
    assert_eq!(handle.backend_id, ACPX_BACKEND_ID);
    assert_eq!(handle.backend_session_id.as_deref(), Some("sess-42"));
    assert_eq!(handle.agent_session_id.as_deref(), Some("agent-42"));
    assert_eq!(
        handle.working_directory.as_deref(),
        Some(temp_dir.as_path())
    );

    let result = backend
        .run_turn(
            &config,
            &handle,
            &AcpTurnRequest {
                session_key: bootstrap.session_key.clone(),
                input: "hello runtime".to_owned(),
                working_directory: None,
                metadata: BTreeMap::new(),
            },
        )
        .await
        .expect("run turn");
    assert_eq!(result.output_text, "hello world");
    assert_eq!(result.state, AcpSessionState::Ready);
    assert_eq!(
        result.usage,
        Some(serde_json::json!({
            "used": 7,
            "size": 128,
        }))
    );

    let status = backend
        .get_status(&config, &handle)
        .await
        .expect("status should succeed")
        .expect("status payload");
    assert_eq!(status.session_key, "agent:codex:session-42");
    assert_eq!(status.backend_id, ACPX_BACKEND_ID);
    assert_eq!(
        status
            .binding
            .as_ref()
            .map(|binding| binding.route_session_id.as_str()),
        Some("telegram:bot_123456:42")
    );
    assert_eq!(
        status
            .binding
            .as_ref()
            .and_then(|binding| binding.thread_id.as_deref()),
        Some("thread-42")
    );
    assert_eq!(status.state, AcpSessionState::Ready);
    assert_eq!(status.pending_turns, 0);
    assert!(status.active_turn_id.is_none());

    backend
        .set_mode(&config, &handle, AcpSessionMode::Review)
        .await
        .expect("set mode");
    backend
        .set_config_option(
            &config,
            &handle,
            &AcpConfigPatch {
                key: "temperature".to_owned(),
                value: "0.1".to_owned(),
            },
        )
        .await
        .expect("set config option");
    backend
        .cancel(&config, &handle)
        .await
        .expect("cancel session");
    backend
        .close(&config, &handle)
        .await
        .expect("close session");

    let log = std::fs::read_to_string(&log_path).expect("read fake acpx log");
    assert!(
        log.contains("sessions ensure --name agent:codex:session-42"),
        "expected ensure command in log: {log}"
    );
    assert!(
        log.contains("prompt --session agent:codex:session-42 --file -"),
        "expected prompt command in log: {log}"
    );
    assert!(
        log.contains("--approve-reads"),
        "expected permission mode args in log: {log}"
    );
    assert!(
        log.contains("--non-interactive-permissions fail"),
        "expected non-interactive permissions in log: {log}"
    );
    assert!(log.contains("--ttl 0.25"), "expected ttl in log: {log}");
    assert!(
        log.contains("set-mode review --session agent:codex:session-42"),
        "expected set-mode command in log: {log}"
    );
    assert!(
        log.contains("set temperature 0.1 --session agent:codex:session-42"),
        "expected set command in log: {log}"
    );
    assert!(
        log.contains("cancel --session agent:codex:session-42"),
        "expected cancel command in log: {log}"
    );
    assert!(
        log.contains("sessions close agent:codex:session-42"),
        "expected close command in log: {log}"
    );
}

#[tokio::test]
#[cfg(unix)]
#[allow(clippy::await_holding_lock)]
async fn runtime_backend_supports_local_abort_for_running_prompt() {
    let _lock = lock_acpx_runtime_tests().await;
    let _env = crate::test_support::ScopedEnv::new();
    let temp_dir = unique_temp_dir("loong-acpx-abort");
    let log_path = temp_dir.join("calls.log");
    let script_path = write_fake_acpx_script(
        &temp_dir,
        "fake-acpx",
        &log_path,
        r#"
case "$*" in
  "--version")
echo 'acpx 0.1.16'
exit 0
;;
esac

case "$*" in
  *"sessions ensure --name"*)
echo '{"acpxSessionId":"sess-abort","agentSessionId":"agent-abort","acpxRecordId":"record-abort"}'
exit 0
;;
esac

case "$*" in
  *"prompt --session"*)
drain_stdin
/bin/sleep 30
exit 0
;;
esac

exit 0
"#,
    );
    let config = fake_acpx_config(&script_path, &temp_dir);
    let backend = AcpxCliProbeBackend;
    let bootstrap = AcpSessionBootstrap {
        session_key: "agent:codex:session-abort".to_owned(),
        conversation_id: Some("telegram:abort".to_owned()),
        binding: None,
        working_directory: Some(temp_dir.clone()),
        initial_prompt: None,
        mode: Some(AcpSessionMode::Interactive),
        mcp_servers: Vec::new(),
        metadata: BTreeMap::new(),
    };
    let handle = backend
        .ensure_session(&config, &bootstrap)
        .await
        .expect("ensure abortable session");

    let abort_controller = crate::acp::AcpAbortController::new();
    let abort_signal = abort_controller.signal();
    let turn_task = {
        let backend = AcpxCliProbeBackend;
        let config = config.clone();
        let handle = handle.clone();
        let session_key = bootstrap.session_key.clone();
        tokio::spawn(async move {
            backend
                .run_turn_with_sink(
                    &config,
                    &handle,
                    &AcpTurnRequest {
                        session_key,
                        input: "abort me".to_owned(),
                        working_directory: None,
                        metadata: BTreeMap::new(),
                    },
                    Some(abort_signal),
                    None,
                )
                .await
        })
    };

    tokio::time::sleep(Duration::from_millis(150)).await;
    abort_controller.abort();

    let result = tokio::time::timeout(Duration::from_secs(2), async {
        turn_task
            .await
            .expect("abortable turn join should succeed")
            .expect("abortable turn result should resolve")
    })
    .await
    .expect("aborted prompt should stop promptly");

    assert_eq!(result.state, AcpSessionState::Ready);
    assert_eq!(result.stop_reason, Some(AcpTurnStopReason::Cancelled));
    assert_eq!(result.output_text, "");
    assert_eq!(
        result
            .events
            .last()
            .and_then(|event| value_string(event, "stopReason")),
        Some("cancelled".to_owned())
    );
}

#[tokio::test]
#[cfg(unix)]
#[allow(clippy::await_holding_lock)]
async fn ensure_session_falls_back_to_sessions_new_when_ensure_has_no_identifiers() {
    let _lock = lock_acpx_runtime_tests().await;
    let _env = crate::test_support::ScopedEnv::new();
    let temp_dir = unique_temp_dir("loong-acpx-fallback");
    let log_path = temp_dir.join("calls.log");
    let script_path = write_fake_acpx_script(
        &temp_dir,
        "fake-acpx",
        &log_path,
        r#"
case "$*" in
  "--version")
echo 'acpx 0.1.16'
exit 0
;;
esac

case "$*" in
  *"sessions ensure --name"*)
echo '{}'
exit 0
;;
esac

case "$*" in
  *"sessions new --name"*)
echo '{"acpxSessionId":"sess-fallback","agentSessionId":"agent-fallback","acpxRecordId":"record-fallback"}'
exit 0
;;
esac

exit 0
"#,
    );
    let config = fake_acpx_config(&script_path, &temp_dir);
    let backend = AcpxCliProbeBackend;

    let handle = backend
        .ensure_session(
            &config,
            &AcpSessionBootstrap {
                session_key: "session-fallback".to_owned(),
                conversation_id: None,
                binding: None,
                working_directory: Some(temp_dir.clone()),
                initial_prompt: None,
                mode: Some(AcpSessionMode::Interactive),
                mcp_servers: Vec::new(),
                metadata: BTreeMap::new(),
            },
        )
        .await
        .expect("fallback ensure");

    assert_eq!(handle.backend_session_id.as_deref(), Some("sess-fallback"));
    assert_eq!(handle.agent_session_id.as_deref(), Some("agent-fallback"));
    let log = std::fs::read_to_string(&log_path).expect("read fake acpx log");
    assert!(
        log.contains("sessions ensure --name session-fallback"),
        "expected ensure attempt in log: {log}"
    );
    assert!(
        log.contains("sessions new --name session-fallback"),
        "expected fallback sessions new in log: {log}"
    );
}
