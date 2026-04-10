use loongclaw_app as mvp;

use super::ChannelDoctorCheck;
use super::ensure_default_env_binding;
use super::{ChannelCheckLevel, ChannelPreflightCheck, ChannelPreview, build_channel_preview};
use crate::migration::ChannelCredentialState;
use crate::migration::{ChannelImportReadiness, ImportSurfaceLevel};

pub(super) const ID: &str = "synology-chat";

const FALLBACK_DESCRIPTOR: mvp::config::ChannelDescriptor = mvp::config::ChannelDescriptor {
    id: ID,
    label: "synology-chat",
    surface_label: "synology chat channel",
    runtime_kind: mvp::config::ChannelRuntimeKind::Service,
    serve_subcommand: Some("synology-chat-serve"),
};

pub(super) fn collect_preview(
    config: &mvp::config::LoongClawConfig,
    readiness: &ChannelImportReadiness,
    source: &str,
) -> Option<ChannelPreview> {
    let credential_state = readiness.state(ID);
    let default_synology = mvp::config::SynologyChatChannelConfig::default();
    let configured = config.synology_chat.enabled
        || credential_state != ChannelCredentialState::Missing
        || config.synology_chat.token_env != default_synology.token_env
        || config.synology_chat.incoming_url_env != default_synology.incoming_url_env
        || config.synology_chat.allowed_user_ids != default_synology.allowed_user_ids;
    if !configured {
        return None;
    }

    let level = if credential_state.is_ready() {
        ImportSurfaceLevel::Ready
    } else if config.synology_chat.enabled {
        ImportSurfaceLevel::Review
    } else {
        ImportSurfaceLevel::Blocked
    };
    let detail = preview_detail(config, credential_state);

    Some(build_channel_preview(
        ID,
        descriptor().label,
        descriptor().surface_label,
        source.to_owned(),
        level,
        detail,
    ))
}

pub(super) fn apply(
    target: &mut mvp::config::LoongClawConfig,
    source: &mvp::config::LoongClawConfig,
) -> bool {
    merge_synology_chat_config(&mut target.synology_chat, &source.synology_chat)
}

pub(super) fn readiness_state(config: &mvp::config::LoongClawConfig) -> ChannelCredentialState {
    let send_ready = synology_chat_send_credentials_ready(config);
    let serve_ready = synology_chat_serve_credentials_ready(config);
    let has_any_credential = synology_chat_has_any_runtime_credential(config);

    if send_ready && serve_ready {
        return ChannelCredentialState::Ready;
    }
    if has_any_credential {
        return ChannelCredentialState::Partial;
    }

    ChannelCredentialState::Missing
}

pub(super) fn apply_import_readiness(
    target: &mut mvp::config::LoongClawConfig,
    state: ChannelCredentialState,
) {
    if state.is_ready() {
        target.synology_chat.enabled = true;
    }
}

pub(super) fn collect_preflight_checks(
    config: &mvp::config::LoongClawConfig,
) -> Vec<ChannelPreflightCheck> {
    let send_ready = synology_chat_send_credentials_ready(config);
    let serve_ready = synology_chat_serve_credentials_ready(config);
    let send_level = if send_ready {
        ChannelCheckLevel::Pass
    } else {
        ChannelCheckLevel::Warn
    };
    let serve_level = if serve_ready {
        ChannelCheckLevel::Pass
    } else {
        ChannelCheckLevel::Warn
    };
    let send_detail = synology_chat_send_detail(send_ready);
    let serve_detail = synology_chat_serve_detail(serve_ready);

    vec![
        ChannelPreflightCheck {
            name: descriptor().surface_label,
            level: send_level,
            detail: send_detail,
        },
        ChannelPreflightCheck {
            name: "synology chat outgoing webhook service",
            level: serve_level,
            detail: serve_detail,
        },
    ]
}

pub(super) fn collect_doctor_checks(
    config: &mvp::config::LoongClawConfig,
) -> Vec<ChannelDoctorCheck> {
    let send_ready = synology_chat_send_credentials_ready(config);
    let serve_ready = synology_chat_serve_credentials_ready(config);
    let send_level = if send_ready {
        ChannelCheckLevel::Pass
    } else {
        ChannelCheckLevel::Fail
    };
    let serve_level = if serve_ready {
        ChannelCheckLevel::Pass
    } else {
        ChannelCheckLevel::Fail
    };
    let send_detail = synology_chat_send_detail(send_ready);
    let serve_detail = synology_chat_serve_detail(serve_ready);

    vec![
        ChannelDoctorCheck {
            name: descriptor().surface_label,
            level: send_level,
            detail: send_detail,
        },
        ChannelDoctorCheck {
            name: "synology chat outgoing webhook service",
            level: serve_level,
            detail: serve_detail,
        },
    ]
}

