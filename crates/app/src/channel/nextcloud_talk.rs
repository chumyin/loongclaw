use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::Json;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Router, routing::post};
use hmac::{KeyInit, Mac};
use serde::Serialize;

use crate::{
    CliResult, KernelContext, config::ChannelDefaultAccountSelectionSource,
    config::LoongClawConfig, config::ResolvedNextcloudTalkChannelConfig,
};

use super::{
    ChannelDelivery, ChannelInboundMessage, ChannelOutboundTarget, ChannelOutboundTargetKind,
    ChannelPlatform, ChannelSession, ChannelTurnFeedbackPolicy,
    NEXTCLOUD_TALK_COMMAND_FAMILY_DESCRIPTOR,
    dispatch::{
        ChannelCommandContext, ChannelServeCommandSpec, process_inbound_with_provider,
        run_channel_serve_command_with_stop,
    },
    http::{ChannelOutboundHttpPolicy, build_outbound_http_client, validate_outbound_http_target},
    runtime::serve::ChannelServeStopHandle,
    runtime::state::ChannelOperationRuntimeTracker,
};

type NextcloudTalkHmacSha256 = hmac::Hmac<sha2::Sha256>;

const NEXTCLOUD_TALK_BOT_RANDOM_HEADER: &str = "X-Nextcloud-Talk-Bot-Random";
const NEXTCLOUD_TALK_BOT_SIGNATURE_HEADER: &str = "X-Nextcloud-Talk-Bot-Signature";
const NEXTCLOUD_TALK_OCS_API_REQUEST_HEADER: &str = "OCS-APIRequest";
const NEXTCLOUD_TALK_RANDOM_HEADER: &str = "x-nextcloud-talk-random";
const NEXTCLOUD_TALK_SIGNATURE_HEADER: &str = "x-nextcloud-talk-signature";

#[derive(Clone)]
struct NextcloudTalkServeState {
    config: LoongClawConfig,
    resolved_path: PathBuf,
    resolved: ResolvedNextcloudTalkChannelConfig,
    shared_secret: String,
    kernel_ctx: Arc<KernelContext>,
    runtime: Arc<ChannelOperationRuntimeTracker>,
}

impl NextcloudTalkServeState {
    fn new(
        config: &LoongClawConfig,
        resolved_path: &Path,
        resolved: &ResolvedNextcloudTalkChannelConfig,
        kernel_ctx: KernelContext,
        runtime: Arc<ChannelOperationRuntimeTracker>,
    ) -> CliResult<Self> {
        let shared_secret = resolved.shared_secret().ok_or_else(|| {
            "nextcloud_talk shared_secret missing (set nextcloud_talk.shared_secret or env)"
                .to_owned()
        })?;

        Ok(Self {
            config: config.clone(),
            resolved_path: resolved_path.to_path_buf(),
            resolved: resolved.clone(),
            shared_secret,
            kernel_ctx: Arc::new(kernel_ctx),
            runtime,
        })
    }
}

#[derive(Debug, Serialize)]
struct NextcloudTalkSendRequestBody {
    message: String,
    #[serde(rename = "referenceId")]
    reference_id: String,
    #[serde(rename = "replyTo", skip_serializing_if = "Option::is_none")]
    reply_to: Option<String>,
}

pub(super) async fn run_nextcloud_talk_send(
    resolved: &ResolvedNextcloudTalkChannelConfig,
    target_kind: ChannelOutboundTargetKind,
    target_id: &str,
    text: &str,
    policy: ChannelOutboundHttpPolicy,
) -> CliResult<()> {
    if target_kind != ChannelOutboundTargetKind::Conversation {
        return Err(format!(
            "nextcloud talk send requires conversation target kind, got {}",
            target_kind.as_str()
        ));
    }

    let server_url = resolved.server_url().ok_or_else(|| {
        "nextcloud_talk server_url missing (set nextcloud_talk.server_url or env)".to_owned()
    })?;
    let shared_secret = resolved.shared_secret().ok_or_else(|| {
        "nextcloud_talk shared_secret missing (set nextcloud_talk.shared_secret or env)".to_owned()
    })?;
    let conversation_token = target_id.trim();
    if conversation_token.is_empty() {
        return Err("nextcloud talk outbound target id is empty".to_owned());
    }

    let random_header = build_random_reference_id();
    let request_body = NextcloudTalkSendRequestBody {
        message: text.to_owned(),
        reference_id: random_header.clone(),
        reply_to: None,
    };
    let request_body_json = serde_json::to_string(&request_body)
        .map_err(|error| format!("serialize nextcloud talk request failed: {error}"))?;
    let request_signature = build_nextcloud_talk_signature(
        shared_secret.as_str(),
        random_header.as_str(),
        request_body_json.as_str(),
    )?;
    let request_url =
        build_nextcloud_talk_request_url(server_url.as_str(), conversation_token, policy)?;

    let client = build_outbound_http_client("nextcloud talk send", policy)?;
    let request = client
        .post(request_url)
        .header(reqwest::header::ACCEPT, "application/json")
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .header(NEXTCLOUD_TALK_OCS_API_REQUEST_HEADER, "true")
        .header(NEXTCLOUD_TALK_BOT_RANDOM_HEADER, random_header.as_str())
        .header(
            NEXTCLOUD_TALK_BOT_SIGNATURE_HEADER,
            request_signature.as_str(),
        )
        .body(request_body_json);
    let response = request
        .send()
        .await
        .map_err(|error| format!("nextcloud talk send failed: {error}"))?;

    ensure_nextcloud_talk_success(response).await
}

