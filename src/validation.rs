use serde_json::to_vec;
use thiserror::Error;
use url::Url;

use crate::contracts::{PushJob, PushTarget};

const MAX_ID_BYTES: usize = 128;
const MAX_IDEMPOTENCY_KEY_BYTES: usize = 256;
const MAX_TITLE_BYTES: usize = 256;
const MAX_BODY_BYTES: usize = 4_096;
const MAX_IMAGE_URL_BYTES: usize = 2_048;
const MAX_DATA_BYTES: usize = 4_096;
const MAX_COLLAPSE_KEY_BYTES: usize = 128;
const MAX_TRACE_FIELD_BYTES: usize = 512;
const MAX_TTL_SECONDS: u32 = 28 * 24 * 60 * 60;
const MIN_TOKEN_BYTES: usize = 8;
const MAX_TOKEN_BYTES: usize = 4_096;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ValidationError {
    #[error("{field} is required")]
    Required { field: &'static str },

    #[error("{field} exceeds {max_bytes} bytes")]
    TooLong {
        field: &'static str,
        max_bytes: usize,
    },

    #[error("{field} has an invalid length")]
    InvalidLength { field: &'static str },

    #[error("{field} contains invalid characters")]
    InvalidCharacters { field: &'static str },

    #[error("job provider does not match target provider")]
    ProviderTargetMismatch,

    #[error("notification must contain title, body, image_url, or data")]
    EmptyNotification,

    #[error("notification data exceeds {max_bytes} bytes")]
    DataTooLarge { max_bytes: usize },

    #[error("ttl_seconds exceeds {max_seconds}")]
    TtlTooLarge { max_seconds: u32 },

    #[error("web push endpoint must be an https URL using port 443 without credentials")]
    InvalidWebPushEndpoint,

    #[error("web push subscription key material is invalid")]
    InvalidWebPushKeyMaterial,
}

pub fn validate_push_job(job: &PushJob) -> Result<(), Vec<ValidationError>> {
    let errors = identifier_errors("job_id", &job.job_id, MAX_ID_BYTES)
        .into_iter()
        .chain(identifier_errors("tenant_id", &job.tenant_id, MAX_ID_BYTES))
        .chain(identifier_errors(
            "application_id",
            &job.application_id,
            MAX_ID_BYTES,
        ))
        .chain(identifier_errors(
            "idempotency_key",
            &job.idempotency_key,
            MAX_IDEMPOTENCY_KEY_BYTES,
        ))
        .chain(
            (job.provider != job.target.provider())
                .then_some(ValidationError::ProviderTargetMismatch),
        )
        .chain(notification_errors(job))
        .chain(option_errors(job))
        .chain(trace_errors(job))
        .chain(target_errors(&job.target))
        .collect::<Vec<_>>();

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

fn identifier_errors(field: &'static str, value: &str, max_bytes: usize) -> Vec<ValidationError> {
    if value.trim().is_empty() {
        return vec![ValidationError::Required { field }];
    }

    [
        (value.len() > max_bytes).then_some(ValidationError::TooLong { field, max_bytes }),
        value
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
            .then_some(ValidationError::InvalidCharacters { field }),
    ]
    .into_iter()
    .flatten()
    .collect()
}

fn notification_errors(job: &PushJob) -> Vec<ValidationError> {
    let notification = &job.notification;
    let has_visible_or_data_content = notification
        .title
        .as_deref()
        .is_some_and(|value| !value.trim().is_empty())
        || notification
            .body
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty())
        || notification
            .image_url
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty())
        || !notification.data.is_empty();

    [
        (!has_visible_or_data_content).then_some(ValidationError::EmptyNotification),
        optional_length_error(
            "notification.title",
            notification.title.as_deref(),
            MAX_TITLE_BYTES,
        ),
        optional_length_error(
            "notification.body",
            notification.body.as_deref(),
            MAX_BODY_BYTES,
        ),
        optional_length_error(
            "notification.image_url",
            notification.image_url.as_deref(),
            MAX_IMAGE_URL_BYTES,
        ),
        to_vec(&notification.data)
            .map(|encoded| encoded.len() > MAX_DATA_BYTES)
            .unwrap_or(true)
            .then_some(ValidationError::DataTooLarge {
                max_bytes: MAX_DATA_BYTES,
            }),
    ]
    .into_iter()
    .flatten()
    .collect()
}

fn option_errors(job: &PushJob) -> Vec<ValidationError> {
    [
        job.options
            .ttl_seconds
            .is_some_and(|ttl| ttl > MAX_TTL_SECONDS)
            .then_some(ValidationError::TtlTooLarge {
                max_seconds: MAX_TTL_SECONDS,
            }),
        optional_length_error(
            "options.collapse_key",
            job.options.collapse_key.as_deref(),
            MAX_COLLAPSE_KEY_BYTES,
        ),
    ]
    .into_iter()
    .flatten()
    .collect()
}

fn trace_errors(job: &PushJob) -> Vec<ValidationError> {
    [
        optional_length_error(
            "trace.traceparent",
            job.trace.traceparent.as_deref(),
            MAX_TRACE_FIELD_BYTES,
        ),
        optional_length_error(
            "trace.tracestate",
            job.trace.tracestate.as_deref(),
            MAX_TRACE_FIELD_BYTES,
        ),
        optional_length_error(
            "trace.correlation_id",
            job.trace.correlation_id.as_deref(),
            MAX_TRACE_FIELD_BYTES,
        ),
    ]
    .into_iter()
    .flatten()
    .collect()
}

fn optional_length_error(
    field: &'static str,
    value: Option<&str>,
    max_bytes: usize,
) -> Option<ValidationError> {
    value
        .is_some_and(|value| value.len() > max_bytes)
        .then_some(ValidationError::TooLong { field, max_bytes })
}

fn target_errors(target: &PushTarget) -> Vec<ValidationError> {
    match target {
        PushTarget::Fcm { token } => token_errors("target.fcm.token", token),
        PushTarget::Apns { token, .. } => token_errors("target.apns.token", token),
        PushTarget::Expo { token } => token_errors("target.expo.token", token),
        PushTarget::WebPush {
            endpoint,
            p256dh,
            auth,
        } => [
            web_push_endpoint_error(endpoint),
            (!valid_subscription_key(p256dh) || !valid_subscription_key(auth))
                .then_some(ValidationError::InvalidWebPushKeyMaterial),
        ]
        .into_iter()
        .flatten()
        .collect(),
    }
}

fn token_errors(field: &'static str, token: &str) -> Vec<ValidationError> {
    if token.len() < MIN_TOKEN_BYTES || token.len() > MAX_TOKEN_BYTES {
        return vec![ValidationError::InvalidLength { field }];
    }

    token
        .chars()
        .any(|character| {
            character.is_control()
                || character.is_whitespace()
                || matches!(character, '/' | '\\' | '?' | '#' | '&')
        })
        .then_some(ValidationError::InvalidCharacters { field })
        .into_iter()
        .collect()
}

fn web_push_endpoint_error(endpoint: &str) -> Option<ValidationError> {
    let valid = Url::parse(endpoint).ok().is_some_and(|url| {
        url.scheme() == "https"
            && url.host_str().is_some()
            && url.port().is_none_or(|port| port == 443)
            && url.username().is_empty()
            && url.password().is_none()
    });

    (!valid).then_some(ValidationError::InvalidWebPushEndpoint)
}

fn valid_subscription_key(value: &str) -> bool {
    (8..=512).contains(&value.len())
        && !value
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use serde_json::json;

    use super::*;
    use crate::contracts::{
        ContractVersion, Notification, ProviderEnvironment, ProviderKind, PushOptions,
        PushPriority, TraceMetadata,
    };

    fn valid_job(target: PushTarget) -> PushJob {
        PushJob {
            version: ContractVersion::V1,
            job_id: "job-1".to_owned(),
            tenant_id: "tenant-1".to_owned(),
            application_id: "app-1".to_owned(),
            idempotency_key: "order-1:user-1".to_owned(),
            provider: target.provider(),
            target,
            notification: Notification {
                title: Some("Order shipped".to_owned()),
                body: Some("Your package is on its way".to_owned()),
                image_url: None,
                data: BTreeMap::from([("order_id".to_owned(), json!("order-1"))]),
            },
            options: PushOptions {
                ttl_seconds: Some(3_600),
                priority: PushPriority::High,
                collapse_key: Some("order-status".to_owned()),
                dry_run: false,
            },
            trace: TraceMetadata {
                correlation_id: Some("request-1".to_owned()),
                ..TraceMetadata::default()
            },
        }
    }

    #[test]
    fn accepts_valid_jobs_for_each_provider() {
        let targets = [
            PushTarget::Fcm {
                token: "fcm:token_123456".to_owned(),
            },
            PushTarget::Apns {
                token: "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
                    .to_owned(),
                environment: ProviderEnvironment::Sandbox,
            },
            PushTarget::Expo {
                token: "ExponentPushToken[abcdefghijk]".to_owned(),
            },
            PushTarget::WebPush {
                endpoint: "https://fcm.googleapis.com/fcm/send/capability".to_owned(),
                p256dh: "p256dh-value".to_owned(),
                auth: "auth-value".to_owned(),
            },
        ];

        for target in targets {
            validate_push_job(&valid_job(target)).expect("valid job");
        }
    }

    #[test]
    fn rejects_provider_target_mismatch() {
        let mut job = valid_job(PushTarget::Fcm {
            token: "fcm:token_123456".to_owned(),
        });
        job.provider = ProviderKind::Apns;

        let errors = validate_push_job(&job).expect_err("mismatch must fail");
        assert!(errors.contains(&ValidationError::ProviderTargetMismatch));
    }

    #[test]
    fn rejects_empty_notification_and_oversized_ttl() {
        let mut job = valid_job(PushTarget::Expo {
            token: "ExponentPushToken[abcdefghijk]".to_owned(),
        });
        job.notification = Notification::default();
        job.options.ttl_seconds = Some(MAX_TTL_SECONDS + 1);

        let errors = validate_push_job(&job).expect_err("invalid job");
        assert!(errors.contains(&ValidationError::EmptyNotification));
        assert!(errors.contains(&ValidationError::TtlTooLarge {
            max_seconds: MAX_TTL_SECONDS
        }));
    }

    #[test]
    fn rejects_unsafe_web_push_endpoints() {
        for endpoint in [
            "http://push.example.test/send/abc",
            "https://user:pass@push.example.test/send/abc",
            "https://push.example.test:8443/send/abc",
        ] {
            let job = valid_job(PushTarget::WebPush {
                endpoint: endpoint.to_owned(),
                p256dh: "p256dh-value".to_owned(),
                auth: "auth-value".to_owned(),
            });

            let errors = validate_push_job(&job).expect_err("unsafe endpoint");
            assert!(errors.contains(&ValidationError::InvalidWebPushEndpoint));
        }
    }

    #[test]
    fn rejects_capability_tokens_with_url_metacharacters() {
        let job = valid_job(PushTarget::Fcm {
            token: "https://internal.example/token".to_owned(),
        });

        let errors = validate_push_job(&job).expect_err("url-like token must fail");
        assert!(errors.contains(&ValidationError::InvalidCharacters {
            field: "target.fcm.token"
        }));
    }
}
