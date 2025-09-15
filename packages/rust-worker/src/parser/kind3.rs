use crate::parser::Parser;
use crate::types::network::Request;
use crate::types::nostr::Event;
use anyhow::{anyhow, Result};

// NEW: Imports for FlatBuffers
use crate::generated::nostr::*;

pub struct Contact {
    pub pubkey: String,
    pub relays: Vec<String>,
    pub petname: Option<String>,
}

pub type Kind3Parsed = Vec<Contact>;

impl Parser {
    pub fn parse_kind_3(&self, event: &Event) -> Result<(Kind3Parsed, Option<Vec<Request>>)> {
        if event.kind != 3 {
            return Err(anyhow!("event is not kind 3"));
        }

        let mut contacts = Vec::new();

        // Extract contacts from p tags
        for tag in &event.tags {
            if tag.len() >= 2 && tag[0] == "p" {
                let mut contact = Contact {
                    pubkey: tag[1].clone(),
                    relays: Vec::new(),
                    petname: None,
                };

                // Add relay if present (position 2)
                if tag.len() >= 3 && !tag[2].is_empty() {
                    contact.relays = vec![tag[2].clone()];
                }

                // Add petname if present (position 3)
                if tag.len() >= 4 && !tag[3].is_empty() {
                    contact.petname = Some(tag[3].clone());
                }

                contacts.push(contact);
            }
        }

        Ok((contacts, None))
    }
}

// NEW: Build the FlatBuffer for Kind3Parsed
pub fn build_flatbuffer<'a, A: flatbuffers::Allocator + 'a>(
    parsed: &Kind3Parsed,
    builder: &mut flatbuffers::FlatBufferBuilder<'a, A>,
) -> Result<flatbuffers::WIPOffset<fb::Kind3Parsed<'a>>> {
    // Build contacts vector
    let mut contacts_offsets = Vec::new();
    for contact in parsed {
        let pubkey = builder.create_string(&contact.pubkey);
        let relays_offsets: Vec<_> = contact
            .relays
            .iter()
            .map(|r| builder.create_string(r))
            .collect();
        let relays = if relays_offsets.is_empty() {
            None
        } else {
            Some(builder.create_vector(&relays_offsets))
        };
        let petname = contact.petname.as_ref().map(|p| builder.create_string(p));

        let contact_args = fb::ContactArgs {
            pubkey: Some(pubkey),
            relays,
            petname,
        };
        let contact_offset = fb::Contact::create(builder, &contact_args);
        contacts_offsets.push(contact_offset);
    }
    let contacts_vector = builder.create_vector(&contacts_offsets);

    let args = fb::Kind3ParsedArgs {
        contacts: Some(contacts_vector),
    };

    let offset = fb::Kind3Parsed::create(builder, &args);

    Ok(offset)
}