pub(super) fn apply_default_env_bindings(config: &mut mvp::config::LoongClawConfig) -> Vec<String> {
    let mut fixes = Vec::new();
    let default = mvp::config::SynologyChatChannelConfig::default();

    ensure_default_env_binding(
        &mut config.synology_chat.token_env,
        default.token_env.as_deref(),
        "set synology_chat.token_env",
        &mut fixes,
    );
    ensure_default_env_binding(
        &mut config.synology_chat.incoming_url_env,
        default.incoming_url_env.as_deref(),
        "set synology_chat.incoming_url_env",
        &mut fixes,
    );

    fixes
}

fn preview_detail(
    config: &mvp::config::LoongClawConfig,
    credential_state: ChannelCredentialState,
) -> String {
    match (config.synology_chat.enabled, credential_state) {
        (true, ChannelCredentialState::Ready) => {
            "enabled · incoming webhook and outgoing webhook credentials resolved".to_owned()
        }
        (false, ChannelCredentialState::Ready) => {
            "incoming webhook and outgoing webhook credentials resolved · can enable during onboarding"
                .to_owned()
        }
        (true, ChannelCredentialState::Partial) => {
            "enabled · token or incoming_url missing".to_owned()
        }
        (false, ChannelCredentialState::Partial) => {
            "configured · token or incoming_url missing".to_owned()
        }
        (true, ChannelCredentialState::Missing) => {
            "enabled · token or incoming_url missing".to_owned()
        }
        (false, ChannelCredentialState::Missing) => "configured but disabled".to_owned(),
    }
}

fn synology_chat_send_credentials_ready(config: &mvp::config::LoongClawConfig) -> bool {
    config.synology_chat.incoming_url().is_some()
}

fn synology_chat_serve_credentials_ready(config: &mvp::config::LoongClawConfig) -> bool {
    config.synology_chat.token().is_some()
}

fn synology_chat_has_any_runtime_credential(config: &mvp::config::LoongClawConfig) -> bool {
    let has_token = config.synology_chat.token().is_some();
    let has_incoming_url = config.synology_chat.incoming_url().is_some();

    has_token || has_incoming_url
}

fn synology_chat_send_detail(send_ready: bool) -> String {
    if send_ready {
        return "incoming webhook delivery target resolved".to_owned();
    }

    "enabled but incoming_url is missing".to_owned()
}

fn synology_chat_serve_detail(serve_ready: bool) -> String {
    if serve_ready {
        return "outgoing webhook token resolved".to_owned();
    }

    "enabled but token is missing".to_owned()
}

fn merge_synology_chat_config(
    target: &mut mvp::config::SynologyChatChannelConfig,
    source: &mvp::config::SynologyChatChannelConfig,
) -> bool {
    let mut changed = false;

    changed |= merge_bool(&mut target.enabled, source.enabled);
    changed |= merge_option(&mut target.account_id, &source.account_id);
    changed |= merge_option(&mut target.default_account, &source.default_account);
    changed |= merge_option(&mut target.token, &source.token);
    changed |= merge_option(&mut target.token_env, &source.token_env);
    changed |= merge_option(&mut target.incoming_url, &source.incoming_url);
    changed |= merge_option(&mut target.incoming_url_env, &source.incoming_url_env);
    if !source.allowed_user_ids.is_empty() && target.allowed_user_ids != source.allowed_user_ids {
        target.allowed_user_ids = source.allowed_user_ids.clone();
        changed = true;
    }

    for (account_id, source_account) in &source.accounts {
        let target_account = target.accounts.entry(account_id.clone()).or_default();
        changed |= merge_option(&mut target_account.enabled, &source_account.enabled);
        changed |= merge_option(&mut target_account.account_id, &source_account.account_id);
        changed |= merge_option(&mut target_account.token, &source_account.token);
        changed |= merge_option(&mut target_account.token_env, &source_account.token_env);
        changed |= merge_option(
            &mut target_account.incoming_url,
            &source_account.incoming_url,
        );
        changed |= merge_option(
            &mut target_account.incoming_url_env,
            &source_account.incoming_url_env,
        );
        changed |= merge_option(
            &mut target_account.allowed_user_ids,
            &source_account.allowed_user_ids,
        );
    }

    changed
}

fn merge_bool(target: &mut bool, source: bool) -> bool {
    if *target == source {
        return false;
    }

    *target = source;
    true
}

fn merge_option<T: Clone + PartialEq>(target: &mut Option<T>, source: &Option<T>) -> bool {
    let Some(source_value) = source.as_ref() else {
        return false;
    };
    let should_update = target.as_ref() != Some(source_value);
    if !should_update {
        return false;
    }

    *target = Some(source_value.clone());
    true
}

fn descriptor() -> &'static mvp::config::ChannelDescriptor {
    mvp::config::channel_descriptor(ID).unwrap_or(&FALLBACK_DESCRIPTOR)
}
