use super::super::*;
use crate::generated::nostr::fb;
use crate::types::Event;
use flatbuffers;
use rustc_hash::FxHashSet;

/// Where muted words are matched against an event.
/// Mirrors `fb::MuteTarget` (schemas/main.fbs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MuteTarget {
    /// Match words against event content only
    Content,
    /// Match words against tag values only
    Tags,
    /// Match words against both content and tag values
    Both,
}

impl From<fb::MuteTarget> for MuteTarget {
    fn from(value: fb::MuteTarget) -> Self {
        match value {
            fb::MuteTarget::Content => Self::Content,
            fb::MuteTarget::Tags => Self::Tags,
            _ => Self::Both,
        }
    }
}

/// Pre-parsed mute criteria. Build this upstream from your parsed mute event (kind 10000).
/// - pubkeys: hex pubkeys to mute
/// - hashtags: lowercase hashtags to mute (compare against "t" tags case-insensitively)
/// - words: lowercase words to mute (case-insensitive substring check, see `target`)
/// - event_ids: hex event IDs to mute (drop events with these ids or events referencing them in "e" tags)
/// - target: whether muted words match against event content, tag values, or both.
///   Tag matching checks every value (index >= 1) of every tag, case-insensitively.
///   Hashtags stay tag-only ("t" tags); pubkeys/event_ids are unaffected by `target`.
pub struct MuteCriteria {
    pub pubkeys: FxHashSet<String>,
    pub hashtags: FxHashSet<String>,
    pub words: Vec<String>,
    pub event_ids: FxHashSet<String>,
    pub target: MuteTarget,
}

impl MuteCriteria {
    pub fn new(
        pubkeys: impl IntoIterator<Item = String>,
        hashtags: impl IntoIterator<Item = String>,
        words: impl IntoIterator<Item = String>,
        event_ids: impl IntoIterator<Item = String>,
        target: MuteTarget,
    ) -> Self {
        Self {
            pubkeys: pubkeys.into_iter().collect(),
            hashtags: hashtags.into_iter().map(|t| t.to_lowercase()).collect(),
            words: words.into_iter().map(|w| w.to_lowercase()).collect(),
            event_ids: event_ids.into_iter().collect(),
            target,
        }
    }
}

/// Pipe that filters events based on pre-parsed mute criteria.
/// Checks author pubkey, event id, "e" references, "t" hashtags, and muted words
/// (against content and/or tag values depending on the criteria target).
pub struct MuteFilterPipe {
    name: String,
    muted_pubkeys: FxHashSet<String>,
    muted_hashtags: FxHashSet<String>,
    muted_words: Vec<String>,
    muted_events: FxHashSet<String>,
    target: MuteTarget,
}

impl MuteFilterPipe {
    pub fn new(criteria: MuteCriteria) -> Self {
        let name = format!(
            "MuteFilter(p:{}, t:{}, word:{}, e:{}, target:{:?})",
            criteria.pubkeys.len(),
            criteria.hashtags.len(),
            criteria.words.len(),
            criteria.event_ids.len(),
            criteria.target,
        );
        Self {
            name,
            muted_pubkeys: criteria.pubkeys,
            muted_hashtags: criteria.hashtags,
            muted_words: criteria.words,
            muted_events: criteria.event_ids,
            target: criteria.target,
        }
    }

