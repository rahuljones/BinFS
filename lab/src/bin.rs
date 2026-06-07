use std::{
    collections::HashSet,
    hash::{DefaultHasher, Hash, Hasher},
};

use async_trait::async_trait;
use tribbler::colon;
use tribbler::{
    err::TribResult,
    storage::{KeyList, KeyString, KeyValue, List, Pattern, Storage},
};

use crate::lab3::backend::BackEnd;

enum ListLogOp {
    Append,
    Remove,
}

struct ListLogEntry {
    clock: u64,
    op: ListLogOp,
    value: String,
    raw: String,
}

#[derive(Clone)]
pub struct Bin {
    pub backends: Vec<BackEnd>,
    pub bin_name: String,
}

impl Bin {
    fn kv_prefixed_key(&self, key: &str) -> String {
        format!(
            "key::{}::{}",
            colon::escape(&self.bin_name),
            colon::escape(key)
        )
    }
    fn list_prefixed_key(&self, key: &str) -> String {
        format!(
            "list::{}::{}",
            colon::escape(&self.bin_name),
            colon::escape(key)
        )
    }

    fn kv_remove_prefix(&self, prefixed_key: &str) -> Option<String> {
        let prefix = format!("key::{}::", colon::escape(&self.bin_name));
        if prefixed_key.starts_with(&prefix) {
            Some(colon::unescape(prefixed_key[prefix.len()..].to_string()))
        } else {
            None
        }
    }

    fn list_remove_prefix(&self, prefixed_key: &str) -> Option<String> {
        let prefix = format!("list::{}::", colon::escape(&self.bin_name));
        if prefixed_key.starts_with(&prefix) {
            Some(colon::unescape(prefixed_key[prefix.len()..].to_string()))
        } else {
            None
        }
    }

    async fn get_live_backends(&self) -> TribResult<(Vec<BackEnd>, u64)> {
        let num_backs = self.backends.len();
        let start = get_hash_index(&self.bin_name, num_backs);

        loop {
            let mut live_backends = Vec::new();
            let mut max_clock = 0;

            for offset in 0..num_backs {
                let backend = self.backends[(start + offset) % num_backs].clone();
                if let Ok(clock) = backend.clock(0).await {
                    max_clock = max_clock.max(clock);
                    live_backends.push(backend);
                    if live_backends.len() == 2 {
                        break;
                    }
                }
            }

            if !live_backends.is_empty() {
                return Ok((live_backends, max_clock));
            }

            tokio::task::yield_now().await;
        }
    }

    async fn sync_clock(backends: &[BackEnd], at_least: u64) -> u64 {
        let mut timestamp = at_least;
        for backend in backends {
            if let Ok(clock) = backend.clock(timestamp).await {
                timestamp = timestamp.max(clock);
            }
        }
        timestamp
    }

    // all of these functions keep looping and keep trying the request until two backends confirm (kind of like 2 phase commit)
    async fn list_get_from_live_backends(
        &self,
        key: &str,
        sync_clock: bool,
    ) -> TribResult<(Vec<String>, u64)> {
        loop {
            let (backends, max_clock) = self.get_live_backends().await?;
            let clock = if sync_clock {
                Self::sync_clock(&backends, max_clock).await
            } else {
                max_clock
            };
            let mut entries = Vec::new();
            let mut read = false;

            for backend in backends {
                if let Ok(log) = backend.list_get(key).await {
                    read = true;
                    entries.extend(log.0);
                }
            }

            if read {
                return Ok((entries, clock));
            }

            tokio::task::yield_now().await;
        }
    }

    async fn list_keys_from_live_backends(&self, p: &Pattern) -> TribResult<Vec<String>> {
        loop {
            let (backends, _) = self.get_live_backends().await?;
            let mut keys = Vec::new();
            let mut read = false;

            for backend in backends {
                if let Ok(found_keys) = backend.list_keys(p).await {
                    read = true;
                    keys.extend(found_keys.0);
                }
            }

            if read {
                return Ok(keys);
            }

            tokio::task::yield_now().await;
        }
    }

