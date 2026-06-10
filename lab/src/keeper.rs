use std::{collections::HashSet, time::Duration};

use tokio::time::{interval, sleep};
use tonic::transport::Channel;
use tribbler::{
    config::KeeperConfig,
    err::TribResult,
    rpc::trib_storage_client::TribStorageClient,
    storage::{KeyList, KeyValue, Pattern, Storage},
};

use crate::lab3::{
    backend::BackEnd,
    utils::{bin_name_from_key, get_live_backend_indices},
};

struct BackendStatus {
    _addr: String,
    backend: BackEnd,
    alive: bool,
    last_clock: Option<u64>,
}

#[allow(unused_variables)]
pub async fn run_keeper(kc: KeeperConfig) -> TribResult<()> {
    let KeeperConfig {
        backs,
        this,
        id,
        ready,
        mut shutdown,
        ..
    } = kc;

    let mut backends = Vec::new();
    for back in backs {
        // in case a backend isn't online yet
        let channel = Channel::from_shared(format!("http://{}", back))?.connect_lazy();
        let client = TribStorageClient::new(channel);
        backends.push(BackendStatus {
            _addr: back,
            backend: BackEnd { client },
            alive: true,
            last_clock: None,
        })
    }

    let mut backend_left_repair_queue = HashSet::new();
    let mut backend_rejoin_repair_queue = HashSet::new();

    heartbeat(&mut backends).await;

    // for all dead backends, sanity check to make sure the left and right backends are properly updated
    let initial_live_backends: Vec<bool> = backends.iter().map(|backend| backend.alive).collect();
    for index in 0..initial_live_backends.len() {
        if initial_live_backends[index] {
            continue;
        }

        if let Some(left_index) = previous_live_backend_index(index, &initial_live_backends) {
            backend_left_repair_queue.insert(left_index);
        }
        if let Some(right_index) = next_live_backend_index(index, &initial_live_backends) {
            backend_left_repair_queue.insert(right_index);
        }
    }

    if let Some(channel) = ready {
        channel.send(true)?;
    }

    let mut tick = interval(Duration::from_secs(1));
    let mut tick_count = 0;
    let sleep_offset_base = (id as u64 + this as u64) % 50;

    loop {
        if let Some(ref mut channel) = shutdown {
            if channel.try_recv().is_ok() {
                return Ok(());
            }
        }

        tick.tick().await;

        // stagger keepers to mitigate the repair race
        tick_count = tick_count + 1;
        let sleep_offset_ms = (sleep_offset_base + tick_count) % 50;
        if sleep_offset_ms > 0 {
            sleep(Duration::from_millis(sleep_offset_ms)).await;
        }

        let old_live_backends: Vec<bool> = backends.iter().map(|backend| backend.alive).collect();

        let restarted_backends = heartbeat(&mut backends).await;

        let live_backends: Vec<bool> = backends.iter().map(|backend| backend.alive).collect();
        for index in 0..live_backends.len() {
            if old_live_backends[index] && !live_backends[index] {
                if let Some(left_index) = previous_live_backend_index(index, &live_backends) {
                    backend_left_repair_queue.insert(left_index);
                }
                if let Some(right_index) = next_live_backend_index(index, &live_backends) {
                    backend_left_repair_queue.insert(right_index);
                }
            }

            if live_backends[index] && !old_live_backends[index] {
                backend_rejoin_repair_queue.insert(index);
            }

            if restarted_backends.contains(&index) {
                backend_rejoin_repair_queue.insert(index);
            }
        }

        let primary_repair_indices = backend_left_repair_queue
            .iter()
            .copied()
            .collect::<Vec<_>>();
        for index in primary_repair_indices {
            // backend in queue went offline, no need to repair
            if index >= live_backends.len() || !live_backends[index] {
                backend_left_repair_queue.remove(&index);
                continue;
            }

            let _ = repair_backend_left(&backends, index).await;
        }

        let rejoin_repair_indices = backend_rejoin_repair_queue
            .iter()
            .copied()
            .collect::<Vec<_>>();
        for index in rejoin_repair_indices {
            if index >= live_backends.len() || !live_backends[index] {
                backend_rejoin_repair_queue.remove(&index);
                continue;
            }

            let _ = repair_backend_rejoined(&backends, index).await;
        }
    }
}

