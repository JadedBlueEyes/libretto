mod room_to_html;

use std::path::{Path, PathBuf};

use anyhow::Context;
use askama::Template;
use clap::Parser;
use futures::StreamExt;
use matrix_sdk::{
    Client, RoomMemberships,
    authentication::matrix::MatrixSession,
    config::SyncSettings,
    encryption::Encryption,
    room::{Messages, MessagesOptions},
    ruma::{
        OwnedRoomId, RoomAliasId,
        api::client::{
            filter::FilterDefinition,
            uiaa::{AuthData, Password, UserIdentifier},
        },
        assign,
    },
};
use rand::{Rng, distr::Alphanumeric};
use room_to_html::RoomTemplate;
use rpassword::prompt_password;
use serde::{Deserialize, Serialize};
use tokio::{fs, io::AsyncWriteExt};
use tracing::{error, info, trace, warn};
use tracing_log::AsTrace;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[derive(Parser, Debug)]
pub struct Config {
    #[clap(flatten)]
    pub account_config: AccountConfig,
    #[clap(flatten)]
    pub room_config: RoomConfig,

    #[clap(flatten)]
    pub(crate) verbose: clap_verbosity_flag::Verbosity,
}

#[derive(Parser, Debug)]
pub struct RoomConfig {
    /// The room ID to translate. Also accepts an alias.
    #[arg(long, env = "MATRIX_ROOM_ID")]
    pub room_id: String,
}

#[derive(Parser, Debug)]
pub struct AccountConfig {
    /// URL of the homeserver to connect to
    #[arg(short, long, env = "MATRIX_SERVER")]
    pub server: String,
    /// Username of the bot
    #[arg(short, long, env = "MATRIX_USERNAME")]
    pub username: String,
    /// Password of the bot
    #[arg(short, long, env = "MATRIX_PASSWORD")]
    pub password: Option<String>,
    /// Delete devices other than the one being used by this instance
    #[arg(long)]
    pub delete_other_devices: bool,
    /// Device name to set, if it doesn't exist
    #[arg(long, default_value_t = String::from("libretto client"), env = "MATRIX_CLIENT_NAME")]
    pub device_name: String,
    /// Set the device name, even if it already exists
    #[arg(long, default_value_t = false)]
    pub set_device_name: bool,

    /// Account recovery key
    #[arg(short, long, env = "MATRIX_ACCOUNT_RECOVERY_KEY")]
    pub recovery_key: Option<String>,

    /// Account data directory
    #[arg(short, long, env = "MATRIX_ACCOUNT_DATA_DIR")]
    pub data_dir: Option<PathBuf>,
}

/// The data needed to re-build a client.
#[derive(Debug, Serialize, Deserialize)]
struct ClientSession {
    /// The URL of the homeserver of the user.
    homeserver: String,

    /// The path of the database.
    db_path: std::path::PathBuf,

    /// The passphrase of the database.
    passphrase: String,
}

/// The full session to persist.
#[derive(Debug, Serialize, Deserialize)]
struct FullSession {
    /// The data to re-build the client.
    client_session: ClientSession,

    /// The Matrix user session.
    user_session: MatrixSession,

    /// The latest sync token.
    ///
    /// It is only needed to persist it when using `Client::sync_once()` and we
    /// want to make our syncs faster by not receiving all the initial sync
    /// again.
    #[serde(skip_serializing_if = "Option::is_none")]
    sync_token: Option<String>,
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    // Read args
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

    let data_dir = config.account_config.data_dir.clone().unwrap_or_else(|| {
        dirs::data_dir()
            .expect("no data_dir directory found")
            .join("libretto")
    });
    let session_file = data_dir.join("session");

    let (client, sync_token) = if session_file.exists() {
        restore_session(&session_file).await?
    } else {
        (
            login(&data_dir, &session_file, &config.account_config).await?,
            None,
        )
    };

    client.event_cache().subscribe()?;

    run(client, sync_token, &session_file, config).await?;

    Ok(())
}

/// Restore a previous session.
async fn restore_session(session_file: &Path) -> anyhow::Result<(Client, Option<String>)> {
    info!(
        "Previous session found in '{}'",
        session_file.to_string_lossy()
    );

    // The session was serialized as JSON in a file.
    let serialized_session = fs::read_to_string(session_file).await?;
    let FullSession {
        client_session,
        user_session,
        sync_token,
    } = serde_json::from_str(&serialized_session)?;

    // Build the client with the previous settings from the session.
    let client = Client::builder()
        .homeserver_url(client_session.homeserver)
        .sqlite_store(client_session.db_path, Some(&client_session.passphrase))
        .build()
        .await?;

    info!("Restoring session for {}…", user_session.meta.user_id);

    // Restore the Matrix user session.
    client.restore_session(user_session).await?;

    verify_device(client.encryption(), None).await?;

    Ok((client, sync_token))
}

