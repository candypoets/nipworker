use crate::db::types::{DatabaseConfig, DatabaseError, EventStorage, ProcessedNostrEvent};
use async_trait::async_trait;
use js_sys::{Array, Function, Object, Promise, Reflect};
use std::collections::HashMap;
use tracing::{debug, error, info};
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;

/// IndexedDB storage implementation for Nostr events
#[derive(Debug, Clone)]
pub struct IndexedDbStorage {
    db_name: String,
    db_version: u32,
    config: DatabaseConfig,
}

impl IndexedDbStorage {
    /// Create a new IndexedDB storage instance
    pub fn new(db_name: String, config: DatabaseConfig) -> Self {
        Self {
            db_name,
            db_version: 1,
            config,
        }
    }

    /// Open or create the IndexedDB database
    async fn open_db(&self) -> Result<JsValue, DatabaseError> {
        // In Web Worker context, use 'self' which is the WorkerGlobalScope
        let worker_self = js_sys::eval("self").map_err(|e| {
            DatabaseError::StorageError(format!("Cannot access worker self: {:?}", e))
        })?;

        // Get IndexedDB
        let idb_factory =
            Reflect::get(&worker_self, &JsValue::from_str("indexedDB")).map_err(|e| {
                DatabaseError::StorageError(format!("IndexedDB not available: {:?}", e))
            })?;

        if idb_factory.is_undefined() || idb_factory.is_null() {
            return Err(DatabaseError::StorageError(
                "IndexedDB not supported".to_string(),
            ));
        }

        // Create the database open request
        let args = Array::new();
        args.push(&JsValue::from_str(&self.db_name));
        args.push(&JsValue::from_f64(self.db_version as f64));

        let open_fn = Reflect::get(&idb_factory, &JsValue::from_str("open"))
            .map_err(|e| DatabaseError::StorageError(format!("Cannot get open method: {:?}", e)))?;

        let open_request = Reflect::apply(
            &open_fn.dyn_into::<Function>().unwrap(),
            &idb_factory,
            &args,
        )
        .map_err(|e| {
            DatabaseError::StorageError(format!("Failed to create open request: {:?}", e))
        })?;

        // Set up upgrade handler
        let db_name = self.db_name.clone();
        let upgrade_callback = Closure::wrap(Box::new(move |event: JsValue| {
            if let Ok(target) = Reflect::get(&event, &JsValue::from_str("target")) {
                if let Ok(result) = Reflect::get(&target, &JsValue::from_str("result")) {
                    if let Err(e) = setup_object_stores(&result) {
                        error!("Failed to setup object stores for {}: {:?}", db_name, e);
                    }
                }
            }
        }) as Box<dyn FnMut(JsValue)>);

        Reflect::set(
            &open_request,
            &JsValue::from_str("onupgradeneeded"),
            upgrade_callback.as_ref().unchecked_ref::<Function>(),
        )
        .map_err(|e| {
            DatabaseError::StorageError(format!("Failed to set upgrade handler: {:?}", e))
        })?;

        upgrade_callback.forget();

        let promise = js_request_to_promise(&open_request);
        let result = JsFuture::from(promise)
            .await
            .map_err(|e| DatabaseError::StorageError(format!("Database open failed: {:?}", e)))?;

        Ok(result)
    }

