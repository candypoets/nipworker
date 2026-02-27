use super::super::*;
use flatbuffers;
use rustc_hash::FxHashSet;
use shared::types::Event;
use shared::generated::nostr::fb;

/// Pre-parsed mute criteria. Build this upstream from your parsed mute event (kind 10000).
/// - pubkeys: hex pubkeys to mute
/// - hashtags: lowercase hashtags to mute (compare against "t" tags case-insensitively)
/// - words: lowercase words to mute (case-insensitive substring check in content)
/// - event_ids: hex event IDs to mute (drop events with these ids or events referencing them in "e" tags)
pub struct MuteCriteria {
    pub pubkeys: FxHashSet<String>,
    pub hashtags: FxHashSet<String>,
    pub words: Vec<String>,
    pub event_ids: FxHashSet<String>,
}

impl MuteCriteria {
    pub fn new(
        pubkeys: impl IntoIterator<Item = String>,
        hashtags: impl IntoIterator<Item = String>,
        words: impl IntoIterator<Item = String>,
        event_ids: impl IntoIterator<Item = String>,
    ) -> Self {
        Self {
            pubkeys: pubkeys.into_iter().collect(),
            hashtags: hashtags.into_iter().map(|t| t.to_lowercase()).collect(),
            words: words.into_iter().map(|w| w.to_lowercase()).collect(),
            event_ids: event_ids.into_iter().collect(),
        }
    }
}

/// Pipe that filters events based on pre-parsed mute criteria.
/// Checks author pubkey, event id, "e" references, "t" hashtags, and content words.
pub struct MuteFilterPipe {
    name: String,
    muted_pubkeys: FxHashSet<String>,
    muted_hashtags: FxHashSet<String>,
    muted_words: Vec<String>,
    muted_events: FxHashSet<String>,
}

impl MuteFilterPipe {
    pub fn new(criteria: MuteCriteria) -> Self {
        let name = format!(
            "MuteFilter(p:{}, t:{}, word:{}, e:{})",
            criteria.pubkeys.len(),
            criteria.hashtags.len(),
            criteria.words.len(),
            criteria.event_ids.len()
        );
        Self {
            name,
            muted_pubkeys: criteria.pubkeys,
            muted_hashtags: criteria.hashtags,
            muted_words: criteria.words,
            muted_events: criteria.event_ids,
        }
    }

    #[inline]
    fn should_drop(&self, ev: &Event) -> bool {
        // 1) Author muted
        let pubkey_hex = ev.pubkey.to_string();
        if self.muted_pubkeys.contains(&pubkey_hex) {
            return true;
        }

        // 2) Event id muted
        let id_hex = ev.id.to_hex();
        if self.muted_events.contains(&id_hex) {
            return true;
        }

        // 3) References muted event via "e" tags
        if !self.muted_events.is_empty() {
            for tag in ev.tags.iter() {
                if tag.len() >= 2 && tag[0] == "e" && self.muted_events.contains(&tag[1]) {
                    return true;
                }
            }
        }

        // 4) Hashtag muted via "t" tags (case-insensitive)
        if !self.muted_hashtags.is_empty() {
            for tag in ev.tags.iter() {
                if tag.len() >= 2 && tag[0] == "t" {
                    if self.muted_hashtags.contains(&tag[1].to_lowercase()) {
                        return true;
                    }
                }
            }
        }

        // 5) Content contains muted word (case-insensitive substring)
        if !self.muted_words.is_empty() && !ev.content.is_empty() {
            let content_lc = ev.content.to_lowercase();
            for w in &self.muted_words {
                if content_lc.contains(w) {
                    return true;
                }
            }
        }

        false
    }
}

impl Pipe for MuteFilterPipe {
    async fn process(&mut self, event: PipelineEvent) -> Result<PipeOutput> {
        let drop = if let Some(ref raw) = event.raw {
            self.should_drop(raw)
        } else if let Some(ref parsed) = event.parsed {
            self.should_drop(&parsed.event)
        } else {
            // If we can’t access an event, safest is to drop it
            true
        };

        if drop {
            Ok(PipeOutput::Drop)
        } else {
            Ok(PipeOutput::Event(event))
        }
    }

    fn name(&self) -> &str {
        &self.name
    }

    async fn process_cached_batch(&mut self, messages: &[Vec<u8>]) -> Result<Vec<Vec<u8>>> {
        // Filter WorkerMessage bytes based on mute criteria
        let mut filtered = Vec::new();
        
        for msg_bytes in messages {
            // Parse WorkerMessage to check if it should be dropped
            let should_drop = if let Ok(wm) = flatbuffers::root::<fb::WorkerMessage>(msg_bytes) {
                match wm.content_type() {
                    fb::Message::ParsedEvent => {
                        wm.content_as_parsed_event().map(|p| {
                            // Check pubkey
                            let pubkey = p.pubkey().to_string();
                            if self.muted_pubkeys.contains(&pubkey) {
                                return true;
                            }
                            // Check event id
                            let id = p.id().to_string();
                            if self.muted_events.contains(&id) {
                                return true;
                            }
                            false
                        }).unwrap_or(false)
                    }
                    fb::Message::NostrEvent => {
                        wm.content_as_nostr_event().map(|n| {
                            // Check pubkey
                            let pubkey = n.pubkey().to_string();
                            if self.muted_pubkeys.contains(&pubkey) {
                                return true;
                            }
                            // Check event id
                            let id = n.id().to_string();
                            if self.muted_events.contains(&id) {
                                return true;
                            }
                            false
                        }).unwrap_or(false)
                    }
                    _ => false, // Other message types pass through
                }
            } else {
                false // Invalid messages pass through (will fail later)
            };
            
            if !should_drop {
                filtered.push(msg_bytes.clone());
            }
        }
        
        Ok(filtered)
    }
}