#[allow(clippy::print_stdout)] // CLI startup banner
pub(super) async fn run_nextcloud_talk_channel(
    config: &LoongClawConfig,
    resolved: &ResolvedNextcloudTalkChannelConfig,
    resolved_path: &Path,
    selected_by_default: bool,
    default_account_source: ChannelDefaultAccountSelectionSource,
    bind_override: Option<&str>,
    path_override: Option<&str>,
    kernel_ctx: KernelContext,
    runtime: Arc<ChannelOperationRuntimeTracker>,
    stop: ChannelServeStopHandle,
) -> CliResult<()> {
    let bind = resolve_nextcloud_talk_bind(bind_override)?;
    let path = resolve_nextcloud_talk_path(path_override);
    let state = NextcloudTalkServeState::new(config, resolved_path, resolved, kernel_ctx, runtime)?;
    let router = Router::new()
        .route(path.as_str(), post(nextcloud_talk_webhook_handler))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind(bind.as_str())
        .await
        .map_err(|error| format!("bind nextcloud talk listener failed: {error}"))?;

    println!(
        "nextcloud talk channel started (config={}, configured_account={}, account={}, selected_by_default={}, default_source={}, bind={}, path={})",
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
        .map_err(|error| format!("nextcloud talk webhook server stopped: {error}"))
}

pub(super) async fn run_nextcloud_talk_channel_with_context(
    context: ChannelCommandContext<ResolvedNextcloudTalkChannelConfig>,
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
            family: NEXTCLOUD_TALK_COMMAND_FAMILY_DESCRIPTOR,
        },
        validate_nextcloud_talk_security_config,
        stop,
        initialize_runtime_environment,
        move |context, kernel_ctx, runtime, stop| {
            Box::pin(async move {
                let route = context.route.clone();
                let resolved_path = context.resolved_path.clone();
                let resolved = context.resolved.clone();
                let config = context.config.clone();

                run_nextcloud_talk_channel(
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

async fn nextcloud_talk_webhook_handler(
    State(state): State<NextcloudTalkServeState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let verification_result = verify_nextcloud_talk_request(&state, &headers, body.as_ref());
    if let Err(error) = verification_result {
        return (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({ "error": error })),
        )
            .into_response();
    }

    let process_result = process_nextcloud_talk_webhook(&state, body).await;
    match process_result {
        Ok(()) => StatusCode::OK.into_response(),
        Err(error) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": error })),
        )
            .into_response(),
    }
}

fn build_random_reference_id() -> String {
    let random_bytes = rand::random::<[u8; 32]>();
    hex::encode(random_bytes)
}

fn build_nextcloud_talk_request_url(
    server_url: &str,
    conversation_token: &str,
    policy: ChannelOutboundHttpPolicy,
) -> CliResult<String> {
    let mut url = validate_outbound_http_target("nextcloud talk server_url", server_url, policy)?;
    let mut path_segments = url.path_segments_mut().map_err(|_path_error| {
        "nextcloud talk server_url cannot be used as a hierarchical base url".to_owned()
    })?;
    path_segments.pop_if_empty();
    path_segments.push("ocs");
    path_segments.push("v2.php");
    path_segments.push("apps");
    path_segments.push("spreed");
    path_segments.push("api");
    path_segments.push("v1");
    path_segments.push("bot");
    path_segments.push(conversation_token);
    path_segments.push("message");
    drop(path_segments);
    Ok(url.to_string())
}

fn build_nextcloud_talk_signature(
    shared_secret: &str,
    random_header: &str,
    request_body_json: &str,
) -> CliResult<String> {
    let mut mac = NextcloudTalkHmacSha256::new_from_slice(shared_secret.as_bytes())
        .map_err(|error| format!("build nextcloud talk signature failed: {error}"))?;
    mac.update(random_header.as_bytes());
    mac.update(request_body_json.as_bytes());
    let signature = mac.finalize().into_bytes();
    Ok(hex::encode(signature))
}

async fn ensure_nextcloud_talk_success(response: reqwest::Response) -> CliResult<()> {
    let status = response.status();
    if status.is_success() {
        return Ok(());
    }

    let body = response
        .text()
        .await
        .map_err(|error| format!("read nextcloud talk error response failed: {error}"))?;
    let trimmed_body = body.trim();
    let detail = if trimmed_body.is_empty() {
        "empty response body".to_owned()
    } else {
        trimmed_body.to_owned()
    };

    Err(format!(
        "nextcloud talk send failed with status {}: {detail}",
        status.as_u16()
    ))
}

fn resolve_nextcloud_talk_bind(bind_override: Option<&str>) -> CliResult<String> {
    let bind = bind_override
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            "nextcloud-talk serve requires --bind because nextcloud_talk config does not define a local listener address"
                .to_owned()
        })?;

    Ok(bind.to_owned())
}

fn resolve_nextcloud_talk_path(path_override: Option<&str>) -> String {
    let explicit_path = path_override
        .map(str::trim)
        .filter(|value| !value.is_empty());
    match explicit_path {
        Some(explicit_path) if explicit_path.starts_with('/') => explicit_path.to_owned(),
        Some(explicit_path) => format!("/{explicit_path}"),
        None => "/".to_owned(),
    }
}

fn validate_nextcloud_talk_security_config(
    config: &ResolvedNextcloudTalkChannelConfig,
) -> CliResult<()> {
    let has_server_url = config
        .server_url()
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false);
    if !has_server_url {
        return Err(
            "nextcloud_talk server_url is required for callback verification (set nextcloud_talk.server_url or env)"
                .to_owned(),
        );
    }

    let has_shared_secret = config
        .shared_secret()
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false);
    if !has_shared_secret {
        return Err(
            "nextcloud_talk shared_secret is required for callback verification (set nextcloud_talk.shared_secret or env)"
                .to_owned(),
        );
    }

    Ok(())
}

