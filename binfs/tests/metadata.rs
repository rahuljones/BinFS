use binfs::{FileManifest, FsOp, FsSnapshot, NodeKind, ROOT_ID};

fn mkdir(op_id: &str, object: &str, name: &str) -> FsOp {
    FsOp::Mkdir {
        op_id: op_id.to_string(),
        parent: ROOT_ID.to_string(),
        name: name.to_string(),
        object: object.to_string(),
        mode: 0o755,
        mtime_ms: 1,
    }
}

fn create(op_id: &str, object: &str, parent: &str, name: &str) -> FsOp {
    FsOp::CreateFile {
        op_id: op_id.to_string(),
        parent: parent.to_string(),
        name: name.to_string(),
        object: object.to_string(),
        mode: 0o644,
        mtime_ms: 2,
    }
}

#[test]
fn replays_directories_and_files() {
    let ops = vec![
        mkdir("m1", "dir1", "a"),
        create("c1", "file1", "dir1", "hello"),
    ];
    let snapshot = FsSnapshot::replay(&ops);

    assert_eq!(
        snapshot.resolve_path("/a").unwrap().kind,
        NodeKind::Directory
    );
    assert_eq!(
        snapshot.resolve_path("/a/hello").unwrap().kind,
        NodeKind::File
    );
    assert_eq!(snapshot.ino_for_id("dir1"), Some(2));
    assert_eq!(snapshot.ino_for_id("file1"), Some(3));
}

#[test]
fn rejects_duplicate_create_by_log_order() {
    let ops = vec![
        create("c1", "file1", ROOT_ID, "same"),
        create("c2", "file2", ROOT_ID, "same"),
    ];
    let snapshot = FsSnapshot::replay(&ops);

    assert!(snapshot.op_effective("c1"));
    assert!(!snapshot.op_effective("c2"));
    assert_eq!(snapshot.lookup_child(ROOT_ID, "same").unwrap().id, "file1");
}

#[test]
fn overwrites_file_manifest_by_last_valid_commit() {
    let first = FileManifest::empty(64, 10);
    let mut second = FileManifest::empty(64, 11);
    second.size = 12;
    let ops = vec![
        create("c1", "file1", ROOT_ID, "f"),
        FsOp::CommitFile {
            op_id: "w1".to_string(),
            object: "file1".to_string(),
            manifest: first,
            mtime_ms: 10,
        },
        FsOp::CommitFile {
            op_id: "w2".to_string(),
            object: "file1".to_string(),
            manifest: second,
            mtime_ms: 11,
        },
    ];
    let snapshot = FsSnapshot::replay(&ops);

    assert_eq!(
        snapshot
            .resolve_path("/f")
            .unwrap()
            .manifest
            .as_ref()
            .unwrap()
            .size,
        12
    );
}

#[test]
fn rejects_non_empty_rmdir_then_allows_after_unlink() {
    let ops = vec![
        mkdir("m1", "dir1", "a"),
        create("c1", "file1", "dir1", "f"),
        FsOp::Rmdir {
            op_id: "r1".to_string(),
            parent: ROOT_ID.to_string(),
            name: "a".to_string(),
            mtime_ms: 3,
        },
        FsOp::Unlink {
            op_id: "u1".to_string(),
            parent: "dir1".to_string(),
            name: "f".to_string(),
            mtime_ms: 4,
        },
        FsOp::Rmdir {
            op_id: "r2".to_string(),
            parent: ROOT_ID.to_string(),
            name: "a".to_string(),
            mtime_ms: 5,
        },
    ];
    let snapshot = FsSnapshot::replay(&ops);

    assert!(!snapshot.op_effective("r1"));
    assert!(snapshot.op_effective("r2"));
    assert!(snapshot.resolve_path("/a").is_none());
}
