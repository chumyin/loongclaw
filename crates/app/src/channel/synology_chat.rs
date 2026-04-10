use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::Json;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Router, routing::post};
use serde::Serialize;
use subtle::ConstantTimeEq;

use crate::{
    CliResult, KernelContext, config::ChannelDefaultAccountSelectionSource,
    config::LoongClawConfig, config::ResolvedSynologyChatChannelConfig,
};

use super::{
    ChannelDelivery, ChannelInboundMessage, ChannelOutboundTarget, ChannelOutboundTargetKind,
    ChannelPlatform, ChannelSession, ChannelTurnFeedbackPolicy,
    SYNOLOGY_CHAT_COMMAND_FAMILY_DESCRIPTOR,
    dispatch::{
        ChannelCommandContext, ChannelServeCommandSpec, process_inbound_with_provider,
        run_channel_serve_command_with_stop,
    },
    http::{ChannelOutboundHttpPolicy, build_outbound_http_client, validate_outbound_http_target},
    runtime::serve::ChannelServeStopHandle,
    runtime::state::ChannelOperationRuntimeTracker,
};

#[derive(Debug, Serialize)]
struct SynologyChatWebhookPayload {
    text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    user_ids: Option<Vec<u64>>,
}

#[derive(Debug, Clone)]
struct ParsedSynologyChatOutgoingWebhook {
    token: String,
    channel_id: String,
    user_id: u64,
    user_id_text: String,
    username: String,
    post_id: Option<String>,
    text: String,
}

#[derive(Clone)]
struct SynologyChatServeState {
    config: LoongClawConfig,
    resolved_path: PathBuf,
    resolved: ResolvedSynologyChatChannelConfig,
    token: String,
    kernel_ctx: Arc<KernelContext>,
    runtime: Arc<ChannelOperationRuntimeTracker>,
}

impl SynologyChatServeState {
    fn new(
        config: &LoongClawConfig,
        resolved_path: &Path,
        resolved: &ResolvedSynologyChatChannelConfig,
        kernel_ctx: KernelContext,
        runtime: Arc<ChannelOperationRuntimeTracker>,
    ) -> CliResult<Self> {
        let token = resolved.token().ok_or_else(|| {
            "synology_chat token missing (set synology_chat.token or env)".to_owned()
        })?;

        Ok(Self {
            config: config.clone(),
            resolved_path: resolved_path.to_path_buf(),
            resolved: resolved.clone(),
            token,
            kernel_ctx: Arc::new(kernel_ctx),
            runtime,
        })
    }
}

pub(super) async fn run_synology_chat_send(
    resolved: &ResolvedSynologyChatChannelConfig,
    target_kind: ChannelOutboundTargetKind,
    target_id: Option<&str>,
    text: &str,
    policy: ChannelOutboundHttpPolicy,
) -> CliResult<()> {
    if target_kind != ChannelOutboundTargetKind::Address {
        return Err(format!(
            "synology chat send requires address target kind, got {}",
            target_kind.as_str()
        ));
    }

    let incoming_url = resolved.incoming_url().ok_or_else(|| {
        "synology_chat incoming_url missing (set synology_chat.incoming_url or env)".to_owned()
    })?;
    let request_url =
        validate_outbound_http_target("synology chat incoming_url", incoming_url.as_str(), policy)?;
    let target_user_id = parse_synology_chat_target_user_id(target_id)?;
    let request_payload_json = build_synology_chat_payload_json(text, target_user_id)?;

    let client = build_outbound_http_client("synology chat send", policy)?;
    let request = client
        .post(request_url)
        .form(&[("payload", request_payload_json)]);
    let response = request
        .send()
        .await
        .map_err(|error| format!("synology chat send failed: {error}"))?;

    ensure_synology_chat_success(response).await
}