    async fn append_to_backends(backends: Vec<BackEnd>, key: &str, value: &str) -> Option<bool> {
        let mut appended = false;
        let mut reached_backend = false;

        for backend in backends {
            if let Ok(result) = backend
                .list_append(&KeyValue {
                    key: key.to_string(),
                    value: value.to_string(),
                })
                .await
            {
                reached_backend = true;
                appended |= result;
            }
        }

        reached_backend.then_some(appended)
    }

    async fn append_to_live_backends(&self, key: &str, value: &str) -> TribResult<bool> {
        loop {
            let (backends, _) = self.get_live_backends().await?;
            if let Some(appended) = Self::append_to_backends(backends, key, value).await {
                return Ok(appended);
            }

            tokio::task::yield_now().await;
        }
    }

    async fn append_kv_log_entry(&self, key: &str, value: &str) -> TribResult<bool> {
        let value = colon::escape(value);
        loop {
            let (backends, max_clock) = self.get_live_backends().await?;
            let clock = Self::sync_clock(&backends, max_clock).await;
            let entry = format!("{}::{}", clock, value);

            if let Some(appended) = Self::append_to_backends(backends, key, &entry).await {
                return Ok(appended);
            }

            tokio::task::yield_now().await;
        }
    }

    async fn append_list_log_entry(&self, key: &str, value: &str) -> TribResult<bool> {
        let value = colon::escape(value);
        loop {
            let (backends, max_clock) = self.get_live_backends().await?;
            let clock = Self::sync_clock(&backends, max_clock).await;
            let entry = format!("append::{}::{}", clock, value);

            if let Some(appended) = Self::append_to_backends(backends, key, &entry).await {
                return Ok(appended);
            }

            tokio::task::yield_now().await;
        }
    }

    fn parse_list_log_entry(entry: &str) -> Option<ListLogEntry> {
        let mut parts = entry.splitn(3, "::");
        let op = match parts.next()? {
            "append" => ListLogOp::Append,
            "remove" => ListLogOp::Remove,
            _ => return None,
        };
        let clock = parts.next()?.parse::<u64>().ok()?;
        let value = colon::unescape(parts.next()?.to_string());

        Some(ListLogEntry {
            clock,
            op,
            value,
            raw: entry.to_string(),
        })
    }
}

#[async_trait]
impl KeyString for Bin {
    async fn get(&self, key: &str) -> TribResult<Option<String>> {
        let mut result = None;
        let log_key = self.kv_prefixed_key(key);
        let (log, _) = self.list_get_from_live_backends(&log_key, false).await?;

        let mut latest_clock = 0;

        for entry in log {
            if let Some((clock_raw, value_raw)) = entry.split_once("::") {
                if let Ok(clock) = clock_raw.parse::<u64>() {
                    if clock >= latest_clock {
                        latest_clock = clock;
                        let value = colon::unescape(value_raw.to_string());
                        result = if value.is_empty() { None } else { Some(value) };
                    }
                }
            }
        }
        Ok(result)
    }

    async fn set(&self, kv: &KeyValue) -> TribResult<bool> {
        let log_key = self.kv_prefixed_key(&kv.key);
        self.append_kv_log_entry(&log_key, &kv.value).await
    }

    async fn keys(&self, p: &Pattern) -> TribResult<List> {
        let mut keys = HashSet::new();
        let log_keys = self
            .list_keys_from_live_backends(&Pattern {
                prefix: self.kv_prefixed_key(&p.prefix),
                suffix: p.suffix.clone(),
            })
            .await?;

        for key in log_keys {
            if let Some(key) = self.kv_remove_prefix(&key) {
                if self.get(&key).await?.is_some() {
                    keys.insert(key);
                }
            }
        }
        let mut keys = keys.into_iter().collect::<Vec<_>>();
        keys.sort();
        Ok(List(keys))
    }
}

