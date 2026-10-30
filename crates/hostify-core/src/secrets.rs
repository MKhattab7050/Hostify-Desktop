use thiserror::Error;

#[derive(Debug, Error)]
pub enum SecretError {
    #[error("Credential store error: {0}")]
    Store(#[from] keyring::Error),
    #[error("Credential store is unavailable")]
    Unavailable,
}
pub trait SecretStore: Send + Sync {
    fn set(&self, key: &str, value: &str) -> Result<(), SecretError>;
    fn get(&self, key: &str) -> Result<Option<String>, SecretError>;
    fn delete(&self, key: &str) -> Result<(), SecretError>;
}
#[derive(Default)]
pub struct WindowsCredentialStore;
impl SecretStore for WindowsCredentialStore {
    fn set(&self, key: &str, value: &str) -> Result<(), SecretError> { keyring::Entry::new("Hostify", key)?.set_password(value)?; Ok(()) }
    fn get(&self, key: &str) -> Result<Option<String>, SecretError> { match keyring::Entry::new("Hostify", key)?.get_password() { Ok(v) => Ok(Some(v)), Err(keyring::Error::NoEntry) => Ok(None), Err(e) => Err(e.into()) } }
    fn delete(&self, key: &str) -> Result<(), SecretError> { match keyring::Entry::new("Hostify", key)?.delete_credential() { Ok(()) | Err(keyring::Error::NoEntry) => Ok(()), Err(e) => Err(e.into()) } }
}
#[cfg(test)]
#[derive(Default)]
pub struct MemoryStore(pub std::sync::Mutex<std::collections::HashMap<String, String>>);
#[cfg(test)]
impl SecretStore for MemoryStore {
    fn set(&self, key: &str, value: &str) -> Result<(), SecretError> { self.0.lock().map_err(|_| SecretError::Unavailable)?.insert(key.into(), value.into()); Ok(()) }
    fn get(&self, key: &str) -> Result<Option<String>, SecretError> { Ok(self.0.lock().map_err(|_| SecretError::Unavailable)?.get(key).cloned()) }
    fn delete(&self, key: &str) -> Result<(), SecretError> { self.0.lock().map_err(|_| SecretError::Unavailable)?.remove(key); Ok(()) }
}
