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

use crate::{config::Config, session::load_session_from_db};
use clap::Parser;
use color_eyre::eyre::{self, Context};
use matrix_sdk::{config::SyncSettings, sync::SyncResponse};
use sqlx::{PgPool, postgres::PgPoolOptions};
use tokio::time::sleep;
use tokio_stream::StreamExt;
use tracing::{error, info};
use tracing_log::AsTrace;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

type DatabasePool = PgPool;

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

        crate::session::insert_account_session(&db, &session).await?;
        (client, session.sync_token)
    };

    client.event_cache().subscribe()?;

    crate::client::run(&client, &config).await?;

    let app = crate::server::build_router(client.clone());

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
        async move {
            let user_id = client.user_id().expect("to be logged in");

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
                    let result: Result<SyncResponse, _> =
                        client.sync_once(sync_settings.clone()).await;

                    match result {
                        Ok(response) => {
                            backoff = None;
                            match crate::session::persist_sync_token(
                                db.clone(),
                                user_id,
                                response.next_batch.clone(),
                            )
                            .await
                            {
                                Ok(_) => {
                                    sync_settings = sync_settings.token(response.next_batch);
                                    if !(response.rooms.invited.is_empty()
                                        && response.rooms.joined.is_empty()
                                        && response.rooms.knocked.is_empty()
                                        && response.rooms.left.is_empty())
                                    {
                                        dbg!(user_id, response.rooms);
                                    }
                                }
                                Err(err) => error!("Failed to persist sync token: {err:?}"),
                            }
                        }
                        Err(err) => {
                            error!("Sync error: {}", err);
                            sleep(Duration::from_secs(backoff.unwrap_or(0))).await;
                            backoff = Some((backoff.unwrap_or(0) + 1).pow(2));
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
            let client = client.clone();
            let db = db.clone();

            let room_updates = tokio::spawn(async move {
                let (mut rooms, mut rooms_stream) = client.rooms_stream();

                room_list::update_rooms(
                    rooms
                        .iter()
                        .map(|r| r.to_owned())
                        .collect::<Vec<_>>()
                        .as_slice(),
                    client.user_id().expect("to be logged in"),
                    &db,
                )
                .await
                .unwrap();
                // Compare from database to find deleted and upsert the rest

                while let Some(room_changes) = rooms_stream.next().await {
                    info!("Rooms have been updated");
                    for diff in room_changes {
                        // match diff {
                        //     VectorDiff::Append { ref values } => {
                        //         // Add values
                        //     }
                        //     VectorDiff::Clear => {
                        //         // Delete all
                        //     }
                        //     VectorDiff::PushFront { ref value }
                        //     | VectorDiff::PushBack { ref value } => {
                        //         // Add value
                        //     }
                        //     VectorDiff::PopFront => {
                        //         // Remove 0
                        //     }
                        //     VectorDiff::PopBack => {
                        //         // Remove last
                        //     }
                        //     VectorDiff::Insert { index, ref value } => {
                        //         // Add last
                        //     }
                        //     VectorDiff::Set { index, ref value } => {
                        //         // Compare index - if the same room ID, update, else remove and insert
                        //     }
                        //     VectorDiff::Remove { index } => {
                        //         // Remove index
                        //     }
                        //     VectorDiff::Truncate { length } => {
                        //         // Remove from point to end
                        //     }
                        //     VectorDiff::Reset { ref values } => {
                        //         // Compare from database to find deleted and upsert the rest
                        //     }
                        // }
                        // In the meantime, just do a full update every time:

                        diff.apply(&mut rooms);
                        room_list::update_rooms(
                            rooms
                                .iter()
                                .map(|r| r.to_owned())
                                .collect::<Vec<_>>()
                                .as_slice(),
                            client.user_id().expect("to be logged in"),
                            &db,
                        )
                        .await
                        .unwrap();
                    }
                }
            });

            tokio::select! {
                _ = room_updates => info!("Room updates finished"),
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
    Ok(())
}
