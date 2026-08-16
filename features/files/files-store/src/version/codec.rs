//! Deterministic binary encodings for the three object kinds this backend
//! writes through [`crate::version::objects::ObjectStore`]: [`Tree`], [`Commit`], and
//! [`CopyHistory`]. Byte-identical objects always encode to byte-identical
//! bytes, which is what makes blake3-of-the-encoding a stable content
//! address (mirrors `crate::chunk::Manifest`'s own contract).

use jj_lib::backend::{
    ChangeId, Commit, CommitId, CopyHistory, CopyId, FileId, MillisSinceEpoch, SecureSig,
    Signature, SymlinkId, Timestamp, Tree, TreeId, TreeValue,
};
use jj_lib::merge::{Merge, MergeBuilder};
use jj_lib::object_id::ObjectId as _;
use jj_lib::repo_path::{RepoPathBuf, RepoPathComponentBuf};

use crate::version::error::{Error, Result};

fn put_bytes(buf: &mut Vec<u8>, bytes: &[u8]) {
    buf.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
    buf.extend_from_slice(bytes);
}

fn put_str(buf: &mut Vec<u8>, s: &str) {
    put_bytes(buf, s.as_bytes());
}

fn put_id(buf: &mut Vec<u8>, id: &[u8]) {
    put_bytes(buf, id);
}

struct Cursor<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }

    fn take_bytes(&mut self) -> Result<&'a [u8]> {
        let len = self.take_u32()? as usize;
        let end = self
            .pos
            .checked_add(len)
            .filter(|&end| end <= self.bytes.len())
            .ok_or_else(|| Error::Object("truncated bytes field".into()))?;
        let slice = &self.bytes[self.pos..end];
        self.pos = end;
        Ok(slice)
    }

    fn take_str(&mut self) -> Result<String> {
        let bytes = self.take_bytes()?;
        String::from_utf8(bytes.to_vec()).map_err(|e| Error::Object(format!("invalid utf-8: {e}")))
    }

    fn take_u32(&mut self) -> Result<u32> {
        let end = self
            .pos
            .checked_add(4)
            .filter(|&end| end <= self.bytes.len())
            .ok_or_else(|| Error::Object("truncated u32 field".into()))?;
        let value = u32::from_le_bytes(self.bytes[self.pos..end].try_into().unwrap());
        self.pos = end;
        Ok(value)
    }

    fn take_u8(&mut self) -> Result<u8> {
        let byte = *self
            .bytes
            .get(self.pos)
            .ok_or_else(|| Error::Object("truncated u8 field".into()))?;
        self.pos += 1;
        Ok(byte)
    }

    fn take_i64(&mut self) -> Result<i64> {
        let end = self
            .pos
            .checked_add(8)
            .filter(|&end| end <= self.bytes.len())
            .ok_or_else(|| Error::Object("truncated i64 field".into()))?;
        let value = i64::from_le_bytes(self.bytes[self.pos..end].try_into().unwrap());
        self.pos = end;
        Ok(value)
    }

    fn take_i32(&mut self) -> Result<i32> {
        let end = self
            .pos
            .checked_add(4)
            .filter(|&end| end <= self.bytes.len())
            .ok_or_else(|| Error::Object("truncated i32 field".into()))?;
        let value = i32::from_le_bytes(self.bytes[self.pos..end].try_into().unwrap());
        self.pos = end;
        Ok(value)
    }

    fn finish(self) -> Result<()> {
        if self.pos != self.bytes.len() {
            return Err(Error::Object("trailing bytes after decode".into()));
        }
        Ok(())
    }
}

const TREE_MAGIC: &[u8; 4] = b"FTVT";
const COMMIT_MAGIC: &[u8; 4] = b"FTVC";
const COPY_MAGIC: &[u8; 4] = b"FTVP";
const VERSION: u8 = 1;

const TAG_FILE: u8 = 0;
const TAG_SYMLINK: u8 = 1;
const TAG_TREE: u8 = 2;
const TAG_GIT_SUBMODULE: u8 = 3;

