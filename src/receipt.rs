use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use utoipa::ToSchema;

use crate::contracts::{ContractVersion, ProviderKind, PushJob};
use crate::redaction::TargetFingerprint;

const MAX_DELIVERY_ATTEMPTS: u16 = 64;
const MAX_PROVIDER_CODE_BYTES: usize = 64;
const RECEIPT_KEY_PREFIX: &str = "receipt:v1:";

/// Stable, non-reversible key for one tenant/application/idempotency/target scope.
///
/// The producer idempotency key and tenant/application identifiers never enter
/// receipts directly. Length-prefixed hashing avoids delimiter ambiguity.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, ToSchema,
)]
#[serde(transparent)]
pub struct DeliveryReceiptKey(String);

impl DeliveryReceiptKey {
    pub fn for_job(job: &PushJob) -> Self {
        let target = job.target.fingerprint();
        let mut hasher = Sha256::new();
        hasher.update(b"fanwaave-delivery-receipt-v1\0");
        hash_part(&mut hasher, &job.tenant_id);
        hash_part(&mut hasher, &job.application_id);
        hash_part(&mut hasher, &job.idempotency_key);
        hash_part(&mut hasher, job.provider.as_str());
        hash_part(&mut hasher, target.as_str());
        Self(format!("{RECEIPT_KEY_PREFIX}{}", hex::encode(hasher.finalize())))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for DeliveryReceiptKey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

fn hash_part(hasher: &mut Sha256, value: &str) {
    hasher.update(u64::try_from(value.len()).unwrap_or(u64::MAX).to_be_bytes());
    hasher.update(value.as_bytes());
}

/// Exhaustive terminal states promised to producers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryTerminalState {
    Delivered,
    Expired,
    Rejected,
    Canceled,
    PermanentlyFailed,
}

/// Bounded reason taxonomy. Arbitrary provider or exception text is forbidden.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryReceiptReason {
    ProviderConfirmed,
    TimeToLiveElapsed,
    ConsentRevoked,
    OwnershipDenied,
    MissingCredentials,
    InvalidToken,
    InvalidPayload,
    PolicyDenied,
    CanceledByCaller,
    ProviderPermanentFailure,
    RetryBudgetExhausted,
    InternalPermanentFailure,
}

impl DeliveryReceiptReason {
    const fn valid_for(self, state: DeliveryTerminalState) -> bool {
        match state {
            DeliveryTerminalState::Delivered => matches!(self, Self::ProviderConfirmed),
            DeliveryTerminalState::Expired => matches!(self, Self::TimeToLiveElapsed),
            DeliveryTerminalState::Rejected => matches!(
                self,
                Self::ConsentRevoked
                    | Self::OwnershipDenied
                    | Self::MissingCredentials
                    | Self::InvalidToken
                    | Self::InvalidPayload
                    | Self::PolicyDenied
            ),
            DeliveryTerminalState::Canceled => matches!(self, Self::CanceledByCaller),
            DeliveryTerminalState::PermanentlyFailed => matches!(
                self,
                Self::ProviderPermanentFailure
                    | Self::RetryBudgetExhausted
                    | Self::InternalPermanentFailure
            ),
        }
    }

    const fn requires_provider_attempt(self) -> bool {
        matches!(
            self,
            Self::ProviderConfirmed
                | Self::InvalidToken
                | Self::ProviderPermanentFailure
                | Self::RetryBudgetExhausted
                | Self::InternalPermanentFailure
        )
    }
}

/// Redacted, terminal, provider-neutral delivery evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct DeliveryReceipt {
    pub version: ContractVersion,
    pub receipt_key: DeliveryReceiptKey,
    pub job_id: String,
    pub provider: ProviderKind,
    pub target_fingerprint: TargetFingerprint,
    pub state: DeliveryTerminalState,
    pub reason: DeliveryReceiptReason,
    pub attempts: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_code: Option<String>,
}

