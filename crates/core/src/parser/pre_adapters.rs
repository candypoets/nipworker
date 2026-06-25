use crate::parser::pre_generic::{PreGenericParsed, PreParticipant, PreRefEvent};
use crate::parser::Coordinate;

/// Convenience: build the NIP-33 address ("a" pointer) string "kind:pubkey_hex:d"
pub fn compute_a_pointer(kind: u16, pubkey_hex: &str, d: &str) -> String {
    format!("{}:{}:{}", kind, pubkey_hex, d)
}

/// Convenience: best-effort naddr-like string for UI/debug in absence of a bech32 encoder.
/// This is NOT a proper bech32-encoded naddr, but it contains the same tuple information.
/// If you later wire a NIP-19 encoder, replace callers with `try_compute_naddr`.
pub fn compute_naddr_like(kind: u16, pubkey_hex: &str, d: &str) -> String {
    // Keep a stable, recognizable prefix to make it clear to consumers this is not bech32.
    format!("naddr-like:{}", compute_a_pointer(kind, pubkey_hex, d))
}

/// Attempt to compute a proper bech32 naddr (if/when a NIP-19 encoder is available).
/// Currently returns None as a placeholder to avoid hard coupling here.
#[allow(unused_variables)]
pub fn try_compute_naddr(
    kind: u16,
    pubkey_hex: &str,
    d: &str,
    relays: &[String],
) -> Option<String> {
    // TODO: integrate with your NIP-19 bech32 encoder (e.g., nip19::to_naddr)
    // Example shape (pseudo):
    // nip19::Naddr {
    //   kind,
    //   pubkey: pubkey_hex,
    //   d,
    //   relays: relays.to_vec(),
    // }.to_bech32().ok()
    None
}

/* -------------------- NIP-53 Live Activities (30311/30312/30313) -------------------- */

#[derive(Debug, Clone)]
pub struct LiveParticipant {
    pub pubkey: String,
    pub relay: Option<String>,
    pub role: Option<String>,
    pub proof: Option<String>,
}