pub fn encode_tree(tree: &Tree) -> Vec<u8> {
    let mut buf = Vec::new();
    buf.extend_from_slice(TREE_MAGIC);
    buf.push(VERSION);
    let entries: Vec<_> = tree.entries().collect();
    buf.extend_from_slice(&(entries.len() as u32).to_le_bytes());
    for entry in entries {
        put_str(&mut buf, entry.name().as_internal_str());
        match entry.value() {
            TreeValue::File {
                id,
                executable,
                copy_id,
            } => {
                buf.push(TAG_FILE);
                put_id(&mut buf, id.as_bytes());
                buf.push(u8::from(*executable));
                put_id(&mut buf, copy_id.as_bytes());
            }
            TreeValue::Symlink(id) => {
                buf.push(TAG_SYMLINK);
                put_id(&mut buf, id.as_bytes());
            }
            TreeValue::Tree(id) => {
                buf.push(TAG_TREE);
                put_id(&mut buf, id.as_bytes());
            }
            TreeValue::GitSubmodule(id) => {
                buf.push(TAG_GIT_SUBMODULE);
                put_id(&mut buf, id.as_bytes());
            }
        }
    }
    buf
}

pub fn decode_tree(bytes: &[u8]) -> Result<Tree> {
    if bytes.len() < 5 || &bytes[0..4] != TREE_MAGIC {
        return Err(Error::Object("bad tree magic".into()));
    }
    if bytes[4] != VERSION {
        return Err(Error::Object(format!(
            "unsupported tree version {}",
            bytes[4]
        )));
    }
    let mut cursor = Cursor::new(&bytes[5..]);
    let count = cursor.take_u32()?;
    let mut entries = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let name = cursor.take_str()?;
        let name = RepoPathComponentBuf::new(name)
            .map_err(|e| Error::Object(format!("invalid path component: {e}")))?;
        let tag = cursor.take_u8()?;
        let value = match tag {
            TAG_FILE => {
                let id = FileId::new(cursor.take_bytes()?.to_vec());
                let executable = cursor.take_u8()? != 0;
                let copy_id = CopyId::new(cursor.take_bytes()?.to_vec());
                TreeValue::File {
                    id,
                    executable,
                    copy_id,
                }
            }
            TAG_SYMLINK => TreeValue::Symlink(SymlinkId::new(cursor.take_bytes()?.to_vec())),
            TAG_TREE => TreeValue::Tree(TreeId::new(cursor.take_bytes()?.to_vec())),
            TAG_GIT_SUBMODULE => {
                TreeValue::GitSubmodule(CommitId::new(cursor.take_bytes()?.to_vec()))
            }
            other => return Err(Error::Object(format!("unknown tree value tag {other}"))),
        };
        entries.push((name, value));
    }
    cursor.finish()?;
    Ok(Tree::from_sorted_entries(entries))
}

fn put_id_merge<T: jj_lib::object_id::ObjectId>(buf: &mut Vec<u8>, merge: &Merge<T>) {
    let terms: Vec<_> = merge.iter().collect();
    buf.extend_from_slice(&(terms.len() as u32).to_le_bytes());
    for term in terms {
        put_id(buf, term.as_bytes());
    }
}

fn take_id_merge<T>(cursor: &mut Cursor<'_>, wrap: impl Fn(Vec<u8>) -> T) -> Result<Merge<T>> {
    let count = cursor.take_u32()?;
    let mut values = Vec::with_capacity(count as usize);
    for _ in 0..count {
        values.push(wrap(cursor.take_bytes()?.to_vec()));
    }
    let builder: MergeBuilder<T> = values.into_iter().collect();
    Ok(builder.build())
}

fn put_signature(buf: &mut Vec<u8>, signature: &Signature) {
    put_str(buf, &signature.name);
    put_str(buf, &signature.email);
    buf.extend_from_slice(&signature.timestamp.timestamp.0.to_le_bytes());
    buf.extend_from_slice(&signature.timestamp.tz_offset.to_le_bytes());
}

fn take_signature(cursor: &mut Cursor<'_>) -> Result<Signature> {
    let name = cursor.take_str()?;
    let email = cursor.take_str()?;
    let millis = cursor.take_i64()?;
    let tz_offset = cursor.take_i32()?;
    Ok(Signature {
        name,
        email,
        timestamp: Timestamp {
            timestamp: MillisSinceEpoch(millis),
            tz_offset,
        },
    })
}