fn verify_nextcloud_talk_request(
    state: &NextcloudTalkServeState,
    headers: &HeaderMap,
    body: &[u8],
) -> CliResult<()> {
    let random_header = headers
        .get(NEXTCLOUD_TALK_RANDOM_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "missing X-Nextcloud-Talk-Random header".to_owned())?;
    let provided_signature = headers
        .get(NEXTCLOUD_TALK_SIGNATURE_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "missing X-Nextcloud-Talk-Signature header".to_owned())?;
    let expected_signature = build_nextcloud_talk_signature(
        state.shared_secret.as_str(),
        random_header,
        std::str::from_utf8(body)
            .map_err(|error| format!("nextcloud talk body is not valid utf-8: {error}"))?,
    )?;
    let valid_length = expected_signature.len() == provided_signature.len();
    let valid_bytes =
        crate::crypto::timing_safe_eq(expected_signature.as_bytes(), provided_signature.as_bytes());
    if !valid_length || !valid_bytes {
        return Err("invalid X-Nextcloud-Talk-Signature header".to_owned());
    }

    Ok(())
}

async fn process_nextcloud_talk_webhook(
    state: &NextcloudTalkServeState,
    body: Bytes,
) -> CliResult<()> {
    let payload: serde_json::Value = serde_json::from_slice(body.as_ref())
        .map_err(|error| format!("parse nextcloud talk webhook payload failed: {error}"))?;
    let inbound_message = build_nextcloud_talk_inbound_message(state, &payload)?;

    state
        .runtime
        .mark_run_start()
        .await
        .map_err(|error| format!("nextcloud talk runtime start failed: {error}"))?;

    let process_result = async {
        let reply = process_inbound_with_provider(
            &state.config,
            Some(state.resolved_path.as_path()),
            &inbound_message,
            state.kernel_ctx.as_ref(),
            ChannelTurnFeedbackPolicy::final_trace_significant(),
        )
        .await?;
        let reply_to = inbound_message.delivery.source_message_id.as_deref();
        send_nextcloud_talk_reply(
            state,
            inbound_message.session.conversation_id.as_str(),
            reply.as_str(),
            reply_to,
        )
        .await
    }
    .await;

    if let Err(error) = state.runtime.mark_run_end().await {
        tracing::warn!(error = %error, "nextcloud talk runtime end failed");
    }

    process_result
}