/// Login to a new session.
async fn login(
    data_dir: &std::path::Path,
    session_file: &std::path::Path,
    config: &AccountConfig,
) -> anyhow::Result<Client> {
    info!("No previous session found, logging in…");
    let mut rng = rand::rng();

    // Generate a random passphrase.
    let passphrase: String = (&mut rng)
        .sample_iter(Alphanumeric)
        .take(32)
        .map(char::from)
        .collect();

    let db_subfolder: String = (&mut rng)
        .sample_iter(Alphanumeric)
        .take(7)
        .map(char::from)
        .collect();
    let db_path = data_dir.join(db_subfolder);

    let client = Client::builder()
        .homeserver_url(&config.server)
        .sqlite_store(&db_path, Some(&passphrase))
        .build()
        .await?;

    let client_session = ClientSession {
        homeserver: config.server.clone(),
        db_path,
        passphrase,
    };
    let matrix_auth = client.matrix_auth();

    loop {
        let username = &config.username;
        let password = config.password.clone().unwrap_or_else(|| {
            println!("Type password for the bot (characters won't show up as you type them)");
            match prompt_password("Password: ") {
                Ok(p) => p,
                Err(err) => {
                    panic!("FATAL: failed to get password: {err}");
                }
            }
        });

        match matrix_auth
            .login_username(username, &password)
            .initial_device_display_name(&config.device_name)
            .await
        {
            Ok(_) => {
                info!("Logged in as {username}");
                break;
            }
            Err(error) => {
                error!("Error logging in: {error}");
                if config.password.is_some() {
                    return Err(error.into());
                }
            }
        }
    }

    verify_device(client.encryption(), config.recovery_key.clone()).await?;

    // Persist the session to reuse it later.
    // This is not very secure, for simplicity. If the system provides a way of
    // storing secrets securely, it should be used instead.
    // Note that we could also build the user session from the login response.
    let user_session = matrix_auth
        .session()
        .expect("A logged-in client should have a session");
    let serialized_session = serde_json::to_string(&FullSession {
        client_session,
        user_session,
        sync_token: None,
    })?;
    fs::write(session_file, serialized_session).await?;

    info!("Session persisted in {}", session_file.to_string_lossy());

    Ok(client)
}

async fn verify_device(encryption: Encryption, recovery_key: Option<String>) -> anyhow::Result<()> {
    let device = encryption
        .get_own_device()
        .await?
        .expect("to have a device");

    if device.is_verified_with_cross_signing() {
        info!(
            "Device {} of user {} is verified",
            device.device_id(),
            device.user_id(),
        );
    } else {
        info!(
            "Device {} of user {} is not verified",
            device.device_id(),
            device.user_id(),
        );
        let recovery_key = recovery_key.unwrap_or_else(|| {
            println!("Type recovery key for the bot (characters won't show up as you type them)");
            match prompt_password("Recovery Key: ") {
                Ok(p) => p,
                Err(err) => {
                    panic!("FATAL: failed to get recovery key: {err}");
                }
            }
        });
        info!("Trying to recover device");
        encryption.recovery().recover(&recovery_key).await?;
    }
    encryption.wait_for_e2ee_initialization_tasks().await;
    Ok(())
}