impl DeliveryReceipt {
    pub fn terminal(
        job: &PushJob,
        state: DeliveryTerminalState,
        reason: DeliveryReceiptReason,
        attempts: u16,
        provider_code: Option<&str>,
    ) -> Result<Self, ReceiptError> {
        if !reason.valid_for(state) {
            return Err(ReceiptError::InconsistentStateReason { state, reason });
        }
        if attempts > MAX_DELIVERY_ATTEMPTS {
            return Err(ReceiptError::TooManyAttempts {
                attempts,
                maximum: MAX_DELIVERY_ATTEMPTS,
            });
        }
        if reason.requires_provider_attempt() && attempts == 0 {
            return Err(ReceiptError::MissingProviderAttempt { reason });
        }
        if provider_code.is_some()
            && !matches!(
                reason,
                DeliveryReceiptReason::ProviderConfirmed
                    | DeliveryReceiptReason::InvalidToken
                    | DeliveryReceiptReason::ProviderPermanentFailure
                    | DeliveryReceiptReason::RetryBudgetExhausted
            )
        {
            return Err(ReceiptError::UnexpectedProviderCode { reason });
        }

        let provider_code = provider_code
            .map(validate_provider_code)
            .transpose()?
            .map(ToOwned::to_owned);

        Ok(Self {
            version: job.version,
            receipt_key: DeliveryReceiptKey::for_job(job),
            job_id: job.job_id.clone(),
            provider: job.provider,
            target_fingerprint: job.target.fingerprint(),
            state,
            reason,
            attempts,
            provider_code,
        })
    }
}

fn validate_provider_code(value: &str) -> Result<&str, ReceiptError> {
    if value.is_empty()
        || value.len() > MAX_PROVIDER_CODE_BYTES
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'-'))
    {
        return Err(ReceiptError::InvalidProviderCode);
    }
    Ok(value)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReceiptRecordResult {
    Recorded,
    AlreadyRecorded,
}

/// In-memory reconciliation authority used by tests and adapters.
///
/// Durable stores should preserve the same insert-once semantics with a unique
/// key. Exact replays are acknowledged; conflicting terminal evidence is never
/// overwritten or silently merged.
#[derive(Debug, Default)]
pub struct DeliveryReceiptLedger {
    receipts: BTreeMap<DeliveryReceiptKey, DeliveryReceipt>,
}

impl DeliveryReceiptLedger {
    pub fn record(
        &mut self,
        receipt: DeliveryReceipt,
    ) -> Result<ReceiptRecordResult, ReceiptError> {
        match self.receipts.get(&receipt.receipt_key) {
            None => {
                self.receipts.insert(receipt.receipt_key.clone(), receipt);
                Ok(ReceiptRecordResult::Recorded)
            }
            Some(existing) if existing == &receipt => Ok(ReceiptRecordResult::AlreadyRecorded),
            Some(_) => Err(ReceiptError::ConflictingTerminalReceipt {
                receipt_key: receipt.receipt_key,
            }),
        }
    }

    pub fn get(&self, key: &DeliveryReceiptKey) -> Option<&DeliveryReceipt> {
        self.receipts.get(key)
    }

    pub fn len(&self) -> usize {
        self.receipts.len()
    }

