use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use loongclaw_contracts::{
    WORK_UNIT_SPLIT_MIN_CHILDREN, WorkRuntimeHealthSnapshot, WorkUnitEventRecord, WorkUnitKind,
    WorkUnitLeaseRecord, WorkUnitPriority, WorkUnitRecord, WorkUnitRetryPolicy,
    WorkUnitReviewRecord, WorkUnitReviewStatus, WorkUnitSnapshot, WorkUnitSourceRef,
    WorkUnitStatus,
};
use rand::random;
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde::Serialize;
use serde_json::{Value, json};

use crate::memory;
use crate::memory::runtime_config::MemoryRuntimeConfig;

const WORK_UNIT_CREATED_EVENT_KIND: &str = "work_unit_created";
const WORK_UNIT_LEASED_EVENT_KIND: &str = "work_unit_leased";
const WORK_UNIT_STARTED_EVENT_KIND: &str = "work_unit_started";
const WORK_UNIT_HEARTBEAT_EVENT_KIND: &str = "work_unit_heartbeat";
const WORK_UNIT_RETRY_EVENT_KIND: &str = "work_unit_retry_scheduled";
const WORK_UNIT_COMPLETED_EVENT_KIND: &str = "work_unit_completed";
const WORK_UNIT_FAILED_EVENT_KIND: &str = "work_unit_failed_terminal";
const WORK_UNIT_CANCELLED_EVENT_KIND: &str = "work_unit_cancelled";
const WORK_UNIT_ARCHIVED_EVENT_KIND: &str = "work_unit_archived";
const WORK_UNIT_LEASE_EXPIRED_EVENT_KIND: &str = "work_unit_lease_expired_recovered";
const WORK_UNIT_ASSIGNED_EVENT_KIND: &str = "work_unit_assigned";
const WORK_UNIT_DEPENDENCY_ADDED_EVENT_KIND: &str = "work_unit_dependency_added";
const WORK_UNIT_DEPENDENCY_REMOVED_EVENT_KIND: &str = "work_unit_dependency_removed";
const WORK_UNIT_NOTE_ADDED_EVENT_KIND: &str = "work_unit_note_added";
const WORK_UNIT_UPDATED_EVENT_KIND: &str = "work_unit_updated";
const WORK_UNIT_REVIEW_REQUESTED_EVENT_KIND: &str = "work_unit_review_requested";
const WORK_UNIT_REVIEW_RECORDED_EVENT_KIND: &str = "work_unit_review_recorded";
const WORK_UNIT_CHILD_CREATED_EVENT_KIND: &str = "work_unit_child_created";
const WORK_UNIT_SPLIT_APPLIED_EVENT_KIND: &str = "work_unit_split_applied";
const WORK_UNIT_SUPERSEDED_EVENT_KIND: &str = "work_unit_superseded";
const WORK_UNIT_REPLANNED_EVENT_KIND: &str = "work_unit_replanned";
const WORK_UNIT_RESEQUENCED_EVENT_KIND: &str = "work_unit_resequenced";

