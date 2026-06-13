use binfs::{FsResult, ROOT_ID};

mod common;

use common::service;

#[tokio::test]
async fn service_runs_named_command_flow() -> FsResult<()> {
    let fs = service();

    fs.mkdir_path("/a", 0o755).await?;
    fs.write_file_path("/a/file", b"hello world").await?;
    assert_eq!(fs.list_dir_path("/a").await?, vec!["file".to_string()]);
    assert_eq!(fs.read_file_path("/a/file").await?, b"hello world");
    fs.write_file_path("/a/file", b"bye").await?;
    assert_eq!(fs.read_file_path("/a/file").await?, b"bye");
    fs.unlink_path("/a/file").await?;
    fs.rmdir_path("/a").await?;
    assert!(fs.list_dir_path("/a").await.is_err());
    Ok(())
}

#[tokio::test]
async fn rmdir_rejects_non_empty_directory() -> FsResult<()> {
    let fs = service();

    fs.mkdir_path("/a", 0o755).await?;
    fs.write_file_path("/a/file", b"hello").await?;
    let err = fs.rmdir_path("/a").await.unwrap_err();
    assert_eq!(err.errno, libc::ENOTEMPTY);
    Ok(())
}

#[tokio::test]
async fn create_without_overwrite_rejects_existing_file() -> FsResult<()> {
    let fs = service();

    let first = fs.create_file(ROOT_ID, "f", 0o644, false).await?;
    let err = fs
        .create_file(ROOT_ID, "f", 0o644, false)
        .await
        .unwrap_err();
    assert_eq!(err.errno, libc::EEXIST);
    assert_eq!(
        fs.snapshot().await?.lookup_child(ROOT_ID, "f").unwrap().id,
        first.id
    );
    Ok(())
}

#[tokio::test]
async fn uncommitted_file_reads_as_empty() -> FsResult<()> {
    let fs = service();
    let file = fs.create_file(ROOT_ID, "f", 0o644, false).await?;

    assert_eq!(fs.read_file(&file.id).await?, Vec::<u8>::new());
    Ok(())
}

#[tokio::test]
async fn commit_after_unlink_is_rejected() -> FsResult<()> {
    let fs = service();
    let file = fs.create_file(ROOT_ID, "f", 0o644, false).await?;
    fs.unlink(ROOT_ID, "f").await?;

    let err = fs.commit_file(&file.id, b"orphan").await.unwrap_err();
    assert_eq!(err.errno, libc::ENOENT);
    assert!(fs.snapshot().await?.resolve_path("/f").is_none());
    Ok(())
}

#[tokio::test]
async fn create_under_missing_parent_is_rejected() {
    let fs = service();

    let err = fs
        .create_file("missing", "f", 0o644, false)
        .await
        .unwrap_err();
    assert_eq!(err.errno, libc::ENOENT);
}
