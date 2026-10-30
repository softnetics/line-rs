//! Key/value storage used for tokens, certificates, E2EE keys and counters.

mod file;
mod memory;

pub use file::FileStorage;
pub use memory::MemoryStorage;

use async_trait::async_trait;

use crate::error::Result;

/// Values are plain JSON, matching what linejs persists.
pub type StorageValue = serde_json::Value;

#[async_trait]
pub trait Storage: Send + Sync {
    async fn set(&self, key: &str, value: StorageValue) -> Result<()>;
    async fn get(&self, key: &str) -> Result<Option<StorageValue>>;
    async fn delete(&self, key: &str) -> Result<()>;
    async fn clear(&self) -> Result<()>;
    /// Every stored entry.
    async fn get_all(&self) -> Result<serde_json::Map<String, StorageValue>>;

    /// Copy all entries into another storage.
    async fn migrate(&self, to: &dyn Storage) -> Result<()> {
        for (k, v) in self.get_all().await? {
            to.set(&k, v).await?;
        }
        Ok(())
    }

    /// `get` that only returns string values.
    async fn get_string(&self, key: &str) -> Result<Option<String>> {
        Ok(self.get(key).await?.and_then(|v| match v {
            serde_json::Value::String(s) => Some(s),
            _ => None,
        }))
    }
}
