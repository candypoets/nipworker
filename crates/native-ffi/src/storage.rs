use nipworker_core::storage::BlobStore;
use nipworker_core::traits::StorageError;
use std::fs;
use std::path::PathBuf;

pub struct FileBlobStore {
    dir: PathBuf,
}

impl FileBlobStore {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    fn path_for_key(&self, key: &str) -> PathBuf {
        let file_name = key
            .chars()
            .map(|c| match c {
                'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' | '.' => c,
                _ => '_',
            })
            .collect::<String>();
        self.dir.join(file_name)
    }
}

#[async_trait::async_trait(?Send)]
impl BlobStore for FileBlobStore {
    async fn get(&self, key: &str) -> Result<Option<Vec<u8>>, StorageError> {
        let path = self.path_for_key(key);
        match fs::read(&path) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(StorageError::Other(format!(
                "Failed to read native blob '{}': {}",
                key, e
            ))),
        }
    }

    async fn put(&self, key: &str, bytes: &[u8]) -> Result<(), StorageError> {
        fs::create_dir_all(&self.dir)
            .map_err(|e| StorageError::Other(format!("Failed to create native blob dir: {}", e)))?;

        let path = self.path_for_key(key);
        let tmp_path = path.with_extension("tmp");
        fs::write(&tmp_path, bytes).map_err(|e| {
            StorageError::Other(format!("Failed to write native blob '{}': {}", key, e))
        })?;
        fs::rename(&tmp_path, &path).map_err(|e| {
            StorageError::Other(format!("Failed to replace native blob '{}': {}", key, e))
        })?;
        Ok(())
    }
}
