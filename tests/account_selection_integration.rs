use color_eyre::eyre;
use config::{Config, File};
use libretto::account::selection::select_primary_account;
use libretto::config::ConfigFile;
use std::io::Write;
use tempfile::Builder;

#[test]
fn test_user_example_config_parsing_and_selection() -> eyre::Result<()> {
    // Create a temporary config file with the exact format from the user's example
    let config_content = r#"
primary_user_id = "@jade:ellis.link"

[[accounts]]
user_id = "jade"
homeserver = "ellis.link"
auth_method = "None"
enable_encryption = true
"#;

    let mut temp_file = Builder::new().suffix(".toml").tempfile()?;
    temp_file.write_all(config_content.as_bytes())?;
    temp_file.flush()?;

    // Parse the config file using the same logic as the main application
    let config_file: ConfigFile = Config::builder()
        .add_source(File::from(temp_file.path()))
        .build()?
        .try_deserialize()?;

    // Verify the config was parsed correctly
    assert_eq!(
        config_file.primary_user_id,
        Some("@jade:ellis.link".to_string())
    );
    assert_eq!(config_file.accounts.len(), 1);

    let account = &config_file.accounts[0];
    assert_eq!(account.user_id, "jade");
    assert_eq!(account.homeserver, Some("ellis.link".to_string()));
    assert!(account.enable_encryption);

    // Test account selection with the parsed config
    let selected_account = select_primary_account(
        &config_file.accounts,
        config_file.primary_user_id.as_deref(),
    )?;

    // Verify the correct account was selected
    assert_eq!(selected_account.user_id, "jade");
    assert_eq!(selected_account.homeserver.as_deref(), Some("ellis.link"));
    assert!(selected_account.enable_encryption);

    Ok(())
}

#[test]
fn test_config_with_multiple_accounts() -> eyre::Result<()> {
    let config_content = r#"
primary_user_id = "@bob:matrix.org"

[[accounts]]
user_id = "@alice:example.com"
auth_method = "None"
enable_encryption = true

[[accounts]]
user_id = "bob"
homeserver = "matrix.org"
auth_method = "None"
enable_encryption = false

[[accounts]]
user_id = "charlie"
homeserver = "synapse.example.net"
auth_method = "None"
enable_encryption = true
"#;

    let mut temp_file = Builder::new().suffix(".toml").tempfile()?;
    temp_file.write_all(config_content.as_bytes())?;
    temp_file.flush()?;

    let config_file: ConfigFile = Config::builder()
        .add_source(File::from(temp_file.path()))
        .build()?
        .try_deserialize()?;

    // Should select the second account (bob@matrix.org)
    let selected_account = select_primary_account(
        &config_file.accounts,
        config_file.primary_user_id.as_deref(),
    )?;

    assert_eq!(selected_account.user_id, "bob");
    assert_eq!(selected_account.homeserver.as_deref(), Some("matrix.org"));
    assert!(!selected_account.enable_encryption);

    Ok(())
}

#[test]
fn test_config_without_primary_user_id() -> eyre::Result<()> {
    let config_content = r#"
[[accounts]]
user_id = "@first:example.com"
auth_method = "None"
enable_encryption = true

[[accounts]]
user_id = "@second:example.com"
auth_method = "None"
enable_encryption = true
"#;

    let mut temp_file = Builder::new().suffix(".toml").tempfile()?;
    temp_file.write_all(config_content.as_bytes())?;
    temp_file.flush()?;

    let config_file: ConfigFile = Config::builder()
        .add_source(File::from(temp_file.path()))
        .build()?
        .try_deserialize()?;

    // Should select the first account when no primary_user_id is specified
    let selected_account = select_primary_account(
        &config_file.accounts,
        config_file.primary_user_id.as_deref(),
    )?;

    assert_eq!(selected_account.user_id, "@first:example.com");

    Ok(())
}

#[test]
fn test_config_parsing_error_handling() {
    // Test invalid primary_user_id
    let config_content = r#"
primary_user_id = "@nonexistent:example.com"

[[accounts]]
user_id = "@alice:example.com"
auth_method = "None"
enable_encryption = true
"#;

    let mut temp_file = Builder::new().suffix(".toml").tempfile().unwrap();
    temp_file.write_all(config_content.as_bytes()).unwrap();
    temp_file.flush().unwrap();

    let config_file: ConfigFile = Config::builder()
        .add_source(File::from(temp_file.path()))
        .build()
        .unwrap()
        .try_deserialize()
        .unwrap();

    let result = select_primary_account(
        &config_file.accounts,
        config_file.primary_user_id.as_deref(),
    );

    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("No account found matching primary_user_id")
    );
}

#[test]
fn test_config_with_full_homeserver_url() -> eyre::Result<()> {
    let config_content = r#"
primary_user_id = "jade"

[[accounts]]
user_id = "jade"
homeserver = "https://matrix.ellis.link:8448"
auth_method = "None"
enable_encryption = true
"#;

    let mut temp_file = Builder::new().suffix(".toml").tempfile()?;
    temp_file.write_all(config_content.as_bytes())?;
    temp_file.flush()?;

    let config_file: ConfigFile = Config::builder()
        .add_source(File::from(temp_file.path()))
        .build()?
        .try_deserialize()?;

    // Test that the full URL homeserver works correctly
    let selected_account = select_primary_account(
        &config_file.accounts,
        config_file.primary_user_id.as_deref(),
    )?;

    assert_eq!(selected_account.user_id, "jade");
    assert_eq!(
        selected_account.homeserver.as_deref(),
        Some("https://matrix.ellis.link:8448")
    );

    Ok(())
}
