use std::sync::Arc;

use base64::{engine::general_purpose::STANDARD, Engine};
use binfs::ChunkStore;
use tribbler::{
    err::TribResult,
    storage::{BinStorage, KeyValue},
};

mod common;

use common::MemBins;

#[tokio::test]
async fn chunks_round_trip_binary_data() -> TribResult<()> {
    let bins = Arc::new(MemBins::default());
    let store = ChunkStore::new(bins, 3, 4);
    let data = b"\0abc\xffhello world".to_vec();

    let manifest = store.store_file(&data, 12).await?;
    assert_eq!(manifest.chunks.len(), 6);
    assert_eq!(store.load_file(&manifest).await?, data);
    Ok(())
}

#[test]
fn bin_hashing_is_stable() {
    let bins = Arc::new(MemBins::default());
    let store = ChunkStore::new(bins, 64, 128);

    assert_eq!(
        store.data_bin_name("000000000000000faaaaaaaaaaaaaaaa"),
        "__fs_data_15"
    );
    assert_eq!(
        store.data_bin_name("0000000000000080aaaaaaaaaaaaaaaa"),
        "__fs_data_0"
    );
}

#[tokio::test]
async fn corrupted_chunk_is_rejected() -> TribResult<()> {
    let bins = Arc::new(MemBins::default());
    let store = ChunkStore::new(bins.clone(), 4, 2);
    let manifest = store.store_file(b"abcdef", 1).await?;
    let first = &manifest.chunks[0];
    let bin = bins.bin(&first.bin).await?;
    bin.set(&KeyValue {
        key: first.key.clone(),
        value: STANDARD.encode(b"zzzz"),
    })
    .await?;

    assert!(store.load_file(&manifest).await.is_err());
    Ok(())
}

#[tokio::test]
async fn missing_chunk_is_rejected() -> TribResult<()> {
    let bins = Arc::new(MemBins::default());
    let store = ChunkStore::new(bins.clone(), 4, 2);
    let manifest = store.store_file(b"abcdefgh", 1).await?;
    let first = &manifest.chunks[0];
    let bin = bins.bin(&first.bin).await?;
    bin.set(&KeyValue {
        key: first.key.clone(),
        value: String::new(),
    })
    .await?;

    assert!(store.load_file(&manifest).await.is_err());
    Ok(())
}

#[tokio::test]
async fn truncated_chunk_is_rejected() -> TribResult<()> {
    let bins = Arc::new(MemBins::default());
    let store = ChunkStore::new(bins.clone(), 4, 2);
    let manifest = store.store_file(b"abcdefgh", 1).await?;
    let first = &manifest.chunks[0];
    let bin = bins.bin(&first.bin).await?;
    bin.set(&KeyValue {
        key: first.key.clone(),
        value: STANDARD.encode(b"abc"),
    })
    .await?;

    assert!(store.load_file(&manifest).await.is_err());
    Ok(())
}

#[tokio::test]
async fn reordered_chunks_are_rejected() -> TribResult<()> {
    let bins = Arc::new(MemBins::default());
    let store = ChunkStore::new(bins, 4, 2);
    let mut manifest = store.store_file(b"abcdefgh", 1).await?;
    manifest.chunks.swap(0, 1);

    assert!(store.load_file(&manifest).await.is_err());
    Ok(())
}

#[tokio::test]
async fn empty_and_boundary_sized_files_round_trip() -> TribResult<()> {
    let bins = Arc::new(MemBins::default());
    let store = ChunkStore::new(bins, 4, 2);

    let empty = store.store_file(&[], 1).await?;
    assert!(empty.chunks.is_empty());
    assert_eq!(store.load_file(&empty).await?, Vec::<u8>::new());

    let boundary = store.store_file(b"abcdefgh", 2).await?;
    assert_eq!(boundary.chunks.len(), 2);
    assert_eq!(store.load_file(&boundary).await?, b"abcdefgh");
    Ok(())
}
