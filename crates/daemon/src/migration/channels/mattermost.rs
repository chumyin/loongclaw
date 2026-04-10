use loongclaw_app as mvp;

use super::ChannelDoctorCheck;
use super::ensure_default_env_binding;
use super::{ChannelCheckLevel, ChannelPreflightCheck, ChannelPreview, build_channel_preview};
use crate::migration::ChannelCredentialState;
use crate::migration::{ChannelImportReadiness, ImportSurfaceLevel};

pub(super) const ID: &str = "mattermost";

const FALLBACK_DESCRIPTOR: mvp::config::ChannelDescriptor = mvp::config::ChannelDescriptor {
    id: ID,
    label: "mattermost",
    surface_label: "mattermost channel",
    runtime_kind: mvp::config::ChannelRuntimeKind::Service,
    serve_subcommand: Some("mattermost-serve"),
};

pub(super) fn collect_preview(
    config: &mvp::config::LoongClawConfig,
    readiness: &ChannelImportReadiness,
    source: &str,
) -> Option<ChannelPreview> {
    let credential_state = readiness.state(ID);
    let default_mattermost = mvp::config::MattermostChannelConfig::default();
    let configured = config.mattermost.enabled
        || credential_state != ChannelCredentialState::Missing
        || config.mattermost.server_url_env != default_mattermost.server_url_env
        || config.mattermost.bot_token_env != default_mattermost.bot_token_env
        || config.mattermost.outgoing_token_env != default_mattermost.outgoing_token_env
        || config.mattermost.allowed_channel_ids != default_mattermost.allowed_channel_ids;
    if !configured {
        return None;
    }

    let level = if credential_state.is_ready() {
        ImportSurfaceLevel::Ready
    } else if config.mattermost.enabled {
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
    merge_mattermost_config(&mut target.mattermost, &source.mattermost)
}

pub(super) fn readiness_state(config: &mvp::config::LoongClawConfig) -> ChannelCredentialState {
    let send_ready = mattermost_send_credentials_ready(config);
    let serve_ready = mattermost_serve_credentials_ready(config);
    let has_any_credential = mattermost_has_any_runtime_credential(config);

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
        target.mattermost.enabled = true;
    }
}

pub(super) fn collect_preflight_checks(
    config: &mvp::config::LoongClawConfig,
) -> Vec<ChannelPreflightCheck> {
    let send_ready = mattermost_send_credentials_ready(config);
    let serve_ready = mattermost_serve_credentials_ready(config);
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
    let send_detail = mattermost_send_detail(send_ready);
    let serve_detail = mattermost_serve_detail(serve_ready);

    vec![
        ChannelPreflightCheck {
            name: descriptor().surface_label,
            level: send_level,
            detail: send_detail,
        },
        ChannelPreflightCheck {
            name: "mattermost outgoing webhook service",
            level: serve_level,
            detail: serve_detail,
        },
    ]
}

pub(super) fn collect_doctor_checks(
    config: &mvp::config::LoongClawConfig,
) -> Vec<ChannelDoctorCheck> {
    let send_ready = mattermost_send_credentials_ready(config);
    let serve_ready = mattermost_serve_credentials_ready(config);
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
    let send_detail = mattermost_send_detail(send_ready);
    let serve_detail = mattermost_serve_detail(serve_ready);

    vec![
        ChannelDoctorCheck {
            name: descriptor().surface_label,
            level: send_level,
            detail: send_detail,
        },
        ChannelDoctorCheck {
            name: "mattermost outgoing webhook service",
            level: serve_level,
            detail: serve_detail,
        },
    ]
}

pub(super) fn apply_default_env_bindings(config: &mut mvp::config::LoongClawConfig) -> Vec<String> {
    let mut fixes = Vec::new();
    let default = mvp::config::MattermostChannelConfig::default();

    ensure_default_env_binding(
        &mut config.mattermost.server_url_env,
        default.server_url_env.as_deref(),
        "set mattermost.server_url_env",
        &mut fixes,
    );
    ensure_default_env_binding(
        &mut config.mattermost.bot_token_env,
        default.bot_token_env.as_deref(),
        "set mattermost.bot_token_env",
        &mut fixes,
    );
    ensure_default_env_binding(
        &mut config.mattermost.outgoing_token_env,
        default.outgoing_token_env.as_deref(),
        "set mattermost.outgoing_token_env",
        &mut fixes,
    );

    fixes
}

