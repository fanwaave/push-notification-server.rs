use serde_json::to_vec;
use thiserror::Error;

use super::contracts::{ContactContent, ContactJob, ContactTarget};

const MAX_ID_BYTES: usize = 128;
const MAX_IDEMPOTENCY_KEY_BYTES: usize = 256;
const MAX_EMAIL_ADDRESS_BYTES: usize = 254;
const MAX_EMAIL_NAME_BYTES: usize = 256;
const MAX_EMAIL_SUBJECT_BYTES: usize = 998;
const MAX_EMAIL_BODY_BYTES: usize = 512 * 1024;
const MAX_TEMPLATE_ID_BYTES: usize = 128;
const MAX_TEMPLATE_DATA_BYTES: usize = 64 * 1024;
const MAX_SMS_CHARACTERS: usize = 1_600;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ContactValidationError {
    #[error("{field} is required")]
    Required { field: &'static str },

    #[error("{field} exceeds {max_bytes} bytes")]
    TooLong {
        field: &'static str,
        max_bytes: usize,
    },

    #[error("{field} contains invalid characters")]
    InvalidCharacters { field: &'static str },

    #[error("provider, target, and content channels must match")]
    ProviderChannelMismatch,

    #[error("email address has an invalid shape")]
    InvalidEmailAddress,

    #[error("email content must use either a dynamic template or explicit subject/body content")]
    InvalidEmailContentMode,

    #[error(
        "dynamic template IDs must start with 'd-' and contain only ASCII letters, digits, or '-'"
    )]
    InvalidTemplateId,

    #[error("dynamic template data requires template_id")]
    TemplateDataWithoutTemplate,

    #[error("dynamic template data exceeds {max_bytes} bytes")]
    TemplateDataTooLarge { max_bytes: usize },

    #[error("SMS target must be an E.164 number")]
    InvalidE164,

    #[error("SMS body exceeds {max_characters} characters")]
    SmsTooLong { max_characters: usize },
}

pub fn validate_contact_job(job: &ContactJob) -> Result<(), Vec<ContactValidationError>> {
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
            (job.provider != job.target.provider() || job.provider != job.content.provider())
                .then_some(ContactValidationError::ProviderChannelMismatch),
        )
        .chain(channel_errors(&job.target, &job.content))
        .collect::<Vec<_>>();

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

/// Channel-specific issues for one target/content pair, in report order.
fn channel_errors(target: &ContactTarget, content: &ContactContent) -> Vec<ContactValidationError> {
    match (target, content) {
        (
            ContactTarget::Email { address, name },
            ContactContent::Email {
                subject,
                text,
                html,
                template_id,
                dynamic_template_data,
                reply_to,
            },
        ) => [
            (!valid_email_address(address)).then_some(ContactValidationError::InvalidEmailAddress),
            optional_length_error("target.email.name", name.as_deref(), MAX_EMAIL_NAME_BYTES),
            name.as_deref()
                .is_some_and(contains_control_characters)
                .then_some(ContactValidationError::InvalidCharacters {
                    field: "target.email.name",
                }),
            reply_to
                .as_deref()
                .is_some_and(|reply_to| !valid_email_address(reply_to))
                .then_some(ContactValidationError::InvalidEmailAddress),
        ]
        .into_iter()
        .flatten()
        .chain(email_content_errors(
            subject.as_deref(),
            text.as_deref(),
            html.as_deref(),
            template_id.as_deref(),
            dynamic_template_data,
        ))
        .collect(),
        (ContactTarget::Sms { e164 }, ContactContent::Sms { body }) => [
            (!valid_e164(e164)).then_some(ContactValidationError::InvalidE164),
            body.trim()
                .is_empty()
                .then_some(ContactValidationError::Required {
                    field: "content.body",
                }),
            (body.chars().count() > MAX_SMS_CHARACTERS).then_some(
                ContactValidationError::SmsTooLong {
                    max_characters: MAX_SMS_CHARACTERS,
                },
            ),
            body.chars().any(|character| character == '\0').then_some(
                ContactValidationError::InvalidCharacters {
                    field: "content.body",
                },
            ),
        ]
        .into_iter()
        .flatten()
        .collect(),
        _ => vec![ContactValidationError::ProviderChannelMismatch],
    }
}

