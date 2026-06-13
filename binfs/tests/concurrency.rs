use std::sync::Arc;

use binfs::{FsResult, ROOT_ID};
use tokio::sync::Barrier;

mod common;

use common::service;

const RACE_REPETITIONS: usize = 200;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_same_name_create_has_one_winner() -> FsResult<()> {
    for _ in 0..RACE_REPETITIONS {
        let fs = service();
        let barrier = Arc::new(Barrier::new(9));
        let mut tasks = Vec::new();
        for _ in 0..8 {
            let fs = fs.clone();
            let barrier = barrier.clone();
            tasks.push(tokio::spawn(async move {
                barrier.wait().await;
                fs.create_file(ROOT_ID, "same", 0o644, false).await
            }));
        }
        barrier.wait().await;

        let mut successes = 0;
        for task in tasks {
            if task.await.unwrap().is_ok() {
                successes += 1;
            }
        }
        assert_eq!(successes, 1);
        assert!(fs.snapshot().await?.resolve_path("/same").is_some());
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_writes_publish_one_complete_payload() -> FsResult<()> {
    let fs = service();
    fs.write_file_path("/same", b"initial").await?;
    let payloads = (0..8)
        .map(|index| format!("writer-{index}:{}", "x".repeat(1024)).into_bytes())
        .collect::<Vec<_>>();
    let barrier = Arc::new(Barrier::new(payloads.len() + 1));
    let mut tasks = Vec::new();

    for payload in payloads.clone() {
        let fs = fs.clone();
        let barrier = barrier.clone();
        tasks.push(tokio::spawn(async move {
            barrier.wait().await;
            fs.write_file_path("/same", &payload).await
        }));
    }
    barrier.wait().await;
    for task in tasks {
        task.await.unwrap()?;
    }

    let final_data = fs.read_file_path("/same").await?;
    assert!(payloads.contains(&final_data));
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn create_child_racing_parent_removal_preserves_invariants() -> FsResult<()> {
    for _ in 0..RACE_REPETITIONS {
        let fs = service();
        let parent = fs.mkdir(ROOT_ID, "parent", 0o755).await?;
        let barrier = Arc::new(Barrier::new(3));

        let create_task = {
            let fs = fs.clone();
            let barrier = barrier.clone();
            let parent_id = parent.id.clone();
            tokio::spawn(async move {
                barrier.wait().await;
                fs.create_file(&parent_id, "child", 0o644, false).await
            })
        };
        let remove_task = {
            let fs = fs.clone();
            let barrier = barrier.clone();
            tokio::spawn(async move {
                barrier.wait().await;
                fs.rmdir(ROOT_ID, "parent").await
            })
        };

        barrier.wait().await;
        let _ = create_task.await.unwrap();
        let _ = remove_task.await.unwrap();

        let snapshot = fs.snapshot().await?;
        match snapshot.resolve_path("/parent") {
            Some(parent) => {
                assert!(snapshot.lookup_child(&parent.id, "child").is_some());
            }
            None => assert!(snapshot.resolve_path("/parent/child").is_none()),
        }
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reads_during_commits_return_complete_versions() -> FsResult<()> {
    let fs = service();
    let initial = b"initial".repeat(256);
    let first = b"first".repeat(256);
    let second = b"second".repeat(256);
    fs.write_file_path("/f", &initial).await?;

    let barrier = Arc::new(Barrier::new(11));
    let mut tasks = Vec::new();
    for payload in [first.clone(), second.clone()] {
        let fs = fs.clone();
        let barrier = barrier.clone();
        tasks.push(tokio::spawn(async move {
            barrier.wait().await;
            fs.write_file_path("/f", &payload).await.map(|_| None)
        }));
    }
    for _ in 0..8 {
        let fs = fs.clone();
        let barrier = barrier.clone();
        tasks.push(tokio::spawn(async move {
            barrier.wait().await;
            fs.read_file_path("/f").await.map(Some)
        }));
    }
    barrier.wait().await;

    for task in tasks {
        if let Some(data) = task.await.unwrap()? {
            assert!(data == initial || data == first || data == second);
        }
    }
    let final_data = fs.read_file_path("/f").await?;
    assert!(final_data == first || final_data == second);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_distinct_creates_are_all_visible() -> FsResult<()> {
    let fs = service();
    let barrier = Arc::new(Barrier::new(17));
    let mut tasks = Vec::new();
    for index in 0..16 {
        let fs = fs.clone();
        let barrier = barrier.clone();
        tasks.push(tokio::spawn(async move {
            barrier.wait().await;
            fs.write_file_path(&format!("/file-{index}"), &[index as u8])
                .await
        }));
    }
    barrier.wait().await;
    for task in tasks {
        task.await.unwrap()?;
    }

    let entries = fs.list_dir_path("/").await?;
    assert_eq!(entries.len(), 16);
    for index in 0..16 {
        assert!(entries.contains(&format!("file-{index}")));
    }
    Ok(())
}
