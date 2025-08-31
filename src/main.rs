#![recursion_limit = "256"]

mod room_list;
mod room_to_html;
mod timeline;

pub mod account;
mod assets;
mod auth;
mod client;
pub mod config;
mod error;

mod server;
mod session;

use std::str::FromStr;

use crate::{
    client::run_sync_tasks,
    config::{Command, CommandConfig, UtilCommand},
    server::serve,
};
use clap::Parser;
use color_eyre::eyre::{self, Context};
use sqlx::postgres::PgPoolOptions;
use tracing::{debug, info, instrument};
use tracing_log::AsTrace;
use tracing_subscriber::{
    EnvFilter, filter::Directive, layer::SubscriberExt, util::SubscriberInitExt,
};

type Database = sqlx::Postgres;
type DatabasePool = sqlx::Pool<Database>;
type DatabaseConnection = <Database as sqlx::Database>::Connection;

#[tokio::main(flavor = "current_thread")]
async fn main() -> eyre::Result<()> {
    // Setup
    let (config, db, data_dir, cache_dir) = setup().await?;

    // Check if we have a utility command
    if let Some(Command::Util(util_cmd)) = &config.command {
        return handle_util_command(util_cmd, &db).await;
    }

    info!(version = env!("CARGO_PKG_VERSION"), "Starting libretto");

    // Run web server and sync tasks independently
    let (web_server_result, sync_tasks_result) = tokio::join!(
        serve(db.clone()),
        run_sync_tasks(config.config_file.clone(), &db, &data_dir, &cache_dir)
    );

    // Handle results
    web_server_result?;
    sync_tasks_result?;
    // Cleanup
    db.close().await;
    info!("Libretto shut down gracefully");
    Ok(())
}

/// Handle utility subcommands
async fn handle_util_command(cmd: &UtilCommand, db: &DatabasePool) -> eyre::Result<()> {
    match cmd {
        UtilCommand::RemoveNextBatch { user_id } => {
            remove_next_batch(db, user_id.as_deref()).await?;
        }
        UtilCommand::DeletePrevBatchAndTimelines { user_id } => {
            delete_prev_batch_and_timelines(db, user_id.as_deref()).await?;
        }
    }
    Ok(())
}

/// Remove next_batch tokens from accounts
async fn remove_next_batch(db: &DatabasePool, user_id: Option<&str>) -> eyre::Result<()> {
    let affected_rows = if let Some(user_id) = user_id {
        info!("Removing next_batch for account: {}", user_id);
        sqlx::query!(
            "UPDATE account SET next_batch = NULL WHERE user_id = $1",
            user_id
        )
        .execute(db)
        .await?
        .rows_affected()
    } else {
        info!("Removing next_batch for all accounts");
        sqlx::query!("UPDATE account SET next_batch = NULL")
            .execute(db)
            .await?
            .rows_affected()
    };

    info!("Updated {} account(s)", affected_rows);
    Ok(())
}

/// Delete prev_batch tokens and all timelines
async fn delete_prev_batch_and_timelines(
    db: &DatabasePool,
    user_id: Option<&str>,
) -> eyre::Result<()> {
    if let Some(user_id) = user_id {
        info!("Deleting prev_batch and timelines for account: {}", user_id);

        // Delete timelines for the specific user
        let timeline_rows = sqlx::query!("DELETE FROM timeline WHERE user_id = $1", user_id)
            .execute(db)
            .await?
            .rows_affected();

        // Remove prev_batch for rooms of the specific user
        let room_rows = sqlx::query!(
            "UPDATE room SET prev_batch = NULL WHERE user_id = $1",
            user_id
        )
        .execute(db)
        .await?
        .rows_affected();

        info!(
            "Deleted {} timeline entries and updated {} room(s)",
            timeline_rows, room_rows
        );
    } else {
        info!("Deleting prev_batch and timelines for all accounts");

        // Delete all timelines
        let timeline_rows = sqlx::query!("DELETE FROM timeline")
            .execute(db)
            .await?
            .rows_affected();

        // Remove all prev_batch tokens
        let room_rows = sqlx::query!("UPDATE room SET prev_batch = NULL")
            .execute(db)
            .await?
            .rows_affected();

        info!(
            "Deleted {} timeline entries and updated {} room(s)",
            timeline_rows, room_rows
        );
    }

    Ok(())
}

/// Setup application configuration, database, and directories
#[instrument(level = "info")]
async fn setup() -> eyre::Result<(
    CommandConfig,
    DatabasePool,
    std::path::PathBuf,
    std::path::PathBuf,
)> {
    // Parse CLI config
    let config = CommandConfig::parse();

    let base_filter: EnvFilter = EnvFilter::builder()
        .with_default_directive(config.verbose.log_level_filter().as_trace().into())
        .from_env_lossy();

    // Setup logging
    let forest_filter = base_filter
        .add_directive(
            Directive::from_str("tonic=info").expect("Failed to set tonic logging to info"),
        )
        .add_directive(Directive::from_str("h2=info").expect("Failed to set h2 logging to info"))
        .add_directive(
            Directive::from_str("hyper=info").expect("Failed to set hyper logging to info"),
        );

    let forest_layer = tracing_forest::ForestLayer::default();

    tracing_subscriber::registry()
        .with(forest_filter)
        .with(forest_layer)
        .init();

    debug!("Initializing libretto");

    // Connect to database
    let db: DatabasePool = PgPoolOptions::new()
        .max_connections(20)
        .connect(&config.database_url)
        .await
        .context("Failed to connect to database")?;

    // Run migrations
    sqlx::migrate!()
        .run(&db)
        .await
        .context("Failed to run database migrations")?;

    // Setup directories
    let data_dir = config.data_dir.clone().unwrap_or_else(|| {
        dirs::data_dir()
            .expect("No data directory found")
            .join("libretto")
    });
    let cache_dir = config.cache_dir.clone().unwrap_or_else(|| {
        dirs::cache_dir()
            .expect("No cache directory found")
            .join("libretto")
    });

    Ok((config, db, data_dir, cache_dir))
}
