use super::super::*;
use crate::generated::nostr::fb;
use crate::types::Event;
use flatbuffers;
use rustc_hash::{FxHashMap, FxHashSet};

struct ChatTracker {
    accepted_count: u32,
    oldest_forwarded_timestamp: u64,
}

pub struct ChatLimiterPipe {
    own_pubkey: String,
    limit_per_chat: u32,
    max_chats: usize,
    kinds: FxHashSet<u16>,
    chat_trackers: FxHashMap<String, ChatTracker>,
    name: String,
}

impl ChatLimiterPipe {
    pub fn new(
        own_pubkey: String,
        limit_per_chat: u32,
        max_chats: u32,
        kinds: Vec<u16>,
    ) -> Self {
        let limit_per_chat = limit_per_chat.max(1);
        let max_chats = max_chats.max(1) as usize;
        let kinds = kinds.into_iter().collect();

        Self {
            name: format!("ChatLimiter(limit:{}, max:{})", limit_per_chat, max_chats),
            own_pubkey,
            limit_per_chat,
            max_chats,
            kinds,
            chat_trackers: FxHashMap::default(),
        }
    }

    fn should_limit_kind(&self, kind: u16) -> bool {
        self.kinds.is_empty() || self.kinds.contains(&kind)
    }

    fn chat_id_from_event(&self, event: &Event) -> Option<String> {
        let author = event.pubkey.to_string();
        if author != self.own_pubkey {
            return Some(author);
        }

        event.tags.iter().find_map(|tag| {
            if tag.len() >= 2 && tag[0] == "p" {
                Some(tag[1].clone())
            } else {
                None
            }
        })
    }

    fn chat_id_from_parsed_event(&self, event: fb::ParsedEvent<'_>) -> Option<String> {
        let author = event.pubkey().to_string();
        if author != self.own_pubkey {
            return Some(author);
        }

        let tags = event.tags()?;
        tags.iter().find_map(|tag| {
            let items = tag.items()?;
            if items.len() >= 2 && items.get(0) == "p" {
                Some(items.get(1).to_string())
            } else {
                None
            }
        })
    }

    fn allow(&mut self, chat_id: String, created_at: u64) -> bool {
        if !self.chat_trackers.contains_key(&chat_id) && self.chat_trackers.len() >= self.max_chats
        {
            if let Some(oldest_key) = self
                .chat_trackers
                .iter()
                .min_by_key(|(_, tracker)| tracker.oldest_forwarded_timestamp)
                .map(|(key, _)| key.clone())
            {
                self.chat_trackers.remove(&oldest_key);
            }
        }

        let tracker = self
            .chat_trackers
            .entry(chat_id)
            .or_insert_with(|| ChatTracker {
                accepted_count: 0,
                oldest_forwarded_timestamp: created_at,
            });

        if tracker.accepted_count < self.limit_per_chat {
            tracker.accepted_count += 1;
            tracker.oldest_forwarded_timestamp = tracker.oldest_forwarded_timestamp.min(created_at);
            return true;
        }

        if created_at > tracker.oldest_forwarded_timestamp {
            return true;
        }

        false
    }
}

impl Pipe for ChatLimiterPipe {
    async fn process(&mut self, event: PipelineEvent) -> Result<PipeOutput> {
        let nostr_event = if let Some(ref raw) = event.raw {
            raw
        } else if let Some(ref parsed) = event.parsed {
            &parsed.event
        } else {
            return Ok(PipeOutput::Drop);
        };

        if !self.should_limit_kind(nostr_event.kind) {
            return Ok(PipeOutput::Event(event));
        }

        let Some(chat_id) = self.chat_id_from_event(nostr_event) else {
            return Ok(PipeOutput::Drop);
        };

        if self.allow(chat_id, nostr_event.created_at) {
            Ok(PipeOutput::Event(event))
        } else {
            Ok(PipeOutput::Drop)
        }
    }

    async fn process_cached_batch(&mut self, messages: &[Vec<u8>]) -> Result<Vec<Vec<u8>>> {
        let mut filtered = Vec::new();

        for msg_bytes in messages {
            let allow = if let Ok(message) = flatbuffers::root::<fb::WorkerMessage>(msg_bytes) {
                if let Some(event) = message.content_as_parsed_event() {
                    if !self.should_limit_kind(event.kind()) {
                        true
                    } else if let Some(chat_id) = self.chat_id_from_parsed_event(event) {
                        self.allow(chat_id, event.created_at() as u64)
                    } else {
                        false
                    }
                } else if let Some(event) = message.content_as_nostr_event() {
                    let kind = event.kind();
                    if !self.should_limit_kind(kind) {
                        true
                    } else {
                        let author = event.pubkey().to_string();
                        let chat_id = if author == self.own_pubkey {
                            event.tags().and_then(|tags| {
                                tags.iter().find_map(|tag| {
                                    let items = tag.items()?;
                                    if items.len() >= 2 && items.get(0) == "p" {
                                        Some(items.get(1).to_string())
                                    } else {
                                        None
                                    }
                                })
                            })
                        } else {
                            Some(author)
                        };

                        chat_id
                            .map(|id| self.allow(id, event.created_at() as u64))
                            .unwrap_or(false)
                    }
                } else {
                    true
                }
            } else {
                true
            };

            if allow {
                filtered.push(msg_bytes.clone());
            }
        }

        Ok(filtered)
    }

    fn name(&self) -> &str {
        &self.name
    }
}
