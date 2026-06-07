use std::{
    fmt::{Display, Formatter},
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use tribbler::{
    err::{TribResult, TribblerError},
    storage::{BinStorage, KeyValue},
};
use uuid::Uuid;

use crate::{
    chunks::{ChunkStore, FileManifest},
    metadata::{FsOp, FsSnapshot, Node, NodeKind},
};

const OPS_KEY: &str = "fs:ops";
const STORAGE_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug, Clone)]
pub struct BinFsConfig {
    pub metadata_bin: String,
    pub chunk_size: usize,
    pub data_bins: usize,
}

impl Default for BinFsConfig {
    fn default() -> Self {
        Self {
            metadata_bin: "__fs_meta__".to_string(),
            chunk_size: 65_536,
            data_bins: 128,
        }
    }
}

#[derive(Debug, Clone)]
pub struct FsError {
    pub errno: i32,
    pub message: String,
}

pub type FsResult<T> = Result<T, FsError>;

impl FsError {
    pub fn new(errno: i32, message: impl Into<String>) -> Self {
        Self {
            errno,
            message: message.into(),
        }
    }

    pub fn from_storage(err: impl Display) -> Self {
        Self::new(libc::EIO, err.to_string())
    }
}

impl Display for FsError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.errno, self.message)
    }
}

impl std::error::Error for FsError {}

#[derive(Clone)]
pub struct BinFsService {
    bins: Arc<dyn BinStorage>,
    metadata_bin: String,
    chunk_store: ChunkStore,
}

impl BinFsService {
    pub fn new(bins: Arc<dyn BinStorage>, config: BinFsConfig) -> Self {
        let chunk_store = ChunkStore::new(bins.clone(), config.chunk_size, config.data_bins);
        Self {
            bins,
            metadata_bin: config.metadata_bin,
            chunk_store,
        }
    }

    pub fn chunk_size(&self) -> usize {
        self.chunk_store.chunk_size()
    }

    pub async fn health_check(&self) -> FsResult<()> {
        let bin = self.metadata_storage().await?;
        with_storage_timeout("metadata health check", bin.clock(0)).await?;
        Ok(())
    }

    pub async fn snapshot(&self) -> FsResult<FsSnapshot> {
        let ops = self.load_ops().await?;
        Ok(FsSnapshot::replay(&ops))
    }

    pub async fn mkdir(&self, parent: &str, name: &str, mode: u16) -> FsResult<Node> {
        let op_id = new_id("op");
        let object = new_id("dir");
        let op = FsOp::Mkdir {
            op_id: op_id.clone(),
            parent: parent.to_string(),
            name: name.to_string(),
            object: object.clone(),
            mode,
            mtime_ms: now_ms(),
        };
        self.append_op(&op).await?;
        let snapshot = self.snapshot().await?;
        if snapshot.op_effective(&op_id) {
            return snapshot
                .node(&object)
                .cloned()
                .ok_or_else(|| FsError::new(libc::EIO, "mkdir op effective but node missing"));
        }
        Err(classify_create_error(
            &snapshot,
            parent,
            name,
            NodeKind::Directory,
        ))
    }

    pub async fn create_file(
        &self,
        parent: &str,
        name: &str,
        mode: u16,
        overwrite: bool,
    ) -> FsResult<Node> {
        let snapshot = self.snapshot().await?;
        if let Some(existing) = snapshot.lookup_child(parent, name) {
            return match existing.kind {
                NodeKind::Directory => {
                    Err(FsError::new(libc::EISDIR, "destination is a directory"))
                }
                NodeKind::File if overwrite => Ok(existing.clone()),
                NodeKind::File => Err(FsError::new(libc::EEXIST, "file already exists")),
            };
        }

        let op_id = new_id("op");
        let object = new_id("file");
        let op = FsOp::CreateFile {
            op_id: op_id.clone(),
            parent: parent.to_string(),
            name: name.to_string(),
            object: object.clone(),
            mode,
            mtime_ms: now_ms(),
        };
        self.append_op(&op).await?;
        let snapshot = self.snapshot().await?;
        if snapshot.op_effective(&op_id) {
            return snapshot
                .node(&object)
                .cloned()
                .ok_or_else(|| FsError::new(libc::EIO, "create op effective but node missing"));
        }
        Err(classify_create_error(
            &snapshot,
            parent,
            name,
            NodeKind::File,
        ))
    }