#[derive(Debug, Clone, PartialEq)]
pub struct NewWorkUnitRecord {
    pub work_unit_id: Option<String>,
    pub kind: WorkUnitKind,
    pub title: String,
    pub description: String,
    pub source_ref: WorkUnitSourceRef,
    pub status: WorkUnitStatus,
    pub priority: WorkUnitPriority,
    pub retry_policy: WorkUnitRetryPolicy,
    pub parent_work_unit_id: Option<String>,
    pub plan_position: Option<i64>,
    pub next_run_at_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkUnitListQuery {
    pub status: Option<WorkUnitStatus>,
    pub include_archived: bool,
    pub limit: usize,
}

impl Default for WorkUnitListQuery {
    fn default() -> Self {
        Self {
            status: None,
            include_archived: false,
            limit: 100,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcquireWorkUnitLeaseRequest {
    pub owner: String,
    pub ttl_ms: u64,
    pub actor: Option<String>,
    pub now_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartWorkUnitLeaseRequest {
    pub work_unit_id: String,
    pub owner: String,
    pub actor: Option<String>,
    pub now_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkUnitHeartbeatRequest {
    pub work_unit_id: String,
    pub owner: String,
    pub ttl_ms: u64,
    pub actor: Option<String>,
    pub now_ms: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkUnitCompletionDisposition {
    Completed,
    RetryPending,
    FailedTerminal,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CompleteWorkUnitRequest {
    pub work_unit_id: String,
    pub owner: String,
    pub disposition: WorkUnitCompletionDisposition,
    pub actor: Option<String>,
    pub now_ms: Option<i64>,
    pub next_run_at_ms: Option<i64>,
    pub result_payload_json: Option<Value>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveWorkUnitRequest {
    pub work_unit_id: String,
    pub actor: Option<String>,
    pub now_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssignWorkUnitRequest {
    pub work_unit_id: String,
    pub assigned_to: Option<String>,
    pub actor: Option<String>,
    pub now_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddWorkUnitDependencyRequest {
    pub blocking_work_unit_id: String,
    pub blocked_work_unit_id: String,
    pub actor: Option<String>,
    pub now_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoveWorkUnitDependencyRequest {
    pub blocking_work_unit_id: String,
    pub blocked_work_unit_id: String,
    pub actor: Option<String>,
    pub now_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppendWorkUnitNoteRequest {
    pub work_unit_id: String,
    pub actor: Option<String>,
    pub note: String,
    pub now_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateWorkUnitRequest {
    pub work_unit_id: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub status: Option<WorkUnitStatus>,
    pub priority: Option<WorkUnitPriority>,
    pub next_run_at_ms: Option<i64>,
    pub blocking_reason: Option<String>,
    pub clear_blocking_reason: bool,
    pub actor: Option<String>,
    pub now_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestWorkUnitReviewRequest {
    pub work_unit_id: String,
    pub requested_by: Option<String>,
    pub reviewer: Option<String>,
    pub summary: Option<String>,
    pub now_ms: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkUnitReviewDecision {
    Approve,
    RequestChanges,
    Reject,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordWorkUnitReviewDecisionRequest {
    pub work_unit_id: String,
    pub reviewer: Option<String>,
    pub decision: WorkUnitReviewDecision,
    pub summary: Option<String>,
    pub now_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CreateChildWorkUnitRequest {
    pub parent_work_unit_id: String,
    pub child: NewWorkUnitRecord,
    pub inherit_parent_source_ref: bool,
    pub inherit_parent_retry_policy: bool,
    pub inherit_parent_priority: bool,
    pub block_parent: bool,
    pub actor: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SplitWorkUnitChildRequest {
    pub child: NewWorkUnitRecord,
    pub inherit_parent_source_ref: bool,
    pub inherit_parent_retry_policy: bool,
    pub inherit_parent_priority: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SplitWorkUnitRequest {
    pub parent_work_unit_id: String,
    pub children: Vec<SplitWorkUnitChildRequest>,
    pub block_parent: bool,
    pub actor: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SplitWorkUnitResult {
    pub parent: WorkUnitSnapshot,
    pub children: Vec<WorkUnitSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SupersedeWorkUnitRequest {
    pub obsolete_work_unit_id: String,
    pub replacement_work_unit_id: String,
    pub actor: Option<String>,
    pub now_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SupersedeWorkUnitResult {
    pub obsolete: WorkUnitSnapshot,
    pub replacement: WorkUnitSnapshot,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplanWorkUnitRequest {
    pub work_unit_id: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub status: Option<WorkUnitStatus>,
    pub priority: Option<WorkUnitPriority>,
    pub next_run_at_ms: Option<i64>,
    pub blocking_reason: Option<String>,
    pub clear_blocking_reason: bool,
    pub ordered_child_work_unit_ids: Option<Vec<String>>,
    pub actor: Option<String>,
    pub now_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ReplanWorkUnitResult {
    pub parent: WorkUnitSnapshot,
    pub children: Vec<WorkUnitSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResequenceChildWorkUnitsRequest {
    pub parent_work_unit_id: String,
    pub ordered_child_work_unit_ids: Vec<String>,
    pub actor: Option<String>,
    pub now_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ResequenceChildWorkUnitsResult {
    pub parent: WorkUnitSnapshot,
    pub children: Vec<WorkUnitSnapshot>,
}

#[derive(Debug, Clone)]
pub struct WorkUnitRepository {
    db_path: PathBuf,
}

#[derive(Debug, Clone)]
struct RawWorkUnitRecord {
    work_unit_id: String,
    kind: String,
    title: String,
    description: String,
    source_ref_json: String,
    status: String,
    priority: String,
    retry_policy_json: String,
    attempt_count: i64,
    next_run_at_ms: i64,
    last_error: Option<String>,
    blocking_reason: Option<String>,
    parent_work_unit_id: Option<String>,
    plan_position: Option<i64>,
    superseded_by_work_unit_id: Option<String>,
    assigned_to: Option<String>,
    child_work_unit_ids: Vec<String>,
    supersedes_work_unit_ids: Vec<String>,
    blocks_work_unit_ids: Vec<String>,
    blocked_by_work_unit_ids: Vec<String>,
    review_json: Option<String>,
    result_payload_json: Option<String>,
    lease_owner: Option<String>,
    lease_version: i64,
    lease_acquired_at_ms: Option<i64>,
    lease_heartbeat_at_ms: Option<i64>,
    lease_expires_at_ms: Option<i64>,
    created_at_ms: i64,
    updated_at_ms: i64,
    archived_at_ms: Option<i64>,
}

#[derive(Debug, Clone)]
struct ResolvedWorkUnitUpdate {
    changed_fields: Vec<String>,
    next_title: String,
    next_description: String,
    next_status: String,
    next_priority: String,
    next_priority_rank: i64,
    next_next_run_at_ms: i64,
    next_blocking_reason: Option<String>,
}

impl WorkUnitRepository {
    pub fn new(config: &MemoryRuntimeConfig) -> Result<Self, String> {
        let db_path = memory::ensure_memory_db_ready(config.sqlite_path.clone(), config)?;
        let repository = Self { db_path };
        repository.ensure_schema()?;
        Ok(repository)
    }

    pub fn create_work_unit(
        &self,
        record: NewWorkUnitRecord,
        actor: Option<&str>,
    ) -> Result<WorkUnitSnapshot, String> {
        let mut connection = self.open_connection()?;
        let transaction = connection
            .transaction()
            .map_err(|error| format!("open work unit create transaction failed: {error}"))?;
        let snapshot = self.create_work_unit_in_tx(&transaction, record, actor)?;
        transaction
            .commit()
            .map_err(|error| format!("commit work unit create transaction failed: {error}"))?;

        self.load_work_unit_snapshot(snapshot.work_unit.work_unit_id.as_str())?
            .ok_or_else(|| {
                format!(
                    "work unit `{}` disappeared after insert",
                    snapshot.work_unit.work_unit_id
                )
            })
    }

    fn create_work_unit_in_tx(
        &self,
        transaction: &Transaction<'_>,
        record: NewWorkUnitRecord,
        actor: Option<&str>,
    ) -> Result<WorkUnitSnapshot, String> {
        validate_initial_status(record.status)?;
        validate_retry_policy(&record.retry_policy)?;

        let generated_id = record
            .work_unit_id
            .as_deref()
            .map(|value| normalize_required_text(value, "work_unit_id"))
            .transpose()?;
        let work_unit_id = generated_id.unwrap_or_else(generate_work_unit_id);

        let title = normalize_required_text(&record.title, "title")?;
        let description = normalize_required_text(&record.description, "description")?;
        let parent_work_unit_id = normalize_optional_text(record.parent_work_unit_id);
        let plan_position = normalize_plan_position(record.plan_position)?;
        let plan_position = if let Some(parent_work_unit_id) = parent_work_unit_id.as_deref() {
            if plan_position.is_none() {
                Some(load_next_child_plan_position(
                    transaction,
                    parent_work_unit_id,
                )?)
            } else {
                plan_position
            }
        } else {
            plan_position
        };
        let source_ref = normalize_source_ref(record.source_ref);
        let source_ref_json = encode_json(&source_ref, "source_ref")?;
        let retry_policy_json = encode_json(&record.retry_policy, "retry_policy")?;
        let now_ms = current_unix_ms();
        let next_run_at_ms = record.next_run_at_ms.unwrap_or(now_ms);
        let priority_rank = priority_rank(record.priority);
        let normalized_actor = normalize_optional_text(actor.map(str::to_owned));

        transaction
            .execute(
                "INSERT INTO work_units(
                    work_unit_id,
                    kind,
                    title,
                    description,
                    source_ref_json,
                    status,
                    priority,
                    priority_rank,
                    retry_policy_json,
                    attempt_count,
                    next_run_at_ms,
                    last_error,
                    blocking_reason,
                    parent_work_unit_id,
                    plan_position,
                    superseded_by_work_unit_id,
                    assigned_to,
                    review_json,
                    result_payload_json,
                    lease_owner,
                    lease_version,
                    lease_acquired_at_ms,
                    lease_heartbeat_at_ms,
                    lease_expires_at_ms,
                    created_at_ms,
                    updated_at_ms,
                    archived_at_ms
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 0, ?10, NULL, NULL, ?11, ?12, NULL, NULL, NULL, NULL, NULL, 0, NULL, NULL, NULL, ?13, ?13, NULL)",
                params![
                    work_unit_id,
                    record.kind.as_str(),
                    title,
                    description,
                    source_ref_json,
                    record.status.as_str(),
                    record.priority.as_str(),
                    priority_rank,
                    retry_policy_json,
                    next_run_at_ms,
                    parent_work_unit_id,
                    plan_position,
                    now_ms,
                ],
            )
            .map_err(|error| format!("insert work unit row failed: {error}"))?;

        let event_payload = json!({
            "kind": record.kind.as_str(),
            "status": record.status.as_str(),
            "priority": record.priority.as_str(),
            "next_run_at_ms": next_run_at_ms,
            "source_ref": source_ref,
        });
        insert_event_in_tx(
            transaction,
            &work_unit_id,
            WORK_UNIT_CREATED_EVENT_KIND,
            normalized_actor.as_deref(),
            &event_payload,
            now_ms,
        )?;
        self.load_work_unit_snapshot_with_conn(transaction, &work_unit_id)?
            .ok_or_else(|| format!("work unit `{work_unit_id}` disappeared after insert"))
    }

    pub fn create_child_work_unit(
        &self,
        request: CreateChildWorkUnitRequest,
    ) -> Result<WorkUnitSnapshot, String> {
        let parent_work_unit_id =
            normalize_required_text(&request.parent_work_unit_id, "parent_work_unit_id")?;
        let actor = normalize_optional_text(request.actor);
        let child = request.child;
        let mut connection = self.open_connection()?;
        let transaction = connection
            .transaction()
            .map_err(|error| format!("open child work-unit transaction failed: {error}"))?;
        let parent_snapshot =
            self.load_parent_work_unit_for_children(&transaction, parent_work_unit_id.as_str())?;
        let child_snapshot = self.create_child_work_unit_in_tx(
            &transaction,
            &parent_snapshot,
            child,
            request.inherit_parent_source_ref,
            request.inherit_parent_retry_policy,
            request.inherit_parent_priority,
            request.block_parent,
            actor.as_deref(),
        )?;

        transaction
            .commit()
            .map_err(|error| format!("commit child work-unit transaction failed: {error}"))?;

        self.load_work_unit_snapshot(child_snapshot.work_unit.work_unit_id.as_str())?
            .ok_or_else(|| "child work unit disappeared after create".to_owned())
    }

    pub fn split_work_unit(
        &self,
        request: SplitWorkUnitRequest,
    ) -> Result<SplitWorkUnitResult, String> {
        let parent_work_unit_id =
            normalize_required_text(&request.parent_work_unit_id, "parent_work_unit_id")?;
        let child_count = request.children.len();
        if child_count < WORK_UNIT_SPLIT_MIN_CHILDREN {
            return Err(format!(
                "split_work_unit requires at least {WORK_UNIT_SPLIT_MIN_CHILDREN} child work units"
            ));
        }

        let actor = normalize_optional_text(request.actor);
        validate_split_child_ids(parent_work_unit_id.as_str(), request.children.as_slice())?;

        let mut connection = self.open_connection()?;
        let transaction = connection
            .transaction()
            .map_err(|error| format!("open split work-unit transaction failed: {error}"))?;
        let parent_snapshot =
            self.load_parent_work_unit_for_children(&transaction, parent_work_unit_id.as_str())?;
        let mut child_work_unit_ids = Vec::with_capacity(child_count);

        for child_request in request.children {
            let child_snapshot = self.create_child_work_unit_in_tx(
                &transaction,
                &parent_snapshot,
                child_request.child,
                child_request.inherit_parent_source_ref,
                child_request.inherit_parent_retry_policy,
                child_request.inherit_parent_priority,
                request.block_parent,
                actor.as_deref(),
            )?;
            let child_work_unit_id = child_snapshot.work_unit.work_unit_id;
            child_work_unit_ids.push(child_work_unit_id);
        }

        let created_child_work_unit_ids = child_work_unit_ids.clone();
        let recorded_at_ms = current_unix_ms();
        let event_payload = json!({
            "parent_work_unit_id": parent_work_unit_id,
            "child_work_unit_ids": child_work_unit_ids,
            "block_parent": request.block_parent,
        });
        insert_event_in_tx(
            &transaction,
            parent_work_unit_id.as_str(),
            WORK_UNIT_SPLIT_APPLIED_EVENT_KIND,
            actor.as_deref(),
            &event_payload,
            recorded_at_ms,
        )?;

        transaction
            .commit()
            .map_err(|error| format!("commit split work-unit transaction failed: {error}"))?;

        let parent = self
            .load_work_unit_snapshot(parent_work_unit_id.as_str())?
            .ok_or_else(|| "parent work unit disappeared after split".to_owned())?;
        let mut children = Vec::with_capacity(child_count);

        for child_work_unit_id in created_child_work_unit_ids.iter() {
            let child = self
                .load_work_unit_snapshot(child_work_unit_id.as_str())?
                .ok_or_else(|| {
                    format!("child work unit `{child_work_unit_id}` disappeared after split")
                })?;
            children.push(child);
        }

        Ok(SplitWorkUnitResult { parent, children })
    }

    pub fn supersede_work_unit(
        &self,
        request: SupersedeWorkUnitRequest,
    ) -> Result<SupersedeWorkUnitResult, String> {
        let obsolete_work_unit_id =
            normalize_required_text(&request.obsolete_work_unit_id, "obsolete_work_unit_id")?;
        let replacement_work_unit_id = normalize_required_text(
            &request.replacement_work_unit_id,
            "replacement_work_unit_id",
        )?;
        validate_supersede_endpoints(
            obsolete_work_unit_id.as_str(),
            replacement_work_unit_id.as_str(),
        )?;
        let actor = normalize_optional_text(request.actor);
        let now_ms = request.now_ms.unwrap_or_else(current_unix_ms);
        let mut connection = self.open_connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| format!("open supersede work-unit transaction failed: {error}"))?;
        let obsolete_snapshot =
            self.load_work_unit_snapshot_with_conn(&transaction, obsolete_work_unit_id.as_str())?;
        let obsolete_snapshot = obsolete_snapshot
            .ok_or_else(|| format!("work unit `{obsolete_work_unit_id}` not found"))?;
        let replacement_snapshot = self
            .load_work_unit_snapshot_with_conn(&transaction, replacement_work_unit_id.as_str())?;
        let replacement_snapshot = replacement_snapshot
            .ok_or_else(|| format!("work unit `{replacement_work_unit_id}` not found"))?;
        validate_supersede_transition(&obsolete_snapshot, &replacement_snapshot, now_ms)?;

        let incoming_blocking_work_unit_ids = obsolete_snapshot
            .work_unit
            .blocked_by_work_unit_ids
            .as_slice();
        let outgoing_blocked_work_unit_ids =
            obsolete_snapshot.work_unit.blocks_work_unit_ids.as_slice();

        for blocking_work_unit_id in incoming_blocking_work_unit_ids {
            self.add_dependency_in_tx(
                &transaction,
                blocking_work_unit_id.as_str(),
                replacement_work_unit_id.as_str(),
                actor.as_deref(),
                now_ms,
            )?;
        }
        for blocked_work_unit_id in outgoing_blocked_work_unit_ids {
            self.add_dependency_in_tx(
                &transaction,
                replacement_work_unit_id.as_str(),
                blocked_work_unit_id.as_str(),
                actor.as_deref(),
                now_ms,
            )?;
        }
        for blocking_work_unit_id in incoming_blocking_work_unit_ids {
            self.remove_dependency_in_tx(
                &transaction,
                blocking_work_unit_id.as_str(),
                obsolete_work_unit_id.as_str(),
                actor.as_deref(),
                now_ms,
            )?;
        }
        for blocked_work_unit_id in outgoing_blocked_work_unit_ids {
            self.remove_dependency_in_tx(
                &transaction,
                obsolete_work_unit_id.as_str(),
                blocked_work_unit_id.as_str(),
                actor.as_deref(),
                now_ms,
            )?;
        }

        transaction
            .execute(
                "UPDATE work_units
                 SET status = ?1,
                     superseded_by_work_unit_id = ?2,
                     blocking_reason = NULL,
                     lease_owner = NULL,
                     lease_version = 0,
                     lease_acquired_at_ms = NULL,
                     lease_heartbeat_at_ms = NULL,
                     lease_expires_at_ms = NULL,
                     updated_at_ms = ?3
                 WHERE work_unit_id = ?4
                   AND archived_at_ms IS NULL",
                params![
                    WorkUnitStatus::Cancelled.as_str(),
                    replacement_work_unit_id,
                    now_ms,
                    obsolete_work_unit_id,
                ],
            )
            .map_err(|error| format!("mark superseded work unit failed: {error}"))?;
        touch_work_unit(&transaction, replacement_work_unit_id.as_str(), now_ms)?;

        let event_payload = json!({
            "obsolete_work_unit_id": obsolete_work_unit_id,
            "replacement_work_unit_id": replacement_work_unit_id,
            "transferred_blocking_work_unit_ids": incoming_blocking_work_unit_ids,
            "transferred_blocked_work_unit_ids": outgoing_blocked_work_unit_ids,
        });
        insert_event_in_tx(
            &transaction,
            obsolete_work_unit_id.as_str(),
            WORK_UNIT_SUPERSEDED_EVENT_KIND,
            actor.as_deref(),
            &event_payload,
            now_ms,
        )?;

        transaction
            .commit()
            .map_err(|error| format!("commit supersede work-unit transaction failed: {error}"))?;

        let obsolete = self
            .load_work_unit_snapshot(obsolete_work_unit_id.as_str())?
            .ok_or_else(|| "obsolete work unit disappeared after supersede".to_owned())?;
        let replacement = self
            .load_work_unit_snapshot(replacement_work_unit_id.as_str())?
            .ok_or_else(|| "replacement work unit disappeared after supersede".to_owned())?;

        Ok(SupersedeWorkUnitResult {
            obsolete,
            replacement,
        })
    }

    fn load_parent_work_unit_for_children(
        &self,
        transaction: &Transaction<'_>,
        parent_work_unit_id: &str,
    ) -> Result<WorkUnitSnapshot, String> {
        let Some(parent_snapshot) =
            self.load_work_unit_snapshot_with_conn(transaction, parent_work_unit_id)?
        else {
            return Err(format!(
                "parent work unit `{parent_work_unit_id}` not found"
            ));
        };
        if parent_snapshot.work_unit.archived_at_ms.is_some() {
            return Err(format!(
                "cannot create child work for archived parent `{parent_work_unit_id}`"
            ));
        }
        Ok(parent_snapshot)
    }

    fn create_child_work_unit_in_tx(
        &self,
        transaction: &Transaction<'_>,
        parent_snapshot: &WorkUnitSnapshot,
        mut child: NewWorkUnitRecord,
        inherit_parent_source_ref: bool,
        inherit_parent_retry_policy: bool,
        inherit_parent_priority: bool,
        block_parent: bool,
        actor: Option<&str>,
    ) -> Result<WorkUnitSnapshot, String> {
        let parent_work_unit_id = parent_snapshot.work_unit.work_unit_id.as_str();
        child.parent_work_unit_id = Some(parent_work_unit_id.to_owned());
        if inherit_parent_source_ref {
            child.source_ref = parent_snapshot.work_unit.source_ref.clone();
        }
        if inherit_parent_retry_policy {
            child.retry_policy = parent_snapshot.work_unit.retry_policy.clone();
        }
        if inherit_parent_priority {
            child.priority = parent_snapshot.work_unit.priority;
        }
        let child_snapshot = self.create_work_unit_in_tx(transaction, child, actor)?;

        if block_parent {
            self.add_dependency_in_tx(
                transaction,
                child_snapshot.work_unit.work_unit_id.as_str(),
                parent_work_unit_id,
                actor,
                child_snapshot.work_unit.created_at_ms,
            )?;
        }

        let event_payload = json!({
            "parent_work_unit_id": parent_work_unit_id,
            "child_work_unit_id": child_snapshot.work_unit.work_unit_id,
            "block_parent": block_parent,
        });
        insert_event_in_tx(
            transaction,
            child_snapshot.work_unit.work_unit_id.as_str(),
            WORK_UNIT_CHILD_CREATED_EVENT_KIND,
            actor,
            &event_payload,
            child_snapshot.work_unit.created_at_ms,
        )?;

        Ok(child_snapshot)
    }

    pub fn resequence_child_work_units(
        &self,
        request: ResequenceChildWorkUnitsRequest,
    ) -> Result<ResequenceChildWorkUnitsResult, String> {
        let parent_work_unit_id =
            normalize_required_text(&request.parent_work_unit_id, "parent_work_unit_id")?;
        let actor = normalize_optional_text(request.actor);
        let now_ms = request.now_ms.unwrap_or_else(current_unix_ms);
        let ordered_child_work_unit_ids =
            normalize_resequence_child_ids(request.ordered_child_work_unit_ids.as_slice())?;
        let mut connection = self.open_connection()?;
        let transaction = connection
            .transaction()
            .map_err(|error| format!("open resequence work-unit transaction failed: {error}"))?;
        let parent_snapshot =
            self.load_parent_work_unit_for_children(&transaction, parent_work_unit_id.as_str())?;
        let current_child_work_unit_ids = parent_snapshot.work_unit.child_work_unit_ids.clone();
        validate_resequence_child_ids(
            parent_work_unit_id.as_str(),
            current_child_work_unit_ids.as_slice(),
            ordered_child_work_unit_ids.as_slice(),
        )?;
        let current_order_matches = current_child_work_unit_ids == ordered_child_work_unit_ids;
        if current_order_matches {
            return Ok(ResequenceChildWorkUnitsResult {
                parent: parent_snapshot,
                children: load_ordered_child_snapshots(
                    self,
                    ordered_child_work_unit_ids.as_slice(),
                )?,
            });
        }

        for (index, child_work_unit_id) in ordered_child_work_unit_ids.iter().enumerate() {
            let plan_position = i64::try_from(index + 1)
                .map_err(|error| format!("plan position overflowed i64: {error}"))?;
            set_child_plan_position_in_tx(
                &transaction,
                child_work_unit_id.as_str(),
                plan_position,
                now_ms,
            )?;
        }
        touch_work_unit(&transaction, parent_work_unit_id.as_str(), now_ms)?;
        let event_payload = json!({
            "parent_work_unit_id": parent_work_unit_id,
            "previous_child_work_unit_ids": current_child_work_unit_ids,
            "ordered_child_work_unit_ids": ordered_child_work_unit_ids,
        });
        insert_event_in_tx(
            &transaction,
            parent_work_unit_id.as_str(),
            WORK_UNIT_RESEQUENCED_EVENT_KIND,
            actor.as_deref(),
            &event_payload,
            now_ms,
        )?;
        transaction
            .commit()
            .map_err(|error| format!("commit resequence work-unit transaction failed: {error}"))?;

        let parent = self
            .load_work_unit_snapshot(parent_work_unit_id.as_str())?
            .ok_or_else(|| "parent work unit disappeared after resequence".to_owned())?;
        let children =
            load_ordered_child_snapshots(self, parent.work_unit.child_work_unit_ids.as_slice())?;

        Ok(ResequenceChildWorkUnitsResult { parent, children })
    }

    pub fn replan_work_unit(
        &self,
        request: ReplanWorkUnitRequest,
    ) -> Result<Option<ReplanWorkUnitResult>, String> {
        let work_unit_id = normalize_required_text(&request.work_unit_id, "work_unit_id")?;
        let actor = normalize_optional_text(request.actor);
        let now_ms = request.now_ms.unwrap_or_else(current_unix_ms);
        let ordered_child_work_unit_ids = request
            .ordered_child_work_unit_ids
            .as_ref()
            .map(|ids| normalize_resequence_child_ids(ids.as_slice()))
            .transpose()?;
        let mut connection = self.open_connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| format!("open work unit replan transaction failed: {error}"))?;
        let Some(raw_record) = load_raw_work_unit_with_conn(&transaction, &work_unit_id)? else {
            return Ok(None);
        };
        if raw_record.archived_at_ms.is_some() {
            return Ok(None);
        }

        let resolved_update = resolve_work_unit_update(
            &raw_record,
            request.title.as_deref(),
            request.description.as_deref(),
            request.status,
            request.priority,
            request.next_run_at_ms,
            request.blocking_reason.as_deref(),
            request.clear_blocking_reason,
        )?;
        let previous_child_work_unit_ids = raw_record.child_work_unit_ids.clone();
        let next_child_work_unit_ids = match ordered_child_work_unit_ids {
            Some(ordered_child_work_unit_ids) => {
                validate_resequence_child_ids(
                    work_unit_id.as_str(),
                    previous_child_work_unit_ids.as_slice(),
                    ordered_child_work_unit_ids.as_slice(),
                )?;
                apply_resequence_in_tx(
                    &transaction,
                    ordered_child_work_unit_ids.as_slice(),
                    now_ms,
                )?;
                ordered_child_work_unit_ids
            }
            None => previous_child_work_unit_ids.clone(),
        };
        let child_order_changed = previous_child_work_unit_ids != next_child_work_unit_ids;
        let update_changed = !resolved_update.changed_fields.is_empty();
        let changed = child_order_changed || update_changed;
        if !changed {
            transaction.commit().map_err(|error| {
                format!("commit unchanged work unit replan transaction failed: {error}")
            })?;
            let parent = self.load_work_unit_snapshot(work_unit_id.as_str())?;
            let parent =
                parent.ok_or_else(|| "work unit disappeared after unchanged replan".to_owned())?;
            let children = load_ordered_child_snapshots(
                self,
                parent.work_unit.child_work_unit_ids.as_slice(),
            )?;
            return Ok(Some(ReplanWorkUnitResult { parent, children }));
        }

        if update_changed {
            apply_resolved_work_unit_update_in_tx(
                &transaction,
                work_unit_id.as_str(),
                &resolved_update,
                now_ms,
            )?;
        } else {
            touch_work_unit(&transaction, work_unit_id.as_str(), now_ms)?;
        }

        let event_payload = json!({
            "changed_fields": resolved_update.changed_fields,
            "previous": {
                "title": raw_record.title,
                "description": raw_record.description,
                "status": raw_record.status,
                "priority": raw_record.priority,
                "next_run_at_ms": raw_record.next_run_at_ms,
                "blocking_reason": raw_record.blocking_reason,
                "ordered_child_work_unit_ids": previous_child_work_unit_ids,
            },
            "current": {
                "title": resolved_update.next_title,
                "description": resolved_update.next_description,
                "status": resolved_update.next_status,
                "priority": resolved_update.next_priority,
                "next_run_at_ms": resolved_update.next_next_run_at_ms,
                "blocking_reason": resolved_update.next_blocking_reason,
                "ordered_child_work_unit_ids": next_child_work_unit_ids,
            }
        });
        insert_event_in_tx(
            &transaction,
            work_unit_id.as_str(),
            WORK_UNIT_REPLANNED_EVENT_KIND,
            actor.as_deref(),
            &event_payload,
            now_ms,
        )?;
        transaction
            .commit()
            .map_err(|error| format!("commit work unit replan transaction failed: {error}"))?;

        let parent = self.load_work_unit_snapshot(work_unit_id.as_str())?;
        let parent = parent.ok_or_else(|| "work unit disappeared after replan".to_owned())?;
        let children =
            load_ordered_child_snapshots(self, parent.work_unit.child_work_unit_ids.as_slice())?;

        Ok(Some(ReplanWorkUnitResult { parent, children }))
    }

    pub fn load_work_unit_snapshot(
        &self,
        work_unit_id: &str,
    ) -> Result<Option<WorkUnitSnapshot>, String> {
        let work_unit_id = normalize_required_text(work_unit_id, "work_unit_id")?;
        let connection = self.open_connection()?;
        self.load_work_unit_snapshot_with_conn(&connection, &work_unit_id)
    }

    fn load_work_unit_snapshot_with_conn(
        &self,
        connection: &Connection,
        work_unit_id: &str,
    ) -> Result<Option<WorkUnitSnapshot>, String> {
        let raw = load_raw_work_unit_with_conn(connection, work_unit_id)?;
        raw.map(try_work_unit_snapshot_from_raw).transpose()
    }

    pub fn list_work_units(
        &self,
        query: WorkUnitListQuery,
    ) -> Result<Vec<WorkUnitSnapshot>, String> {
        let limit = normalize_limit(query.limit)?;
        let connection = self.open_connection()?;
        let raw_records = load_raw_work_units_with_query(
            &connection,
            query.status,
            query.include_archived,
            limit,
        )?;
        let mut snapshots = Vec::with_capacity(raw_records.len());

        for raw_record in raw_records {
            let snapshot = try_work_unit_snapshot_from_raw(raw_record)?;
            snapshots.push(snapshot);
        }

        Ok(snapshots)
    }

    pub fn list_work_unit_events(
        &self,
        work_unit_id: &str,
        limit: usize,
    ) -> Result<Vec<WorkUnitEventRecord>, String> {
        let work_unit_id = normalize_required_text(work_unit_id, "work_unit_id")?;
        let limit = normalize_limit(limit)?;
        let limit =
            i64::try_from(limit).map_err(|error| format!("event limit overflowed i64: {error}"))?;
        let connection = self.open_connection()?;
        let mut statement = connection
            .prepare(
                "SELECT id, work_unit_id, event_kind, actor, payload_json, recorded_at_ms
                 FROM work_unit_events
                 WHERE work_unit_id = ?1
                 ORDER BY id DESC
                 LIMIT ?2",
            )
            .map_err(|error| format!("prepare work unit event query failed: {error}"))?;
        let rows = statement
            .query_map(params![work_unit_id, limit], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, i64>(5)?,
                ))
            })
            .map_err(|error| format!("query work unit events failed: {error}"))?;
        let mut events = Vec::new();

        for row in rows {
            let (sequence_id, row_work_unit_id, event_kind, actor, payload_json, recorded_at_ms) =
                row.map_err(|error| format!("decode work unit event row failed: {error}"))?;
            let payload_value = decode_json::<Value>(&payload_json, "work unit event payload")?;
            let event = WorkUnitEventRecord {
                sequence_id,
                work_unit_id: row_work_unit_id,
                event_kind,
                actor,
                payload_json: payload_value,
                recorded_at_ms,
            };
            events.push(event);
        }

        Ok(events)
    }

    pub fn acquire_next_ready_lease(
        &self,
        request: AcquireWorkUnitLeaseRequest,
    ) -> Result<Option<WorkUnitSnapshot>, String> {
        let owner = normalize_required_text(&request.owner, "owner")?;
        validate_ttl_ms(request.ttl_ms)?;
        let actor = normalize_optional_text(request.actor);
        let now_ms = request.now_ms.unwrap_or_else(current_unix_ms);
        let expires_at_ms = add_delay_ms(now_ms, request.ttl_ms)?;
        let mut connection = self.open_connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| format!("open work unit lease transaction failed: {error}"))?;
        let Some(raw_record) = select_next_ready_raw_work_unit(&transaction, now_ms)? else {
            return Ok(None);
        };
        let next_lease_version = raw_record.lease_version + 1;

        transaction
            .execute(
                "UPDATE work_units
                 SET status = ?1,
                     attempt_count = attempt_count + 1,
                     lease_owner = ?2,
                     lease_version = ?3,
                     lease_acquired_at_ms = ?4,
                     lease_heartbeat_at_ms = ?4,
                     lease_expires_at_ms = ?5,
                     updated_at_ms = ?4
                 WHERE work_unit_id = ?6
                   AND status IN ('ready', 'retry_pending')
                   AND archived_at_ms IS NULL
                   AND next_run_at_ms <= ?4
                   AND (lease_expires_at_ms IS NULL OR lease_expires_at_ms <= ?4)",
                params![
                    WorkUnitStatus::Leased.as_str(),
                    owner,
                    next_lease_version,
                    now_ms,
                    expires_at_ms,
                    raw_record.work_unit_id,
                ],
            )
            .map_err(|error| format!("update work unit lease state failed: {error}"))?;

        let event_payload = json!({
            "owner": owner,
            "lease_version": next_lease_version,
            "previous_status": raw_record.status,
            "ttl_ms": request.ttl_ms,
            "expires_at_ms": expires_at_ms,
        });
        insert_event_in_tx(
            &transaction,
            &raw_record.work_unit_id,
            WORK_UNIT_LEASED_EVENT_KIND,
            actor.as_deref(),
            &event_payload,
            now_ms,
        )?;

        transaction
            .commit()
            .map_err(|error| format!("commit work unit lease transaction failed: {error}"))?;

        self.load_work_unit_snapshot(&raw_record.work_unit_id)
    }

    pub fn mark_leased_running(
        &self,
        request: StartWorkUnitLeaseRequest,
    ) -> Result<Option<WorkUnitSnapshot>, String> {
        let work_unit_id = normalize_required_text(&request.work_unit_id, "work_unit_id")?;
        let owner = normalize_required_text(&request.owner, "owner")?;
        let actor = normalize_optional_text(request.actor);
        let now_ms = request.now_ms.unwrap_or_else(current_unix_ms);
        let mut connection = self.open_connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| format!("open work unit start transaction failed: {error}"))?;
        let Some(raw_record) = load_raw_work_unit_with_conn(&transaction, &work_unit_id)? else {
            return Ok(None);
        };
        if raw_record.status != WorkUnitStatus::Leased.as_str() {
            return Ok(None);
        }
        if raw_record.lease_owner.as_deref() != Some(owner.as_str()) {
            return Ok(None);
        }

        transaction
            .execute(
                "UPDATE work_units
                 SET status = ?1,
                     updated_at_ms = ?2
                 WHERE work_unit_id = ?3
                   AND status = ?4
                   AND lease_owner = ?5",
                params![
                    WorkUnitStatus::Running.as_str(),
                    now_ms,
                    work_unit_id,
                    WorkUnitStatus::Leased.as_str(),
                    owner,
                ],
            )
            .map_err(|error| format!("mark work unit running failed: {error}"))?;

        let event_payload = json!({
            "owner": owner,
            "previous_status": raw_record.status,
        });
        insert_event_in_tx(
            &transaction,
            &work_unit_id,
            WORK_UNIT_STARTED_EVENT_KIND,
            actor.as_deref(),
            &event_payload,
            now_ms,
        )?;

        transaction
            .commit()
            .map_err(|error| format!("commit work unit start transaction failed: {error}"))?;

        self.load_work_unit_snapshot(&work_unit_id)
    }

    pub fn heartbeat_lease(
        &self,
        request: WorkUnitHeartbeatRequest,
    ) -> Result<Option<WorkUnitSnapshot>, String> {
        let work_unit_id = normalize_required_text(&request.work_unit_id, "work_unit_id")?;
        let owner = normalize_required_text(&request.owner, "owner")?;
        validate_ttl_ms(request.ttl_ms)?;
        let actor = normalize_optional_text(request.actor);
        let now_ms = request.now_ms.unwrap_or_else(current_unix_ms);
        let expires_at_ms = add_delay_ms(now_ms, request.ttl_ms)?;
        let mut connection = self.open_connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| format!("open work unit heartbeat transaction failed: {error}"))?;
        let Some(raw_record) = load_raw_work_unit_with_conn(&transaction, &work_unit_id)? else {
            return Ok(None);
        };
        let status = raw_record.status.as_str();
        let is_active = status == WorkUnitStatus::Leased.as_str();
        let is_running = status == WorkUnitStatus::Running.as_str();
        if !is_active && !is_running {
            return Ok(None);
        }
        if raw_record.lease_owner.as_deref() != Some(owner.as_str()) {
            return Ok(None);
        }

        transaction
            .execute(
                "UPDATE work_units
                 SET lease_heartbeat_at_ms = ?1,
                     lease_expires_at_ms = ?2,
                     updated_at_ms = ?1
                 WHERE work_unit_id = ?3
                   AND lease_owner = ?4
                   AND status IN ('leased', 'running')",
                params![now_ms, expires_at_ms, work_unit_id, owner],
            )
            .map_err(|error| format!("update work unit heartbeat failed: {error}"))?;

        let event_payload = json!({
            "owner": owner,
            "ttl_ms": request.ttl_ms,
            "expires_at_ms": expires_at_ms,
        });
        insert_event_in_tx(
            &transaction,
            &work_unit_id,
            WORK_UNIT_HEARTBEAT_EVENT_KIND,
            actor.as_deref(),
            &event_payload,
            now_ms,
        )?;

        transaction
            .commit()
            .map_err(|error| format!("commit work unit heartbeat transaction failed: {error}"))?;

        self.load_work_unit_snapshot(&work_unit_id)
    }

    pub fn complete_work_unit(
        &self,
        request: CompleteWorkUnitRequest,
    ) -> Result<Option<WorkUnitSnapshot>, String> {
        let work_unit_id = normalize_required_text(&request.work_unit_id, "work_unit_id")?;
        let owner = normalize_required_text(&request.owner, "owner")?;
        let actor = normalize_optional_text(request.actor);
        let now_ms = request.now_ms.unwrap_or_else(current_unix_ms);
        let mut connection = self.open_connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| format!("open work unit complete transaction failed: {error}"))?;
        let Some(raw_record) = load_raw_work_unit_with_conn(&transaction, &work_unit_id)? else {
            return Ok(None);
        };
        let status = raw_record.status.as_str();
        let is_leased = status == WorkUnitStatus::Leased.as_str();
        let is_running = status == WorkUnitStatus::Running.as_str();
        if !is_leased && !is_running {
            return Ok(None);
        }
        if raw_record.lease_owner.as_deref() != Some(owner.as_str()) {
            return Ok(None);
        }

        let retry_policy =
            decode_json::<WorkUnitRetryPolicy>(&raw_record.retry_policy_json, "retry policy")?;
        let attempt_count = u32::try_from(raw_record.attempt_count)
            .map_err(|error| format!("attempt_count overflowed u32: {error}"))?;
        let error = normalize_optional_text(request.error);
        let result_payload_json = request
            .result_payload_json
            .as_ref()
            .map(|value| encode_json(value, "result_payload"))
            .transpose()?;
        let completion = resolve_completion(
            request.disposition,
            &retry_policy,
            attempt_count,
            now_ms,
            request.next_run_at_ms,
            error.as_deref(),
        )?;

        transaction
            .execute(
                "UPDATE work_units
                 SET status = ?1,
                     next_run_at_ms = ?2,
                     last_error = ?3,
                     blocking_reason = NULL,
                     result_payload_json = ?4,
                     lease_owner = NULL,
                     lease_acquired_at_ms = NULL,
                     lease_heartbeat_at_ms = NULL,
                     lease_expires_at_ms = NULL,
                     updated_at_ms = ?5
                 WHERE work_unit_id = ?6
                   AND lease_owner = ?7
                   AND status IN ('leased', 'running')",
                params![
                    completion.status.as_str(),
                    completion.next_run_at_ms,
                    completion.last_error,
                    result_payload_json,
                    now_ms,
                    work_unit_id,
                    owner,
                ],
            )
            .map_err(|error| format!("update completed work unit failed: {error}"))?;

        let event_payload = json!({
            "owner": owner,
            "previous_status": raw_record.status,
            "next_status": completion.status.as_str(),
            "next_run_at_ms": completion.next_run_at_ms,
            "last_error": completion.last_error,
            "attempt_count": attempt_count,
        });
        insert_event_in_tx(
            &transaction,
            &work_unit_id,
            completion.event_kind,
            actor.as_deref(),
            &event_payload,
            now_ms,
        )?;

        transaction
            .commit()
            .map_err(|error| format!("commit work unit complete transaction failed: {error}"))?;

        self.load_work_unit_snapshot(&work_unit_id)
    }

    pub fn archive_work_unit(
        &self,
        request: ArchiveWorkUnitRequest,
    ) -> Result<Option<WorkUnitSnapshot>, String> {
        let work_unit_id = normalize_required_text(&request.work_unit_id, "work_unit_id")?;
        let actor = normalize_optional_text(request.actor);
        let now_ms = request.now_ms.unwrap_or_else(current_unix_ms);
        let mut connection = self.open_connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| format!("open work unit archive transaction failed: {error}"))?;
        let Some(raw_record) = load_raw_work_unit_with_conn(&transaction, &work_unit_id)? else {
            return Ok(None);
        };
        let current_status = WorkUnitStatus::parse(&raw_record.status)
            .ok_or_else(|| format!("unknown work unit status `{}`", raw_record.status))?;
        if !current_status.is_terminal() {
            return Ok(None);
        }
        if raw_record.archived_at_ms.is_some() {
            return Ok(None);
        }

        transaction
            .execute(
                "UPDATE work_units
                 SET status = ?1,
                     archived_at_ms = ?2,
                     updated_at_ms = ?2
                 WHERE work_unit_id = ?3
                   AND archived_at_ms IS NULL",
                params![WorkUnitStatus::Archived.as_str(), now_ms, work_unit_id],
            )
            .map_err(|error| format!("archive work unit failed: {error}"))?;

        let event_payload = json!({
            "previous_status": raw_record.status,
            "archived_at_ms": now_ms,
        });
        insert_event_in_tx(
            &transaction,
            &work_unit_id,
            WORK_UNIT_ARCHIVED_EVENT_KIND,
            actor.as_deref(),
            &event_payload,
            now_ms,
        )?;

        transaction
            .commit()
            .map_err(|error| format!("commit work unit archive transaction failed: {error}"))?;

        self.load_work_unit_snapshot(&work_unit_id)
    }

    pub fn update_work_unit(
        &self,
        request: UpdateWorkUnitRequest,
    ) -> Result<Option<WorkUnitSnapshot>, String> {
        let work_unit_id = normalize_required_text(&request.work_unit_id, "work_unit_id")?;
        let actor = normalize_optional_text(request.actor);
        let now_ms = request.now_ms.unwrap_or_else(current_unix_ms);
        let mut connection = self.open_connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| format!("open work unit update transaction failed: {error}"))?;
        let Some(raw_record) = load_raw_work_unit_with_conn(&transaction, &work_unit_id)? else {
            return Ok(None);
        };
        if raw_record.archived_at_ms.is_some() {
            return Ok(None);
        }
        let resolved_update = resolve_work_unit_update(
            &raw_record,
            request.title.as_deref(),
            request.description.as_deref(),
            request.status,
            request.priority,
            request.next_run_at_ms,
            request.blocking_reason.as_deref(),
            request.clear_blocking_reason,
        )?;

        if resolved_update.changed_fields.is_empty() {
            transaction.commit().map_err(|error| {
                format!("commit unchanged work unit update transaction failed: {error}")
            })?;
            return self.load_work_unit_snapshot(&work_unit_id);
        }

        apply_resolved_work_unit_update_in_tx(
            &transaction,
            &work_unit_id,
            &resolved_update,
            now_ms,
        )?;

        let event_payload = json!({
            "changed_fields": resolved_update.changed_fields,
            "previous": {
                "title": raw_record.title,
                "description": raw_record.description,
                "status": raw_record.status,
                "priority": raw_record.priority,
                "next_run_at_ms": raw_record.next_run_at_ms,
                "blocking_reason": raw_record.blocking_reason,
            },
            "current": {
                "title": resolved_update.next_title,
                "description": resolved_update.next_description,
                "status": resolved_update.next_status,
                "priority": resolved_update.next_priority,
                "next_run_at_ms": resolved_update.next_next_run_at_ms,
                "blocking_reason": resolved_update.next_blocking_reason,
            }
        });
        insert_event_in_tx(
            &transaction,
            &work_unit_id,
            WORK_UNIT_UPDATED_EVENT_KIND,
            actor.as_deref(),
            &event_payload,
            now_ms,
        )?;

        transaction
            .commit()
            .map_err(|error| format!("commit work unit update transaction failed: {error}"))?;

        self.load_work_unit_snapshot(&work_unit_id)
    }

    pub fn request_work_unit_review(
        &self,
        request: RequestWorkUnitReviewRequest,
    ) -> Result<Option<WorkUnitSnapshot>, String> {
        let work_unit_id = normalize_required_text(&request.work_unit_id, "work_unit_id")?;
        let requested_by = normalize_optional_text(request.requested_by);
        let reviewer = normalize_optional_text(request.reviewer);
        let summary = normalize_optional_text(request.summary);
        let now_ms = request.now_ms.unwrap_or_else(current_unix_ms);
        let mut connection = self.open_connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| {
                format!("open work unit review request transaction failed: {error}")
            })?;
        let Some(raw_record) = load_raw_work_unit_with_conn(&transaction, &work_unit_id)? else {
            return Ok(None);
        };
        if raw_record.archived_at_ms.is_some() {
            return Ok(None);
        }

        let current_status = WorkUnitStatus::parse(&raw_record.status)
            .ok_or_else(|| format!("unknown work unit status `{}`", raw_record.status))?;
        validate_review_request_status(current_status)?;
        let review = WorkUnitReviewRecord {
            status: WorkUnitReviewStatus::Pending,
            requested_by: requested_by.clone(),
            reviewer: reviewer.clone(),
            requested_at_ms: now_ms,
            decided_at_ms: None,
            summary: summary.clone(),
        };
        let review_json = encode_json(&review, "review")?;
        let next_blocking_reason = summary
            .clone()
            .or_else(|| Some("waiting for review".to_owned()));

        transaction
            .execute(
                "UPDATE work_units
                 SET status = ?1,
                     review_json = ?2,
                     blocking_reason = ?3,
                     updated_at_ms = ?4
                 WHERE work_unit_id = ?5
                   AND archived_at_ms IS NULL",
                params![
                    WorkUnitStatus::WaitingReview.as_str(),
                    review_json,
                    next_blocking_reason,
                    now_ms,
                    work_unit_id,
                ],
            )
            .map_err(|error| format!("request work unit review failed: {error}"))?;

        let event_payload = json!({
            "requested_by": requested_by,
            "reviewer": reviewer,
            "summary": summary,
            "previous_status": raw_record.status,
            "next_status": WorkUnitStatus::WaitingReview.as_str(),
        });
        insert_event_in_tx(
            &transaction,
            &work_unit_id,
            WORK_UNIT_REVIEW_REQUESTED_EVENT_KIND,
            review.requested_by.as_deref(),
            &event_payload,
            now_ms,
        )?;

        transaction.commit().map_err(|error| {
            format!("commit work unit review request transaction failed: {error}")
        })?;

        self.load_work_unit_snapshot(&work_unit_id)
    }

    pub fn record_work_unit_review_decision(
        &self,
        request: RecordWorkUnitReviewDecisionRequest,
    ) -> Result<Option<WorkUnitSnapshot>, String> {
        let work_unit_id = normalize_required_text(&request.work_unit_id, "work_unit_id")?;
        let reviewer = normalize_optional_text(request.reviewer);
        let summary = normalize_optional_text(request.summary);
        let now_ms = request.now_ms.unwrap_or_else(current_unix_ms);
        let mut connection = self.open_connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| {
                format!("open work unit review decision transaction failed: {error}")
            })?;
        let Some(raw_record) = load_raw_work_unit_with_conn(&transaction, &work_unit_id)? else {
            return Ok(None);
        };
        if raw_record.archived_at_ms.is_some() {
            return Ok(None);
        }

        let review = raw_record
            .review_json
            .as_deref()
            .map(|value| decode_json::<WorkUnitReviewRecord>(value, "review"))
            .transpose()?;
        let Some(previous_review) = review else {
            return Err(format!(
                "work unit `{}` does not have a pending review request",
                work_unit_id
            ));
        };
        if previous_review.status != WorkUnitReviewStatus::Pending {
            return Err(format!(
                "work unit `{}` review is no longer pending",
                work_unit_id
            ));
        }

        let current_status = WorkUnitStatus::parse(&raw_record.status)
            .ok_or_else(|| format!("unknown work unit status `{}`", raw_record.status))?;
        if current_status != WorkUnitStatus::WaitingReview {
            return Err(format!(
                "work unit `{}` is not currently waiting for review",
                work_unit_id
            ));
        }

        let next_review_status = review_status_from_decision(request.decision);
        let next_status = work_unit_status_from_review_decision(request.decision);
        let next_blocking_reason =
            blocking_reason_from_review_decision(request.decision, summary.as_deref());
        let next_review = WorkUnitReviewRecord {
            status: next_review_status,
            requested_by: previous_review.requested_by.clone(),
            reviewer: reviewer.or(previous_review.reviewer),
            requested_at_ms: previous_review.requested_at_ms,
            decided_at_ms: Some(now_ms),
            summary: summary.or(previous_review.summary),
        };
        let next_review_json = encode_json(&next_review, "review")?;

        transaction
            .execute(
                "UPDATE work_units
                 SET status = ?1,
                     review_json = ?2,
                     blocking_reason = ?3,
                     updated_at_ms = ?4
                 WHERE work_unit_id = ?5
                   AND archived_at_ms IS NULL",
                params![
                    next_status.as_str(),
                    next_review_json,
                    next_blocking_reason,
                    now_ms,
                    work_unit_id,
                ],
            )
            .map_err(|error| format!("record work unit review decision failed: {error}"))?;

        let event_payload = json!({
            "review_status": next_review_status.as_str(),
            "reviewer": next_review.reviewer,
            "summary": next_review.summary,
            "previous_status": raw_record.status,
            "next_status": next_status.as_str(),
        });
        insert_event_in_tx(
            &transaction,
            &work_unit_id,
            WORK_UNIT_REVIEW_RECORDED_EVENT_KIND,
            next_review.reviewer.as_deref(),
            &event_payload,
            now_ms,
        )?;

        transaction.commit().map_err(|error| {
            format!("commit work unit review decision transaction failed: {error}")
        })?;

        self.load_work_unit_snapshot(&work_unit_id)
    }

    pub fn assign_work_unit(
        &self,
        request: AssignWorkUnitRequest,
    ) -> Result<Option<WorkUnitSnapshot>, String> {
        let work_unit_id = normalize_required_text(&request.work_unit_id, "work_unit_id")?;
        let assigned_to = normalize_optional_text(request.assigned_to);
        let actor = normalize_optional_text(request.actor);
        let now_ms = request.now_ms.unwrap_or_else(current_unix_ms);
        let mut connection = self.open_connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| format!("open work unit assignment transaction failed: {error}"))?;
        let Some(raw_record) = load_raw_work_unit_with_conn(&transaction, &work_unit_id)? else {
            return Ok(None);
        };
        if raw_record.archived_at_ms.is_some() {
            return Ok(None);
        }

        let previous_assigned_to = raw_record.assigned_to;
        let changed = previous_assigned_to != assigned_to;
        if !changed {
            transaction.commit().map_err(|error| {
                format!("commit unchanged work unit assignment transaction failed: {error}")
            })?;
            return self.load_work_unit_snapshot(&work_unit_id);
        }

        transaction
            .execute(
                "UPDATE work_units
                 SET assigned_to = ?1,
                     updated_at_ms = ?2
                 WHERE work_unit_id = ?3
                   AND archived_at_ms IS NULL",
                params![assigned_to, now_ms, work_unit_id],
            )
            .map_err(|error| format!("assign work unit failed: {error}"))?;

        let event_payload = json!({
            "previous_assigned_to": previous_assigned_to,
            "assigned_to": assigned_to,
        });
        insert_event_in_tx(
            &transaction,
            &work_unit_id,
            WORK_UNIT_ASSIGNED_EVENT_KIND,
            actor.as_deref(),
            &event_payload,
            now_ms,
        )?;

        transaction
            .commit()
            .map_err(|error| format!("commit work unit assignment transaction failed: {error}"))?;

        self.load_work_unit_snapshot(&work_unit_id)
    }

    pub fn add_dependency(
        &self,
        request: AddWorkUnitDependencyRequest,
    ) -> Result<Option<WorkUnitSnapshot>, String> {
        let blocking_work_unit_id =
            normalize_required_text(&request.blocking_work_unit_id, "blocking_work_unit_id")?;
        let blocked_work_unit_id =
            normalize_required_text(&request.blocked_work_unit_id, "blocked_work_unit_id")?;
        let actor = normalize_optional_text(request.actor);
        let now_ms = request.now_ms.unwrap_or_else(current_unix_ms);
        validate_dependency_endpoints(
            blocking_work_unit_id.as_str(),
            blocked_work_unit_id.as_str(),
        )?;
        let mut connection = self.open_connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| format!("open work unit dependency transaction failed: {error}"))?;
        self.add_dependency_in_tx(
            &transaction,
            blocking_work_unit_id.as_str(),
            blocked_work_unit_id.as_str(),
            actor.as_deref(),
            now_ms,
        )?;

        transaction
            .commit()
            .map_err(|error| format!("commit work unit dependency transaction failed: {error}"))?;

        self.load_work_unit_snapshot(&blocked_work_unit_id)
    }

    fn add_dependency_in_tx(
        &self,
        transaction: &Transaction<'_>,
        blocking_work_unit_id: &str,
        blocked_work_unit_id: &str,
        actor: Option<&str>,
        now_ms: i64,
    ) -> Result<(), String> {
        ensure_work_unit_exists(transaction, blocking_work_unit_id)?;
        ensure_work_unit_exists(transaction, blocked_work_unit_id)?;
        let creates_cycle = would_create_dependency_cycle(
            transaction,
            blocking_work_unit_id,
            blocked_work_unit_id,
        )?;
        if creates_cycle {
            return Err(format!(
                "work unit dependency would create a cycle: `{}` -> `{}`",
                blocking_work_unit_id, blocked_work_unit_id
            ));
        }

        let inserted_rows = transaction
            .execute(
                "INSERT OR IGNORE INTO work_unit_dependencies(
                    blocking_work_unit_id,
                    blocked_work_unit_id,
                    created_at_ms,
                    created_by
                 ) VALUES (?1, ?2, ?3, ?4)",
                params![blocking_work_unit_id, blocked_work_unit_id, now_ms, actor],
            )
            .map_err(|error| format!("insert work unit dependency failed: {error}"))?;
        if inserted_rows == 0 {
            return Ok(());
        }

        touch_work_unit(transaction, blocked_work_unit_id, now_ms)?;
        touch_work_unit(transaction, blocking_work_unit_id, now_ms)?;
        let event_payload = json!({
            "blocking_work_unit_id": blocking_work_unit_id,
            "blocked_work_unit_id": blocked_work_unit_id,
        });
        insert_event_in_tx(
            transaction,
            blocked_work_unit_id,
            WORK_UNIT_DEPENDENCY_ADDED_EVENT_KIND,
            actor,
            &event_payload,
            now_ms,
        )?;

        Ok(())
    }

    pub fn remove_dependency(
        &self,
        request: RemoveWorkUnitDependencyRequest,
    ) -> Result<Option<WorkUnitSnapshot>, String> {
        let blocking_work_unit_id =
            normalize_required_text(&request.blocking_work_unit_id, "blocking_work_unit_id")?;
        let blocked_work_unit_id =
            normalize_required_text(&request.blocked_work_unit_id, "blocked_work_unit_id")?;
        let actor = normalize_optional_text(request.actor);
        let now_ms = request.now_ms.unwrap_or_else(current_unix_ms);
        validate_dependency_endpoints(
            blocking_work_unit_id.as_str(),
            blocked_work_unit_id.as_str(),
        )?;
        let mut connection = self.open_connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| {
                format!("open work unit dependency removal transaction failed: {error}")
            })?;
        let Some(_raw_record) = load_raw_work_unit_with_conn(&transaction, &blocked_work_unit_id)?
        else {
            return Ok(None);
        };

        self.remove_dependency_in_tx(
            &transaction,
            blocking_work_unit_id.as_str(),
            blocked_work_unit_id.as_str(),
            actor.as_deref(),
            now_ms,
        )?;

        transaction.commit().map_err(|error| {
            format!("commit work unit dependency removal transaction failed: {error}")
        })?;

        self.load_work_unit_snapshot(&blocked_work_unit_id)
    }

    fn remove_dependency_in_tx(
        &self,
        transaction: &Transaction<'_>,
        blocking_work_unit_id: &str,
        blocked_work_unit_id: &str,
        actor: Option<&str>,
        now_ms: i64,
    ) -> Result<(), String> {
        let removed_rows = transaction
            .execute(
                "DELETE FROM work_unit_dependencies
                 WHERE blocking_work_unit_id = ?1
                   AND blocked_work_unit_id = ?2",
                params![blocking_work_unit_id, blocked_work_unit_id],
            )
            .map_err(|error| format!("remove work unit dependency failed: {error}"))?;

        if removed_rows == 0 {
            return Ok(());
        }

        touch_work_unit(transaction, blocked_work_unit_id, now_ms)?;
        touch_work_unit(transaction, blocking_work_unit_id, now_ms)?;
        let event_payload = json!({
            "blocking_work_unit_id": blocking_work_unit_id,
            "blocked_work_unit_id": blocked_work_unit_id,
        });
        insert_event_in_tx(
            transaction,
            blocked_work_unit_id,
            WORK_UNIT_DEPENDENCY_REMOVED_EVENT_KIND,
            actor,
            &event_payload,
            now_ms,
        )?;
        Ok(())
    }

    pub fn append_note(
        &self,
        request: AppendWorkUnitNoteRequest,
    ) -> Result<Option<WorkUnitEventRecord>, String> {
        let work_unit_id = normalize_required_text(&request.work_unit_id, "work_unit_id")?;
        let note = normalize_required_text(&request.note, "note")?;
        let actor = normalize_optional_text(request.actor);
        let now_ms = request.now_ms.unwrap_or_else(current_unix_ms);
        let mut connection = self.open_connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| format!("open work unit note transaction failed: {error}"))?;
        let Some(raw_record) = load_raw_work_unit_with_conn(&transaction, &work_unit_id)? else {
            return Ok(None);
        };
        if raw_record.archived_at_ms.is_some() {
            return Ok(None);
        }

        touch_work_unit(&transaction, work_unit_id.as_str(), now_ms)?;
        let event_payload = json!({
            "note": note,
        });
        let event = insert_event_in_tx(
            &transaction,
            work_unit_id.as_str(),
            WORK_UNIT_NOTE_ADDED_EVENT_KIND,
            actor.as_deref(),
            &event_payload,
            now_ms,
        )?;
        transaction
            .commit()
            .map_err(|error| format!("commit work unit note transaction failed: {error}"))?;

        Ok(Some(event))
    }

    pub fn recover_expired_leases(
        &self,
        actor: Option<&str>,
        now_ms: Option<i64>,
    ) -> Result<Vec<WorkUnitSnapshot>, String> {
        let normalized_actor = normalize_optional_text(actor.map(str::to_owned));
        let now_ms = now_ms.unwrap_or_else(current_unix_ms);
        let mut connection = self.open_connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| format!("open expired lease recovery transaction failed: {error}"))?;
        let raw_records = load_expired_raw_work_units_with_conn(&transaction, now_ms)?;
        let mut recovered_ids = Vec::new();

        for raw_record in raw_records {
            let retry_policy =
                decode_json::<WorkUnitRetryPolicy>(&raw_record.retry_policy_json, "retry policy")?;
            let attempt_count = u32::try_from(raw_record.attempt_count)
                .map_err(|error| format!("attempt_count overflowed u32: {error}"))?;
            let previous_owner = raw_record.lease_owner.clone();
            let previous_status = raw_record.status.clone();
            let expiration_error = build_expired_lease_error(&raw_record)?;
            let recovery =
                resolve_recovery(&retry_policy, attempt_count, now_ms, &expiration_error)?;

            transaction
                .execute(
                    "UPDATE work_units
                     SET status = ?1,
                         next_run_at_ms = ?2,
                         last_error = ?3,
                         lease_owner = NULL,
                         lease_acquired_at_ms = NULL,
                         lease_heartbeat_at_ms = NULL,
                         lease_expires_at_ms = NULL,
                         updated_at_ms = ?4
                     WHERE work_unit_id = ?5
                       AND status IN ('leased', 'running')
                       AND lease_expires_at_ms IS NOT NULL
                       AND lease_expires_at_ms < ?4",
                    params![
                        recovery.status.as_str(),
                        recovery.next_run_at_ms,
                        recovery.last_error,
                        now_ms,
                        raw_record.work_unit_id,
                    ],
                )
                .map_err(|error| format!("recover expired work unit lease failed: {error}"))?;

            let event_payload = json!({
                "previous_owner": previous_owner,
                "previous_status": previous_status,
                "next_status": recovery.status.as_str(),
                "next_run_at_ms": recovery.next_run_at_ms,
                "last_error": recovery.last_error,
                "attempt_count": attempt_count,
            });
            insert_event_in_tx(
                &transaction,
                &raw_record.work_unit_id,
                WORK_UNIT_LEASE_EXPIRED_EVENT_KIND,
                normalized_actor.as_deref(),
                &event_payload,
                now_ms,
            )?;
            recovered_ids.push(raw_record.work_unit_id);
        }

        transaction.commit().map_err(|error| {
            format!("commit expired lease recovery transaction failed: {error}")
        })?;

        let mut recovered_snapshots = Vec::new();
        for recovered_id in recovered_ids {
            let snapshot = self
                .load_work_unit_snapshot(&recovered_id)?
                .ok_or_else(|| format!("recovered work unit `{recovered_id}` disappeared"))?;
            recovered_snapshots.push(snapshot);
        }

        Ok(recovered_snapshots)
    }

    pub fn load_runtime_health(
        &self,
        now_ms: Option<i64>,
    ) -> Result<WorkRuntimeHealthSnapshot, String> {
        let now_ms = now_ms.unwrap_or_else(current_unix_ms);
        let connection = self.open_connection()?;
        let row = connection
            .query_row(
                "SELECT
                    COUNT(*),
                    SUM(CASE WHEN status = 'ready' THEN 1 ELSE 0 END),
                    SUM(CASE WHEN status = 'leased' THEN 1 ELSE 0 END),
                    SUM(CASE WHEN status = 'running' THEN 1 ELSE 0 END),
                    SUM(CASE
                            WHEN status IN ('waiting_external', 'waiting_review') THEN 1
                            WHEN status IN ('ready', 'retry_pending')
                                 AND EXISTS (
                                     SELECT 1
                                     FROM work_unit_dependencies dependencies
                                     JOIN work_units blockers
                                       ON blockers.work_unit_id = dependencies.blocking_work_unit_id
                                     WHERE dependencies.blocked_work_unit_id = work_units.work_unit_id
                                       AND blockers.status NOT IN ('completed', 'failed_terminal', 'cancelled', 'archived')
                                 ) THEN 1
                            ELSE 0
                        END),
                    SUM(CASE WHEN status = 'retry_pending' THEN 1 ELSE 0 END),
                    SUM(CASE WHEN status IN ('completed', 'failed_terminal', 'cancelled') THEN 1 ELSE 0 END),
                    SUM(CASE WHEN status = 'archived' THEN 1 ELSE 0 END),
                    SUM(CASE WHEN status IN ('leased', 'running')
                                 AND lease_expires_at_ms IS NOT NULL
                                 AND lease_expires_at_ms < ?1
                             THEN 1 ELSE 0 END)
                 FROM work_units",
                params![now_ms],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, i64>(3)?,
                        row.get::<_, i64>(4)?,
                        row.get::<_, i64>(5)?,
                        row.get::<_, i64>(6)?,
                        row.get::<_, i64>(7)?,
                        row.get::<_, i64>(8)?,
                    ))
                },
            )
            .map_err(|error| format!("load work runtime health failed: {error}"))?;
        let total_count = usize_from_i64(row.0, "total_count")?;
        let ready_count = usize_from_i64(row.1, "ready_count")?;
        let leased_count = usize_from_i64(row.2, "leased_count")?;
        let running_count = usize_from_i64(row.3, "running_count")?;
        let blocked_count = usize_from_i64(row.4, "blocked_count")?;
        let retry_pending_count = usize_from_i64(row.5, "retry_pending_count")?;
        let terminal_count = usize_from_i64(row.6, "terminal_count")?;
        let archived_count = usize_from_i64(row.7, "archived_count")?;
        let expired_lease_count = usize_from_i64(row.8, "expired_lease_count")?;

        Ok(WorkRuntimeHealthSnapshot {
            total_count,
            ready_count,
            leased_count,
            running_count,
            blocked_count,
            retry_pending_count,
            terminal_count,
            archived_count,
            expired_lease_count,
        })
    }

    fn ensure_schema(&self) -> Result<(), String> {
        let connection = self.open_connection()?;
        connection
            .execute_batch(
                "
                CREATE TABLE IF NOT EXISTS work_units(
                    work_unit_id TEXT PRIMARY KEY,
                    kind TEXT NOT NULL,
                    title TEXT NOT NULL,
                    description TEXT NOT NULL,
                    source_ref_json TEXT NOT NULL,
                    status TEXT NOT NULL,
                    priority TEXT NOT NULL,
                    priority_rank INTEGER NOT NULL,
                    retry_policy_json TEXT NOT NULL,
                    attempt_count INTEGER NOT NULL,
                    next_run_at_ms INTEGER NOT NULL,
                    last_error TEXT NULL,
                    blocking_reason TEXT NULL,
                    parent_work_unit_id TEXT NULL,
                    plan_position INTEGER NULL,
                    superseded_by_work_unit_id TEXT NULL,
                    assigned_to TEXT NULL,
                    review_json TEXT NULL,
                    result_payload_json TEXT NULL,
                    lease_owner TEXT NULL,
                    lease_version INTEGER NOT NULL DEFAULT 0,
                    lease_acquired_at_ms INTEGER NULL,
                    lease_heartbeat_at_ms INTEGER NULL,
                    lease_expires_at_ms INTEGER NULL,
                    created_at_ms INTEGER NOT NULL,
                    updated_at_ms INTEGER NOT NULL,
                    archived_at_ms INTEGER NULL
                );
                CREATE INDEX IF NOT EXISTS idx_work_units_status_next_run
                  ON work_units(status, next_run_at_ms, priority_rank, updated_at_ms, work_unit_id);
                CREATE INDEX IF NOT EXISTS idx_work_units_lease_expiry
                  ON work_units(lease_expires_at_ms, status, updated_at_ms, work_unit_id);
                CREATE INDEX IF NOT EXISTS idx_work_units_archived_status
                  ON work_units(archived_at_ms, status, updated_at_ms, work_unit_id);
                CREATE TABLE IF NOT EXISTS work_unit_dependencies(
                    blocking_work_unit_id TEXT NOT NULL,
                    blocked_work_unit_id TEXT NOT NULL,
                    created_at_ms INTEGER NOT NULL,
                    created_by TEXT NULL,
                    PRIMARY KEY(blocking_work_unit_id, blocked_work_unit_id)
                );
                CREATE INDEX IF NOT EXISTS idx_work_unit_dependencies_blocked
                  ON work_unit_dependencies(blocked_work_unit_id, blocking_work_unit_id);
                CREATE TABLE IF NOT EXISTS work_unit_events(
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    work_unit_id TEXT NOT NULL,
                    event_kind TEXT NOT NULL,
                    actor TEXT NULL,
                    payload_json TEXT NOT NULL,
                    recorded_at_ms INTEGER NOT NULL
                );
                CREATE INDEX IF NOT EXISTS idx_work_unit_events_work_unit_id
                  ON work_unit_events(work_unit_id, id);
                ",
            )
            .map_err(|error| format!("ensure work unit schema failed: {error}"))?;
        ensure_work_units_column_exists(&connection, "plan_position", "INTEGER NULL")?;
        ensure_work_units_column_exists(&connection, "superseded_by_work_unit_id", "TEXT NULL")?;
        Ok(())
    }

    fn open_connection(&self) -> Result<Connection, String> {
        Connection::open(&self.db_path)
            .map_err(|error| format!("open work unit repository sqlite db failed: {error}"))
    }
}

fn ensure_work_units_column_exists(
    connection: &Connection,
    column_name: &str,
    column_definition: &str,
) -> Result<(), String> {
    let pragma_sql = "PRAGMA table_info(work_units)";
    let mut statement = connection
        .prepare(pragma_sql)
        .map_err(|error| format!("prepare work unit schema inspection failed: {error}"))?;
    let rows = statement
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(|error| format!("query work unit schema inspection failed: {error}"))?;
    let mut column_exists = false;

    for row in rows {
        let existing_column_name =
            row.map_err(|error| format!("decode work unit schema column failed: {error}"))?;
        if existing_column_name == column_name {
            column_exists = true;
            break;
        }
    }
    if column_exists {
        return Ok(());
    }

    let alter_sql = format!("ALTER TABLE work_units ADD COLUMN {column_name} {column_definition}");
    connection
        .execute(alter_sql.as_str(), [])
        .map_err(|error| format!("add work unit column `{column_name}` failed: {error}"))?;
    Ok(())
}

fn insert_event_in_tx(
    transaction: &Transaction<'_>,
    work_unit_id: &str,
    event_kind: &str,
    actor: Option<&str>,
    payload_json: &Value,
    recorded_at_ms: i64,
) -> Result<WorkUnitEventRecord, String> {
    let encoded_payload = encode_json(payload_json, "work unit event payload")?;
    transaction
        .execute(
            "INSERT INTO work_unit_events(
                work_unit_id,
                event_kind,
                actor,
                payload_json,
                recorded_at_ms
             ) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                work_unit_id,
                event_kind,
                actor,
                encoded_payload,
                recorded_at_ms
            ],
        )
        .map_err(|error| format!("insert work unit event failed: {error}"))?;

    Ok(WorkUnitEventRecord {
        sequence_id: transaction.last_insert_rowid(),
        work_unit_id: work_unit_id.to_owned(),
        event_kind: event_kind.to_owned(),
        actor: actor.map(str::to_owned),
        payload_json: payload_json.clone(),
        recorded_at_ms,
    })
}

fn load_raw_work_unit_with_conn(
    connection: &Connection,
    work_unit_id: &str,
) -> Result<Option<RawWorkUnitRecord>, String> {
    let raw_record = connection
        .query_row(
            "SELECT
                work_unit_id,
                kind,
                title,
                description,
                source_ref_json,
                status,
                priority,
                retry_policy_json,
                attempt_count,
                next_run_at_ms,
                last_error,
                blocking_reason,
                parent_work_unit_id,
                plan_position,
                superseded_by_work_unit_id,
                assigned_to,
                review_json,
                result_payload_json,
                lease_owner,
                lease_version,
                lease_acquired_at_ms,
                lease_heartbeat_at_ms,
                lease_expires_at_ms,
                created_at_ms,
                updated_at_ms,
                archived_at_ms
             FROM work_units
             WHERE work_unit_id = ?1",
            params![work_unit_id],
            |row| {
                Ok(RawWorkUnitRecord {
                    work_unit_id: row.get(0)?,
                    kind: row.get(1)?,
                    title: row.get(2)?,
                    description: row.get(3)?,
                    source_ref_json: row.get(4)?,
                    status: row.get(5)?,
                    priority: row.get(6)?,
                    retry_policy_json: row.get(7)?,
                    attempt_count: row.get(8)?,
                    next_run_at_ms: row.get(9)?,
                    last_error: row.get(10)?,
                    blocking_reason: row.get(11)?,
                    parent_work_unit_id: row.get(12)?,
                    plan_position: row.get(13)?,
                    superseded_by_work_unit_id: row.get(14)?,
                    assigned_to: row.get(15)?,
                    child_work_unit_ids: Vec::new(),
                    supersedes_work_unit_ids: Vec::new(),
                    blocks_work_unit_ids: Vec::new(),
                    blocked_by_work_unit_ids: Vec::new(),
                    review_json: row.get(16)?,
                    result_payload_json: row.get(17)?,
                    lease_owner: row.get(18)?,
                    lease_version: row.get(19)?,
                    lease_acquired_at_ms: row.get(20)?,
                    lease_heartbeat_at_ms: row.get(21)?,
                    lease_expires_at_ms: row.get(22)?,
                    created_at_ms: row.get(23)?,
                    updated_at_ms: row.get(24)?,
                    archived_at_ms: row.get(25)?,
                })
            },
        )
        .optional()
        .map_err(|error| format!("load work unit row failed: {error}"))?;
    let Some(raw_record) = raw_record else {
        return Ok(None);
    };
    let raw_record = hydrate_raw_work_unit_relationships(connection, raw_record)?;
    Ok(Some(raw_record))
}

fn load_raw_work_units_with_query(
    connection: &Connection,
    status: Option<WorkUnitStatus>,
    include_archived: bool,
    limit: usize,
) -> Result<Vec<RawWorkUnitRecord>, String> {
    let limit =
        i64::try_from(limit).map_err(|error| format!("list limit overflowed i64: {error}"))?;
    let sql = match (status, include_archived) {
        (Some(_), false) => {
            "SELECT
                work_unit_id,
                kind,
                title,
                description,
                source_ref_json,
                status,
                priority,
                retry_policy_json,
                attempt_count,
                next_run_at_ms,
                last_error,
                blocking_reason,
                parent_work_unit_id,
                plan_position,
                superseded_by_work_unit_id,
                assigned_to,
                review_json,
                result_payload_json,
                lease_owner,
                lease_version,
                lease_acquired_at_ms,
                lease_heartbeat_at_ms,
                lease_expires_at_ms,
                created_at_ms,
                updated_at_ms,
                archived_at_ms
             FROM work_units
             WHERE status = ?1
               AND archived_at_ms IS NULL
             ORDER BY priority_rank DESC, next_run_at_ms ASC, updated_at_ms DESC, work_unit_id ASC
             LIMIT ?2"
        }
        (Some(_), true) => {
            "SELECT
                work_unit_id,
                kind,
                title,
                description,
                source_ref_json,
                status,
                priority,
                retry_policy_json,
                attempt_count,
                next_run_at_ms,
                last_error,
                blocking_reason,
                parent_work_unit_id,
                plan_position,
                superseded_by_work_unit_id,
                assigned_to,
                review_json,
                result_payload_json,
                lease_owner,
                lease_version,
                lease_acquired_at_ms,
                lease_heartbeat_at_ms,
                lease_expires_at_ms,
                created_at_ms,
                updated_at_ms,
                archived_at_ms
             FROM work_units
             WHERE status = ?1
             ORDER BY priority_rank DESC, next_run_at_ms ASC, updated_at_ms DESC, work_unit_id ASC
             LIMIT ?2"
        }
        (None, false) => {
            "SELECT
                work_unit_id,
                kind,
                title,
                description,
                source_ref_json,
                status,
                priority,
                retry_policy_json,
                attempt_count,
                next_run_at_ms,
                last_error,
                blocking_reason,
                parent_work_unit_id,
                plan_position,
                superseded_by_work_unit_id,
                assigned_to,
                review_json,
                result_payload_json,
                lease_owner,
                lease_version,
                lease_acquired_at_ms,
                lease_heartbeat_at_ms,
                lease_expires_at_ms,
                created_at_ms,
                updated_at_ms,
                archived_at_ms
             FROM work_units
             WHERE archived_at_ms IS NULL
             ORDER BY priority_rank DESC, next_run_at_ms ASC, updated_at_ms DESC, work_unit_id ASC
             LIMIT ?1"
        }
        (None, true) => {
            "SELECT
                work_unit_id,
                kind,
                title,
                description,
                source_ref_json,
                status,
                priority,
                retry_policy_json,
                attempt_count,
                next_run_at_ms,
                last_error,
                blocking_reason,
                parent_work_unit_id,
                plan_position,
                superseded_by_work_unit_id,
                assigned_to,
                review_json,
                result_payload_json,
                lease_owner,
                lease_version,
                lease_acquired_at_ms,
                lease_heartbeat_at_ms,
                lease_expires_at_ms,
                created_at_ms,
                updated_at_ms,
                archived_at_ms
             FROM work_units
             ORDER BY priority_rank DESC, next_run_at_ms ASC, updated_at_ms DESC, work_unit_id ASC
             LIMIT ?1"
        }
    };
    let mut statement = connection
        .prepare(sql)
        .map_err(|error| format!("prepare work unit list query failed: {error}"))?;
    let row_mapper = |row: &rusqlite::Row<'_>| {
        Ok(RawWorkUnitRecord {
            work_unit_id: row.get(0)?,
            kind: row.get(1)?,
            title: row.get(2)?,
            description: row.get(3)?,
            source_ref_json: row.get(4)?,
            status: row.get(5)?,
            priority: row.get(6)?,
            retry_policy_json: row.get(7)?,
            attempt_count: row.get(8)?,
            next_run_at_ms: row.get(9)?,
            last_error: row.get(10)?,
            blocking_reason: row.get(11)?,
            parent_work_unit_id: row.get(12)?,
            plan_position: row.get(13)?,
            superseded_by_work_unit_id: row.get(14)?,
            assigned_to: row.get(15)?,
            child_work_unit_ids: Vec::new(),
            supersedes_work_unit_ids: Vec::new(),
            blocks_work_unit_ids: Vec::new(),
            blocked_by_work_unit_ids: Vec::new(),
            review_json: row.get(16)?,
            result_payload_json: row.get(17)?,
            lease_owner: row.get(18)?,
            lease_version: row.get(19)?,
            lease_acquired_at_ms: row.get(20)?,
            lease_heartbeat_at_ms: row.get(21)?,
            lease_expires_at_ms: row.get(22)?,
            created_at_ms: row.get(23)?,
            updated_at_ms: row.get(24)?,
            archived_at_ms: row.get(25)?,
        })
    };
    let rows = match status {
        Some(status) => statement
            .query_map(params![status.as_str(), limit], row_mapper)
            .map_err(|error| format!("query work unit list failed: {error}"))?,
        None => statement
            .query_map(params![limit], row_mapper)
            .map_err(|error| format!("query work unit list failed: {error}"))?,
    };
    let mut raw_records = Vec::new();

