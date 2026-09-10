use std::{env, fmt, net::SocketAddr, path::Path};

use flags2env::{
    BundledFlags2Env,
    env_map::{EnvBindingSpec, EnvMap, EnvValueKind, resolve_typed_bindings},
};
use push_notification_server::{
    ApiState, ContactApiState, NatsConfig, application_router, canonical_json,
    contact_registry_from_env, openapi_document, provider_registry_from_env,
    public_openapi_document, request_authenticator_from_env, run_nats_consumer,
};
use tracing_subscriber::EnvFilter;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OpenApiScope {
    Internal,
    Public,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RuntimeArgs {
    merged_env: EnvMap,
    export_openapi: Option<OpenApiScope>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ArgumentError(String);

impl fmt::Display for ArgumentError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for ArgumentError {}

/// Run the notification service process.
///
/// Keeping orchestration in a module makes the binary entrypoint a thin shell
/// and lets argument and address parsing be tested without initializing
/// telemetry, provider credentials, NATS, or sockets.
pub(crate) async fn run<I>(args: I) -> Result<(), Box<dyn std::error::Error>>
where
    I: IntoIterator<Item = String>,
{
    let args = args.into_iter().collect::<Vec<_>>();
    let runtime = parse_runtime_args(&args, env::vars().collect(), Path::new(".cli-flags.toml"))?;

    if let Some(scope) = runtime.export_openapi {
        let openapi = match scope {
            OpenApiScope::Internal => openapi_document(),
            OpenApiScope::Public => public_openapi_document()?,
        };
        print!("{}", canonical_json(&openapi)?);
        return Ok(());
    }

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                EnvFilter::new("push_notification_server=info,tower_http=info")
            }),
        )
        .init();

    let address = bind_address(&runtime.merged_env)?;
    let registry = provider_registry_from_env()?;
    let contact_registry = contact_registry_from_env()?;
    let authenticator = request_authenticator_from_env()?;

    if let Some(nats_config) = NatsConfig::from_env()? {
        let nats_registry = registry.clone();
        tokio::spawn(async move {
            if let Err(error) = run_nats_consumer(nats_config, nats_registry).await {
                tracing::error!(%error, "JetStream push ingestion stopped");
            }
        });
    } else {
        tracing::info!("JetStream push ingestion disabled because NATS_URL is not configured");
    }

    let app = application_router(
        ApiState::new(registry, authenticator.clone()),
        ContactApiState::new(contact_registry, authenticator.clone()),
        authenticator,
    )?;
    let listener = tokio::net::TcpListener::bind(address).await?;

    tracing::info!(%address, "notification server listening");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    Ok(())
}

fn parse_runtime_args(
    args: &[String],
    ambient_env: EnvMap,
    config_path: &Path,
) -> Result<RuntimeArgs, ArgumentError> {
    let config_path = config_path
        .to_str()
        .ok_or_else(|| ArgumentError(".cli-flags.toml path is not valid UTF-8".to_owned()))?;
    let parser = BundledFlags2Env::new();
    parser.audit_config(Some(config_path)).map_err(|error| {
        ArgumentError(format!("flags-2-env configuration audit failed: {error}"))
    })?;
    let parsed = parser
        .parse_structured(args, Some(config_path))
        .map_err(|error| ArgumentError(format!("flags-2-env parse failed: {error}")))?;

    if !parsed.unknown_options.is_empty() {
        return Err(ArgumentError(format!(
            "unknown command-line option(s): {}",
            parsed.unknown_options.join(", ")
        )));
    }
    if !parsed.errors.is_empty() {
        return Err(ArgumentError(format!(
            "invalid command-line value(s): {}",
            parsed.errors.join("; ")
        )));
    }
    if !parsed.command.is_empty() {
        return Err(ArgumentError(format!(
            "unexpected command or positional argument: {}",
            parsed.command
        )));
    }

    let mut merged_env = ambient_env;
    merged_env.extend(parsed.provided_flags);
    validate_runtime_env(&merged_env)?;

    let export_openapi = merged_env
        .get("FANWAAVE_EXPORT_OPENAPI")
        .map(String::as_str)
        .map(parse_openapi_scope)
        .transpose()?;

    Ok(RuntimeArgs {
        merged_env,
        export_openapi,
    })
}

fn validate_runtime_env(runtime_env: &EnvMap) -> Result<(), ArgumentError> {
    let specs = [
        EnvBindingSpec::optional("host", "HOST", EnvValueKind::String),
        EnvBindingSpec::optional("port", "PORT", EnvValueKind::Integer),
        EnvBindingSpec::optional(
            "export_openapi",
            "FANWAAVE_EXPORT_OPENAPI",
            EnvValueKind::String,
        ),
    ];

    resolve_typed_bindings(runtime_env, &specs)
        .map(|_| ())
        .map_err(|diagnostics| {
            ArgumentError(format!(
                "runtime environment preflight failed: {}",
                diagnostics
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join("; ")
            ))
        })
}