#[allow(clippy::print_stdout)] // CLI startup banner
pub(super) async fn run_synology_chat_channel(
    config: &LoongClawConfig,
    resolved: &ResolvedSynologyChatChannelConfig,
    resolved_path: &Path,
    selected_by_default: bool,
    default_account_source: ChannelDefaultAccountSelectionSource,
    bind_override: Option<&str>,
    path_override: Option<&str>,
    kernel_ctx: KernelContext,
    runtime: Arc<ChannelOperationRuntimeTracker>,
    stop: ChannelServeStopHandle,
) -> CliResult<()> {
    let bind = resolve_synology_chat_bind(bind_override)?;
    let path = resolve_synology_chat_path(path_override);
    let state = SynologyChatServeState::new(config, resolved_path, resolved, kernel_ctx, runtime)?;
    let router = Router::new()
        .route(path.as_str(), post(synology_chat_webhook_handler))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind(bind.as_str())
        .await
        .map_err(|error| format!("bind synology chat webhook listener failed: {error}"))?;

    println!(
        "synology chat channel started (config={}, configured_account={}, account={}, selected_by_default={}, default_source={}, bind={}, path={})",
        resolved_path.display(),
        resolved.configured_account_id,
        resolved.account.label,
        selected_by_default,
        default_account_source.as_str(),
        bind,
        path
    );

    axum::serve(listener, router)
        .with_graceful_shutdown(async move {
            stop.wait().await;
        })
        .await
        .map_err(|error| format!("synology chat webhook server stopped: {error}"))
}

pub(super) async fn run_synology_chat_channel_with_context(
    context: ChannelCommandContext<ResolvedSynologyChatChannelConfig>,
    bind_override: Option<&str>,
    path_override: Option<&str>,
    stop: ChannelServeStopHandle,
    initialize_runtime_environment: bool,
) -> CliResult<()> {
    let bind_override = bind_override.map(str::to_owned);
    let path_override = path_override.map(str::to_owned);

    run_channel_serve_command_with_stop(
        context,
        ChannelServeCommandSpec {
            family: SYNOLOGY_CHAT_COMMAND_FAMILY_DESCRIPTOR,
        },
        validate_synology_chat_security_config,
        stop,
        initialize_runtime_environment,
        move |context, kernel_ctx, runtime, stop| {
            Box::pin(async move {
                let route = context.route.clone();
                let resolved_path = context.resolved_path.clone();
                let resolved = context.resolved.clone();
                let config = context.config.clone();

                run_synology_chat_channel(
                    &config,
                    &resolved,
                    &resolved_path,
                    route.selected_by_default(),
                    route.default_account_source,
                    bind_override.as_deref(),
                    path_override.as_deref(),
                    kernel_ctx,
                    runtime,
                    stop,
                )
                .await
            })
        },
    )
    .await
}

fn parse_synology_chat_target_user_id(target_id: Option<&str>) -> CliResult<Option<u64>> {
    let trimmed_target_id = target_id.map(str::trim);
    let target_id = trimmed_target_id.filter(|value| !value.is_empty());
    let Some(target_id) = target_id else {
        return Ok(None);
    };

    let user_id = target_id.parse::<u64>().map_err(|error| {
        format!(
            "synology chat target user id must be a numeric address, got `{target_id}`: {error}"
        )
    })?;
    Ok(Some(user_id))
}

fn build_synology_chat_payload_json(text: &str, target_user_id: Option<u64>) -> CliResult<String> {
    let user_ids = target_user_id.map(|user_id| vec![user_id]);
    let payload = SynologyChatWebhookPayload {
        text: text.to_owned(),
        user_ids,
    };
    serde_json::to_string(&payload)
        .map_err(|error| format!("serialize synology chat webhook payload failed: {error}"))
}

async fn ensure_synology_chat_success(response: reqwest::Response) -> CliResult<()> {
    let status = response.status();
    if status.is_success() {
        return Ok(());
    }

    let body = response
        .text()
        .await
        .map_err(|error| format!("read synology chat error response failed: {error}"))?;
    let trimmed_body = body.trim();
    let detail = if trimmed_body.is_empty() {
        "empty response body".to_owned()
    } else {
        trimmed_body.to_owned()
    };

    Err(format!(
        "synology chat send failed with status {}: {detail}",
        status.as_u16()
    ))
}

async fn synology_chat_webhook_handler(
    State(state): State<SynologyChatServeState>,
    body: Bytes,
) -> Response {
    let parsed = parse_synology_chat_outgoing_webhook(body.as_ref());
    let parsed = match parsed {
        Ok(parsed) => parsed,
        Err(error) => {
            return build_synology_chat_error_response(StatusCode::BAD_REQUEST, error);
        }
    };

    let token_result = verify_synology_chat_token(&state, &parsed);
    if let Err(error) = token_result {
        return build_synology_chat_error_response(StatusCode::UNAUTHORIZED, error);
    }

    let user_result = verify_synology_chat_allowed_user(&state, &parsed);
    if let Err(error) = user_result {
        return build_synology_chat_error_response(StatusCode::FORBIDDEN, error);
    }

    let response = process_synology_chat_outgoing_webhook(&state, parsed).await;
    match response {
        Ok(response) => response,
        Err(error) => build_synology_chat_error_response(StatusCode::BAD_REQUEST, error),
    }
}

