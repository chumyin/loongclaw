use serde::Serialize;

use crate::{CliResult, mvp};

pub const CHANNEL_RESOLVE_JSON_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize)]
pub struct ChannelResolveOutput {
    pub schema_version: u32,
    pub config: String,
    pub input: String,
    pub resolution: ChannelResolveReadModel,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChannelCatalogResolutionDetails {
    pub canonical_channel_id: String,
    pub catalog: mvp::channel::ChannelCatalogEntry,
    pub surface: Option<mvp::channel::ChannelSurface>,
    pub access_policies: Vec<mvp::channel::ChannelConfiguredAccountAccessPolicy>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChannelSessionResolutionDetails {
    pub route_session_id: String,
    pub target: mvp::channel::ResolvedKnownChannelSessionTarget,
    pub surface: Option<mvp::channel::ChannelSurface>,
    pub matched_configured_account_id: Option<String>,
    pub matched_account: Option<mvp::channel::ChannelStatusSnapshot>,
    pub matched_access_policy: Option<mvp::channel::ChannelConfiguredAccountAccessPolicy>,
    pub pairing_resolution: Option<mvp::channel::ChannelPairingResolution>,
    pub send_operation: Option<mvp::channel::ChannelOperationStatus>,
    pub serve_operation: Option<mvp::channel::ChannelOperationStatus>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ChannelResolveReadModel {
    Catalog(Box<ChannelCatalogResolutionDetails>),
    Session(Box<ChannelSessionResolutionDetails>),
}

pub fn build_channel_resolution(
    config_path: &str,
    config: &mvp::config::LoongClawConfig,
    inventory: &mvp::channel::ChannelInventory,
    input: &str,
) -> CliResult<ChannelResolveOutput> {
    let trimmed_input = input.trim();
    if trimmed_input.is_empty() {
        return Err("channels --resolve requires a non-empty query".to_owned());
    }

    if let Ok(target) = mvp::channel::resolve_known_channel_session_target(config, trimmed_input) {
        let surface = inventory
            .channel_surfaces
            .iter()
            .find(|surface| surface.catalog.id == target.channel_id)
            .cloned();
        let matched_configured_account_id =
            matched_configured_account_id_for_target(config, &target)?;
        let matched_account = surface.as_ref().and_then(|surface| {
            matched_configured_account_id
                .as_deref()
                .and_then(|configured_account_id| {
                    surface
                        .configured_accounts
                        .iter()
                        .find(|snapshot| snapshot.configured_account_id == configured_account_id)
                        .cloned()
                })
        });
        let matched_access_policy =
            matched_configured_account_id
                .as_deref()
                .and_then(|configured_account_id| {
                    inventory
                        .channel_access_policies
                        .iter()
                        .find(|policy| {
                            policy.channel_id == target.channel_id
                                && policy.configured_account_id == configured_account_id
                        })
                        .cloned()
                });
        let send_operation = matched_account
            .as_ref()
            .and_then(|account| account.operation(mvp::channel::CHANNEL_OPERATION_SEND_ID))
            .cloned();
        let serve_operation = matched_account
            .as_ref()
            .and_then(|account| account.operation(mvp::channel::CHANNEL_OPERATION_SERVE_ID))
            .cloned();
        let pairing_resolution =
            resolve_channel_pairing_resolution(config, &target, matched_access_policy.as_ref())?;

        return Ok(ChannelResolveOutput {
            schema_version: CHANNEL_RESOLVE_JSON_SCHEMA_VERSION,
            config: config_path.to_owned(),
            input: trimmed_input.to_owned(),
            resolution: ChannelResolveReadModel::Session(Box::new(
                ChannelSessionResolutionDetails {
                    route_session_id: trimmed_input.to_owned(),
                    target,
                    surface,
                    matched_configured_account_id,
                    matched_account,
                    matched_access_policy,
                    pairing_resolution,
                    send_operation,
                    serve_operation,
                },
            )),
        });
    }

    let catalog = mvp::channel::resolve_channel_catalog_entry(trimmed_input)
        .ok_or_else(|| format!("unknown channel or route-session `{trimmed_input}`"))?;
    let canonical_channel_id = catalog.id.to_owned();
    let surface = inventory
        .channel_surfaces
        .iter()
        .find(|surface| surface.catalog.id == catalog.id)
        .cloned();
    let access_policies = inventory
        .channel_access_policies
        .iter()
        .filter(|policy| policy.channel_id == catalog.id)
        .cloned()
        .collect::<Vec<_>>();

    Ok(ChannelResolveOutput {
        schema_version: CHANNEL_RESOLVE_JSON_SCHEMA_VERSION,
        config: config_path.to_owned(),
        input: trimmed_input.to_owned(),
        resolution: ChannelResolveReadModel::Catalog(Box::new(ChannelCatalogResolutionDetails {
            canonical_channel_id,
            catalog,
            surface,
            access_policies,
        })),
    })
}

pub fn render_channel_resolution_text(resolution: &ChannelResolveOutput) -> String {
    let mut lines = vec![
        format!("schema_version={}", resolution.schema_version),
        format!("config={}", resolution.config),
        format!("input={}", resolution.input),
    ];

    match &resolution.resolution {
        ChannelResolveReadModel::Catalog(details) => {
            let canonical_channel_id = details.canonical_channel_id.as_str();
            let catalog = &details.catalog;
            let surface = details.surface.as_ref();
            let access_policies = &details.access_policies;
            lines.push("resolve_kind=catalog".to_owned());
            lines.push(format!("channel_id={canonical_channel_id}"));
            lines.push(format!("label={}", catalog.label));
            lines.push(format!(
                "aliases={}",
                if catalog.aliases.is_empty() {
                    "-".to_owned()
                } else {
                    catalog.aliases.join(",")
                }
            ));
            lines.push(format!(
                "implementation_status={}",
                catalog.implementation_status.as_str()
            ));
            lines.push(format!("transport={}", catalog.transport));
            lines.push(format!("selection_order={}", catalog.selection_order));
            lines.push(format!("selection_label={}", catalog.selection_label));
            lines.push(format!(
                "supported_target_kinds={}",
                crate::render_channel_target_kind_ids(catalog.supported_target_kinds.as_slice())
            ));
            if let Some(surface) = surface {
                lines.push(format!(
                    "configured_accounts={} default_configured_account={}",
                    surface.configured_accounts.len(),
                    surface
                        .default_configured_account_id
                        .as_deref()
                        .unwrap_or("-")
                ));
                if let Some(discovery) = surface.plugin_bridge_discovery.as_ref() {
                    lines.push(format!(
                        "plugin_bridge_discovery_status={}",
                        discovery.status.as_str()
                    ));
                    lines.push(format!(
                        "plugin_bridge_selected_plugin={}",
                        discovery.selected_plugin_id.as_deref().unwrap_or("-")
                    ));
                }
            }
            if let Some(contract) = catalog.plugin_bridge_contract.as_ref() {
                lines.push(format!(
                    "plugin_bridge_runtime_owner={}",
                    contract.runtime_owner
                ));
                lines.push(format!(
                    "plugin_bridge_required_setup_surface={}",
                    contract.required_setup_surface
                ));
                for stable_target in &contract.stable_targets {
                    lines.push(format!(
                        "stable_target template={} target_kind={} description={}",
                        stable_target.template,
                        stable_target.target_kind.as_str(),
                        stable_target.description,
                    ));
                }
            }
            for access_policy in access_policies {
                lines.push(render_access_policy_resolution_line(access_policy));
            }
            for operation in &catalog.operations {
                lines.push(format!(
                    "operation id={} command={} availability={} tracks_runtime={} default_target_kind={} supported_target_kinds={}",
                    operation.id,
                    operation.command,
                    operation.availability.as_str(),
                    operation.tracks_runtime,
                    operation
                        .default_target_kind()
                        .map(mvp::channel::ChannelCatalogTargetKind::as_str)
                        .unwrap_or("-"),
                    crate::render_channel_target_kind_ids(operation.supported_target_kinds),
                ));
            }
        }
        ChannelResolveReadModel::Session(details) => {
            let route_session_id = details.route_session_id.as_str();
            let target = &details.target;
            let surface = details.surface.as_ref();
            let matched_configured_account_id = details.matched_configured_account_id.as_ref();
            let matched_account = details.matched_account.as_ref();
            let matched_access_policy = details.matched_access_policy.as_ref();
            let pairing_resolution = details.pairing_resolution.as_ref();
            let send_operation = details.send_operation.as_ref();
            let serve_operation = details.serve_operation.as_ref();
            lines.push("resolve_kind=session".to_owned());
            lines.push(format!("route_session_id={route_session_id}"));
            lines.push(format!("channel_id={}", target.channel_id));
            lines.push(format!("session_shape={}", target.session_shape));
            lines.push(format!("target_kind={}", target.target_kind.as_str()));
            lines.push(format!("target_id={}", target.target_id));
            lines.push(format!(
                "account_id={}",
                target.account_id.as_deref().unwrap_or("-")
            ));
            lines.push(format!(
                "matched_configured_account={}",
                matched_configured_account_id
                    .map(String::as_str)
                    .unwrap_or("-")
            ));
            lines.push(format!(
                "conversation_id={}",
                target.conversation_id.as_deref().unwrap_or("-")
            ));
            lines.push(format!(
                "participant_id={}",
                target.participant_id.as_deref().unwrap_or("-")
            ));
            lines.push(format!(
                "thread_id={}",
                target.thread_id.as_deref().unwrap_or("-")
            ));
            lines.push(format!(
                "reply_message_id={}",
                target.reply_message_id.as_deref().unwrap_or("-")
            ));
            lines.push(format!(
                "chat_type={}",
                target
                    .chat_type
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "-".to_owned())
            ));
            lines.push(format!(
                "raw_scope={}",
                if target.raw_scope.is_empty() {
                    "-".to_owned()
                } else {
                    target.raw_scope.join(":")
                }
            ));
            if let Some(surface) = surface {
                lines.push(format!("label={}", surface.catalog.label));
                lines.push(format!(
                    "implementation_status={}",
                    surface.catalog.implementation_status.as_str()
                ));
            }
            if let Some(matched_account) = matched_account {
                lines.push(format!(
                    "matched_account_enabled={} api_base_url={}",
                    matched_account.enabled,
                    matched_account.api_base_url.as_deref().unwrap_or("-")
                ));
            }
            if let Some(matched_access_policy) = matched_access_policy {
                lines.push(render_access_policy_resolution_line(matched_access_policy));
            }
            if let Some(pairing_resolution) = pairing_resolution {
                lines.push(format!(
                    "pairing_mode={} pairing_state={} pairing_request_id={} pairing_code={} pairing_code_expires_at_ms={} pairing_binding_id={}",
                    pairing_resolution.mode.as_str(),
                    pairing_resolution.state.as_str(),
                    pairing_resolution
                        .pairing_request_id
                        .as_deref()
                        .unwrap_or("-"),
                    pairing_resolution
                        .pairing_code
                        .as_deref()
                        .unwrap_or("-"),
                    pairing_resolution
                        .pairing_code_expires_at_ms
                        .map(|value| value.to_string())
                        .unwrap_or_else(|| "-".to_owned()),
                    pairing_resolution.binding_id.as_deref().unwrap_or("-"),
                ));
            }
            if let Some(send_operation) = send_operation {
                lines.push(format!(
                    "send_health={} send_command={} send_detail={}",
                    send_operation.health.as_str(),
                    send_operation.command,
                    send_operation.detail,
                ));
            }
            if let Some(serve_operation) = serve_operation {
                lines.push(format!(
                    "serve_health={} serve_command={} serve_detail={}",
                    serve_operation.health.as_str(),
                    serve_operation.command,
                    serve_operation.detail,
                ));
            }
        }
    }

    lines.join("\n")
}

fn resolve_channel_pairing_resolution(
    config: &mvp::config::LoongClawConfig,
    target: &mvp::channel::ResolvedKnownChannelSessionTarget,
    matched_access_policy: Option<&mvp::channel::ChannelConfiguredAccountAccessPolicy>,
) -> CliResult<Option<mvp::channel::ChannelPairingResolution>> {
    let (configured_account_id, mode) = match target.channel_id.as_str() {
        "telegram" => {
            let resolved = config
                .telegram
                .resolve_account_for_session_account_id(target.account_id.as_deref())?;
            (resolved.configured_account_id, resolved.pairing_mode)
        }
        "feishu" => {
            let resolved = config
                .feishu
                .resolve_account_for_session_account_id(target.account_id.as_deref())?;
            (resolved.configured_account_id, resolved.pairing_mode)
        }
        "matrix" => {
            let resolved = config
                .matrix
                .resolve_account_for_session_account_id(target.account_id.as_deref())?;
            (resolved.configured_account_id, resolved.pairing_mode)
        }
        "wecom" => {
            let resolved = config
                .wecom
                .resolve_account_for_session_account_id(target.account_id.as_deref())?;
            (resolved.configured_account_id, resolved.pairing_mode)
        }
        _ => return Ok(None),
    };
    let static_sender_gate_present = matched_access_policy
        .map(|policy| {
            policy.summary.sender_mode != mvp::channel::ChannelAccessRestrictionMode::Open
        })
        .unwrap_or(false);
    let subject = target
        .participant_id
        .as_deref()
        .map(|participant_id| {
            mvp::channel::pairing::ChannelPairingSubject::new(
                target.channel_id.as_str(),
                configured_account_id.as_str(),
                target.account_id.clone(),
                target
                    .conversation_id
                    .clone()
                    .unwrap_or_else(|| target.target_id.clone()),
                participant_id.to_owned(),
                target.route_session_id.clone(),
                None,
            )
        })
        .transpose()?;

    #[cfg(feature = "memory-sqlite")]
    {
        let memory_config =
            mvp::memory::runtime_config::MemoryRuntimeConfig::from_memory_config(&config.memory);
        let resolution = mvp::channel::pairing::describe_channel_pairing_resolution(
            &memory_config,
            mode,
            static_sender_gate_present,
            subject.as_ref(),
        )?;
        Ok(Some(resolution))
    }

    #[cfg(not(feature = "memory-sqlite"))]
    {
        let resolution = mvp::channel::ChannelPairingResolution {
            mode,
            state: mvp::channel::ChannelPairingState::NotApplicable,
            static_sender_gate_present,
            pairing_request_id: None,
            binding_id: None,
        };
        let _ = subject;
        Ok(Some(resolution))
    }
}

fn render_access_policy_resolution_line(
    access_policy: &mvp::channel::ChannelConfiguredAccountAccessPolicy,
) -> String {
    let conversations = if access_policy.summary.allowed_conversations.is_empty() {
        "-".to_owned()
    } else {
        access_policy.summary.allowed_conversations.join(",")
    };
    let senders = if access_policy.summary.allowed_senders.is_empty() {
        "-".to_owned()
    } else {
        access_policy.summary.allowed_senders.join(",")
    };

    format!(
        "access_policy configured_account={} conversation_key={} conversation_mode={} sender_key={} sender_mode={} mention_required={} pairing_required={} conversations={} senders={}",
        access_policy.configured_account_id,
        access_policy.conversation_config_key,
        access_policy.summary.conversation_mode.as_str(),
        access_policy.sender_config_key,
        access_policy.summary.sender_mode.as_str(),
        access_policy.summary.mention_required,
        access_policy.summary.pairing_required,
        conversations,
        senders,
    )
}

fn matched_configured_account_id_for_target(
    config: &mvp::config::LoongClawConfig,
    target: &mvp::channel::ResolvedKnownChannelSessionTarget,
) -> CliResult<Option<String>> {
    match target.channel_id.as_str() {
        "telegram" => config
            .telegram
            .resolve_account_for_session_account_id(target.account_id.as_deref())
            .map(|resolved| Some(resolved.configured_account_id)),
        "feishu" => config
            .feishu
            .resolve_account_for_session_account_id(target.account_id.as_deref())
            .map(|resolved| Some(resolved.configured_account_id)),
        "matrix" => config
            .matrix
            .resolve_account_for_session_account_id(target.account_id.as_deref())
            .map(|resolved| Some(resolved.configured_account_id)),
        "wecom" => config
            .wecom
            .resolve_account_for_session_account_id(target.account_id.as_deref())
            .map(|resolved| Some(resolved.configured_account_id)),
        _ => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_resolution_prefers_known_session_targets_over_catalog_aliases() {
        let config: mvp::config::LoongClawConfig = serde_json::from_value(serde_json::json!({
            "telegram": {
                "enabled": true,
                "bot_token": "123456:test-token",
                "allowed_chat_ids": [123]
            }
        }))
        .expect("deserialize telegram config");
        let inventory = mvp::channel::channel_inventory(&config);

        let resolution =
            build_channel_resolution("/tmp/loongclaw.toml", &config, &inventory, "telegram:123")
                .expect("resolve session");

        match resolution.resolution {
            ChannelResolveReadModel::Session(details) => {
                let target = &details.target;
                assert_eq!(target.channel_id, "telegram");
                assert_eq!(target.target_id, "123");
            }
            other => panic!("expected session resolution, got {other:?}"),
        }
    }

    #[test]
    fn channel_resolution_resolves_catalog_aliases() {
        let config = mvp::config::LoongClawConfig::default();
        let inventory = mvp::channel::channel_inventory(&config);

        let resolution =
            build_channel_resolution("/tmp/loongclaw.toml", &config, &inventory, "lark")
                .expect("resolve catalog");

        assert_eq!(
            resolution.schema_version,
            CHANNEL_RESOLVE_JSON_SCHEMA_VERSION
        );
        match resolution.resolution {
            ChannelResolveReadModel::Catalog(details) => {
                let canonical_channel_id = details.canonical_channel_id.as_str();
                let catalog = &details.catalog;
                assert_eq!(canonical_channel_id, "feishu");
                assert_eq!(catalog.id, "feishu");
                assert_eq!(catalog.aliases, vec!["lark"]);
            }
            other => panic!("expected catalog resolution, got {other:?}"),
        }
    }

    #[test]
    fn channel_resolution_text_renders_known_session_summary() {
        let config: mvp::config::LoongClawConfig = serde_json::from_value(serde_json::json!({
            "telegram": {
                "enabled": true,
                "accounts": {
                    "ops": {
                        "account_id": "Ops-Bot",
                        "bot_token": "123456:test-token",
                        "allowed_chat_ids": [123]
                    }
                }
            }
        }))
        .expect("deserialize telegram config");
        let inventory = mvp::channel::channel_inventory(&config);
        let resolution = build_channel_resolution(
            "/tmp/loongclaw.toml",
            &config,
            &inventory,
            "telegram:Ops-Bot:123",
        )
        .expect("resolve known session");

        let rendered = render_channel_resolution_text(&resolution);

        assert!(rendered.contains("schema_version=1"));
        assert!(rendered.contains("resolve_kind=session"));
        assert!(rendered.contains("channel_id=telegram"));
        assert!(rendered.contains("session_shape=telegram_chat"));
        assert!(rendered.contains("matched_configured_account=ops"));
        assert!(rendered.contains("send_command=telegram-send"));
        assert!(rendered.contains("access_policy configured_account=ops"));
    }

    #[test]
    fn channel_resolution_text_renders_telegram_participant_scope_without_rewriting_target() {
        let config: mvp::config::LoongClawConfig = serde_json::from_value(serde_json::json!({
            "telegram": {
                "enabled": true,
                "accounts": {
                    "ops": {
                        "account_id": "Ops-Bot",
                        "bot_token": "123456:test-token",
                        "allowed_chat_ids": [123]
                    }
                }
            }
        }))
        .expect("deserialize telegram config");
        let inventory = mvp::channel::channel_inventory(&config);
        let resolution = build_channel_resolution(
            "/tmp/loongclaw.toml",
            &config,
            &inventory,
            "telegram:Ops-Bot:123:p=7:t=42",
        )
        .expect("resolve tagged telegram session");

        let rendered = render_channel_resolution_text(&resolution);

        assert!(rendered.contains("session_shape=telegram_thread"));
        assert!(rendered.contains("target_id=123:42"));
        assert!(rendered.contains("participant_id=7"));
        assert!(rendered.contains("thread_id=42"));
        assert!(rendered.contains("raw_scope=123:p=7:t=42"));
    }

    #[test]
    fn channel_resolution_text_renders_catalog_access_policy_and_stable_targets() {
        let config: mvp::config::LoongClawConfig = serde_json::from_value(serde_json::json!({
            "telegram": {
                "enabled": true,
                "bot_token": "123456:test-token",
                "allowed_chat_ids": [123],
                "require_mention": true
            }
        }))
        .expect("deserialize telegram config");
        let inventory = mvp::channel::channel_inventory(&config);
        let resolution =
            build_channel_resolution("/tmp/loongclaw.toml", &config, &inventory, "telegram")
                .expect("resolve catalog");

        let rendered = render_channel_resolution_text(&resolution);

        assert!(rendered.contains("schema_version=1"));
        assert!(rendered.contains("resolve_kind=catalog"));
        assert!(rendered.contains("channel_id=telegram"));
        assert!(rendered.contains("aliases=-"));
        assert!(rendered.contains("default_configured_account=bot_123456"));
        assert!(rendered.contains("access_policy configured_account=bot_123456"));
        assert!(rendered.contains("mention_required=true"));
    }

    #[cfg(feature = "memory-sqlite")]
    #[test]
    fn channel_resolution_text_renders_pairing_state_for_participant_scoped_session() {
        let mut env = crate::test_support::ScopedEnv::new();
        env.remove("LOONGCLAW_SQLITE_PATH");
        let temp_root = std::env::temp_dir().join(format!(
            "loongclaw-channel-resolution-pairing-{}",
            std::process::id()
        ));
        let sqlite_path = temp_root.join("memory.sqlite3");
        let config: mvp::config::LoongClawConfig = serde_json::from_value(serde_json::json!({
            "memory": {
                "sqlite_path": sqlite_path.to_string_lossy()
            },
            "feishu": {
                "enabled": true,
                "app_id": "cli_a1b2c3",
                "app_secret": "secret",
                "allowed_chat_ids": ["oc_123"],
                "pairing_mode": "participant_approval"
            }
        }))
        .expect("deserialize feishu config");
        let memory_config =
            mvp::memory::runtime_config::MemoryRuntimeConfig::from_memory_config(&config.memory);
        let resolved = config
            .feishu
            .resolve_account(None)
            .expect("resolve default feishu account");
        let subject = mvp::channel::pairing::ChannelPairingSubject::new(
            "feishu",
            resolved.configured_account_id,
            Some(resolved.account.id.clone()),
            "oc_123",
            "ou_sender_1",
            format!("feishu:{}:oc_123:ou_sender_1", resolved.account.id),
            Some(format!("{}:ou_sender_1", resolved.account.id)),
        )
        .expect("build pairing subject");
        let _ = mvp::channel::pairing::evaluate_channel_pairing(&memory_config, &subject)
            .expect("create pending pairing request");
        let inventory = mvp::channel::channel_inventory(&config);
        let resolution = build_channel_resolution(
            "/tmp/loongclaw.toml",
            &config,
            &inventory,
            format!("feishu:{}:oc_123:ou_sender_1", resolved.account.id).as_str(),
        )
        .expect("resolve feishu participant session");

        let rendered = render_channel_resolution_text(&resolution);

        assert!(rendered.contains("pairing_mode=participant_approval"));
        assert!(rendered.contains("pairing_state=pending"));
        assert!(rendered.contains("pairing_code="));
    }
}
