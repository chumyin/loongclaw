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
                "- pairing_request_id={} channel_id={} configured_account_id={} account_id={} conversation_id={} participant_id={} route_session_id={} status={} requested_at_ms={} resolved_at_ms={} approved_binding_id={} sender_principal_key={}",
                request.pairing_request_id,
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
            );
        }
        Ok(())
    }
}

pub fn run_resolve_channel_pairing_cli(
    config_path: Option<&str>,
    pairing_request_id: &str,
    approve: bool,
    as_json: bool,
) -> CliResult<()> {
    #[cfg(not(feature = "memory-sqlite"))]
    {
        let _ = (config_path, pairing_request_id, approve, as_json);
        Err("channel pairing persistence requires feature `memory-sqlite`".to_owned())
    }

    #[cfg(feature = "memory-sqlite")]
    {
        let (resolved_path, config) = mvp::config::load(config_path)?;
        let memory_config =
            mvp::memory::runtime_config::MemoryRuntimeConfig::from_memory_config(&config.memory);
        let updated = mvp::channel::pairing::resolve_channel_pairing_request(
            &memory_config,
            pairing_request_id,
            approve,
            None,
        )?;
        let Some(updated) = updated else {
            return Err(format!(
                "channel pairing request `{pairing_request_id}` not found"
            ));
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
            "config={} pairing_request_id={} status={} conversation_id={} participant_id={} approved_binding_id={}",
            resolved_path.display(),
            updated.pairing_request_id,
            updated.status.as_str(),
            updated.conversation_id,
            updated.participant_id,
            updated.approved_binding_id.as_deref().unwrap_or("(none)"),
        );
        Ok(())
    }
}
