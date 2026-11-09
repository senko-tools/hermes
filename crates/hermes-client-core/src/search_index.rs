//! Searching what is encrypted (docs/e2ee.md). The server cannot read these messages, so each
//! device keeps its own index of the words in them: word to message IDs, never the messages
//! themselves. On disk it is compressed and encrypted with a key from the OS credential store. It
//! is loaded by the first search and let go when idle; a slow pass over the history fills it in.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use hermes_proto::api::{HistoryQuery, SearchQuery, SearchResponse};
use hermes_proto::id::Id;
use hermes_proto::models::Message;
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::config::write_atomic;
use crate::core::{Core, now_ms};
use crate::e2ee;
use crate::rest::{self, Rest};
use crate::secure::AccountKeys;

/// Longest word indexed, in characters; longer ones are cut.
const MAX_WORD_CHARS: usize = 64;
/// Operations kept while the index is not in memory. What falls off is found by the next pass
/// over the history.
const MAX_PENDING: usize = 5_000;
/// The index is let go after this long without a search.
const IDLE_MS: i64 = 5 * 60 * 1000;
/// Messages asked for at once, and the pause after each page, so the server hardly notices.
const SYNC_PAGE: u32 = 100;
const SYNC_PAUSE: Duration = Duration::from_millis(250);
/// How long after connecting the pass over the history starts.
const SYNC_DELAY: Duration = Duration::from_secs(30);
const ZSTD_LEVEL: i32 = 3;

/// How far the pass over a channel's history has got.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
struct Progress {
    newest: Option<Id>,
    oldest: Option<Id>,
    /// Everything older than `oldest` is done: there is nothing more.
    complete: bool,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Data {
    words: BTreeMap<String, Vec<Id>>,
    /// Each message's channel and author.
    messages: HashMap<Id, (Id, Id)>,
    progress: HashMap<Id, Progress>,
}

impl Data {
    fn add(&mut self, id: Id, channel: Id, author: Id, words: &BTreeSet<String>) {
        self.remove(id);
        self.messages.insert(id, (channel, author));
        for word in words {
            let ids = self.words.entry(word.clone()).or_default();
            if let Err(at) = ids.binary_search(&id) {
                ids.insert(at, id);
            }
        }
    }

    fn remove(&mut self, id: Id) {
        if self.messages.remove(&id).is_none() {
            return;
        }
        self.words.retain(|_, ids| {
            if let Ok(at) = ids.binary_search(&id) {
                ids.remove(at);
            }
            !ids.is_empty()
        });
    }

    /// The messages holding a word that starts with `prefix`.
    fn with_prefix(&self, prefix: &str) -> BTreeSet<Id> {
        self.words
            .range(prefix.to_owned()..)
            .take_while(|(word, _)| word.starts_with(prefix))
            .flat_map(|(_, ids)| ids.iter().copied())
            .collect()
    }

    /// The messages with every word, each as the start of a word, newest first.
    fn find(&self, query: &str) -> Vec<Id> {
        let mut found: Option<BTreeSet<Id>> = None;
        for word in words_of(query) {
            let ids = self.with_prefix(&word);
            found = Some(match found {
                Some(have) => have.intersection(&ids).copied().collect(),
                None => ids,
            });
        }
        let mut found: Vec<Id> = found.unwrap_or_default().into_iter().collect();
        found.reverse();
        found
    }
}

/// The words of a text, in lower case.
fn words_of(text: &str) -> BTreeSet<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(|word| word.chars().take(MAX_WORD_CHARS).collect::<String>())
        .map(|word| word.to_lowercase())
        .collect()
}

enum Pending {
    Add {
        id: Id,
        channel: Id,
        author: Id,
        words: BTreeSet<String>,
    },
    Remove(Id),
}

pub(crate) struct SearchIndex {
    path: Option<PathBuf>,
    key: Zeroizing<[u8; 32]>,
    data: Option<Data>,
    pending: Vec<Pending>,
    dirty: bool,
    used: i64,
    syncing: bool,
}

impl SearchIndex {
    pub(crate) fn new(path: Option<PathBuf>, key: Zeroizing<[u8; 32]>) -> SearchIndex {
        SearchIndex {
            path,
            key,
            data: None,
            pending: Vec::new(),
            dirty: false,
            used: 0,
            syncing: false,
        }
    }

