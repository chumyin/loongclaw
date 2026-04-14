use serde_json::json;

use crate::{CliResult, mvp};

fn parse_channel_pairing_status(
    raw: Option<&str>,
) -> CliResult<Option<mvp::session::repository::ChannelPairingRequestStatus>> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    let normalized = raw.trim().to_ascii_lowercase();
    let status = match normalized.as_str() {
        "pending" => mvp::session::repository::ChannelPairingRequestStatus::Pending,
        "approved" => mvp::session::repository::ChannelPairingRequestStatus::Approved,
        "rejected" => mvp::session::repository::ChannelPairingRequestStatus::Rejected,
        _ => {
            return Err(format!("unknown channel pairing status `{raw}`"));
        }
    };
    Ok(Some(status))
}

fn local_channel_pairing_actor_session_id() -> String {
    let process_id = std::process::id();
    format!("local-cli:{process_id}")
}

pub fn run_list_channel_pairings_cli(
    config_path: Option<&str>,
    status: Option<&str>,
    limit: usize,
    as_json: bool,
) -> CliResult<()> {
    #[cfg(not(feature = "memory-sqlite"))]
    {
        let _ = (config_path, status, limit, as_json);
        Err("channel pairing persistence requires feature `memory-sqlite`".to_owned())
    }

    #[cfg(feature = "memory-sqlite")]
    {
        let (resolved_path, config) = mvp::config::load(config_path)?;
        let memory_config =
            mvp::memory::runtime_config::MemoryRuntimeConfig::from_memory_config(&config.memory);
        let status = parse_channel_pairing_status(status)?;
        let requests =
            mvp::channel::pairing::list_channel_pairing_requests(&memory_config, status, limit)?;
        let sqlite_path = config.memory.resolved_sqlite_path();

        if as_json {
            let payload = json!({
                "config": resolved_path.display().to_string(),
                "sqlite_path": sqlite_path.display().to_string(),
                "requests": requests,
            });
            let pretty = serde_json::to_string_pretty(&payload).map_err(|error| {
                format!("serialize channel pairing list output failed: {error}")
            })?;
            println!("{pretty}");
            return Ok(());
        }

        println!(
            "config={} sqlite_path={}",
            resolved_path.display(),
            sqlite_path.display()
        );
        if requests.is_empty() {
            println!("requests: (none)");
            return Ok(());
        }
        println!("requests:");
        for request in requests {
            println!(
                "- pairing_request_id={} pairing_code={} expires_at_ms={} channel_id={} configured_account_id={} account_id={} conversation_id={} participant_id={} route_session_id={} status={} requested_at_ms={} resolved_at_ms={} approved_binding_id={} sender_principal_key={} last_error={}",
                request.pairing_request_id,
                request.pairing_code,
                request.expires_at_ms,
                request.channel_id,
                request.configured_account_id,
                request.account_id.as_deref().unwrap_or("(none)"),
                request.conversation_id,
                request.participant_id,
                request.route_session_id,
                request.status.as_str(),
                request.requested_at_ms,
                request
                    .resolved_at_ms
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "(none)".to_owned()),
                request.approved_binding_id.as_deref().unwrap_or("(none)"),
                request.sender_principal_key.as_deref().unwrap_or("(none)"),
                request.last_error.as_deref().unwrap_or("(none)"),
            );
        }
        Ok(())
    }
}

