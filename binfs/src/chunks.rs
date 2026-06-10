use std::{error::Error, sync::Arc};

use base64::{engine::general_purpose::STANDARD, Engine};
use serde::{Deserialize, Serialize};
use tokio::task::JoinSet;
use tribbler::{
    err::{TribResult, TribblerError},
    storage::{BinStorage, KeyValue},
};

use crate::service::with_storage_timeout;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChunkRef {
    pub bin: String,
    pub key: String,
    pub hash: String,
    pub len: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileManifest {
    pub size: u64,
    pub chunk_size: usize,
    pub chunks: Vec<ChunkRef>,
    pub content_hash: String,
    pub modified_ms: u64,
}

impl FileManifest {
    pub fn empty(chunk_size: usize, modified_ms: u64) -> Self {
        Self {
            size: 0,
            chunk_size,
            chunks: Vec::new(),
            content_hash: blake3::hash(&[]).to_hex().to_string(),
            modified_ms,
        }
    }
}

#[derive(Clone)]
pub struct ChunkStore {
    bins: Arc<dyn BinStorage>,
    chunk_size: usize,
    data_bins: usize,
}

impl ChunkStore {
    pub fn new(bins: Arc<dyn BinStorage>, chunk_size: usize, data_bins: usize) -> Self {
        Self {
            bins,
            chunk_size,
            data_bins,
        }
    }

    pub fn chunk_size(&self) -> usize {
        self.chunk_size
    }

    pub fn data_bin_name(&self, hash: &str) -> String {
        let prefix = hash.get(..16).unwrap_or(hash);
        let raw = u64::from_str_radix(prefix, 16).unwrap_or(0);
        format!("__fs_data_{}", raw as usize % self.data_bins.max(1))
    }

    pub async fn store_file(&self, data: &[u8], modified_ms: u64) -> TribResult<FileManifest> {
        let mut writes = JoinSet::new();
        for (index, chunk) in data.chunks(self.chunk_size).enumerate() {
            let hash = blake3::hash(chunk).to_hex().to_string();
            let bin_name = self.data_bin_name(&hash);
            let key = format!("chunk:{hash}");
            let bins = self.bins.clone();
            let chunk = chunk.to_vec();
            writes.spawn(async move { store_chunk(bins, index, chunk, bin_name, key, hash).await });
        }

        let mut chunks = vec![None; data.chunks(self.chunk_size).len()];
        while let Some(result) = writes.join_next().await {
            let (index, chunk_ref) =
                result.map_err(|err| unknown_error(format!("chunk task failed: {err}")))??;
            chunks[index] = Some(chunk_ref);
        }
        let chunks = chunks
            .into_iter()
            .map(|chunk_ref| {
                chunk_ref.ok_or_else(|| unknown_error("chunk write task did not return a result"))
            })
            .collect::<TribResult<Vec<_>>>()?;

        Ok(FileManifest {
            size: data.len() as u64,
            chunk_size: self.chunk_size,
            chunks,
            content_hash: blake3::hash(data).to_hex().to_string(),
            modified_ms,
        })
    }

    pub async fn load_file(&self, manifest: &FileManifest) -> TribResult<Vec<u8>> {
        let mut reads = JoinSet::new();
        for (index, chunk_ref) in manifest.chunks.iter().cloned().enumerate() {
            let bins = self.bins.clone();
            reads.spawn(async move { load_chunk(bins, index, chunk_ref).await });
        }

        let mut chunks = vec![None; manifest.chunks.len()];
        while let Some(result) = reads.join_next().await {
            let (index, chunk) =
                result.map_err(|err| unknown_error(format!("chunk task failed: {err}")))??;
            chunks[index] = Some(chunk);
        }

        let mut data = Vec::with_capacity(manifest.size as usize);
        for chunk in chunks {
            let chunk =
                chunk.ok_or_else(|| unknown_error("chunk read task did not return a result"))?;
            data.extend_from_slice(&chunk);
        }

        if data.len() as u64 != manifest.size
            || blake3::hash(&data).to_hex().to_string() != manifest.content_hash
        {
            return Err(Box::new(TribblerError::Unknown(
                "file manifest hash mismatch".to_string(),
            )));
        }
        Ok(data)
    }
}

async fn store_chunk(
    bins: Arc<dyn BinStorage>,
    index: usize,
    chunk: Vec<u8>,
    bin_name: String,
    key: String,
    hash: String,
) -> TribResult<(usize, ChunkRef)> {
    let bin = bins.bin(&bin_name).await?;
    with_storage_timeout(
        "chunk set",
        bin.set(&KeyValue {
            key: key.clone(),
            value: STANDARD.encode(&chunk),
        }),
    )
    .await
    .map_err(|err| unknown_error(err.to_string()))?;

    Ok((
        index,
        ChunkRef {
            bin: bin_name,
            key,
            hash,
            len: chunk.len(),
        },
    ))
}

async fn load_chunk(
    bins: Arc<dyn BinStorage>,
    index: usize,
    chunk_ref: ChunkRef,
) -> TribResult<(usize, Vec<u8>)> {
    let bin = bins.bin(&chunk_ref.bin).await?;
    let Some(encoded) = with_storage_timeout("chunk get", bin.get(&chunk_ref.key))
        .await
        .map_err(|err| unknown_error(err.to_string()))?
    else {
        return Err(unknown_error(format!("missing chunk {}", chunk_ref.key)));
    };
    let chunk = STANDARD.decode(encoded)?;
    if chunk.len() != chunk_ref.len || blake3::hash(&chunk).to_hex().to_string() != chunk_ref.hash {
        return Err(unknown_error(format!("corrupt chunk {}", chunk_ref.key)));
    }

    Ok((index, chunk))
}

fn unknown_error(message: impl Into<String>) -> Box<dyn Error + Send + Sync> {
    Box::new(TribblerError::Unknown(message.into()))
}