    pub fn is_empty(&self) -> bool {
        self.receipts.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ReceiptError {
    #[error("terminal state {state:?} is inconsistent with reason {reason:?}")]
    InconsistentStateReason {
        state: DeliveryTerminalState,
        reason: DeliveryReceiptReason,
    },

    #[error("delivery attempt count {attempts} exceeds {maximum}")]
    TooManyAttempts { attempts: u16, maximum: u16 },

    #[error("reason {reason:?} requires at least one provider attempt")]
    MissingProviderAttempt { reason: DeliveryReceiptReason },

    #[error("reason {reason:?} must not include a provider code")]
    UnexpectedProviderCode { reason: DeliveryReceiptReason },

    #[error("provider code must be 1..=64 bytes from [A-Za-z0-9._:-]")]
    InvalidProviderCode,

    #[error("conflicting terminal receipt already exists for {receipt_key}")]
    ConflictingTerminalReceipt { receipt_key: DeliveryReceiptKey },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contracts::{Notification, PushOptions, PushTarget, TraceMetadata};

    fn job(idempotency_key: &str, token: &str) -> PushJob {
        PushJob {
            version: ContractVersion::V1,
            job_id: "job-1875".to_owned(),
            tenant_id: "tenant-fanwaave".to_owned(),
            application_id: "fanwaave-mobile".to_owned(),
            idempotency_key: idempotency_key.to_owned(),
            provider: ProviderKind::Fcm,
            target: PushTarget::Fcm {
                token: token.to_owned(),
            },
            notification: Notification {
                title: Some("private notification title".to_owned()),
                body: Some("private notification body".to_owned()),
                ..Notification::default()
            },
            options: PushOptions::default(),
            trace: TraceMetadata::default(),
        }
    }

    #[test]
    fn terminal_receipt_contains_no_target_content_or_idempotency_material() {
        let job = job("event-secret:user-secret", "very-secret-device-token");
        let receipt = DeliveryReceipt::terminal(
            &job,
            DeliveryTerminalState::Delivered,
            DeliveryReceiptReason::ProviderConfirmed,
            1,
            Some("fcm:accepted"),
        )
        .expect("valid delivered receipt");

        let serialized = serde_json::to_string(&receipt).expect("serialize receipt");
        for forbidden in [
            "very-secret-device-token",
            "event-secret:user-secret",
            "tenant-fanwaave",
            "fanwaave-mobile",
            "private notification title",
            "private notification body",
        ] {
            assert!(!serialized.contains(forbidden), "receipt leaked {forbidden}");
        }
        assert!(serialized.contains("target_fingerprint"));
        assert!(serialized.contains(RECEIPT_KEY_PREFIX));
    }

    #[test]
    fn exact_replay_is_idempotent_and_conflict_never_overwrites() {
        let job = job("event-1:user-1", "device-token-one");
        let delivered = DeliveryReceipt::terminal(
            &job,
            DeliveryTerminalState::Delivered,
            DeliveryReceiptReason::ProviderConfirmed,
            1,
            Some("accepted"),
        )
        .unwrap();
        let failed = DeliveryReceipt::terminal(
            &job,
            DeliveryTerminalState::PermanentlyFailed,
            DeliveryReceiptReason::ProviderPermanentFailure,
            1,
            Some("unavailable"),
        )
        .unwrap();

        let mut ledger = DeliveryReceiptLedger::default();
        assert_eq!(
            ledger.record(delivered.clone()).unwrap(),
            ReceiptRecordResult::Recorded
        );
        assert_eq!(
            ledger.record(delivered.clone()).unwrap(),
            ReceiptRecordResult::AlreadyRecorded
        );
        assert!(matches!(
            ledger.record(failed),
            Err(ReceiptError::ConflictingTerminalReceipt { .. })
        ));
        assert_eq!(ledger.len(), 1);
        assert_eq!(ledger.get(&delivered.receipt_key), Some(&delivered));
    }

    #[test]
    fn receipt_key_changes_across_idempotency_or_target_scope() {
        let first = DeliveryReceiptKey::for_job(&job("event-1", "device-token-one"));
        let replay = DeliveryReceiptKey::for_job(&job("event-1", "device-token-one"));
        let different_event = DeliveryReceiptKey::for_job(&job("event-2", "device-token-one"));
        let different_target = DeliveryReceiptKey::for_job(&job("event-1", "device-token-two"));

        assert_eq!(first, replay);
        assert_ne!(first, different_event);
        assert_ne!(first, different_target);
        assert_eq!(first.as_str().len(), RECEIPT_KEY_PREFIX.len() + 64);
    }

    #[test]
    fn state_reason_and_attempt_invariants_fail_closed() {
        let job = job("event-1", "device-token-one");
        assert!(matches!(
            DeliveryReceipt::terminal(
                &job,
                DeliveryTerminalState::Delivered,
                DeliveryReceiptReason::ConsentRevoked,
                1,
                None,
            ),
            Err(ReceiptError::InconsistentStateReason { .. })
        ));
        assert!(matches!(
            DeliveryReceipt::terminal(
                &job,
                DeliveryTerminalState::Delivered,
                DeliveryReceiptReason::ProviderConfirmed,
                0,
                None,
            ),
            Err(ReceiptError::MissingProviderAttempt { .. })
        ));
        assert!(matches!(
            DeliveryReceipt::terminal(
                &job,
                DeliveryTerminalState::Rejected,
                DeliveryReceiptReason::ConsentRevoked,
                0,
                Some("not-called"),
            ),
            Err(ReceiptError::UnexpectedProviderCode { .. })
        ));
        assert!(matches!(
            DeliveryReceipt::terminal(
                &job,
                DeliveryTerminalState::PermanentlyFailed,
                DeliveryReceiptReason::RetryBudgetExhausted,
                MAX_DELIVERY_ATTEMPTS + 1,
                None,
            ),
            Err(ReceiptError::TooManyAttempts { .. })
        ));
    }

    #[test]
    fn provider_codes_are_bounded_and_cannot_carry_headers_or_urls() {
        let job = job("event-1", "device-token-one");
        for unsafe_code in [
            "Bearer secret-token",
            "https://provider.example/error",
            "invalid token",
            "x".repeat(MAX_PROVIDER_CODE_BYTES + 1).as_str(),
        ] {
            assert!(matches!(
                DeliveryReceipt::terminal(
                    &job,
                    DeliveryTerminalState::PermanentlyFailed,
                    DeliveryReceiptReason::ProviderPermanentFailure,
                    1,
                    Some(unsafe_code),
                ),
                Err(ReceiptError::InvalidProviderCode)
            ));
        }
    }
}