    pub async fn commit_file(&self, object: &str, data: &[u8]) -> FsResult<FileManifest> {
        let mtime_ms = now_ms();
        let manifest = self
            .chunk_store
            .store_file(data, mtime_ms)
            .await
            .map_err(FsError::from_storage)?;
        let op_id = new_id("op");
        let op = FsOp::CommitFile {
            op_id: op_id.clone(),
            object: object.to_string(),
            manifest: manifest.clone(),
            mtime_ms,
        };
        self.append_op(&op).await?;
        let snapshot = self.snapshot().await?;
        if snapshot.op_effective(&op_id) {
            Ok(manifest)
        } else {
            Err(FsError::new(libc::ENOENT, "file no longer exists"))
        }
    }

    pub async fn read_file(&self, object: &str) -> FsResult<Vec<u8>> {
        let snapshot = self.snapshot().await?;
        let node = snapshot
            .node(object)
            .ok_or_else(|| FsError::new(libc::ENOENT, "file not found"))?;
        if node.kind != NodeKind::File {
            return Err(FsError::new(libc::EISDIR, "cannot read a directory"));
        }
        let Some(manifest) = &node.manifest else {
            return Ok(Vec::new());
        };
        self.chunk_store
            .load_file(manifest)
            .await
            .map_err(FsError::from_storage)
    }

    pub async fn unlink(&self, parent: &str, name: &str) -> FsResult<()> {
        let op_id = new_id("op");
        let op = FsOp::Unlink {
            op_id: op_id.clone(),
            parent: parent.to_string(),
            name: name.to_string(),
            mtime_ms: now_ms(),
        };
        self.append_op(&op).await?;
        let snapshot = self.snapshot().await?;
        if snapshot.op_effective(&op_id) {
            Ok(())
        } else {
            match snapshot.lookup_child(parent, name) {
                Some(node) if node.kind == NodeKind::Directory => {
                    Err(FsError::new(libc::EISDIR, "target is a directory"))
                }
                Some(_) => Err(FsError::new(libc::EIO, "unlink lost a metadata race")),
                None => Err(FsError::new(libc::ENOENT, "file not found")),
            }
        }
    }

    pub async fn rmdir(&self, parent: &str, name: &str) -> FsResult<()> {
        let op_id = new_id("op");
        let op = FsOp::Rmdir {
            op_id: op_id.clone(),
            parent: parent.to_string(),
            name: name.to_string(),
            mtime_ms: now_ms(),
        };
        self.append_op(&op).await?;
        let snapshot = self.snapshot().await?;
        if snapshot.op_effective(&op_id) {
            Ok(())
        } else {
            match snapshot.lookup_child(parent, name) {
                Some(node) if node.kind == NodeKind::File => {
                    Err(FsError::new(libc::ENOTDIR, "target is not a directory"))
                }
                Some(node) if !snapshot.children_of(&node.id).is_empty() => {
                    Err(FsError::new(libc::ENOTEMPTY, "directory is not empty"))
                }
                Some(_) => Err(FsError::new(libc::EIO, "rmdir lost a metadata race")),
                None => Err(FsError::new(libc::ENOENT, "directory not found")),
            }
        }
    }

    pub async fn mkdir_path(&self, path: &str, mode: u16) -> FsResult<Node> {
        let (parent, name) = self.resolve_parent(path).await?;
        self.mkdir(&parent, &name, mode).await
    }

    pub async fn write_file_path(&self, path: &str, data: &[u8]) -> FsResult<FileManifest> {
        let (parent, name) = self.resolve_parent(path).await?;
        let node = self.create_file(&parent, &name, 0o644, true).await?;
        self.commit_file(&node.id, data).await
    }

    pub async fn read_file_path(&self, path: &str) -> FsResult<Vec<u8>> {
        let snapshot = self.snapshot().await?;
        let node = snapshot
            .resolve_path(path)
            .ok_or_else(|| FsError::new(libc::ENOENT, "file not found"))?;
        self.read_file(&node.id).await
    }

    pub async fn list_dir_path(&self, path: &str) -> FsResult<Vec<String>> {
        let snapshot = self.snapshot().await?;
        let node = snapshot
            .resolve_path(path)
            .ok_or_else(|| FsError::new(libc::ENOENT, "directory not found"))?;
        if node.kind != NodeKind::Directory {
            return Err(FsError::new(libc::ENOTDIR, "not a directory"));
        }
        Ok(snapshot
            .children_of(&node.id)
            .into_iter()
            .map(|child| child.name.clone())
            .collect())
    }

