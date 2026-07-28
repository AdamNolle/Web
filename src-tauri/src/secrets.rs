use keyring::Entry;

#[derive(Debug, thiserror::Error)]
pub enum SecretStoreError {
    #[error("the operating-system credential vault is unavailable")]
    Unavailable,
    #[error("the credential reference is invalid")]
    InvalidReference,
}

pub trait SecretStore: Send + Sync {
    fn put(&self, reference: &str, secret: &str) -> Result<(), SecretStoreError>;
    fn get(&self, reference: &str) -> Result<String, SecretStoreError>;
    fn delete(&self, reference: &str) -> Result<(), SecretStoreError>;
}

/// Production adapter backed by the current user's operating-system credential vault.
/// It never falls back to files, preferences, environment variables, or SQLite.
pub struct OsSecretStore;

impl OsSecretStore {
    const SERVICE: &'static str = "local.web.digest";

    fn entry(reference: &str) -> Result<Entry, SecretStoreError> {
        validate_reference(reference)?;
        Entry::new(Self::SERVICE, reference).map_err(|_| SecretStoreError::Unavailable)
    }
}

impl SecretStore for OsSecretStore {
    fn put(&self, reference: &str, secret: &str) -> Result<(), SecretStoreError> {
        validate_secret(secret)?;
        Self::entry(reference)?
            .set_password(secret)
            .map_err(|_| SecretStoreError::Unavailable)
    }

    fn get(&self, reference: &str) -> Result<String, SecretStoreError> {
        let secret = Self::entry(reference)?
            .get_password()
            .map_err(|_| SecretStoreError::Unavailable)?;
        validate_secret(&secret)?;
        Ok(secret)
    }

    fn delete(&self, reference: &str) -> Result<(), SecretStoreError> {
        map_delete_result(Self::entry(reference)?.delete_credential())
    }
}

fn map_delete_result(result: Result<(), keyring::Error>) -> Result<(), SecretStoreError> {
    match result {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(_) => Err(SecretStoreError::Unavailable),
    }
}

fn validate_secret(secret: &str) -> Result<(), SecretStoreError> {
    // Windows Credential Manager caps credential blobs at 2,560 bytes. Keep a
    // conservative cross-platform UTF-8 ceiling so every accepted value is portable.
    if secret.is_empty() || secret.len() > 2_048 {
        return Err(SecretStoreError::InvalidReference);
    }
    Ok(())
}

fn validate_reference(reference: &str) -> Result<(), SecretStoreError> {
    if reference.is_empty()
        || reference.len() > 128
        || !reference
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
    {
        return Err(SecretStoreError::InvalidReference);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_byte_ceiling_is_portable_and_utf8_aware() {
        assert!(validate_secret(&"a".repeat(2_048)).is_ok());
        assert!(validate_secret(&"a".repeat(2_049)).is_err());
        assert!(validate_secret(&"é".repeat(1_024)).is_ok());
        assert!(validate_secret(&"é".repeat(1_025)).is_err());
    }

    #[test]
    fn missing_vault_entry_is_an_idempotent_delete_success() {
        assert!(map_delete_result(Ok(())).is_ok());
        assert!(map_delete_result(Err(keyring::Error::NoEntry)).is_ok());
        assert!(
            map_delete_result(Err(keyring::Error::NoStorageAccess(Box::new(
                std::io::Error::other("locked")
            ),)))
            .is_err()
        );
    }

    #[test]
    fn invalid_secret_references_fail_before_vault_access() {
        for reference in ["", "../token", "account token", "a/b"] {
            assert!(validate_reference(reference).is_err());
        }
        assert!(validate_reference("source_01-refresh").is_ok());
    }
}
