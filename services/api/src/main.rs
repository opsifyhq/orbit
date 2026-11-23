use std::sync::Arc;

use anyhow::Context;
use orbit_api::{
    build_router, initialize, rate_limit::RateLimiter, telemetry, ApiConfig, AppState,
};
use orbit_events::{connect as connect_bus, EventBusConfig};
use orbit_harness::HarnessRegistry;
use orbit_security::{JwtValidator, LicenseHandle, LicenseSource, SecretCipher};
use orbit_store::Store;
use tokio::net::TcpListener;

/// Upper bound on how long open connections may delay exit after a shutdown signal.
const SHUTDOWN_GRACE: std::time::Duration = std::time::Duration::from_secs(10);

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let shutdown = tokio_util::sync::CancellationToken::new();
    dotenvy::dotenv().ok();
    telemetry::init("orbit-api");

    let config = ApiConfig::from_env().context("invalid API configuration")?;
    if config.is_production() {
        if matches!(config.auth.mode, orbit_security::AuthMode::Dev) {
            anyhow::bail!("ORBIT_AUTH_MODE=dev is not allowed in production");
        }
        if config.cors_origins.iter().any(|origin| origin == "*") {
            tracing::warn!("CORS is configured to allow any origin in production");
        }
    }

    let cipher = Arc::new(
        SecretCipher::from_base64_key(&config.master_key)
            .context("ORBIT_MASTER_KEY must be a base64-encoded 32-byte key")?,
    );
    let metrics = Arc::new(orbit_api::metrics::Metrics::new());
    let store = Store::connect(&config.database_url, config.db_max_connections)
        .await
        .context("could not connect to PostgreSQL")?
        .with_cipher(cipher)
        .with_execution_observer(metrics.clone());

    let bus_config = EventBusConfig::from_env().context("invalid event bus configuration")?;
    let bus = connect_bus(&bus_config)
        .await
        .context("could not connect to the event bus")?;
    let bus = orbit_api::metrics::MeteredBus::wrap(bus, metrics.clone());
    let registry = Arc::new(HarnessRegistry::builtin());

    let initialized = initialize(&store, &registry, &config, bus.as_ref())
        .await
        .context("bootstrap failed")?;

    // Settings saved through the UI are applied on startup (restart to take effect).
    let auth_config =
        orbit_api::handlers::settings::apply_stored_oidc(&store, config.auth.clone()).await;

    let jwt = match auth_config.mode {
        orbit_security::AuthMode::Oidc => {
            let oidc = auth_config
                .oidc
                .as_ref()
                .context("OIDC configuration missing")?;
            Some(Arc::new(
                JwtValidator::new(oidc).context("invalid OIDC config")?,
            ))
        }
        orbit_security::AuthMode::Dev => None,
    };

    let state = AppState {
        store,
        bus,
        registry,
        auth: Arc::new(auth_config),
        jwt,
        default_org_id: initialized.default_org_id,
        dev_identity: initialized.dev_identity.map(Arc::new),
        environment: config.environment.clone(),
        events_driver: bus_config.driver.clone(),
        public_base_url: config.public_base_url.clone(),
        metrics,
        metrics_access: orbit_api::MetricsAccess {
            token: config.metrics_token.clone(),
            dedicated_listener: config.metrics_addr.is_some(),
        },
        max_body_bytes: config.body_limit_bytes,
        rate_limiter: Arc::new(
            RateLimiter::new(config.anon_rate_limit_per_minute)
                .with_trusted_proxies(config.trusted_proxies.clone()),
        ),
        cors_origins: config.cors_origins.clone(),
        launcher_url: config.launcher_url.clone(),
        launcher_token: config.launcher_token.clone(),
        worker_api_url: config.worker_api_url.clone(),
        ca_secret_name: config.ca_secret_name.clone(),
        ca_secret_key: config.ca_secret_key.clone(),
        archive_after_days: config.archive_after_days,
        output_delivery_retention_days: config.output_delivery_retention_days,
        license: LicenseHandle::load(LicenseSource::from_env()),
        shutdown: shutdown.clone(),
    };

    let background = orbit_api::background::spawn(state.clone());
    let metrics_server = match &config.metrics_addr {
        Some(addr) => {
            let listener = TcpListener::bind(addr)
                .await
                .with_context(|| format!("could not bind metrics listener {addr}"))?;
            tracing::info!(
                address = %addr,
                token_required = config.metrics_token.is_some(),
                "ORBIT metrics listening"
            );
            let router = orbit_api::build_metrics_router(state.clone());
            let token = shutdown.clone();
            Some(tokio::spawn(async move {
                let served = axum::serve(listener, router)
                    .with_graceful_shutdown(async move { token.cancelled().await })
                    .await;
                if let Err(error) = served {
                    tracing::error!(%error, "metrics listener failed");
                }
            }))
        }
        None => None,
    };
    if config.is_production() && config.metrics_token.is_none() && config.metrics_addr.is_none() {
        tracing::warn!(
            "/metrics is served on the public API port without a token; set \
             ORBIT_METRICS_ADDR or ORBIT_METRICS_TOKEN, or keep it off the ingress"
        );
    }
    let router = build_router(state);
    let listener = TcpListener::bind(&config.http_addr)
        .await
        .with_context(|| format!("could not bind {}", config.http_addr))?;
    tracing::info!(
        address = %config.http_addr,
        auth_mode = config.auth.mode.as_str(),
        events_driver = %bus_config.driver,
        "ORBIT API listening"
    );

    // Signal -> cancel token (ends SSE streams, stops background loops) -> axum drains
    // remaining connections. A hard deadline keeps a stuck connection from outliving the
    // kubelet grace period.
    let signal_token = shutdown.clone();
    let serve = axum::serve(
        listener,
        router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(async move {
        shutdown_signal().await;
        signal_token.cancel();
    });
    let deadline = {
        let token = shutdown.clone();
        async move {
            token.cancelled().await;
            tokio::time::sleep(SHUTDOWN_GRACE).await;
        }
    };
    tokio::select! {
        result = serve => result.context("HTTP server failed")?,
        _ = deadline => tracing::warn!(
            grace_secs = SHUTDOWN_GRACE.as_secs(),
            "graceful shutdown deadline exceeded; exiting with connections still open"
        ),
    }
    shutdown.cancel();
    if let Some(server) = metrics_server {
        let _ = tokio::time::timeout(std::time::Duration::from_secs(3), server).await;
    }
    let _ = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        futures::future::join_all(background),
    )
    .await;
    tracing::info!("ORBIT API stopped");
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c().await.expect("ctrl-c handler");
    };
    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("SIGTERM handler")
            .recv()
            .await;
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
    tracing::info!("shutdown signal received");
}