async fn run(
    client: Client,
    initial_sync_token: Option<String>,
    session_file: &Path,
    config: Config,
) -> anyhow::Result<()> {
    // handler for autojoin
    // Handers here run for historic messages too
    // client.add_event_handler(crate::handlers::on_stripped_state_member);

    info!("Launching a first sync...");

    // Enable room members lazy-loading, it will speed up the initial sync a lot
    // with accounts in lots of rooms.
    // See <https://spec.matrix.org/v1.6/client-server-api/#lazy-loading-room-members>.
    let filter = FilterDefinition::with_lazy_loading();

    let mut sync_settings = SyncSettings::default().filter(filter.into());

    // We restore the sync where we left.
    // This is not necessary when not using `sync_once`. The other sync methods get
    // the sync token from the store.
    if let Some(sync_token) = initial_sync_token {
        sync_settings = sync_settings.token(sync_token);
    }

    // Let's ignore messages before the program was launched.
    // This is a loop in case the initial sync is longer than our timeout. The
    // server should cache the response and it will ultimately take less time to
    // receive.
    loop {
        match client.sync_once(sync_settings.clone()).await {
            Ok(response) => {
                // This is the last time we need to provide this token, the sync method after
                // will handle it on its own.
                sync_settings = sync_settings.token(response.next_batch.clone());
                persist_sync_token(session_file, response.next_batch).await?;
                break;
            }
            Err(error) => {
                warn!("An error occurred during initial sync: {error}");
            }
        }
    }
    info!("Initial sync done");

    let current_session = client.device_id().map(|d| d.to_owned());
    if config.account_config.delete_other_devices {
        info!(
            current_session = format!("{current_session:?}"),
            "Checking for other devices to delete"
        );
        let other_devices: Vec<_> = client
            .devices()
            .await?
            .devices
            .iter()
            .filter(|device| Some(&device.device_id) != current_session.as_ref())
            .map(|device| device.device_id.clone())
            .collect();
        if !other_devices.is_empty() {
            trace!(
                current_session = format!("{current_session:?}"),
                other_devices = format!("{other_devices:?}"),
                "Deleting other devices"
            );
            client
                .delete_devices(
                    &other_devices,
                    Some(AuthData::Password(Password::new(
                        UserIdentifier::UserIdOrLocalpart(config.account_config.username.clone()),
                        config.account_config.password.clone().unwrap_or_else(|| {
                            println!(
                            "Type password for the account (characters won't show up as you type them)"
                        );
                            match prompt_password("Password: ") {
                                Ok(p) => p,
                                Err(err) => {
                                    panic!("FATAL: failed to get password: {err}");
                                }
                            }
                        }),
                    ))),
                )
                .await?;
        }
    }

    if config.account_config.set_device_name {
        if let Some(current_session) = current_session {
            info!(
                current_session = format!("{current_session:?}"),
                "Renaming device to {}", &config.account_config.device_name
            );
            client
                .rename_device(&current_session, &config.account_config.device_name)
                .await?;
        } else {
            warn!("No device ID found, cannot name device");
        }
    }

    // We have done initial sync & setup, wait for new messages.

    // Write a function to parse and resolve a room alias, or a room ID from config.room_config.room_id
    // client.resolve_room_alias(alias)
    let room_id: OwnedRoomId =
        if let Ok(alias) = <&RoomAliasId>::try_from(config.room_config.room_id.as_str()) {
            client.resolve_room_alias(alias).await?.room_id
        } else {
            OwnedRoomId::try_from(config.room_config.room_id.as_str())
                .context("Room ID was not a valid ID or alias!")?
        };

    client
        .encryption()
        .backups()
        .download_room_keys_for_room(&room_id)
        .await
        .inspect_err(|e| {
            error!("Failed to download room keys for room {room_id}: {e}");
        })?;

    let room = client.get_room(&room_id).context("Failed to get room")?;
    let Messages {
        end: token,
        chunk: events,
        ..
    } = room
        .messages(assign!(MessagesOptions::backward(), {limit: 100u8.into()}))
        .await?;

    // let paginator = Paginator::new(room.clone());
    // paginator.start_from(event_id, num_events)
    // let PaginationResult { events, hit_end_of_timeline } = paginator.paginate_backward(100u8.into()).await?;
    let members = room.members(RoomMemberships::empty()).await?;
    let template = RoomTemplate {
        name: room
            .display_name()
            .await
            .map(|name| name.to_string())
            .unwrap_or("Unknown Room".to_owned()),
        room_id: &room_id,
        events: events.into_iter().rev().collect(),
        members: members.iter().map(|m| (m.user_id(), m)).collect(),
        hit_end_of_timeline: token.is_none(),
        room: &room,
    };

    let path = Path::new("./out/output.html");
    let mut file = fs::File::create(path).await?;
    let mut buf: Vec<u8> = Vec::new();
    template.write_into(&mut buf)?;
    file.write_all(&buf).await?;
    Ok(())
}

/// Persist the sync token for a future session.
/// Note that this is needed only when using `sync_once`. Other sync methods get
/// the sync token from the store.
async fn persist_sync_token(session_file: &Path, sync_token: String) -> anyhow::Result<()> {
    let serialized_session = fs::read_to_string(session_file).await?;
    let mut full_session: FullSession = serde_json::from_str(&serialized_session)?;

    full_session.sync_token = Some(sync_token);
    let serialized_session = serde_json::to_string(&full_session)?;
    fs::write(session_file, serialized_session).await?;

    Ok(())
}
