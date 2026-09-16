//! Durable NATS JetStream ingestion for generic email/SMS contact jobs.
//!
//! This module is deliberately product-agnostic. Producers own recipient
//! selection, consent/preferences, subjects, bodies, templates, and campaign
//! semantics. Fanwaave owns only validated provider delivery, retry policy,
//! result publication, and dead-lettering.

use std::env;
use std::error::Error;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_nats::jetstream;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::Semaphore;

use crate::contact::{
    ContactJob, ContactOutcome, ContactOutcomeClass, ContactProviderError, ContactProviderRegistry,
    validate_contact_job,
};
use crate::contracts::TraceMetadata;
use crate::redaction::truncate_utf8;

pub const DEFAULT_CONTACT_JOB_STREAM: &str = "CONTACT_JOBS_V1";
pub const DEFAULT_CONTACT_RESULT_STREAM: &str = "CONTACT_RESULTS_V1";
pub const DEFAULT_CONTACT_DEAD_STREAM: &str = "CONTACT_DEAD_V1";
pub const DEFAULT_CONTACT_JOB_SUBJECT: &str = "contact.jobs.v1";
pub const DEFAULT_CONTACT_RESULT_SUBJECT: &str = "contact.results.v1";
pub const DEFAULT_CONTACT_DEAD_SUBJECT: &str = "contact.dead.v1";
pub const DEFAULT_CONTACT_CONSUMER: &str = "fanwaave-contact-worker-v1";

const ENVELOPE_SCHEMA: &str = "contact.job.envelope.v1";
const RESULT_SCHEMA: &str = "contact.result.v1";
const DEAD_SCHEMA: &str = "contact.dead.v1";
const MAX_SAFE_DETAIL_BYTES: usize = 512;
const DEFAULT_MAX_PAYLOAD_BYTES: usize = 1024 * 1024;

pub type ContactNatsRuntimeError = Box<dyn Error + Send + Sync>;

#[derive(Debug, Clone)]
pub struct ContactNatsConfig {
    pub url: String,
    pub job_stream: String,
    pub result_stream: String,
    pub dead_stream: String,
    pub job_subject: String,
    pub result_subject: String,
    pub dead_subject: String,
    pub consumer: String,
    pub ack_wait: Duration,
    pub nak_delay: Duration,
    pub max_deliver: i64,
    pub max_payload_bytes: usize,
    pub max_concurrency: usize,
    envelope_secret: Option<Arc<[u8]>>,
}

impl ContactNatsConfig {
    /// Contact queue ingestion is opt-in even when the process already uses
    /// `NATS_URL` for push jobs. This lets deployments run API-only, push-only,
    /// contact-only, or combined workers without accidentally provisioning an
    /// unused work queue.
    pub fn from_env() -> Result<Option<Self>, ContactNatsConfigError> {
        if !environment_flag("ENABLE_NATS_CONTACT_INGESTION") {
            return Ok(None);
        }
        let url = non_empty_env("NATS_URL").ok_or(ContactNatsConfigError::MissingNatsUrl)?;
        let envelope_auth_enabled = environment_flag("ENABLE_NATS_ENVELOPE_AUTH");
        let envelope_secret = if envelope_auth_enabled {
            let value = non_empty_env("NATS_SHARED_SECRET")
                .ok_or(ContactNatsConfigError::MissingEnvelopeSecret)?;
            validate_secret(&value)?;
            Some(Arc::from(value.as_bytes()))
        } else {
            None
        };

        Ok(Some(Self {
            url,
            job_stream: env_value("NATS_CONTACT_JOB_STREAM", DEFAULT_CONTACT_JOB_STREAM),
            result_stream: env_value("NATS_CONTACT_RESULT_STREAM", DEFAULT_CONTACT_RESULT_STREAM),
            dead_stream: env_value("NATS_CONTACT_DEAD_STREAM", DEFAULT_CONTACT_DEAD_STREAM),
            job_subject: env_value("NATS_CONTACT_JOB_SUBJECT", DEFAULT_CONTACT_JOB_SUBJECT),
            result_subject: env_value(
                "NATS_CONTACT_RESULT_SUBJECT",
                DEFAULT_CONTACT_RESULT_SUBJECT,
            ),
            dead_subject: env_value("NATS_CONTACT_DEAD_SUBJECT", DEFAULT_CONTACT_DEAD_SUBJECT),
            consumer: env_value("NATS_CONTACT_CONSUMER", DEFAULT_CONTACT_CONSUMER),
            ack_wait: Duration::from_secs(positive_u64(
                "NATS_CONTACT_ACK_WAIT_SECONDS",
                120,
            )?),
            nak_delay: Duration::from_secs(positive_u64(
                "NATS_CONTACT_NAK_DELAY_SECONDS",
                15,
            )?),
            max_deliver: positive_i64("NATS_CONTACT_MAX_DELIVER", 8)?,
            max_payload_bytes: positive_usize(
                "NATS_CONTACT_MAX_PAYLOAD_BYTES",
                DEFAULT_MAX_PAYLOAD_BYTES,
            )?,
            max_concurrency: positive_usize("NATS_CONTACT_MAX_CONCURRENCY", 32)?,
            envelope_secret,
        }))
    }