    for row in rows {
        let raw_record =
            row.map_err(|error| format!("decode work unit list row failed: {error}"))?;
        let raw_record = hydrate_raw_work_unit_relationships(connection, raw_record)?;
        raw_records.push(raw_record);
    }

    Ok(raw_records)
}

fn hydrate_raw_work_unit_relationships(
    connection: &Connection,
    mut raw_record: RawWorkUnitRecord,
) -> Result<RawWorkUnitRecord, String> {
    let child_work_unit_ids =
        load_child_work_unit_ids(connection, raw_record.work_unit_id.as_str())?;
    let supersedes_work_unit_ids =
        load_supersedes_work_unit_ids(connection, raw_record.work_unit_id.as_str())?;
    let blocks_work_unit_ids =
        load_blocked_work_unit_ids(connection, raw_record.work_unit_id.as_str())?;
    let blocked_by_work_unit_ids =
        load_blocking_work_unit_ids(connection, raw_record.work_unit_id.as_str())?;
    raw_record.child_work_unit_ids = child_work_unit_ids;
    raw_record.supersedes_work_unit_ids = supersedes_work_unit_ids;
    raw_record.blocks_work_unit_ids = blocks_work_unit_ids;
    raw_record.blocked_by_work_unit_ids = blocked_by_work_unit_ids;
    Ok(raw_record)
}

fn load_child_work_unit_ids(
    connection: &Connection,
    work_unit_id: &str,
) -> Result<Vec<String>, String> {
    let mut statement = connection
        .prepare(
            "SELECT work_unit_id
             FROM work_units
             WHERE parent_work_unit_id = ?1
             ORDER BY
                CASE WHEN plan_position IS NULL THEN 1 ELSE 0 END ASC,
                plan_position ASC,
                work_unit_id ASC",
        )
        .map_err(|error| format!("prepare child work unit query failed: {error}"))?;
    let rows = statement
        .query_map(params![work_unit_id], |row| row.get::<_, String>(0))
        .map_err(|error| format!("query child work units failed: {error}"))?;
    let mut work_unit_ids = Vec::new();

    for row in rows {
        let child_work_unit_id =
            row.map_err(|error| format!("decode child work unit row failed: {error}"))?;
        work_unit_ids.push(child_work_unit_id);
    }

    Ok(work_unit_ids)
}

fn load_next_child_plan_position(
    connection: &Connection,
    parent_work_unit_id: &str,
) -> Result<i64, String> {
    let mut statement = connection
        .prepare(
            "SELECT COALESCE(MAX(plan_position), 0)
             FROM work_units
             WHERE parent_work_unit_id = ?1",
        )
        .map_err(|error| format!("prepare next child plan position query failed: {error}"))?;
    let max_plan_position = statement
        .query_row(params![parent_work_unit_id], |row| row.get::<_, i64>(0))
        .map_err(|error| format!("query next child plan position failed: {error}"))?;
    Ok(max_plan_position.saturating_add(1))
}

fn set_child_plan_position_in_tx(
    transaction: &Transaction<'_>,
    child_work_unit_id: &str,
    plan_position: i64,
    now_ms: i64,
) -> Result<(), String> {
    transaction
        .execute(
            "UPDATE work_units
             SET plan_position = ?1,
                 updated_at_ms = ?2
             WHERE work_unit_id = ?3",
            params![plan_position, now_ms, child_work_unit_id],
        )
        .map_err(|error| format!("set child plan position failed: {error}"))?;
    Ok(())
}

fn load_supersedes_work_unit_ids(
    connection: &Connection,
    work_unit_id: &str,
) -> Result<Vec<String>, String> {
    let mut statement = connection
        .prepare(
            "SELECT work_unit_id
             FROM work_units
             WHERE superseded_by_work_unit_id = ?1
             ORDER BY work_unit_id ASC",
        )
        .map_err(|error| format!("prepare superseded work unit query failed: {error}"))?;
    let rows = statement
        .query_map(params![work_unit_id], |row| row.get::<_, String>(0))
        .map_err(|error| format!("query superseded work units failed: {error}"))?;
    let mut work_unit_ids = Vec::new();

    for row in rows {
        let superseded_work_unit_id =
            row.map_err(|error| format!("decode superseded work unit row failed: {error}"))?;
        work_unit_ids.push(superseded_work_unit_id);
    }

    Ok(work_unit_ids)
}

fn load_blocked_work_unit_ids(
    connection: &Connection,
    work_unit_id: &str,
) -> Result<Vec<String>, String> {
    let mut statement = connection
        .prepare(
            "SELECT blocked_work_unit_id
             FROM work_unit_dependencies
             WHERE blocking_work_unit_id = ?1
             ORDER BY blocked_work_unit_id ASC",
        )
        .map_err(|error| format!("prepare blocked work unit query failed: {error}"))?;
    let rows = statement
        .query_map(params![work_unit_id], |row| row.get::<_, String>(0))
        .map_err(|error| format!("query blocked work units failed: {error}"))?;
    let mut work_unit_ids = Vec::new();

    for row in rows {
        let blocked_work_unit_id =
            row.map_err(|error| format!("decode blocked work unit row failed: {error}"))?;
        work_unit_ids.push(blocked_work_unit_id);
    }

    Ok(work_unit_ids)
}

fn load_blocking_work_unit_ids(
    connection: &Connection,
    work_unit_id: &str,
) -> Result<Vec<String>, String> {
    let mut statement = connection
        .prepare(
            "SELECT blocking_work_unit_id
             FROM work_unit_dependencies
             WHERE blocked_work_unit_id = ?1
             ORDER BY blocking_work_unit_id ASC",
        )
        .map_err(|error| format!("prepare blocking work unit query failed: {error}"))?;
    let rows = statement
        .query_map(params![work_unit_id], |row| row.get::<_, String>(0))
        .map_err(|error| format!("query blocking work units failed: {error}"))?;
    let mut work_unit_ids = Vec::new();

    for row in rows {
        let blocking_work_unit_id =
            row.map_err(|error| format!("decode blocking work unit row failed: {error}"))?;
        work_unit_ids.push(blocking_work_unit_id);
    }

    Ok(work_unit_ids)
}

fn select_next_ready_raw_work_unit(
    connection: &Connection,
    now_ms: i64,
) -> Result<Option<RawWorkUnitRecord>, String> {
    let mut statement = connection
        .prepare(
            "SELECT
                work_unit_id,
                kind,
                title,
                description,
                source_ref_json,
                status,
                priority,
                retry_policy_json,
                attempt_count,
                next_run_at_ms,
                last_error,
                blocking_reason,
                parent_work_unit_id,
                plan_position,
                superseded_by_work_unit_id,
                assigned_to,
                review_json,
                result_payload_json,
                lease_owner,
                lease_version,
                lease_acquired_at_ms,
                lease_heartbeat_at_ms,
                lease_expires_at_ms,
                created_at_ms,
                updated_at_ms,
                archived_at_ms
             FROM work_units
             WHERE status IN ('ready', 'retry_pending')
               AND archived_at_ms IS NULL
               AND next_run_at_ms <= ?1
               AND (lease_expires_at_ms IS NULL OR lease_expires_at_ms <= ?1)
               AND NOT EXISTS (
                    SELECT 1
                    FROM work_unit_dependencies dependencies
                    JOIN work_units blockers
                      ON blockers.work_unit_id = dependencies.blocking_work_unit_id
                    WHERE dependencies.blocked_work_unit_id = work_units.work_unit_id
                      AND blockers.status NOT IN ('completed', 'failed_terminal', 'cancelled', 'archived')
               )
             ORDER BY priority_rank DESC, next_run_at_ms ASC, updated_at_ms ASC, work_unit_id ASC
             LIMIT 1",
        )
        .map_err(|error| format!("prepare next ready work unit query failed: {error}"))?;
    let raw_record = statement
        .query_row(params![now_ms], |row| {
            Ok(RawWorkUnitRecord {
                work_unit_id: row.get(0)?,
                kind: row.get(1)?,
                title: row.get(2)?,
                description: row.get(3)?,
                source_ref_json: row.get(4)?,
                status: row.get(5)?,
                priority: row.get(6)?,
                retry_policy_json: row.get(7)?,
                attempt_count: row.get(8)?,
                next_run_at_ms: row.get(9)?,
                last_error: row.get(10)?,
                blocking_reason: row.get(11)?,
                parent_work_unit_id: row.get(12)?,
                plan_position: row.get(13)?,
                superseded_by_work_unit_id: row.get(14)?,
                assigned_to: row.get(15)?,
                child_work_unit_ids: Vec::new(),
                supersedes_work_unit_ids: Vec::new(),
                blocks_work_unit_ids: Vec::new(),
                blocked_by_work_unit_ids: Vec::new(),
                review_json: row.get(16)?,
                result_payload_json: row.get(17)?,
                lease_owner: row.get(18)?,
                lease_version: row.get(19)?,
                lease_acquired_at_ms: row.get(20)?,
                lease_heartbeat_at_ms: row.get(21)?,
                lease_expires_at_ms: row.get(22)?,
                created_at_ms: row.get(23)?,
                updated_at_ms: row.get(24)?,
                archived_at_ms: row.get(25)?,
            })
        })
        .optional()
        .map_err(|error| format!("query next ready work unit failed: {error}"))?;
    let hydrated = raw_record
        .map(|raw_record| hydrate_raw_work_unit_relationships(connection, raw_record))
        .transpose()?;
    Ok(hydrated)
}

fn load_expired_raw_work_units_with_conn(
    connection: &Connection,
    now_ms: i64,
) -> Result<Vec<RawWorkUnitRecord>, String> {
    let mut statement = connection
        .prepare(
            "SELECT
                work_unit_id,
                kind,
                title,
                description,
                source_ref_json,
                status,
                priority,
                retry_policy_json,
                attempt_count,
                next_run_at_ms,
                last_error,
                blocking_reason,
                parent_work_unit_id,
                plan_position,
                superseded_by_work_unit_id,
                assigned_to,
                review_json,
                result_payload_json,
                lease_owner,
                lease_version,
                lease_acquired_at_ms,
                lease_heartbeat_at_ms,
                lease_expires_at_ms,
                created_at_ms,
                updated_at_ms,
                archived_at_ms
             FROM work_units
             WHERE status IN ('leased', 'running')
               AND lease_expires_at_ms IS NOT NULL
               AND lease_expires_at_ms < ?1
             ORDER BY lease_expires_at_ms ASC, work_unit_id ASC",
        )
        .map_err(|error| format!("prepare expired work unit query failed: {error}"))?;
    let rows = statement
        .query_map(params![now_ms], |row| {
            Ok(RawWorkUnitRecord {
                work_unit_id: row.get(0)?,
                kind: row.get(1)?,
                title: row.get(2)?,
                description: row.get(3)?,
                source_ref_json: row.get(4)?,
                status: row.get(5)?,
                priority: row.get(6)?,
                retry_policy_json: row.get(7)?,
                attempt_count: row.get(8)?,
                next_run_at_ms: row.get(9)?,
                last_error: row.get(10)?,
                blocking_reason: row.get(11)?,
                parent_work_unit_id: row.get(12)?,
                plan_position: row.get(13)?,
                superseded_by_work_unit_id: row.get(14)?,
                assigned_to: row.get(15)?,
                child_work_unit_ids: Vec::new(),
                supersedes_work_unit_ids: Vec::new(),
                blocks_work_unit_ids: Vec::new(),
                blocked_by_work_unit_ids: Vec::new(),
                review_json: row.get(16)?,
                result_payload_json: row.get(17)?,
                lease_owner: row.get(18)?,
                lease_version: row.get(19)?,
                lease_acquired_at_ms: row.get(20)?,
                lease_heartbeat_at_ms: row.get(21)?,
                lease_expires_at_ms: row.get(22)?,
                created_at_ms: row.get(23)?,
                updated_at_ms: row.get(24)?,
                archived_at_ms: row.get(25)?,
            })
        })
        .map_err(|error| format!("query expired work units failed: {error}"))?;
    let mut raw_records = Vec::new();

    for row in rows {
        let raw_record =
            row.map_err(|error| format!("decode expired work unit row failed: {error}"))?;
        let raw_record = hydrate_raw_work_unit_relationships(connection, raw_record)?;
        raw_records.push(raw_record);
    }

    Ok(raw_records)
}

fn try_work_unit_snapshot_from_raw(
    raw_record: RawWorkUnitRecord,
) -> Result<WorkUnitSnapshot, String> {
    let kind = WorkUnitKind::parse(&raw_record.kind)
        .ok_or_else(|| format!("unknown work unit kind `{}`", raw_record.kind))?;
    let status = WorkUnitStatus::parse(&raw_record.status)
        .ok_or_else(|| format!("unknown work unit status `{}`", raw_record.status))?;
    let priority = WorkUnitPriority::parse(&raw_record.priority)
        .ok_or_else(|| format!("unknown work unit priority `{}`", raw_record.priority))?;
    let source_ref = decode_json::<WorkUnitSourceRef>(&raw_record.source_ref_json, "source_ref")?;
    let retry_policy =
        decode_json::<WorkUnitRetryPolicy>(&raw_record.retry_policy_json, "retry_policy")?;
    let attempt_count = u32::try_from(raw_record.attempt_count)
        .map_err(|error| format!("attempt_count overflowed u32: {error}"))?;
    let result_payload_json = raw_record
        .result_payload_json
        .as_deref()
        .map(|value| decode_json::<Value>(value, "result_payload"))
        .transpose()?;
    let review = raw_record
        .review_json
        .as_deref()
        .map(|value| decode_json::<WorkUnitReviewRecord>(value, "review"))
        .transpose()?;
    let lease = build_lease_record(&raw_record)?;
    let work_unit = WorkUnitRecord {
        work_unit_id: raw_record.work_unit_id.clone(),
        kind,
        title: raw_record.title,
        description: raw_record.description,
        source_ref,
        status,
        priority,
        assigned_to: raw_record.assigned_to.clone(),
        retry_policy,
        attempt_count,
        next_run_at_ms: raw_record.next_run_at_ms,
        last_error: raw_record.last_error.clone(),
        blocking_reason: raw_record.blocking_reason.clone(),
        parent_work_unit_id: raw_record.parent_work_unit_id.clone(),
        plan_position: raw_record.plan_position,
        superseded_by_work_unit_id: raw_record.superseded_by_work_unit_id.clone(),
        child_work_unit_ids: raw_record.child_work_unit_ids.clone(),
        supersedes_work_unit_ids: raw_record.supersedes_work_unit_ids.clone(),
        blocks_work_unit_ids: raw_record.blocks_work_unit_ids.clone(),
        blocked_by_work_unit_ids: raw_record.blocked_by_work_unit_ids.clone(),
        review,
        result_payload_json,
        created_at_ms: raw_record.created_at_ms,
        updated_at_ms: raw_record.updated_at_ms,
        archived_at_ms: raw_record.archived_at_ms,
    };
    Ok(WorkUnitSnapshot { work_unit, lease })
}

fn build_lease_record(
    raw_record: &RawWorkUnitRecord,
) -> Result<Option<WorkUnitLeaseRecord>, String> {
    let Some(owner) = raw_record.lease_owner.clone() else {
        return Ok(None);
    };
    let lease_version = u64::try_from(raw_record.lease_version)
        .map_err(|error| format!("lease_version overflowed u64: {error}"))?;
    let acquired_at_ms = raw_record
        .lease_acquired_at_ms
        .ok_or_else(|| "lease_owner set without lease_acquired_at_ms".to_owned())?;
    let heartbeat_at_ms = raw_record
        .lease_heartbeat_at_ms
        .ok_or_else(|| "lease_owner set without lease_heartbeat_at_ms".to_owned())?;
    let expires_at_ms = raw_record
        .lease_expires_at_ms
        .ok_or_else(|| "lease_owner set without lease_expires_at_ms".to_owned())?;
    let lease = WorkUnitLeaseRecord {
        work_unit_id: raw_record.work_unit_id.clone(),
        owner,
        lease_version,
        acquired_at_ms,
        heartbeat_at_ms,
        expires_at_ms,
    };
    Ok(Some(lease))
}

struct CompletionResolution {
    status: WorkUnitStatus,
    next_run_at_ms: i64,
    last_error: Option<String>,
    event_kind: &'static str,
}

fn resolve_completion(
    disposition: WorkUnitCompletionDisposition,
    retry_policy: &WorkUnitRetryPolicy,
    attempt_count: u32,
    now_ms: i64,
    next_run_at_ms: Option<i64>,
    error: Option<&str>,
) -> Result<CompletionResolution, String> {
    match disposition {
        WorkUnitCompletionDisposition::Completed => Ok(CompletionResolution {
            status: WorkUnitStatus::Completed,
            next_run_at_ms: now_ms,
            last_error: None,
            event_kind: WORK_UNIT_COMPLETED_EVENT_KIND,
        }),
        WorkUnitCompletionDisposition::Cancelled => Ok(CompletionResolution {
            status: WorkUnitStatus::Cancelled,
            next_run_at_ms: now_ms,
            last_error: error.map(str::to_owned),
            event_kind: WORK_UNIT_CANCELLED_EVENT_KIND,
        }),
        WorkUnitCompletionDisposition::FailedTerminal => Ok(CompletionResolution {
            status: WorkUnitStatus::FailedTerminal,
            next_run_at_ms: now_ms,
            last_error: error.map(str::to_owned),
            event_kind: WORK_UNIT_FAILED_EVENT_KIND,
        }),
        WorkUnitCompletionDisposition::RetryPending => {
            if attempt_count >= retry_policy.max_attempts {
                let retry_exhausted_error = error.map(str::to_owned).unwrap_or_else(|| {
                    format!(
                        "retry budget exhausted after {} attempt(s)",
                        retry_policy.max_attempts
                    )
                });
                return Ok(CompletionResolution {
                    status: WorkUnitStatus::FailedTerminal,
                    next_run_at_ms: now_ms,
                    last_error: Some(retry_exhausted_error),
                    event_kind: WORK_UNIT_FAILED_EVENT_KIND,
                });
            }

            let computed_next_run_at_ms = match next_run_at_ms {
                Some(next_run_at_ms) => next_run_at_ms,
                None => {
                    let delay_ms = compute_retry_delay_ms(retry_policy, attempt_count)?;
                    add_delay_ms(now_ms, delay_ms)?
                }
            };
            Ok(CompletionResolution {
                status: WorkUnitStatus::RetryPending,
                next_run_at_ms: computed_next_run_at_ms,
                last_error: error.map(str::to_owned),
                event_kind: WORK_UNIT_RETRY_EVENT_KIND,
            })
        }
    }
}

struct RecoveryResolution {
    status: WorkUnitStatus,
    next_run_at_ms: i64,
    last_error: Option<String>,
}

fn resolve_recovery(
    retry_policy: &WorkUnitRetryPolicy,
    attempt_count: u32,
    now_ms: i64,
    last_error: &str,
) -> Result<RecoveryResolution, String> {
    if attempt_count >= retry_policy.max_attempts {
        return Ok(RecoveryResolution {
            status: WorkUnitStatus::FailedTerminal,
            next_run_at_ms: now_ms,
            last_error: Some(last_error.to_owned()),
        });
    }

    let delay_ms = compute_retry_delay_ms(retry_policy, attempt_count)?;
    let next_run_at_ms = add_delay_ms(now_ms, delay_ms)?;
    Ok(RecoveryResolution {
        status: WorkUnitStatus::RetryPending,
        next_run_at_ms,
        last_error: Some(last_error.to_owned()),
    })
}

fn build_expired_lease_error(raw_record: &RawWorkUnitRecord) -> Result<String, String> {
    let owner = raw_record
        .lease_owner
        .as_deref()
        .ok_or_else(|| "expired lease recovery requires lease owner".to_owned())?;
    let expires_at_ms = raw_record
        .lease_expires_at_ms
        .ok_or_else(|| "expired lease recovery requires lease_expires_at_ms".to_owned())?;
    Ok(format!(
        "lease expired for owner `{owner}` at {expires_at_ms}"
    ))
}

fn ensure_work_unit_exists(connection: &Connection, work_unit_id: &str) -> Result<(), String> {
    let exists = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM work_units WHERE work_unit_id = ?1)",
            params![work_unit_id],
            |row| row.get::<_, i64>(0),
        )
        .map_err(|error| format!("check work unit existence failed: {error}"))?;
    if exists == 0 {
        return Err(format!("work unit `{work_unit_id}` not found"));
    }
    Ok(())
}