    /// Save events in batches for better performance
    async fn save_events_batch(&self, events: &[ProcessedNostrEvent]) -> Result<(), DatabaseError> {
        if events.is_empty() {
            return Ok(());
        }

        let db = self.open_db().await?;

        // Create transaction
        let tx_args = Array::new();
        tx_args.push(&JsValue::from_str("events"));
        tx_args.push(&JsValue::from_str("readwrite"));

        let tx_fn = Reflect::get(&db, &JsValue::from_str("transaction")).map_err(|e| {
            DatabaseError::StorageError(format!("Cannot get transaction method: {:?}", e))
        })?;

        let tx =
            Reflect::apply(&tx_fn.dyn_into::<Function>().unwrap(), &db, &tx_args).map_err(|e| {
                DatabaseError::StorageError(format!("Failed to create transaction: {:?}", e))
            })?;

        let store_args = Array::new();
        store_args.push(&JsValue::from_str("events"));

        let store_fn = Reflect::get(&tx, &JsValue::from_str("objectStore")).map_err(|e| {
            DatabaseError::StorageError(format!("Cannot get objectStore method: {:?}", e))
        })?;

        let store = Reflect::apply(&store_fn.dyn_into::<Function>().unwrap(), &tx, &store_args)
            .map_err(|e| {
                DatabaseError::StorageError(format!("Failed to get object store: {:?}", e))
            })?;

        // Convert events to JS objects and save them directly as event properties
        let mut promises = Vec::new();

        for event in events {
            // Serialize the event to JSON first, then parse it back to get a JS object
            let json_str =
                serde_json::to_string(event).map_err(|e| DatabaseError::SerializationError(e))?;

            // Parse the JSON string into a JS object
            let js_event = js_sys::JSON::parse(&json_str).map_err(|e| {
                DatabaseError::StorageError(format!("Failed to parse JSON: {:?}", e))
            })?;

            let put_args = Array::new();
            put_args.push(&js_event);

            let put_fn = Reflect::get(&store, &JsValue::from_str("put")).map_err(|e| {
                DatabaseError::StorageError(format!("Cannot get put method: {:?}", e))
            })?;

            let put_request =
                Reflect::apply(&put_fn.dyn_into::<Function>().unwrap(), &store, &put_args)
                    .map_err(|e| {
                        DatabaseError::StorageError(format!("Failed to put event: {:?}", e))
                    })?;

            promises.push(js_request_to_promise(&put_request));
        }

        // Wait for all operations to complete
        let promise_array = Array::new();
        for promise in promises {
            promise_array.push(&promise);
        }

        let all_promise = Promise::all(&promise_array);
        JsFuture::from(all_promise)
            .await
            .map_err(|e| DatabaseError::StorageError(format!("Batch save failed: {:?}", e)))?;

        // Wait for transaction to complete
        let tx_promise = js_transaction_to_promise(&tx);
        JsFuture::from(tx_promise)
            .await
            .map_err(|e| DatabaseError::StorageError(format!("Transaction failed: {:?}", e)))?;

        if self.config.debug_logging {
            debug!("Successfully saved {} events to IndexedDB", events.len());
        }

        Ok(())
    }

    /// Load events in batches for better memory management
    async fn load_events_batch(
        &self,
        _cursor_start: Option<String>,
    ) -> Result<Vec<ProcessedNostrEvent>, DatabaseError> {
        let db = self.open_db().await?;
        info!("Opened database: {:?}", db);

        let tx_args = Array::new();
        tx_args.push(&JsValue::from_str("events"));

        let tx_fn = Reflect::get(&db, &JsValue::from_str("transaction")).map_err(|e| {
            DatabaseError::StorageError(format!("Cannot get transaction method: {:?}", e))
        })?;

        let tx =
            Reflect::apply(&tx_fn.dyn_into::<Function>().unwrap(), &db, &tx_args).map_err(|e| {
                DatabaseError::StorageError(format!("Failed to create transaction: {:?}", e))
            })?;

        let store_args = Array::new();
        store_args.push(&JsValue::from_str("events"));

        let store_fn = Reflect::get(&tx, &JsValue::from_str("objectStore")).map_err(|e| {
            DatabaseError::StorageError(format!("Cannot get objectStore method: {:?}", e))
        })?;

        let store = Reflect::apply(&store_fn.dyn_into::<Function>().unwrap(), &tx, &store_args)
            .map_err(|e| {
                DatabaseError::StorageError(format!("Failed to get object store: {:?}", e))
            })?;

        // Use getAll for simplicity instead of cursors for now
        let getall_fn = Reflect::get(&store, &JsValue::from_str("getAll")).map_err(|e| {
            DatabaseError::StorageError(format!("Cannot get getAll method: {:?}", e))
        })?;

        let getall_request = Reflect::apply(
            &getall_fn.dyn_into::<Function>().unwrap(),
            &store,
            &Array::new(),
        )
        .map_err(|e| DatabaseError::StorageError(format!("Failed to call getAll: {:?}", e)))?;

        let promise = js_request_to_promise(&getall_request);
        let result = JsFuture::from(promise).await.map_err(|e| {
            DatabaseError::StorageError(format!("GetAll operation failed: {:?}", e))
        })?;

        info!(
            "GetAll result length: {:?}",
            result
                .as_ref()
                .dyn_ref::<Array>()
                .map(|a| a.length())
                .unwrap_or(0)
        );

        let mut events = Vec::new();

        if let Ok(array) = result.dyn_into::<Array>() {
            for i in 0..array.length() {
                if let Ok(item) = array.get(i).dyn_into::<Object>() {
                    // Convert the JS object directly to JSON string and then deserialize
                    match js_sys::JSON::stringify(&item) {
                        Ok(json_str) => {
                            if let Some(json_string) = json_str.as_string() {
                                match serde_json::from_str::<ProcessedNostrEvent>(&json_string) {
                                    Ok(event) => events.push(event),
                                    Err(e) => error!(
                                        "Failed to deserialize event at index {}: {:?}",
                                        i, e
                                    ),
                                }
                            }
                        }
                        Err(e) => error!("Failed to stringify JS object at index {}: {:?}", i, e),
                    }
                }
            }
        }

        debug!("Loaded {} events from IndexedDB", events.len());

        Ok(events)
    }

