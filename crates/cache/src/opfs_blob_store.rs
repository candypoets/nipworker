use async_trait::async_trait;
use js_sys::Uint8Array;
use nipworker_core::{storage::BlobStore, traits::StorageError};
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::{
    Blob, DedicatedWorkerGlobalScope, FileSystemDirectoryHandle, FileSystemFileHandle,
    FileSystemGetDirectoryOptions, FileSystemGetFileOptions, FileSystemWritableFileStream,
    StorageManager, WorkerGlobalScope,
};

pub struct OpfsBlobStore {
    directory_name: String,
}

impl OpfsBlobStore {
    pub fn new(directory_name: String) -> Self {
        Self { directory_name }
    }

    async fn directory(&self) -> Result<FileSystemDirectoryHandle, StorageError> {
        let worker = js_sys::global()
            .dyn_into::<DedicatedWorkerGlobalScope>()
            .map_err(|_| StorageError::Other("OPFS requires a dedicated worker".into()))?;
        let worker_scope: WorkerGlobalScope = worker.unchecked_into();
        let navigator = worker_scope.navigator();

        // `navigator.storage` is undefined outside secure contexts (plain
        // HTTP), and `getDirectory` is missing on browsers without OPFS.
        // Check via Reflect so these surface as a StorageError instead of an
        // uncaught TypeError crossing the JS/WASM boundary.
        let storage = js_sys::Reflect::get(navigator.as_ref(), &JsValue::from_str("storage"))
            .map_err(|_| StorageError::Other("OPFS unavailable: navigator.storage inaccessible".into()))?;
        if storage.is_null() || storage.is_undefined() {
            return Err(StorageError::Other(
                "OPFS unavailable: navigator.storage is undefined (secure context required)"
                    .into(),
            ));
        }
        let get_directory = js_sys::Reflect::get(&storage, &JsValue::from_str("getDirectory"))
            .map_err(|_| StorageError::Other("OPFS unavailable".into()))?;
        if !get_directory.is_function() {
            return Err(StorageError::Other(
                "OPFS unavailable: navigator.storage.getDirectory is not supported".into(),
            ));
        }
        let storage: StorageManager = storage.unchecked_into();

        let root = JsFuture::from(storage.get_directory())
            .await
            .map_err(|e| StorageError::Other(format!("OPFS getDirectory failed: {:?}", e)))?
            .dyn_into::<FileSystemDirectoryHandle>()
            .map_err(|_| StorageError::Other("OPFS root handle has unexpected type".into()))?;

        let options = FileSystemGetDirectoryOptions::new();
        options.set_create(true);
        JsFuture::from(root.get_directory_handle_with_options(&self.directory_name, &options))
            .await
            .map_err(|e| {
                StorageError::Other(format!(
                    "OPFS getDirectoryHandle '{}' failed: {:?}",
                    self.directory_name, e
                ))
            })?
            .dyn_into::<FileSystemDirectoryHandle>()
            .map_err(|_| StorageError::Other("OPFS directory handle has unexpected type".into()))
    }

    fn file_name(key: &str) -> String {
        key.chars()
            .map(|c| match c {
                'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' | '.' => c,
                _ => '_',
            })
            .collect()
    }

    async fn file_handle(
        &self,
        key: &str,
        create: bool,
    ) -> Result<FileSystemFileHandle, StorageError> {
        let dir = self.directory().await?;
        let options = FileSystemGetFileOptions::new();
        options.set_create(create);
        let file_name = Self::file_name(key);

        JsFuture::from(dir.get_file_handle_with_options(&file_name, &options))
            .await
            .map_err(|e| {
                StorageError::Other(format!(
                    "OPFS getFileHandle '{}' failed: {:?}",
                    file_name, e
                ))
            })?
            .dyn_into::<FileSystemFileHandle>()
            .map_err(|_| StorageError::Other("OPFS file handle has unexpected type".into()))
    }
}

#[async_trait(?Send)]
impl BlobStore for OpfsBlobStore {
    async fn get(&self, key: &str) -> Result<Option<Vec<u8>>, StorageError> {
        let handle = match self.file_handle(key, false).await {
            Ok(handle) => handle,
            Err(StorageError::Other(message)) if message.contains("NotFoundError") => {
                return Ok(None);
            }
            Err(e) => return Err(e),
        };

        let file = JsFuture::from(handle.get_file())
            .await
            .map_err(|e| StorageError::Other(format!("OPFS getFile failed: {:?}", e)))?;
        let blob: Blob = file
            .dyn_into()
            .map_err(|_| StorageError::Other("OPFS file has unexpected type".into()))?;
        let buffer = JsFuture::from(blob.array_buffer())
            .await
            .map_err(|e| StorageError::Other(format!("OPFS arrayBuffer failed: {:?}", e)))?;

        Ok(Some(Uint8Array::new(&buffer).to_vec()))
    }

    async fn put(&self, key: &str, bytes: &[u8]) -> Result<(), StorageError> {
        let handle = self.file_handle(key, true).await?;
        let writable = JsFuture::from(handle.create_writable())
            .await
            .map_err(|e| StorageError::Other(format!("OPFS createWritable failed: {:?}", e)))?
            .dyn_into::<FileSystemWritableFileStream>()
            .map_err(|_| StorageError::Other("OPFS writable stream has unexpected type".into()))?;

        let bytes = Uint8Array::from(bytes);
        JsFuture::from(
            writable
                .write_with_js_u8_array(&bytes)
                .map_err(|e| StorageError::Other(format!("OPFS write failed: {:?}", e)))?,
        )
        .await
        .map_err(|e| StorageError::Other(format!("OPFS write rejected: {:?}", e)))?;
        JsFuture::from(writable.close())
            .await
            .map_err(|e| StorageError::Other(format!("OPFS close failed: {:?}", e)))?;

        Ok(())
    }
}