fn email_content_errors(
    subject: Option<&str>,
    text: Option<&str>,
    html: Option<&str>,
    template_id: Option<&str>,
    dynamic_template_data: &std::collections::BTreeMap<String, serde_json::Value>,
) -> Vec<ContactValidationError> {
    let has_subject = subject.is_some_and(|value| !value.trim().is_empty());
    let has_text = text.is_some_and(|value| !value.trim().is_empty());
    let has_html = html.is_some_and(|value| !value.trim().is_empty());

    // Provider request construction selects template mode whenever the field is
    // present. Validate that same state so blank IDs cannot enter explicit mode
    // here and then become template requests at the delivery boundary.
    let mode_errors = match template_id {
        Some(template_id) => [
            (has_subject || has_text || has_html)
                .then_some(ContactValidationError::InvalidEmailContentMode),
            (!valid_template_id(template_id)).then_some(ContactValidationError::InvalidTemplateId),
        ],
        None => [
            (!has_subject || (!has_text && !has_html))
                .then_some(ContactValidationError::InvalidEmailContentMode),
            (!dynamic_template_data.is_empty())
                .then_some(ContactValidationError::TemplateDataWithoutTemplate),
        ],
    };

    mode_errors
        .into_iter()
        .chain([
            optional_length_error("content.subject", subject, MAX_EMAIL_SUBJECT_BYTES),
            subject.is_some_and(contains_control_characters).then_some(
                ContactValidationError::InvalidCharacters {
                    field: "content.subject",
                },
            ),
            optional_length_error("content.text", text, MAX_EMAIL_BODY_BYTES),
            optional_length_error("content.html", html, MAX_EMAIL_BODY_BYTES),
            to_vec(dynamic_template_data)
                .map(|encoded| encoded.len() > MAX_TEMPLATE_DATA_BYTES)
                .unwrap_or(true)
                .then_some(ContactValidationError::TemplateDataTooLarge {
                    max_bytes: MAX_TEMPLATE_DATA_BYTES,
                }),
        ])
        .flatten()
        .collect()
}

fn valid_template_id(template_id: &str) -> bool {
    template_id.len() <= MAX_TEMPLATE_ID_BYTES
        && template_id.starts_with("d-")
        && template_id
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '-')
}

fn identifier_errors(
    field: &'static str,
    value: &str,
    max_bytes: usize,
) -> Vec<ContactValidationError> {
    if value.is_empty() {
        return vec![ContactValidationError::Required { field }];
    }
    [
        (value.len() > max_bytes).then_some(ContactValidationError::TooLong { field, max_bytes }),
        value
            .chars()
            .any(|character| {
                !(character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.' | ':'))
            })
            .then_some(ContactValidationError::InvalidCharacters { field }),
    ]
    .into_iter()
    .flatten()
    .collect()
}

// Subjects and display names are header material at the provider; CR/LF or any
// other control character must never pass through, matching the legacy
// dd-email-sms-contact-rs boundary.
fn contains_control_characters(value: &str) -> bool {
    value.chars().any(char::is_control)
}

fn optional_length_error(
    field: &'static str,
    value: Option<&str>,
    max_bytes: usize,
) -> Option<ContactValidationError> {
    value
        .is_some_and(|value| value.len() > max_bytes)
        .then_some(ContactValidationError::TooLong { field, max_bytes })
}

pub fn valid_email_address(value: &str) -> bool {
    if value.len() < 3 || value.len() > MAX_EMAIL_ADDRESS_BYTES || !value.is_ascii() {
        return false;
    }
    if value
        .chars()
        .any(|character| character.is_control() || character.is_whitespace())
    {
        return false;
    }
    let Some((local, domain)) = value.split_once('@') else {
        return false;
    };
    if domain.contains('@')
        || local.is_empty()
        || local.len() > 64
        || local.starts_with('.')
        || local.ends_with('.')
        || local.contains("..")
        || domain.is_empty()
        || domain.starts_with('.')
        || domain.ends_with('.')
        || domain.contains("..")
    {
        return false;
    }
    if local.chars().any(|character| {
        !(character.is_ascii_alphanumeric()
            || matches!(
                character,
                '!' | '#'
                    | '$'
                    | '%'
                    | '&'
                    | '\''
                    | '*'
                    | '+'
                    | '-'
                    | '/'
                    | '='
                    | '?'
                    | '^'
                    | '_'
                    | '`'
                    | '{'
                    | '|'
                    | '}'
                    | '~'
                    | '.'
            ))
    }) {
        return false;
    }
    domain.split('.').all(|label| {
        !label.is_empty()
            && !label.starts_with('-')
            && !label.ends_with('-')
            && label
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || character == '-')
    })
}

