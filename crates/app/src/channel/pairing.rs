use serde::Serialize;

use crate::config::ChannelPairingMode;

#[cfg(feature = "memory-sqlite")]
use crate::memory::runtime_config::MemoryRuntimeConfig;
#[cfg(feature = "memory-sqlite")]
use crate::session::repository::ChannelPairingRequestRecord;
#[cfg(feature = "memory-sqlite")]
use crate::session::repository::ChannelPairingRequestStatus;
#[cfg(feature = "memory-sqlite")]
use crate::session::repository::NewChannelPairingBindingRecord;
#[cfg(feature = "memory-sqlite")]
use crate::session::repository::NewChannelPairingRequestRecord;
#[cfg(feature = "memory-sqlite")]
use crate::session::repository::SessionRepository;

const CHANNEL_PAIRING_CODE_ALPHABET: &[u8; 32] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
const CHANNEL_PAIRING_CODE_LENGTH: usize = 8;
const CHANNEL_PAIRING_CODE_TTL_MS: i64 = 60 * 60 * 1000;
const CHANNEL_PAIRING_REQUEST_COOLDOWN_MS: i64 = 10 * 60 * 1000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChannelPairingState {
    Disabled,
    NotApplicable,
    Required,
    Pending,
    Cooldown,
    Approved,
    Rejected,
}

impl ChannelPairingState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::NotApplicable => "not_applicable",
            Self::Required => "required",
            Self::Pending => "pending",
            Self::Cooldown => "cooldown",
            Self::Approved => "approved",
            Self::Rejected => "rejected",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelPairingSubject {
    pub channel_id: String,
    pub configured_account_id: String,
    pub account_id: Option<String>,
    pub conversation_id: String,
    pub participant_id: String,
    pub route_session_id: String,
    pub sender_principal_key: Option<String>,
}