fn touch_work_unit(connection: &Connection, work_unit_id: &str, now_ms: i64) -> Result<(), String> {
    connection
        .execute(
            "UPDATE work_units
             SET updated_at_ms = ?1
             WHERE work_unit_id = ?2",
            params![now_ms, work_unit_id],
        )
        .map_err(|error| format!("touch work unit failed: {error}"))?;
    Ok(())
}

fn validate_dependency_endpoints(
    blocking_work_unit_id: &str,
    blocked_work_unit_id: &str,
) -> Result<(), String> {
    if blocking_work_unit_id == blocked_work_unit_id {
        return Err("work unit dependency cannot target the same work unit".to_owned());
    }
    Ok(())
}

fn validate_supersede_endpoints(
    obsolete_work_unit_id: &str,
    replacement_work_unit_id: &str,
) -> Result<(), String> {
    if obsolete_work_unit_id == replacement_work_unit_id {
        return Err("work unit supersede cannot target the same work unit".to_owned());
    }
    Ok(())
}

fn validate_supersede_transition(
    obsolete_snapshot: &WorkUnitSnapshot,
    replacement_snapshot: &WorkUnitSnapshot,
    now_ms: i64,
) -> Result<(), String> {
    let obsolete_status = obsolete_snapshot.work_unit.status;
    if obsolete_snapshot.work_unit.archived_at_ms.is_some() {
        return Err(format!(
            "cannot supersede archived work unit `{}`",
            obsolete_snapshot.work_unit.work_unit_id
        ));
    }
    if obsolete_snapshot
        .work_unit
        .superseded_by_work_unit_id
        .is_some()
    {
        return Err(format!(
            "work unit `{}` is already superseded",
            obsolete_snapshot.work_unit.work_unit_id
        ));
    }
    if obsolete_status.is_terminal() {
        return Err(format!(
            "cannot supersede terminal work unit `{}` with status `{}`",
            obsolete_snapshot.work_unit.work_unit_id,
            obsolete_status.as_str()
        ));
    }
    let obsolete_is_runtime_owned = matches!(
        obsolete_status,
        WorkUnitStatus::Leased | WorkUnitStatus::Running
    );
    if obsolete_is_runtime_owned {
        return Err(format!(
            "cannot supersede runtime-owned work unit `{}` while status is `{}`",
            obsolete_snapshot.work_unit.work_unit_id,
            obsolete_status.as_str()
        ));
    }

    if replacement_snapshot.work_unit.archived_at_ms.is_some() {
        return Err(format!(
            "cannot supersede with archived replacement `{}`",
            replacement_snapshot.work_unit.work_unit_id
        ));
    }
    if replacement_snapshot
        .work_unit
        .superseded_by_work_unit_id
        .is_some()
    {
        return Err(format!(
            "replacement work unit `{}` is already superseded",
            replacement_snapshot.work_unit.work_unit_id
        ));
    }
    if replacement_snapshot
        .work_unit
        .blocked_by_work_unit_ids
        .iter()
        .any(|id| id == obsolete_snapshot.work_unit.work_unit_id.as_str())
    {
        return Err(format!(
            "replacement work unit `{}` is blocked by obsolete work unit `{}`",
            replacement_snapshot.work_unit.work_unit_id, obsolete_snapshot.work_unit.work_unit_id
        ));
    }
    if obsolete_snapshot
        .work_unit
        .blocked_by_work_unit_ids
        .iter()
        .any(|id| id == replacement_snapshot.work_unit.work_unit_id.as_str())
    {
        return Err(format!(
            "obsolete work unit `{}` is already blocked by replacement `{}`",
            obsolete_snapshot.work_unit.work_unit_id, replacement_snapshot.work_unit.work_unit_id
        ));
    }

    let stale_lease = obsolete_snapshot
        .lease
        .as_ref()
        .is_some_and(|lease| lease.expires_at_ms >= now_ms);
    if stale_lease {
        return Err(format!(
            "cannot supersede work unit `{}` while it still holds an active lease",
            obsolete_snapshot.work_unit.work_unit_id
        ));
    }

    Ok(())
}