async fn heartbeat(backends: &mut [BackendStatus]) -> HashSet<usize> {
    let mut reads = Vec::new();
    for (index, backend_status) in backends.iter().enumerate() {
        let backend = backend_status.backend.clone();
        let handle = tokio::spawn(async move { (index, backend.clock(0).await) });
        reads.push(handle);
    }

    let mut max_clock = 0;
    let mut restarted_backends = HashSet::new();
    for handle in reads {
        if let Ok((index, clock_result)) = handle.await {
            match clock_result {
                Ok(clock) => {
                    // Detect restarts that happen between heartbeat ticks.
                    if backends[index].alive
                        && backends[index]
                            .last_clock
                            .is_some_and(|last_clock| clock <= last_clock)
                    {
                        restarted_backends.insert(index);
                    }
                    backends[index].alive = true;
                    max_clock = max_clock.max(clock);
                }
                Err(_) => {
                    backends[index].alive = false;
                    backends[index].last_clock = None;
                }
            }
        }
    }

    let mut writes = Vec::new();
    for (index, backend_status) in backends.iter().enumerate() {
        if !backend_status.alive {
            continue;
        }

        let backend = backend_status.backend.clone();
        let handle = tokio::spawn(async move { (index, backend.clock(max_clock).await) });
        writes.push(handle);
    }

    for handle in writes {
        if let Ok((index, clock_result)) = handle.await {
            match clock_result {
                Ok(clock) => {
                    backends[index].alive = true;
                    backends[index].last_clock = Some(clock);
                }
                Err(_) => {
                    backends[index].alive = false;
                    backends[index].last_clock = None;
                }
            }
        }
    }

    restarted_backends
}

async fn repair_backend_left(backends: &[BackendStatus], primary_index: usize) -> TribResult<()> {
    let live_backends: Vec<bool> = backends.iter().map(|backend| backend.alive).collect();

    if primary_index >= live_backends.len() || !live_backends[primary_index] {
        return Ok(());
    }

    let primary = &backends[primary_index].backend;

    let keys = match primary.list_keys(&Pattern::default()).await {
        Ok(keys) => keys.0,
        Err(_) => return Ok(()),
    };

    for key in keys {
        let Some(bin_name) = bin_name_from_key(&key) else {
            continue;
        };
        let target_indices = match get_live_backend_indices(&bin_name, &live_backends) {
            Ok(indices) => indices,
            Err(_) => continue,
        };

        if target_indices.len() < 2 {
            continue;
        }

        let assigned_primary_index = target_indices[0];
        let backup_index = target_indices[1];
        // might not be necessary
        if assigned_primary_index != primary_index {
            continue;
        }

        let backup = &backends[backup_index].backend;
        let _ = copy_log_entries(primary, backup, &key).await;
    }

    Ok(())
}

