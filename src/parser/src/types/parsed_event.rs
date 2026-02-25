use crate::parser::nip51::ListParsed;
use crate::parser::pre_generic::PreGenericParsed;
use crate::parser::Kind30023Parsed;
use shared::types::network::Request;
use shared::types::nostr::Event;
use shared::{generated::nostr::fb, types::TypesError};

use crate::parser::{
    Kind0Parsed, Kind10002Parsed, Kind10019Parsed, Kind1111Parsed, Kind17375Parsed, Kind17Parsed,
    Kind1Parsed, Kind20Parsed, Kind22Parsed, Kind3Parsed, Kind4Parsed, Kind6Parsed, Kind7374Parsed,
    Kind7375Parsed, Kind7376Parsed, Kind7Parsed, Kind9321Parsed, Kind9735Parsed,
};

/// Strongly typed parsed data for different event kinds
pub enum ParsedData {
    Kind0(Kind0Parsed),
    Kind1(Kind1Parsed),
    Kind3(Kind3Parsed),
    Kind4(Kind4Parsed),
    Kind6(Kind6Parsed),
    Kind7(Kind7Parsed),
    Kind17(Kind17Parsed),
    Kind20(Kind20Parsed),
    Kind22(Kind22Parsed),
    Kind1111(Kind1111Parsed),
    Kind7374(Kind7374Parsed),
    Kind7375(Kind7375Parsed),
    Kind7376(Kind7376Parsed),
    Kind9321(Kind9321Parsed),
    Kind9735(Kind9735Parsed),
    Kind10002(Kind10002Parsed),
    Kind10019(Kind10019Parsed),
    Kind17375(Kind17375Parsed),
    Kind30023(Kind30023Parsed),
    List(ListParsed),
    PreGeneric(PreGenericParsed),
}

impl ParsedData {
    /// Build FlatBuffer for the parsed data, returning the union type and offset
    pub fn build_flatbuffer<'a, A: flatbuffers::Allocator + 'a>(
        &self,
        builder: &mut flatbuffers::FlatBufferBuilder<'a, A>,
    ) -> Result<
        (
            shared::generated::nostr::fb::ParsedData,
            flatbuffers::WIPOffset<flatbuffers::UnionWIPOffset>,
        ),
        TypesError,
    > {
        use shared::generated::nostr::fb;

        match self {
            ParsedData::Kind0(data) => {
                let offset = crate::parser::kind0::build_flatbuffer(data, builder)?;
                Ok((fb::ParsedData::Kind0Parsed, offset.as_union_value()))
            }
            ParsedData::Kind1(data) => {
                let offset = crate::parser::kind1::build_flatbuffer(data, builder)?;
                Ok((fb::ParsedData::Kind1Parsed, offset.as_union_value()))
            }
            ParsedData::Kind3(data) => {
                let offset = crate::parser::kind3::build_flatbuffer(data, builder)?;
                Ok((fb::ParsedData::Kind3Parsed, offset.as_union_value()))
            }
            ParsedData::Kind4(data) => {
                let offset = crate::parser::kind4::build_flatbuffer(data, builder)?;
                Ok((fb::ParsedData::Kind4Parsed, offset.as_union_value()))
            }
            ParsedData::Kind6(data) => {
                let offset = crate::parser::kind6::build_flatbuffer(data, builder)?;
                Ok((fb::ParsedData::Kind6Parsed, offset.as_union_value()))
            }
            ParsedData::Kind7(data) => {
                let offset = crate::parser::kind7::build_flatbuffer(data, builder)?;
                Ok((fb::ParsedData::Kind7Parsed, offset.as_union_value()))
            }
            ParsedData::Kind17(data) => {
                let offset = crate::parser::kind17::build_flatbuffer(data, builder)?;
                Ok((fb::ParsedData::Kind17Parsed, offset.as_union_value()))
            }
            ParsedData::Kind20(data) => {
                let offset = crate::parser::kind20::build_flatbuffer(data, builder)?;
                Ok((fb::ParsedData::Kind20Parsed, offset.as_union_value()))
            }
            ParsedData::Kind22(data) => {
                let offset = crate::parser::kind22::build_flatbuffer(data, builder)?;
                Ok((fb::ParsedData::Kind22Parsed, offset.as_union_value()))
            }
            ParsedData::Kind1111(data) => {
                let offset = crate::parser::kind1111::build_flatbuffer(data, builder)?;
                Ok((fb::ParsedData::Kind1111Parsed, offset.as_union_value()))
            }
            ParsedData::Kind7374(data) => {
                let offset = crate::parser::kind7374::build_flatbuffer(data, builder)?;
                Ok((fb::ParsedData::Kind7374Parsed, offset.as_union_value()))
            }
            ParsedData::Kind7375(data) => {
                let offset = crate::parser::kind7375::build_flatbuffer(data, builder)?;
                Ok((fb::ParsedData::Kind7375Parsed, offset.as_union_value()))
            }
            ParsedData::Kind7376(data) => {
                let offset = crate::parser::kind7376::build_flatbuffer(data, builder)?;
                Ok((fb::ParsedData::Kind7376Parsed, offset.as_union_value()))
            }
            ParsedData::Kind9321(data) => {
                let offset = crate::parser::kind9321::build_flatbuffer(data, builder)?;
                Ok((fb::ParsedData::Kind9321Parsed, offset.as_union_value()))
            }
            ParsedData::Kind9735(data) => {
                let offset = crate::parser::kind9735::build_flatbuffer(data, builder)?;
                Ok((fb::ParsedData::Kind9735Parsed, offset.as_union_value()))
            }
            ParsedData::Kind10002(data) => {
                let offset = crate::parser::kind10002::build_flatbuffer(data, builder)?;
                Ok((fb::ParsedData::Kind10002Parsed, offset.as_union_value()))
            }
            ParsedData::Kind10019(data) => {
                let offset = crate::parser::kind10019::build_flatbuffer(data, builder)?;
                Ok((fb::ParsedData::Kind10019Parsed, offset.as_union_value()))
            }
            ParsedData::Kind17375(data) => {
                let offset = crate::parser::kind17375::build_flatbuffer(data, builder)?;
                Ok((fb::ParsedData::Kind17375Parsed, offset.as_union_value()))
            }
            ParsedData::Kind30023(data) => {
                let offset = crate::parser::kind30023::build_flatbuffer(data, builder)?;
                Ok((fb::ParsedData::Kind30023Parsed, offset.as_union_value()))
            }
            ParsedData::List(data) => {
                let offset = crate::parser::nip51::build_flatbuffer(data, builder)?;
                Ok((fb::ParsedData::ListParsed, offset.as_union_value()))
            }
            ParsedData::PreGeneric(data) => {
                let offset = crate::parser::pre_generic::build_flatbuffer(data, builder)?;
                Ok((fb::ParsedData::PreGenericParsed, offset.as_union_value()))
            }
        }
    }
}

