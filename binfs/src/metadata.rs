use std::collections::{BTreeMap, HashMap};

use serde::{Deserialize, Serialize};

pub const ROOT_ID: &str = "root";
pub const ROOT_INO: u64 = 1;

use crate::chunks::FileManifest;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeKind {
    Directory,
    File,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Node {
    pub id: String,
    pub parent: Option<String>,
    pub name: String,
    pub kind: NodeKind,
    pub mode: u16,
    pub created_ms: u64,
    pub modified_ms: u64,
    pub live: bool,
    pub manifest: Option<FileManifest>,
}

impl Node {
    fn root() -> Self {
        Self {
            id: ROOT_ID.to_string(),
            parent: None,
            name: String::new(),
            kind: NodeKind::Directory,
            mode: 0o755,
            created_ms: 0,
            modified_ms: 0,
            live: true,
            manifest: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FsOp {
    Mkdir {
        op_id: String,
        parent: String,
        name: String,
        object: String,
        mode: u16,
        mtime_ms: u64,
    },
    CreateFile {
        op_id: String,
        parent: String,
        name: String,
        object: String,
        mode: u16,
        mtime_ms: u64,
    },
    CommitFile {
        op_id: String,
        object: String,
        manifest: FileManifest,
        mtime_ms: u64,
    },
    Unlink {
        op_id: String,
        parent: String,
        name: String,
        mtime_ms: u64,
    },
    Rmdir {
        op_id: String,
        parent: String,
        name: String,
        mtime_ms: u64,
    },
}

impl FsOp {
    pub fn op_id(&self) -> &str {
        match self {
            FsOp::Mkdir { op_id, .. }
            | FsOp::CreateFile { op_id, .. }
            | FsOp::CommitFile { op_id, .. }
            | FsOp::Unlink { op_id, .. }
            | FsOp::Rmdir { op_id, .. } => op_id,
        }
    }
}

#[derive(Debug, Clone)]
pub struct FsSnapshot {
    nodes: BTreeMap<String, Node>,
    children: BTreeMap<String, BTreeMap<String, String>>,
    ino_to_id: BTreeMap<u64, String>,
    id_to_ino: HashMap<String, u64>,
    effective_ops: HashMap<String, bool>,
}

impl FsSnapshot {
    pub fn replay(ops: &[FsOp]) -> Self {
        let mut snapshot = Self {
            nodes: BTreeMap::from([(ROOT_ID.to_string(), Node::root())]),
            children: BTreeMap::from([(ROOT_ID.to_string(), BTreeMap::new())]),
            ino_to_id: BTreeMap::from([(ROOT_INO, ROOT_ID.to_string())]),
            id_to_ino: HashMap::from([(ROOT_ID.to_string(), ROOT_INO)]),
            effective_ops: HashMap::new(),
        };

        for op in ops {
            let effective = snapshot.apply(op);
            snapshot
                .effective_ops
                .insert(op.op_id().to_string(), effective);
        }

        snapshot
    }

    pub fn op_effective(&self, op_id: &str) -> bool {
        self.effective_ops.get(op_id).copied().unwrap_or(false)
    }

    pub fn node(&self, id: &str) -> Option<&Node> {
        self.nodes.get(id).filter(|node| node.live)
    }

    pub fn node_by_ino(&self, ino: u64) -> Option<&Node> {
        self.ino_to_id.get(&ino).and_then(|id| self.node(id))
    }

    pub fn id_for_ino(&self, ino: u64) -> Option<&str> {
        self.ino_to_id
            .get(&ino)
            .and_then(|id| self.node(id).map(|_| id.as_str()))
    }

    pub fn ino_for_id(&self, id: &str) -> Option<u64> {
        self.id_to_ino.get(id).copied()
    }

    pub fn lookup_child(&self, parent: &str, name: &str) -> Option<&Node> {
        let child_id = self.children.get(parent)?.get(name)?;
        self.node(child_id)
    }

    pub fn child_id(&self, parent: &str, name: &str) -> Option<&str> {
        self.children
            .get(parent)?
            .get(name)
            .and_then(|child_id| self.node(child_id).map(|_| child_id.as_str()))
    }

    pub fn children_of(&self, parent: &str) -> Vec<&Node> {
        self.children
            .get(parent)
            .into_iter()
            .flat_map(|children| children.values())
            .filter_map(|id| self.node(id))
            .collect()
    }

    pub fn resolve_path(&self, path: &str) -> Option<&Node> {
        if path == "/" || path.is_empty() {
            return self.node(ROOT_ID);
        }

        let mut current = ROOT_ID.to_string();
        for part in path.split('/').filter(|part| !part.is_empty()) {
            let child = self.child_id(&current, part)?;
            current = child.to_string();
        }
        self.node(&current)
    }

    pub fn live_node_count(&self) -> u64 {
        self.nodes.values().filter(|node| node.live).count() as u64
    }

    fn apply(&mut self, op: &FsOp) -> bool {
        match op {
            FsOp::Mkdir {
                parent,
                name,
                object,
                mode,
                mtime_ms,
                ..
            } => self.create_node(parent, name, object, NodeKind::Directory, *mode, *mtime_ms),
            FsOp::CreateFile {
                parent,
                name,
                object,
                mode,
                mtime_ms,
                ..
            } => self.create_node(parent, name, object, NodeKind::File, *mode, *mtime_ms),
            FsOp::CommitFile {
                object,
                manifest,
                mtime_ms,
                ..
            } => match self.nodes.get_mut(object) {
                Some(node) if node.live && node.kind == NodeKind::File => {
                    node.manifest = Some(manifest.clone());
                    node.modified_ms = *mtime_ms;
                    true
                }
                _ => false,
            },
            FsOp::Unlink {
                parent,
                name,
                mtime_ms,
                ..
            } => {
                let Some(child_id) = self.child_id(parent, name).map(str::to_string) else {
                    return false;
                };
                let Some(node) = self.nodes.get_mut(&child_id) else {
                    return false;
                };
                if node.kind != NodeKind::File {
                    return false;
                }
                node.live = false;
                node.modified_ms = *mtime_ms;
                if let Some(children) = self.children.get_mut(parent) {
                    children.remove(name);
                }
                true
            }
            FsOp::Rmdir {
                parent,
                name,
                mtime_ms,
                ..
            } => {
                let Some(child_id) = self.child_id(parent, name).map(str::to_string) else {
                    return false;
                };
                let Some(node) = self.nodes.get(&child_id) else {
                    return false;
                };
                if node.kind != NodeKind::Directory {
                    return false;
                }
                if self
                    .children
                    .get(&child_id)
                    .map(|children| !children.is_empty())
                    .unwrap_or(false)
                {
                    return false;
                }
                if let Some(node) = self.nodes.get_mut(&child_id) {
                    node.live = false;
                    node.modified_ms = *mtime_ms;
                }
                if let Some(children) = self.children.get_mut(parent) {
                    children.remove(name);
                }
                true
            }
        }
    }

    fn create_node(
        &mut self,
        parent: &str,
        name: &str,
        object: &str,
        kind: NodeKind,
        mode: u16,
        mtime_ms: u64,
    ) -> bool {
        if name.is_empty() || name.contains('/') || object == ROOT_ID {
            return false;
        }
        if self.nodes.contains_key(object) {
            return false;
        }
        match self.nodes.get(parent) {
            Some(node) if node.live && node.kind == NodeKind::Directory => {}
            _ => return false,
        }
        if self
            .children
            .get(parent)
            .map(|children| children.contains_key(name))
            .unwrap_or(false)
        {
            return false;
        }

        let ino = self.ino_to_id.len() as u64 + 1;
        self.ino_to_id.insert(ino, object.to_string());
        self.id_to_ino.insert(object.to_string(), ino);
        self.children
            .entry(parent.to_string())
            .or_default()
            .insert(name.to_string(), object.to_string());
        if kind == NodeKind::Directory {
            self.children.entry(object.to_string()).or_default();
        }
        self.nodes.insert(
            object.to_string(),
            Node {
                id: object.to_string(),
                parent: Some(parent.to_string()),
                name: name.to_string(),
                kind,
                mode,
                created_ms: mtime_ms,
                modified_ms: mtime_ms,
                live: true,
                manifest: None,
            },
        );
        true
    }
}
