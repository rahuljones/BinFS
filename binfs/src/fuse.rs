use std::{
    collections::HashMap,
    ffi::OsStr,
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use fuser::{
    BsdFileFlags, Config, Errno, FileAttr, FileHandle, FileType, Filesystem, FopenFlags,
    Generation, INodeNo, KernelConfig, LockOwner, MountOption, OpenFlags, ReplyAttr, ReplyCreate,
    ReplyData, ReplyDirectory, ReplyEmpty, ReplyEntry, ReplyOpen, ReplyStatfs, ReplyWrite, Request,
    TimeOrNow, WriteFlags,
};
use tokio::runtime::Runtime;

use crate::{
    metadata::{Node, NodeKind, ROOT_INO},
    service::{with_fs_timeout, BinFsService, FsError, FsResult},
};

const TTL: Duration = Duration::from_secs(1);

pub struct BinFuse {
    service: BinFsService,
    runtime: Runtime,
    next_fh: AtomicU64,
    handles: Mutex<HashMap<u64, WriteHandle>>,
    uid: u32,
    gid: u32,
}

struct WriteHandle {
    object: String,
    data: Vec<u8>,
    dirty: bool,
}

impl BinFuse {
    pub fn new(service: BinFsService) -> std::io::Result<Self> {
        Ok(Self {
            service,
            runtime: Runtime::new()?,
            next_fh: AtomicU64::new(1),
            handles: Mutex::new(HashMap::new()),
            uid: unsafe { libc::geteuid() },
            gid: unsafe { libc::getegid() },
        })
    }

    pub fn mount(self, mountpoint: impl AsRef<std::path::Path>) -> std::io::Result<()> {
        let mut config = Config::default();
        config.mount_options = vec![
            MountOption::FSName("binfs".to_string()),
            MountOption::DefaultPermissions,
        ];
        fuser::mount2(self, mountpoint, &config)
    }

    fn fs_wait<T, F>(&self, op: &'static str, future: F) -> FsResult<T>
    where
        F: std::future::Future<Output = FsResult<T>>,
    {
        self.runtime.block_on(with_fs_timeout(op, future))
    }

    fn alloc_handle(&self, object: String, data: Vec<u8>, dirty: bool) -> u64 {
        let fh = self.next_fh.fetch_add(1, Ordering::Relaxed);
        self.handles.lock().unwrap().insert(
            fh,
            WriteHandle {
                object,
                data,
                dirty,
            },
        );
        fh
    }

    fn attr_for(&self, node: &Node, ino: u64) -> FileAttr {
        let size = match node.kind {
            NodeKind::Directory => 0,
            NodeKind::File => node.manifest.as_ref().map(|m| m.size).unwrap_or(0),
        };
        let kind = match node.kind {
            NodeKind::Directory => FileType::Directory,
            NodeKind::File => FileType::RegularFile,
        };
        let time = UNIX_EPOCH + Duration::from_millis(node.modified_ms);
        FileAttr {
            ino: INodeNo(ino),
            size,
            blocks: size.div_ceil(512),
            atime: time,
            mtime: time,
            ctime: time,
            crtime: UNIX_EPOCH + Duration::from_millis(node.created_ms),
            kind,
            perm: node.mode,
            nlink: if node.kind == NodeKind::Directory {
                2
            } else {
                1
            },
            uid: self.uid,
            gid: self.gid,
            rdev: 0,
            blksize: self.service.chunk_size() as u32,
            flags: 0,
        }
    }

    fn parent_id(&self, parent: INodeNo) -> FsResult<String> {
        let snapshot = self.fs_wait("snapshot", self.service.snapshot())?;
        snapshot
            .id_for_ino(parent.into())
            .map(str::to_string)
            .ok_or_else(|| FsError::new(libc::ENOENT, "parent inode not found"))
    }

    fn node_attr(&self, ino: INodeNo) -> FsResult<FileAttr> {
        let raw_ino = u64::from(ino);
        let snapshot = self.fs_wait("snapshot", self.service.snapshot())?;
        let node = snapshot
            .node_by_ino(raw_ino)
            .ok_or_else(|| FsError::new(libc::ENOENT, "inode not found"))?;
        Ok(self.attr_for(node, raw_ino))
    }

    fn commit_handle(&self, fh: FileHandle) -> FsResult<()> {
        let mut handles = self.handles.lock().unwrap();
        let Some(handle) = handles.get_mut(&fh.into()) else {
            return Ok(());
        };
        if handle.dirty {
            self.fs_wait(
                "commit file",
                self.service.commit_file(&handle.object, &handle.data),
            )?;
            handle.dirty = false;
        }
        Ok(())
    }
}

impl Filesystem for BinFuse {
    fn init(&mut self, _req: &Request, _config: &mut KernelConfig) -> std::io::Result<()> {
        Ok(())
    }

    fn lookup(&self, _req: &Request, parent: INodeNo, name: &OsStr, reply: ReplyEntry) {
        let Some(name) = name.to_str() else {
            reply.error(Errno::EINVAL);
            return;
        };
        let result = (|| {
            let snapshot = self.fs_wait("snapshot", self.service.snapshot())?;
            let parent_id = snapshot
                .id_for_ino(parent.into())
                .ok_or_else(|| FsError::new(libc::ENOENT, "parent not found"))?;
            let node = snapshot
                .lookup_child(parent_id, name)
                .ok_or_else(|| FsError::new(libc::ENOENT, "entry not found"))?;
            let ino = snapshot
                .ino_for_id(&node.id)
                .ok_or_else(|| FsError::new(libc::EIO, "inode missing"))?;
            Ok::<_, FsError>(self.attr_for(node, ino))
        })();
        match result {
            Ok(attr) => reply.entry(&TTL, &attr, Generation(0)),
            Err(err) => reply.error(errno(err.errno)),
        }
    }

    fn getattr(&self, _req: &Request, ino: INodeNo, _fh: Option<FileHandle>, reply: ReplyAttr) {
        match self.node_attr(ino) {
            Ok(attr) => reply.attr(&TTL, &attr),
            Err(err) => reply.error(errno(err.errno)),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn setattr(
        &self,
        _req: &Request,
        ino: INodeNo,
        _mode: Option<u32>,
        _uid: Option<u32>,
        _gid: Option<u32>,
        size: Option<u64>,
        _atime: Option<TimeOrNow>,
        _mtime: Option<TimeOrNow>,
        _ctime: Option<SystemTime>,
        fh: Option<FileHandle>,
        _crtime: Option<SystemTime>,
        _chgtime: Option<SystemTime>,
        _bkuptime: Option<SystemTime>,
        _flags: Option<BsdFileFlags>,
        reply: ReplyAttr,
    ) {
        let result = (|| {
            if let Some(size) = size {
                if let Some(fh) = fh {
                    let mut handles = self.handles.lock().unwrap();
                    if let Some(handle) = handles.get_mut(&fh.into()) {
                        handle.data.resize(size as usize, 0);
                        handle.dirty = true;
                    }
                } else {
                    let snapshot = self.fs_wait("snapshot", self.service.snapshot())?;
                    let node = snapshot
                        .node_by_ino(ino.into())
                        .ok_or_else(|| FsError::new(libc::ENOENT, "file not found"))?;
                    if node.kind != NodeKind::File {
                        return Err(FsError::new(libc::EISDIR, "cannot truncate directory"));
                    }
                    let mut data = self.fs_wait("read file", self.service.read_file(&node.id))?;
                    data.resize(size as usize, 0);
                    self.fs_wait("commit file", self.service.commit_file(&node.id, &data))?;
                }
            }
            self.node_attr(ino)
        })();
        match result {
            Ok(attr) => reply.attr(&TTL, &attr),
            Err(err) => reply.error(errno(err.errno)),
        }
    }

    fn mkdir(
        &self,
        _req: &Request,
        parent: INodeNo,
        name: &OsStr,
        mode: u32,
        _umask: u32,
        reply: ReplyEntry,
    ) {
        let Some(name) = name.to_str() else {
            reply.error(Errno::EINVAL);
            return;
        };
        let result = (|| {
            let parent_id = self.parent_id(parent)?;
            let node = self.fs_wait(
                "mkdir",
                self.service.mkdir(&parent_id, name, (mode & 0o777) as u16),
            )?;
            let snapshot = self.fs_wait("snapshot", self.service.snapshot())?;
            let ino = snapshot
                .ino_for_id(&node.id)
                .ok_or_else(|| FsError::new(libc::EIO, "inode missing"))?;
            Ok::<_, FsError>(self.attr_for(&node, ino))
        })();
        match result {
            Ok(attr) => reply.entry(&TTL, &attr, Generation(0)),
            Err(err) => reply.error(errno(err.errno)),
        }
    }

    fn unlink(&self, _req: &Request, parent: INodeNo, name: &OsStr, reply: ReplyEmpty) {
        let Some(name) = name.to_str() else {
            reply.error(Errno::EINVAL);
            return;
        };
        let result = (|| {
            let parent_id = self.parent_id(parent)?;
            self.fs_wait("unlink", self.service.unlink(&parent_id, name))
        })();
        match result {
            Ok(()) => reply.ok(),
            Err(err) => reply.error(errno(err.errno)),
        }
    }

    fn rmdir(&self, _req: &Request, parent: INodeNo, name: &OsStr, reply: ReplyEmpty) {
        let Some(name) = name.to_str() else {
            reply.error(Errno::EINVAL);
            return;
        };
        let result = (|| {
            let parent_id = self.parent_id(parent)?;
            self.fs_wait("rmdir", self.service.rmdir(&parent_id, name))
        })();
        match result {
            Ok(()) => reply.ok(),
            Err(err) => reply.error(errno(err.errno)),
        }
    }

    fn open(&self, _req: &Request, ino: INodeNo, _flags: OpenFlags, reply: ReplyOpen) {
        let result = (|| {
            let snapshot = self.fs_wait("snapshot", self.service.snapshot())?;
            let node = snapshot
                .node_by_ino(ino.into())
                .ok_or_else(|| FsError::new(libc::ENOENT, "file not found"))?;
            if node.kind != NodeKind::File {
                return Err(FsError::new(libc::EISDIR, "cannot open directory"));
            }
            let data = self.fs_wait("read file", self.service.read_file(&node.id))?;
            Ok::<_, FsError>(self.alloc_handle(node.id.clone(), data, false))
        })();
        match result {
            Ok(fh) => reply.opened(FileHandle(fh), FopenFlags::empty()),
            Err(err) => reply.error(errno(err.errno)),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn read(
        &self,
        _req: &Request,
        ino: INodeNo,
        fh: FileHandle,
        offset: u64,
        size: u32,
        _flags: OpenFlags,
        _lock_owner: Option<LockOwner>,
        reply: ReplyData,
    ) {
        let result = (|| {
            if let Some(handle) = self.handles.lock().unwrap().get(&fh.into()) {
                return Ok(slice_read(&handle.data, offset, size));
            }
            let snapshot = self.fs_wait("snapshot", self.service.snapshot())?;
            let node = snapshot
                .node_by_ino(ino.into())
                .ok_or_else(|| FsError::new(libc::ENOENT, "file not found"))?;
            let data = self.fs_wait("read file", self.service.read_file(&node.id))?;
            Ok::<_, FsError>(slice_read(&data, offset, size))
        })();
        match result {
            Ok(data) => reply.data(&data),
            Err(err) => reply.error(errno(err.errno)),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn write(
        &self,
        _req: &Request,
        _ino: INodeNo,
        fh: FileHandle,
        offset: u64,
        data: &[u8],
        _write_flags: WriteFlags,
        _flags: OpenFlags,
        _lock_owner: Option<LockOwner>,
        reply: ReplyWrite,
    ) {
        let mut handles = self.handles.lock().unwrap();
        let Some(handle) = handles.get_mut(&fh.into()) else {
            reply.error(Errno::EBADF);
            return;
        };
        let offset = offset as usize;
        if handle.data.len() < offset {
            handle.data.resize(offset, 0);
        }
        if handle.data.len() < offset + data.len() {
            handle.data.resize(offset + data.len(), 0);
        }
        handle.data[offset..offset + data.len()].copy_from_slice(data);
        handle.dirty = true;
        reply.written(data.len() as u32);
    }

    fn flush(
        &self,
        _req: &Request,
        _ino: INodeNo,
        fh: FileHandle,
        _lock_owner: LockOwner,
        reply: ReplyEmpty,
    ) {
        match self.commit_handle(fh) {
            Ok(()) => reply.ok(),
            Err(err) => reply.error(errno(err.errno)),
        }
    }

    fn release(
        &self,
        _req: &Request,
        _ino: INodeNo,
        fh: FileHandle,
        _flags: OpenFlags,
        _lock_owner: Option<LockOwner>,
        _flush: bool,
        reply: ReplyEmpty,
    ) {
        let result = self.commit_handle(fh);
        self.handles.lock().unwrap().remove(&fh.into());
        match result {
            Ok(()) => reply.ok(),
            Err(err) => reply.error(errno(err.errno)),
        }
    }

    fn opendir(&self, _req: &Request, ino: INodeNo, _flags: OpenFlags, reply: ReplyOpen) {
        match self.node_attr(ino) {
            Ok(attr) if attr.kind == FileType::Directory => {
                reply.opened(FileHandle(0), FopenFlags::empty())
            }
            Ok(_) => reply.error(Errno::ENOTDIR),
            Err(err) => reply.error(errno(err.errno)),
        }
    }

    fn readdir(
        &self,
        _req: &Request,
        ino: INodeNo,
        _fh: FileHandle,
        offset: u64,
        mut reply: ReplyDirectory,
    ) {
        let result = (|| {
            let snapshot = self.fs_wait("snapshot", self.service.snapshot())?;
            let node = snapshot
                .node_by_ino(ino.into())
                .ok_or_else(|| FsError::new(libc::ENOENT, "directory not found"))?;
            if node.kind != NodeKind::Directory {
                return Err(FsError::new(libc::ENOTDIR, "not a directory"));
            }
            let parent_ino = node
                .parent
                .as_ref()
                .and_then(|parent| snapshot.ino_for_id(parent))
                .unwrap_or(ROOT_INO);
            let mut entries = vec![
                (u64::from(ino), FileType::Directory, ".".to_string()),
                (parent_ino, FileType::Directory, "..".to_string()),
            ];
            entries.extend(
                snapshot
                    .children_of(&node.id)
                    .into_iter()
                    .filter_map(|child| {
                        let ino = snapshot.ino_for_id(&child.id)?;
                        let kind = match child.kind {
                            NodeKind::Directory => FileType::Directory,
                            NodeKind::File => FileType::RegularFile,
                        };
                        Some((ino, kind, child.name.clone()))
                    }),
            );
            Ok::<_, FsError>(entries)
        })();
        let entries = match result {
            Ok(entries) => entries,
            Err(err) => {
                reply.error(errno(err.errno));
                return;
            }
        };
        for (idx, (entry_ino, kind, name)) in entries.into_iter().enumerate().skip(offset as usize)
        {
            if reply.add(INodeNo(entry_ino), (idx + 1) as u64, kind, name) {
                break;
            }
        }
        reply.ok();
    }

    fn create(
        &self,
        _req: &Request,
        parent: INodeNo,
        name: &OsStr,
        mode: u32,
        _umask: u32,
        _flags: i32,
        reply: ReplyCreate,
    ) {
        let Some(name) = name.to_str() else {
            reply.error(Errno::EINVAL);
            return;
        };
        let result = (|| {
            let parent_id = self.parent_id(parent)?;
            let node = self.fs_wait(
                "create file",
                self.service
                    .create_file(&parent_id, name, (mode & 0o777) as u16, true),
            )?;
            let snapshot = self.fs_wait("snapshot", self.service.snapshot())?;
            let ino = snapshot
                .ino_for_id(&node.id)
                .ok_or_else(|| FsError::new(libc::EIO, "inode missing"))?;
            let fh = self.alloc_handle(node.id.clone(), Vec::new(), true);
            Ok::<_, FsError>((self.attr_for(&node, ino), fh))
        })();
        match result {
            Ok((attr, fh)) => reply.created(
                &TTL,
                &attr,
                Generation(0),
                FileHandle(fh),
                FopenFlags::empty(),
            ),
            Err(err) => reply.error(errno(err.errno)),
        }
    }

    fn statfs(&self, _req: &Request, _ino: INodeNo, reply: ReplyStatfs) {
        let files = self
            .fs_wait("snapshot", self.service.snapshot())
            .map(|snapshot| snapshot.live_node_count())
            .unwrap_or(0);
        reply.statfs(
            1_000_000, 1_000_000, 1_000_000, files, 1_000_000, 4096, 255, 4096,
        );
    }
}

fn errno(errno: i32) -> Errno {
    Errno::from_i32(errno)
}

fn slice_read(data: &[u8], offset: u64, size: u32) -> Vec<u8> {
    let offset = offset as usize;
    if offset >= data.len() {
        return Vec::new();
    }
    let end = data.len().min(offset + size as usize);
    data[offset..end].to_vec()
}
