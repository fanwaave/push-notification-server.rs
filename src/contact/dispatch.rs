use std::sync::Arc;

use serde::Serialize;
use utoipa::ToSchema;

use super::contracts::{ContactJob, ContactOutcome, ContactOutcomeClass, ContactProviderKind};
use super::provider::{ContactProvider, ContactProviderError};
use super::rate_limit::ContactRateLimiter;
use crate::provider::ProviderReadiness;

#[derive(Clone, Default)]
pub struct ContactProviderRegistry {
    sendgrid: Option<Arc<dyn ContactProvider>>,
    twilio: Option<Arc<dyn ContactProvider>>,
    sendgrid_limiter: Option<Arc<ContactRateLimiter>>,
    twilio_limiter: Option<Arc<ContactRateLimiter>>,
}

impl ContactProviderRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_provider(mut self, provider: Arc<dyn ContactProvider>) -> Self {
        match provider.kind() {
            ContactProviderKind::Sendgrid => self.sendgrid = Some(provider),
            ContactProviderKind::Twilio => self.twilio = Some(provider),
        }
        self
    }

    pub fn with_rate_limit(mut self, provider: ContactProviderKind, per_minute: u32) -> Self {
        let limiter = Some(Arc::new(ContactRateLimiter::per_minute(per_minute)));
        match provider {
            ContactProviderKind::Sendgrid => self.sendgrid_limiter = limiter,
            ContactProviderKind::Twilio => self.twilio_limiter = limiter,
        }
        self
    }

    pub async fn dispatch(&self, job: &ContactJob) -> Result<ContactOutcome, ContactProviderError> {
        let (provider, limiter) = match job.provider {
            ContactProviderKind::Sendgrid => (self.sendgrid.as_ref(), self.sendgrid_limiter.as_ref()),
            ContactProviderKind::Twilio => (self.twilio.as_ref(), self.twilio_limiter.as_ref()),
        };
        let provider = provider.ok_or_else(|| {
            ContactProviderError::not_configured(format!(
                "{} provider is not configured",
                job.provider.as_str()
            ))
        })?;
        if provider.kind() != job.provider {
            return Err(ContactProviderError::internal(
                "contact registry returned the wrong provider kind",
            ));
        }
        if let Some(limiter) = limiter {
            if let Err(retry_after) = limiter.try_acquire().await {
                return Err(ContactProviderError::delivery(
                    ContactOutcomeClass::Throttled,
                    "contact delivery rate limit reached",
                    Some(retry_after),
                    Some("local_rate_limit".to_owned()),
                ));
            }
        }
        provider.send(job).await
    }

    pub fn readiness(&self) -> ContactRegistryReadiness {
        ContactRegistryReadiness {
            sendgrid: readiness(self.sendgrid.as_ref()),
            twilio: readiness(self.twilio.as_ref()),
        }
    }

    pub fn has_configured_provider(&self) -> bool {
        [self.sendgrid.as_ref(), self.twilio.as_ref()]
            .into_iter()
            .flatten()
            .any(|provider| provider.readiness().configured)
    }
}

fn readiness(provider: Option<&Arc<dyn ContactProvider>>) -> ContactProviderReadinessView {
    provider
        .map(|provider| provider.readiness())
        .unwrap_or_else(|| ProviderReadiness::not_ready("provider is not configured"))
        .into()
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ContactRegistryReadiness {
    pub sendgrid: ContactProviderReadinessView,
    pub twilio: ContactProviderReadinessView,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ContactProviderReadinessView {
    pub configured: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub safe_reason: Option<String>,
}

impl From<ProviderReadiness> for ContactProviderReadinessView {
    fn from(value: ProviderReadiness) -> Self {
        Self {
            configured: value.configured,
            safe_reason: value.safe_reason,
        }
    }
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;

    use super::*;
    use crate::contact::contracts::{ContactContent, ContactTarget};
    use crate::contracts::{ContractVersion, TraceMetadata};

    struct AcceptingProvider(ContactProviderKind);

    #[async_trait]
    impl ContactProvider for AcceptingProvider {
        fn kind(&self) -> ContactProviderKind {
            self.0
        }

        fn readiness(&self) -> ProviderReadiness {
            ProviderReadiness::ready()
        }

        async fn send(&self, job: &ContactJob) -> Result<ContactOutcome, ContactProviderError> {
            Ok(ContactOutcome::accepted(
                job,
                Some("accepted-id".to_owned()),
            ))
        }
    }

    fn email_job() -> ContactJob {
        ContactJob {
            version: ContractVersion::V1,
            job_id: "job-1".to_owned(),
            tenant_id: "tenant-1".to_owned(),
            application_id: "app-1".to_owned(),
            idempotency_key: "event-1".to_owned(),
            provider: ContactProviderKind::Sendgrid,
            target: ContactTarget::Email {
                address: "person@example.com".to_owned(),
                name: None,
            },
            content: ContactContent::Email {
                subject: Some("Hello".to_owned()),
                text: Some("World".to_owned()),
                html: None,
                template_id: None,
                dynamic_template_data: Default::default(),
                reply_to: None,
            },
            trace: TraceMetadata::default(),
        }
    }

    #[tokio::test]
    async fn dispatches_to_the_registered_provider() {
        let registry = ContactProviderRegistry::new()
            .with_provider(Arc::new(AcceptingProvider(ContactProviderKind::Sendgrid)));
        let outcome = registry.dispatch(&email_job()).await.expect("outcome");
        assert_eq!(outcome.provider, ContactProviderKind::Sendgrid);
        assert_eq!(outcome.provider_code.as_deref(), Some("accepted-id"));
    }

    #[tokio::test]
    async fn configured_rate_limit_throttles_before_provider_send() {
        let registry = ContactProviderRegistry::new()
            .with_provider(Arc::new(AcceptingProvider(ContactProviderKind::Sendgrid)))
            .with_rate_limit(ContactProviderKind::Sendgrid, 1);
        registry.dispatch(&email_job()).await.expect("first token");
        let error = registry
            .dispatch(&email_job())
            .await
            .expect_err("second token should be throttled");
        assert!(matches!(
            error,
            ContactProviderError::Delivery {
                class: ContactOutcomeClass::Throttled,
                ..
            }
        ));
    }

    #[test]
    fn missing_providers_are_not_ready() {
        let readiness = ContactProviderRegistry::new().readiness();
        assert!(!readiness.sendgrid.configured);
        assert!(!readiness.twilio.configured);
    }
}