/// ParsedEvent represents a Nostr event with additional parsed data
pub struct ParsedEvent {
    pub event: Event,

    pub parsed: Option<ParsedData>,

    pub requests: Option<Vec<Request>>,

    pub relays: Vec<String>,
}

impl ParsedEvent {
    pub fn new(event: Event) -> Self {
        Self {
            event,
            parsed: None,
            requests: None,
            relays: Vec::new(),
        }
    }

    pub fn with_parsed(mut self, parsed: ParsedData) -> Self {
        self.parsed = Some(parsed);
        self
    }

    pub fn with_relays(mut self, relays: Vec<String>) -> Self {
        self.relays = relays;
        self
    }

    pub fn with_requests(mut self, requests: Vec<Request>) -> Self {
        self.requests = Some(requests);
        self
    }

    pub fn build_flatbuffer<'a>(
        &self,
        fbb: &mut flatbuffers::FlatBufferBuilder<'a>,
    ) -> Result<flatbuffers::WIPOffset<fb::ParsedEvent<'a>>, TypesError> {
        let parsed_data = self
            .parsed
            .as_ref()
            .ok_or_else(|| TypesError::MissingField("No parsed data available".to_string()))?;

        // Build the ParsedData union directly in our builder
        let (parsed_type, parsed_union_offset) = parsed_data.build_flatbuffer(fbb)?;

        // Build the NostrEvent from parsed_event.event
        let id_offset = fbb.create_string(&self.event.id.to_hex());
        let pubkey_offset = fbb.create_string(&self.event.pubkey.to_hex());

        // Build relays based on source
        let relays_offsets: Vec<_> = if !self.relays.is_empty() {
            self.relays
                .iter()
                .map(|relay| fbb.create_string(relay))
                .collect()
        } else {
            vec![]
        };

        let relays_offset = if relays_offsets.is_empty() {
            None
        } else {
            Some(fbb.create_vector(&relays_offsets))
        };

        let requests_offset = if let Some(reqs) = self.requests.as_ref() {
            if !reqs.is_empty() {
                let req_offsets: Vec<_> = reqs.iter().map(|r| r.build_flatbuffer(fbb)).collect();
                Some(fbb.create_vector(&req_offsets))
            } else {
                None
            }
        } else {
            None
        };

        let mut string_vec_offsets = Vec::new();
        for tag in &self.event.tags {
            let tag_strings: Vec<_> = tag.iter().map(|t| fbb.create_string(t)).collect();
            let tag_vector = fbb.create_vector(&tag_strings);
            let string_vec = fb::StringVec::create(
                fbb,
                &fb::StringVecArgs {
                    items: Some(tag_vector),
                },
            );
            string_vec_offsets.push(string_vec);
        }
        let tags_offset = fbb.create_vector(&string_vec_offsets);

        // Build ParsedEvent with the union
        let parsed_event_args = fb::ParsedEventArgs {
            id: Some(id_offset),
            pubkey: Some(pubkey_offset),
            created_at: self.event.created_at as u32,
            kind: self.event.kind as u16,
            parsed_type,
            parsed: Some(parsed_union_offset),
            requests: requests_offset,
            relays: relays_offset,
            tags: Some(tags_offset),
        };

        Ok(fb::ParsedEvent::create(fbb, &parsed_event_args))
    }
}