    fn authenticate(&self, supplied: Option<&str>) -> bool {
        match &self.envelope_secret {
            None => true,
            Some(expected) => supplied
                .map(|value| constant_time_eq(value.as_bytes(), expected))
                .unwrap_or(false),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ContactNatsConfigError {
    #[error("ENABLE_NATS_CONTACT_INGESTION requires NATS_URL")]
    MissingNatsUrl,
    #[error("ENABLE_NATS_ENVELOPE_AUTH is true but NATS_SHARED_SECRET is absent")]
    MissingEnvelopeSecret,
    #[error("NATS_SHARED_SECRET must contain 32 to 4096 non-control characters")]
    InvalidEnvelopeSecret,
    #[error("{0} must be a positive integer")]
    InvalidPositiveInteger(&'static str),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContactJobEnvelopeV1 {
    pub schema: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<String>,
    pub job: ContactJob,
}

impl ContactJobEnvelopeV1 {
    pub fn new(job: ContactJob) -> Self {
        Self {
            schema: ENVELOPE_SCHEMA.to_owned(),
            auth: None,
            job,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ContactResultEventV1 {
    pub schema: &'static str,
    pub outcome: ContactOutcome,
    pub trace: TraceMetadata,
    pub delivery_attempt: u64,
    pub emitted_at_ms: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ContactDeadLetterEventV1 {
    pub schema: &'static str,
    pub reason_code: String,
    pub payload_sha256: String,
    pub payload_bytes: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub job_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tenant_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub application_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outcome: Option<ContactOutcome>,
    pub delivery_attempt: u64,
    pub max_deliver: i64,
    pub emitted_at_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContactNatsDisposition {
    Ack,
    Nak(Duration),
    DeadLetter,
}

pub async fn run_contact_nats_consumer(
    config: ContactNatsConfig,
    registry: ContactProviderRegistry,
) -> Result<(), ContactNatsRuntimeError> {
    let client = async_nats::ConnectOptions::new()
        .name("fanwaave-contact-worker")
        .retry_on_initial_connect()
        .connect(config.url.clone())
        .await?;
    let context = jetstream::new(client);
    ensure_streams(&context, &config).await?;
    let semaphore = Arc::new(Semaphore::new(config.max_concurrency));
    let config = Arc::new(config);

    loop {
        let consumer = build_consumer(&context, &config).await?;
        let mut messages = consumer.messages().await?;
        while let Some(next) = messages.next().await {
            let message = match next {
                Ok(message) => message,
                Err(error) => {
                    tracing::error!(%error, "JetStream contact message fetch failed; recreating consumer stream");
                    break;
                }
            };
            let permit = semaphore.clone().acquire_owned().await?;
            let context = context.clone();
            let config = config.clone();
            let registry = registry.clone();
            tokio::spawn(async move {
                let _permit = permit;
                if let Err(error) = handle_message(&context, &config, &registry, message).await {
                    tracing::error!(%error, "JetStream contact message handling failed");
                }
            });
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

async fn ensure_streams(
    context: &jetstream::Context,
    config: &ContactNatsConfig,
) -> Result<(), ContactNatsRuntimeError> {
    context
        .get_or_create_stream(jetstream::stream::Config {
            name: config.job_stream.clone(),
            subjects: vec![config.job_subject.clone()],
            retention: jetstream::stream::RetentionPolicy::WorkQueue,
            storage: jetstream::stream::StorageType::File,
            max_age: Duration::from_secs(14 * 24 * 60 * 60),
            max_message_size: i32::try_from(config.max_payload_bytes).unwrap_or(i32::MAX),
            ..Default::default()
        })
        .await?;
    context
        .get_or_create_stream(jetstream::stream::Config {
            name: config.result_stream.clone(),
            subjects: vec![config.result_subject.clone()],
            retention: jetstream::stream::RetentionPolicy::Limits,
            storage: jetstream::stream::StorageType::File,
            max_age: Duration::from_secs(14 * 24 * 60 * 60),
            ..Default::default()
        })
        .await?;
    context
        .get_or_create_stream(jetstream::stream::Config {
            name: config.dead_stream.clone(),
            subjects: vec![config.dead_subject.clone()],
            retention: jetstream::stream::RetentionPolicy::Limits,
            storage: jetstream::stream::StorageType::File,
            max_age: Duration::from_secs(30 * 24 * 60 * 60),
            ..Default::default()
        })
        .await?;
    Ok(())
}

async fn build_consumer(
    context: &jetstream::Context,
    config: &ContactNatsConfig,
) -> Result<jetstream::consumer::PullConsumer, ContactNatsRuntimeError> {
    let stream = context.get_stream(&config.job_stream).await?;
    Ok(stream
        .get_or_create_consumer::<jetstream::consumer::pull::Config>(
            &config.consumer,
            jetstream::consumer::pull::Config {
                durable_name: Some(config.consumer.clone()),
                filter_subject: config.job_subject.clone(),
                ack_wait: config.ack_wait,
                max_deliver: config.max_deliver,
                ..Default::default()
            },
        )
        .await?)
}

async fn handle_message(
    context: &jetstream::Context,
    config: &ContactNatsConfig,
    registry: &ContactProviderRegistry,
    message: jetstream::Message,
) -> Result<(), ContactNatsRuntimeError> {
    let payload = message.payload.as_ref();
    let delivery_attempt = message
        .info()
        .ok()
        .and_then(|info| u64::try_from(info.delivered).ok())
        .unwrap_or(1);

    if payload.len() > config.max_payload_bytes {
        let event = dead_letter_for_payload(
            payload,
            "payload_too_large",
            None,
            None,
            delivery_attempt,
            config.max_deliver,
        );
        publish_json(context, &config.dead_subject, &event).await?;
        message.ack_with(jetstream::AckKind::Term).await?;
        return Ok(());
    }

    let envelope = match serde_json::from_slice::<ContactJobEnvelopeV1>(payload) {
        Ok(envelope) => envelope,
        Err(_) => {
            let event = dead_letter_for_payload(
                payload,
                "invalid_envelope_json",
                None,
                None,
                delivery_attempt,
                config.max_deliver,
            );
            publish_json(context, &config.dead_subject, &event).await?;
            message.ack_with(jetstream::AckKind::Term).await?;
            return Ok(());
        }
    };

    if envelope.schema != ENVELOPE_SCHEMA {
        let event = dead_letter_for_payload(
            payload,
            "unsupported_envelope_schema",
            Some(&envelope.job),
            None,
            delivery_attempt,
            config.max_deliver,
        );
        publish_json(context, &config.dead_subject, &event).await?;
        message.ack_with(jetstream::AckKind::Term).await?;
        return Ok(());
    }

    if !config.authenticate(envelope.auth.as_deref()) {
        let event = dead_letter_for_payload(
            payload,
            "envelope_authentication_failed",
            Some(&envelope.job),
            None,
            delivery_attempt,
            config.max_deliver,
        );
        publish_json(context, &config.dead_subject, &event).await?;
        message.ack_with(jetstream::AckKind::Term).await?;
        return Ok(());
    }

    let progress_every = (config.ack_wait / 3).max(Duration::from_secs(5));
    let outcome = run_with_ack_progress(
        &message,
        progress_every,
        process_contact_job(registry, &envelope.job),
    )
    .await;
    let result = ContactResultEventV1 {
        schema: RESULT_SCHEMA,
        outcome: outcome.clone(),
        trace: envelope.job.trace.clone(),
        delivery_attempt,
        emitted_at_ms: now_ms(),
    };

    if let Err(error) = publish_json(context, &config.result_subject, &result).await {
        // A provider acceptance is an irreversible external side effect. If the
        // result bus is unavailable after an accepted/permanent outcome, never
        // redeliver the source message merely to repair observability: doing so
        // can send the same email/SMS twice. Retry only outcomes for which the
        // provider did not accept the delivery.
        match disposition_for_result_publish_failure(
            outcome.class,
            delivery_attempt,
            config.max_deliver,
            config.nak_delay,
        ) {
            ContactNatsDisposition::Ack => message.ack().await?,
            ContactNatsDisposition::Nak(delay) => {
                message
                    .ack_with(jetstream::AckKind::Nak(Some(delay)))
                    .await?
            }
            ContactNatsDisposition::DeadLetter => {
                let event = dead_letter_for_payload(
                    payload,
                    "result_publish_failed_after_terminal_outcome",
                    Some(&envelope.job),
                    Some(outcome.clone()),
                    delivery_attempt,
                    config.max_deliver,
                );
                if let Err(dead_error) = publish_json(context, &config.dead_subject, &event).await {
                    tracing::error!(
                        %dead_error,
                        "contact result and dead-letter publication both failed after terminal provider outcome; terminating source message to prevent duplicate delivery"
                    );
                }
                message.ack_with(jetstream::AckKind::Term).await?;
            }
        }
        return Err(error);
    }

    match disposition_for_outcome(
        outcome.class,
        delivery_attempt,
        config.max_deliver,
        config.nak_delay,
    ) {
        ContactNatsDisposition::Ack => message.ack().await?,
        ContactNatsDisposition::Nak(delay) => {
            message
                .ack_with(jetstream::AckKind::Nak(Some(delay)))
                .await?
        }
        ContactNatsDisposition::DeadLetter => {
            let event = dead_letter_for_payload(
                payload,
                "retry_budget_exhausted",
                Some(&envelope.job),
                Some(outcome),
                delivery_attempt,
                config.max_deliver,
            );
            publish_json(context, &config.dead_subject, &event).await?;
            message.ack_with(jetstream::AckKind::Term).await?;
        }
    }
    Ok(())
}

async fn process_contact_job(
    registry: &ContactProviderRegistry,
    job: &ContactJob,
) -> ContactOutcome {
    if let Err(errors) = validate_contact_job(job) {
        return ContactOutcome {
            version: job.version,
            job_id: job.job_id.clone(),
            provider: job.provider,
            target_fingerprint: job.target.fingerprint(),
            class: ContactOutcomeClass::InvalidPayload,
            provider_code: Some("contract_validation_failed".to_owned()),
            retry_after_ms: None,
            safe_detail: Some(truncate_utf8(
                &errors
                    .into_iter()
                    .map(|error| error.to_string())
                    .collect::<Vec<_>>()
                    .join("; "),
                MAX_SAFE_DETAIL_BYTES,
            )),
        };
    }

    match registry.dispatch(job).await {
        Ok(outcome) => outcome,
        Err(error) => outcome_from_provider_error(job, error),
    }
}

fn outcome_from_provider_error(job: &ContactJob, error: ContactProviderError) -> ContactOutcome {
    let (class, safe_detail, retry_after, provider_code) = match error {
        ContactProviderError::NotConfigured { safe_reason } => (
            ContactOutcomeClass::PermanentProviderFailure,
            safe_reason,
            None,
            Some("provider_not_configured".to_owned()),
        ),
        ContactProviderError::Delivery {
            class,
            safe_detail,
            retry_after,
            provider_code,
        } => (class, safe_detail, retry_after, provider_code),
        ContactProviderError::Internal { safe_detail } => (
            ContactOutcomeClass::InternalFailure,
            safe_detail,
            None,
            Some("internal_provider_error".to_owned()),
        ),
    };
    ContactOutcome {
        version: job.version,
        job_id: job.job_id.clone(),
        provider: job.provider,
        target_fingerprint: job.target.fingerprint(),
        class,
        provider_code,
        retry_after_ms: retry_after.map(duration_to_millis),
        safe_detail: Some(truncate_utf8(&safe_detail, MAX_SAFE_DETAIL_BYTES)),
    }
}

pub fn disposition_for_outcome(
    class: ContactOutcomeClass,
    delivery_attempt: u64,
    max_deliver: i64,
    nak_delay: Duration,
) -> ContactNatsDisposition {
    if !class.is_retryable() {
        return ContactNatsDisposition::Ack;
    }
    if delivery_attempt >= u64::try_from(max_deliver).unwrap_or(u64::MAX) {
        return ContactNatsDisposition::DeadLetter;
    }
    ContactNatsDisposition::Nak(nak_delay)
}

fn disposition_for_result_publish_failure(
    class: ContactOutcomeClass,
    delivery_attempt: u64,
    max_deliver: i64,
    nak_delay: Duration,
) -> ContactNatsDisposition {
    if class == ContactOutcomeClass::Accepted || !class.is_retryable() {
        return ContactNatsDisposition::DeadLetter;
    }
    disposition_for_outcome(class, delivery_attempt, max_deliver, nak_delay)
}

fn dead_letter_for_payload(
    payload: &[u8],
    reason_code: &str,
    job: Option<&ContactJob>,
    outcome: Option<ContactOutcome>,
    delivery_attempt: u64,
    max_deliver: i64,
) -> ContactDeadLetterEventV1 {
    ContactDeadLetterEventV1 {
        schema: DEAD_SCHEMA,
        reason_code: reason_code.to_owned(),
        payload_sha256: hex::encode(Sha256::digest(payload)),
        payload_bytes: payload.len(),
        job_id: job.map(|job| job.job_id.clone()),
        tenant_id: job.map(|job| job.tenant_id.clone()),
        application_id: job.map(|job| job.application_id.clone()),
        outcome,
        delivery_attempt,
        max_deliver,
        emitted_at_ms: now_ms(),
    }
}

async fn publish_json<T: Serialize>(
    context: &jetstream::Context,
    subject: &str,
    event: &T,
) -> Result<(), ContactNatsRuntimeError> {
    let payload = serde_json::to_vec(event)?;
    context.publish(subject.to_owned(), payload.into()).await?.await?;
    Ok(())
}

async fn run_with_ack_progress<F, T>(
    message: &jetstream::Message,
    interval: Duration,
    future: F,
) -> T
where
    F: std::future::Future<Output = T>,
{
    tokio::pin!(future);
    let mut ticker = tokio::time::interval(interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let _ = ticker.tick().await;
    loop {
        tokio::select! {
            result = &mut future => return result,
            _ = ticker.tick() => {
                if let Err(error) = message.ack_with(jetstream::AckKind::Progress).await {
                    tracing::warn!(%error, "failed to extend JetStream contact ack deadline");
                }
            }
        }
    }
}

fn duration_to_millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| u64::try_from(value.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or_default()
}

fn env_value(name: &str, default: &str) -> String {
    non_empty_env(name).unwrap_or_else(|| default.to_owned())
}

fn non_empty_env(name: &str) -> Option<String> {
    env::var(name)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

fn positive_u64(name: &'static str, default: u64) -> Result<u64, ContactNatsConfigError> {
    match non_empty_env(name) {
        None => Ok(default),
        Some(value) => value
            .parse::<u64>()
            .ok()
            .filter(|value| *value > 0)
            .ok_or(ContactNatsConfigError::InvalidPositiveInteger(name)),
    }
}

fn positive_i64(name: &'static str, default: i64) -> Result<i64, ContactNatsConfigError> {
    match non_empty_env(name) {
        None => Ok(default),
        Some(value) => value
            .parse::<i64>()
            .ok()
            .filter(|value| *value > 0)
            .ok_or(ContactNatsConfigError::InvalidPositiveInteger(name)),
    }
}

fn positive_usize(name: &'static str, default: usize) -> Result<usize, ContactNatsConfigError> {
    match non_empty_env(name) {
        None => Ok(default),
        Some(value) => value
            .parse::<usize>()
            .ok()
            .filter(|value| *value > 0)
            .ok_or(ContactNatsConfigError::InvalidPositiveInteger(name)),
    }
}

fn environment_flag(name: &str) -> bool {
    env::var(name).ok().is_some_and(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

fn validate_secret(value: &str) -> Result<(), ContactNatsConfigError> {
    if !(32..=4096).contains(&value.len()) || value.chars().any(char::is_control) {
        return Err(ContactNatsConfigError::InvalidEnvelopeSecret);
    }
    Ok(())
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right.iter())
        .fold(0_u8, |diff, (left, right)| diff | (left ^ right))
        == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_contact_outcomes_are_acked() {
        let delay = Duration::from_secs(10);
        assert_eq!(
            disposition_for_outcome(ContactOutcomeClass::Accepted, 1, 5, delay),
            ContactNatsDisposition::Ack
        );
        assert_eq!(
            disposition_for_outcome(ContactOutcomeClass::InvalidPayload, 1, 5, delay),
            ContactNatsDisposition::Ack
        );
        assert_eq!(
            disposition_for_outcome(ContactOutcomeClass::PermanentProviderFailure, 1, 5, delay),
            ContactNatsDisposition::Ack
        );
    }

    #[test]
    fn retryable_contact_outcomes_nak_then_dead_letter() {
        let delay = Duration::from_secs(10);
        assert_eq!(
            disposition_for_outcome(ContactOutcomeClass::Throttled, 2, 5, delay),
            ContactNatsDisposition::Nak(delay)
        );
        assert_eq!(
            disposition_for_outcome(ContactOutcomeClass::TransientProviderFailure, 5, 5, delay),
            ContactNatsDisposition::DeadLetter
        );
    }

    #[test]
    fn accepted_delivery_is_never_replayed_to_repair_result_publication() {
        assert_eq!(
            disposition_for_result_publish_failure(
                ContactOutcomeClass::Accepted,
                1,
                5,
                Duration::from_secs(10)
            ),
            ContactNatsDisposition::DeadLetter
        );
    }

    #[test]
    fn constant_time_secret_comparison_rejects_mismatches() {
        assert!(constant_time_eq(b"same", b"same"));
        assert!(!constant_time_eq(b"same", b"diff"));
        assert!(!constant_time_eq(b"short", b"longer"));
    }
}