fn preview_detail(
    config: &mvp::config::LoongClawConfig,
    credential_state: ChannelCredentialState,
) -> String {
    match (config.mattermost.enabled, credential_state) {
        (true, ChannelCredentialState::Ready) => {
            "enabled · REST send and outgoing webhook credentials resolved".to_owned()
        }
        (false, ChannelCredentialState::Ready) => {
            "REST send and outgoing webhook credentials resolved · can enable during onboarding"
                .to_owned()
        }
        (true, ChannelCredentialState::Partial) => {
            "enabled · server_url, bot_token, outgoing_token, or allowed_channel_ids missing"
                .to_owned()
        }
        (false, ChannelCredentialState::Partial) => {
            "configured · server_url, bot_token, outgoing_token, or allowed_channel_ids missing"
                .to_owned()
        }
        (true, ChannelCredentialState::Missing) => {
            "enabled · server_url, bot_token, outgoing_token, or allowed_channel_ids missing"
                .to_owned()
        }
        (false, ChannelCredentialState::Missing) => "configured but disabled".to_owned(),
    }
}

fn mattermost_send_credentials_ready(config: &mvp::config::LoongClawConfig) -> bool {
    let has_server_url = config.mattermost.server_url().is_some();
    let has_bot_token = config.mattermost.bot_token().is_some();

    has_server_url && has_bot_token
}

fn mattermost_serve_credentials_ready(config: &mvp::config::LoongClawConfig) -> bool {
    let has_outgoing_token = config.mattermost.outgoing_token().is_some();
    let has_allowed_channel_ids = !config.mattermost.allowed_channel_ids.is_empty();

    has_outgoing_token && has_allowed_channel_ids
}

fn mattermost_has_any_runtime_credential(config: &mvp::config::LoongClawConfig) -> bool {
    let has_server_url = config.mattermost.server_url().is_some();
    let has_bot_token = config.mattermost.bot_token().is_some();
    let has_outgoing_token = config.mattermost.outgoing_token().is_some();
    let has_allowed_channel_ids = !config.mattermost.allowed_channel_ids.is_empty();

    has_server_url || has_bot_token || has_outgoing_token || has_allowed_channel_ids
}

fn mattermost_send_detail(send_ready: bool) -> String {
    if send_ready {
        return "REST post send credentials resolved".to_owned();
    }

    "enabled but server_url or bot_token is missing".to_owned()
}

fn mattermost_serve_detail(serve_ready: bool) -> String {
    if serve_ready {
        return "outgoing webhook token and allowed channel scope resolved".to_owned();
    }

    "enabled but outgoing_token or allowed_channel_ids is missing".to_owned()
}

fn merge_mattermost_config(
    target: &mut mvp::config::MattermostChannelConfig,
    source: &mvp::config::MattermostChannelConfig,
) -> bool {
    let mut changed = false;

    changed |= merge_bool(&mut target.enabled, source.enabled);
    changed |= merge_option(&mut target.account_id, &source.account_id);
    changed |= merge_option(&mut target.default_account, &source.default_account);
    changed |= merge_option(&mut target.server_url, &source.server_url);
    changed |= merge_option(&mut target.server_url_env, &source.server_url_env);
    changed |= merge_option(&mut target.bot_token, &source.bot_token);
    changed |= merge_option(&mut target.bot_token_env, &source.bot_token_env);
    changed |= merge_option(&mut target.outgoing_token, &source.outgoing_token);
    changed |= merge_option(&mut target.outgoing_token_env, &source.outgoing_token_env);
    if !source.allowed_channel_ids.is_empty()
        && target.allowed_channel_ids != source.allowed_channel_ids
    {
        target.allowed_channel_ids = source.allowed_channel_ids.clone();
        changed = true;
    }

    for (account_id, source_account) in &source.accounts {
        let target_account = target.accounts.entry(account_id.clone()).or_default();
        changed |= merge_option(&mut target_account.enabled, &source_account.enabled);
        changed |= merge_option(&mut target_account.account_id, &source_account.account_id);
        changed |= merge_option(&mut target_account.server_url, &source_account.server_url);
        changed |= merge_option(
            &mut target_account.server_url_env,
            &source_account.server_url_env,
        );
        changed |= merge_option(&mut target_account.bot_token, &source_account.bot_token);
        changed |= merge_option(
            &mut target_account.bot_token_env,
            &source_account.bot_token_env,
        );
        changed |= merge_option(
            &mut target_account.outgoing_token,
            &source_account.outgoing_token,
        );
        changed |= merge_option(
            &mut target_account.outgoing_token_env,
            &source_account.outgoing_token_env,
        );
        changed |= merge_option(
            &mut target_account.allowed_channel_ids,
            &source_account.allowed_channel_ids,
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