pub fn valid_e164(value: &str) -> bool {
    let Some(digits) = value.strip_prefix('+') else {
        return false;
    };
    (8..=15).contains(&digits.len())
        && !digits.starts_with('0')
        && digits.chars().all(|character| character.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::contact::contracts::{ContactProviderKind, ContactTarget};
    use crate::contracts::{ContractVersion, TraceMetadata};

    fn email_job(content: ContactContent) -> ContactJob {
        ContactJob {
            version: ContractVersion::V1,
            job_id: "job-1".to_owned(),
            tenant_id: "tenant-1".to_owned(),
            application_id: "app-1".to_owned(),
            idempotency_key: "event-1".to_owned(),
            provider: ContactProviderKind::Sendgrid,
            target: ContactTarget::Email {
                address: "person@example.com".to_owned(),
                name: Some("Person".to_owned()),
            },
            content,
            trace: TraceMetadata::default(),
        }
    }

    #[test]
    fn accepts_explicit_and_template_email_modes() {
        validate_contact_job(&email_job(ContactContent::Email {
            subject: Some("Hello".to_owned()),
            text: Some("Hello".to_owned()),
            html: None,
            template_id: None,
            dynamic_template_data: BTreeMap::new(),
            reply_to: Some("support@example.com".to_owned()),
        }))
        .expect("explicit email");
        validate_contact_job(&email_job(ContactContent::Email {
            subject: None,
            text: None,
            html: None,
            template_id: Some("d-0123456789abcdef".to_owned()),
            dynamic_template_data: BTreeMap::from([(
                "name".to_owned(),
                serde_json::json!("Person"),
            )]),
            reply_to: None,
        }))
        .expect("template email");
    }

    #[test]
    fn rejects_present_but_blank_template_ids_before_provider_dispatch() {
        for template_id in ["", " ", "\t"] {
            let errors = validate_contact_job(&email_job(ContactContent::Email {
                subject: Some("Hello".to_owned()),
                text: Some("Body".to_owned()),
                html: None,
                template_id: Some(template_id.to_owned()),
                dynamic_template_data: BTreeMap::new(),
                reply_to: None,
            }))
            .expect_err("a present template ID must satisfy the template contract");
            assert!(errors.contains(&ContactValidationError::InvalidTemplateId));
        }
    }

    #[test]
    fn rejects_ambiguous_email_modes_and_bad_targets() {
        let errors = validate_contact_job(&email_job(ContactContent::Email {
            subject: Some("ignored".to_owned()),
            text: None,
            html: None,
            template_id: Some("d-template".to_owned()),
            dynamic_template_data: BTreeMap::new(),
            reply_to: None,
        }))
        .expect_err("ambiguous mode");
        assert!(errors.contains(&ContactValidationError::InvalidEmailContentMode));
        assert!(!valid_email_address("person @example.com"));
        assert!(!valid_e164("5551234567"));
    }

    #[test]
    fn rejects_control_characters_in_subject_and_display_name() {
        let errors = validate_contact_job(&email_job(ContactContent::Email {
            subject: Some("Hello\r\nBcc: attacker@example.invalid".to_owned()),
            text: Some("Body".to_owned()),
            html: None,
            template_id: None,
            dynamic_template_data: BTreeMap::new(),
            reply_to: None,
        }))
        .expect_err("subject with CRLF");
        assert!(errors.contains(&ContactValidationError::InvalidCharacters {
            field: "content.subject",
        }));

        let mut job = email_job(ContactContent::Email {
            subject: Some("Hello".to_owned()),
            text: Some("Body".to_owned()),
            html: None,
            template_id: None,
            dynamic_template_data: BTreeMap::new(),
            reply_to: None,
        });
        job.target = ContactTarget::Email {
            address: "person@example.com".to_owned(),
            name: Some("Evil\r\nName".to_owned()),
        };
        let errors = validate_contact_job(&job).expect_err("display name with CRLF");
        assert!(errors.contains(&ContactValidationError::InvalidCharacters {
            field: "target.email.name",
        }));
    }

    #[test]
    fn enforces_twilio_character_limit() {
        let job = ContactJob {
            version: ContractVersion::V1,
            job_id: "job-1".to_owned(),
            tenant_id: "tenant-1".to_owned(),
            application_id: "app-1".to_owned(),
            idempotency_key: "event-1".to_owned(),
            provider: ContactProviderKind::Twilio,
            target: ContactTarget::Sms {
                e164: "+15551234567".to_owned(),
            },
            content: ContactContent::Sms {
                body: "x".repeat(MAX_SMS_CHARACTERS + 1),
            },
            trace: TraceMetadata::default(),
        };
        let errors = validate_contact_job(&job).expect_err("too long");
        assert!(errors.contains(&ContactValidationError::SmsTooLong {
            max_characters: MAX_SMS_CHARACTERS,
        }));
    }
}
