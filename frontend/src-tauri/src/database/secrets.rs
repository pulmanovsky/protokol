//! Cloud API keys (Anthropic, OpenAI) live in the macOS Keychain, not in SQLite.
//! On other platforms these functions report "not handled" and the settings table
//! keeps storing the keys as before.

/// Keychain service name for all Протокол secrets
#[cfg(target_os = "macos")]
const SERVICE: &str = "Protokol";

/// Keychain account for a provider whose key is kept out of the database
pub fn keychain_account(provider: &str) -> Option<&'static str> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    match provider {
        "claude" => Some("anthropic_api_key"),
        "openai" => Some("openai_api_key"),
        _ => None,
    }
}

#[cfg(target_os = "macos")]
fn entry(account: &str) -> Result<keyring::Entry, String> {
    keyring::Entry::new(SERVICE, account).map_err(|e| format!("Keychain: {}", e))
}

#[cfg(target_os = "macos")]
pub fn get_secret(account: &str) -> Result<Option<String>, String> {
    match entry(account)?.get_password() {
        Ok(value) if !value.is_empty() => Ok(Some(value)),
        Ok(_) | Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(format!("Keychain: {}", e)),
    }
}

#[cfg(target_os = "macos")]
pub fn set_secret(account: &str, value: &str) -> Result<(), String> {
    if value.is_empty() {
        return delete_secret(account);
    }
    entry(account)?.set_password(value).map_err(|e| format!("Keychain: {}", e))
}

#[cfg(target_os = "macos")]
pub fn delete_secret(account: &str) -> Result<(), String> {
    match entry(account)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(format!("Keychain: {}", e)),
    }
}

#[cfg(not(target_os = "macos"))]
pub fn get_secret(_account: &str) -> Result<Option<String>, String> {
    Ok(None)
}

#[cfg(not(target_os = "macos"))]
pub fn set_secret(_account: &str, _value: &str) -> Result<(), String> {
    Err("Keychain is only used on macOS".to_string())
}

#[cfg(not(target_os = "macos"))]
pub fn delete_secret(_account: &str) -> Result<(), String> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_cloud_providers_use_the_keychain() {
        let expected = cfg!(target_os = "macos");
        assert_eq!(keychain_account("claude").is_some(), expected);
        assert_eq!(keychain_account("openai").is_some(), expected);
        assert_eq!(keychain_account("ollama"), None);
        assert_eq!(keychain_account("builtin-ai"), None);
    }
}
