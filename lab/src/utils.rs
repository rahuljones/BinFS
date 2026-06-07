use std::hash::{DefaultHasher, Hash, Hasher};

use tribbler::{colon, err::TribResult, storage::Storage};

use crate::lab3::backend::BackEnd;

#[allow(dead_code)]
pub async fn get_live_backends(
    backends: Vec<BackEnd>,
    bin_name: &str,
) -> TribResult<(Vec<BackEnd>, u64)> {
    let num_backs = backends.len();
    if num_backs == 0 {
        return Err("no backends configured".into());
    }

    let start = get_hash_index(bin_name, num_backs);
    let mut live_backends = Vec::new();
    let mut max_clock = 0;

    for offset in 0..num_backs {
        let backend = backends[(start + offset) % num_backs].clone();
        if let Ok(clock) = backend.clock(0).await {
            max_clock = max_clock.max(clock);
            live_backends.push(backend);
            if live_backends.len() == 2 {
                break;
            }
        }
    }
    if live_backends.is_empty() {
        Err("no live backends".into())
    } else {
        Ok((live_backends, max_clock))
    }
}

pub fn get_hash_index(name: &str, num_backs: usize) -> usize {
    let mut hasher = DefaultHasher::new();
    name.hash(&mut hasher);

    let hash_value = hasher.finish();
    hash_value as usize % num_backs
}

pub fn get_live_backend_indices(bin_name: &str, live_backends: &[bool]) -> TribResult<Vec<usize>> {
    let num_backs = live_backends.len();
    if num_backs == 0 {
        return Err("no backends configured".into());
    }

    let start = get_hash_index(bin_name, num_backs);
    let mut indices = Vec::new();

    for offset in 0..num_backs {
        let index = (start + offset) % num_backs;
        if live_backends[index] {
            indices.push(index);
            if indices.len() == 2 {
                break;
            }
        }
    }

    if indices.is_empty() {
        Err("no live backends".into())
    } else {
        Ok(indices)
    }
}

pub fn next_live_backend_index(start: usize, live_backends: &[bool]) -> Option<usize> {
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

pub fn bin_name_from_key(key: &str) -> Option<String> {
    let (first, rest) = key.split_once("::")?;

    if matches!(first, "key" | "list") {
        if let Some((bin_name, _)) = rest.split_once("::") {
            return Some(colon::unescape(bin_name.to_string()));
        }
    }

    Some(colon::unescape(first.to_string()))
}
