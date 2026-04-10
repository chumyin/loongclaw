use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::Json;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Router, routing::post};
use serde_json::{Value, json};
use subtle::ConstantTimeEq;

use crate::{
    CliResult, KernelContext, config::ChannelDefaultAccountSelectionSource,
    config::LoongClawConfig, config::ResolvedMattermostChannelConfig,
};

use super::{
    ChannelDelivery, ChannelInboundMessage, ChannelOutboundTarget, ChannelOutboundTargetKind,
    ChannelPlatform, ChannelSession, ChannelTurnFeedbackPolicy,
    MATTERMOST_COMMAND_FAMILY_DESCRIPTOR,
    dispatch::{
        ChannelCommandContext, ChannelServeCommandSpec, process_inbound_with_provider,
        run_channel_serve_command_with_stop,
    },
    http::{
        ChannelOutboundHttpPolicy, build_outbound_http_client, read_json_or_text_response,
        response_body_detail, validate_outbound_http_target,
    },
    runtime::serve::ChannelServeStopHandle,
    runtime::state::ChannelOperationRuntimeTracker,
};

const MATTERMOST_JSON_CONTENT_TYPE: &str = "application/json";
const MATTERMOST_COMMENT_RESPONSE_TYPE: &str = "comment";

#[derive(Debug, Clone)]
struct ParsedMattermostOutgoingWebhook {
    token: String,
    channel_id: String,
    user_id: String,
    user_name: String,
    post_id: String,
    text: String,
}

#[derive(Clone)]
struct MattermostServeState {
    config: LoongClawConfig,
    resolved_path: PathBuf,
    resolved: ResolvedMattermostChannelConfig,
    outgoing_token: String,
    kernel_ctx: Arc<KernelContext>,
    runtime: Arc<ChannelOperationRuntimeTracker>,
}

impl MattermostServeState {
    fn new(
        config: &LoongClawConfig,
        resolved_path: &Path,
        resolved: &ResolvedMattermostChannelConfig,
        kernel_ctx: KernelContext,
        runtime: Arc<ChannelOperationRuntimeTracker>,
    ) -> CliResult<Self> {
        let outgoing_token = resolved.outgoing_token().ok_or_else(|| {
            "mattermost outgoing_token missing (set mattermost.outgoing_token or env)".to_owned()
        })?;

        Ok(Self {
            config: config.clone(),
            resolved_path: resolved_path.to_path_buf(),
            resolved: resolved.clone(),
            outgoing_token,
            kernel_ctx: Arc::new(kernel_ctx),
            runtime,
        })
    }
}

pub(super) async fn run_mattermost_send(
    resolved: &ResolvedMattermostChannelConfig,
    target_kind: ChannelOutboundTargetKind,
    target_id: &str,
    text: &str,
    policy: ChannelOutboundHttpPolicy,
) -> CliResult<()> {
    if target_kind != ChannelOutboundTargetKind::Conversation {
        return Err(format!(
            "mattermost send requires conversation target kind, got {}",
            target_kind.as_str()
        ));
    }

    let server_url = resolved.server_url().ok_or_else(|| {
        "mattermost server_url missing (set mattermost.server_url or env)".to_owned()
    })?;
    let bot_token = resolved.bot_token().ok_or_else(|| {
        "mattermost bot_token missing (set mattermost.bot_token or env)".to_owned()
    })?;
    let channel_id = target_id.trim();
    if channel_id.is_empty() {
        return Err("mattermost outbound target id is empty".to_owned());
    }

    let trimmed_server_url = server_url.trim_end_matches('/');
    let request_url = format!("{trimmed_server_url}/api/v4/posts");
    let request_url =
        validate_outbound_http_target("mattermost server_url", request_url.as_str(), policy)?;
    let request_body = json!({
        "channel_id": channel_id,
        "message": text,
    });

    let client = build_outbound_http_client("mattermost send", policy)?;
    let request = client
        .post(request_url)
        .bearer_auth(bot_token)
        .json(&request_body);
    let response = request
        .send()
        .await
        .map_err(|error| format!("mattermost send failed: {error}"))?;
    let payload = read_mattermost_json_response(response).await?;

    let message_id = payload
        .get("id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if message_id.is_none() {
        return Err(format!(
            "mattermost send did not return a post id: {payload}"
        ));
    }

    Ok(())
}

