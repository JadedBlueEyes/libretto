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

use crate::{
    config::Config,
    session::{ClientSession, FullSession},
};
use clap::Parser;
use color_eyre::eyre::{self, Context};
use matrix_sdk::{SessionMeta, SessionTokens, authentication::matrix::MatrixSession};
use sqlx::{PgPool, postgres::PgPoolOptions};
use tracing::info;
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

    let data_dir = config.account_config.data_dir.clone().unwrap_or_else(|| {
        dirs::data_dir()
            .expect("no data_dir directory found")
            .join("libretto")
    });

    let maybe_session = sqlx::query!(
        r#"select user_id,
        device_id,
        access_token,
        refresh_token,
        homeserver_url,
        db_path,
        db_passphrase,
        next_batch
        from "account""#
    )
    .fetch_optional(&db)
    .await?;
    let (client, sync_token) = if let Some(session_res) = maybe_session {
        let session = FullSession {
            sync_token: session_res.next_batch,
            client_session: ClientSession {
                homeserver: session_res.homeserver_url,
                db_path: session_res.db_path,
                passphrase: session_res.db_passphrase,
            },
            user_session: MatrixSession {
                meta: SessionMeta {
                    user_id: session_res.user_id.try_into()?,
                    device_id: session_res.device_id.into(),
                },
                tokens: SessionTokens {
                    access_token: session_res.access_token,
                    refresh_token: None,
                },
            },
        };
        crate::session::restore_session(session, &data_dir).await?
    } else {
        let (client, session) = crate::auth::login(&data_dir, &config.account_config).await?;

        let _ = sqlx::query!(
            // language=PostgreSQL
            r#"
            insert into "account"(
                user_id,
                device_id,
                access_token,
                refresh_token,
                db_passphrase,
                homeserver_url,
                db_path,
                next_batch)
                values ($1, $2, $3, $4, $5, $6, $7, $8)
            "#,
            session.user_session.meta.user_id.to_string(),
            session.user_session.meta.device_id.to_string(),
            session.user_session.tokens.access_token,
            session.user_session.tokens.refresh_token,
            session.client_session.passphrase,
            session.client_session.homeserver,
            session.client_session.db_path,
            session.sync_token
        )
        .execute(&db)
        .await?;
        (client, session.sync_token)
    };

    client.event_cache().subscribe()?;

    crate::client::run(&client, sync_token, db.clone(), &config).await?;

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
            let sync_loop = client.sync_with_result_callback(
                matrix_sdk::config::SyncSettings::default(),
                |sync_result| async {
                    let response = sync_result?;

                    // We persist the token each time to be able to restore our session
                    crate::session::persist_sync_token(
                        db.clone(),
                        client.user_id().expect("to be logged in"),
                        response.next_batch.clone(),
                    )
                    .await
                    .map_err(|err| matrix_sdk::Error::UnknownError(err.into()))?;

                    Ok(matrix_sdk::LoopCtrl::Continue)
                },
            );

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
    Ok(())
}