impl From<&PreParticipant> for LiveParticipant {
    fn from(p: &PreParticipant) -> Self {
        Self {
            pubkey: p.pubkey.clone(),
            relay: p.relay.clone(),
            role: p.role.clone(),
            proof: p.proof.clone(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct LiveActivity {
    pub kind: u16, // 30311
    pub d: String, // stream id
    pub title: Option<String>,
    pub description: Option<String>,
    pub image: Option<String>,
    pub streaming: Option<String>,
    pub recording: Option<String>,
    pub status: Option<String>, // planned|live|ended
    pub starts: Option<u64>,
    pub ends: Option<u64>,
    pub topics: Vec<String>,
    pub participants: Vec<LiveParticipant>,
    pub a_pointer: String,  // "30311:<pubkey>:<d>"
    pub naddr_like: String, // best-effort naddr-like string
}

pub fn adapt_live_activity(
    pre: &PreGenericParsed,
    author_pubkey_hex: &str,
) -> Option<LiveActivity> {
    if pre.kind != 30311 {
        return None;
    }
    let d = pre.d.clone()?;
    let a = compute_a_pointer(pre.kind, author_pubkey_hex, &d);
    let naddr_like = compute_naddr_like(pre.kind, author_pubkey_hex, &d);

    Some(LiveActivity {
        kind: pre.kind,
        d,
        title: pre.title.clone(),
        description: pre.description.clone(),
        image: pre.image.clone(),
        streaming: pre.streaming.clone(),
        recording: pre.recording.clone(),
        status: pre.status.clone(),
        starts: pre.starts,
        ends: pre.ends,
        topics: pre.topics.clone(),
        participants: pre.participants.iter().map(LiveParticipant::from).collect(),
        a_pointer: a,
        naddr_like,
    })
}

#[derive(Debug, Clone)]
pub struct LiveSpace {
    pub kind: u16, // 30312
    pub d: String, // space id
    pub room: Option<String>,
    pub title: Option<String>, // some clients might use title instead of room
    pub description: Option<String>,
    pub image: Option<String>,
    pub service: Option<String>,
    pub endpoint: Option<String>,
    pub status: Option<String>, // open|private|closed
    pub topics: Vec<String>,
    pub participants: Vec<LiveParticipant>,
    pub relays: Vec<String>,
    pub a_pointer: String,
    pub naddr_like: String,
}

pub fn adapt_live_space(pre: &PreGenericParsed, author_pubkey_hex: &str) -> Option<LiveSpace> {
    if pre.kind != 30312 {
        return None;
    }
    let d = pre.d.clone()?;
    let a = compute_a_pointer(pre.kind, author_pubkey_hex, &d);
    let naddr_like = compute_naddr_like(pre.kind, author_pubkey_hex, &d);

    Some(LiveSpace {
        kind: pre.kind,
        d,
        room: pre.room.clone(),
        title: pre.title.clone(),
        description: pre.description.clone(),
        image: pre.image.clone(),
        service: pre.service.clone(),
        endpoint: pre.endpoint.clone(),
        status: pre.status.clone(),
        topics: pre.topics.clone(),
        participants: pre.participants.iter().map(LiveParticipant::from).collect(),
        relays: pre.relays.clone(),
        a_pointer: a,
        naddr_like,
    })
}

#[derive(Debug, Clone)]
pub struct LiveSession {
    pub kind: u16, // 30313
    pub d: String, // session id
    pub title: Option<String>,
    pub description: Option<String>,
    pub image: Option<String>,
    pub status: Option<String>, // planned|live|ended
    pub starts: Option<u64>,
    pub ends: Option<u64>,
    pub topics: Vec<String>,
    pub participants: Vec<LiveParticipant>,
    pub a_pointer: String,
    pub naddr_like: String,
}

pub fn adapt_live_session(pre: &PreGenericParsed, author_pubkey_hex: &str) -> Option<LiveSession> {
    if pre.kind != 30313 {
        return None;
    }
    let d = pre.d.clone()?;
    let a = compute_a_pointer(pre.kind, author_pubkey_hex, &d);
    let naddr_like = compute_naddr_like(pre.kind, author_pubkey_hex, &d);

    Some(LiveSession {
        kind: pre.kind,
        d,
        title: pre.title.clone(),
        description: pre.description.clone(),
        image: pre.image.clone(),
        status: pre.status.clone(),
        starts: pre.starts,
        ends: pre.ends,
        topics: pre.topics.clone(),
        participants: pre.participants.iter().map(LiveParticipant::from).collect(),
        a_pointer: a,
        naddr_like,
    })
}

/* -------------------- NIP-58 Badges (30009/30008) -------------------- */

#[derive(Debug, Clone)]
pub struct BadgeDefinition {
    pub kind: u16,            // 30009
    pub d: String,            // badge id
    pub name: Option<String>, // often in "title"
    pub description: Option<String>,
    pub image: Option<String>,
    pub a_pointer: String,
    pub naddr_like: String,
}

pub fn adapt_badge_definition(
    pre: &PreGenericParsed,
    issuer_pubkey_hex: &str,
) -> Option<BadgeDefinition> {
    if pre.kind != 30009 {
        return None;
    }
    let d = pre.d.clone()?;
    let a = compute_a_pointer(pre.kind, issuer_pubkey_hex, &d);
    let naddr_like = compute_naddr_like(pre.kind, issuer_pubkey_hex, &d);

    Some(BadgeDefinition {
        kind: pre.kind,
        d,
        name: pre.title.clone(),
        description: pre.description.clone(),
        image: pre.image.clone(),
        a_pointer: a,
        naddr_like,
    })
}

#[derive(Debug, Clone)]
pub struct ProfileBadges {
    pub kind: u16,                  // 30008
    pub d: String,                  // usually "profile_badges"
    pub addresses: Vec<Coordinate>, // references to definitions (a)
    pub awards: Vec<PreRefEvent>,   // references to award events (e)
    pub a_pointer: String,
    pub naddr_like: String,
}

pub fn adapt_profile_badges(
    pre: &PreGenericParsed,
    owner_pubkey_hex: &str,
) -> Option<ProfileBadges> {
    if pre.kind != 30008 {
        return None;
    }
    let d = pre.d.clone()?;
    let a = compute_a_pointer(pre.kind, owner_pubkey_hex, &d);
    let naddr_like = compute_naddr_like(pre.kind, owner_pubkey_hex, &d);

    Some(ProfileBadges {
        kind: pre.kind,
        d,
        addresses: pre.addresses.clone(),
        awards: pre.events.clone(),
        a_pointer: a,
        naddr_like,
    })
}

/* -------------------- NIP-52 Calendar (31922/31923/31924/31925) -------------------- */

#[derive(Debug, Clone)]
pub struct CalendarEvent {
    pub kind: u16, // 31922 or 31923
    pub d: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub image: Option<String>,
    pub starts_ts: Option<u64>, // only filled for time-based (31923)
    pub ends_ts: Option<u64>,   // only filled for time-based (31923)
    pub topics: Vec<String>,
    pub links: Vec<String>,
    pub participants: Vec<LiveParticipant>, // reuse shape (pubkey/relay/role)
    pub a_pointer: String,
    pub naddr_like: String,
}

pub fn adapt_calendar_event(
    pre: &PreGenericParsed,
    author_pubkey_hex: &str,
) -> Option<CalendarEvent> {
    if pre.kind != 31922 && pre.kind != 31923 {
        return None;
    }
    let d = pre.d.clone()?;
    let a = compute_a_pointer(pre.kind, author_pubkey_hex, &d);
    let naddr_like = compute_naddr_like(pre.kind, author_pubkey_hex, &d);

    Some(CalendarEvent {
        kind: pre.kind,
        d,
        title: pre.title.clone(),
        description: pre.description.clone(),
        image: pre.image.clone(),
        starts_ts: pre.starts,
        ends_ts: pre.ends,
        topics: pre.topics.clone(),
        links: pre.links.clone(),
        participants: pre.participants.iter().map(LiveParticipant::from).collect(),
        a_pointer: a,
        naddr_like,
    })
}

#[derive(Debug, Clone)]
pub struct Calendar {
    pub kind: u16, // 31924
    pub d: String,
    pub title: Option<String>,
    pub references: Vec<Coordinate>, // "a" to 31922/31923
    pub a_pointer: String,
    pub naddr_like: String,
}

pub fn adapt_calendar(pre: &PreGenericParsed, owner_pubkey_hex: &str) -> Option<Calendar> {
    if pre.kind != 31924 {
        return None;
    }
    let d = pre.d.clone()?;
    let a = compute_a_pointer(pre.kind, owner_pubkey_hex, &d);
    let naddr_like = compute_naddr_like(pre.kind, owner_pubkey_hex, &d);

    Some(Calendar {
        kind: pre.kind,
        d,
        title: pre.title.clone(),
        references: pre.addresses.clone(),
        a_pointer: a,
        naddr_like,
    })
}

/* -------------------- NIP-54 Wiki (30818/30819) -------------------- */

#[derive(Debug, Clone)]
pub struct WikiArticle {
    pub kind: u16, // 30818
    pub d: String, // normalized id
    pub title: Option<String>,
    pub summary: Option<String>, // some clients use "summary"
    pub description: Option<String>,
    pub a_pointer: String,
    pub naddr_like: String,
}

pub fn adapt_wiki_article(pre: &PreGenericParsed, author_pubkey_hex: &str) -> Option<WikiArticle> {
    if pre.kind != 30818 {
        return None;
    }
    let d = pre.d.clone()?;
    let a = compute_a_pointer(pre.kind, author_pubkey_hex, &d);
    let naddr_like = compute_naddr_like(pre.kind, author_pubkey_hex, &d);

    Some(WikiArticle {
        kind: pre.kind,
        d,
        title: pre.title.clone(),
        // Some ecosystems still use "summary" for list previews; we only kept description here.
        summary: None,
        description: pre.description.clone(),
        a_pointer: a,
        naddr_like,
    })
}

#[derive(Debug, Clone)]
pub struct WikiRedirect {
    pub kind: u16,                // 30819
    pub d: String,                // source
    pub targets: Vec<Coordinate>, // target "a" references (as available)
    pub a_pointer: String,
    pub naddr_like: String,
}

pub fn adapt_wiki_redirect(
    pre: &PreGenericParsed,
    author_pubkey_hex: &str,
) -> Option<WikiRedirect> {
    if pre.kind != 30819 {
        return None;
    }
    let d = pre.d.clone()?;
    let a = compute_a_pointer(pre.kind, author_pubkey_hex, &d);
    let naddr_like = compute_naddr_like(pre.kind, author_pubkey_hex, &d);

    Some(WikiRedirect {
        kind: pre.kind,
        d,
        targets: pre.addresses.clone(),
        a_pointer: a,
        naddr_like,
    })
}

/* -------------------- Misc helpers to check kind families -------------------- */

pub fn is_nip53_live_activity(kind: u16) -> bool {
    kind == 30311
}
pub fn is_nip53_space(kind: u16) -> bool {
    kind == 30312
}
pub fn is_nip53_session(kind: u16) -> bool {
    kind == 30313
}
pub fn is_badge_definition(kind: u16) -> bool {
    kind == 30009
}
pub fn is_profile_badges(kind: u16) -> bool {
    kind == 30008
}
pub fn is_calendar_event(kind: u16) -> bool {
    kind == 31922 || kind == 31923
}
pub fn is_calendar(kind: u16) -> bool {
    kind == 31924
}
pub fn is_wiki_article(kind: u16) -> bool {
    kind == 30818
}
pub fn is_wiki_redirect(kind: u16) -> bool {
    kind == 30819
}