#[allow(clippy::print_stdout)]
pub(super) async fn run_mattermost_channel(
    config: &LoongClawConfig,
    resolved: &ResolvedMattermostChannelConfig,
    resolved_path: &Path,
    selected_by_default: bool,
    default_account_source: ChannelDefaultAccountSelectionSource,
    bind_override: Option<&str>,
    path_override: Option<&str>,
    kernel_ctx: KernelContext,
    runtime: Arc<ChannelOperationRuntimeTracker>,
    stop: ChannelServeStopHandle,
) -> CliResult<()> {
    let bind = resolve_mattermost_bind(bind_override)?;
    let path = resolve_mattermost_path(path_override);
    let state = MattermostServeState::new(config, resolved_path, resolved, kernel_ctx, runtime)?;
    let router = Router::new()
        .route(path.as_str(), post(mattermost_webhook_handler))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind(bind.as_str())
        .await
        .map_err(|error| format!("bind mattermost webhook listener failed: {error}"))?;

    println!(
        "mattermost channel started (config={}, configured_account={}, account={}, selected_by_default={}, default_source={}, bind={}, path={})",
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
        .map_err(|error| format!("mattermost webhook server stopped: {error}"))
}

pub(super) async fn run_mattermost_channel_with_context(
    context: ChannelCommandContext<ResolvedMattermostChannelConfig>,
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
            family: MATTERMOST_COMMAND_FAMILY_DESCRIPTOR,
        },
        validate_mattermost_security_config,
        stop,
        initialize_runtime_environment,
        move |context, kernel_ctx, runtime, stop| {
            Box::pin(async move {
                let route = context.route.clone();
                let resolved_path = context.resolved_path.clone();
                let resolved = context.resolved.clone();
                let config = context.config.clone();

                run_mattermost_channel(
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

async fn read_mattermost_json_response(response: reqwest::Response) -> CliResult<Value> {
    let (status, body, payload) = read_json_or_text_response(response, "mattermost send").await?;

    if status.is_success() {
        if payload.is_object() {
            return Ok(payload);
        }

        let detail = response_body_detail(body.as_str());
        return Err(format!(
            "mattermost send returned a non-json success payload: {detail}"
        ));
    }

    let detail = payload
        .get("message")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| response_body_detail(body.as_str()));

    Err(format!(
        "mattermost send failed with status {}: {detail}",
        status.as_u16()
    ))
}

fn resolve_mattermost_bind(bind_override: Option<&str>) -> CliResult<String> {
    let trimmed_bind = bind_override.map(str::trim);
    let bind = trimmed_bind.filter(|value| !value.is_empty());
    let bind = bind.ok_or_else(|| {
        "mattermost serve requires --bind because config does not define a local listener address"
            .to_owned()
    })?;

    Ok(bind.to_owned())
}

fn resolve_mattermost_path(path_override: Option<&str>) -> String {
    let trimmed_path = path_override.map(str::trim);
    let path = trimmed_path.filter(|value| !value.is_empty());
    match path {
        Some(path) if path.starts_with('/') => path.to_owned(),
        Some(path) => format!("/{path}"),
        None => "/".to_owned(),
    }
}

fn validate_mattermost_security_config(config: &ResolvedMattermostChannelConfig) -> CliResult<()> {
    let outgoing_token = config.outgoing_token();
    let has_outgoing_token = outgoing_token
        .as_deref()
        .map(str::trim)
        .map(|value| !value.is_empty())
        .unwrap_or(false);
    if !has_outgoing_token {
        return Err(
            "mattermost outgoing_token is required for webhook serve (set mattermost.outgoing_token or env)"
                .to_owned(),
        );
    }

    let allowed_channel_ids = &config.allowed_channel_ids;
    let has_allowed_channel_ids = !allowed_channel_ids.is_empty();
    if !has_allowed_channel_ids {
        return Err(
            "mattermost allowed_channel_ids must list at least one channel for webhook serve"
                .to_owned(),
        );
    }

    Ok(())
}

async fn mattermost_webhook_handler(
    State(state): State<MattermostServeState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let parsed = parse_mattermost_outgoing_webhook(&headers, body.as_ref());
    let parsed = match parsed {
        Ok(parsed) => parsed,
        Err(error) => {
            return build_mattermost_error_response(StatusCode::BAD_REQUEST, error);
        }
    };

    let token_result = verify_mattermost_outgoing_token(&state, &parsed);
    if let Err(error) = token_result {
        return build_mattermost_error_response(StatusCode::UNAUTHORIZED, error);
    }

    let channel_result = verify_mattermost_allowed_channel(&state, &parsed);
    if let Err(error) = channel_result {
        return build_mattermost_error_response(StatusCode::FORBIDDEN, error);
    }

    let response = process_mattermost_outgoing_webhook(&state, parsed).await;
    match response {
        Ok(response) => response,
        Err(error) => build_mattermost_error_response(StatusCode::BAD_REQUEST, error),
    }
}

fn parse_mattermost_outgoing_webhook(
    headers: &HeaderMap,
    body: &[u8],
) -> CliResult<ParsedMattermostOutgoingWebhook> {
    let content_type = headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .unwrap_or_default()
        .to_owned();
    let uses_json = content_type.contains(MATTERMOST_JSON_CONTENT_TYPE);
    if uses_json {
        return parse_mattermost_json_webhook(body);
    }

    parse_mattermost_form_webhook(body)
}

fn parse_mattermost_json_webhook(body: &[u8]) -> CliResult<ParsedMattermostOutgoingWebhook> {
    let payload: Value = serde_json::from_slice(body)
        .map_err(|error| format!("parse mattermost outgoing webhook json failed: {error}"))?;
    let token = required_mattermost_json_field(&payload, "token")?;
    let channel_id = required_mattermost_json_field(&payload, "channel_id")?;
    let user_id = required_mattermost_json_field(&payload, "user_id")?;
    let user_name = required_mattermost_json_field(&payload, "user_name")?;
    let post_id = required_mattermost_json_field(&payload, "post_id")?;
    let text = required_mattermost_json_field(&payload, "text")?;

    Ok(ParsedMattermostOutgoingWebhook {
        token,
        channel_id,
        user_id,
        user_name,
        post_id,
        text,
    })
}

fn required_mattermost_json_field(payload: &Value, field_name: &str) -> CliResult<String> {
    let field_value = payload
        .get(field_name)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("mattermost outgoing webhook missing {field_name}"))?;

    Ok(field_value.to_owned())
}

fn parse_mattermost_form_webhook(body: &[u8]) -> CliResult<ParsedMattermostOutgoingWebhook> {
    let body_text = std::str::from_utf8(body)
        .map_err(|error| format!("mattermost outgoing webhook body is not valid utf-8: {error}"))?;
    let synthetic_url_text = format!("http://localhost/?{body_text}");
    let synthetic_url = reqwest::Url::parse(synthetic_url_text.as_str())
        .map_err(|error| format!("parse mattermost outgoing webhook body failed: {error}"))?;

    let mut token = None;
    let mut channel_id = None;
    let mut user_id = None;
    let mut user_name = None;
    let mut post_id = None;
    let mut text_value = None;

    for (key, value) in synthetic_url.query_pairs() {
        let key_text = key.as_ref();
        let value_text = value.into_owned();
        match key_text {
            "token" => token = Some(value_text),
            "channel_id" => channel_id = Some(value_text),
            "user_id" => user_id = Some(value_text),
            "user_name" => user_name = Some(value_text),
            "post_id" => post_id = Some(value_text),
            "text" => text_value = Some(value_text),
            _ => {}
        }
    }

    let token = required_mattermost_form_field(token, "token")?;
    let channel_id = required_mattermost_form_field(channel_id, "channel_id")?;
    let user_id = required_mattermost_form_field(user_id, "user_id")?;
    let user_name = required_mattermost_form_field(user_name, "user_name")?;
    let post_id = required_mattermost_form_field(post_id, "post_id")?;
    let text = required_mattermost_form_field(text_value, "text")?;

    Ok(ParsedMattermostOutgoingWebhook {
        token,
        channel_id,
        user_id,
        user_name,
        post_id,
        text,
    })
}

fn required_mattermost_form_field(value: Option<String>, field_name: &str) -> CliResult<String> {
    let value = value.ok_or_else(|| format!("mattermost outgoing webhook missing {field_name}"))?;
    let trimmed_value = value.trim();
    if trimmed_value.is_empty() {
        return Err(format!("mattermost outgoing webhook {field_name} is empty"));
    }

    Ok(trimmed_value.to_owned())
}

fn verify_mattermost_outgoing_token(
    state: &MattermostServeState,
    parsed: &ParsedMattermostOutgoingWebhook,
) -> CliResult<()> {
    let expected_token = state.outgoing_token.as_bytes();
    let provided_token = parsed.token.as_bytes();
    let matches = expected_token.ct_eq(provided_token).unwrap_u8() == 1;
    if matches {
        return Ok(());
    }

    Err("invalid outgoing webhook token".to_owned())
}

fn verify_mattermost_allowed_channel(
    state: &MattermostServeState,
    parsed: &ParsedMattermostOutgoingWebhook,
) -> CliResult<()> {
    let is_allowed = state
        .resolved
        .allowed_channel_ids
        .iter()
        .any(|allowed_channel_id| allowed_channel_id.trim() == parsed.channel_id);
    if is_allowed {
        return Ok(());
    }

    Err(format!(
        "channel {} is not allowed to trigger mattermost runtime",
        parsed.channel_id
    ))
}

async fn process_mattermost_outgoing_webhook(
    state: &MattermostServeState,
    parsed: ParsedMattermostOutgoingWebhook,
) -> CliResult<Response> {
    state
        .runtime
        .mark_run_start()
        .await
        .map_err(|error| format!("mattermost runtime start failed: {error}"))?;

    let process_result = async {
        let session = ChannelSession::with_account(
            ChannelPlatform::Mattermost,
            state.resolved.account.id.as_str(),
            parsed.channel_id.as_str(),
        )
        .with_configured_account_id(state.resolved.configured_account_id.as_str())
        .with_participant_id(parsed.user_id.as_str());
        let reply_target = ChannelOutboundTarget::new(
            ChannelPlatform::Mattermost,
            ChannelOutboundTargetKind::Conversation,
            parsed.channel_id.as_str(),
        );
        let source_message_id = Some(parsed.post_id.clone());
        let sender_principal_key = Some(parsed.user_name.clone());
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

        Ok(build_mattermost_reply_response(reply.as_str()))
    }
    .await;

    if let Err(error) = state.runtime.mark_run_end().await {
        tracing::warn!(error = %error, "mattermost runtime end failed");
    }

    process_result
}

fn build_mattermost_reply_response(reply: &str) -> Response {
    let trimmed_reply = reply.trim();
    if trimmed_reply.is_empty() {
        return StatusCode::NO_CONTENT.into_response();
    }

    let payload = json!({
        "text": reply,
        "response_type": MATTERMOST_COMMENT_RESPONSE_TYPE,
    });
    (StatusCode::OK, Json(payload)).into_response()
}

fn build_mattermost_error_response(status: StatusCode, error: String) -> Response {
    let payload = json!({
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
    use std::time::{SystemTime, UNIX_EPOCH};

    fn test_resolved_mattermost_config() -> ResolvedMattermostChannelConfig {
        let config: crate::config::MattermostChannelConfig =
            serde_json::from_value(serde_json::json!({
                "enabled": true,
                "account_id": "mattermost-main",
                "server_url": "https://mattermost.example.test",
                "bot_token": "mattermost-bot-token",
                "outgoing_token": "mattermost-outgoing-token",
                "allowed_channel_ids": ["channel-town-square"]
            }))
            .expect("deserialize mattermost config");

        config
            .resolve_account(None)
            .expect("resolve mattermost config for tests")
    }

    fn temp_mattermost_test_dir(label: &str) -> PathBuf {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        std::env::temp_dir().join(format!("loongclaw-mattermost-test-{label}-{timestamp}"))
    }

    async fn build_test_serve_state(
        outgoing_token: Option<&str>,
        allowed_channel_ids: Vec<&str>,
    ) -> MattermostServeState {
        let runtime_dir = temp_mattermost_test_dir("state");
        let runtime = start_channel_operation_runtime_tracker_for_test(
            runtime_dir.as_path(),
            ChannelPlatform::Mattermost,
            "serve",
            "mattermost-test",
            "mattermost:test",
            std::process::id(),
        )
        .await
        .expect("start runtime tracker");
        let kernel_ctx =
            bootstrap_test_kernel_context("mattermost-serve-test", DEFAULT_TOKEN_TTL_S)
                .expect("bootstrap mattermost kernel context");
        let mut resolved = test_resolved_mattermost_config();
        resolved.outgoing_token = outgoing_token.map(|value| {
            serde_json::from_value(serde_json::json!(value))
                .expect("deserialize outgoing token for test state")
        });
        resolved.allowed_channel_ids = allowed_channel_ids
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>();

        MattermostServeState::new(
            &LoongClawConfig::default(),
            Path::new("/tmp/loongclaw.toml"),
            &resolved,
            kernel_ctx,
            runtime.into(),
        )
        .expect("build mattermost serve state")
    }

    #[test]
    fn parse_mattermost_form_webhook_decodes_fields() {
        let body = b"token=test-token&channel_id=channel-town-square&user_id=user-42&user_name=alice&post_id=post-9&text=hello+world";
        let parsed =
            parse_mattermost_form_webhook(body).expect("parse mattermost outgoing webhook body");

        assert_eq!(parsed.token, "test-token");
        assert_eq!(parsed.channel_id, "channel-town-square");
        assert_eq!(parsed.user_id, "user-42");
        assert_eq!(parsed.user_name, "alice");
        assert_eq!(parsed.post_id, "post-9");
        assert_eq!(parsed.text, "hello world");
    }

    #[tokio::test]
    async fn mattermost_webhook_handler_rejects_missing_token() {
        let state = build_test_serve_state(
            Some("mattermost-outgoing-token"),
            vec!["channel-town-square"],
        )
        .await;
        let body = Bytes::from_static(
            b"channel_id=channel-town-square&user_id=user-42&user_name=alice&post_id=post-9&text=hello",
        );

        let response = mattermost_webhook_handler(State(state), HeaderMap::new(), body)
            .await
            .into_response();
        let status = response.status();
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read mattermost response body");
        let payload: Value = serde_json::from_slice(&body).expect("parse mattermost error body");

        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(
            payload,
            serde_json::json!({
                "error": "mattermost outgoing webhook missing token"
            })
        );
    }

    #[tokio::test]
    async fn mattermost_webhook_handler_rejects_invalid_token() {
        let state = build_test_serve_state(
            Some("mattermost-outgoing-token"),
            vec!["channel-town-square"],
        )
        .await;
        let body = Bytes::from_static(
            b"token=wrong-token&channel_id=channel-town-square&user_id=user-42&user_name=alice&post_id=post-9&text=hello",
        );

        let response = mattermost_webhook_handler(State(state), HeaderMap::new(), body)
            .await
            .into_response();
        let status = response.status();
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read mattermost response body");
        let payload: Value = serde_json::from_slice(&body).expect("parse mattermost error body");

        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(
            payload,
            serde_json::json!({
                "error": "invalid outgoing webhook token"
            })
        );
    }

    #[tokio::test]
    async fn mattermost_webhook_handler_rejects_disallowed_channel() {
        let state =
            build_test_serve_state(Some("mattermost-outgoing-token"), vec!["channel-ops"]).await;
        let body = Bytes::from_static(
            b"token=mattermost-outgoing-token&channel_id=channel-town-square&user_id=user-42&user_name=alice&post_id=post-9&text=hello",
        );

        let response = mattermost_webhook_handler(State(state), HeaderMap::new(), body)
            .await
            .into_response();
        let status = response.status();
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read mattermost response body");
        let payload: Value = serde_json::from_slice(&body).expect("parse mattermost error body");

        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(
            payload,
            serde_json::json!({
                "error": "channel channel-town-square is not allowed to trigger mattermost runtime"
            })
        );
    }
}