    fn read_file(&self) -> Option<Data> {
        let sealed = std::fs::read(self.path.as_ref()?).ok()?;
        let packed = e2ee::open_local(&self.key, &sealed).ok()?;
        let json = Zeroizing::new(zstd::stream::decode_all(packed.as_slice()).ok()?);
        serde_json::from_slice(&json).ok()
    }

    /// The index, in memory from now on, with what happened while it was not.
    fn load(&mut self) -> &mut Data {
        if self.data.is_none() {
            let mut data = self.read_file().unwrap_or_default();
            for op in self.pending.drain(..) {
                match op {
                    Pending::Add {
                        id,
                        channel,
                        author,
                        words,
                    } => data.add(id, channel, author, &words),
                    Pending::Remove(id) => data.remove(id),
                }
                self.dirty = true;
            }
            self.data = Some(data);
        }
        self.used = now_ms();
        self.data.as_mut().expect("just loaded")
    }

    fn apply(&mut self, op: Pending) {
        match (&mut self.data, op) {
            (
                Some(data),
                Pending::Add {
                    id,
                    channel,
                    author,
                    words,
                },
            ) => {
                data.add(id, channel, author, &words);
                self.dirty = true;
            }
            (Some(data), Pending::Remove(id)) => {
                data.remove(id);
                self.dirty = true;
            }
            (None, op) => {
                if self.pending.len() < MAX_PENDING {
                    self.pending.push(op);
                }
            }
        }
    }

    fn save(&mut self) {
        let (Some(path), Some(data), true) = (&self.path, &self.data, self.dirty) else {
            return;
        };
        let Ok(json) = serde_json::to_vec(data).map(Zeroizing::new) else {
            return;
        };
        let Ok(packed) = zstd::stream::encode_all(json.as_slice(), ZSTD_LEVEL).map(Zeroizing::new)
        else {
            return;
        };
        let Ok(sealed) = e2ee::seal_local(&self.key, &packed) else {
            return;
        };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        match write_atomic(path, &sealed) {
            Ok(()) => self.dirty = false,
            Err(err) => tracing::warn!("could not keep the search index: {err}"),
        }
    }

    /// Saves it and lets it go, unless it was used a moment ago or the history pass needs it.
    fn unload_if_idle(&mut self) {
        if !self.syncing && now_ms() - self.used >= IDLE_MS {
            self.save();
            self.data = None;
        }
    }
}

impl Core {
    /// Notes the words of a message just opened.
    pub(crate) fn index_message(&self, keys: &AccountKeys, message: &Message) {
        let mut text = message.content.clone();
        for attachment in &message.attachments {
            text.push(' ');
            text.push_str(&attachment.filename);
        }
        let words = words_of(&text);
        keys.index.lock().expect("index").apply(Pending::Add {
            id: message.id,
            channel: message.channel_id,
            author: message.author.id,
            words,
        });
    }

    /// A message was deleted.
    pub(crate) fn index_remove(&self, inst: &str, id: Id) {
        if let Some(keys) = self.account_keys(inst) {
            keys.index.lock().expect("index").apply(Pending::Remove(id));
        }
    }

    /// Searches the encrypted messages here and the plain ones on the server, newest first. Like
    /// the server's answer, the messages have yet to be opened.
    pub async fn search_with(
        &self,
        rest: &Rest,
        inst: &str,
        query: &SearchQuery,
    ) -> rest::Result<SearchResponse> {
        let limit = query.limit.unwrap_or(25).max(1) as usize;
        // The server can only search what it can read; where that is nothing, it need not
        // learn what is being looked for.
        let sealed = self.config().privacy.local_search_only
            || self
                .store()
                .instance(inst)
                .is_some_and(|s| s.search_only_sealed(query));
        let mut merged = if sealed {
            SearchResponse {
                messages: Vec::new(),
                total: 0,
            }
        } else {
            rest.search(query).await?
        };
        let Some(keys) = self.account_keys(inst) else {
            return Ok(merged);
        };
        let hits = self.local_hits(inst, &keys, query).await;
        merged.total += hits.len() as u64;
        // Enough of each list to fill a page: the server's are sorted already.
        for (channel, id) in hits.into_iter().take(limit) {
            let around = HistoryQuery {
                around: Some(id),
                limit: Some(3),
                ..HistoryQuery::default()
            };
            let page = rest.messages(channel, &around).await?;
            if let Some(message) = page.into_iter().find(|m| m.id == id && !m.deleted) {
                merged.messages.push(message);
            }
        }
        merged.messages.sort_by_key(|m| std::cmp::Reverse(m.id));
        merged.messages.truncate(limit);
        Ok(merged)
    }

