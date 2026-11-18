//! A file on the server read in parts, for media that plays while it downloads. It is read in
//! blocks: a sealed file's own pieces (`e2ee::seal_file_chunked`), each opened as it comes, or
//! equal parts of a plain one. Blocks read are kept, up to [`KEEP_BYTES`], so seeking back and
//! the sound and picture decoders reading the same parts fetch them once.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use hermes_proto::id::Id;
use hermes_proto::models::Attachment;

use crate::e2ee;
use crate::rest::Rest;

/// The size of a plain file's blocks.
const PLAIN_BLOCK: u32 = 64 * 1024;
/// How many bytes of a file's blocks stay in memory.
pub const KEEP_BYTES: usize = 64 * 1024 * 1024;

/// A sealed file's key and nonce prefix.
struct Seal {
    key: String,
    prefix: String,
}

#[derive(Default)]
struct Kept {
    blocks: HashMap<u64, Arc<Vec<u8>>>,
    /// In the order they came, the oldest first out.
    order: VecDeque<u64>,
    bytes: usize,
}

pub struct RemoteFile {
    rest: Rest,
    id: Id,
    stored_name: String,
    /// The file's size when open.
    size: u64,
    block: u32,
    seal: Option<Seal>,
    kept: Mutex<Kept>,
    /// One fetch at a time: a second reader of the same part waits and then finds it kept.
    fetching: tokio::sync::Mutex<()>,
}

impl RemoteFile {
    /// `None` for a file sealed as one block (or compressed): it can only be opened whole.
    pub fn new(rest: Rest, attachment: &Attachment) -> Option<RemoteFile> {
        let (stored_name, block, seal) = match &attachment.sealed {
            None => (attachment.filename.clone(), PLAIN_BLOCK, None),
            Some(sealed) => {
                let chunk = sealed.chunk_size.filter(|c| *c > 0)?;
                if sealed.compression.is_some() {
                    return None;
                }
                (
                    sealed.stored_name.clone(),
                    chunk,
                    Some(Seal {
                        key: sealed.key.clone(),
                        prefix: sealed.nonce.clone(),
                    }),
                )
            }
        };
        Some(RemoteFile {
            rest,
            id: attachment.id,
            stored_name,
            size: attachment.size,
            block,
            seal,
            kept: Mutex::default(),
            fetching: tokio::sync::Mutex::new(()),
        })
    }

    /// The file's size when open.
    pub fn size(&self) -> u64 {
        self.size
    }

    /// How many bytes of it are kept in memory now.
    pub fn kept_bytes(&self) -> usize {
        self.kept.lock().unwrap_or_else(|e| e.into_inner()).bytes
    }

    fn block_count(&self) -> u64 {
        e2ee::chunk_count(self.size, self.block)
    }

    /// Bytes `start..start + len` of the open file (fewer at its end).
    pub async fn read(&self, start: u64, len: u64) -> Result<Vec<u8>, String> {
        let end = (start + len).min(self.size);
        if start >= end {
            return Ok(Vec::new());
        }
        let step = u64::from(self.block);
        let (first, last) = (start / step, (end - 1) / step);
        let blocks = self.blocks(first, last).await?;
        let mut out = Vec::with_capacity((end - start) as usize);
        for (n, block) in blocks.iter().enumerate() {
            let at = (first + n as u64) * step;
            let from = start.saturating_sub(at) as usize;
            let to = ((end - at) as usize).min(block.len());
            out.extend_from_slice(&block[from.min(to)..to]);
        }
        Ok(out)
    }

    /// Blocks `first..=last`, fetching those not kept in one request.
    async fn blocks(&self, first: u64, last: u64) -> Result<Vec<Arc<Vec<u8>>>, String> {
        if let Some(all) = self.kept_run(first, last) {
            return Ok(all);
        }
        let _one_at_a_time = self.fetching.lock().await;
        if let Some(all) = self.kept_run(first, last) {
            return Ok(all);
        }
        // From the first block missing to the last one missing: one request.
        let missing: Vec<u64> = {
            let kept = self.kept.lock().unwrap_or_else(|e| e.into_inner());
            (first..=last)
                .filter(|b| !kept.blocks.contains_key(b))
                .collect()
        };
        let (from, to) = (missing[0], missing[missing.len() - 1]);
        let fetched = self.fetch(from, to).await?;
        let mut kept = self.kept.lock().unwrap_or_else(|e| e.into_inner());
        for (n, block) in fetched.into_iter().enumerate() {
            let index = from + n as u64;
            kept.bytes += block.len();
            if let Some(old) = kept.blocks.insert(index, Arc::new(block)) {
                kept.bytes -= old.len();
            } else {
                kept.order.push_back(index);
            }
        }
        let all: Vec<Arc<Vec<u8>>> = (first..=last)
            .filter_map(|b| kept.blocks.get(&b).cloned())
            .collect();
        // Let the oldest go; this read holds its own blocks already.
        while kept.bytes > KEEP_BYTES {
            let Some(oldest) = kept.order.pop_front() else {
                break;
            };
            if let Some(gone) = kept.blocks.remove(&oldest) {
                kept.bytes -= gone.len();
            }
        }
        if all.len() as u64 != last - first + 1 {
            return Err("a part of the file went missing".to_owned());
        }
        Ok(all)
    }

    fn kept_run(&self, first: u64, last: u64) -> Option<Vec<Arc<Vec<u8>>>> {
        let kept = self.kept.lock().unwrap_or_else(|e| e.into_inner());
        (first..=last)
            .map(|b| kept.blocks.get(&b).cloned())
            .collect()
    }

    /// Fetches blocks `from..=to` and opens them.
    async fn fetch(&self, from: u64, to: u64) -> Result<Vec<Vec<u8>>, String> {
        let count = self.block_count();
        let to = to.min(count - 1);
        let tag = if self.seal.is_some() {
            e2ee::CHUNK_TAG
        } else {
            0
        };
        let step = u64::from(self.block) + tag;
        let stored_size = self.size + if self.seal.is_some() { count * tag } else { 0 };
        let start = from * step;
        let end = ((to + 1) * step).min(stored_size) - 1;
        let bytes = self
            .rest
            .download_range(self.id, &self.stored_name, start, end)
            .await
            .map_err(|e| e.to_string())?;
        if bytes.len() as u64 != end - start + 1 {
            return Err("the server sent a part of the wrong size".to_owned());
        }
        let Some(seal) = &self.seal else {
            return Ok(bytes
                .chunks(self.block as usize)
                .map(<[u8]>::to_vec)
                .collect());
        };
        let (key, prefix) = (seal.key.clone(), seal.prefix.clone());
        tokio::task::spawn_blocking(move || {
            bytes
                .chunks(step as usize)
                .enumerate()
                .map(|(n, piece)| {
                    let index = from + n as u64;
                    e2ee::open_chunk(piece, &key, &prefix, index, index + 1 == count)
                        .map_err(|e| format!("The file could not be decrypted: {e}"))
                })
                .collect()
        })
        .await
        .map_err(|e| e.to_string())?
    }
}
