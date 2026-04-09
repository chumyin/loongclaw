use loongclaw_app as mvp;

use super::ChannelDoctorCheck;
use super::ensure_default_env_binding;
use super::{ChannelCheckLevel, ChannelPreflightCheck, ChannelPreview, build_channel_preview};
use crate::migration::ChannelCredentialState;
use crate::migration::{ChannelImportReadiness, ImportSurfaceLevel};

pub(super) const ID: &str = "nextcloud-talk";

const FALLBACK_DESCRIPTOR: mvp::config::ChannelDescriptor = mvp::config::ChannelDescriptor {
    id: ID,
    label: "nextcloud-talk",
    surface_label: "nextcloud talk channel",
    runtime_kind: mvp::config::ChannelRuntimeKind::Service,
    serve_subcommand: Some("nextcloud-talk-serve"),
};

pub(super) fn collect_preview(
    config: &mvp::config::LoongClawConfig,
    readiness: &ChannelImportReadiness,
    source: &str,
) -> Option<ChannelPreview> {
    let credential_state = readiness.state(ID);
    let default_nextcloud_talk = mvp::config::NextcloudTalkChannelConfig::default();
    let configured = config.nextcloud_talk.enabled
        || credential_state != ChannelCredentialState::Missing
        || config.nextcloud_talk.server_url_env != default_nextcloud_talk.server_url_env
        || config.nextcloud_talk.shared_secret_env != default_nextcloud_talk.shared_secret_env;
    if !configured {
        return None;
    }

    let level = if credential_state.is_ready() {
        ImportSurfaceLevel::Ready
    } else if config.nextcloud_talk.enabled {
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
    merge_nextcloud_talk_config(&mut target.nextcloud_talk, &source.nextcloud_talk)
}

pub(super) fn readiness_state(config: &mvp::config::LoongClawConfig) -> ChannelCredentialState {
    let has_server_url = config.nextcloud_talk.server_url().is_some();
    let has_shared_secret = config.nextcloud_talk.shared_secret().is_some();
    match (has_server_url, has_shared_secret) {
        (true, true) => ChannelCredentialState::Ready,
        (true, false) | (false, true) => ChannelCredentialState::Partial,
        (false, false) => ChannelCredentialState::Missing,
    }
}

pub(super) fn apply_import_readiness(
    target: &mut mvp::config::LoongClawConfig,
    state: ChannelCredentialState,
) {
    if state.is_ready() {
        target.nextcloud_talk.enabled = true;
    }
}

pub(super) fn collect_preflight_checks(
    config: &mvp::config::LoongClawConfig,
) -> Vec<ChannelPreflightCheck> {
    let state = readiness_state(config);
    let level = if state.is_ready() {
        ChannelCheckLevel::Pass
    } else {
        ChannelCheckLevel::Warn
    };
    let detail = nextcloud_talk_detail(state);

    vec![ChannelPreflightCheck {
        name: descriptor().surface_label,
        level,
        detail,
    }]
}

pub(super) fn collect_doctor_checks(
    config: &mvp::config::LoongClawConfig,
) -> Vec<ChannelDoctorCheck> {
    let state = readiness_state(config);
    let level = if state.is_ready() {
        ChannelCheckLevel::Pass
    } else {
        ChannelCheckLevel::Fail
    };
    let detail = nextcloud_talk_detail(state);

    vec![ChannelDoctorCheck {
        name: descriptor().surface_label,
        level,
        detail,
    }]
}

pub(super) fn apply_default_env_bindings(config: &mut mvp::config::LoongClawConfig) -> Vec<String> {
    let mut fixes = Vec::new();
    let default = mvp::config::NextcloudTalkChannelConfig::default();

    ensure_default_env_binding(
        &mut config.nextcloud_talk.server_url_env,
        default.server_url_env.as_deref(),
        "set nextcloud_talk.server_url_env",
        &mut fixes,
    );
    ensure_default_env_binding(
        &mut config.nextcloud_talk.shared_secret_env,
        default.shared_secret_env.as_deref(),
        "set nextcloud_talk.shared_secret_env",
        &mut fixes,
    );

    fixes
}

fn preview_detail(
    config: &mvp::config::LoongClawConfig,
    credential_state: ChannelCredentialState,
) -> String {
    match (config.nextcloud_talk.enabled, credential_state) {
        (true, ChannelCredentialState::Ready) => {
            "enabled · server_url and shared_secret resolved".to_owned()
        }
        (false, ChannelCredentialState::Ready) => {
            "server_url and shared_secret resolved · can enable during onboarding".to_owned()
        }
        (true, ChannelCredentialState::Partial) => {
            "enabled · server_url or shared_secret missing".to_owned()
        }
        (false, ChannelCredentialState::Partial) => {
            "configured · server_url or shared_secret missing".to_owned()
        }
        (true, ChannelCredentialState::Missing) => {
            "enabled · server_url or shared_secret missing".to_owned()
        }
        (false, ChannelCredentialState::Missing) => "configured but disabled".to_owned(),
    }
}

fn nextcloud_talk_detail(state: ChannelCredentialState) -> String {
    match state {
        ChannelCredentialState::Ready => "server_url and shared_secret resolved".to_owned(),
        ChannelCredentialState::Partial => "server_url or shared_secret missing".to_owned(),
        ChannelCredentialState::Missing => "server_url and shared_secret missing".to_owned(),
    }
}

fn merge_nextcloud_talk_config(
    target: &mut mvp::config::NextcloudTalkChannelConfig,
    source: &mvp::config::NextcloudTalkChannelConfig,
) -> bool {
    let default = mvp::config::NextcloudTalkChannelConfig::default();
    let mut changed = false;

    if !target.enabled && source.enabled {
        target.enabled = true;
        changed = true;
    }
    if target.account_id.is_none() && source.account_id.is_some() {
        target.account_id = source.account_id.clone();
        changed = true;
    }
    if target.default_account.is_none() && source.default_account.is_some() {
        target.default_account = source.default_account.clone();
        changed = true;
    }
    if target.server_url.is_none() && source.server_url.is_some() {
        target.server_url = source.server_url.clone();
        changed = true;
    }
    if let Some(source_server_url_env) = source.server_url_env.as_ref() {
        let target_uses_default_env =
            target.server_url_env.is_none() || target.server_url_env == default.server_url_env;
        let target_matches_source = target.server_url_env.as_ref() == Some(source_server_url_env);
        if target_uses_default_env && !target_matches_source {
            target.server_url_env = Some(source_server_url_env.clone());
            changed = true;
        }
    }
    if target.shared_secret.is_none() && source.shared_secret.is_some() {
        target.shared_secret = source.shared_secret.clone();
        changed = true;
    }
    if let Some(source_shared_secret_env) = source.shared_secret_env.as_ref() {
        let target_uses_default_env = target.shared_secret_env.is_none()
            || target.shared_secret_env == default.shared_secret_env;
        let target_matches_source =
            target.shared_secret_env.as_ref() == Some(source_shared_secret_env);
        if target_uses_default_env && !target_matches_source {
            target.shared_secret_env = Some(source_shared_secret_env.clone());
            changed = true;
        }
    }

    changed
}

fn descriptor() -> &'static mvp::config::ChannelDescriptor {
    mvp::config::channel_descriptor(ID).unwrap_or(&FALLBACK_DESCRIPTOR)
}