fn resolve_synology_chat_bind(bind_override: Option<&str>) -> CliResult<String> {
    let trimmed_bind = bind_override.map(str::trim);
    let bind = trimmed_bind.filter(|value| !value.is_empty());
    let bind = bind.ok_or_else(|| {
        "synology chat serve requires --bind because config does not define a local listener address"
            .to_owned()
    })?;

    Ok(bind.to_owned())
}

fn resolve_synology_chat_path(path_override: Option<&str>) -> String {
    let trimmed_path = path_override.map(str::trim);
    let path = trimmed_path.filter(|value| !value.is_empty());
    match path {
        Some(path) if path.starts_with('/') => path.to_owned(),
        Some(path) => format!("/{path}"),
        None => "/".to_owned(),
    }
}

fn validate_synology_chat_security_config(
    config: &ResolvedSynologyChatChannelConfig,
) -> CliResult<()> {
    let token = config.token();
    let has_token = token
        .as_deref()
        .map(str::trim)
        .map(|value| !value.is_empty())
        .unwrap_or(false);
    if !has_token {
        return Err(
            "synology_chat token is required for outgoing webhook serve (set synology_chat.token or env)"
                .to_owned(),
        );
    }

    Ok(())
}

fn parse_synology_chat_outgoing_webhook(
    body: &[u8],
) -> CliResult<ParsedSynologyChatOutgoingWebhook> {
    let body_text = std::str::from_utf8(body).map_err(|error| {
        format!("synology chat outgoing webhook body is not valid utf-8: {error}")
    })?;
    let synthetic_url_text = format!("http://localhost/?{body_text}");
    let synthetic_url = reqwest::Url::parse(synthetic_url_text.as_str())
        .map_err(|error| format!("parse synology chat outgoing webhook body failed: {error}"))?;

    let mut token = None;
    let mut channel_id = None;
    let mut user_id_text = None;
    let mut username = None;
    let mut post_id = None;
    let mut text_value = None;

    for (key, value) in synthetic_url.query_pairs() {
        let key_text = key.into_owned();
        let value_text = value.into_owned();
        match key_text.as_str() {
            "token" => token = Some(value_text),
            "channel_id" => channel_id = Some(value_text),
            "user_id" => user_id_text = Some(value_text),
            "username" => username = Some(value_text),
            "post_id" => post_id = Some(value_text),
            "text" => text_value = Some(value_text),
            _ => {}
        }
    }

    let token = required_synology_field(token, "token")?;
    let channel_id = required_synology_field(channel_id, "channel_id")?;
    let user_id_text = required_synology_field(user_id_text, "user_id")?;
    let username = required_synology_field(username, "username")?;
    let text = required_synology_field(text_value, "text")?;
    let user_id = user_id_text.parse::<u64>().map_err(|error| {
        format!(
            "synology chat outgoing webhook user_id must be numeric, got `{user_id_text}`: {error}"
        )
    })?;
    let post_id = post_id.map(|value| value.trim().to_owned());
    let post_id = post_id.filter(|value| !value.is_empty());

    Ok(ParsedSynologyChatOutgoingWebhook {
        token,
        channel_id,
        user_id,
        user_id_text,
        username,
        post_id,
        text,
    })
}

fn required_synology_field(value: Option<String>, field_name: &str) -> CliResult<String> {
    let value =
        value.ok_or_else(|| format!("synology chat outgoing webhook missing {field_name}"))?;
    let trimmed_value = value.trim();
    if trimmed_value.is_empty() {
        return Err(format!(
            "synology chat outgoing webhook {field_name} is empty"
        ));
    }

    Ok(trimmed_value.to_owned())
}

fn verify_synology_chat_token(
    state: &SynologyChatServeState,
    parsed: &ParsedSynologyChatOutgoingWebhook,
) -> CliResult<()> {
    let expected_token = state.token.as_bytes();
    let provided_token = parsed.token.as_bytes();
    let matches = expected_token.ct_eq(provided_token).unwrap_u8() == 1;
    if matches {
        return Ok(());
    }

    Err("invalid outgoing webhook token".to_owned())
}