    /// The indexed messages that match, newest first, in channels I can still see.
    async fn local_hits(
        &self,
        inst: &str,
        keys: &Arc<AccountKeys>,
        query: &SearchQuery,
    ) -> Vec<(Id, Id)> {
        let found = {
            let keys = keys.clone();
            let q = query.clone();
            let core = self.clone();
            let inst = inst.to_owned();
            tokio::task::spawn_blocking(move || {
                let mut index = keys.index.lock().expect("index");
                let data = index.load();
                let hits: Vec<(Id, Id, Id)> = data
                    .find(&q.q)
                    .into_iter()
                    .filter(|id| q.before.is_none_or(|before| *id < before))
                    .filter_map(|id| data.messages.get(&id).map(|(c, a)| (id, *c, *a)))
                    .collect();
                index.save();
                drop(index);
                let store = core.store();
                let s = store.instance(&inst);
                hits.into_iter()
                    .filter(|(_, channel, author)| {
                        q.channel_id.is_none_or(|wanted| wanted == *channel)
                            && q.author_id.is_none_or(|wanted| wanted == *author)
                            && s.is_some_and(|s| {
                                s.channel(*channel).is_some()
                                    && q.space_id.is_none_or(|w| s.space_of(*channel) == Some(w))
                            })
                    })
                    .map(|(id, channel, _)| (channel, id))
                    .collect::<Vec<_>>()
            })
            .await
            .unwrap_or_default()
        };
        let (keys, _) = (keys.clone(), ());
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(IDLE_MS as u64 + 1000)).await;
            keys.index.lock().expect("index").unload_if_idle();
        });
        found
    }

    /// Starts the slow pass over the history, once per connection.
    pub(crate) fn start_index_sync(&self, inst: &str, keys: &Arc<AccountKeys>) {
        {
            let mut index = keys.index.lock().expect("index");
            if index.syncing {
                return;
            }
            index.syncing = true;
        }
        let (core, inst, keys) = (self.clone(), inst.to_owned(), keys.clone());
        tokio::spawn(async move {
            tokio::time::sleep(SYNC_DELAY).await;
            core.sync_search_index(&inst, &keys).await;
        });
    }

    /// Reads every encrypted channel's history it has not seen yet, a page at a time.
    pub async fn index_history(&self, inst: &str) {
        if let Some(keys) = self.account_keys(inst) {
            keys.index.lock().expect("index").syncing = true;
            self.sync_search_index(inst, &keys).await;
        }
    }

    async fn sync_search_index(&self, inst: &str, keys: &Arc<AccountKeys>) {
        keys.index.lock().expect("index").load();
        let channels: Vec<Id> = {
            let store = self.store();
            store.instance(inst).map_or_else(Vec::new, |s| {
                s.spaces
                    .values()
                    .flat_map(|space| space.channels.keys().copied())
                    .chain(s.dms.keys().copied())
                    .collect()
            })
        };
        for channel in channels {
            if self.channel_is_encrypted(inst, channel)
                && let Err(err) = self.sync_channel(inst, keys, channel).await
            {
                tracing::debug!("could not index channel {channel}: {err}");
            }
        }
        let mut index = keys.index.lock().expect("index");
        index.save();
        index.syncing = false;
        index.unload_if_idle();
    }

    async fn sync_channel(
        &self,
        inst: &str,
        keys: &AccountKeys,
        channel: Id,
    ) -> crate::core::Result<()> {
        let rest = self.rest(inst)?;
        let progress = |keys: &AccountKeys| -> Progress {
            let mut index = keys.index.lock().expect("index");
            index
                .load()
                .progress
                .get(&channel)
                .copied()
                .unwrap_or_default()
        };
        // What arrived since last time first, then further into the past.
        loop {
            let at = progress(keys);
            let query = HistoryQuery {
                after: at.newest,
                limit: Some(SYNC_PAGE),
                ..HistoryQuery::default()
            };
            let page = self.index_page(&rest, inst, channel, query).await?;
            if page < SYNC_PAGE as usize {
                break;
            }
            tokio::time::sleep(SYNC_PAUSE).await;
        }
        loop {
            let at = progress(keys);
            if at.complete || at.oldest.is_none() {
                break;
            }
            let query = HistoryQuery {
                before: at.oldest,
                limit: Some(SYNC_PAGE),
                ..HistoryQuery::default()
            };
            let page = self.index_page(&rest, inst, channel, query).await?;
            if page < SYNC_PAGE as usize {
                let mut index = keys.index.lock().expect("index");
                index.load().progress.entry(channel).or_default().complete = true;
                index.dirty = true;
                break;
            }
            keys.index.lock().expect("index").save();
            tokio::time::sleep(SYNC_PAUSE).await;
        }
        Ok(())
    }

    /// Opens a page of history, which indexes it, and notes how far that got. Its length.
    async fn index_page(
        &self,
        rest: &Rest,
        inst: &str,
        channel: Id,
        query: HistoryQuery,
    ) -> crate::core::Result<usize> {
        let mut page = rest.messages(channel, &query).await?;
        let (Some(oldest), Some(newest)) = (
            page.iter().map(|m| m.id).min(),
            page.iter().map(|m| m.id).max(),
        ) else {
            return Ok(0);
        };
        self.open_messages(inst, &mut page).await;
        if let Some(keys) = self.account_keys(inst) {
            let mut index = keys.index.lock().expect("index");
            let at = index.load().progress.entry(channel).or_default();
            at.newest = at.newest.max(Some(newest));
            at.oldest = Some(at.oldest.map_or(oldest, |have| have.min(oldest)));
            index.dirty = true;
        }
        Ok(page.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CHANNEL: Id = Id(10);
    const AUTHOR: Id = Id(20);

    fn data(messages: &[(u64, &str)]) -> Data {
        let mut data = Data::default();
        for (id, text) in messages {
            data.add(Id(*id), CHANNEL, AUTHOR, &words_of(text));
        }
        data
    }

    #[test]
    fn words_are_lower_case_and_split_on_anything_else() {
        let words: Vec<String> = words_of("Hello, wörld! it's 2 o'clock")
            .into_iter()
            .collect();
        assert_eq!(words, ["2", "clock", "hello", "it", "o", "s", "wörld"]);
    }

    #[test]
    fn every_word_has_to_match_as_the_start_of_a_word() {
        let data = data(&[
            (1, "encrypted channels"),
            (2, "encrypted files"),
            (3, "channels"),
        ]);
        assert_eq!(data.find("encr"), vec![Id(2), Id(1)]);
        assert_eq!(data.find("ENCRYPTED chan"), vec![Id(1)]);
        assert_eq!(data.find("crypted"), Vec::<Id>::new());
        assert_eq!(data.find("  "), Vec::<Id>::new());
    }

    #[test]
    fn an_edit_replaces_the_old_words_and_a_delete_removes_them() {
        let mut d = data(&[(1, "old words here"), (2, "old words too")]);
        d.add(Id(1), CHANNEL, AUTHOR, &words_of("new text"));
        assert_eq!(d.find("old"), vec![Id(2)]);
        assert_eq!(d.find("new"), vec![Id(1)]);
        d.remove(Id(2));
        assert_eq!(d.find("old"), Vec::<Id>::new());
        assert!(!d.words.contains_key("too"), "words nobody uses are gone");
    }

    #[test]
    fn what_happens_while_it_is_not_in_memory_is_applied_when_it_loads() {
        let mut index = SearchIndex::new(None, e2ee::new_channel_key());
        index.apply(Pending::Add {
            id: Id(1),
            channel: CHANNEL,
            author: AUTHOR,
            words: words_of("later"),
        });
        assert!(index.data.is_none());
        assert_eq!(index.load().find("later"), vec![Id(1)]);
    }

    #[test]
    fn the_file_opens_only_with_its_key_and_holds_no_readable_words() {
        let dir = std::env::temp_dir().join(format!("hermes-index-{}", std::process::id()));
        let path = dir.join("index.bin");
        let key = e2ee::new_channel_key();
        let mut index = SearchIndex::new(Some(path.clone()), key.clone());
        index
            .load()
            .add(Id(1), CHANNEL, AUTHOR, &words_of("topsecretword"));
        index.dirty = true;
        index.save();
        let bytes = std::fs::read(&path).unwrap();
        assert!(!bytes.windows(13).any(|w| w == b"topsecretword"));

        let mut same = SearchIndex::new(Some(path.clone()), key);
        assert_eq!(same.load().find("topsecret"), vec![Id(1)]);
        let mut wrong = SearchIndex::new(Some(path), e2ee::new_channel_key());
        assert_eq!(wrong.load().find("topsecret"), Vec::<Id>::new());
        let _ = std::fs::remove_dir_all(dir);
    }
}