    /// Reset and refill storage with most recent events
    pub async fn reset_and_refill(
        &self,
        events: Vec<ProcessedNostrEvent>,
    ) -> Result<(), DatabaseError> {
        info!(
            "Resetting and refilling IndexedDB with {} events",
            events.len()
        );

        // Clear existing data
        self.clear_storage().await?;

        // Sort events by creation time (newest first)
        let mut sorted_events = events;
        sorted_events.sort_by(|a, b| b.created_at().cmp(&a.created_at()));

        // Limit to configured maximum
        if sorted_events.len() > self.config.max_events_in_storage {
            sorted_events.truncate(self.config.max_events_in_storage);
        }

        // Save in batches
        for chunk in sorted_events.chunks(self.config.batch_size) {
            self.save_events_batch(chunk).await?;
        }

        info!(
            "Successfully reset and refilled IndexedDB with {} events",
            sorted_events.len()
        );
        Ok(())
    }
}

#[async_trait(?Send)]
impl EventStorage for IndexedDbStorage {
    async fn save_events(&self, events: Vec<ProcessedNostrEvent>) -> Result<(), DatabaseError> {
        if events.is_empty() {
            return Ok(());
        }

        // Process events in batches
        for chunk in events.chunks(self.config.batch_size) {
            self.save_events_batch(chunk).await?;
        }

        Ok(())
    }

    async fn load_events(&self) -> Result<Vec<ProcessedNostrEvent>, DatabaseError> {
        let events = self.load_events_batch(None).await?;

        // if self.config.debug_logging {
        info!("Loaded total of {} events from IndexedDB", events.len());
        // }

        Ok(events)
    }

    async fn clear_storage(&self) -> Result<(), DatabaseError> {
        let db = self.open_db().await?;

        let tx_args = Array::new();
        tx_args.push(&JsValue::from_str("events"));
        tx_args.push(&JsValue::from_str("readwrite"));

        let tx_fn = Reflect::get(&db, &JsValue::from_str("transaction")).map_err(|e| {
            DatabaseError::StorageError(format!("Cannot get transaction method: {:?}", e))
        })?;

        let tx =
            Reflect::apply(&tx_fn.dyn_into::<Function>().unwrap(), &db, &tx_args).map_err(|e| {
                DatabaseError::StorageError(format!("Failed to create transaction: {:?}", e))
            })?;

        let store_args = Array::new();
        store_args.push(&JsValue::from_str("events"));

        let store_fn = Reflect::get(&tx, &JsValue::from_str("objectStore")).map_err(|e| {
            DatabaseError::StorageError(format!("Cannot get objectStore method: {:?}", e))
        })?;

        let store = Reflect::apply(&store_fn.dyn_into::<Function>().unwrap(), &tx, &store_args)
            .map_err(|e| {
                DatabaseError::StorageError(format!("Failed to get object store: {:?}", e))
            })?;

        let clear_fn = Reflect::get(&store, &JsValue::from_str("clear")).map_err(|e| {
            DatabaseError::StorageError(format!("Cannot get clear method: {:?}", e))
        })?;

        let clear_request = Reflect::apply(
            &clear_fn.dyn_into::<Function>().unwrap(),
            &store,
            &Array::new(),
        )
        .map_err(|e| DatabaseError::StorageError(format!("Failed to clear store: {:?}", e)))?;

        let promise = js_request_to_promise(&clear_request);
        JsFuture::from(promise)
            .await
            .map_err(|e| DatabaseError::StorageError(format!("Clear operation failed: {:?}", e)))?;

        if self.config.debug_logging {
            debug!("Successfully cleared all events from IndexedDB");
        }

        Ok(())
    }