async fn repair_backend_rejoined(
    backends: &[BackendStatus],
    rejoined_index: usize,
) -> TribResult<()> {
    let live_backends: Vec<bool> = backends.iter().map(|backend| backend.alive).collect();

    if rejoined_index >= live_backends.len() || !live_backends[rejoined_index] {
        return Ok(());
    }

    if let Some(left_index) = previous_live_backend_index(rejoined_index, &live_backends) {
        let left = &backends[left_index].backend;
        let rejoined = &backends[rejoined_index].backend;

        let keys = match left.list_keys(&Pattern::default()).await {
            Ok(keys) => keys.0,
            Err(_) => Vec::new(),
        };

        for key in keys {
            let Some(bin_name) = bin_name_from_key(&key) else {
                continue;
            };

            let target_indices = match get_live_backend_indices(&bin_name, &live_backends) {
                Ok(indices) => indices,
                Err(_) => continue,
            };
            if target_indices.len() < 2 {
                continue;
            }

            // required to check if the bin belongs to left backend, since the left backend serves both as a primary and a backup
            let assigned_primary_index = target_indices[0];
            let assigned_backup_index = target_indices[1];
            if assigned_primary_index == left_index && assigned_backup_index == rejoined_index {
                let _ = copy_log_entries(left, rejoined, &key).await;
            }
        }
    }

    if let Some(right_index) = next_live_backend_index(rejoined_index, &live_backends) {
        let rejoined = &backends[rejoined_index].backend;
        let right = &backends[right_index].backend;

        let keys = match right.list_keys(&Pattern::default()).await {
            Ok(keys) => keys.0,
            Err(_) => Vec::new(),
        };

        for key in keys {
            let Some(bin_name) = bin_name_from_key(&key) else {
                continue;
            };

            let target_indices = match get_live_backend_indices(&bin_name, &live_backends) {
                Ok(indices) => indices,
                Err(_) => continue,
            };
            if target_indices.len() < 2 {
                continue;
            }

            let assigned_primary_index = target_indices[0];
            let assigned_backup_index = target_indices[1];
            if assigned_primary_index == rejoined_index && assigned_backup_index == right_index {
                let _ = copy_log_entries(right, rejoined, &key).await;
            }
        }
    }

    Ok(())
}

async fn copy_log_entries(source: &BackEnd, target: &BackEnd, key: &str) -> TribResult<()> {
    let mut target_entries: HashSet<String> = match target.list_get(key).await {
        Ok(entries) => entries.0.into_iter().collect(),
        Err(_) => return Ok(()),
    };
    let source_entries = match source.list_get(key).await {
        Ok(entries) => entries.0,
        Err(_) => return Ok(()),
    };

    if key.starts_with("key::") {
        let mut latest_entry = None;
        let mut latest_clock = 0;

        // get entry with latest clock value
        for entry in source_entries {
            let Some((clock, _)) = entry.split_once("::") else {
                continue;
            };
            let Ok(clock) = clock.parse::<u64>() else {
                continue;
            };

            if latest_entry.is_none() || clock >= latest_clock {
                latest_clock = clock;
                latest_entry = Some(entry);
            }
        }

        if let Some(entry) = latest_entry {
            let _ = append_missing_log_entry(target, key, entry, &mut target_entries).await;
        }
    } else if key.starts_with("list::") {
        // copy all log entries from source to target
        for entry in source_entries {
            let _ = append_missing_log_entry(target, key, entry, &mut target_entries).await;
        }
    }

    Ok(())
}

async fn append_missing_log_entry(
    target: &BackEnd,
    key: &str,
    entry: String,
    target_entries: &mut HashSet<String>,
) -> TribResult<()> {
    if target_entries.contains(&entry) {
        return Ok(());
    }

    let appended = target
        .list_append(&KeyValue {
            key: key.to_string(),
            value: entry.clone(),
        })
        .await;

    if appended.is_ok() {
        target_entries.insert(entry);
    }

    Ok(())
}

fn previous_live_backend_index(start: usize, live_backends: &[bool]) -> Option<usize> {
    if live_backends.is_empty() {
        return None;
    }

    for offset in 1..live_backends.len() {
        let index = (start + live_backends.len() - offset) % live_backends.len();
        if live_backends[index] {
            return Some(index);
        }
    }

    None
}

fn next_live_backend_index(start: usize, live_backends: &[bool]) -> Option<usize> {
    if live_backends.is_empty() {
        return None;
    }

    for offset in 1..live_backends.len() {
        let index = (start + offset) % live_backends.len();
        if live_backends[index] {
            return Some(index);
        }
    }

    None
}