fn build_nextcloud_talk_inbound_message(
    state: &NextcloudTalkServeState,
    payload: &serde_json::Value,
) -> CliResult<ChannelInboundMessage> {
    let event_type = payload
        .get("type")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    if event_type != "Create" {
        return Err(format!(
            "unsupported nextcloud talk event type `{event_type}`"
        ));
    }

    let actor = payload
        .get("actor")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| "nextcloud talk payload missing actor object".to_owned())?;
    let object = payload
        .get("object")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| "nextcloud talk payload missing object".to_owned())?;
    let target = payload
        .get("target")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| "nextcloud talk payload missing target object".to_owned())?;
    let conversation_id = target
        .get("id")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "nextcloud talk target.id is missing".to_owned())?;
    let source_message_id = object
        .get("id")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    let participant_id = actor
        .get("id")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    let text = resolve_nextcloud_talk_message_text(object)?;
    let session = ChannelSession::with_account(
        ChannelPlatform::NextcloudTalk,
        state.resolved.account.id.as_str(),
        conversation_id,
    )
    .with_configured_account_id(state.resolved.configured_account_id.as_str());
    let session = match participant_id.as_deref() {
        Some(participant_id) => session.with_participant_id(participant_id),
        None => session,
    };
    let reply_target = ChannelOutboundTarget::new(
        ChannelPlatform::NextcloudTalk,
        ChannelOutboundTargetKind::Conversation,
        conversation_id,
    );

    Ok(ChannelInboundMessage {
        session,
        reply_target,
        text,
        delivery: ChannelDelivery {
            ack_cursor: None,
            source_message_id,
            sender_principal_key: participant_id,
            thread_root_id: None,
            parent_message_id: None,
            resources: Vec::new(),
            feishu_callback: None,
        },
    })
}

fn resolve_nextcloud_talk_message_text(
    object: &serde_json::Map<String, serde_json::Value>,
) -> CliResult<String> {
    let raw_content = object
        .get("content")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "nextcloud talk object.content is missing".to_owned())?;
    let parsed_content = serde_json::from_str::<serde_json::Value>(raw_content).ok();
    if let Some(parsed_content) = parsed_content {
        let message = parsed_content
            .get("message")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty());
        if let Some(message) = message {
            return Ok(message.to_owned());
        }
    }

    Ok(raw_content.to_owned())
}