pub fn encode_commit(commit: &Commit) -> Vec<u8> {
    let mut buf = Vec::new();
    buf.extend_from_slice(COMMIT_MAGIC);
    buf.push(VERSION);

    buf.extend_from_slice(&(commit.parents.len() as u32).to_le_bytes());
    for id in &commit.parents {
        put_id(&mut buf, id.as_bytes());
    }
    buf.extend_from_slice(&(commit.predecessors.len() as u32).to_le_bytes());
    for id in &commit.predecessors {
        put_id(&mut buf, id.as_bytes());
    }
    put_id_merge(&mut buf, &commit.root_tree);

    let labels: Vec<_> = commit.conflict_labels.iter().collect();
    buf.extend_from_slice(&(labels.len() as u32).to_le_bytes());
    for label in labels {
        put_str(&mut buf, label);
    }

    put_id(&mut buf, commit.change_id.as_bytes());
    put_str(&mut buf, &commit.description);
    put_signature(&mut buf, &commit.author);
    put_signature(&mut buf, &commit.committer);

    match &commit.secure_sig {
        Some(sig) => {
            buf.push(1);
            put_bytes(&mut buf, &sig.data);
            put_bytes(&mut buf, &sig.sig);
        }
        None => buf.push(0),
    }
    buf
}

pub fn decode_commit(bytes: &[u8]) -> Result<Commit> {
    if bytes.len() < 5 || &bytes[0..4] != COMMIT_MAGIC {
        return Err(Error::Object("bad commit magic".into()));
    }
    if bytes[4] != VERSION {
        return Err(Error::Object(format!(
            "unsupported commit version {}",
            bytes[4]
        )));
    }
    let mut cursor = Cursor::new(&bytes[5..]);

    let parent_count = cursor.take_u32()?;
    let mut parents = Vec::with_capacity(parent_count as usize);
    for _ in 0..parent_count {
        parents.push(CommitId::new(cursor.take_bytes()?.to_vec()));
    }
    let predecessor_count = cursor.take_u32()?;
    let mut predecessors = Vec::with_capacity(predecessor_count as usize);
    for _ in 0..predecessor_count {
        predecessors.push(CommitId::new(cursor.take_bytes()?.to_vec()));
    }
    let root_tree = take_id_merge(&mut cursor, TreeId::new)?;

    let label_count = cursor.take_u32()?;
    let mut labels = Vec::with_capacity(label_count as usize);
    for _ in 0..label_count {
        labels.push(cursor.take_str()?);
    }
    let conflict_labels_builder: MergeBuilder<String> = labels.into_iter().collect();
    let conflict_labels = conflict_labels_builder.build();

    let change_id = ChangeId::new(cursor.take_bytes()?.to_vec());
    let description = cursor.take_str()?;
    let author = take_signature(&mut cursor)?;
    let committer = take_signature(&mut cursor)?;

    let has_sig = cursor.take_u8()?;
    let secure_sig = if has_sig == 1 {
        let data = cursor.take_bytes()?.to_vec();
        let sig = cursor.take_bytes()?.to_vec();
        Some(SecureSig { data, sig })
    } else {
        None
    };
    cursor.finish()?;

    Ok(Commit {
        parents,
        predecessors,
        root_tree,
        conflict_labels,
        change_id,
        description,
        author,
        committer,
        secure_sig,
    })
}

pub fn encode_copy_history(history: &CopyHistory) -> Vec<u8> {
    let mut buf = Vec::new();
    buf.extend_from_slice(COPY_MAGIC);
    buf.push(VERSION);
    put_str(&mut buf, history.current_path.as_internal_file_string());
    buf.extend_from_slice(&(history.parents.len() as u32).to_le_bytes());
    for parent in &history.parents {
        put_id(&mut buf, parent.as_bytes());
    }
    put_bytes(&mut buf, &history.salt);
    buf
}

pub fn decode_copy_history(bytes: &[u8]) -> Result<CopyHistory> {
    if bytes.len() < 5 || &bytes[0..4] != COPY_MAGIC {
        return Err(Error::Object("bad copy-history magic".into()));
    }
    if bytes[4] != VERSION {
        return Err(Error::Object(format!(
            "unsupported copy-history version {}",
            bytes[4]
        )));
    }
    let mut cursor = Cursor::new(&bytes[5..]);
    let current_path = cursor.take_str()?;
    let current_path = RepoPathBuf::from_internal_string(current_path)
        .map_err(|e| Error::Object(format!("invalid repo path: {e}")))?;
    let parent_count = cursor.take_u32()?;
    let mut parents = Vec::with_capacity(parent_count as usize);
    for _ in 0..parent_count {
        parents.push(CopyId::new(cursor.take_bytes()?.to_vec()));
    }
    let salt = cursor.take_bytes()?.to_vec();
    cursor.finish()?;
    Ok(CopyHistory {
        current_path,
        parents,
        salt,
    })
}
