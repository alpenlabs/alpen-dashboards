use std::{net::SocketAddr, sync::Arc, time::Duration};

use anyhow::Result;
use axum::{routing::get, Router};
use status_bridge::{
    bridge_monitoring_task, deposit_status_monitoring_task, get_bridge_status,
    run_bitcoin_observer, run_withdrawal_indexer, BitcoinObserverDbSled, BitcoinObserverStatus,
    BitcoinParams, BridgeMonitoringContext, BridgeStatusDbSled, DepositStatusReader,
    WithdrawalIndexerDbSled,
};
use status_config::Config;
use status_network::{get_network_status, network_monitoring_task, NetworkMonitoringContext};
use strata_tasks::TaskManager;
use tokio::{net::TcpListener, runtime, sync::watch};
use tower_http::cors::{Any, CorsLayer};
use tracing::info;

const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

fn main() -> Result<()> {
    tracing_subscriber::fmt::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let runtime = runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("status-dashboard-rt")
        .build()?;

    let config_path = std::env::var("CONFIG_PATH").unwrap_or_else(|_| "config.toml".to_string());
    let config = Arc::new(Config::load_from_path(&config_path));

    let task_manager = TaskManager::new(runtime.handle().clone());
    let executor = task_manager.create_executor();

    let withdrawal_index_db = Arc::new(WithdrawalIndexerDbSled::open(config.datadir())?);
    let bridge_status_db = Arc::new(BridgeStatusDbSled::open(config.datadir())?);
    let network_context = Arc::new(NetworkMonitoringContext::new(config.network().clone()));
    let bitcoin_params = BitcoinParams::from_path(config.params_path())?;
    let bitcoin_observer_db = Arc::new(BitcoinObserverDbSled::open(config.datadir())?);
    let (observer_status, observer_status_rx) = watch::channel(BitcoinObserverStatus::default());
    let deposit_status_reader = DepositStatusReader::new(
        config.bitcoin_observer(),
        &bitcoin_params,
        config.bridge().max_tx_confirmations(),
        Arc::clone(&bitcoin_observer_db),
        observer_status_rx,
    );
    let bridge_context = Arc::new(BridgeMonitoringContext::new(
        config.bridge().clone(),
        Arc::clone(&withdrawal_index_db),
        Arc::clone(&bridge_status_db),
    )?);

    let cors = CorsLayer::new().allow_origin(Any);
    let app = Router::new()
        .route(
            "/api/status",
            get({
                let network_context = Arc::clone(&network_context);
                move || get_network_status(Arc::clone(&network_context))
            }),
        )
        .route(
            "/api/bridge_status",
            get({
                let bridge_context = Arc::clone(&bridge_context);
                move || get_bridge_status(Arc::clone(&bridge_context))
            }),
        )
        .layer(cors);

    let addr = SocketAddr::from((
        config.server().host().parse::<std::net::IpAddr>()?,
        config.server().port(),
    ));

    executor.spawn_critical_async_with_shutdown("network-monitoring", {
        let network_context = Arc::clone(&network_context);
        move |shutdown| async move { network_monitoring_task(network_context, shutdown).await }
    });

    executor.spawn_critical_async_with_shutdown("withdrawal-indexer", {
        let withdrawal_index_db = Arc::clone(&withdrawal_index_db);
        let cfg = config.withdrawal_indexer().clone();
        move |shutdown| async move { run_withdrawal_indexer(withdrawal_index_db, cfg, shutdown).await }
    });

    executor.spawn_critical_async_with_shutdown("bridge-monitoring", {
        let bridge_context = Arc::clone(&bridge_context);
        move |shutdown| async move { bridge_monitoring_task(bridge_context, shutdown).await }
    });

    executor.spawn_critical_async_with_shutdown("deposit-status", {
        let bridge_context = Arc::clone(&bridge_context);
        move |shutdown| async move {
            deposit_status_monitoring_task(bridge_context, deposit_status_reader, shutdown).await
        }
    });

    executor.spawn_critical_async_with_shutdown("bitcoin-observer", {
        let bitcoin = config.bitcoin().clone();
        let observer = config.bitcoin_observer().clone();
        let max_tx_confirmations = config.bridge().max_tx_confirmations();
        move |shutdown| async move {
            run_bitcoin_observer(
                bitcoin_observer_db,
                bitcoin,
                observer,
                bitcoin_params,
                max_tx_confirmations,
                observer_status,
                shutdown,
            )
            .await
            .map_err(|error| anyhow::anyhow!("Bitcoin observer failed: {error}"))?;
            Ok(())
        }
    });

    executor.spawn_critical_async_with_shutdown("http-server", {
        move |shutdown| async move {
            let listener = TcpListener::bind(addr).await?;
            info!(%addr, "Server running at http://{}", addr);
            let shutdown_future = async move {
                shutdown.wait_for_shutdown().await;
            };

            axum::serve(listener, app)
                .with_graceful_shutdown(shutdown_future)
                .await?;

            Ok(())
        }
    });

    task_manager.start_signal_listeners();
    task_manager.monitor(Some(SHUTDOWN_TIMEOUT))?;

    info!("Exiting status dashboard backend");
    Ok(())
}
