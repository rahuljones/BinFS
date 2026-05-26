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
