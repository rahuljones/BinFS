use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use binfs::{BinFsConfig, BinFsService};
use tribbler::{
    err::TribResult,
    storage::{BinStorage, KeyList, KeyString, KeyValue, MemStorage, Pattern, Storage},
};

#[derive(Default)]
pub struct MemBins {
    bins: Mutex<HashMap<String, Arc<MemStorage>>>,
}

#[async_trait]
impl BinStorage for MemBins {
    async fn bin(&self, name: &str) -> TribResult<Box<dyn Storage>> {
        let mut bins = self.bins.lock().unwrap();
        let storage = bins
            .entry(name.to_string())
            .or_insert_with(|| Arc::new(MemStorage::new()))
            .clone();
        Ok(Box::new(MemStorageBox(storage)))
    }
}

struct MemStorageBox(Arc<MemStorage>);

#[async_trait]
impl KeyString for MemStorageBox {
    async fn get(&self, key: &str) -> TribResult<Option<String>> {
        self.0.get(key).await
    }

    async fn set(&self, kv: &KeyValue) -> TribResult<bool> {
        self.0.set(kv).await
    }

    async fn keys(&self, p: &Pattern) -> TribResult<tribbler::storage::List> {
        self.0.keys(p).await
    }
}

#[async_trait]
impl KeyList for MemStorageBox {
    async fn list_get(&self, key: &str) -> TribResult<tribbler::storage::List> {
        self.0.list_get(key).await
    }

    async fn list_append(&self, kv: &KeyValue) -> TribResult<bool> {
        self.0.list_append(kv).await
    }

    async fn list_remove(&self, kv: &KeyValue) -> TribResult<u32> {
        self.0.list_remove(kv).await
    }

    async fn list_keys(&self, p: &Pattern) -> TribResult<tribbler::storage::List> {
        self.0.list_keys(p).await
    }
}

#[async_trait]
impl Storage for MemStorageBox {
    async fn clock(&self, at_least: u64) -> TribResult<u64> {
        self.0.clock(at_least).await
    }
}

#[allow(dead_code)]
pub fn service() -> BinFsService {
    BinFsService::new(
        Arc::new(MemBins::default()),
        BinFsConfig {
            metadata_bin: "__test_meta__".to_string(),
            chunk_size: 4,
            data_bins: 8,
        },
    )
}