impl ChannelPairingSubject {
    pub fn new(
        channel_id: impl Into<String>,
        configured_account_id: impl Into<String>,
        account_id: Option<String>,
        conversation_id: impl Into<String>,
        participant_id: impl Into<String>,
        route_session_id: impl Into<String>,
        sender_principal_key: Option<String>,
    ) -> Result<Self, String> {
        let channel_id = normalize_required_field(channel_id.into(), "channel_id")?;
        let configured_account_id =
            normalize_required_field(configured_account_id.into(), "configured_account_id")?;
        let account_id = normalize_optional_field(account_id);
        let conversation_id = normalize_required_field(conversation_id.into(), "conversation_id")?;
        let participant_id = normalize_required_field(participant_id.into(), "participant_id")?;
        let route_session_id =
            normalize_required_field(route_session_id.into(), "route_session_id")?;
        let sender_principal_key = normalize_optional_field(sender_principal_key);

        Ok(Self {
            channel_id,
            configured_account_id,
            account_id,
            conversation_id,
            participant_id,
            route_session_id,
            sender_principal_key,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ChannelPairingResolution {
    pub mode: ChannelPairingMode,
    pub state: ChannelPairingState,
    pub static_sender_gate_present: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pairing_request_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pairing_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pairing_code_expires_at_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binding_id: Option<String>,
}

#[cfg(feature = "memory-sqlite")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChannelPairingDecision {
    Authorized,
    PairingRequired {
        request: Box<ChannelPairingRequestRecord>,
        created: bool,
    },
    Cooldown {
        request: Box<ChannelPairingRequestRecord>,
        retry_after_ms: i64,
    },
    Rejected {
        request: Box<ChannelPairingRequestRecord>,
    },
}

#[cfg(feature = "memory-sqlite")]
pub fn evaluate_channel_pairing(
    memory_config: &MemoryRuntimeConfig,
    subject: &ChannelPairingSubject,
) -> Result<ChannelPairingDecision, String> {
    let repo = SessionRepository::new(memory_config)?;
    let binding = repo.load_channel_pairing_binding(
        subject.channel_id.as_str(),
        subject.configured_account_id.as_str(),
        subject.conversation_id.as_str(),
        subject.participant_id.as_str(),
    )?;
    if binding.is_some() {
        return Ok(ChannelPairingDecision::Authorized);
    }

    let latest_request = repo.load_latest_channel_pairing_request_by_subject(
        subject.channel_id.as_str(),
        subject.configured_account_id.as_str(),
        subject.conversation_id.as_str(),
        subject.participant_id.as_str(),
    )?;
    let Some(latest_request) = latest_request else {
        let request = create_channel_pairing_request(&repo, subject)?;
        return Ok(ChannelPairingDecision::PairingRequired {
            request: Box::new(request),
            created: true,
        });
    };

    match latest_request.status {
        ChannelPairingRequestStatus::Approved => Ok(ChannelPairingDecision::Authorized),
        ChannelPairingRequestStatus::Pending => {
            if request_is_expired(&latest_request) {
                let _ = repo.set_channel_pairing_request_resolution(
                    latest_request.pairing_request_id.as_str(),
                    ChannelPairingRequestStatus::Rejected,
                    None,
                    Some("expired".to_owned()),
                )?;
                let request = create_channel_pairing_request(&repo, subject)?;
                return Ok(ChannelPairingDecision::PairingRequired {
                    request: Box::new(request),
                    created: true,
                });
            }
            Ok(ChannelPairingDecision::PairingRequired {
                request: Box::new(latest_request),
                created: false,
            })
        }
        ChannelPairingRequestStatus::Rejected => {
            let retry_after_ms = retry_after_ms_for_request(&latest_request);
            if retry_after_ms > 0 {
                return Ok(ChannelPairingDecision::Cooldown {
                    request: Box::new(latest_request),
                    retry_after_ms,
                });
            }
            Ok(ChannelPairingDecision::Rejected {
                request: Box::new(latest_request),
            })
        }
    }
}

#[cfg(feature = "memory-sqlite")]
pub fn list_channel_pairing_requests(
    memory_config: &MemoryRuntimeConfig,
    status: Option<ChannelPairingRequestStatus>,
    limit: usize,
) -> Result<Vec<ChannelPairingRequestRecord>, String> {
    let repo = SessionRepository::new(memory_config)?;
    repo.list_channel_pairing_requests(status, limit)
}

#[cfg(feature = "memory-sqlite")]
pub fn resolve_channel_pairing_request(
    memory_config: &MemoryRuntimeConfig,
    pairing_request_id: &str,
    approve: bool,
    approved_by_session_id: Option<String>,
) -> Result<Option<ChannelPairingRequestRecord>, String> {
    let repo = SessionRepository::new(memory_config)?;
    let request = repo.load_channel_pairing_request(pairing_request_id)?;
    let Some(request) = request else {
        return Ok(None);
    };
    if request_is_expired(&request) {
        let expired = repo.set_channel_pairing_request_resolution(
            request.pairing_request_id.as_str(),
            ChannelPairingRequestStatus::Rejected,
            None,
            Some("expired".to_owned()),
        )?;
        return Ok(expired);
    }

    if approve {
        let existing_binding = repo.load_channel_pairing_binding(
            request.channel_id.as_str(),
            request.configured_account_id.as_str(),
            request.conversation_id.as_str(),
            request.participant_id.as_str(),
        )?;
        let candidate_binding_id = existing_binding
            .as_ref()
            .map(|binding| binding.binding_id.clone())
            .unwrap_or_else(next_channel_pairing_binding_id);
        let approved_at_ms = unix_time_ms_now();
        let new_binding = NewChannelPairingBindingRecord {
            binding_id: candidate_binding_id,
            channel_id: request.channel_id.clone(),
            configured_account_id: request.configured_account_id.clone(),
            account_id: request.account_id.clone(),
            conversation_id: request.conversation_id.clone(),
            participant_id: request.participant_id.clone(),
            route_session_id: request.route_session_id.clone(),
            sender_principal_key: request.sender_principal_key.clone(),
            approved_at_ms,
            pairing_request_id: Some(request.pairing_request_id.clone()),
            approved_by_session_id,
        };
        let binding = repo.upsert_channel_pairing_binding(new_binding)?;
        return repo.set_channel_pairing_request_resolution(
            request.pairing_request_id.as_str(),
            ChannelPairingRequestStatus::Approved,
            Some(binding.binding_id),
            None,
        );
    }

    let _ = repo.delete_channel_pairing_binding(
        request.channel_id.as_str(),
        request.configured_account_id.as_str(),
        request.conversation_id.as_str(),
        request.participant_id.as_str(),
    )?;
    repo.set_channel_pairing_request_resolution(
        request.pairing_request_id.as_str(),
        ChannelPairingRequestStatus::Rejected,
        None,
        None,
    )
}

#[cfg(feature = "memory-sqlite")]
pub fn resolve_channel_pairing_request_by_code(
    memory_config: &MemoryRuntimeConfig,
    pairing_code: &str,
    approve: bool,
    approved_by_session_id: Option<String>,
) -> Result<Option<ChannelPairingRequestRecord>, String> {
    let repo = SessionRepository::new(memory_config)?;
    let request = repo.load_latest_channel_pairing_request_by_code(pairing_code)?;
    let Some(request) = request else {
        return Ok(None);
    };
    resolve_channel_pairing_request(
        memory_config,
        request.pairing_request_id.as_str(),
        approve,
        approved_by_session_id,
    )
}

#[cfg(feature = "memory-sqlite")]
pub fn describe_channel_pairing_resolution(
    memory_config: &MemoryRuntimeConfig,
    mode: ChannelPairingMode,
    static_sender_gate_present: bool,
    subject: Option<&ChannelPairingSubject>,
) -> Result<ChannelPairingResolution, String> {
    if !mode.requires_participant_approval() {
        return Ok(ChannelPairingResolution {
            mode,
            state: ChannelPairingState::Disabled,
            static_sender_gate_present,
            pairing_request_id: None,
            pairing_code: None,
            pairing_code_expires_at_ms: None,
            binding_id: None,
        });
    }
    if static_sender_gate_present {
        return Ok(ChannelPairingResolution {
            mode,
            state: ChannelPairingState::NotApplicable,
            static_sender_gate_present,
            pairing_request_id: None,
            pairing_code: None,
            pairing_code_expires_at_ms: None,
            binding_id: None,
        });
    }
    let Some(subject) = subject else {
        return Ok(ChannelPairingResolution {
            mode,
            state: ChannelPairingState::NotApplicable,
            static_sender_gate_present,
            pairing_request_id: None,
            pairing_code: None,
            pairing_code_expires_at_ms: None,
            binding_id: None,
        });
    };

    let repo = SessionRepository::new(memory_config)?;
    let binding = repo.load_channel_pairing_binding(
        subject.channel_id.as_str(),
        subject.configured_account_id.as_str(),
        subject.conversation_id.as_str(),
        subject.participant_id.as_str(),
    )?;
    if let Some(binding) = binding {
        return Ok(ChannelPairingResolution {
            mode,
            state: ChannelPairingState::Approved,
            static_sender_gate_present,
            pairing_request_id: binding.pairing_request_id,
            pairing_code: None,
            pairing_code_expires_at_ms: None,
            binding_id: Some(binding.binding_id),
        });
    }

    let request = repo.load_latest_channel_pairing_request_by_subject(
        subject.channel_id.as_str(),
        subject.configured_account_id.as_str(),
        subject.conversation_id.as_str(),
        subject.participant_id.as_str(),
    )?;
    let Some(request) = request else {
        return Ok(ChannelPairingResolution {
            mode,
            state: ChannelPairingState::Required,
            static_sender_gate_present,
            pairing_request_id: None,
            pairing_code: None,
            pairing_code_expires_at_ms: None,
            binding_id: None,
        });
    };

    let state = match request.status {
        ChannelPairingRequestStatus::Pending => ChannelPairingState::Pending,
        ChannelPairingRequestStatus::Approved => ChannelPairingState::Approved,
        ChannelPairingRequestStatus::Rejected => {
            let retry_after_ms = retry_after_ms_for_request(&request);
            if retry_after_ms > 0 {
                ChannelPairingState::Cooldown
            } else {
                ChannelPairingState::Rejected
            }
        }
    };

    Ok(ChannelPairingResolution {
        mode,
        state,
        static_sender_gate_present,
        pairing_request_id: Some(request.pairing_request_id),
        pairing_code: Some(request.pairing_code),
        pairing_code_expires_at_ms: Some(request.expires_at_ms),
        binding_id: request.approved_binding_id,
    })
}

#[cfg(feature = "memory-sqlite")]
pub fn render_channel_pairing_reply(decision: &ChannelPairingDecision) -> String {
    match decision {
        ChannelPairingDecision::Authorized => String::new(),
        ChannelPairingDecision::PairingRequired { request, created } => {
            let request_id = request.pairing_request_id.as_str();
            let pairing_code = format_pairing_code_for_display(request.pairing_code.as_str());
            let expires_in_minutes = expires_in_minutes(request.expires_at_ms);
            let created_note = if *created {
                " A new approval request has been created."
            } else {
                " The existing approval request is still pending."
            };
            format!(
                "This conversation requires operator pairing approval before I can respond. Share pairing_code={pairing_code} with the operator. It expires in about {expires_in_minutes} minutes. request_id={request_id}.{created_note}"
            )
        }
        ChannelPairingDecision::Cooldown {
            request,
            retry_after_ms,
        } => {
            let request_id = request.pairing_request_id.as_str();
            let retry_after_minutes = retry_after_minutes(*retry_after_ms);
            format!(
                "A recent channel pairing request was already handled for this participant. Wait about {retry_after_minutes} minutes before requesting a new pairing code. request_id={request_id}."
            )
        }
        ChannelPairingDecision::Rejected { request } => {
            let request_id = request.pairing_request_id.as_str();
            format!(
                "This conversation is still blocked because operator pairing approval was rejected. request_id={request_id}."
            )
        }
    }
}

#[cfg(feature = "memory-sqlite")]
fn create_channel_pairing_request(
    repo: &SessionRepository,
    subject: &ChannelPairingSubject,
) -> Result<ChannelPairingRequestRecord, String> {
    let request_id = next_channel_pairing_request_id();
    let pairing_code = next_channel_pairing_code();
    let expires_at_ms = unix_time_ms_now() + CHANNEL_PAIRING_CODE_TTL_MS;
    let record = NewChannelPairingRequestRecord {
        pairing_request_id: request_id,
        channel_id: subject.channel_id.clone(),
        configured_account_id: subject.configured_account_id.clone(),
        account_id: subject.account_id.clone(),
        conversation_id: subject.conversation_id.clone(),
        participant_id: subject.participant_id.clone(),
        route_session_id: subject.route_session_id.clone(),
        sender_principal_key: subject.sender_principal_key.clone(),
        pairing_code,
        expires_at_ms,
    };
    repo.ensure_channel_pairing_request(record)
}

#[cfg(feature = "memory-sqlite")]
fn request_is_expired(request: &ChannelPairingRequestRecord) -> bool {
    unix_time_ms_now() >= request.expires_at_ms
}

#[cfg(feature = "memory-sqlite")]
fn retry_after_ms_for_request(request: &ChannelPairingRequestRecord) -> i64 {
    let retry_at_ms = request.requested_at_ms + CHANNEL_PAIRING_REQUEST_COOLDOWN_MS;
    let now_ms = unix_time_ms_now();
    retry_at_ms.saturating_sub(now_ms)
}

fn normalize_optional_field(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

fn normalize_required_field(value: String, field: &str) -> Result<String, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(format!("channel pairing {field} is empty"));
    }
    Ok(trimmed.to_owned())
}

#[cfg(feature = "memory-sqlite")]
fn next_channel_pairing_code() -> String {
    let mut random_bits = rand::random::<u64>();
    let mut code = String::with_capacity(CHANNEL_PAIRING_CODE_LENGTH);
    for _ in 0..CHANNEL_PAIRING_CODE_LENGTH {
        let index = (random_bits & 0x1f) as usize;
        let symbol = CHANNEL_PAIRING_CODE_ALPHABET
            .get(index)
            .copied()
            .unwrap_or(b'A') as char;
        code.push(symbol);
        random_bits >>= 5;
    }
    code
}

fn format_pairing_code_for_display(code: &str) -> String {
    let trimmed = code.trim();
    if trimmed.len() != CHANNEL_PAIRING_CODE_LENGTH {
        return trimmed.to_owned();
    }
    let first = &trimmed[0..4];
    let second = &trimmed[4..8];
    format!("{first}-{second}")
}

fn expires_in_minutes(expires_at_ms: i64) -> i64 {
    let now_ms = unix_time_ms_now();
    let remaining_ms = expires_at_ms.saturating_sub(now_ms);
    let remaining_minutes = remaining_ms / 60_000;
    remaining_minutes.max(1)
}

fn retry_after_minutes(retry_after_ms: i64) -> i64 {
    let remaining_minutes = retry_after_ms / 60_000;
    remaining_minutes.max(1)
}

#[cfg(feature = "memory-sqlite")]
fn next_channel_pairing_request_id() -> String {
    let now_ms = unix_time_ms_now() as u64;
    let random_component = rand::random::<u64>();
    format!("cpr-{now_ms:016x}-{random_component:016x}")
}

#[cfg(feature = "memory-sqlite")]
fn next_channel_pairing_binding_id() -> String {
    let now_ms = unix_time_ms_now() as u64;
    let random_component = rand::random::<u64>();
    format!("cpb-{now_ms:016x}-{random_component:016x}")
}

#[cfg(feature = "memory-sqlite")]
fn unix_time_ms_now() -> i64 {
    let now = std::time::SystemTime::now();
    let duration = now
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_else(|_| std::time::Duration::from_millis(0));
    duration.as_millis() as i64
}

#[cfg(all(test, feature = "memory-sqlite"))]
#[allow(clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::memory;
    use crate::memory::runtime_config::MemoryRuntimeConfig;
    use crate::test_support::unique_temp_dir;

    fn pairing_test_memory(label: &str) -> (std::path::PathBuf, MemoryRuntimeConfig) {
        let root = unique_temp_dir(label);
        let sqlite_path = root.join("memory.sqlite3");
        let runtime = MemoryRuntimeConfig {
            sqlite_path: Some(sqlite_path.clone()),
            ..MemoryRuntimeConfig::default()
        };
        memory::ensure_memory_db_ready(Some(sqlite_path), &runtime)
            .expect("initialize pairing test memory");
        (root, runtime)
    }

    fn cleanup_pairing_test_memory(root: &std::path::Path) {
        let _ = std::fs::remove_dir_all(root);
    }

    fn subject() -> ChannelPairingSubject {
        ChannelPairingSubject::new(
            "feishu",
            "work",
            Some("feishu_cli_a1b2c3".to_owned()),
            "oc_demo",
            "ou_sender_1",
            "feishu:feishu_cli_a1b2c3:oc_demo:ou_sender_1",
            Some("feishu_cli_a1b2c3:ou_sender_1".to_owned()),
        )
        .expect("build channel pairing subject")
    }

    #[test]
    fn channel_pairing_creates_and_deduplicates_pending_request() {
        let (root, runtime) = pairing_test_memory("channel-pairing-pending");
        let subject = subject();

        let first = evaluate_channel_pairing(&runtime, &subject).expect("evaluate first request");
        let second =
            evaluate_channel_pairing(&runtime, &subject).expect("evaluate deduplicated request");

        let first_request_id = match first {
            ChannelPairingDecision::PairingRequired { request, created } => {
                assert!(created);
                request.pairing_request_id
            }
            other @ ChannelPairingDecision::Authorized
            | other @ ChannelPairingDecision::Cooldown { .. }
            | other @ ChannelPairingDecision::Rejected { .. } => {
                panic!("expected pending request, got {other:?}")
            }
        };
        let second_request_id = match second {
            ChannelPairingDecision::PairingRequired { request, created } => {
                assert!(!created);
                request.pairing_request_id
            }
            other @ ChannelPairingDecision::Authorized
            | other @ ChannelPairingDecision::Cooldown { .. }
            | other @ ChannelPairingDecision::Rejected { .. } => {
                panic!("expected reused pending request, got {other:?}")
            }
        };

        assert_eq!(first_request_id, second_request_id);
        cleanup_pairing_test_memory(&root);
    }

    #[test]
    fn channel_pairing_approval_creates_binding_and_authorizes_future_turns() {
        let (root, runtime) = pairing_test_memory("channel-pairing-approve");
        let subject = subject();

        let pending = evaluate_channel_pairing(&runtime, &subject).expect("create pending request");
        let pairing_request_id = match pending {
            ChannelPairingDecision::PairingRequired { request, .. } => request.pairing_request_id,
            other @ ChannelPairingDecision::Authorized
            | other @ ChannelPairingDecision::Cooldown { .. }
            | other @ ChannelPairingDecision::Rejected { .. } => {
                panic!("expected pending request, got {other:?}")
            }
        };

        let approved = resolve_channel_pairing_request(
            &runtime,
            pairing_request_id.as_str(),
            true,
            Some("root-session".to_owned()),
        )
        .expect("approve channel pairing")
        .expect("persisted approval");
        assert_eq!(approved.status, ChannelPairingRequestStatus::Approved);

        let decision = evaluate_channel_pairing(&runtime, &subject).expect("re-evaluate approval");
        assert!(matches!(decision, ChannelPairingDecision::Authorized));
        cleanup_pairing_test_memory(&root);
    }

    #[test]
    fn channel_pairing_can_resolve_pending_request_by_pairing_code() {
        let (root, runtime) = pairing_test_memory("channel-pairing-code");
        let subject = subject();

        let pending = evaluate_channel_pairing(&runtime, &subject).expect("create pending request");
        let pairing_code = match pending {
            ChannelPairingDecision::PairingRequired { request, .. } => request.pairing_code,
            other @ ChannelPairingDecision::Authorized
            | other @ ChannelPairingDecision::Cooldown { .. }
            | other @ ChannelPairingDecision::Rejected { .. } => {
                panic!("expected pending request, got {other:?}")
            }
        };

        let approved = resolve_channel_pairing_request_by_code(
            &runtime,
            pairing_code.as_str(),
            true,
            Some("root-session".to_owned()),
        )
        .expect("approve pairing by code")
        .expect("persisted approval");
        assert_eq!(approved.status, ChannelPairingRequestStatus::Approved);

        let decision = evaluate_channel_pairing(&runtime, &subject).expect("re-evaluate approval");
        assert!(matches!(decision, ChannelPairingDecision::Authorized));
        cleanup_pairing_test_memory(&root);
    }

    #[test]
    fn channel_pairing_rejection_blocks_without_creating_new_pending_request() {
        let (root, runtime) = pairing_test_memory("channel-pairing-reject");
        let subject = subject();

        let pending = evaluate_channel_pairing(&runtime, &subject).expect("create pending request");
        let pairing_request_id = match pending {
            ChannelPairingDecision::PairingRequired { request, .. } => request.pairing_request_id,
            other @ ChannelPairingDecision::Authorized
            | other @ ChannelPairingDecision::Cooldown { .. }
            | other @ ChannelPairingDecision::Rejected { .. } => {
                panic!("expected pending request, got {other:?}")
            }
        };

        let rejected = resolve_channel_pairing_request(
            &runtime,
            pairing_request_id.as_str(),
            false,
            Some("root-session".to_owned()),
        )
        .expect("reject channel pairing")
        .expect("persisted rejection");
        assert_eq!(rejected.status, ChannelPairingRequestStatus::Rejected);

        let decision = evaluate_channel_pairing(&runtime, &subject).expect("re-evaluate rejection");
        let seen_request_id = match decision {
            ChannelPairingDecision::Cooldown { request, .. } => request.pairing_request_id,
            other @ ChannelPairingDecision::Authorized
            | other @ ChannelPairingDecision::PairingRequired { .. }
            | other @ ChannelPairingDecision::Rejected { .. } => {
                panic!("expected cooldown after rejection, got {other:?}")
            }
        };
        assert_eq!(seen_request_id, pairing_request_id);
        cleanup_pairing_test_memory(&root);
    }

    #[test]
    fn channel_pairing_resolution_reports_required_pending_and_approved_states() {
        let (root, runtime) = pairing_test_memory("channel-pairing-resolution");
        let subject = subject();

        let required = describe_channel_pairing_resolution(
            &runtime,
            ChannelPairingMode::ParticipantApproval,
            false,
            Some(&subject),
        )
        .expect("describe required pairing");
        assert_eq!(required.state, ChannelPairingState::Required);

        let pending = evaluate_channel_pairing(&runtime, &subject).expect("create pending request");
        let pairing_request_id = match pending {
            ChannelPairingDecision::PairingRequired { request, .. } => request.pairing_request_id,
            other @ ChannelPairingDecision::Authorized
            | other @ ChannelPairingDecision::Cooldown { .. }
            | other @ ChannelPairingDecision::Rejected { .. } => {
                panic!("expected pending request, got {other:?}")
            }
        };

        let pending_resolution = describe_channel_pairing_resolution(
            &runtime,
            ChannelPairingMode::ParticipantApproval,
            false,
            Some(&subject),
        )
        .expect("describe pending pairing");
        assert_eq!(pending_resolution.state, ChannelPairingState::Pending);
        assert_eq!(
            pending_resolution.pairing_request_id.as_deref(),
            Some(pairing_request_id.as_str())
        );

        let _ = resolve_channel_pairing_request(
            &runtime,
            pairing_request_id.as_str(),
            true,
            Some("root-session".to_owned()),
        )
        .expect("approve request")
        .expect("approved request record");

        let approved_resolution = describe_channel_pairing_resolution(
            &runtime,
            ChannelPairingMode::ParticipantApproval,
            false,
            Some(&subject),
        )
        .expect("describe approved pairing");
        assert_eq!(approved_resolution.state, ChannelPairingState::Approved);
        assert!(approved_resolution.binding_id.is_some());
        cleanup_pairing_test_memory(&root);
    }

    #[test]
    fn channel_pairing_reply_surfaces_formatted_pairing_code() {
        let (root, runtime) = pairing_test_memory("channel-pairing-reply");
        let subject = subject();
        let decision = evaluate_channel_pairing(&runtime, &subject).expect("create request");

        let reply = render_channel_pairing_reply(&decision);

        assert!(reply.contains("pairing_code="));
        assert!(reply.contains("request_id="));
        assert!(reply.contains("-"));
        cleanup_pairing_test_memory(&root);
    }

    #[test]
    fn channel_pairing_resolution_reports_cooldown_after_rejection() {
        let (root, runtime) = pairing_test_memory("channel-pairing-cooldown");
        let subject = subject();

        let pending = evaluate_channel_pairing(&runtime, &subject).expect("create pending request");
        let pairing_request_id = match pending {
            ChannelPairingDecision::PairingRequired { request, .. } => request.pairing_request_id,
            other @ ChannelPairingDecision::Authorized
            | other @ ChannelPairingDecision::Cooldown { .. }
            | other @ ChannelPairingDecision::Rejected { .. } => {
                panic!("expected pending request, got {other:?}")
            }
        };

        let _ = resolve_channel_pairing_request(
            &runtime,
            pairing_request_id.as_str(),
            false,
            Some("root-session".to_owned()),
        )
        .expect("reject request")
        .expect("persisted rejection");

        let decision = evaluate_channel_pairing(&runtime, &subject).expect("re-evaluate rejection");
        assert!(matches!(decision, ChannelPairingDecision::Cooldown { .. }));

        let resolution = describe_channel_pairing_resolution(
            &runtime,
            ChannelPairingMode::ParticipantApproval,
            false,
            Some(&subject),
        )
        .expect("describe cooldown");
        assert_eq!(resolution.state, ChannelPairingState::Cooldown);
        cleanup_pairing_test_memory(&root);
    }
}
