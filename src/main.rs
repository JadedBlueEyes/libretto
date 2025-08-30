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

use crate::{client::run_sync_tasks, config::CommandConfig, server::serve};
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
