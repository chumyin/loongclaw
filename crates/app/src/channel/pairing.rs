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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChannelPairingState {
    Disabled,
    NotApplicable,
    Required,
    Pending,
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
        let request_id = next_channel_pairing_request_id();
        let record = NewChannelPairingRequestRecord {
            pairing_request_id: request_id,
            channel_id: subject.channel_id.clone(),
            configured_account_id: subject.configured_account_id.clone(),
            account_id: subject.account_id.clone(),
            conversation_id: subject.conversation_id.clone(),
            participant_id: subject.participant_id.clone(),
            route_session_id: subject.route_session_id.clone(),
            sender_principal_key: subject.sender_principal_key.clone(),
        };
        let request = repo.ensure_channel_pairing_request(record)?;
        return Ok(ChannelPairingDecision::PairingRequired {
            request: Box::new(request),
            created: true,
        });
    };

    match latest_request.status {
        ChannelPairingRequestStatus::Approved => Ok(ChannelPairingDecision::Authorized),
        ChannelPairingRequestStatus::Pending => Ok(ChannelPairingDecision::PairingRequired {
            request: Box::new(latest_request),
            created: false,
        }),
        ChannelPairingRequestStatus::Rejected => Ok(ChannelPairingDecision::Rejected {
            request: Box::new(latest_request),
        }),
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
            binding_id: None,
        });
    }
    if static_sender_gate_present {
        return Ok(ChannelPairingResolution {
            mode,
            state: ChannelPairingState::NotApplicable,
            static_sender_gate_present,
            pairing_request_id: None,
            binding_id: None,
        });
    }
    let Some(subject) = subject else {
        return Ok(ChannelPairingResolution {
            mode,
            state: ChannelPairingState::NotApplicable,
            static_sender_gate_present,
            pairing_request_id: None,
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
            binding_id: None,
        });
    };

    let state = match request.status {
        ChannelPairingRequestStatus::Pending => ChannelPairingState::Pending,
        ChannelPairingRequestStatus::Approved => ChannelPairingState::Approved,
        ChannelPairingRequestStatus::Rejected => ChannelPairingState::Rejected,
    };

    Ok(ChannelPairingResolution {
        mode,
        state,
        static_sender_gate_present,
        pairing_request_id: Some(request.pairing_request_id),
        binding_id: request.approved_binding_id,
    })
}

#[cfg(feature = "memory-sqlite")]
pub fn render_channel_pairing_reply(decision: &ChannelPairingDecision) -> String {
    match decision {
        ChannelPairingDecision::Authorized => String::new(),
        ChannelPairingDecision::PairingRequired { request, created } => {
            let request_id = request.pairing_request_id.as_str();
            let created_note = if *created {
                " A new approval request has been created."
            } else {
                " The existing approval request is still pending."
            };
            format!(
                "This conversation requires operator pairing approval before I can respond. request_id={request_id}.{created_note}"
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
    fn channel_pairing_rejection_blocks_without_creating_new_pending_request() {
        let (root, runtime) = pairing_test_memory("channel-pairing-reject");
        let subject = subject();

        let pending = evaluate_channel_pairing(&runtime, &subject).expect("create pending request");
        let pairing_request_id = match pending {
            ChannelPairingDecision::PairingRequired { request, .. } => request.pairing_request_id,
            other @ ChannelPairingDecision::Authorized
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
            ChannelPairingDecision::Rejected { request } => request.pairing_request_id,
            other @ ChannelPairingDecision::Authorized
            | other @ ChannelPairingDecision::PairingRequired { .. } => {
                panic!("expected rejected request, got {other:?}")
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
}
