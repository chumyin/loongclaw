use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

use loong_kernel as kernel;
use loong_kernel::{
    PluginActivationStatus, PluginBridgeKind, PluginCompatibilityMode, PluginIR, PluginScanReport,
    PluginScanner, PluginSetupReadinessContext, PluginTranslator,
};
use serde_json::{Map, Value};

use crate::CliResult;

#[derive(Debug, Clone)]
pub struct ExtensionRuntimeBinding {
    pub plugin_id: String,
    pub provider: kernel::ProviderConfig,
    pub allowed_process_commands: BTreeSet<String>,
}

pub fn resolve_extension_runtime_bindings(
    config: &crate::config::LoongConfig,
    required_methods: &[&str],
) -> CliResult<Vec<ExtensionRuntimeBinding>> {
    if !config.runtime_plugins.enabled {
        return Ok(Vec::new());
    }

    let roots = config.runtime_plugins.resolved_roots();
    if roots.is_empty() {
        return Ok(Vec::new());
    }

    let scan_report = scan_runtime_plugin_roots(&roots)?;
    let translation = PluginTranslator::new().translate_scan_report(&scan_report);
    let readiness = runtime_plugin_setup_readiness_context(config)?;
    let bridge_matrix = config
        .runtime_plugins
        .resolved_bridge_support_matrix()
        .map_err(|error| format!("resolve runtime plugin bridge matrix failed: {error}"))?;
    let activation =
        PluginTranslator::new().plan_activation(&translation, &bridge_matrix, &readiness);
    let allowed_process_commands = config
        .runtime_plugins
        .normalized_allowed_process_commands()
        .into_iter()
        .collect::<BTreeSet<_>>();

    let mut bindings = Vec::new();
    for plugin in &translation.entries {
        let Some(candidate) =
            activation.candidate_for(plugin.source_path.as_str(), plugin.plugin_id.as_str())
        else {
            continue;
        };
        if candidate.status != PluginActivationStatus::Ready {
            continue;
        }
        if !required_methods
            .iter()
            .all(|required_method| supports_extension_method(plugin, required_method))
        {
            continue;
        }
        let Some(command) = resolved_process_stdio_command(plugin) else {
            continue;
        };
        if !process_command_is_allowed(command.as_str(), &allowed_process_commands) {
            continue;
        }

        bindings.push(ExtensionRuntimeBinding {
            plugin_id: plugin.plugin_id.clone(),
            provider: kernel::ProviderConfig {
                provider_id: plugin.provider_id.clone(),
                connector_name: plugin.connector_name.clone(),
                version: plugin
                    .plugin_version
                    .clone()
                    .or_else(|| plugin.metadata.get("version").cloned())
                    .unwrap_or_else(|| "0.1.0".to_owned()),
                metadata: plugin.metadata.clone(),
            },
            allowed_process_commands: allowed_process_commands.clone(),
        });
    }

    Ok(bindings)
}

pub fn supports_extension_method(plugin: &PluginIR, required_method: &str) -> bool {
    if plugin.compatibility_mode != PluginCompatibilityMode::Native {
        return false;
    }
    if plugin.runtime.bridge_kind != PluginBridgeKind::ProcessStdio {
        return false;
    }
    if plugin
        .metadata
        .get("loong_extension_contract")
        .map(|value| value.trim())
        != Some("process_stdio_json_line_v1")
    {
        return false;
    }

    let methods = plugin
        .metadata
        .get("loong_extension_methods_json")
        .and_then(|raw| serde_json::from_str::<Vec<String>>(raw).ok());
    methods
        .map(|methods| methods.iter().any(|method| method == required_method))
        .unwrap_or(true)
}

pub fn resolved_process_stdio_command(plugin: &PluginIR) -> Option<String> {
    let explicit_command = non_empty_metadata_value(&plugin.metadata, "command");
    if explicit_command.is_some() {
        return explicit_command;
    }

    let explicit_entrypoint = non_empty_metadata_value(&plugin.metadata, "entrypoint");
    if explicit_entrypoint.is_some() {
        return explicit_entrypoint;
    }

    let runtime_entrypoint = plugin.runtime.entrypoint_hint.trim();
    if runtime_entrypoint.is_empty() || runtime_entrypoint == "stdin/stdout::invoke" {
        return None;
    }

    Some(runtime_entrypoint.to_owned())
}

pub fn process_command_is_allowed(command: &str, allowed_commands: &BTreeSet<String>) -> bool {
    let trimmed_command = command.trim();
    let normalized_command = trimmed_command.to_ascii_lowercase();
    if allowed_commands.contains(&normalized_command) {
        return true;
    }

    let command_path = std::path::Path::new(trimmed_command);
    let has_path_component = command_path.is_absolute()
        || command_path
            .parent()
            .is_some_and(|parent| !parent.as_os_str().is_empty());
    if has_path_component {
        return false;
    }

    let Some(file_name) = command_path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };

    allowed_commands.contains(&file_name.to_ascii_lowercase())
}

pub fn non_empty_metadata_value(metadata: &BTreeMap<String, String>, key: &str) -> Option<String> {
    let value = metadata.get(key)?;
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(trimmed.to_owned())
}

pub fn string_field(object: &Map<String, Value>, keys: &[&str]) -> Option<String> {
    for key in keys {
        let Some(value) = object.get(*key).and_then(Value::as_str) else {
            continue;
        };
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_owned());
        }
    }

    None
}

fn scan_runtime_plugin_roots(roots: &[PathBuf]) -> CliResult<PluginScanReport> {
    let scanner = PluginScanner::new();
    let mut combined = PluginScanReport::default();

    for root in roots {
        let report = scanner.scan_path(root).map_err(|error| {
            format!("runtime plugin scan failed for {}: {error}", root.display())
        })?;
        combined.scanned_files = combined.scanned_files.saturating_add(report.scanned_files);
        combined.matched_plugins = combined
            .matched_plugins
            .saturating_add(report.matched_plugins);
        combined
            .diagnostic_findings
            .extend(report.diagnostic_findings);
        combined.descriptors.extend(report.descriptors);
    }

    Ok(combined)
}

fn runtime_plugin_setup_readiness_context(
    config: &crate::config::LoongConfig,
) -> CliResult<PluginSetupReadinessContext> {
    let mut verified_env_vars = BTreeSet::new();
    for (key, value) in std::env::vars_os() {
        let value_string = value.to_string_lossy();
        if value_string.trim().is_empty() {
            continue;
        }
        verified_env_vars.insert(key.to_string_lossy().to_string());
    }

    let config_value = serde_json::to_value(config)
        .map_err(|error| format!("serialize config failed: {error}"))?;
    let mut verified_config_keys = BTreeSet::new();
    collect_config_paths(&config_value, None, &mut verified_config_keys);

    Ok(PluginSetupReadinessContext {
        verified_env_vars,
        verified_config_keys,
    })
}

fn collect_config_paths(value: &Value, prefix: Option<&str>, out: &mut BTreeSet<String>) {
    match value {
        Value::Object(map) => {
            for (key, child) in map {
                let next_prefix = match prefix {
                    Some(prefix) => format!("{prefix}.{key}"),
                    None => key.clone(),
                };
                if child.is_null() {
                    continue;
                }

                out.insert(next_prefix.clone());
                collect_config_paths(child, Some(next_prefix.as_str()), out);
            }
        }
        Value::Array(items) => {
            for child in items {
                collect_config_paths(child, prefix, out);
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
}