fn would_create_dependency_cycle(
    connection: &Connection,
    blocking_work_unit_id: &str,
    blocked_work_unit_id: &str,
) -> Result<bool, String> {
    let mut statement = connection
        .prepare(
            "WITH RECURSIVE dependency_chain(work_unit_id) AS (
                 SELECT blocked_work_unit_id
                 FROM work_unit_dependencies
                 WHERE blocking_work_unit_id = ?1
                 UNION
                 SELECT dependencies.blocked_work_unit_id
                 FROM work_unit_dependencies dependencies
                 JOIN dependency_chain chain
                   ON chain.work_unit_id = dependencies.blocking_work_unit_id
             )
             SELECT EXISTS(
                 SELECT 1
                 FROM dependency_chain
                 WHERE work_unit_id = ?2
             )",
        )
        .map_err(|error| format!("prepare dependency cycle query failed: {error}"))?;
    let exists = statement
        .query_row(
            params![blocked_work_unit_id, blocking_work_unit_id],
            |row| row.get::<_, i64>(0),
        )
        .map_err(|error| format!("query dependency cycle failed: {error}"))?;
    Ok(exists != 0)
}

fn validate_initial_status(status: WorkUnitStatus) -> Result<(), String> {
    let is_allowed = matches!(
        status,
        WorkUnitStatus::Captured
            | WorkUnitStatus::Triaged
            | WorkUnitStatus::Ready
            | WorkUnitStatus::WaitingExternal
            | WorkUnitStatus::WaitingReview
            | WorkUnitStatus::RetryPending
    );
    if !is_allowed {
        return Err(format!(
            "work unit repository does not allow initial status `{}`",
            status.as_str()
        ));
    }
    Ok(())
}

fn validate_manual_update_status(
    current_status: WorkUnitStatus,
    next_status: WorkUnitStatus,
) -> Result<(), String> {
    let next_is_allowed = matches!(
        next_status,
        WorkUnitStatus::Captured
            | WorkUnitStatus::Triaged
            | WorkUnitStatus::Ready
            | WorkUnitStatus::WaitingExternal
            | WorkUnitStatus::WaitingReview
            | WorkUnitStatus::RetryPending
            | WorkUnitStatus::Cancelled
    );
    if !next_is_allowed {
        return Err(format!(
            "manual work unit update does not allow target status `{}`",
            next_status.as_str()
        ));
    }

    let current_is_mutable = matches!(
        current_status,
        WorkUnitStatus::Captured
            | WorkUnitStatus::Triaged
            | WorkUnitStatus::Ready
            | WorkUnitStatus::WaitingExternal
            | WorkUnitStatus::WaitingReview
            | WorkUnitStatus::RetryPending
            | WorkUnitStatus::Cancelled
    );
    if !current_is_mutable {
        return Err(format!(
            "manual work unit update cannot change status from `{}`",
            current_status.as_str()
        ));
    }

    Ok(())
}

fn validate_review_request_status(status: WorkUnitStatus) -> Result<(), String> {
    let is_allowed = matches!(
        status,
        WorkUnitStatus::Captured
            | WorkUnitStatus::Triaged
            | WorkUnitStatus::Ready
            | WorkUnitStatus::RetryPending
            | WorkUnitStatus::WaitingExternal
    );
    if !is_allowed {
        return Err(format!(
            "work unit review cannot be requested from `{}`",
            status.as_str()
        ));
    }
    Ok(())
}

fn review_status_from_decision(decision: WorkUnitReviewDecision) -> WorkUnitReviewStatus {
    match decision {
        WorkUnitReviewDecision::Approve => WorkUnitReviewStatus::Approved,
        WorkUnitReviewDecision::RequestChanges => WorkUnitReviewStatus::ChangesRequested,
        WorkUnitReviewDecision::Reject => WorkUnitReviewStatus::Rejected,
    }
}

fn work_unit_status_from_review_decision(decision: WorkUnitReviewDecision) -> WorkUnitStatus {
    match decision {
        WorkUnitReviewDecision::Approve => WorkUnitStatus::Ready,
        WorkUnitReviewDecision::RequestChanges => WorkUnitStatus::Triaged,
        WorkUnitReviewDecision::Reject => WorkUnitStatus::Cancelled,
    }
}

fn blocking_reason_from_review_decision(
    decision: WorkUnitReviewDecision,
    summary: Option<&str>,
) -> Option<String> {
    match decision {
        WorkUnitReviewDecision::Approve => None,
        WorkUnitReviewDecision::RequestChanges => summary
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .or_else(|| Some("changes requested".to_owned())),
        WorkUnitReviewDecision::Reject => summary
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .or_else(|| Some("review rejected".to_owned())),
    }
}

fn validate_retry_policy(retry_policy: &WorkUnitRetryPolicy) -> Result<(), String> {
    if retry_policy.max_attempts == 0 {
        return Err("work unit retry policy requires max_attempts >= 1".to_owned());
    }
    if retry_policy.initial_backoff_ms == 0 {
        return Err("work unit retry policy requires initial_backoff_ms >= 1".to_owned());
    }
    if retry_policy.max_backoff_ms < retry_policy.initial_backoff_ms {
        return Err(
            "work unit retry policy requires max_backoff_ms >= initial_backoff_ms".to_owned(),
        );
    }
    Ok(())
}

fn validate_ttl_ms(ttl_ms: u64) -> Result<(), String> {
    if ttl_ms == 0 {
        return Err("work unit lease ttl_ms must be greater than zero".to_owned());
    }
    Ok(())
}

fn normalize_required_text(value: &str, field_name: &str) -> Result<String, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(format!("work unit repository requires {field_name}"));
    }
    Ok(trimmed.to_owned())
}

fn normalize_plan_position(plan_position: Option<i64>) -> Result<Option<i64>, String> {
    let Some(plan_position) = plan_position else {
        return Ok(None);
    };
    if plan_position <= 0 {
        return Err("work unit plan_position must be greater than zero".to_owned());
    }
    Ok(Some(plan_position))
}

fn normalize_optional_text(value: Option<String>) -> Option<String> {
    value.and_then(|raw_value| {
        let trimmed_value = raw_value.trim();
        if trimmed_value.is_empty() {
            return None;
        }
        Some(trimmed_value.to_owned())
    })
}

fn normalize_source_ref(source_ref: WorkUnitSourceRef) -> WorkUnitSourceRef {
    WorkUnitSourceRef {
        source_kind: source_ref.source_kind,
        project_id: normalize_optional_text(source_ref.project_id),
        channel_id: normalize_optional_text(source_ref.channel_id),
        thread_id: normalize_optional_text(source_ref.thread_id),
        message_id: normalize_optional_text(source_ref.message_id),
        external_ref: normalize_optional_text(source_ref.external_ref),
        source_url: normalize_optional_text(source_ref.source_url),
    }
}

fn normalize_limit(limit: usize) -> Result<usize, String> {
    if limit == 0 {
        return Err("work unit repository requires limit >= 1".to_owned());
    }
    Ok(limit)
}

fn encode_json<T>(value: &T, label: &str) -> Result<String, String>
where
    T: serde::Serialize,
{
    serde_json::to_string(value).map_err(|error| format!("encode {label} failed: {error}"))
}

fn decode_json<T>(raw: &str, label: &str) -> Result<T, String>
where
    T: serde::de::DeserializeOwned,
{
    serde_json::from_str(raw).map_err(|error| format!("decode {label} failed: {error}"))
}

fn current_unix_ms() -> i64 {
    let now = SystemTime::now();
    let since_epoch = now.duration_since(UNIX_EPOCH).unwrap_or_default();
    let millis = since_epoch.as_millis();
    i64::try_from(millis).unwrap_or(i64::MAX)
}

fn add_delay_ms(base_ms: i64, delay_ms: u64) -> Result<i64, String> {
    let delay_ms =
        i64::try_from(delay_ms).map_err(|error| format!("delay_ms overflowed i64: {error}"))?;
    Ok(base_ms.saturating_add(delay_ms))
}