    #[inline]
    fn should_drop(&self, ev: &Event) -> bool {
        // 1) Author muted
        if !self.muted_pubkeys.is_empty() {
            let pubkey_hex = ev.pubkey.to_string();
            if self.muted_pubkeys.contains(&pubkey_hex) {
                return true;
            }
        }

        // 2) Event id muted
        if !self.muted_events.is_empty() {
            let id_hex = ev.id.to_hex();
            if self.muted_events.contains(&id_hex) {
                return true;
            }

            // 3) References muted event via "e" tags
            for tag in ev.tags.iter() {
                if tag.len() >= 2 && tag[0] == "e" && self.muted_events.contains(&tag[1]) {
                    return true;
                }
            }
        }

        // 4) Hashtag muted via "t" tags (case-insensitive, always tag-only)
        if !self.muted_hashtags.is_empty() {
            for tag in ev.tags.iter() {
                if tag.len() >= 2 && tag[0] == "t" {
                    if self.muted_hashtags.contains(&tag[1].to_lowercase()) {
                        return true;
                    }
                }
            }
        }

        // 5) Muted word match (case-insensitive substring), scoped by target
        if !self.muted_words.is_empty() {
            // 5a) Event content
            if self.target != MuteTarget::Tags && !ev.content.is_empty() {
                let content_lc = ev.content.to_lowercase();
                for w in &self.muted_words {
                    if content_lc.contains(w) {
                        return true;
                    }
                }
            }

            // 5b) Tag values: every value (index >= 1) of every tag.
            // "t" and "e" tags are additionally covered by the dedicated
            // hashtag/event-id checks above; here they are just tag values.
            if self.target != MuteTarget::Content {
                for tag in ev.tags.iter() {
                    for value in tag.iter().skip(1) {
                        let value_lc = value.to_lowercase();
                        for w in &self.muted_words {
                            if value_lc.contains(w) {
                                return true;
                            }
                        }
                    }
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
                        wm.content_as_parsed_event()
                            .map(|p| {
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
                            })
                            .unwrap_or(false)
                    }
                    fb::Message::NostrEvent => {
                        wm.content_as_nostr_event()
                            .map(|n| {
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
                            })
                            .unwrap_or(false)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::nostr::{EventId, PublicKey};

    fn make_event(content: &str, tags: Vec<Vec<String>>) -> Event {
        Event {
            id: EventId([1; 32]),
            pubkey: PublicKey([2; 32]),
            created_at: 1_700_000_000,
            kind: 1,
            tags,
            content: content.to_string(),
            sig: hex::encode([4; 64]),
        }
    }

    fn pipe_with_words(words: Vec<String>, target: MuteTarget) -> MuteFilterPipe {
        MuteFilterPipe::new(MuteCriteria::new(
            vec![],
            vec![],
            words,
            vec![],
            target,
        ))
    }

    #[test]
    fn words_match_content_with_target_both() {
        let pipe = pipe_with_words(vec!["spam".to_string()], MuteTarget::Both);
        assert!(pipe.should_drop(&make_event("this is spam", vec![])));
        assert!(!pipe.should_drop(&make_event("all good", vec![])));
    }

    #[test]
    fn words_match_tag_values_with_target_both() {
        let pipe = pipe_with_words(vec!["spam".to_string()], MuteTarget::Both);
        // Muted word appears only in a tag value
        let ev = make_event("all good", vec![vec!["t".to_string(), "Spammy".to_string()]]);
        assert!(pipe.should_drop(&ev));
    }

    #[test]
    fn words_skip_tags_with_target_content() {
        let pipe = pipe_with_words(vec!["spam".to_string()], MuteTarget::Content);
        let tagged = make_event("all good", vec![vec!["t".to_string(), "spam".to_string()]]);
        assert!(!pipe.should_drop(&tagged));
        let in_content = make_event("this is spam", vec![]);
        assert!(pipe.should_drop(&in_content));
    }

    #[test]
    fn words_skip_content_with_target_tags() {
        let pipe = pipe_with_words(vec!["spam".to_string()], MuteTarget::Tags);
        let in_content = make_event("this is spam", vec![]);
        assert!(!pipe.should_drop(&in_content));
        let tagged = make_event(
            "all good",
            vec![vec!["p".to_string(), "notspamword".to_string()]],
        );
        assert!(pipe.should_drop(&tagged));
    }

    #[test]
    fn hashtags_stay_tag_only_regardless_of_target() {
        // Even with target=Content, hashtags still match "t" tags
        let pipe = MuteFilterPipe::new(MuteCriteria::new(
            vec![],
            vec!["nsfw".to_string()],
            vec![],
            vec![],
            MuteTarget::Content,
        ));
        let tagged = make_event("all good", vec![vec!["t".to_string(), "NSFW".to_string()]]);
        assert!(pipe.should_drop(&tagged));
        // ...and hashtag text in content does not match
        let in_content = make_event("talking about nsfw", vec![]);
        assert!(!pipe.should_drop(&in_content));
    }

    #[test]
    fn pubkeys_and_event_ids_unaffected_by_target() {
        let pubkey_hex = hex::encode([2; 32]);
        let id_hex = hex::encode([1; 32]);
        for target in [MuteTarget::Content, MuteTarget::Tags, MuteTarget::Both] {
            let pipe = MuteFilterPipe::new(MuteCriteria::new(
                vec![pubkey_hex.clone()],
                vec![],
                vec![],
                vec![id_hex.clone()],
                target,
            ));
            assert!(pipe.should_drop(&make_event("hello", vec![])));
        }
    }

    #[test]
    fn empty_criteria_never_drop() {
        let pipe = pipe_with_words(vec![], MuteTarget::Both);
        let ev = make_event("spam nsfw whatever", vec![vec!["t".to_string(), "spam".to_string()]]);
        assert!(!pipe.should_drop(&ev));
    }

    #[test]
    fn muted_event_reference_via_e_tag() {
        let id_hex = hex::encode([9; 32]);
        let pipe = MuteFilterPipe::new(MuteCriteria::new(
            vec![],
            vec![],
            vec![],
            vec![id_hex.clone()],
            MuteTarget::Both,
        ));
        let ev = make_event("repost", vec![vec!["e".to_string(), id_hex]]);
        assert!(pipe.should_drop(&ev));
    }
}
