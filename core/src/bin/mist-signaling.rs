use mist_core::server::{serve, AppState, Config};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let filter = tracing_subscriber::EnvFilter::try_from_env("MIST_LOG").unwrap_or_else(|_| "info".into());
    if std::env::var("MIST_LOG_FORMAT").as_deref() == Ok("json") {
        tracing_subscriber::fmt().json().with_env_filter(filter).init();
    } else {
        tracing_subscriber::fmt().with_env_filter(filter).init();
    }
    let config = Config::from_env()?;
    tracing::info!(
        bind = %config.bind,
        tls = config.tls_cert.is_some(),
        stun = config.stun_urls.len(),
        turn = config.turn_urls.len(),
        access_token = config.access_token.is_some(),
        "starting {}",
        mist_core::server::SERVER_VERSION
    );
    let listener = std::net::TcpListener::bind(config.bind)?;
    let state = AppState::new(config);
    tokio::select! {
        r = serve(state, listener) => r,
        _ = tokio::signal::ctrl_c() => {
            tracing::info!("shutting down");
            Ok(())
        }
    }
}
