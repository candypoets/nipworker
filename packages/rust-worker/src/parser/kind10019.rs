use crate::nostr::Template;
use crate::parser::Parser;
use crate::parser::{ParserError, Result};
use crate::signer::interface::SignerManagerInterface;
use crate::types::network::Request;
use crate::types::nostr::Event;
use tracing::info;

// NEW: Imports for FlatBuffers
use crate::generated::nostr::*;
use flatbuffers::FlatBufferBuilder;

pub struct MintInfo {
    pub url: String,
    pub base_units: Option<Vec<String>>,
}

pub struct Kind10019Parsed {
    pub trusted_mints: Vec<MintInfo>,
    pub p2pk_pubkey: Option<String>,
    pub read_relays: Option<Vec<String>>,
}

impl Parser {
    pub fn parse_kind_10019(
        &self,
        event: &Event,
    ) -> Result<(Kind10019Parsed, Option<Vec<Request>>)> {
        if event.kind != 10019 {
            return Err(ParserError::Other("event is not kind 10019".to_string()));
        }

        let mut parsed = Kind10019Parsed {
            trusted_mints: Vec::new(),
            p2pk_pubkey: None,
            read_relays: Some(Vec::new()),
        };

        // Extract relay, mint, and pubkey tags
        for tag in &event.tags {
            if tag.len() >= 2 {
                match tag[0].as_str() {
                    "relay" => {
                        if let Some(ref mut relays) = parsed.read_relays {
                            relays.push(tag[1].clone());
                        }
                    }
                    "mint" => {
                        let mut mint_info = MintInfo {
                            url: tag[1].clone(),
                            base_units: Some(Vec::new()),
                        };

                        // Extract base units if provided (position 2 and beyond)
                        for i in 2..tag.len() {
                            if !tag[i].is_empty() {
                                if let Some(ref mut units) = mint_info.base_units {
                                    units.push(tag[i].clone());
                                }
                            }
                        }

                        parsed.trusted_mints.push(mint_info);
                    }
                    "pubkey" => {
                        parsed.p2pk_pubkey = Some(tag[1].clone());
                    }
                    _ => {}
                }
            }
        }

        // Check if required fields are present
        if parsed.trusted_mints.is_empty() || parsed.p2pk_pubkey.is_none() {
            return Err(ParserError::Other(
                "missing required mint tags or pubkey tag".to_string(),
            ));
        }

        Ok((parsed, None))
    }

    pub fn prepare_kind_10019(&self, template: &Template) -> Result<Event> {
        if template.kind != 10019 {
            return Err(ParserError::Other("event is not kind 10019".to_string()));
        }

        // Validate required tags
        let mut has_mint = false;
        let mut has_pubkey = false;

        for tag in &template.tags {
            if tag.len() >= 2 {
                match tag[0].as_str() {
                    "mint" => has_mint = true,
                    "pubkey" => has_pubkey = true,
                    _ => {}
                }
            }
        }

        if !has_mint {
            return Err(ParserError::Other(
                "kind 10019 must include at least one mint tag".to_string(),
            ));
        }

        if !has_pubkey {
            return Err(ParserError::Other(
                "kind 10019 must include a pubkey tag".to_string(),
            ));
        }
        self.signer_manager
            .sign_event(template)
            .map_err(|e| ParserError::Other(format!("failed to sign event: {}", e)))
    }
}

// NEW: Build the FlatBuffer for Kind10019Parsed
pub fn build_flatbuffer<'a, A: flatbuffers::Allocator + 'a>(
    parsed: &Kind10019Parsed,
    builder: &mut flatbuffers::FlatBufferBuilder<'a, A>,
) -> Result<flatbuffers::WIPOffset<fb::Kind10019Parsed<'a>>> {
    // Build trusted_mints vector
    let mut trusted_mints_offsets = Vec::new();
    for mint in &parsed.trusted_mints {
        let url = builder.create_string(&mint.url);
        let base_units_offsets: Vec<_> = mint
            .base_units
            .iter()
            .flatten()
            .map(|unit| builder.create_string(unit))
            .collect();
        let base_units = Some(builder.create_vector(&base_units_offsets));

        let mint_info_args = fb::MintInfoArgs {
            url: Some(url),
            base_units,
        };
        let mint_info_offset = fb::MintInfo::create(builder, &mint_info_args);
        trusted_mints_offsets.push(mint_info_offset);
    }
    let trusted_mints_vector = builder.create_vector(&trusted_mints_offsets);

    let p2pk_pubkey = parsed
        .p2pk_pubkey
        .as_ref()
        .map(|p| builder.create_string(p));

    // Build read_relays vector
    let read_relays_offsets: Vec<_> = parsed
        .read_relays
        .iter()
        .flatten()
        .map(|relay| builder.create_string(relay))
        .collect();
    let read_relays_vector = if read_relays_offsets.is_empty() {
        None
    } else {
        Some(builder.create_vector(&read_relays_offsets))
    };

    let args = fb::Kind10019ParsedArgs {
        trusted_mints: Some(trusted_mints_vector),
        p2pk_pubkey,
        read_relays: read_relays_vector,
    };

    let offset = fb::Kind10019Parsed::create(builder, &args);

    Ok(offset)
}