fn compute_retry_delay_ms(
    retry_policy: &WorkUnitRetryPolicy,
    attempt_count: u32,
) -> Result<u64, String> {
    validate_retry_policy(retry_policy)?;
    let mut delay_ms = retry_policy.initial_backoff_ms;
    let mut remaining_steps = attempt_count.saturating_sub(1);

    while remaining_steps > 0 {
        let doubled_delay_ms = delay_ms.saturating_mul(2);
        let clamped_delay_ms = doubled_delay_ms.min(retry_policy.max_backoff_ms);
        delay_ms = clamped_delay_ms;
        remaining_steps = remaining_steps.saturating_sub(1);
    }

    Ok(delay_ms)
}

fn generate_work_unit_id() -> String {
    let entropy = random::<u64>();
    format!("wu-{entropy:016x}")
}

fn priority_rank(priority: WorkUnitPriority) -> i64 {
    match priority {
        WorkUnitPriority::Low => 1,
        WorkUnitPriority::Normal => 2,
        WorkUnitPriority::High => 3,
        WorkUnitPriority::Critical => 4,
        _ => 2,
    }
}

fn validate_split_child_ids(
    parent_work_unit_id: &str,
    children: &[SplitWorkUnitChildRequest],
) -> Result<(), String> {
    let mut seen_ids = std::collections::BTreeSet::new();

    for child in children {
        let Some(child_work_unit_id) = child.child.work_unit_id.as_deref() else {
            continue;
        };
        let normalized_child_work_unit_id =
            normalize_required_text(child_work_unit_id, "child.work_unit_id")?;
        if normalized_child_work_unit_id == parent_work_unit_id {
            return Err(format!(
                "split child work unit id `{normalized_child_work_unit_id}` cannot match parent `{parent_work_unit_id}`"
            ));
        }
        let inserted = seen_ids.insert(normalized_child_work_unit_id.clone());
        if !inserted {
            return Err(format!(
                "split child work unit id `{normalized_child_work_unit_id}` is duplicated"
            ));
        }
    }

    Ok(())
}

fn normalize_resequence_child_ids(raw_ids: &[String]) -> Result<Vec<String>, String> {
    let mut normalized_ids = Vec::with_capacity(raw_ids.len());

    for raw_id in raw_ids {
        let normalized_id = normalize_required_text(raw_id, "ordered_child_work_unit_ids")?;
        normalized_ids.push(normalized_id);
    }

    Ok(normalized_ids)
}

fn validate_resequence_child_ids(
    parent_work_unit_id: &str,
    current_child_work_unit_ids: &[String],
    ordered_child_work_unit_ids: &[String],
) -> Result<(), String> {
    if current_child_work_unit_ids.len() < WORK_UNIT_SPLIT_MIN_CHILDREN {
        return Err(format!(
            "work unit `{parent_work_unit_id}` requires at least {WORK_UNIT_SPLIT_MIN_CHILDREN} child work units to resequence"
        ));
    }

    let mut current_ids = current_child_work_unit_ids.to_vec();
    let mut ordered_ids = ordered_child_work_unit_ids.to_vec();
    current_ids.sort();
    ordered_ids.sort();
    if current_ids != ordered_ids {
        return Err(format!(
            "resequence child ids must exactly match current child set for parent `{parent_work_unit_id}`"
        ));
    }

    let unique_ordered_ids = ordered_child_work_unit_ids
        .iter()
        .collect::<std::collections::BTreeSet<_>>();
    let unique_count = unique_ordered_ids.len();
    let ordered_count = ordered_child_work_unit_ids.len();
    if unique_count != ordered_count {
        return Err("resequence child ids must be unique".to_owned());
    }

    Ok(())
}

fn apply_resequence_in_tx(
    transaction: &Transaction<'_>,
    ordered_child_work_unit_ids: &[String],
    now_ms: i64,
) -> Result<(), String> {
    for (index, child_work_unit_id) in ordered_child_work_unit_ids.iter().enumerate() {
        let plan_position = i64::try_from(index + 1)
            .map_err(|error| format!("plan position overflowed i64: {error}"))?;
        set_child_plan_position_in_tx(
            transaction,
            child_work_unit_id.as_str(),
            plan_position,
            now_ms,
        )?;
    }

    Ok(())
}

fn resolve_work_unit_update(
    raw_record: &RawWorkUnitRecord,
    title: Option<&str>,
    description: Option<&str>,
    status: Option<WorkUnitStatus>,
    priority: Option<WorkUnitPriority>,
    next_run_at_ms: Option<i64>,
    blocking_reason: Option<&str>,
    clear_blocking_reason: bool,
) -> Result<ResolvedWorkUnitUpdate, String> {
    let current_status = WorkUnitStatus::parse(&raw_record.status)
        .ok_or_else(|| format!("unknown work unit status `{}`", raw_record.status))?;
    let mut changed_fields = Vec::new();

    let next_title = match title {
        Some(title) => {
            let title = normalize_required_text(title, "title")?;
            if title != raw_record.title {
                changed_fields.push("title".to_owned());
            }
            title
        }
        None => raw_record.title.clone(),
    };
    let next_description = match description {
        Some(description) => {
            let description = normalize_required_text(description, "description")?;
            if description != raw_record.description {
                changed_fields.push("description".to_owned());
            }
            description
        }
        None => raw_record.description.clone(),
    };
    let next_status = match status {
        Some(status) => {
            validate_manual_update_status(current_status, status)?;
            let status_label = status.as_str().to_owned();
            if status_label != raw_record.status {
                changed_fields.push("status".to_owned());
            }
            status_label
        }
        None => raw_record.status.clone(),
    };
    let next_priority = match priority {
        Some(priority) => {
            let priority_label = priority.as_str().to_owned();
            if priority_label != raw_record.priority {
                changed_fields.push("priority".to_owned());
            }
            priority_label
        }
        None => raw_record.priority.clone(),
    };
    let parsed_priority = WorkUnitPriority::parse(next_priority.as_str())
        .ok_or_else(|| format!("unknown work unit priority `{}`", next_priority))?;
    let next_priority_rank = priority_rank(parsed_priority);
    let next_next_run_at_ms = match next_run_at_ms {
        Some(next_run_at_ms) => {
            if next_run_at_ms != raw_record.next_run_at_ms {
                changed_fields.push("next_run_at_ms".to_owned());
            }
            next_run_at_ms
        }
        None => raw_record.next_run_at_ms,
    };
    let explicit_blocking_reason = blocking_reason
        .map(str::to_owned)
        .map(Some)
        .unwrap_or_else(|| raw_record.blocking_reason.clone());
    let normalized_blocking_reason = normalize_optional_text(explicit_blocking_reason);
    let next_blocking_reason = if clear_blocking_reason {
        None
    } else {
        normalized_blocking_reason
    };
    if next_blocking_reason != raw_record.blocking_reason {
        changed_fields.push("blocking_reason".to_owned());
    }

    Ok(ResolvedWorkUnitUpdate {
        changed_fields,
        next_title,
        next_description,
        next_status,
        next_priority,
        next_priority_rank,
        next_next_run_at_ms,
        next_blocking_reason,
    })
}

fn apply_resolved_work_unit_update_in_tx(
    transaction: &Transaction<'_>,
    work_unit_id: &str,
    resolved_update: &ResolvedWorkUnitUpdate,
    now_ms: i64,
) -> Result<(), String> {
    transaction
        .execute(
            "UPDATE work_units
             SET title = ?1,
                 description = ?2,
                 status = ?3,
                 priority = ?4,
                 priority_rank = ?5,
                 next_run_at_ms = ?6,
                 blocking_reason = ?7,
                 updated_at_ms = ?8
             WHERE work_unit_id = ?9
               AND archived_at_ms IS NULL",
            params![
                resolved_update.next_title,
                resolved_update.next_description,
                resolved_update.next_status,
                resolved_update.next_priority,
                resolved_update.next_priority_rank,
                resolved_update.next_next_run_at_ms,
                resolved_update.next_blocking_reason,
                now_ms,
                work_unit_id,
            ],
        )
        .map_err(|error| format!("update work unit fields failed: {error}"))?;
    Ok(())
}

fn load_ordered_child_snapshots(
    repository: &WorkUnitRepository,
    child_work_unit_ids: &[String],
) -> Result<Vec<WorkUnitSnapshot>, String> {
    let mut children = Vec::with_capacity(child_work_unit_ids.len());

    for child_work_unit_id in child_work_unit_ids {
        let child = repository
            .load_work_unit_snapshot(child_work_unit_id.as_str())?
            .ok_or_else(|| {
                format!("child work unit `{child_work_unit_id}` disappeared while loading order")
            })?;
        children.push(child);
    }

    Ok(children)
}

