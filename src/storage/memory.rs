use std::collections::HashMap;

use async_trait::async_trait;
use tokio::sync::RwLock;

use super::{Storage, StorageValue};
use crate::error::Result;

/// In-memory storage (the default). Lost when the process exits.
#[derive(Default)]
pub struct MemoryStorage {
    data: RwLock<HashMap<String, StorageValue>>,
}

impl MemoryStorage {
    pub fn new() -> Self {
        Self::default()
    }

    /// Create with initial data.
    pub fn with_data(data: impl IntoIterator<Item = (String, StorageValue)>) -> Self {
        Self { data: RwLock::new(data.into_iter().collect()) }
    }
}

#[async_trait]
impl Storage for MemoryStorage {
    async fn set(&self, key: &str, value: StorageValue) -> Result<()> {
        self.data.write().await.insert(key.to_string(), value);
        Ok(())
    }
    async fn get(&self, key: &str) -> Result<Option<StorageValue>> {
        Ok(self.data.read().await.get(key).cloned())
    }
    async fn delete(&self, key: &str) -> Result<()> {
        self.data.write().await.remove(key);
        Ok(())
    }
    async fn clear(&self) -> Result<()> {
        self.data.write().await.clear();
        Ok(())
    }
    async fn get_all(&self) -> Result<serde_json::Map<String, StorageValue>> {
        Ok(self.data.read().await.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
    }
}
