use crate::settings::Provider;

const SERVICE: &str = "ViDub";

fn entry(provider: Provider) -> anyhow::Result<keyring::Entry> {
    let user = provider.key_user();
    keyring::Entry::new(SERVICE, user)
        .map_err(|e| anyhow::anyhow!("keyring entry {SERVICE}/{user}: {e}"))
}

#[must_use]
pub fn stored_key(provider: Provider) -> Option<String> {
    let entry = match entry(provider) {
        Ok(e) => e,
        Err(e) => {
            tracing::warn!("vidub_config: keyring unavailable: {e:#}; callers fall back to env");
            return None;
        }
    };
    match entry.get_password() {
        Ok(k) => Some(k),
        Err(keyring::Error::NoEntry) => None,
        Err(e) => {
            tracing::warn!("vidub_config: keyring read failed: {e}; callers fall back to env");
            None
        }
    }
}

pub fn store_key(provider: Provider, key: &str) -> anyhow::Result<()> {
    if key.is_empty() {
        return clear_key(provider);
    }
    entry(provider)?
        .set_password(key)
        .map_err(|e| anyhow::anyhow!("keyring store: {e}"))
}

pub fn clear_key(provider: Provider) -> anyhow::Result<()> {
    match entry(provider)?.delete_credential() {
        Ok(()) => Ok(()),
        Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(anyhow::anyhow!("keyring delete: {e}")),
    }
}