pub fn run_channel_pairing_history_cli(
    config_path: Option<&str>,
    pairing_request_id: Option<&str>,
    channel_id: Option<&str>,
    configured_account_id: Option<&str>,
    conversation_id: Option<&str>,
    participant_id: Option<&str>,
    event_kind: Option<&str>,
    actor_session_id: Option<&str>,
    limit: usize,
    as_json: bool,
) -> CliResult<()> {
    #[cfg(not(feature = "memory-sqlite"))]
    {
        let _ = (
            config_path,
            pairing_request_id,
            channel_id,
            configured_account_id,
            conversation_id,
            participant_id,
            event_kind,
            actor_session_id,
            limit,
            as_json,
        );
        Err("channel pairing persistence requires feature `memory-sqlite`".to_owned())
    }

    #[cfg(feature = "memory-sqlite")]
    {
        let (resolved_path, config) = mvp::config::load(config_path)?;
        let memory_config =
            mvp::memory::runtime_config::MemoryRuntimeConfig::from_memory_config(&config.memory);
        let query = mvp::channel::pairing::ChannelPairingHistoryQuery::new(
            pairing_request_id.map(str::to_owned),
            channel_id.map(str::to_owned),
            configured_account_id.map(str::to_owned),
            conversation_id.map(str::to_owned),
            participant_id.map(str::to_owned),
            event_kind.map(str::to_owned),
            actor_session_id.map(str::to_owned),
        )?;
        let events =
            mvp::channel::pairing::list_channel_pairing_history(&memory_config, &query, limit)?;

        if as_json {
            let payload = json!({
                "config": resolved_path.display().to_string(),
                "query": query,
                "events": events,
            });
            let pretty = serde_json::to_string_pretty(&payload).map_err(|error| {
                format!("serialize channel pairing history output failed: {error}")
            })?;
            println!("{pretty}");
            return Ok(());
        }

        println!("config={}", resolved_path.display());
        println!(
            "query pairing_request_id={} channel_id={} configured_account_id={} conversation_id={} participant_id={} event_kind={} actor_session_id={}",
            query.pairing_request_id.as_deref().unwrap_or("(none)"),
            query.channel_id.as_deref().unwrap_or("(none)"),
            query.configured_account_id.as_deref().unwrap_or("(none)"),
            query.conversation_id.as_deref().unwrap_or("(none)"),
            query.participant_id.as_deref().unwrap_or("(none)"),
            query
                .event_kind
                .map(|event_kind| event_kind.as_str())
                .unwrap_or("(none)"),
            query.actor_session_id.as_deref().unwrap_or("(none)"),
        );
        if events.is_empty() {
            println!("events: (none)");
            return Ok(());
        }
        println!("events:");
        for event in events {
            println!(
                "- event_id={} event_kind={} event_at_ms={} pairing_request_id={} binding_id={} channel_id={} configured_account_id={} account_id={} conversation_id={} participant_id={} route_session_id={} sender_principal_key={} actor_session_id={} detail={}",
                event.event_id,
                event.event_kind.as_str(),
                event.event_at_ms,
                event.pairing_request_id.as_deref().unwrap_or("(none)"),
                event.binding_id.as_deref().unwrap_or("(none)"),
                event.channel_id,
                event.configured_account_id,
                event.account_id.as_deref().unwrap_or("(none)"),
                event.conversation_id,
                event.participant_id,
                event.route_session_id,
                event.sender_principal_key.as_deref().unwrap_or("(none)"),
                event.actor_session_id.as_deref().unwrap_or("(none)"),
                event.detail.as_deref().unwrap_or("(none)"),
            );
        }
        Ok(())
    }
}

pub fn run_resolve_channel_pairing_cli(
    config_path: Option<&str>,
    pairing_request_id: Option<&str>,
    pairing_code: Option<&str>,
    approve: bool,
    as_json: bool,
) -> CliResult<()> {
    #[cfg(not(feature = "memory-sqlite"))]
    {
        let _ = (
            config_path,
            pairing_request_id,
            pairing_code,
            approve,
            as_json,
        );
        Err("channel pairing persistence requires feature `memory-sqlite`".to_owned())
    }

    #[cfg(feature = "memory-sqlite")]
    {
        let (resolved_path, config) = mvp::config::load(config_path)?;
        let memory_config =
            mvp::memory::runtime_config::MemoryRuntimeConfig::from_memory_config(&config.memory);
        let pairing_request_id = pairing_request_id
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let pairing_code = pairing_code
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let updated = match (pairing_request_id, pairing_code) {
            (Some(pairing_request_id), None) => {
                let actor_session_id = Some(local_channel_pairing_actor_session_id());
                mvp::channel::pairing::resolve_channel_pairing_request(
                    &memory_config,
                    pairing_request_id,
                    approve,
                    actor_session_id,
                )?
            }
            (None, Some(pairing_code)) => {
                let actor_session_id = Some(local_channel_pairing_actor_session_id());
                mvp::channel::pairing::resolve_channel_pairing_request_by_code(
                    &memory_config,
                    pairing_code,
                    approve,
                    actor_session_id,
                )?
            }
            (Some(_), Some(_)) => {
                return Err(
                    "channel pairing resolve accepts either pairing_request_id or pairing_code"
                        .to_owned(),
                );
            }
            (None, None) => {
                return Err(
                    "channel pairing resolve requires pairing_request_id or pairing_code"
                        .to_owned(),
                );
            }
        };
        let Some(updated) = updated else {
            return Err("channel pairing request not found".to_owned());
        };

        if as_json {
            let payload = json!({
                "config": resolved_path.display().to_string(),
                "request": updated,
            });
            let pretty = serde_json::to_string_pretty(&payload).map_err(|error| {
                format!("serialize channel pairing resolve output failed: {error}")
            })?;
            println!("{pretty}");
            return Ok(());
        }

        println!(
            "config={} pairing_request_id={} pairing_code={} expires_at_ms={} status={} conversation_id={} participant_id={} approved_binding_id={}",
            resolved_path.display(),
            updated.pairing_request_id,
            updated.pairing_code,
            updated.expires_at_ms,
            updated.status.as_str(),
            updated.conversation_id,
            updated.participant_id,
            updated.approved_binding_id.as_deref().unwrap_or("(none)"),
        );
        Ok(())
    }
}

