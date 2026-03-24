use loongclaw_app as mvp;

pub(crate) const MODEL_CATALOG_PROBE_FAILED_MARKER: &str = "model catalog probe failed";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProviderModelProbeFailureLevel {
    Warn,
    Fail,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ProviderModelProbeConfiguredRecoveryKind {
    ExplicitModel {
        model: String,
    },
    PreferredModels {
        fallback_models: Vec<String>,
    },
    RequiresExplicitModel {
        recommended_onboarding_model: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ProviderModelProbeRecoveryKind {
    TransportFailure,
    ExplicitModel {
        model: String,
    },
    PreferredModels {
        fallback_models: Vec<String>,
    },
    RequiresExplicitModel {
        recommended_onboarding_model: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProviderModelProbeFailure {
    pub(crate) level: ProviderModelProbeFailureLevel,
    pub(crate) detail: String,
    pub(crate) recovery_kind: ProviderModelProbeRecoveryKind,
}

pub(crate) fn provider_model_probe_configured_recovery_kind(
    config: &mvp::config::LoongClawConfig,
) -> ProviderModelProbeConfiguredRecoveryKind {
    let recovery = config.provider.model_catalog_probe_recovery();

    match recovery {
        mvp::config::ModelCatalogProbeRecovery::ExplicitModel(model) => {
            ProviderModelProbeConfiguredRecoveryKind::ExplicitModel { model }
        }
        mvp::config::ModelCatalogProbeRecovery::ConfiguredPreferredModels(fallback_models) => {
            ProviderModelProbeConfiguredRecoveryKind::PreferredModels { fallback_models }
        }
        mvp::config::ModelCatalogProbeRecovery::RequiresExplicitModel {
            recommended_onboarding_model,
        } => ProviderModelProbeConfiguredRecoveryKind::RequiresExplicitModel {
            recommended_onboarding_model: recommended_onboarding_model.map(str::to_owned),
        },
    }
}

pub(crate) fn provider_model_probe_failure(
    config: &mvp::config::LoongClawConfig,
    error: &str,
) -> ProviderModelProbeFailure {
    let provider_prefix = crate::provider_presentation::active_provider_detail_label(config);
    let is_transport_failure =
        crate::provider_route_diagnostics::is_transport_style_model_probe_failure(error);

    if is_transport_failure {
        let detail = format!(
            "{provider_prefix}: {} ({error}); runtime could not verify the provider route. inspect provider route diagnostics and retry once dns / proxy / TUN routing is stable",
            crate::provider_route_diagnostics::MODEL_CATALOG_TRANSPORT_FAILED_MARKER
        );

        return ProviderModelProbeFailure {
            level: ProviderModelProbeFailureLevel::Fail,
            detail,
            recovery_kind: ProviderModelProbeRecoveryKind::TransportFailure,
        };
    }

    let configured_recovery_kind = provider_model_probe_configured_recovery_kind(config);
    let recovery_kind = provider_model_probe_failure_recovery_kind(configured_recovery_kind);
    let mut detail =
        render_provider_model_probe_failure_detail(provider_prefix.as_str(), error, &recovery_kind);
    let auth_style_failure = mvp::provider::is_auth_style_failure_message(error);

    if auth_style_failure && let Some(hint) = config.provider.region_endpoint_failure_hint() {
        detail.push(' ');
        detail.push_str(hint.as_str());
    }

    let level = provider_model_probe_failure_level(&recovery_kind);

    ProviderModelProbeFailure {
        level,
        detail,
        recovery_kind,
    }
}

pub(crate) fn provider_model_probe_failed_detail(detail: &str) -> bool {
    let has_model_catalog_failure = detail.contains(MODEL_CATALOG_PROBE_FAILED_MARKER);
    let has_transport_failure =
        detail.contains(crate::provider_route_diagnostics::MODEL_CATALOG_TRANSPORT_FAILED_MARKER);

    has_model_catalog_failure || has_transport_failure
}

pub(crate) fn provider_model_probe_transport_failure_detail(detail: &str) -> bool {
    detail.contains(crate::provider_route_diagnostics::MODEL_CATALOG_TRANSPORT_FAILED_MARKER)
}

pub(crate) fn provider_model_probe_auth_failure_detail(detail: &str) -> bool {
    let has_model_catalog_failure = detail.contains(MODEL_CATALOG_PROBE_FAILED_MARKER);
    if !has_model_catalog_failure {
        return false;
    }

    mvp::provider::is_auth_style_failure_message(detail)
}

fn provider_model_probe_failure_recovery_kind(
    configured_recovery_kind: ProviderModelProbeConfiguredRecoveryKind,
) -> ProviderModelProbeRecoveryKind {
    match configured_recovery_kind {
        ProviderModelProbeConfiguredRecoveryKind::ExplicitModel { model } => {
            ProviderModelProbeRecoveryKind::ExplicitModel { model }
        }
        ProviderModelProbeConfiguredRecoveryKind::PreferredModels { fallback_models } => {
            ProviderModelProbeRecoveryKind::PreferredModels { fallback_models }
        }
        ProviderModelProbeConfiguredRecoveryKind::RequiresExplicitModel {
            recommended_onboarding_model,
        } => ProviderModelProbeRecoveryKind::RequiresExplicitModel {
            recommended_onboarding_model,
        },
    }
}

fn provider_model_probe_failure_level(
    recovery_kind: &ProviderModelProbeRecoveryKind,
) -> ProviderModelProbeFailureLevel {
    match recovery_kind {
        ProviderModelProbeRecoveryKind::TransportFailure => ProviderModelProbeFailureLevel::Fail,
        ProviderModelProbeRecoveryKind::ExplicitModel { .. } => {
            ProviderModelProbeFailureLevel::Warn
        }
        ProviderModelProbeRecoveryKind::PreferredModels { .. } => {
            ProviderModelProbeFailureLevel::Warn
        }
        ProviderModelProbeRecoveryKind::RequiresExplicitModel { .. } => {
            ProviderModelProbeFailureLevel::Fail
        }
    }
}

fn render_provider_model_probe_failure_detail(
    provider_prefix: &str,
    error: &str,
    recovery_kind: &ProviderModelProbeRecoveryKind,
) -> String {
    match recovery_kind {
        ProviderModelProbeRecoveryKind::TransportFailure => format!(
            "{provider_prefix}: {} ({error}); runtime could not verify the provider route. inspect provider route diagnostics and retry once dns / proxy / TUN routing is stable",
            crate::provider_route_diagnostics::MODEL_CATALOG_TRANSPORT_FAILED_MARKER
        ),
        ProviderModelProbeRecoveryKind::ExplicitModel { model } => format!(
            "{provider_prefix}: {MODEL_CATALOG_PROBE_FAILED_MARKER} ({error}); chat may still work because model `{model}` is explicitly configured"
        ),
        ProviderModelProbeRecoveryKind::PreferredModels { fallback_models } => format!(
            "{provider_prefix}: {MODEL_CATALOG_PROBE_FAILED_MARKER} ({error}); runtime will try configured preferred model fallback(s): {}",
            render_model_candidate_list(fallback_models)
        ),
        ProviderModelProbeRecoveryKind::RequiresExplicitModel {
            recommended_onboarding_model,
        } => render_requires_explicit_model_detail(
            provider_prefix,
            error,
            recommended_onboarding_model.as_deref(),
        ),
    }
}

fn render_requires_explicit_model_detail(
    provider_prefix: &str,
    error: &str,
    recommended_onboarding_model: Option<&str>,
) -> String {
    match recommended_onboarding_model {
        Some(model) => format!(
            "{provider_prefix}: {MODEL_CATALOG_PROBE_FAILED_MARKER} ({error}); current config still uses `model = auto`; rerun onboarding and accept reviewed model `{model}`, or set `provider.model` / `preferred_models` explicitly"
        ),
        None => format!(
            "{provider_prefix}: {MODEL_CATALOG_PROBE_FAILED_MARKER} ({error}); current config still uses `model = auto`; set `provider.model` explicitly or configure `preferred_models` before retrying"
        ),
    }
}

fn render_model_candidate_list(models: &[String]) -> String {
    models
        .iter()
        .map(|model| format!("`{model}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_model_probe_recovery_kind_preserves_reviewed_model() {
        let mut config = mvp::config::LoongClawConfig::default();
        config.provider.kind = mvp::config::ProviderKind::Deepseek;
        config.provider.model = "auto".to_owned();

        let recovery_kind = provider_model_probe_configured_recovery_kind(&config);

        assert_eq!(
            recovery_kind,
            ProviderModelProbeConfiguredRecoveryKind::RequiresExplicitModel {
                recommended_onboarding_model: Some("deepseek-chat".to_owned()),
            }
        );
    }

    #[test]
    fn provider_model_probe_failure_marks_transport_route_failures() {
        let config = mvp::config::LoongClawConfig::default();
        let failure = provider_model_probe_failure(
            &config,
            "provider model-list request failed on attempt 3/3: operation timed out",
        );

        assert_eq!(failure.level, ProviderModelProbeFailureLevel::Fail);
        assert_eq!(
            failure.recovery_kind,
            ProviderModelProbeRecoveryKind::TransportFailure
        );
        assert!(
            provider_model_probe_transport_failure_detail(failure.detail.as_str()),
            "transport-style failures should keep the route-focused marker in the rendered detail"
        );
    }

    #[test]
    fn provider_model_probe_failure_appends_region_hint_for_auth_failures() {
        let mut config = mvp::config::LoongClawConfig::default();
        config.provider.kind = mvp::config::ProviderKind::Minimax;
        config.provider.model = "auto".to_owned();

        let failure = provider_model_probe_failure(&config, "provider returned status 401");

        assert!(
            failure.detail.contains("https://api.minimax.io"),
            "auth-style failures should keep provider-specific endpoint guidance in the shared policy detail"
        );
        assert!(
            provider_model_probe_auth_failure_detail(failure.detail.as_str()),
            "the shared detail classifier should recognize auth-style model probe failures"
        );
    }
}