#[async_trait]
impl KeyList for Bin {
    async fn list_get(&self, key: &str) -> TribResult<List> {
        let log_key = self.list_prefixed_key(key);
        let (log_entries, _) = self.list_get_from_live_backends(&log_key, true).await?;

        let mut log_entries = log_entries
            .into_iter()
            .collect::<HashSet<_>>() // dedup
            .into_iter()
            .filter_map(|entry| Self::parse_list_log_entry(&entry))
            .collect::<Vec<_>>();
        log_entries.sort_by(|a, b| a.clock.cmp(&b.clock).then_with(|| a.raw.cmp(&b.raw)));

        let mut list = Vec::new();
        for entry in log_entries {
            match entry.op {
                ListLogOp::Append => list.push(entry.value),
                ListLogOp::Remove => list.retain(|value| value != &entry.value),
            }
        }

        Ok(List(list))
    }

    async fn list_append(&self, kv: &KeyValue) -> TribResult<bool> {
        let log_key = self.list_prefixed_key(&kv.key);
        self.append_list_log_entry(&log_key, &kv.value).await
    }

    async fn list_remove(&self, kv: &KeyValue) -> TribResult<u32> {
        let log_key = self.list_prefixed_key(&kv.key);
        let (log_entries, remove_clock) = self.list_get_from_live_backends(&log_key, true).await?;

        let mut log_entries = log_entries
            .into_iter()
            .collect::<HashSet<_>>() // dedup
            .into_iter()
            .filter_map(|entry| Self::parse_list_log_entry(&entry))
            .filter(|entry| entry.clock < remove_clock)
            .collect::<Vec<_>>();
        log_entries.sort_by(|a, b| a.clock.cmp(&b.clock).then_with(|| a.raw.cmp(&b.raw)));

        let mut list = Vec::new();
        for entry in log_entries {
            match entry.op {
                ListLogOp::Append => list.push(entry.value),
                ListLogOp::Remove => list.retain(|value| value != &entry.value),
            }
        }

        let removed = list.iter().filter(|value| *value == &kv.value).count() as u32;

        if removed > 0 {
            self.append_to_live_backends(
                &log_key,
                &format!("remove::{}::{}", remove_clock, colon::escape(&kv.value)),
            )
            .await?;
        }

        Ok(removed)
    }

    async fn list_keys(&self, p: &Pattern) -> TribResult<List> {
        let mut keys = HashSet::new();
        let log_keys = self
            .list_keys_from_live_backends(&Pattern {
                prefix: self.list_prefixed_key(&p.prefix),
                suffix: p.suffix.clone(),
            })
            .await?;

        for key in log_keys {
            if let Some(key) = self.list_remove_prefix(&key) {
                if !self.list_get(&key).await?.0.is_empty() {
                    keys.insert(key);
                }
            }
        }
        let mut keys = keys.into_iter().collect::<Vec<_>>();
        keys.sort();
        Ok(List(keys))
    }
}

#[async_trait]
impl Storage for Bin {
    async fn clock(&self, at_least: u64) -> TribResult<u64> {
        loop {
            let (backends, max_clock) = self.get_live_backends().await?;
            let mut timestamp = at_least.max(max_clock);
            let mut ticked = false;

            for backend in backends {
                if let Ok(clock) = backend.clock(timestamp).await {
                    ticked = true;
                    timestamp = timestamp.max(clock);
                }
            }

            if ticked {
                return Ok(timestamp);
            }

            tokio::task::yield_now().await;
        }
    }
}

fn get_hash_index(name: &str, num_backs: usize) -> usize {
    let mut hasher = DefaultHasher::new();
    name.hash(&mut hasher);

    let hash_value = hasher.finish();
    hash_value as usize % num_backs
}