async fn send_nextcloud_talk_reply(
    state: &NextcloudTalkServeState,
    conversation_id: &str,
    text: &str,
    reply_to: Option<&str>,
) -> CliResult<()> {
    let server_url = state.resolved.server_url().ok_or_else(|| {
        "nextcloud_talk server_url missing (set nextcloud_talk.server_url or env)".to_owned()
    })?;
    let request_url = build_nextcloud_talk_request_url(
        server_url.as_str(),
        conversation_id,
        super::http::outbound_http_policy_from_config(&state.config),
    )?;
    let random_header = build_random_reference_id();
    let request_body = NextcloudTalkSendRequestBody {
        message: text.to_owned(),
        reference_id: random_header.clone(),
        reply_to: reply_to.map(str::to_owned),
    };
    let request_body_json = serde_json::to_string(&request_body)
        .map_err(|error| format!("serialize nextcloud talk reply failed: {error}"))?;
    let request_signature = build_nextcloud_talk_signature(
        state.shared_secret.as_str(),
        random_header.as_str(),
        request_body_json.as_str(),
    )?;
    let client = build_outbound_http_client(
        "nextcloud talk reply",
        super::http::outbound_http_policy_from_config(&state.config),
    )?;
    let response = client
        .post(request_url)
        .header(reqwest::header::ACCEPT, "application/json")
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .header(NEXTCLOUD_TALK_OCS_API_REQUEST_HEADER, "true")
        .header(NEXTCLOUD_TALK_BOT_RANDOM_HEADER, random_header.as_str())
        .header(
            NEXTCLOUD_TALK_BOT_SIGNATURE_HEADER,
            request_signature.as_str(),
        )
        .body(request_body_json)
        .send()
        .await
        .map_err(|error| format!("nextcloud talk reply failed: {error}"))?;

    ensure_nextcloud_talk_success(response).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channel::runtime::state::start_channel_operation_runtime_tracker_for_test;
    use crate::context::{DEFAULT_TOKEN_TTL_S, bootstrap_test_kernel_context};
    use axum::body::to_bytes;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn build_nextcloud_talk_request_url_preserves_base_path() {
        let policy = ChannelOutboundHttpPolicy {
            allow_private_hosts: false,
        };
        let request_url = build_nextcloud_talk_request_url(
            "https://cloud.example.test/nextcloud",
            "room-token",
            policy,
        )
        .expect("build nextcloud talk request url");

        assert_eq!(
            request_url,
            "https://cloud.example.test/nextcloud/ocs/v2.php/apps/spreed/api/v1/bot/room-token/message"
        );
    }

    #[test]
    fn build_nextcloud_talk_signature_matches_reference_vector() {
        let signature = build_nextcloud_talk_signature(
            "shared-secret",
            "0123456789abcdef",
            "{\"message\":\"hello\",\"referenceId\":\"abc123\"}",
        )
        .expect("build nextcloud talk signature");

        assert_eq!(
            signature,
            "70194893e46ad651aac6e3d23beb9285c984894876e6420fac105c5be9edd0bf"
        );
    }

    fn test_resolved_nextcloud_talk_config() -> ResolvedNextcloudTalkChannelConfig {
        let config: crate::config::NextcloudTalkChannelConfig =
            serde_json::from_value(serde_json::json!({
                "enabled": true,
                "account_id": "talk-bot",
                "server_url": "https://cloud.example.test",
                "shared_secret": "nextcloud-shared-secret"
            }))
            .expect("deserialize nextcloud talk config");

        config
            .resolve_account(None)
            .expect("resolve nextcloud talk config for tests")
    }

    fn temp_nextcloud_talk_test_dir(label: &str) -> PathBuf {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        std::env::temp_dir().join(format!("loongclaw-nextcloud-talk-test-{label}-{timestamp}"))
    }

    async fn build_test_serve_state() -> NextcloudTalkServeState {
        let runtime_dir = temp_nextcloud_talk_test_dir("state");
        let runtime = start_channel_operation_runtime_tracker_for_test(
            runtime_dir.as_path(),
            ChannelPlatform::NextcloudTalk,
            "serve",
            "nextcloud-talk-test",
            "nextcloud-talk:test",
            std::process::id(),
        )
        .await
        .expect("start nextcloud talk runtime tracker");
        let kernel_ctx =
            bootstrap_test_kernel_context("nextcloud-talk-serve-test", DEFAULT_TOKEN_TTL_S)
                .expect("bootstrap nextcloud talk kernel context");
        let resolved = test_resolved_nextcloud_talk_config();

        NextcloudTalkServeState::new(
            &LoongClawConfig::default(),
            Path::new("/tmp/loongclaw.toml"),
            &resolved,
            kernel_ctx,
            runtime.into(),
        )
        .expect("build nextcloud talk serve state")
    }

    #[tokio::test]
    async fn nextcloud_talk_webhook_handler_rejects_missing_signature_headers() {
        let state = build_test_serve_state().await;
        let response = nextcloud_talk_webhook_handler(
            State(state),
            HeaderMap::new(),
            Bytes::from_static(br#"{}"#),
        )
        .await
        .into_response();
        let status = response.status();
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read nextcloud talk response body");
        let payload: serde_json::Value =
            serde_json::from_slice(&body).expect("parse nextcloud talk error payload");

        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(
            payload,
            serde_json::json!({ "error": "missing X-Nextcloud-Talk-Random header" })
        );
    }

    #[tokio::test]
    async fn nextcloud_talk_webhook_handler_rejects_invalid_signature_header() {
        let state = build_test_serve_state().await;
        let mut headers = HeaderMap::new();
        headers.insert(
            NEXTCLOUD_TALK_RANDOM_HEADER,
            "0123456789abcdef".parse().expect("header"),
        );
        headers.insert(
            NEXTCLOUD_TALK_SIGNATURE_HEADER,
            "bad-signature".parse().expect("header"),
        );
        let response =
            nextcloud_talk_webhook_handler(State(state), headers, Bytes::from_static(br#"{}"#))
                .await
                .into_response();
        let status = response.status();
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read nextcloud talk response body");
        let payload: serde_json::Value =
            serde_json::from_slice(&body).expect("parse nextcloud talk error payload");

        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(
            payload,
            serde_json::json!({ "error": "invalid X-Nextcloud-Talk-Signature header" })
        );
    }
}