pub fn run_revoke_channel_pairing_cli(
    config_path: Option<&str>,
    pairing_request_id: Option<&str>,
    pairing_code: Option<&str>,
    as_json: bool,
) -> CliResult<()> {
    #[cfg(not(feature = "memory-sqlite"))]
    {
        let _ = (config_path, pairing_request_id, pairing_code, as_json);
        Err("channel pairing persistence requires feature `memory-sqlite`".to_owned())
    }

    #[cfg(feature = "memory-sqlite")]
    {
        let (resolved_path, config) = mvp::config::load(config_path)?;
        let memory_config =
            mvp::memory::runtime_config::MemoryRuntimeConfig::from_memory_config(&config.memory);
        let selection = mvp::channel::pairing::ChannelPairingRequestSelection::new(
            pairing_request_id.map(str::to_owned),
            pairing_code.map(str::to_owned),
        )?;
        let actor_session_id = Some(local_channel_pairing_actor_session_id());
        let updated = mvp::channel::pairing::revoke_channel_pairing(
            &memory_config,
            &selection,
            actor_session_id,
        )?;
        let Some(updated) = updated else {
            return Err("channel pairing request not found".to_owned());
        };

        if as_json {
            let payload = json!({
                "config": resolved_path.display().to_string(),
                "request": updated,
            });
            let pretty = serde_json::to_string_pretty(&payload).map_err(|error| {
                format!("serialize channel pairing revoke output failed: {error}")
            })?;
            println!("{pretty}");
            return Ok(());
        }

        println!(
            "config={} pairing_request_id={} pairing_code={} status={} last_error={}",
            resolved_path.display(),
            updated.pairing_request_id,
            updated.pairing_code,
            updated.status.as_str(),
            updated.last_error.as_deref().unwrap_or("(none)"),
        );
        Ok(())
    }
}

pub fn run_clear_pending_channel_pairings_cli(
    config_path: Option<&str>,
    channel_id: &str,
    configured_account_id: &str,
    conversation_id: Option<&str>,
    participant_id: Option<&str>,
    as_json: bool,
) -> CliResult<()> {
    #[cfg(not(feature = "memory-sqlite"))]
    {
        let _ = (
            config_path,
            channel_id,
            configured_account_id,
            conversation_id,
            participant_id,
            as_json,
        );
        Err("channel pairing persistence requires feature `memory-sqlite`".to_owned())
    }

    #[cfg(feature = "memory-sqlite")]
    {
        let (resolved_path, config) = mvp::config::load(config_path)?;
        let memory_config =
            mvp::memory::runtime_config::MemoryRuntimeConfig::from_memory_config(&config.memory);
        let scope = mvp::channel::pairing::ChannelPairingPendingScope::new(
            channel_id.to_owned(),
            configured_account_id.to_owned(),
            conversation_id.map(str::to_owned),
            participant_id.map(str::to_owned),
        )?;
        let actor_session_id = Some(local_channel_pairing_actor_session_id());
        let cleared_request_ids = mvp::channel::pairing::clear_pending_channel_pairings(
            &memory_config,
            &scope,
            actor_session_id,
        )?;
        let cleared_count = cleared_request_ids.len();

        if as_json {
            let payload = json!({
                "config": resolved_path.display().to_string(),
                "channel_id": scope.channel_id,
                "configured_account_id": scope.configured_account_id,
                "conversation_id": scope.conversation_id,
                "participant_id": scope.participant_id,
                "cleared_count": cleared_count,
                "cleared_request_ids": cleared_request_ids,
            });
            let pretty = serde_json::to_string_pretty(&payload).map_err(|error| {
                format!("serialize clear pending channel pairings output failed: {error}")
            })?;
            println!("{pretty}");
            return Ok(());
        }

        println!(
            "config={} channel_id={} configured_account_id={} conversation_id={} participant_id={} cleared_count={} cleared_request_ids={}",
            resolved_path.display(),
            scope.channel_id,
            scope.configured_account_id,
            scope.conversation_id.as_deref().unwrap_or("(none)"),
            scope.participant_id.as_deref().unwrap_or("(none)"),
            cleared_count,
            if cleared_request_ids.is_empty() {
                "(none)".to_owned()
            } else {
                cleared_request_ids.join(",")
            }
        );
        Ok(())
    }
}