fn verify_synology_chat_allowed_user(
    state: &SynologyChatServeState,
    parsed: &ParsedSynologyChatOutgoingWebhook,
) -> CliResult<()> {
    let allowed_user_ids = &state.resolved.allowed_user_ids;
    if allowed_user_ids.is_empty() {
        return Ok(());
    }

    let is_allowed = allowed_user_ids
        .iter()
        .copied()
        .any(|allowed_user_id| allowed_user_id == parsed.user_id);
    if is_allowed {
        return Ok(());
    }

    Err(format!(
        "user {} is not allowed to trigger synology chat runtime",
        parsed.user_id
    ))
}

async fn process_synology_chat_outgoing_webhook(
    state: &SynologyChatServeState,
    parsed: ParsedSynologyChatOutgoingWebhook,
) -> CliResult<Response> {
    state
        .runtime
        .mark_run_start()
        .await
        .map_err(|error| format!("synology chat runtime start failed: {error}"))?;

    let process_result = async {
        let session = ChannelSession::with_account(
            ChannelPlatform::SynologyChat,
            state.resolved.account.id.as_str(),
            parsed.channel_id.as_str(),
        )
        .with_configured_account_id(state.resolved.configured_account_id.as_str())
        .with_participant_id(parsed.user_id_text.as_str());
        let reply_target = ChannelOutboundTarget::new(
            ChannelPlatform::SynologyChat,
            ChannelOutboundTargetKind::Endpoint,
            state.resolved.account.id.as_str(),
        );
        let source_message_id = parsed.post_id.clone();
        let sender_principal_key = Some(parsed.username.clone());
        let channel_message = ChannelInboundMessage {
            session,
            reply_target,
            text: parsed.text,
            delivery: ChannelDelivery {
                ack_cursor: None,
                source_message_id,
                sender_principal_key,
                thread_root_id: None,
                parent_message_id: None,
                resources: Vec::new(),
                feishu_callback: None,
            },
        };

        let reply = process_inbound_with_provider(
            &state.config,
            Some(state.resolved_path.as_path()),
            &channel_message,
            state.kernel_ctx.as_ref(),
            ChannelTurnFeedbackPolicy::final_trace_significant(),
        )
        .await?;

        Ok(build_synology_chat_reply_response(reply.as_str()))
    }
    .await;

    if let Err(error) = state.runtime.mark_run_end().await {
        tracing::warn!(error = %error, "synology chat runtime end failed");
    }

    process_result
}

fn build_synology_chat_reply_response(reply: &str) -> Response {
    let trimmed_reply = reply.trim();
    if trimmed_reply.is_empty() {
        return StatusCode::NO_CONTENT.into_response();
    }

    let payload = serde_json::json!({
        "text": reply,
    });
    (StatusCode::OK, Json(payload)).into_response()
}

