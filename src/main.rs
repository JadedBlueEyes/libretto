mod room_list;
mod room_to_html;
mod timeline;

mod assets;
mod auth;
mod client;
mod config;
mod error;
mod server;
mod session;

use std::time::{Duration, Instant};

use crate::{client::sync_handler, config::Config, session::load_session_from_db};
use clap::Parser;
use color_eyre::eyre::{self, Context};
use futures::TryFutureExt;
use matrix_sdk::{config::SyncSettings, sync::SyncResponse};
use sqlx::postgres::PgPoolOptions;
use tokio::time::sleep;
use tracing::{error, info, warn};
use tracing_log::AsTrace;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

type Database = sqlx::Postgres;
type DatabasePool = sqlx::Pool<Database>;
type DatabaseConnection = <Database as sqlx::Database>::Connection;

#[tokio::main(flavor = "current_thread")]
async fn main() -> eyre::Result<()> {
    // Parse CLI config
    let config = Config::parse();

    // Logging
    let filter = tracing_subscriber::EnvFilter::builder()
        .with_default_directive(config.verbose.log_level_filter().as_trace().into())
        .from_env_lossy();
    tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer())
        .init();

    info!("Starting up");

    let db: DatabasePool = PgPoolOptions::new()
        .max_connections(20)
        .connect(&config.database_url)
        .await
        .context("Failed to connect to database")?;
    // db.begin().await

    sqlx::migrate!()
        .run(&db)
        .await
        .context("failed to run migrations")?;

    let data_dir = config
        .account_config
        .as_ref()
        .and_then(|ac| ac.data_dir.clone())
        .unwrap_or_else(|| {
            dirs::data_dir()
                .expect("no data_dir directory found")
                .join("libretto")
        });

    let maybe_session = load_session_from_db(&db).await?;
    let (client, sync_token) = if let Some(session) = maybe_session {
        crate::session::restore_session(session, &data_dir).await?
    } else {
        let account_config = config
            .account_config
            .as_ref()
            .expect("AccountConfig required for login");
        let (client, session) = crate::auth::login(&data_dir, account_config).await?;

        let _ = db
            .acquire()
            .map_err(|e| e.into())
            .and_then(async |mut tx| {
                crate::session::insert_account_session(&mut tx, &session).await
            })
            .await
            .inspect_err(|e| warn!("Error doing initial room update: {e}"));

        (client, session.sync_token)
    };

    client.event_cache().subscribe()?;

    crate::client::run(&client, &config).await?;

    let app = crate::server::build_router(client.clone(), db.clone());

    // try to first get a socket from listenfd, if that does not give us
    // one (eg: no systemd or systemfd), open on port 3000 instead.
    let mut listenfd = listenfd::ListenFd::from_env();
    let listener = match listenfd.take_tcp_listener(0).unwrap() {
        Some(listener) => {
            listener.set_nonblocking(true)?;
            tokio::net::TcpListener::from_std(listener)
        }
        None => tokio::net::TcpListener::bind("0.0.0.0:3000").await,
    }?;

    let signal = server::shutdown_signal();

    let sync_task = tokio::spawn({
        let client = client.clone();
        let db = db.clone();
        async move {
            let user_id = client.user_id().expect("to be logged in");

            let _ = db
                .acquire()
                .map_err(|e| e.into())
                .and_then(async |mut tx| {
                    room_list::update_rooms(
                        client.rooms().into_iter().collect::<Vec<_>>().as_slice(),
                        user_id,
                        &mut tx,
                    )
                    .await
                })
                .await
                .inspect_err(|e| warn!("Error doing initial room update: {e}"));

            let sync_loop = async {
                let mut last_sync_time: Option<Instant> = None;
                let filter =
                    matrix_sdk::ruma::api::client::filter::FilterDefinition::with_lazy_loading(); // Member lazy loading speeds up initial sync
                let mut sync_settings = SyncSettings::default().filter(filter.into());
                let mut backoff = None;
                if let Some(token) = sync_token {
                    sync_settings = sync_settings.token(token);
                }

                loop {
                    let result: eyre::Result<String> = client
                        .sync_once(sync_settings.clone())
                        .map_err(|e| e.into())
                        .and_then(async |response: SyncResponse| {
                            let tx = db.begin().await?;
                            Ok((response, tx))
                        })
                        .and_then(async |(response, mut tx)| {
                            backoff = None;
                            sync_handler(&mut tx, &client, user_id, &response).await?;
                            crate::session::persist_sync_token(
                                &mut tx,
                                user_id,
                                response.next_batch.clone(),
                            )
                            .await?;
                            Ok((response, tx))
                        })
                        .and_then(async |(response, tx)| {
                            tx.commit().await?;

                            Ok(response.next_batch)
                        })
                        .await;

                    match result {
                        Ok(next_batch) => {
                            // trace!("Sync completed");
                            sync_settings = sync_settings.token(next_batch);
                        }
                        Err(err) => {
                            error!("Sync error: {}", err);
                            warn!("Sleeping {} seconds", 2u64.pow(backoff.unwrap_or(0)));
                            sleep(Duration::from_secs(2u64.pow(backoff.unwrap_or(0)))).await;
                            backoff = Some((backoff.unwrap_or(0) + 1).min(7));
                            continue;
                        }
                    }
                    let now = Instant::now();

                    if let Some(t) = last_sync_time {
                        let duration = now - t;
                        if duration <= Duration::from_secs(1) {
                            sleep(Duration::from_secs(1) - duration).await;
                        }
                    }

                    last_sync_time = Some(now);
                }
            };

            tokio::select! {
                _ = sync_loop => info!("Sync loop finished"),
                _ = signal => info!("Sync shutdown in progress"),
            }
        }
    });

    info!(listener = ?listener,  "Serving!");
    axum::serve(listener, app)
        .with_graceful_shutdown(crate::server::shutdown_signal())
        .await?;
    sync_task.await?;
    db.close().await;
    Ok(())
}