    pub async fn unlink_path(&self, path: &str) -> FsResult<()> {
        let (parent, name) = self.resolve_parent(path).await?;
        self.unlink(&parent, &name).await
    }

    pub async fn rmdir_path(&self, path: &str) -> FsResult<()> {
        let (parent, name) = self.resolve_parent(path).await?;
        self.rmdir(&parent, &name).await
    }

    async fn resolve_parent(&self, path: &str) -> FsResult<(String, String)> {
        let (parent_path, name) = split_parent(path)?;
        let snapshot = self.snapshot().await?;
        let parent = snapshot
            .resolve_path(&parent_path)
            .ok_or_else(|| FsError::new(libc::ENOENT, "parent not found"))?;
        if parent.kind != NodeKind::Directory {
            return Err(FsError::new(libc::ENOTDIR, "parent is not a directory"));
        }
        Ok((parent.id.clone(), name))
    }

    async fn load_ops(&self) -> FsResult<Vec<FsOp>> {
        let bin = self.metadata_storage().await?;
        let raw = with_storage_timeout("metadata list_get", bin.list_get(OPS_KEY))
            .await?
            .0;
        Ok(raw
            .iter()
            .filter_map(|entry| serde_json::from_str::<FsOp>(entry).ok())
            .collect())
    }

    async fn append_op(&self, op: &FsOp) -> FsResult<()> {
        let bin = self.metadata_storage().await?;
        let value = serde_json::to_string(op).map_err(FsError::from_storage)?;
        with_storage_timeout(
            "metadata list_append",
            bin.list_append(&KeyValue {
                key: OPS_KEY.to_string(),
                value,
            }),
        )
        .await?;
        Ok(())
    }

    async fn metadata_storage(&self) -> FsResult<Box<dyn tribbler::storage::Storage>> {
        self.bins
            .bin(&self.metadata_bin)
            .await
            .map_err(FsError::from_storage)
    }
}

pub(crate) async fn with_storage_timeout<T, F>(op: &'static str, future: F) -> FsResult<T>
where
    F: std::future::Future<Output = TribResult<T>>,
{
    match tokio::time::timeout(STORAGE_TIMEOUT, future).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(err)) => Err(FsError::from_storage(err)),
        Err(_) => Err(FsError::new(
            libc::EIO,
            format!("{op} timed out after {} ms", STORAGE_TIMEOUT.as_millis()),
        )),
    }
}

pub(crate) async fn with_fs_timeout<T, F>(op: &'static str, future: F) -> FsResult<T>
where
    F: std::future::Future<Output = FsResult<T>>,
{
    match tokio::time::timeout(STORAGE_TIMEOUT, future).await {
        Ok(result) => result,
        Err(_) => Err(FsError::new(
            libc::EIO,
            format!("{op} timed out after {} ms", STORAGE_TIMEOUT.as_millis()),
        )),
    }
}

fn classify_create_error(
    snapshot: &FsSnapshot,
    parent: &str,
    name: &str,
    kind: NodeKind,
) -> FsError {
    match snapshot.node(parent) {
        None => FsError::new(libc::ENOENT, "parent not found"),
        Some(node) if node.kind != NodeKind::Directory => {
            FsError::new(libc::ENOTDIR, "parent is not a directory")
        }
        Some(_) => match snapshot.lookup_child(parent, name) {
            Some(node) if kind == NodeKind::File && node.kind == NodeKind::Directory => {
                FsError::new(libc::EISDIR, "destination is a directory")
            }
            Some(_) => FsError::new(libc::EEXIST, "entry already exists"),
            None => FsError::new(libc::EIO, "metadata operation was not effective"),
        },
    }
}

fn split_parent(path: &str) -> FsResult<(String, String)> {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() || trimmed == "/" {
        return Err(FsError::new(libc::EINVAL, "path has no parent"));
    }
    let (parent, name) = trimmed.rsplit_once('/').unwrap_or(("", trimmed));
    if name.is_empty() || name == "." || name == ".." || name.contains('/') {
        return Err(FsError::new(libc::EINVAL, "invalid path name"));
    }
    let parent = if parent.is_empty() { "/" } else { parent };
    Ok((parent.to_string(), name.to_string()))
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

fn new_id(prefix: &str) -> String {
    format!("{prefix}:{}", Uuid::new_v4())
}

#[allow(dead_code)]
fn trib_err(message: impl Into<String>) -> TribResult<()> {
    Err(Box::new(TribblerError::Unknown(message.into())))
}
