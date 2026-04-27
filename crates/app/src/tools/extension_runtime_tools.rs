use std::io::Write;
use std::process::{Command, Stdio};
use std::time::Duration;

use loong_kernel as kernel;
use serde_json::{Map, Value, json};
use wait_timeout::ChildExt;

use crate::extension_runtime_support::{
    ExtensionRuntimeBinding, process_command_is_allowed, resolve_extension_runtime_bindings,
    string_field,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ExtensionRuntimeUiTone {
    Info,
    Success,
    Warning,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ExtensionRuntimeUiAction {
    Notify {
        tone: ExtensionRuntimeUiTone,
        title: Option<String>,
        lines: Vec<String>,
        footer_lines: Vec<String>,
    },
    Overlay {
        title: String,
        lines: Vec<String>,
        footer_lines: Vec<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExtensionRuntimeCommandOutcome {
    pub text: Option<String>,
    pub ui_action: Option<ExtensionRuntimeUiAction>,
}

#[derive(Debug, Clone)]
struct ExtensionRuntimeCommandDefinition {
    plugin_id: String,
    command_name: String,
    description: String,
}

pub(crate) fn maybe_execute_extension_runtime_command(
    input: &str,
    session_id: &str,
    app_config: &crate::config::LoongConfig,
) -> Result<Option<ExtensionRuntimeCommandOutcome>, String> {
    let trimmed = input.trim();
    if !trimmed.starts_with('/') {
        return Ok(None);
    }

    let command_body = trimmed.trim_start_matches('/').trim();
    if command_body.is_empty() {
        return Ok(None);
    }
    let mut parts = command_body.splitn(2, char::is_whitespace);
    let command_name = parts.next().unwrap_or_default().trim();
    if command_name.is_empty() {
        return Ok(None);
    }
    let raw_args = parts.next().map(str::trim).unwrap_or("");

    let bindings = resolve_extension_runtime_bindings(
        app_config,
        &["extension/resource", "extension/command"],
    )?;
    if bindings.is_empty() {
        return Ok(None);
    }

    let mut matches = Vec::new();
    for binding in bindings {
        let commands = discover_extension_command_definitions(&binding)?;
        for command in commands {
            if command.command_name == command_name {
                matches.push((binding.clone(), command));
            }
        }
    }

    if matches.is_empty() {
        return Ok(None);
    }
    if matches.len() > 1 {
        let owners = matches
            .into_iter()
            .map(|(_, command)| format!("{} ({})", command.plugin_id, command.description))
            .collect::<Vec<_>>()
            .join(", ");
        return Err(format!(
            "extension_command_conflict: command `/{command_name}` is declared by multiple extensions: {owners}"
        ));
    }

    let Some((binding, command)) = matches.pop() else {
        return Ok(None);
    };
    let payload = execute_extension_frame(
        &binding.provider,
        &binding.allowed_process_commands,
        "extension/command",
        json!({
            "command_name": command.command_name,
            "session_id": session_id,
            "raw_args": raw_args,
            "argv": if raw_args.is_empty() {
                Vec::<String>::new()
            } else {
                raw_args.split_whitespace().map(str::to_owned).collect::<Vec<_>>()
            },
        }),
    )?;

    Ok(Some(ExtensionRuntimeCommandOutcome {
        text: render_extension_command_payload(&payload),
        ui_action: extract_extension_command_ui_action(&payload),
    }))
}

pub(crate) fn list_extension_runtime_commands(
    app_config: &crate::config::LoongConfig,
) -> Result<Vec<(String, String)>, String> {
    let bindings = resolve_extension_runtime_bindings(
        app_config,
        &["extension/resource", "extension/command"],
    )?;
    let mut commands = Vec::new();
    for binding in bindings {
        let discovered = discover_extension_command_definitions(&binding)?;
        for command in discovered {
            commands.push((format!("/{}", command.command_name), command.description));
        }
    }
    commands.sort_by(|a, b| a.0.cmp(&b.0));
    commands.dedup_by(|a, b| a.0 == b.0 && a.1 == b.1);
    Ok(commands)
}

pub(crate) fn render_extension_command_outcome_text(
    outcome: &ExtensionRuntimeCommandOutcome,
) -> String {
    if let Some(text) = outcome.text.as_deref() {
        return text.to_owned();
    }

    if let Some(ui_action) = outcome.ui_action.as_ref() {
        return match ui_action {
            ExtensionRuntimeUiAction::Notify { title, lines, .. } => {
                let mut parts = Vec::new();
                if let Some(title) = title.as_deref() {
                    parts.push(title.to_owned());
                }
                parts.extend(lines.iter().cloned());
                parts.join("\n")
            }
            ExtensionRuntimeUiAction::Overlay { title, lines, .. } => {
                let mut parts = vec![format!("Opened extension panel: {title}")];
                parts.extend(lines.iter().cloned());
                parts.join("\n")
            }
        };
    }

    String::new()
}

fn discover_extension_command_definitions(
    binding: &ExtensionRuntimeBinding,
) -> Result<Vec<ExtensionRuntimeCommandDefinition>, String> {
    let payload = execute_extension_frame(
        &binding.provider,
        &binding.allowed_process_commands,
        "extension/resource",
        json!({
            "resource": "commands",
            "kind": "commands",
        }),
    )?;
    Ok(parse_extension_command_definitions(
        binding.plugin_id.as_str(),
        &payload,
    ))
}

fn parse_extension_command_definitions(
    plugin_id: &str,
    payload: &Value,
) -> Vec<ExtensionRuntimeCommandDefinition> {
    let direct_commands = payload.get("commands").and_then(Value::as_array);
    let nested_commands = payload
        .get("result")
        .and_then(Value::as_object)
        .and_then(|value| value.get("commands"))
        .and_then(Value::as_array);
    let commands = direct_commands
        .or(nested_commands)
        .cloned()
        .unwrap_or_default();

    let mut definitions = Vec::new();
    for command in commands {
        let Some(object) = command.as_object() else {
            continue;
        };
        let Some(command_name) = string_field(object, &["name", "command_name"]) else {
            continue;
        };
        let command_name = command_name.trim_start_matches('/').to_owned();
        if command_name.is_empty() {
            continue;
        }
        let description = string_field(object, &["description", "summary"])
            .unwrap_or_else(|| format!("Extension command `/{command_name}` from `{plugin_id}`"));

        definitions.push(ExtensionRuntimeCommandDefinition {
            plugin_id: plugin_id.to_owned(),
            command_name,
            description,
        });
    }

    definitions
}

fn execute_extension_frame(
    provider: &kernel::ProviderConfig,
    allowed_process_commands: &std::collections::BTreeSet<String>,
    method: &str,
    payload: Value,
) -> Result<Value, String> {
    let command = provider
        .metadata
        .get("command")
        .map(String::as_str)
        .or_else(|| provider.metadata.get("entrypoint").map(String::as_str))
        .ok_or_else(|| {
            "extension_runtime_process_missing_command: provider metadata.command or entrypoint is required"
                .to_owned()
        })?;
    if !process_command_is_allowed(command, allowed_process_commands) {
        return Err(format!(
            "extension_runtime_process_not_allowlisted: command `{command}` is not allowed"
        ));
    }

    let args = parse_process_args(provider);
    let timeout_seconds = parse_process_timeout_seconds(provider);
    let request = json!({
        "method": method,
        "id": null,
        "payload": payload,
    });
    let encoded_request = serde_json::to_vec(&request)
        .map_err(|error| format!("extension_runtime_request_encode_failed: {error}"))?;

    let mut child = Command::new(command)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("extension_runtime_spawn_failed: {error}"))?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(&encoded_request)
            .map_err(|error| format!("extension_runtime_stdin_write_failed: {error}"))?;
        stdin
            .write_all(b"\n")
            .map_err(|error| format!("extension_runtime_stdin_write_failed: {error}"))?;
        stdin
            .flush()
            .map_err(|error| format!("extension_runtime_stdin_write_failed: {error}"))?;
    }

    let timeout = Duration::from_secs(timeout_seconds.max(1));
    let output = match child.wait_timeout(timeout) {
        Ok(Some(_status)) => child
            .wait_with_output()
            .map_err(|error| format!("extension_runtime_wait_failed: {error}"))?,
        Ok(None) => {
            let _ = child.kill();
            let output = child
                .wait_with_output()
                .map_err(|error| format!("extension_runtime_wait_failed: {error}"))?;
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
            return Err(format!(
                "extension_runtime_timeout: method `{method}` exceeded {timeout_seconds}s stderr={stderr}"
            ));
        }
        Err(error) => return Err(format!("extension_runtime_wait_failed: {error}")),
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        return Err(format!(
            "extension_runtime_process_failed: method `{method}` status={} stderr={stderr}",
            output.status
        ));
    }

    let stdout = String::from_utf8(output.stdout)
        .map_err(|error| format!("extension_runtime_stdout_utf8_failed: {error}"))?;
    let response_line = stdout
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .ok_or_else(|| "extension_runtime_empty_stdout".to_owned())?;
    let response = serde_json::from_str::<Value>(response_line)
        .map_err(|error| format!("extension_runtime_invalid_json: {error}"))?;
    let response_method = response
        .get("method")
        .and_then(Value::as_str)
        .ok_or_else(|| "extension_runtime_missing_method".to_owned())?;
    if response_method != method {
        return Err(format!(
            "extension_runtime_method_mismatch: expected `{method}`, got `{response_method}`"
        ));
    }

    Ok(response.get("payload").cloned().unwrap_or(Value::Null))
}

fn render_extension_command_payload(payload: &Value) -> Option<String> {
    if let Some(text) = payload
        .as_object()
        .and_then(|value| string_field(value, &["text", "message", "output"]))
    {
        return Some(text);
    }
    if let Some(text) = payload
        .get("result")
        .and_then(Value::as_object)
        .and_then(|value| string_field(value, &["text", "message", "output"]))
    {
        return Some(text);
    }

    None
}

fn extract_extension_command_ui_action(payload: &Value) -> Option<ExtensionRuntimeUiAction> {
    let ui = payload
        .as_object()
        .and_then(|value| value.get("ui"))
        .or_else(|| {
            payload
                .get("result")
                .and_then(Value::as_object)
                .and_then(|value| value.get("ui"))
        })?;
    let object = ui.as_object()?;
    let kind = string_field(object, &["kind"]).unwrap_or_else(|| "notify".to_owned());
    let footer_lines = string_list_field(object, "footer_lines");
    let lines = {
        let listed = string_list_field(object, "lines");
        if listed.is_empty() {
            string_field(object, &["text", "message"])
                .map(|value| vec![value])
                .unwrap_or_default()
        } else {
            listed
        }
    };

    match kind.as_str() {
        "notify" => Some(ExtensionRuntimeUiAction::Notify {
            tone: parse_extension_ui_tone(string_field(object, &["tone"]).as_deref()),
            title: string_field(object, &["title"]),
            lines,
            footer_lines,
        }),
        "overlay" | "panel" => Some(ExtensionRuntimeUiAction::Overlay {
            title: string_field(object, &["title"]).unwrap_or_else(|| "extension".to_owned()),
            lines,
            footer_lines,
        }),
        _ => None,
    }
}

fn parse_extension_ui_tone(raw: Option<&str>) -> ExtensionRuntimeUiTone {
    match raw.unwrap_or("info").trim().to_ascii_lowercase().as_str() {
        "success" => ExtensionRuntimeUiTone::Success,
        "warning" => ExtensionRuntimeUiTone::Warning,
        "error" => ExtensionRuntimeUiTone::Error,
        _ => ExtensionRuntimeUiTone::Info,
    }
}

fn parse_process_args(provider: &kernel::ProviderConfig) -> Vec<String> {
    if let Some(args_json) = provider.metadata.get("args_json")
        && let Ok(args) = serde_json::from_str::<Vec<String>>(args_json)
    {
        return args;
    }

    provider
        .metadata
        .get("args")
        .map(|args: &String| args.split_whitespace().map(str::to_owned).collect())
        .unwrap_or_default()
}

fn parse_process_timeout_seconds(provider: &kernel::ProviderConfig) -> u64 {
    provider
        .metadata
        .get("timeout_ms")
        .and_then(|value: &String| value.parse::<u64>().ok())
        .map(|value| value / 1_000)
        .filter(|value| *value > 0)
        .unwrap_or(10)
}

fn string_list_field(object: &Map<String, Value>, key: &str) -> Vec<String> {
    object
        .get(key)
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use tempfile::TempDir;

    use super::*;

    fn write_runtime_plugin_config(
        root: &std::path::Path,
    ) -> (PathBuf, crate::config::LoongConfig) {
        let config_path = root.join("loong.toml");
        let raw = format!(
            r#"
[runtime_plugins]
enabled = true
roots = ["{}"]
supported_bridges = ["process_stdio"]
allowed_process_commands = ["python3"]
"#,
            root.display()
        );
        std::fs::write(&config_path, raw).expect("write config");
        let (_resolved, config) =
            crate::config::load(Some(config_path.display().to_string().as_str()))
                .expect("load config");
        (config_path, config)
    }

    fn write_extension_command_manifest_package(
        package_root: &std::path::Path,
        plugin_id: &str,
        methods_json: &str,
    ) {
        let script_path = package_root.join("index.py");
        let args_json = serde_json::to_string(&vec![script_path.display().to_string()])
            .expect("serialize args");
        let manifest = json!({
            "api_version": "v1alpha1",
            "version": "0.1.0",
            "plugin_id": plugin_id,
            "provider_id": plugin_id,
            "connector_name": plugin_id,
            "capabilities": ["InvokeConnector"],
            "metadata": {
                "bridge_kind": "process_stdio",
                "source_language": "python",
                "entrypoint": script_path.display().to_string(),
                "command": "python3",
                "args_json": args_json,
                "loong_extension_contract": "process_stdio_json_line_v1",
                "loong_extension_methods_json": methods_json
            },
            "summary": "Extension command package"
        });
        std::fs::write(
            package_root.join("loong.plugin.json"),
            serde_json::to_string_pretty(&manifest).expect("serialize manifest"),
        )
        .expect("write manifest");
    }

    #[test]
    fn maybe_execute_extension_runtime_command_executes_extension_command() {
        let temp = TempDir::new().expect("temp dir");
        let package_root = temp.path().join("commands-extension");
        std::fs::create_dir_all(&package_root).expect("package root");
        let script_path = package_root.join("index.py");
        let script = r#"#!/usr/bin/env python3
import json
import sys

for line in sys.stdin:
    frame = json.loads(line)
    method = frame.get("method")
    payload = frame.get("payload") or {}
    if method == "extension/resource":
        payload = {
            "commands": [
                {
                    "name": "hello-ext",
                    "description": "Say hello from the extension"
                }
            ]
        }
    elif method == "extension/command":
        payload = {
            "text": f"hello extension {payload.get('raw_args')}"
        }
    print(json.dumps({"method": method, "id": None, "payload": payload}), flush=True)
    break
"#;
        std::fs::write(&script_path, script).expect("write extension script");
        write_extension_command_manifest_package(
            &package_root,
            "commands-extension",
            "[\"extension/resource\",\"extension/command\"]",
        );
        let (_config_path, config) = write_runtime_plugin_config(temp.path());

        let outcome =
            maybe_execute_extension_runtime_command("/hello-ext world", "session-1", &config)
                .expect("execute extension command")
                .expect("expected extension command outcome");

        assert_eq!(outcome.text.as_deref(), Some("hello extension world"));
        assert!(outcome.ui_action.is_none());
    }

    #[test]
    fn maybe_execute_extension_runtime_command_extracts_ui_notify_action() {
        let temp = TempDir::new().expect("temp dir");
        let package_root = temp.path().join("notify-extension");
        std::fs::create_dir_all(&package_root).expect("package root");
        let script_path = package_root.join("index.py");
        let script = r#"#!/usr/bin/env python3
import json
import sys

for line in sys.stdin:
    frame = json.loads(line)
    method = frame.get("method")
    payload = {}
    if method == "extension/resource":
        payload = {
            "commands": [
                {
                    "name": "notify-ext",
                    "description": "Notify from extension"
                }
            ]
        }
    elif method == "extension/command":
        payload = {
            "ui": {
                "kind": "notify",
                "tone": "success",
                "title": "extension ready",
                "lines": ["hello from extension ui"]
            }
        }
    print(json.dumps({"method": method, "id": None, "payload": payload}), flush=True)
    break
"#;
        std::fs::write(&script_path, script).expect("write extension script");
        write_extension_command_manifest_package(
            &package_root,
            "notify-extension",
            "[\"extension/resource\",\"extension/command\"]",
        );
        let (_config_path, config) = write_runtime_plugin_config(temp.path());

        let outcome = maybe_execute_extension_runtime_command("/notify-ext", "session-1", &config)
            .expect("execute extension command")
            .expect("expected extension command outcome");

        assert!(outcome.text.is_none());
        assert_eq!(
            outcome.ui_action,
            Some(ExtensionRuntimeUiAction::Notify {
                tone: ExtensionRuntimeUiTone::Success,
                title: Some("extension ready".to_owned()),
                lines: vec!["hello from extension ui".to_owned()],
                footer_lines: Vec::new(),
            })
        );
    }
}