    async fn get_stats(&self) -> Result<HashMap<String, serde_json::Value>, DatabaseError> {
        let db = self.open_db().await?;

        let tx_args = Array::new();
        tx_args.push(&JsValue::from_str("events"));

        let tx_fn = Reflect::get(&db, &JsValue::from_str("transaction")).map_err(|e| {
            DatabaseError::StorageError(format!("Cannot get transaction method: {:?}", e))
        })?;

        let tx =
            Reflect::apply(&tx_fn.dyn_into::<Function>().unwrap(), &db, &tx_args).map_err(|e| {
                DatabaseError::StorageError(format!("Failed to create transaction: {:?}", e))
            })?;

        let store_args = Array::new();
        store_args.push(&JsValue::from_str("events"));

        let store_fn = Reflect::get(&tx, &JsValue::from_str("objectStore")).map_err(|e| {
            DatabaseError::StorageError(format!("Cannot get objectStore method: {:?}", e))
        })?;

        let store = Reflect::apply(&store_fn.dyn_into::<Function>().unwrap(), &tx, &store_args)
            .map_err(|e| {
                DatabaseError::StorageError(format!("Failed to get object store: {:?}", e))
            })?;

        let count_fn = Reflect::get(&store, &JsValue::from_str("count")).map_err(|e| {
            DatabaseError::StorageError(format!("Cannot get count method: {:?}", e))
        })?;

        let count_request = Reflect::apply(
            &count_fn.dyn_into::<Function>().unwrap(),
            &store,
            &Array::new(),
        )
        .map_err(|e| DatabaseError::StorageError(format!("Failed to count events: {:?}", e)))?;

        let promise = js_request_to_promise(&count_request);
        let result = JsFuture::from(promise)
            .await
            .map_err(|e| DatabaseError::StorageError(format!("Count operation failed: {:?}", e)))?;

        let count = result.as_f64().unwrap_or(0.0) as usize;

        let mut stats = HashMap::new();
        stats.insert(
            "total_events".to_string(),
            serde_json::Value::Number(count.into()),
        );
        stats.insert(
            "db_name".to_string(),
            serde_json::Value::String(self.db_name.clone()),
        );
        stats.insert(
            "db_version".to_string(),
            serde_json::Value::Number(self.db_version.into()),
        );

        Ok(stats)
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// Setup object stores for the database
fn setup_object_stores(db: &JsValue) -> Result<(), JsValue> {
    // Check if events object store exists
    let store_names = Reflect::get(db, &JsValue::from_str("objectStoreNames"))?;

    let contains_fn = Reflect::get(&store_names, &JsValue::from_str("contains"))?;
    let contains_args = Array::new();
    contains_args.push(&JsValue::from_str("events"));

    let contains_events = Reflect::apply(
        &contains_fn.dyn_into::<Function>().unwrap(),
        &store_names,
        &contains_args,
    )?;

    if !contains_events.as_bool().unwrap_or(false) {
        // Create object store options
        let options = Object::new();
        Reflect::set(
            &options,
            &JsValue::from_str("keyPath"),
            &JsValue::from_str("id"),
        )?;

        let create_store_fn = Reflect::get(db, &JsValue::from_str("createObjectStore"))?;
        let create_store_args = Array::new();
        create_store_args.push(&JsValue::from_str("events"));
        create_store_args.push(&options);

        let _store = Reflect::apply(
            &create_store_fn.dyn_into::<Function>().unwrap(),
            db,
            &create_store_args,
        )?;

        // Note: We're not creating indexes for now to keep it simple
        // They can be added later if needed for query performance
    }

    Ok(())
}

/// Convert IndexedDB request to Promise
fn js_request_to_promise(request: &JsValue) -> Promise {
    Promise::new(&mut |resolve, reject| {
        let resolve_clone = resolve.clone();
        let reject_clone = reject.clone();
        let request_clone = request.clone();

        let success_callback = Closure::wrap(Box::new(move |_event: JsValue| {
            let result =
                Reflect::get(&request_clone, &JsValue::from_str("result")).unwrap_or(JsValue::NULL);
            let _ = resolve_clone.call1(&JsValue::NULL, &result);
        }) as Box<dyn FnMut(JsValue)>);

        let error_callback = Closure::wrap(Box::new(move |event: JsValue| {
            let error_msg = if let Ok(target) = Reflect::get(&event, &JsValue::from_str("target")) {
                if let Ok(error) = Reflect::get(&target, &JsValue::from_str("error")) {
                    error
                } else {
                    JsValue::from_str("Unknown IndexedDB error")
                }
            } else {
                JsValue::from_str("Unknown IndexedDB error")
            };
            let _ = reject_clone.call1(&JsValue::NULL, &error_msg);
        }) as Box<dyn FnMut(JsValue)>);

        let _ = Reflect::set(
            request,
            &JsValue::from_str("onsuccess"),
            success_callback.as_ref().unchecked_ref::<Function>(),
        );
        let _ = Reflect::set(
            request,
            &JsValue::from_str("onerror"),
            error_callback.as_ref().unchecked_ref::<Function>(),
        );

        success_callback.forget();
        error_callback.forget();
    })
}

/// Convert IndexedDB transaction to Promise
fn js_transaction_to_promise(transaction: &JsValue) -> Promise {
    Promise::new(&mut |resolve, reject| {
        let resolve_clone = resolve.clone();
        let reject_clone = reject.clone();

        let complete_callback = Closure::wrap(Box::new(move |_event: JsValue| {
            let _ = resolve_clone.call0(&JsValue::NULL);
        }) as Box<dyn FnMut(JsValue)>);

        let error_callback = Closure::wrap(Box::new(move |event: JsValue| {
            let error_msg = if let Ok(target) = Reflect::get(&event, &JsValue::from_str("target")) {
                if let Ok(error) = Reflect::get(&target, &JsValue::from_str("error")) {
                    error
                } else {
                    JsValue::from_str("Unknown transaction error")
                }
            } else {
                JsValue::from_str("Unknown transaction error")
            };
            let _ = reject_clone.call1(&JsValue::NULL, &error_msg);
        }) as Box<dyn FnMut(JsValue)>);

        let abort_callback = Closure::wrap(Box::new(move |_event: JsValue| {
            let _ = reject.call1(&JsValue::NULL, &JsValue::from_str("Transaction aborted"));
        }) as Box<dyn FnMut(JsValue)>);

        let _ = Reflect::set(
            transaction,
            &JsValue::from_str("oncomplete"),
            complete_callback.as_ref().unchecked_ref::<Function>(),
        );
        let _ = Reflect::set(
            transaction,
            &JsValue::from_str("onerror"),
            error_callback.as_ref().unchecked_ref::<Function>(),
        );
        let _ = Reflect::set(
            transaction,
            &JsValue::from_str("onabort"),
            abort_callback.as_ref().unchecked_ref::<Function>(),
        );

        complete_callback.forget();
        error_callback.forget();
        abort_callback.forget();
    })
}

/// Default storage factory
impl Default for IndexedDbStorage {
    fn default() -> Self {
        Self::new("nostr-local-relay".to_string(), DatabaseConfig::default())
    }
}