fn parse_openapi_scope(value: &str) -> Result<OpenApiScope, ArgumentError> {
    match value {
        "internal" => Ok(OpenApiScope::Internal),
        "public" => Ok(OpenApiScope::Public),
        other => Err(ArgumentError(format!("unsupported OpenAPI scope: {other}"))),
    }
}

fn bind_address(runtime_env: &EnvMap) -> Result<SocketAddr, std::net::AddrParseError> {
    let host = runtime_env
        .get("HOST")
        .map(String::as_str)
        .unwrap_or("0.0.0.0");
    let port = runtime_env
        .get("PORT")
        .map(String::as_str)
        .unwrap_or("8121");
    parse_bind_address(host, port)
}

fn parse_bind_address(host: &str, port: &str) -> Result<SocketAddr, std::net::AddrParseError> {
    format!("{host}:{port}").parse()
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };

    #[cfg(unix)]
    let terminate = async {
        use tokio::signal::unix::{SignalKind, signal};
        if let Ok(mut stream) = signal(SignalKind::terminate()) {
            let _ = stream.recv().await;
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use push_notification_server::{
        ContactProviderRegistry, DenyAllAuthenticator, ProviderRegistry,
    };

    use super::*;

    fn flags_contract() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join(".cli-flags.toml")
    }

    #[test]
    fn export_scope_is_owned_by_flags_2_env() {
        let runtime = parse_runtime_args(
            &["server".to_owned(), "--export-openapi=public".to_owned()],
            EnvMap::new(),
            &flags_contract(),
        )
        .expect("public scope");
        assert_eq!(runtime.export_openapi, Some(OpenApiScope::Public));

        let runtime = parse_runtime_args(
            &["server".to_owned(), "--export-openapi=internal".to_owned()],
            EnvMap::new(),
            &flags_contract(),
        )
        .expect("internal scope");
        assert_eq!(runtime.export_openapi, Some(OpenApiScope::Internal));
    }

    #[test]
    fn rejects_unknown_flags_and_unsupported_openapi_scope() {
        assert!(
            parse_runtime_args(
                &["server".to_owned(), "--not-declared=yes".to_owned()],
                EnvMap::new(),
                &flags_contract(),
            )
            .is_err()
        );

        assert!(
            parse_runtime_args(
                &["server".to_owned(), "--export-openapi=partner".to_owned()],
                EnvMap::new(),
                &flags_contract(),
            )
            .is_err()
        );
    }

    #[test]
    fn rejects_noncanonical_ambient_port_before_socket_startup() {
        let error = parse_runtime_args(
            &["server".to_owned()],
            EnvMap::from([("PORT".to_owned(), "08121".to_owned())]),
            &flags_contract(),
        )
        .expect_err("noncanonical integer must fail closed");

        assert!(error.to_string().contains("ENV_PARSE"));
        assert!(error.to_string().contains("PORT"));
        assert!(!error.to_string().contains("08121"));
    }

    #[test]
    fn argv_overrides_ambient_env_without_mutating_process_env() {
        let before = std::env::var_os("PORT");
        let runtime = parse_runtime_args(
            &["server".to_owned(), "--port=9001".to_owned()],
            EnvMap::from([
                ("HOST".to_owned(), "127.0.0.1".to_owned()),
                ("PORT".to_owned(), "9000".to_owned()),
            ]),
            &flags_contract(),
        )
        .expect("valid runtime args");

        assert_eq!(
            runtime.merged_env.get("PORT").map(String::as_str),
            Some("9001")
        );
        assert_eq!(
            bind_address(&runtime.merged_env).expect("valid bind"),
            "127.0.0.1:9001".parse().expect("socket address")
        );
        assert_eq!(std::env::var_os("PORT"), before);
    }

    #[test]
    fn ambient_env_beats_contract_default_when_argv_is_silent() {
        let runtime = parse_runtime_args(
            &["server".to_owned()],
            EnvMap::from([
                ("HOST".to_owned(), "127.0.0.1".to_owned()),
                ("PORT".to_owned(), "9443".to_owned()),
            ]),
            &flags_contract(),
        )
        .expect("valid runtime args");

        assert_eq!(
            bind_address(&runtime.merged_env).expect("valid bind"),
            "127.0.0.1:9443".parse().expect("socket address")
        );
    }

    #[test]
    fn bind_address_parser_preserves_the_runtime_contract() {
        let address = parse_bind_address("0.0.0.0", "8121").expect("valid default address");
        assert_eq!(address, "0.0.0.0:8121".parse().expect("socket address"));
        assert!(parse_bind_address("0.0.0.0", "not-a-port").is_err());
    }

    #[test]
    fn routers_can_be_constructed_without_runtime_credentials() {
        let authenticator = Arc::new(DenyAllAuthenticator);
        let _ = push_notification_server::application_router(
            ApiState::new(ProviderRegistry::new(), authenticator.clone()),
            ContactApiState::new(ContactProviderRegistry::new(), authenticator.clone()),
            authenticator,
        )
        .expect("application router");
    }
}
