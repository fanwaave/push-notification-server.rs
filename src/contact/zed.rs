use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use url::Url;
use utoipa::ToSchema;

use super::contracts::{ContactContent, ContactJob, ContactProviderKind, ContactTarget};
use crate::contracts::{ContractVersion, TraceMetadata};

const TENANT_ID: &str = "zed-pkg";
const APPLICATION_ID: &str = "registry-email";
const MAX_DIGEST_ITEMS: usize = 100;
const MAX_TEXT_FIELD_BYTES: usize = 4096;

/// A Zed registry email request. The producer supplies facts and destination;
/// fanwaave owns the user-facing rendering and SendGrid delivery contract.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ZedEmailJob {
    #[serde(default)]
    pub version: ContractVersion,
    pub job_id: String,
    pub idempotency_key: String,
    pub recipient: ZedEmailRecipient,
    pub event: ZedEmailEvent,
    pub preferences_url: String,
    pub unsubscribe_url: String,
    #[serde(default)]
    pub trace: TraceMetadata,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ZedEmailRecipient {
    pub email: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ZedEmailEvent {
    MajorRelease {
        package_name: String,
        previous_version: String,
        new_version: String,
        package_url: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        release_notes: Option<String>,
        interest: ZedPackageInterest,
    },
    SecurityPatch {
        package_name: String,
        affected_version: String,
        fixed_version: String,
        package_url: String,
        severity: ZedSecuritySeverity,
        advisory_id: String,
        advisory_url: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        summary: Option<String>,
        interest: ZedPackageInterest,
    },
    Digest {
        period_start: String,
        period_end: String,
        items: Vec<ZedDigestItem>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ZedPackageInterest {
    Favorite,
    Downloaded,
    FavoriteAndDownloaded,
}

impl ZedPackageInterest {
    fn label(self) -> &'static str {
        match self {
            Self::Favorite => "you favorited",
            Self::Downloaded => "you downloaded",
            Self::FavoriteAndDownloaded => "you favorited and downloaded",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ZedSecuritySeverity {
    Moderate,
    High,
    Critical,
}

impl ZedSecuritySeverity {
    fn label(self) -> &'static str {
        match self {
            Self::Moderate => "moderate",
            Self::High => "high",
            Self::Critical => "critical",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ZedDigestItem {
    pub package_name: String,
    pub current_version: String,
    pub latest_version: String,
    pub package_url: String,
    pub kind: ZedDigestItemKind,
    pub interest: ZedPackageInterest,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub security_severity: Option<ZedSecuritySeverity>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ZedDigestItemKind {
    MajorRelease,
    SecurityPatch,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ZedEmailRenderError {
    #[error("digest must contain 1 to {MAX_DIGEST_ITEMS} items")]
    InvalidDigestSize,
    #[error("{0} must be an http or https URL")]
    InvalidUrl(&'static str),
    #[error("{0} exceeds the maximum supported length")]
    FieldTooLong(&'static str),
}

pub fn render_zed_email_job(job: ZedEmailJob) -> Result<ContactJob, ZedEmailRenderError> {
    validate_url("preferences_url", &job.preferences_url)?;
    validate_url("unsubscribe_url", &job.unsubscribe_url)?;

    let (subject, text, html) = match &job.event {
        ZedEmailEvent::MajorRelease {
            package_name,
            previous_version,
            new_version,
            package_url,
            release_notes,
            interest,
        } => {
            validate_url("package_url", package_url)?;
            validate_fields(&[
                ("package_name", package_name),
                ("previous_version", previous_version),
                ("new_version", new_version),
            ])?;
            if let Some(notes) = release_notes {
                validate_field("release_notes", notes)?;
            }
            render_major_release(
                package_name,
                previous_version,
                new_version,
                package_url,
                release_notes.as_deref(),
                *interest,
                &job.preferences_url,
                &job.unsubscribe_url,
            )
        }
        ZedEmailEvent::SecurityPatch {
            package_name,
            affected_version,
            fixed_version,
            package_url,
            severity,
            advisory_id,
            advisory_url,
            summary,
            interest,
        } => {
            validate_url("package_url", package_url)?;
            validate_url("advisory_url", advisory_url)?;
            validate_fields(&[
                ("package_name", package_name),
                ("affected_version", affected_version),
                ("fixed_version", fixed_version),
                ("advisory_id", advisory_id),
            ])?;
            if let Some(summary) = summary {
                validate_field("summary", summary)?;
            }
            render_security_patch(
                package_name,
                affected_version,
                fixed_version,
                package_url,
                *severity,
                advisory_id,
                advisory_url,
                summary.as_deref(),
                *interest,
                &job.preferences_url,
                &job.unsubscribe_url,
            )
        }
        ZedEmailEvent::Digest {
            period_start,
            period_end,
            items,
        } => {
            if items.is_empty() || items.len() > MAX_DIGEST_ITEMS {
                return Err(ZedEmailRenderError::InvalidDigestSize);
            }
            validate_fields(&[("period_start", period_start), ("period_end", period_end)])?;
            for item in items {
                validate_fields(&[
                    ("package_name", &item.package_name),
                    ("current_version", &item.current_version),
                    ("latest_version", &item.latest_version),
                ])?;
                validate_url("package_url", &item.package_url)?;
            }
            render_digest(
                period_start,
                period_end,
                items,
                &job.preferences_url,
                &job.unsubscribe_url,
            )
        }
    };

    Ok(ContactJob {
        version: job.version,
        job_id: job.job_id,
        tenant_id: TENANT_ID.to_owned(),
        application_id: APPLICATION_ID.to_owned(),
        idempotency_key: job.idempotency_key,
        provider: ContactProviderKind::Sendgrid,
        target: ContactTarget::Email {
            address: job.recipient.email,
            name: job.recipient.name,
        },
        content: ContactContent::Email {
            subject: Some(subject),
            text: Some(text),
            html: Some(html),
            template_id: None,
            dynamic_template_data: BTreeMap::new(),
            reply_to: None,
        },
        trace: job.trace,
    })
}

fn render_major_release(
    package_name: &str,
    previous_version: &str,
    new_version: &str,
    package_url: &str,
    release_notes: Option<&str>,
    interest: ZedPackageInterest,
    preferences_url: &str,
    unsubscribe_url: &str,
) -> (String, String, String) {
    let subject = format!("Zed: {package_name} {new_version} is a new major release");
    let notes_text = release_notes
        .filter(|value| !value.trim().is_empty())
        .map(|value| format!("\n\nRelease notes:\n{}", value.trim()))
        .unwrap_or_default();
    let text = format!(
        "{package_name} moved from {previous_version} to {new_version}. You are receiving this because {} this package.{notes_text}\n\nPackage: {package_url}\nNotification settings: {preferences_url}\nUnsubscribe from Zed package email: {unsubscribe_url}",
        interest.label()
    );
    let notes_html = release_notes
        .filter(|value| !value.trim().is_empty())
        .map(|value| format!("<p><strong>Release notes:</strong><br>{}</p>", escape_html(value.trim())))
        .unwrap_or_default();
    let html = shell(
        &format!("New major release: {}", escape_html(package_name)),
        &format!(
            "<p><strong>{}</strong> moved from <code>{}</code> to <code>{}</code>.</p><p>You are receiving this because {} this package.</p>{}<p><a href=\"{}\">View package</a></p>",
            escape_html(package_name),
            escape_html(previous_version),
            escape_html(new_version),
            interest.label(),
            notes_html,
            escape_attr(package_url)
        ),
        preferences_url,
        unsubscribe_url,
    );
    (subject, text, html)
}

#[allow(clippy::too_many_arguments)]
fn render_security_patch(
    package_name: &str,
    affected_version: &str,
    fixed_version: &str,
    package_url: &str,
    severity: ZedSecuritySeverity,
    advisory_id: &str,
    advisory_url: &str,
    summary: Option<&str>,
    interest: ZedPackageInterest,
    preferences_url: &str,
    unsubscribe_url: &str,
) -> (String, String, String) {
    let subject = format!(
        "Zed security update: {package_name} {fixed_version} ({})",
        severity.label()
    );
    let summary_text = summary
        .filter(|value| !value.trim().is_empty())
        .map(|value| format!("\n\n{}", value.trim()))
        .unwrap_or_default();
    let text = format!(
        "A {} security issue affects {package_name} {affected_version}. The fix is available in {fixed_version}. Advisory: {advisory_id}.{summary_text}\n\nYou are receiving this because {} this package.\nPackage: {package_url}\nAdvisory: {advisory_url}\nNotification settings: {preferences_url}\nUnsubscribe from Zed package email: {unsubscribe_url}",
        severity.label(),
        interest.label()
    );
    let summary_html = summary
        .filter(|value| !value.trim().is_empty())
        .map(|value| format!("<p>{}</p>", escape_html(value.trim())))
        .unwrap_or_default();
    let html = shell(
        &format!("Security update: {}", escape_html(package_name)),
        &format!(
            "<p>A <strong>{}</strong> security issue affects <strong>{}</strong> <code>{}</code>. The fix is available in <code>{}</code>.</p><p>Advisory: <a href=\"{}\">{}</a></p>{}<p>You are receiving this because {} this package.</p><p><a href=\"{}\">View package</a></p>",
            severity.label(),
            escape_html(package_name),
            escape_html(affected_version),
            escape_html(fixed_version),
            escape_attr(advisory_url),
            escape_html(advisory_id),
            summary_html,
            interest.label(),
            escape_attr(package_url)
        ),
        preferences_url,
        unsubscribe_url,
    );
    (subject, text, html)
}

fn render_digest(
    period_start: &str,
    period_end: &str,
    items: &[ZedDigestItem],
    preferences_url: &str,
    unsubscribe_url: &str,
) -> (String, String, String) {
    let security_count = items
        .iter()
        .filter(|item| item.kind == ZedDigestItemKind::SecurityPatch)
        .count();
    let subject = if security_count == 0 {
        format!("Zed package digest: {} updates", items.len())
    } else {
        format!(
            "Zed package digest: {} updates, {} security",
            items.len(), security_count
        )
    };

    let mut text = format!("Zed package digest for {period_start} through {period_end}\n");
    let mut rows = String::new();
    for item in items {
        let kind = match item.kind {
            ZedDigestItemKind::MajorRelease => "major release".to_owned(),
            ZedDigestItemKind::SecurityPatch => format!(
                "{} security patch",
                item.security_severity
                    .map(ZedSecuritySeverity::label)
                    .unwrap_or("security")
            ),
        };
        text.push_str(&format!(
            "\n- {}: {} -> {} ({kind}); {} it\n  {}",
            item.package_name,
            item.current_version,
            item.latest_version,
            item.interest.label(),
            item.package_url
        ));
        rows.push_str(&format!(
            "<li><a href=\"{}\"><strong>{}</strong></a>: <code>{}</code> → <code>{}</code> — {} ({})</li>",
            escape_attr(&item.package_url),
            escape_html(&item.package_name),
            escape_html(&item.current_version),
            escape_html(&item.latest_version),
            escape_html(&kind),
            item.interest.label()
        ));
    }
    text.push_str(&format!(
        "\n\nNotification settings: {preferences_url}\nUnsubscribe from Zed package email: {unsubscribe_url}"
    ));
    let html = shell(
        "Your Zed package digest",
        &format!(
            "<p>Updates from <strong>{}</strong> through <strong>{}</strong>.</p><ul>{}</ul>",
            escape_html(period_start),
            escape_html(period_end),
            rows
        ),
        preferences_url,
        unsubscribe_url,
    );
    (subject, text, html)
}

fn shell(title: &str, body: &str, preferences_url: &str, unsubscribe_url: &str) -> String {
    format!(
        "<!doctype html><html><body><main><h1>{title}</h1>{body}<hr><p><a href=\"{}\">Notification settings</a> · <a href=\"{}\">Unsubscribe from package email</a></p></main></body></html>",
        escape_attr(preferences_url),
        escape_attr(unsubscribe_url)
    )
}

fn validate_fields(fields: &[(&'static str, &str)]) -> Result<(), ZedEmailRenderError> {
    for (name, value) in fields {
        validate_field(name, value)?;
    }
    Ok(())
}

fn validate_field(name: &'static str, value: &str) -> Result<(), ZedEmailRenderError> {
    if value.len() > MAX_TEXT_FIELD_BYTES {
        return Err(ZedEmailRenderError::FieldTooLong(name));
    }
    Ok(())
}

fn validate_url(name: &'static str, value: &str) -> Result<(), ZedEmailRenderError> {
    let parsed = Url::parse(value).map_err(|_| ZedEmailRenderError::InvalidUrl(name))?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
        return Err(ZedEmailRenderError::InvalidUrl(name));
    }
    Ok(())
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn escape_attr(value: &str) -> String {
    escape_html(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_job(event: ZedEmailEvent) -> ZedEmailJob {
        ZedEmailJob {
            version: ContractVersion::V1,
            job_id: "job-1".into(),
            idempotency_key: "zed:event:1".into(),
            recipient: ZedEmailRecipient {
                email: "dev@example.com".into(),
                name: Some("Developer".into()),
            },
            event,
            preferences_url: "https://zed.pkg/settings/notifications".into(),
            unsubscribe_url: "https://zed.pkg/unsubscribe/token".into(),
            trace: TraceMetadata::default(),
        }
    }

    #[test]
    fn renders_major_release_as_sendgrid_contact_job() {
        let job = base_job(ZedEmailEvent::MajorRelease {
            package_name: "ores-cache".into(),
            previous_version: "1.9.0".into(),
            new_version: "2.0.0".into(),
            package_url: "https://zed.pkg/p/oresoftware/ores-cache".into(),
            release_notes: Some("Breaking API cleanup".into()),
            interest: ZedPackageInterest::Favorite,
        });
        let contact = render_zed_email_job(job).expect("render");
        assert_eq!(contact.provider, ContactProviderKind::Sendgrid);
        assert_eq!(contact.tenant_id, TENANT_ID);
        match contact.content {
            ContactContent::Email { subject, text, html, .. } => {
                assert!(subject.expect("subject").contains("2.0.0"));
                assert!(text.expect("text").contains("1.9.0"));
                assert!(html.expect("html").contains("Notification settings"));
            }
            _ => panic!("expected email"),
        }
    }

    #[test]
    fn escapes_package_control_text_in_html() {
        let job = base_job(ZedEmailEvent::MajorRelease {
            package_name: "<script>alert(1)</script>".into(),
            previous_version: "1".into(),
            new_version: "2".into(),
            package_url: "https://example.com/pkg".into(),
            release_notes: Some("<b>unsafe</b>".into()),
            interest: ZedPackageInterest::Downloaded,
        });
        let contact = render_zed_email_job(job).expect("render");
        match contact.content {
            ContactContent::Email { html, .. } => {
                let html = html.expect("html");
                assert!(!html.contains("<script>"));
                assert!(html.contains("&lt;script&gt;"));
                assert!(html.contains("&lt;b&gt;unsafe&lt;/b&gt;"));
            }
            _ => panic!("expected email"),
        }
    }

    #[test]
    fn rejects_empty_digest_and_non_http_links() {
        let empty = base_job(ZedEmailEvent::Digest {
            period_start: "2026-09-01".into(),
            period_end: "2026-09-08".into(),
            items: vec![],
        });
        assert_eq!(
            render_zed_email_job(empty).expect_err("empty digest"),
            ZedEmailRenderError::InvalidDigestSize
        );

        let mut invalid = base_job(ZedEmailEvent::MajorRelease {
            package_name: "pkg".into(),
            previous_version: "1".into(),
            new_version: "2".into(),
            package_url: "https://example.com/pkg".into(),
            release_notes: None,
            interest: ZedPackageInterest::Favorite,
        });
        invalid.unsubscribe_url = "javascript:alert(1)".into();
        assert_eq!(
            render_zed_email_job(invalid).expect_err("invalid URL"),
            ZedEmailRenderError::InvalidUrl("unsubscribe_url")
        );
    }
}