fn build_synology_chat_error_response(status: StatusCode, error: String) -> Response {
    let payload = serde_json::json!({
        "error": error,
    });
    (status, Json(payload)).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channel::runtime::state::start_channel_operation_runtime_tracker_for_test;
    use crate::context::{DEFAULT_TOKEN_TTL_S, bootstrap_test_kernel_context};
    use axum::body::to_bytes;
    use serde_json::Value;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn test_resolved_synology_chat_config() -> ResolvedSynologyChatChannelConfig {
        let config: crate::config::SynologyChatChannelConfig =
            serde_json::from_value(serde_json::json!({
                "enabled": true,
                "account_id": "synology-main",
                "token": "synology-outgoing-token",
                "incoming_url": "https://chat.example.test/webhook/incoming",
                "allowed_user_ids": [7, 42]
            }))
            .expect("deserialize synology chat config");

        config
            .resolve_account(None)
            .expect("resolve synology chat config for tests")
    }

    fn temp_synology_chat_test_dir(label: &str) -> PathBuf {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        std::env::temp_dir().join(format!("loongclaw-synology-chat-test-{label}-{timestamp}"))
    }

    async fn build_test_serve_state(
        token: Option<&str>,
        allowed_user_ids: Vec<u64>,
    ) -> SynologyChatServeState {
        let runtime_dir = temp_synology_chat_test_dir("state");
        let runtime = start_channel_operation_runtime_tracker_for_test(
            runtime_dir.as_path(),
            ChannelPlatform::SynologyChat,
            "serve",
            "synology-chat-test",
            "synology-chat:test",
            std::process::id(),
        )
        .await
        .expect("start runtime tracker");
        let kernel_ctx =
            bootstrap_test_kernel_context("synology-chat-serve-test", DEFAULT_TOKEN_TTL_S)
                .expect("bootstrap synology chat kernel context");
        let mut resolved = test_resolved_synology_chat_config();
        resolved.token = token.map(|value| {
            serde_json::from_value(serde_json::json!(value))
                .expect("deserialize token for test state")
        });
        resolved.allowed_user_ids = allowed_user_ids;

        SynologyChatServeState::new(
            &LoongClawConfig::default(),
            std::path::Path::new("/tmp/loongclaw.toml"),
            &resolved,
            kernel_ctx,
            runtime.into(),
        )
        .expect("build synology chat serve state")
    }

    #[test]
    fn build_synology_chat_payload_json_omits_user_ids_without_target() {
        let payload_json = build_synology_chat_payload_json("hello synology", None)
            .expect("build synology chat payload json");

        assert_eq!(payload_json, "{\"text\":\"hello synology\"}");
    }

    #[test]
    fn build_synology_chat_payload_json_includes_target_user_id() {
        let payload_json = build_synology_chat_payload_json("hello synology", Some(42))
            .expect("build synology chat payload json");

        assert_eq!(
            payload_json,
            "{\"text\":\"hello synology\",\"user_ids\":[42]}"
        );
    }

    #[test]
    fn parse_synology_chat_target_user_id_rejects_non_numeric_target() {
        let error = parse_synology_chat_target_user_id(Some("user-abc"))
            .expect_err("non-numeric target should fail");

        assert!(
            error.contains("synology chat target user id must be a numeric address"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn parse_synology_chat_outgoing_webhook_decodes_form_fields() {
        let body = b"token=test-token&channel_id=chan-1&user_id=42&username=alice&post_id=post-9&text=hello+world";
        let parsed = parse_synology_chat_outgoing_webhook(body)
            .expect("parse synology outgoing webhook body");

        assert_eq!(parsed.token, "test-token");
        assert_eq!(parsed.channel_id, "chan-1");
        assert_eq!(parsed.user_id, 42);
        assert_eq!(parsed.username, "alice");
        assert_eq!(parsed.post_id.as_deref(), Some("post-9"));
        assert_eq!(parsed.text, "hello world");
    }

    #[tokio::test]
    async fn synology_chat_webhook_handler_rejects_missing_token_field() {
        let state = build_test_serve_state(Some("synology-outgoing-token"), Vec::new()).await;
        let body = Bytes::from_static(
            b"channel_id=chan-1&user_id=42&username=alice&post_id=post-9&text=hello",
        );

        let response = synology_chat_webhook_handler(State(state), body)
            .await
            .into_response();
        let status = response.status();
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read synology response body");
        let payload: Value = serde_json::from_slice(&body).expect("parse synology error body");

        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(
            payload,
            serde_json::json!({
                "error": "synology chat outgoing webhook missing token"
            })
        );
    }

    #[tokio::test]
    async fn synology_chat_webhook_handler_rejects_invalid_token() {
        let state = build_test_serve_state(Some("synology-outgoing-token"), Vec::new()).await;
        let body = Bytes::from_static(
            b"token=wrong-token&channel_id=chan-1&user_id=42&username=alice&post_id=post-9&text=hello",
        );

        let response = synology_chat_webhook_handler(State(state), body)
            .await
            .into_response();
        let status = response.status();
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read synology response body");
        let payload: Value = serde_json::from_slice(&body).expect("parse synology error body");

        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(
            payload,
            serde_json::json!({
                "error": "invalid outgoing webhook token"
            })
        );
    }

    #[tokio::test]
    async fn synology_chat_webhook_handler_rejects_disallowed_user() {
        let state = build_test_serve_state(Some("synology-outgoing-token"), vec![7]).await;
        let body = Bytes::from_static(
            b"token=synology-outgoing-token&channel_id=chan-1&user_id=42&username=alice&post_id=post-9&text=hello",
        );

        let response = synology_chat_webhook_handler(State(state), body)
            .await
            .into_response();
        let status = response.status();
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read synology response body");
        let payload: Value = serde_json::from_slice(&body).expect("parse synology error body");

        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(
            payload,
            serde_json::json!({
                "error": "user 42 is not allowed to trigger synology chat runtime"
            })
        );
    }
}