fn usize_from_i64(value: i64, label: &str) -> Result<usize, String> {
    usize::try_from(value).map_err(|error| format!("{label} overflowed usize: {error}"))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use serde_json::json;

    use crate::memory::runtime_config::MemoryRuntimeConfig;

    use super::{
        AcquireWorkUnitLeaseRequest, AddWorkUnitDependencyRequest, AppendWorkUnitNoteRequest,
        ArchiveWorkUnitRequest, AssignWorkUnitRequest, CompleteWorkUnitRequest,
        CreateChildWorkUnitRequest, NewWorkUnitRecord, RecordWorkUnitReviewDecisionRequest,
        RemoveWorkUnitDependencyRequest, ReplanWorkUnitRequest, RequestWorkUnitReviewRequest,
        ResequenceChildWorkUnitsRequest, SplitWorkUnitChildRequest, SplitWorkUnitRequest,
        StartWorkUnitLeaseRequest, SupersedeWorkUnitRequest, UpdateWorkUnitRequest,
        WORK_UNIT_ASSIGNED_EVENT_KIND, WORK_UNIT_CHILD_CREATED_EVENT_KIND,
        WORK_UNIT_DEPENDENCY_ADDED_EVENT_KIND, WORK_UNIT_DEPENDENCY_REMOVED_EVENT_KIND,
        WORK_UNIT_NOTE_ADDED_EVENT_KIND, WORK_UNIT_REPLANNED_EVENT_KIND,
        WORK_UNIT_RESEQUENCED_EVENT_KIND, WORK_UNIT_REVIEW_RECORDED_EVENT_KIND,
        WORK_UNIT_REVIEW_REQUESTED_EVENT_KIND, WORK_UNIT_SPLIT_APPLIED_EVENT_KIND,
        WORK_UNIT_SUPERSEDED_EVENT_KIND, WORK_UNIT_UPDATED_EVENT_KIND,
        WorkUnitCompletionDisposition, WorkUnitHeartbeatRequest, WorkUnitListQuery,
        WorkUnitRepository, WorkUnitReviewDecision,
    };
    use loongclaw_contracts::{
        WorkSourceKind, WorkUnitKind, WorkUnitPriority, WorkUnitRetryPolicy, WorkUnitReviewStatus,
        WorkUnitSourceRef, WorkUnitStatus,
    };

    fn isolated_memory_config(test_name: &str) -> MemoryRuntimeConfig {
        let base = std::env::temp_dir().join(format!(
            "loongclaw-work-unit-repository-{test_name}-{}",
            std::process::id()
        ));
        let _ = fs::create_dir_all(&base);
        let db_path = base.join("memory.sqlite3");
        let _ = fs::remove_file(&db_path);
        MemoryRuntimeConfig {
            sqlite_path: Some(db_path),
            ..MemoryRuntimeConfig::default()
        }
    }

    fn sample_source_ref() -> WorkUnitSourceRef {
        WorkUnitSourceRef {
            source_kind: WorkSourceKind::Discord,
            project_id: Some("loongclaw-ai/server".to_owned()),
            channel_id: Some("feature".to_owned()),
            thread_id: Some("thread-42".to_owned()),
            message_id: Some("msg-7".to_owned()),
            external_ref: Some("feature-thread".to_owned()),
            source_url: Some("https://discord.example/feature/thread-42".to_owned()),
        }
    }

    fn sample_work_unit(status: WorkUnitStatus) -> NewWorkUnitRecord {
        NewWorkUnitRecord {
            work_unit_id: Some("wu-test".to_owned()),
            kind: WorkUnitKind::Feature,
            title: "Durable runtime foundation".to_owned(),
            description: "Implement the first durable work-unit runtime slice".to_owned(),
            source_ref: sample_source_ref(),
            status,
            priority: WorkUnitPriority::High,
            retry_policy: WorkUnitRetryPolicy {
                max_attempts: 3,
                initial_backoff_ms: 1_000,
                max_backoff_ms: 8_000,
            },
            parent_work_unit_id: None,
            plan_position: None,
            next_run_at_ms: Some(1_000),
        }
    }

    #[test]
    fn create_work_unit_round_trips_snapshot_fields() {
        let config = isolated_memory_config("create-roundtrip");
        let repository = WorkUnitRepository::new(&config).expect("repository");
        let created = repository
            .create_work_unit(sample_work_unit(WorkUnitStatus::Ready), Some("operator"))
            .expect("create work unit");

        assert_eq!(created.work_unit.work_unit_id, "wu-test");
        assert_eq!(created.work_unit.status, WorkUnitStatus::Ready);
        assert_eq!(created.work_unit.priority, WorkUnitPriority::High);
        assert_eq!(
            created.work_unit.source_ref.source_kind,
            WorkSourceKind::Discord
        );
        assert_eq!(created.work_unit.attempt_count, 0);
        assert_eq!(created.work_unit.assigned_to, None);
        assert!(created.work_unit.blocks_work_unit_ids.is_empty());
        assert!(created.work_unit.blocked_by_work_unit_ids.is_empty());
        assert!(created.lease.is_none());

        let events = repository
            .list_work_unit_events("wu-test", 10)
            .expect("list work unit events");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_kind, "work_unit_created");
    }

    #[test]
    fn create_child_work_unit_inherits_parent_shape_and_blocks_parent_when_requested() {
        let config = isolated_memory_config("create-child-work-unit");
        let repository = WorkUnitRepository::new(&config).expect("repository");
        let parent = NewWorkUnitRecord {
            work_unit_id: Some("wu-parent".to_owned()),
            title: "Parent".to_owned(),
            description: "Coordinate child work".to_owned(),
            ..sample_work_unit(WorkUnitStatus::Ready)
        };
        repository
            .create_work_unit(parent, Some("operator"))
            .expect("create parent work unit");

        let child = NewWorkUnitRecord {
            work_unit_id: Some("wu-child".to_owned()),
            kind: WorkUnitKind::Issue,
            title: "Child".to_owned(),
            description: "Handle child work".to_owned(),
            source_ref: WorkUnitSourceRef::default(),
            status: WorkUnitStatus::Ready,
            priority: WorkUnitPriority::Normal,
            retry_policy: WorkUnitRetryPolicy::default(),
            parent_work_unit_id: None,
            plan_position: None,
            next_run_at_ms: Some(1_100),
        };
        let child_snapshot = repository
            .create_child_work_unit(CreateChildWorkUnitRequest {
                parent_work_unit_id: "wu-parent".to_owned(),
                child,
                inherit_parent_source_ref: true,
                inherit_parent_retry_policy: true,
                inherit_parent_priority: true,
                block_parent: true,
                actor: Some("planner".to_owned()),
            })
            .expect("create child work unit");

        assert_eq!(
            child_snapshot.work_unit.parent_work_unit_id.as_deref(),
            Some("wu-parent")
        );
        assert_eq!(
            child_snapshot.work_unit.source_ref.source_kind,
            WorkSourceKind::Discord
        );
        assert_eq!(child_snapshot.work_unit.priority, WorkUnitPriority::High);
        assert_eq!(
            child_snapshot.work_unit.retry_policy.max_attempts,
            sample_work_unit(WorkUnitStatus::Ready)
                .retry_policy
                .max_attempts
        );

        let parent_snapshot = repository
            .load_work_unit_snapshot("wu-parent")
            .expect("load parent snapshot")
            .expect("parent snapshot");
        assert_eq!(
            parent_snapshot.work_unit.child_work_unit_ids,
            vec!["wu-child".to_owned()]
        );
        assert_eq!(
            parent_snapshot.work_unit.blocked_by_work_unit_ids,
            vec!["wu-child".to_owned()]
        );

        let claimed_parent = repository
            .acquire_next_ready_lease(AcquireWorkUnitLeaseRequest {
                owner: "worker-a".to_owned(),
                ttl_ms: 5_000,
                actor: Some("scheduler".to_owned()),
                now_ms: Some(2_000),
            })
            .expect("claim next ready");
        assert_eq!(
            claimed_parent
                .as_ref()
                .map(|snapshot| snapshot.work_unit.work_unit_id.as_str()),
            Some("wu-child")
        );

        let events = repository
            .list_work_unit_events("wu-child", 10)
            .expect("list child events");
        assert!(
            events
                .iter()
                .any(|event| event.event_kind == WORK_UNIT_CHILD_CREATED_EVENT_KIND),
            "expected child-created event"
        );
    }

    #[test]
    fn create_child_work_unit_respects_explicit_default_overrides() {
        let config = isolated_memory_config("create-child-work-unit-explicit-overrides");
        let repository = WorkUnitRepository::new(&config).expect("repository");
        let parent = NewWorkUnitRecord {
            work_unit_id: Some("wu-parent".to_owned()),
            title: "Parent".to_owned(),
            description: "Coordinate child work".to_owned(),
            ..sample_work_unit(WorkUnitStatus::Ready)
        };
        repository
            .create_work_unit(parent, Some("operator"))
            .expect("create parent work unit");

        let child_retry_policy = WorkUnitRetryPolicy::default();
        let child_source_ref = WorkUnitSourceRef::default();
        let child = NewWorkUnitRecord {
            work_unit_id: Some("wu-child-manual".to_owned()),
            kind: WorkUnitKind::Issue,
            title: "Child".to_owned(),
            description: "Stay manual and normal".to_owned(),
            source_ref: child_source_ref.clone(),
            status: WorkUnitStatus::Ready,
            priority: WorkUnitPriority::Normal,
            retry_policy: child_retry_policy.clone(),
            parent_work_unit_id: None,
            plan_position: None,
            next_run_at_ms: Some(1_200),
        };
        let child_snapshot = repository
            .create_child_work_unit(CreateChildWorkUnitRequest {
                parent_work_unit_id: "wu-parent".to_owned(),
                child,
                inherit_parent_source_ref: false,
                inherit_parent_retry_policy: false,
                inherit_parent_priority: false,
                block_parent: false,
                actor: Some("planner".to_owned()),
            })
            .expect("create child work unit with explicit overrides");

        assert_eq!(child_snapshot.work_unit.priority, WorkUnitPriority::Normal);
        assert_eq!(child_snapshot.work_unit.retry_policy, child_retry_policy);
        assert_eq!(child_snapshot.work_unit.source_ref, child_source_ref);
        assert!(child_snapshot.work_unit.blocks_work_unit_ids.is_empty());

        let parent_snapshot = repository
            .load_work_unit_snapshot("wu-parent")
            .expect("load parent snapshot")
            .expect("parent snapshot");
        assert_eq!(
            parent_snapshot.work_unit.child_work_unit_ids,
            vec!["wu-child-manual".to_owned()]
        );
        assert!(
            parent_snapshot
                .work_unit
                .blocked_by_work_unit_ids
                .is_empty()
        );
    }

    #[test]
    fn split_work_unit_creates_multiple_children_and_blocks_parent_until_each_finishes() {
        let config = isolated_memory_config("split-work-unit");
        let repository = WorkUnitRepository::new(&config).expect("repository");
        let parent = NewWorkUnitRecord {
            work_unit_id: Some("wu-parent".to_owned()),
            title: "Parent".to_owned(),
            description: "Coordinate child work".to_owned(),
            ..sample_work_unit(WorkUnitStatus::Ready)
        };
        repository
            .create_work_unit(parent, Some("operator"))
            .expect("create parent work unit");

        let child_a = SplitWorkUnitChildRequest {
            child: NewWorkUnitRecord {
                work_unit_id: Some("wu-child-a".to_owned()),
                kind: WorkUnitKind::Issue,
                title: "Child A".to_owned(),
                description: "Handle dependency A".to_owned(),
                source_ref: WorkUnitSourceRef::default(),
                status: WorkUnitStatus::Ready,
                priority: WorkUnitPriority::Normal,
                retry_policy: WorkUnitRetryPolicy::default(),
                parent_work_unit_id: None,
                plan_position: None,
                next_run_at_ms: Some(1_010),
            },
            inherit_parent_source_ref: true,
            inherit_parent_retry_policy: true,
            inherit_parent_priority: true,
        };
        let child_b = SplitWorkUnitChildRequest {
            child: NewWorkUnitRecord {
                work_unit_id: Some("wu-child-b".to_owned()),
                kind: WorkUnitKind::Issue,
                title: "Child B".to_owned(),
                description: "Handle dependency B".to_owned(),
                source_ref: WorkUnitSourceRef::default(),
                status: WorkUnitStatus::Ready,
                priority: WorkUnitPriority::Normal,
                retry_policy: WorkUnitRetryPolicy::default(),
                parent_work_unit_id: None,
                plan_position: None,
                next_run_at_ms: Some(1_020),
            },
            inherit_parent_source_ref: true,
            inherit_parent_retry_policy: true,
            inherit_parent_priority: true,
        };
        let split = repository
            .split_work_unit(SplitWorkUnitRequest {
                parent_work_unit_id: "wu-parent".to_owned(),
                children: vec![child_a, child_b],
                block_parent: true,
                actor: Some("planner".to_owned()),
            })
            .expect("split work unit");

        assert_eq!(split.parent.work_unit.work_unit_id, "wu-parent");
        assert_eq!(
            split.parent.work_unit.child_work_unit_ids,
            vec!["wu-child-a".to_owned(), "wu-child-b".to_owned()]
        );
        assert_eq!(
            split.parent.work_unit.blocked_by_work_unit_ids,
            vec!["wu-child-a".to_owned(), "wu-child-b".to_owned()]
        );
        assert_eq!(split.children.len(), 2);
        assert_eq!(
            split.children[0].work_unit.blocks_work_unit_ids,
            vec!["wu-parent".to_owned()]
        );
        assert_eq!(
            split.children[1].work_unit.blocks_work_unit_ids,
            vec!["wu-parent".to_owned()]
        );
        assert_eq!(split.children[0].work_unit.plan_position, Some(1));
        assert_eq!(split.children[1].work_unit.plan_position, Some(2));

        let claimed_child_a = repository
            .acquire_next_ready_lease(AcquireWorkUnitLeaseRequest {
                owner: "worker-a".to_owned(),
                ttl_ms: 5_000,
                actor: Some("scheduler".to_owned()),
                now_ms: Some(2_000),
            })
            .expect("claim first ready child")
            .expect("expected first child claim");
        assert_eq!(claimed_child_a.work_unit.work_unit_id, "wu-child-a");

        repository
            .complete_work_unit(CompleteWorkUnitRequest {
                work_unit_id: "wu-child-a".to_owned(),
                owner: "worker-a".to_owned(),
                disposition: WorkUnitCompletionDisposition::Completed,
                actor: Some("worker-a".to_owned()),
                now_ms: Some(2_010),
                next_run_at_ms: None,
                result_payload_json: None,
                error: None,
            })
            .expect("complete child a");

        let claimed_child_b = repository
            .acquire_next_ready_lease(AcquireWorkUnitLeaseRequest {
                owner: "worker-b".to_owned(),
                ttl_ms: 5_000,
                actor: Some("scheduler".to_owned()),
                now_ms: Some(2_020),
            })
            .expect("claim second ready child")
            .expect("expected second child claim");
        assert_eq!(claimed_child_b.work_unit.work_unit_id, "wu-child-b");

        repository
            .complete_work_unit(CompleteWorkUnitRequest {
                work_unit_id: "wu-child-b".to_owned(),
                owner: "worker-b".to_owned(),
                disposition: WorkUnitCompletionDisposition::Completed,
                actor: Some("worker-b".to_owned()),
                now_ms: Some(2_030),
                next_run_at_ms: None,
                result_payload_json: None,
                error: None,
            })
            .expect("complete child b");

        let claimed_parent = repository
            .acquire_next_ready_lease(AcquireWorkUnitLeaseRequest {
                owner: "worker-c".to_owned(),
                ttl_ms: 5_000,
                actor: Some("scheduler".to_owned()),
                now_ms: Some(2_040),
            })
            .expect("claim parent after children complete")
            .expect("expected parent claim");
        assert_eq!(claimed_parent.work_unit.work_unit_id, "wu-parent");

        let events = repository
            .list_work_unit_events("wu-parent", 10)
            .expect("list parent events");
        assert!(
            events
                .iter()
                .any(|event| event.event_kind == WORK_UNIT_SPLIT_APPLIED_EVENT_KIND),
            "expected split-applied event on the parent"
        );
    }

    #[test]
    fn split_work_unit_rejects_less_than_two_children() {
        let config = isolated_memory_config("split-work-unit-too-small");
        let repository = WorkUnitRepository::new(&config).expect("repository");
        repository
            .create_work_unit(sample_work_unit(WorkUnitStatus::Ready), Some("operator"))
            .expect("create parent work unit");

        let error = repository
            .split_work_unit(SplitWorkUnitRequest {
                parent_work_unit_id: "wu-test".to_owned(),
                children: vec![SplitWorkUnitChildRequest {
                    child: NewWorkUnitRecord {
                        work_unit_id: Some("wu-child-only".to_owned()),
                        kind: WorkUnitKind::Issue,
                        title: "Only child".to_owned(),
                        description: "This should fail".to_owned(),
                        source_ref: WorkUnitSourceRef::default(),
                        status: WorkUnitStatus::Ready,
                        priority: WorkUnitPriority::Normal,
                        retry_policy: WorkUnitRetryPolicy::default(),
                        parent_work_unit_id: None,
                        plan_position: None,
                        next_run_at_ms: Some(1_010),
                    },
                    inherit_parent_source_ref: true,
                    inherit_parent_retry_policy: true,
                    inherit_parent_priority: true,
                }],
                block_parent: true,
                actor: Some("planner".to_owned()),
            })
            .expect_err("split should require at least two children");

        assert!(error.contains("at least 2 child work units"));
    }

    #[test]
    fn split_work_unit_rejects_duplicate_child_ids() {
        let config = isolated_memory_config("split-work-unit-duplicate-child-id");
        let repository = WorkUnitRepository::new(&config).expect("repository");
        repository
            .create_work_unit(sample_work_unit(WorkUnitStatus::Ready), Some("operator"))
            .expect("create parent work unit");

        let duplicate_child = SplitWorkUnitChildRequest {
            child: NewWorkUnitRecord {
                work_unit_id: Some("wu-child-dup".to_owned()),
                kind: WorkUnitKind::Issue,
                title: "Child".to_owned(),
                description: "Duplicate id".to_owned(),
                source_ref: WorkUnitSourceRef::default(),
                status: WorkUnitStatus::Ready,
                priority: WorkUnitPriority::Normal,
                retry_policy: WorkUnitRetryPolicy::default(),
                parent_work_unit_id: None,
                plan_position: None,
                next_run_at_ms: Some(1_010),
            },
            inherit_parent_source_ref: true,
            inherit_parent_retry_policy: true,
            inherit_parent_priority: true,
        };
        let error = repository
            .split_work_unit(SplitWorkUnitRequest {
                parent_work_unit_id: "wu-test".to_owned(),
                children: vec![duplicate_child.clone(), duplicate_child],
                block_parent: true,
                actor: Some("planner".to_owned()),
            })
            .expect_err("split should reject duplicate child ids");

        assert!(error.contains("is duplicated"));
    }

    #[test]
    fn resequence_child_work_units_updates_parent_order_and_plan_positions() {
        let config = isolated_memory_config("resequence-child-work-units");
        let repository = WorkUnitRepository::new(&config).expect("repository");
        let parent = NewWorkUnitRecord {
            work_unit_id: Some("wu-parent".to_owned()),
            title: "Parent".to_owned(),
            description: "Coordinate child work".to_owned(),
            ..sample_work_unit(WorkUnitStatus::Ready)
        };
        repository
            .create_work_unit(parent, Some("operator"))
            .expect("create parent work unit");

        let child_a = SplitWorkUnitChildRequest {
            child: NewWorkUnitRecord {
                work_unit_id: Some("wu-child-a".to_owned()),
                kind: WorkUnitKind::Issue,
                title: "Child A".to_owned(),
                description: "Handle dependency A".to_owned(),
                source_ref: WorkUnitSourceRef::default(),
                status: WorkUnitStatus::Ready,
                priority: WorkUnitPriority::Normal,
                retry_policy: WorkUnitRetryPolicy::default(),
                parent_work_unit_id: None,
                plan_position: None,
                next_run_at_ms: Some(1_010),
            },
            inherit_parent_source_ref: true,
            inherit_parent_retry_policy: true,
            inherit_parent_priority: true,
        };
        let child_b = SplitWorkUnitChildRequest {
            child: NewWorkUnitRecord {
                work_unit_id: Some("wu-child-b".to_owned()),
                kind: WorkUnitKind::Issue,
                title: "Child B".to_owned(),
                description: "Handle dependency B".to_owned(),
                source_ref: WorkUnitSourceRef::default(),
                status: WorkUnitStatus::Ready,
                priority: WorkUnitPriority::Normal,
                retry_policy: WorkUnitRetryPolicy::default(),
                parent_work_unit_id: None,
                plan_position: None,
                next_run_at_ms: Some(1_020),
            },
            inherit_parent_source_ref: true,
            inherit_parent_retry_policy: true,
            inherit_parent_priority: true,
        };
        repository
            .split_work_unit(SplitWorkUnitRequest {
                parent_work_unit_id: "wu-parent".to_owned(),
                children: vec![child_a, child_b],
                block_parent: true,
                actor: Some("planner".to_owned()),
            })
            .expect("split work unit");

        let resequenced = repository
            .resequence_child_work_units(ResequenceChildWorkUnitsRequest {
                parent_work_unit_id: "wu-parent".to_owned(),
                ordered_child_work_unit_ids: vec!["wu-child-b".to_owned(), "wu-child-a".to_owned()],
                actor: Some("planner".to_owned()),
                now_ms: Some(2_000),
            })
            .expect("resequence children");

        assert_eq!(
            resequenced.parent.work_unit.child_work_unit_ids,
            vec!["wu-child-b".to_owned(), "wu-child-a".to_owned()]
        );
        assert_eq!(resequenced.children[0].work_unit.plan_position, Some(1));
        assert_eq!(resequenced.children[1].work_unit.plan_position, Some(2));

        let events = repository
            .list_work_unit_events("wu-parent", 20)
            .expect("list parent events");
        assert!(
            events
                .iter()
                .any(|event| event.event_kind == WORK_UNIT_RESEQUENCED_EVENT_KIND),
            "expected resequenced event on parent"
        );
    }

    #[test]
    fn replan_work_unit_updates_parent_fields_and_child_order_atomically() {
        let config = isolated_memory_config("replan-work-unit");
        let repository = WorkUnitRepository::new(&config).expect("repository");
        let parent = NewWorkUnitRecord {
            work_unit_id: Some("wu-parent".to_owned()),
            title: "Parent".to_owned(),
            description: "Original plan".to_owned(),
            ..sample_work_unit(WorkUnitStatus::Ready)
        };
        repository
            .create_work_unit(parent, Some("operator"))
            .expect("create parent work unit");

        let child_a = SplitWorkUnitChildRequest {
            child: NewWorkUnitRecord {
                work_unit_id: Some("wu-child-a".to_owned()),
                kind: WorkUnitKind::Issue,
                title: "Child A".to_owned(),
                description: "Handle dependency A".to_owned(),
                source_ref: WorkUnitSourceRef::default(),
                status: WorkUnitStatus::Ready,
                priority: WorkUnitPriority::Normal,
                retry_policy: WorkUnitRetryPolicy::default(),
                parent_work_unit_id: None,
                plan_position: None,
                next_run_at_ms: Some(1_010),
            },
            inherit_parent_source_ref: true,
            inherit_parent_retry_policy: true,
            inherit_parent_priority: true,
        };
        let child_b = SplitWorkUnitChildRequest {
            child: NewWorkUnitRecord {
                work_unit_id: Some("wu-child-b".to_owned()),
                kind: WorkUnitKind::Issue,
                title: "Child B".to_owned(),
                description: "Handle dependency B".to_owned(),
                source_ref: WorkUnitSourceRef::default(),
                status: WorkUnitStatus::Ready,
                priority: WorkUnitPriority::Normal,
                retry_policy: WorkUnitRetryPolicy::default(),
                parent_work_unit_id: None,
                plan_position: None,
                next_run_at_ms: Some(1_020),
            },
            inherit_parent_source_ref: true,
            inherit_parent_retry_policy: true,
            inherit_parent_priority: true,
        };
        repository
            .split_work_unit(SplitWorkUnitRequest {
                parent_work_unit_id: "wu-parent".to_owned(),
                children: vec![child_a, child_b],
                block_parent: true,
                actor: Some("planner".to_owned()),
            })
            .expect("split work unit");

        let replanned = repository
            .replan_work_unit(ReplanWorkUnitRequest {
                work_unit_id: "wu-parent".to_owned(),
                title: Some("Replanned parent".to_owned()),
                description: Some("Updated plan summary".to_owned()),
                status: Some(WorkUnitStatus::Triaged),
                priority: Some(WorkUnitPriority::Critical),
                next_run_at_ms: Some(1_111),
                blocking_reason: Some("waiting on revised execution".to_owned()),
                clear_blocking_reason: false,
                ordered_child_work_unit_ids: Some(vec![
                    "wu-child-b".to_owned(),
                    "wu-child-a".to_owned(),
                ]),
                actor: Some("planner".to_owned()),
                now_ms: Some(2_000),
            })
            .expect("replan work unit")
            .expect("replanned result");

        assert_eq!(replanned.parent.work_unit.title, "Replanned parent");
        assert_eq!(
            replanned.parent.work_unit.description,
            "Updated plan summary"
        );
        assert_eq!(replanned.parent.work_unit.status, WorkUnitStatus::Triaged);
        assert_eq!(
            replanned.parent.work_unit.priority,
            WorkUnitPriority::Critical
        );
        assert_eq!(replanned.parent.work_unit.next_run_at_ms, 1_111);
        assert_eq!(
            replanned.parent.work_unit.blocking_reason.as_deref(),
            Some("waiting on revised execution")
        );
        assert_eq!(
            replanned.parent.work_unit.child_work_unit_ids,
            vec!["wu-child-b".to_owned(), "wu-child-a".to_owned()]
        );
        assert_eq!(replanned.children[0].work_unit.plan_position, Some(1));
        assert_eq!(replanned.children[1].work_unit.plan_position, Some(2));

        let events = repository
            .list_work_unit_events("wu-parent", 20)
            .expect("list parent events");
        assert!(
            events
                .iter()
                .any(|event| event.event_kind == WORK_UNIT_REPLANNED_EVENT_KIND),
            "expected replanned event on parent"
        );
    }

    #[test]
    fn supersede_work_unit_transfers_dependency_context_to_replacement() {
        let config = isolated_memory_config("supersede-work-unit");
        let repository = WorkUnitRepository::new(&config).expect("repository");
        let blocker = NewWorkUnitRecord {
            work_unit_id: Some("wu-blocker".to_owned()),
            kind: WorkUnitKind::Ops,
            title: "Blocker".to_owned(),
            description: "Must complete first".to_owned(),
            source_ref: WorkUnitSourceRef::default(),
            status: WorkUnitStatus::Ready,
            priority: WorkUnitPriority::Low,
            retry_policy: WorkUnitRetryPolicy::default(),
            parent_work_unit_id: None,
            plan_position: None,
            next_run_at_ms: Some(1_000),
        };
        let obsolete = NewWorkUnitRecord {
            work_unit_id: Some("wu-obsolete".to_owned()),
            title: "Obsolete".to_owned(),
            description: "Will be replaced".to_owned(),
            ..sample_work_unit(WorkUnitStatus::Ready)
        };
        let replacement = NewWorkUnitRecord {
            work_unit_id: Some("wu-replacement".to_owned()),
            title: "Replacement".to_owned(),
            description: "Takes over".to_owned(),
            ..sample_work_unit(WorkUnitStatus::Triaged)
        };
        let blocked = NewWorkUnitRecord {
            work_unit_id: Some("wu-blocked".to_owned()),
            kind: WorkUnitKind::Review,
            title: "Blocked".to_owned(),
            description: "Depends on obsolete".to_owned(),
            source_ref: WorkUnitSourceRef::default(),
            status: WorkUnitStatus::Ready,
            priority: WorkUnitPriority::Normal,
            retry_policy: WorkUnitRetryPolicy::default(),
            parent_work_unit_id: None,
            plan_position: None,
            next_run_at_ms: Some(1_020),
        };
        repository
            .create_work_unit(blocker, Some("operator"))
            .expect("create blocker");
        repository
            .create_work_unit(obsolete, Some("operator"))
            .expect("create obsolete");
        repository
            .create_work_unit(replacement, Some("operator"))
            .expect("create replacement");
        repository
            .create_work_unit(blocked, Some("operator"))
            .expect("create blocked");
        repository
            .add_dependency(AddWorkUnitDependencyRequest {
                blocking_work_unit_id: "wu-blocker".to_owned(),
                blocked_work_unit_id: "wu-obsolete".to_owned(),
                actor: Some("planner".to_owned()),
                now_ms: Some(1_030),
            })
            .expect("block obsolete");
        repository
            .add_dependency(AddWorkUnitDependencyRequest {
                blocking_work_unit_id: "wu-obsolete".to_owned(),
                blocked_work_unit_id: "wu-blocked".to_owned(),
                actor: Some("planner".to_owned()),
                now_ms: Some(1_040),
            })
            .expect("obsolete blocks blocked");

        let result = repository
            .supersede_work_unit(SupersedeWorkUnitRequest {
                obsolete_work_unit_id: "wu-obsolete".to_owned(),
                replacement_work_unit_id: "wu-replacement".to_owned(),
                actor: Some("planner".to_owned()),
                now_ms: Some(1_050),
            })
            .expect("supersede obsolete");

        assert_eq!(result.obsolete.work_unit.status, WorkUnitStatus::Cancelled);
        assert_eq!(
            result
                .obsolete
                .work_unit
                .superseded_by_work_unit_id
                .as_deref(),
            Some("wu-replacement")
        );
        assert_eq!(
            result.replacement.work_unit.supersedes_work_unit_ids,
            vec!["wu-obsolete".to_owned()]
        );
        assert_eq!(
            result.replacement.work_unit.blocked_by_work_unit_ids,
            vec!["wu-blocker".to_owned()]
        );

        let blocked_snapshot = repository
            .load_work_unit_snapshot("wu-blocked")
            .expect("load blocked snapshot")
            .expect("blocked snapshot");
        assert_eq!(
            blocked_snapshot.work_unit.blocked_by_work_unit_ids,
            vec!["wu-replacement".to_owned()]
        );

        let events = repository
            .list_work_unit_events("wu-obsolete", 10)
            .expect("list obsolete events");
        assert!(
            events
                .iter()
                .any(|event| event.event_kind == WORK_UNIT_SUPERSEDED_EVENT_KIND),
            "expected superseded event on the obsolete work unit"
        );
    }

    #[test]
    fn supersede_work_unit_rejects_terminal_obsolete_work() {
        let config = isolated_memory_config("supersede-work-unit-terminal");
        let repository = WorkUnitRepository::new(&config).expect("repository");
        repository
            .create_work_unit(sample_work_unit(WorkUnitStatus::Ready), Some("operator"))
            .expect("create obsolete");
        repository
            .update_work_unit(UpdateWorkUnitRequest {
                work_unit_id: "wu-test".to_owned(),
                title: None,
                description: None,
                status: Some(WorkUnitStatus::Cancelled),
                priority: None,
                next_run_at_ms: None,
                blocking_reason: None,
                clear_blocking_reason: false,
                actor: Some("operator".to_owned()),
                now_ms: Some(990),
            })
            .expect("cancel obsolete")
            .expect("cancelled snapshot");
        repository
            .create_work_unit(
                NewWorkUnitRecord {
                    work_unit_id: Some("wu-replacement".to_owned()),
                    title: "Replacement".to_owned(),
                    description: "Takes over".to_owned(),
                    ..sample_work_unit(WorkUnitStatus::Ready)
                },
                Some("operator"),
            )
            .expect("create replacement");

        let error = repository
            .supersede_work_unit(SupersedeWorkUnitRequest {
                obsolete_work_unit_id: "wu-test".to_owned(),
                replacement_work_unit_id: "wu-replacement".to_owned(),
                actor: Some("planner".to_owned()),
                now_ms: Some(1_000),
            })
            .expect_err("terminal obsolete work should not supersede");

        assert!(error.contains("cannot supersede terminal work unit"));
    }

    #[test]
    fn update_work_unit_mutates_editable_fields_and_records_event() {
        let config = isolated_memory_config("update-fields");
        let repository = WorkUnitRepository::new(&config).expect("repository");
        repository
            .create_work_unit(sample_work_unit(WorkUnitStatus::Triaged), Some("operator"))
            .expect("create work unit");

        let updated = repository
            .update_work_unit(UpdateWorkUnitRequest {
                work_unit_id: "wu-test".to_owned(),
                title: Some("Durable runtime foundation v2".to_owned()),
                description: Some("Refine orchestration surface".to_owned()),
                status: Some(WorkUnitStatus::WaitingReview),
                priority: Some(WorkUnitPriority::Critical),
                next_run_at_ms: Some(2_500),
                blocking_reason: Some("waiting for design review".to_owned()),
                clear_blocking_reason: false,
                actor: Some("planner".to_owned()),
                now_ms: Some(2_000),
            })
            .expect("update work unit")
            .expect("updated snapshot");
        assert_eq!(updated.work_unit.title, "Durable runtime foundation v2");
        assert_eq!(
            updated.work_unit.description,
            "Refine orchestration surface"
        );
        assert_eq!(updated.work_unit.status, WorkUnitStatus::WaitingReview);
        assert_eq!(updated.work_unit.priority, WorkUnitPriority::Critical);
        assert_eq!(updated.work_unit.next_run_at_ms, 2_500);
        assert_eq!(
            updated.work_unit.blocking_reason.as_deref(),
            Some("waiting for design review")
        );

        let ready = repository
            .update_work_unit(UpdateWorkUnitRequest {
                work_unit_id: "wu-test".to_owned(),
                title: None,
                description: None,
                status: Some(WorkUnitStatus::Ready),
                priority: None,
                next_run_at_ms: None,
                blocking_reason: None,
                clear_blocking_reason: true,
                actor: Some("planner".to_owned()),
                now_ms: Some(2_100),
            })
            .expect("clear blocking reason")
            .expect("ready snapshot");
        assert_eq!(ready.work_unit.status, WorkUnitStatus::Ready);
        assert_eq!(ready.work_unit.blocking_reason, None);

        let events = repository
            .list_work_unit_events("wu-test", 10)
            .expect("list work unit events");
        assert!(
            events
                .iter()
                .any(|event| event.event_kind == WORK_UNIT_UPDATED_EVENT_KIND),
            "expected work-unit update event"
        );
    }

    #[test]
    fn update_work_unit_rejects_runtime_owned_status_transition() {
        let config = isolated_memory_config("update-runtime-owned-status");
        let repository = WorkUnitRepository::new(&config).expect("repository");
        repository
            .create_work_unit(sample_work_unit(WorkUnitStatus::Ready), Some("operator"))
            .expect("create work unit");
        repository
            .acquire_next_ready_lease(AcquireWorkUnitLeaseRequest {
                owner: "worker-a".to_owned(),
                ttl_ms: 5_000,
                actor: Some("scheduler".to_owned()),
                now_ms: Some(1_000),
            })
            .expect("acquire lease")
            .expect("leased snapshot");

        let error = repository
            .update_work_unit(UpdateWorkUnitRequest {
                work_unit_id: "wu-test".to_owned(),
                title: None,
                description: None,
                status: Some(WorkUnitStatus::WaitingReview),
                priority: None,
                next_run_at_ms: None,
                blocking_reason: None,
                clear_blocking_reason: false,
                actor: Some("planner".to_owned()),
                now_ms: Some(1_100),
            })
            .expect_err("runtime-owned status transition should be rejected");

        assert!(error.contains("cannot change status from `leased`"));
    }

    #[test]
    fn review_request_and_decision_round_trip_through_snapshot() {
        let config = isolated_memory_config("review-round-trip");
        let repository = WorkUnitRepository::new(&config).expect("repository");
        repository
            .create_work_unit(sample_work_unit(WorkUnitStatus::Ready), Some("operator"))
            .expect("create work unit");

        let pending = repository
            .request_work_unit_review(RequestWorkUnitReviewRequest {
                work_unit_id: "wu-test".to_owned(),
                requested_by: Some("planner".to_owned()),
                reviewer: Some("reviewer-a".to_owned()),
                summary: Some("Please verify the plan".to_owned()),
                now_ms: Some(3_000),
            })
            .expect("request review")
            .expect("pending review snapshot");
        assert_eq!(pending.work_unit.status, WorkUnitStatus::WaitingReview);
        let pending_review = pending.work_unit.review.expect("pending review");
        assert_eq!(pending_review.status, WorkUnitReviewStatus::Pending);
        assert_eq!(pending_review.requested_by.as_deref(), Some("planner"));
        assert_eq!(pending_review.reviewer.as_deref(), Some("reviewer-a"));

        let approved = repository
            .record_work_unit_review_decision(RecordWorkUnitReviewDecisionRequest {
                work_unit_id: "wu-test".to_owned(),
                reviewer: Some("reviewer-a".to_owned()),
                decision: WorkUnitReviewDecision::Approve,
                summary: Some("looks good".to_owned()),
                now_ms: Some(3_100),
            })
            .expect("record review decision")
            .expect("approved snapshot");
        assert_eq!(approved.work_unit.status, WorkUnitStatus::Ready);
        assert_eq!(approved.work_unit.blocking_reason, None);
        let approved_review = approved.work_unit.review.expect("approved review");
        assert_eq!(approved_review.status, WorkUnitReviewStatus::Approved);
        assert_eq!(approved_review.decided_at_ms, Some(3_100));
        assert_eq!(approved_review.summary.as_deref(), Some("looks good"));

        let events = repository
            .list_work_unit_events("wu-test", 10)
            .expect("list work unit events");
        assert!(
            events
                .iter()
                .any(|event| event.event_kind == WORK_UNIT_REVIEW_REQUESTED_EVENT_KIND),
            "expected review request event"
        );
        assert!(
            events
                .iter()
                .any(|event| event.event_kind == WORK_UNIT_REVIEW_RECORDED_EVENT_KIND),
            "expected review decision event"
        );
    }

    #[test]
    fn review_decision_can_request_changes_and_reject() {
        let config = isolated_memory_config("review-decision-variants");
        let repository = WorkUnitRepository::new(&config).expect("repository");

        let triage = NewWorkUnitRecord {
            work_unit_id: Some("wu-triage".to_owned()),
            ..sample_work_unit(WorkUnitStatus::Ready)
        };
        let reject = NewWorkUnitRecord {
            work_unit_id: Some("wu-reject".to_owned()),
            ..sample_work_unit(WorkUnitStatus::Ready)
        };

        repository
            .create_work_unit(triage, Some("operator"))
            .expect("create review triage work unit");
        repository
            .create_work_unit(reject, Some("operator"))
            .expect("create review reject work unit");

        repository
            .request_work_unit_review(RequestWorkUnitReviewRequest {
                work_unit_id: "wu-triage".to_owned(),
                requested_by: Some("planner".to_owned()),
                reviewer: Some("reviewer-a".to_owned()),
                summary: Some("needs plan review".to_owned()),
                now_ms: Some(4_000),
            })
            .expect("request triage review")
            .expect("triage pending review");
        let triaged = repository
            .record_work_unit_review_decision(RecordWorkUnitReviewDecisionRequest {
                work_unit_id: "wu-triage".to_owned(),
                reviewer: Some("reviewer-a".to_owned()),
                decision: WorkUnitReviewDecision::RequestChanges,
                summary: Some("tighten acceptance criteria".to_owned()),
                now_ms: Some(4_100),
            })
            .expect("record change request")
            .expect("triaged snapshot");
        assert_eq!(triaged.work_unit.status, WorkUnitStatus::Triaged);
        assert_eq!(
            triaged.work_unit.blocking_reason.as_deref(),
            Some("tighten acceptance criteria")
        );

        repository
            .request_work_unit_review(RequestWorkUnitReviewRequest {
                work_unit_id: "wu-reject".to_owned(),
                requested_by: Some("planner".to_owned()),
                reviewer: Some("reviewer-b".to_owned()),
                summary: Some("needs go/no-go".to_owned()),
                now_ms: Some(4_200),
            })
            .expect("request reject review")
            .expect("reject pending review");
        let rejected = repository
            .record_work_unit_review_decision(RecordWorkUnitReviewDecisionRequest {
                work_unit_id: "wu-reject".to_owned(),
                reviewer: Some("reviewer-b".to_owned()),
                decision: WorkUnitReviewDecision::Reject,
                summary: Some("not worth pursuing".to_owned()),
                now_ms: Some(4_300),
            })
            .expect("record reject decision")
            .expect("rejected snapshot");
        assert_eq!(rejected.work_unit.status, WorkUnitStatus::Cancelled);
        assert_eq!(
            rejected.work_unit.blocking_reason.as_deref(),
            Some("not worth pursuing")
        );
    }

    #[test]
    fn acquire_start_heartbeat_complete_flow_updates_snapshot_and_events() {
        let config = isolated_memory_config("lease-flow");
        let repository = WorkUnitRepository::new(&config).expect("repository");
        repository
            .create_work_unit(sample_work_unit(WorkUnitStatus::Ready), Some("operator"))
            .expect("create work unit");

        let leased = repository
            .acquire_next_ready_lease(AcquireWorkUnitLeaseRequest {
                owner: "worker-a".to_owned(),
                ttl_ms: 5_000,
                actor: Some("scheduler".to_owned()),
                now_ms: Some(2_000),
            })
            .expect("acquire lease")
            .expect("leased work unit");
        assert_eq!(leased.work_unit.status, WorkUnitStatus::Leased);
        assert_eq!(leased.work_unit.attempt_count, 1);
        assert_eq!(leased.lease.as_ref().expect("lease").owner, "worker-a");

        let running = repository
            .mark_leased_running(StartWorkUnitLeaseRequest {
                work_unit_id: "wu-test".to_owned(),
                owner: "worker-a".to_owned(),
                actor: Some("worker-a".to_owned()),
                now_ms: Some(2_500),
            })
            .expect("mark running")
            .expect("running snapshot");
        assert_eq!(running.work_unit.status, WorkUnitStatus::Running);

        let heartbeat = repository
            .heartbeat_lease(WorkUnitHeartbeatRequest {
                work_unit_id: "wu-test".to_owned(),
                owner: "worker-a".to_owned(),
                ttl_ms: 7_000,
                actor: Some("worker-a".to_owned()),
                now_ms: Some(3_000),
            })
            .expect("heartbeat")
            .expect("heartbeat snapshot");
        let heartbeat_lease = heartbeat.lease.expect("lease after heartbeat");
        assert_eq!(heartbeat_lease.expires_at_ms, 10_000);

        let completed = repository
            .complete_work_unit(CompleteWorkUnitRequest {
                work_unit_id: "wu-test".to_owned(),
                owner: "worker-a".to_owned(),
                disposition: WorkUnitCompletionDisposition::Completed,
                actor: Some("worker-a".to_owned()),
                now_ms: Some(4_000),
                next_run_at_ms: None,
                result_payload_json: Some(json!({"summary": "done"})),
                error: None,
            })
            .expect("complete work unit")
            .expect("completed snapshot");
        assert_eq!(completed.work_unit.status, WorkUnitStatus::Completed);
        assert!(completed.lease.is_none());
        assert_eq!(
            completed.work_unit.result_payload_json,
            Some(json!({"summary": "done"}))
        );

        let events = repository
            .list_work_unit_events("wu-test", 10)
            .expect("list events");
        let event_kinds = events
            .iter()
            .map(|event| event.event_kind.as_str())
            .collect::<Vec<_>>();
        assert!(event_kinds.contains(&"work_unit_created"));
        assert!(event_kinds.contains(&"work_unit_leased"));
        assert!(event_kinds.contains(&"work_unit_started"));
        assert!(event_kinds.contains(&"work_unit_heartbeat"));
        assert!(event_kinds.contains(&"work_unit_completed"));
    }

    #[test]
    fn retry_completion_schedules_backoff_and_exhaustion_becomes_failed_terminal() {
        let config = isolated_memory_config("retry-completion");
        let repository = WorkUnitRepository::new(&config).expect("repository");
        repository
            .create_work_unit(sample_work_unit(WorkUnitStatus::Ready), Some("operator"))
            .expect("create work unit");

        let first_lease = repository
            .acquire_next_ready_lease(AcquireWorkUnitLeaseRequest {
                owner: "worker-a".to_owned(),
                ttl_ms: 5_000,
                actor: None,
                now_ms: Some(2_000),
            })
            .expect("first lease")
            .expect("leased snapshot");
        assert_eq!(first_lease.work_unit.attempt_count, 1);

        let first_retry = repository
            .complete_work_unit(CompleteWorkUnitRequest {
                work_unit_id: "wu-test".to_owned(),
                owner: "worker-a".to_owned(),
                disposition: WorkUnitCompletionDisposition::RetryPending,
                actor: Some("worker-a".to_owned()),
                now_ms: Some(4_000),
                next_run_at_ms: None,
                result_payload_json: None,
                error: Some("transient".to_owned()),
            })
            .expect("first retry")
            .expect("retry snapshot");
        assert_eq!(first_retry.work_unit.status, WorkUnitStatus::RetryPending);
        assert_eq!(first_retry.work_unit.next_run_at_ms, 5_000);
        assert_eq!(
            first_retry.work_unit.last_error.as_deref(),
            Some("transient")
        );

        let second_lease = repository
            .acquire_next_ready_lease(AcquireWorkUnitLeaseRequest {
                owner: "worker-b".to_owned(),
                ttl_ms: 5_000,
                actor: None,
                now_ms: Some(5_000),
            })
            .expect("second lease")
            .expect("second leased snapshot");
        assert_eq!(second_lease.work_unit.attempt_count, 2);

        let second_retry = repository
            .complete_work_unit(CompleteWorkUnitRequest {
                work_unit_id: "wu-test".to_owned(),
                owner: "worker-b".to_owned(),
                disposition: WorkUnitCompletionDisposition::RetryPending,
                actor: None,
                now_ms: Some(6_000),
                next_run_at_ms: None,
                result_payload_json: None,
                error: Some("still transient".to_owned()),
            })
            .expect("second retry")
            .expect("second retry snapshot");
        assert_eq!(second_retry.work_unit.next_run_at_ms, 8_000);

        let third_lease = repository
            .acquire_next_ready_lease(AcquireWorkUnitLeaseRequest {
                owner: "worker-c".to_owned(),
                ttl_ms: 5_000,
                actor: None,
                now_ms: Some(8_000),
            })
            .expect("third lease")
            .expect("third leased snapshot");
        assert_eq!(third_lease.work_unit.attempt_count, 3);

        let exhausted = repository
            .complete_work_unit(CompleteWorkUnitRequest {
                work_unit_id: "wu-test".to_owned(),
                owner: "worker-c".to_owned(),
                disposition: WorkUnitCompletionDisposition::RetryPending,
                actor: None,
                now_ms: Some(9_000),
                next_run_at_ms: None,
                result_payload_json: None,
                error: Some("retry budget exhausted".to_owned()),
            })
            .expect("retry exhaustion")
            .expect("failed terminal snapshot");
        assert_eq!(exhausted.work_unit.status, WorkUnitStatus::FailedTerminal);
    }

    #[test]
    fn recover_expired_leases_moves_units_to_retry_pending_or_failed_terminal() {
        let config = isolated_memory_config("recover-expired");
        let repository = WorkUnitRepository::new(&config).expect("repository");

        let first = NewWorkUnitRecord {
            work_unit_id: Some("wu-first".to_owned()),
            retry_policy: WorkUnitRetryPolicy {
                max_attempts: 3,
                initial_backoff_ms: 1_000,
                max_backoff_ms: 4_000,
            },
            ..sample_work_unit(WorkUnitStatus::Ready)
        };
        let second = NewWorkUnitRecord {
            work_unit_id: Some("wu-second".to_owned()),
            retry_policy: WorkUnitRetryPolicy {
                max_attempts: 1,
                initial_backoff_ms: 1_000,
                max_backoff_ms: 4_000,
            },
            ..sample_work_unit(WorkUnitStatus::Ready)
        };

        repository
            .create_work_unit(first, None)
            .expect("create first work unit");
        repository
            .create_work_unit(second, None)
            .expect("create second work unit");

        repository
            .acquire_next_ready_lease(AcquireWorkUnitLeaseRequest {
                owner: "worker-a".to_owned(),
                ttl_ms: 2_000,
                actor: None,
                now_ms: Some(1_000),
            })
            .expect("lease first work unit")
            .expect("first leased");
        repository
            .acquire_next_ready_lease(AcquireWorkUnitLeaseRequest {
                owner: "worker-b".to_owned(),
                ttl_ms: 2_000,
                actor: None,
                now_ms: Some(1_100),
            })
            .expect("lease second work unit")
            .expect("second leased");

        let recovered = repository
            .recover_expired_leases(Some("recovery-scan"), Some(5_000))
            .expect("recover expired leases");
        assert_eq!(recovered.len(), 2);

        let first_snapshot = repository
            .load_work_unit_snapshot("wu-first")
            .expect("load first snapshot")
            .expect("first snapshot");
        assert_eq!(
            first_snapshot.work_unit.status,
            WorkUnitStatus::RetryPending
        );
        assert_eq!(first_snapshot.work_unit.next_run_at_ms, 6_000);

        let second_snapshot = repository
            .load_work_unit_snapshot("wu-second")
            .expect("load second snapshot")
            .expect("second snapshot");
        assert_eq!(
            second_snapshot.work_unit.status,
            WorkUnitStatus::FailedTerminal
        );
    }

    #[test]
    fn archive_work_unit_requires_terminal_status() {
        let config = isolated_memory_config("archive-work-unit");
        let repository = WorkUnitRepository::new(&config).expect("repository");
        repository
            .create_work_unit(sample_work_unit(WorkUnitStatus::Ready), None)
            .expect("create work unit");

        let archived_before_terminal = repository
            .archive_work_unit(ArchiveWorkUnitRequest {
                work_unit_id: "wu-test".to_owned(),
                actor: Some("operator".to_owned()),
                now_ms: Some(2_000),
            })
            .expect("archive before terminal should not error");
        assert!(archived_before_terminal.is_none());

        repository
            .acquire_next_ready_lease(AcquireWorkUnitLeaseRequest {
                owner: "worker-a".to_owned(),
                ttl_ms: 5_000,
                actor: None,
                now_ms: Some(2_100),
            })
            .expect("lease for archive flow")
            .expect("leased snapshot");
        repository
            .complete_work_unit(CompleteWorkUnitRequest {
                work_unit_id: "wu-test".to_owned(),
                owner: "worker-a".to_owned(),
                disposition: WorkUnitCompletionDisposition::Cancelled,
                actor: None,
                now_ms: Some(2_200),
                next_run_at_ms: None,
                result_payload_json: None,
                error: Some("operator cancelled".to_owned()),
            })
            .expect("cancel work unit")
            .expect("cancelled snapshot");

        let archived = repository
            .archive_work_unit(ArchiveWorkUnitRequest {
                work_unit_id: "wu-test".to_owned(),
                actor: Some("operator".to_owned()),
                now_ms: Some(2_300),
            })
            .expect("archive terminal work unit")
            .expect("archived snapshot");
        assert_eq!(archived.work_unit.status, WorkUnitStatus::Archived);
        assert_eq!(archived.work_unit.archived_at_ms, Some(2_300));
    }

    #[test]
    fn runtime_health_reports_counts_and_expired_leases() {
        let config = isolated_memory_config("runtime-health");
        let repository = WorkUnitRepository::new(&config).expect("repository");

        let ready = NewWorkUnitRecord {
            work_unit_id: Some("wu-ready".to_owned()),
            ..sample_work_unit(WorkUnitStatus::Ready)
        };
        let blocked = NewWorkUnitRecord {
            work_unit_id: Some("wu-blocked".to_owned()),
            ..sample_work_unit(WorkUnitStatus::WaitingReview)
        };

        repository
            .create_work_unit(ready, None)
            .expect("create ready");
        repository
            .create_work_unit(blocked, None)
            .expect("create blocked");

        repository
            .acquire_next_ready_lease(AcquireWorkUnitLeaseRequest {
                owner: "worker-a".to_owned(),
                ttl_ms: 1_000,
                actor: None,
                now_ms: Some(1_000),
            })
            .expect("lease ready")
            .expect("leased snapshot");

        let health = repository
            .load_runtime_health(Some(5_000))
            .expect("load runtime health");
        assert_eq!(health.total_count, 2);
        assert_eq!(health.ready_count, 0);
        assert_eq!(health.leased_count, 1);
        assert_eq!(health.blocked_count, 1);
        assert_eq!(health.expired_lease_count, 1);
    }

    #[test]
    fn list_work_units_filters_archived_entries_by_default() {
        let config = isolated_memory_config("list-filter");
        let repository = WorkUnitRepository::new(&config).expect("repository");

        let active = NewWorkUnitRecord {
            work_unit_id: Some("wu-active".to_owned()),
            ..sample_work_unit(WorkUnitStatus::Ready)
        };
        let archived = NewWorkUnitRecord {
            work_unit_id: Some("wu-archived".to_owned()),
            ..sample_work_unit(WorkUnitStatus::Ready)
        };

        repository
            .create_work_unit(active, None)
            .expect("create active");
        repository
            .create_work_unit(archived, None)
            .expect("create archived candidate");
        repository
            .acquire_next_ready_lease(AcquireWorkUnitLeaseRequest {
                owner: "worker-a".to_owned(),
                ttl_ms: 5_000,
                actor: None,
                now_ms: Some(1_000),
            })
            .expect("lease active")
            .expect("leased active");
        repository
            .complete_work_unit(CompleteWorkUnitRequest {
                work_unit_id: "wu-active".to_owned(),
                owner: "worker-a".to_owned(),
                disposition: WorkUnitCompletionDisposition::Completed,
                actor: None,
                now_ms: Some(2_000),
                next_run_at_ms: None,
                result_payload_json: None,
                error: None,
            })
            .expect("complete active")
            .expect("completed active");
        repository
            .archive_work_unit(ArchiveWorkUnitRequest {
                work_unit_id: "wu-active".to_owned(),
                actor: None,
                now_ms: Some(3_000),
            })
            .expect("archive active")
            .expect("archived active");

        let visible = repository
            .list_work_units(WorkUnitListQuery::default())
            .expect("list visible work units");
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].work_unit.work_unit_id, "wu-archived");

        let with_archived = repository
            .list_work_units(WorkUnitListQuery {
                include_archived: true,
                ..WorkUnitListQuery::default()
            })
            .expect("list work units including archived");
        assert_eq!(with_archived.len(), 2);
    }

    #[test]
    fn assignment_dependency_and_note_actions_round_trip_through_snapshot() {
        let config = isolated_memory_config("assignment-dependency-note");
        let repository = WorkUnitRepository::new(&config).expect("repository");
        let blocker = NewWorkUnitRecord {
            work_unit_id: Some("wu-blocker".to_owned()),
            title: "Blocker".to_owned(),
            description: "Complete prerequisite".to_owned(),
            priority: WorkUnitPriority::Low,
            ..sample_work_unit(WorkUnitStatus::Ready)
        };
        let blocked = sample_work_unit(WorkUnitStatus::Ready);

        repository
            .create_work_unit(blocker, Some("operator"))
            .expect("create blocker work unit");
        repository
            .create_work_unit(blocked, Some("operator"))
            .expect("create blocked work unit");

        let assigned = repository
            .assign_work_unit(AssignWorkUnitRequest {
                work_unit_id: "wu-test".to_owned(),
                assigned_to: Some("designer".to_owned()),
                actor: Some("operator".to_owned()),
                now_ms: Some(1_200),
            })
            .expect("assign work unit")
            .expect("assigned snapshot");
        assert_eq!(assigned.work_unit.assigned_to.as_deref(), Some("designer"));

        let dependency_added = repository
            .add_dependency(AddWorkUnitDependencyRequest {
                blocking_work_unit_id: "wu-blocker".to_owned(),
                blocked_work_unit_id: "wu-test".to_owned(),
                actor: Some("operator".to_owned()),
                now_ms: Some(1_300),
            })
            .expect("add dependency")
            .expect("dependency snapshot");
        assert_eq!(
            dependency_added.work_unit.blocked_by_work_unit_ids,
            vec!["wu-blocker".to_owned()]
        );

        let note = repository
            .append_note(AppendWorkUnitNoteRequest {
                work_unit_id: "wu-test".to_owned(),
                actor: Some("operator".to_owned()),
                note: "needs design review".to_owned(),
                now_ms: Some(1_350),
            })
            .expect("append note")
            .expect("note event");
        assert_eq!(note.event_kind, WORK_UNIT_NOTE_ADDED_EVENT_KIND);

        let leased = repository
            .acquire_next_ready_lease(AcquireWorkUnitLeaseRequest {
                owner: "worker-a".to_owned(),
                ttl_ms: 5_000,
                actor: Some("scheduler".to_owned()),
                now_ms: Some(1_400),
            })
            .expect("acquire lease")
            .expect("leased snapshot");
        assert_eq!(leased.work_unit.work_unit_id, "wu-blocker");

        let dependency_removed = repository
            .remove_dependency(RemoveWorkUnitDependencyRequest {
                blocking_work_unit_id: "wu-blocker".to_owned(),
                blocked_work_unit_id: "wu-test".to_owned(),
                actor: Some("operator".to_owned()),
                now_ms: Some(1_500),
            })
            .expect("remove dependency")
            .expect("dependency removed snapshot");
        assert!(
            dependency_removed
                .work_unit
                .blocked_by_work_unit_ids
                .is_empty()
        );

        let snapshot = repository
            .load_work_unit_snapshot("wu-test")
            .expect("load work unit snapshot")
            .expect("work unit snapshot");
        assert_eq!(snapshot.work_unit.assigned_to.as_deref(), Some("designer"));
        assert!(snapshot.work_unit.blocked_by_work_unit_ids.is_empty());

        let blocker_snapshot = repository
            .load_work_unit_snapshot("wu-blocker")
            .expect("load blocker snapshot")
            .expect("blocker snapshot");
        assert!(
            blocker_snapshot.work_unit.blocks_work_unit_ids.is_empty(),
            "dependency removal should clear blocker-side relation view"
        );

        let events = repository
            .list_work_unit_events("wu-test", 20)
            .expect("list work unit events");
        let event_kinds = events
            .iter()
            .map(|event| event.event_kind.as_str())
            .collect::<Vec<_>>();
        assert!(event_kinds.contains(&WORK_UNIT_ASSIGNED_EVENT_KIND));
        assert!(event_kinds.contains(&WORK_UNIT_DEPENDENCY_ADDED_EVENT_KIND));
        assert!(event_kinds.contains(&WORK_UNIT_DEPENDENCY_REMOVED_EVENT_KIND));
        assert!(event_kinds.contains(&WORK_UNIT_NOTE_ADDED_EVENT_KIND));
    }

    #[test]
    fn dependency_cycle_is_rejected_before_persisting_edge() {
        let config = isolated_memory_config("dependency-cycle");
        let repository = WorkUnitRepository::new(&config).expect("repository");
        let first = NewWorkUnitRecord {
            work_unit_id: Some("wu-first".to_owned()),
            title: "First".to_owned(),
            description: "First work unit".to_owned(),
            ..sample_work_unit(WorkUnitStatus::Ready)
        };
        let second = NewWorkUnitRecord {
            work_unit_id: Some("wu-second".to_owned()),
            title: "Second".to_owned(),
            description: "Second work unit".to_owned(),
            ..sample_work_unit(WorkUnitStatus::Ready)
        };

        repository
            .create_work_unit(first, Some("operator"))
            .expect("create first work unit");
        repository
            .create_work_unit(second, Some("operator"))
            .expect("create second work unit");
        repository
            .add_dependency(AddWorkUnitDependencyRequest {
                blocking_work_unit_id: "wu-first".to_owned(),
                blocked_work_unit_id: "wu-second".to_owned(),
                actor: Some("operator".to_owned()),
                now_ms: Some(1_000),
            })
            .expect("add first dependency");

        let error = repository
            .add_dependency(AddWorkUnitDependencyRequest {
                blocking_work_unit_id: "wu-second".to_owned(),
                blocked_work_unit_id: "wu-first".to_owned(),
                actor: Some("operator".to_owned()),
                now_ms: Some(1_100),
            })
            .expect_err("dependency cycle should be rejected");

        assert!(error.contains("would create a cycle"));
    }
}
