use crate::account::config::AccountDetails;
use color_eyre::eyre;
use ruma::{ServerName, UserId};

/// Selects the primary account from a list of accounts based on the provided primary_user_id.
///
/// # Arguments
/// * `accounts` - List of available accounts
/// * `primary_user_id` - Optional primary user ID to select. If None, returns first account.
///
/// # Returns
/// * `Ok(AccountDetails)` - The selected primary account
/// * `Err(eyre::Error)` - If no accounts provided or no matching account found
pub fn select_primary_account<'a>(
    accounts: &'a [AccountDetails],
    primary_user_id: Option<&str>,
) -> eyre::Result<&'a AccountDetails> {
    if accounts.is_empty() {
        return Err(eyre::eyre!("No accounts found in config file"));
    }

    let Some(primary_user_id) = primary_user_id else {
        return Ok(&accounts[0]);
    };

    // Try to find a matching account
    for account in accounts {
        let homeserver = account
            .homeserver
            .as_ref()
            .and_then(|s| <&ServerName>::try_from(s.as_str()).ok());
        if matches_account(&account.user_id, homeserver, primary_user_id) {
            return Ok(account);
        }
    }

    Err(eyre::eyre!(
        "No account found matching primary_user_id: '{}'. Available accounts: [{}]",
        primary_user_id,
        accounts
            .iter()
            .map(|a| a.user_id.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    ))
}

/// Check if an account matches the given primary user ID
fn matches_account(
    account_user_id: &str,
    account_homeserver: Option<&ServerName>,
    primary_user_id: &str,
) -> bool {
    // Direct string match
    if account_user_id == primary_user_id {
        return true;
    }

    // Try to construct full user ID from account and compare
    if let Ok(full_account_id) = construct_full_user_id(account_user_id, account_homeserver) {
        if full_account_id == primary_user_id {
            return true;
        }
    }

    false
}

/// Construct a full Matrix user ID from user_id and homeserver
fn construct_full_user_id(
    user_id: &str,
    homeserver: Option<&ServerName>,
) -> eyre::Result<ruma::OwnedUserId> {
    Ok(if let Some(homeserver) = homeserver {
        UserId::parse_with_server_name(user_id, homeserver)?
    } else {
        UserId::parse(user_id)?
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::account::config::{AccountDetails, AuthMethod};

    fn create_test_account(user_id: &str, homeserver: Option<&str>) -> AccountDetails {
        AccountDetails {
            user_id: user_id.to_string(),
            homeserver: homeserver.map(|s| s.to_string()),
            auth_method: AuthMethod::None,
            recovery_key: None,
            enable_encryption: true,
            device_name: None,
            set_device_name: false,
            delete_other_devices: false,
        }
    }

    #[test]
    fn test_empty_accounts_list() {
        let accounts = vec![];
        let result = select_primary_account(&accounts, None);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("No accounts found")
        );
    }

    #[test]
    fn test_no_primary_user_id_returns_first_account() {
        let accounts = vec![
            create_test_account("@alice:example.com", None),
            create_test_account("@bob:example.com", None),
        ];

        let result = select_primary_account(&accounts, None).unwrap();
        assert_eq!(result.user_id, "@alice:example.com");
    }

    #[test]
    fn test_exact_match_full_user_id() {
        let accounts = vec![
            create_test_account("@alice:example.com", None),
            create_test_account("@jade:ellis.link", None),
        ];

        let result = select_primary_account(&accounts, Some("@jade:ellis.link")).unwrap();
        assert_eq!(result.user_id, "@jade:ellis.link");
    }

    #[test]
    fn test_local_part_with_homeserver_in_config() {
        let accounts = vec![
            create_test_account("@alice:example.com", None),
            create_test_account("jade", Some("ellis.link")),
        ];

        let result = select_primary_account(&accounts, Some("@jade:ellis.link")).unwrap();
        assert_eq!(result.user_id, "jade");
        assert_eq!(result.homeserver.as_deref(), Some("ellis.link"));
    }

    #[test]
    fn test_primary_local_part() {
        let accounts = vec![
            create_test_account("@alice:example.com", None),
            create_test_account("jade", Some("ellis.link")),
        ];

        let result = select_primary_account(&accounts, Some("jade")).unwrap();
        assert_eq!(result.user_id, "jade");
    }

    #[test]
    fn test_invalid_primary_user_id() {
        let accounts = vec![create_test_account("@alice:example.com", None)];

        let result = select_primary_account(&accounts, Some("invalid-user-id"));
        assert!(result.is_err());
        let error_msg = result.unwrap_err().to_string();
        println!("Actual error message: {error_msg}");
        assert!(error_msg.contains("No account found"));
    }

    #[test]
    fn test_no_matching_account() {
        let accounts = vec![
            create_test_account("@alice:example.com", None),
            create_test_account("@bob:example.com", None),
        ];

        let result = select_primary_account(&accounts, Some("@charlie:example.com"));
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("No account found matching primary_user_id")
        );
    }

    #[test]
    fn test_account_selection_with_full_homeserver_url() {
        let accounts = vec![create_test_account(
            "jade",
            Some("https://matrix.ellis.link:443"),
        )];

        let result = select_primary_account(&accounts, Some("jade")).unwrap();
        assert_eq!(result.user_id, "jade");
        assert_eq!(
            result.homeserver.as_deref(),
            Some("https://matrix.ellis.link:443")
        );
    }

    #[test]
    fn test_localpart_with_no_homeserver() {
        let accounts = vec![create_test_account("jade", None)];

        let result = select_primary_account(&accounts, Some("@jade:ellis.link"));
        assert!(result.is_err());
    }
}
