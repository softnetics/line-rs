use std::path::{Path, PathBuf};

use async_trait::async_trait;
use tokio::sync::Mutex;

use super::{Storage, StorageValue};
use crate::error::Result;

/// JSON file storage. Every write rewrites the whole file, serialized by a
/// lock, and goes through a temp file + rename so a crash cannot truncate it.
pub struct FileStorage {
    path: PathBuf,
    lock: Mutex<()>,
}

impl FileStorage {
    /// Open (or create with `{}`) the storage file at `path`.
    pub fn new(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        if !path.exists() {
            std::fs::write(&path, "{}")?;
        }
        Ok(Self { path, lock: Mutex::new(()) })
    }

    async fn read(&self) -> Result<serde_json::Map<String, StorageValue>> {
        let text = match tokio::fs::read_to_string(&self.path).await {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(e.into()),
        };
        if text.trim().is_empty() {
            return Ok(Default::default());
        }
        Ok(serde_json::from_str(&text)?)
    }

    async fn write(&self, data: &serde_json::Map<String, StorageValue>) -> Result<()> {
        let tmp = self.path.with_extension("tmp");
        tokio::fs::write(&tmp, serde_json::to_vec(data)?).await?;
        tokio::fs::rename(&tmp, &self.path).await?;
        Ok(())
    }
}

#[async_trait]
impl Storage for FileStorage {
    async fn set(&self, key: &str, value: StorageValue) -> Result<()> {
        let _g = self.lock.lock().await;
        let mut data = self.read().await?;
        data.insert(key.to_string(), value);
        self.write(&data).await
    }
    async fn get(&self, key: &str) -> Result<Option<StorageValue>> {
        Ok(self.read().await?.remove(key))
    }
    async fn delete(&self, key: &str) -> Result<()> {
        let _g = self.lock.lock().await;
        let mut data = self.read().await?;
        data.remove(key);
        self.write(&data).await
    }
    async fn clear(&self) -> Result<()> {
        let _g = self.lock.lock().await;
        self.write(&Default::default()).await
    }
    async fn get_all(&self) -> Result<serde_json::Map<String, StorageValue>> {
        self.read().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn file_roundtrip() {
        let dir = std::env::temp_dir().join(format!("line-rs-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("s.json");
        let s = FileStorage::new(&path).unwrap();
        s.set("a", "x".into()).await.unwrap();
        s.set("b", 2.into()).await.unwrap();
        assert_eq!(s.get_string("a").await.unwrap().as_deref(), Some("x"));
        s.delete("a").await.unwrap();
        assert!(s.get("a").await.unwrap().is_none());
        let s2 = FileStorage::new(&path).unwrap();
        assert_eq!(s2.get("b").await.unwrap(), Some(2.into()));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
