use std::env;
use std::sync::Arc;

use thiserror::Error;
use url::Url;

use super::dispatch::ContactProviderRegistry;
use super::sendgrid::{SendGridConfig, SendGridConfigError, SendGridProvider, SendGridRegion};
use super::twilio::{
    TwilioConfig, TwilioConfigError, TwilioCredentials, TwilioProvider, TwilioSender,
};

#[derive(Debug, Error)]
pub enum ContactRuntimeConfigError {
    #[error("{0} configuration is incomplete")]
    IncompleteProvider(&'static str),

    #[error("SENDGRID_REGION must be global or eu")]
    InvalidSendGridRegion,

    #[error("SENDGRID_API_KEY must be an exact SG.-prefixed key of 20 to 69 printable ASCII bytes")]
    InvalidSendGridApiKey,

    #[error("{0} must contain exact printable ASCII bytes without whitespace")]
    InvalidProviderCredential(&'static str),

    #[error("configure exactly one Twilio credential mode: Auth Token or API Key")]
    AmbiguousTwilioCredentials,

    #[error("configure exactly one Twilio sender: Messaging Service SID or From number")]
    AmbiguousTwilioSender,

    #[error("TWILIO_STATUS_CALLBACK_URL is not a valid URL")]
    InvalidTwilioStatusCallbackUrl,

    #[error("TWILIO_VALIDITY_PERIOD_SECONDS must be a positive integer")]
    InvalidTwilioValidityPeriod,

    #[error(transparent)]
    SendGrid(#[from] SendGridConfigError),

    #[error(transparent)]
    Twilio(#[from] TwilioConfigError),
}

pub fn contact_registry_from_env() -> Result<ContactProviderRegistry, ContactRuntimeConfigError> {
    let registry = ContactProviderRegistry::new();
    let registry = configure_sendgrid(registry)?;
    configure_twilio(registry)
}

fn configure_sendgrid(
    registry: ContactProviderRegistry,
) -> Result<ContactProviderRegistry, ContactRuntimeConfigError> {
    let api_key = provider_credential_env("SENDGRID_API_KEY")?;
    let from_email = non_empty_env("SENDGRID_FROM_EMAIL").or_else(|| non_empty_env("EMAIL_FROM"));
    if api_key.is_none() && from_email.is_none() {
        return Ok(registry);
    }
    let api_key = api_key.ok_or(ContactRuntimeConfigError::IncompleteProvider("sendgrid"))?;
    validate_sendgrid_api_key(&api_key)?;
    let from_email = from_email.ok_or(ContactRuntimeConfigError::IncompleteProvider("sendgrid"))?;
    let region = parse_sendgrid_region(non_empty_env("SENDGRID_REGION").as_deref())?;
    let config = SendGridConfig::new(
        api_key,
        from_email,
        non_empty_env("SENDGRID_FROM_NAME"),
        region,
    )?
    .with_sandbox_mode(environment_flag("SENDGRID_SANDBOX_MODE"));
    let provider = SendGridProvider::new(config)?;
    Ok(registry.with_provider(Arc::new(provider)))
}

fn configure_twilio(
    registry: ContactProviderRegistry,
) -> Result<ContactProviderRegistry, ContactRuntimeConfigError> {
    let account_sid = provider_credential_env("TWILIO_ACCOUNT_SID")?;
    let auth_token = provider_credential_env("TWILIO_AUTH_TOKEN")?;
    let api_key_sid = provider_credential_env("TWILIO_API_KEY_SID")?;
    let api_key_secret = provider_credential_env("TWILIO_API_KEY_SECRET")?;
    let messaging_service_sid = provider_credential_env("TWILIO_MESSAGING_SERVICE_SID")?;
    let from_number = non_empty_env("TWILIO_FROM_NUMBER");
    let status_callback = non_empty_env("TWILIO_STATUS_CALLBACK_URL");
    let validity_period = non_empty_env("TWILIO_VALIDITY_PERIOD_SECONDS");

    let any_present = [
        account_sid.is_some(),
        auth_token.is_some(),
        api_key_sid.is_some(),
        api_key_secret.is_some(),
        messaging_service_sid.is_some(),
        from_number.is_some(),
        status_callback.is_some(),
        validity_period.is_some(),
    ]
    .into_iter()
    .any(|present| present);
    if !any_present {
        return Ok(registry);
    }

    let account_sid = account_sid.ok_or(ContactRuntimeConfigError::IncompleteProvider("twilio"))?;
    let credentials = twilio_credentials(auth_token, api_key_sid, api_key_secret)?;
    let sender = twilio_sender(messaging_service_sid, from_number)?;
    let mut config = TwilioConfig::new(account_sid, credentials, sender)?;
    if let Some(callback) = status_callback {
        let callback = Url::parse(&callback)
            .map_err(|_| ContactRuntimeConfigError::InvalidTwilioStatusCallbackUrl)?;
        config = config.with_status_callback_url(callback)?;
    }
    if let Some(validity_period) = validity_period {
        let validity_period = validity_period
            .parse::<u32>()
            .ok()
            .filter(|value| *value > 0)
            .ok_or(ContactRuntimeConfigError::InvalidTwilioValidityPeriod)?;
        config = config.with_validity_period_seconds(validity_period)?;
    }
    let provider = TwilioProvider::new(config)?;
    Ok(registry.with_provider(Arc::new(provider)))
}

fn parse_sendgrid_region(value: Option<&str>) -> Result<SendGridRegion, ContactRuntimeConfigError> {
    match value
        .unwrap_or("global")
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "global" | "us" => Ok(SendGridRegion::Global),
        "eu" | "europe" => Ok(SendGridRegion::Europe),
        _ => Err(ContactRuntimeConfigError::InvalidSendGridRegion),
    }
}

fn validate_sendgrid_api_key(value: &str) -> Result<(), ContactRuntimeConfigError> {
    if !value.starts_with("SG.") || !(20..=69).contains(&value.len()) {
        return Err(ContactRuntimeConfigError::InvalidSendGridApiKey);
    }
    Ok(())
}

fn twilio_credentials(
    auth_token: Option<String>,
    api_key_sid: Option<String>,
    api_key_secret: Option<String>,
) -> Result<TwilioCredentials, ContactRuntimeConfigError> {
    let auth_configured = auth_token.is_some();
    let api_any = api_key_sid.is_some() || api_key_secret.is_some();
    if auth_configured && api_any {
        return Err(ContactRuntimeConfigError::AmbiguousTwilioCredentials);
    }
    if let Some(token) = auth_token {
        return Ok(TwilioCredentials::AuthToken { token });
    }
    match (api_key_sid, api_key_secret) {
        (Some(sid), Some(secret)) => Ok(TwilioCredentials::ApiKey { sid, secret }),
        _ => Err(ContactRuntimeConfigError::IncompleteProvider("twilio")),
    }
}

fn twilio_sender(
    messaging_service_sid: Option<String>,
    from_number: Option<String>,
) -> Result<TwilioSender, ContactRuntimeConfigError> {
    match (messaging_service_sid, from_number) {
        (Some(_), Some(_)) => Err(ContactRuntimeConfigError::AmbiguousTwilioSender),
        (Some(sid), None) => Ok(TwilioSender::MessagingService { sid }),
        (None, Some(e164)) => Ok(TwilioSender::PhoneNumber { e164 }),
        (None, None) => Err(ContactRuntimeConfigError::IncompleteProvider("twilio")),
    }
}

fn provider_credential_env(
    name: &'static str,
) -> Result<Option<String>, ContactRuntimeConfigError> {
    match env::var(name) {
        Ok(value) if value.is_empty() => Ok(None),
        Ok(value) => validate_provider_credential(name, value).map(Some),
        Err(env::VarError::NotPresent) => Ok(None),
        Err(env::VarError::NotUnicode(_)) => {
            Err(ContactRuntimeConfigError::InvalidProviderCredential(name))
        }
    }
}

fn validate_provider_credential(
    name: &'static str,
    value: String,
) -> Result<String, ContactRuntimeConfigError> {
    if !value.bytes().all(|byte| byte.is_ascii_graphic()) {
        return Err(ContactRuntimeConfigError::InvalidProviderCredential(name));
    }
    Ok(value)
}

fn non_empty_env(name: &str) -> Option<String> {
    env::var(name)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

fn environment_flag(name: &str) -> bool {
    env::var(name).ok().is_some_and(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_sendgrid_regions() {
        assert_eq!(
            parse_sendgrid_region(None).expect("default"),
            SendGridRegion::Global
        );
        assert_eq!(
            parse_sendgrid_region(Some("EU")).expect("EU"),
            SendGridRegion::Europe
        );
        assert!(parse_sendgrid_region(Some("moon")).is_err());
    }

    #[test]
    fn validates_sendgrid_key_shape_without_echoing_values() {
        let valid = format!("SG.{}.{}", "a".repeat(22), "b".repeat(43));
        validate_sendgrid_api_key(&valid).expect("valid SendGrid key shape");

        let marker = "not-a-sendgrid-key";
        let error = validate_sendgrid_api_key(marker)
            .expect_err("invalid key")
            .to_string();
        assert!(error.contains("SENDGRID_API_KEY"));
        assert!(!error.contains(marker));
    }

    #[test]
    fn rejects_provider_credential_whitespace_without_echoing_values() {
        let marker = "secret with whitespace";
        let error = validate_provider_credential("TWILIO_AUTH_TOKEN", marker.to_owned())
            .expect_err("invalid credential")
            .to_string();
        assert!(error.contains("TWILIO_AUTH_TOKEN"));
        assert!(!error.contains(marker));
    }

    #[test]
    fn rejects_ambiguous_twilio_configuration() {
        let credentials = twilio_credentials(
            Some("auth-token-that-is-long-enough".to_owned()),
            Some(format!("SK{}", "0".repeat(32))),
            Some("api-key-secret-that-is-long-enough".to_owned()),
        );
        assert!(matches!(
            credentials,
            Err(ContactRuntimeConfigError::AmbiguousTwilioCredentials)
        ));
        let sender = twilio_sender(
            Some(format!("MG{}", "0".repeat(32))),
            Some("+15551234567".to_owned()),
        );
        assert!(matches!(
            sender,
            Err(ContactRuntimeConfigError::AmbiguousTwilioSender)
        ));
    }
}
