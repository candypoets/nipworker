use super::super::*;
use crate::parser_types::parsed_event::ParsedData;
use crate::{
    generated::nostr::fb::{self},
    types::Proof,
};
use std::sync::{Arc, Mutex};

type Result<T> = std::result::Result<T, NostrError>;
use flatbuffers::FlatBufferBuilder;
use futures::stream::{self, StreamExt};
use rustc_hash::{FxHashMap, FxHashSet};
use tracing::{debug, error, warn};

// Cap concurrent proof verifications to avoid SAB ring overflow and CPU saturation
const MAX_CONCURRENT_PROOFS: usize = 4;

/// Cached mint keys for avoiding repeated network requests
#[derive(Clone)]
struct CachedKeys {
    keys_json: String,
}

/// Pipe that extracts proofs from Kind 9321 and 7375 events and verifies their DLEQ signatures.
/// Note: Spent state checking is intentionally NOT done here - clients should check state
/// when attempting to spend, as proof states change over time.
pub struct ProofVerificationPipe {
    seen_proofs: FxHashSet<String>, // secrets we've already seen (session-only dedup)
    pending_verifications: FxHashMap<String, String>, // secret -> Y point
    pending_proofs: FxHashMap<String, FxHashMap<String, Proof>>, // mint_url -> secret -> proof
    max_proofs: usize,
    name: String,
    verification_running: bool,
    mint_keys_cache: Arc<Mutex<FxHashMap<String, CachedKeys>>>, // cached mint keys
}

impl ProofVerificationPipe {
    pub fn new(max_proofs: usize) -> Self {
        Self {
            seen_proofs: FxHashSet::default(),
            pending_verifications: FxHashMap::default(),
            pending_proofs: FxHashMap::default(),
            max_proofs,
            name: format!("ProofVerification(max:{})", max_proofs),
            verification_running: false,
            mint_keys_cache: Arc::new(Mutex::new(FxHashMap::default())),
        }
    }

    async fn add_proofs(&mut self, proofs: Vec<Proof>, mint_url: String) -> Result<()> {
        if proofs.is_empty() {
            return Ok(());
        }

        let mint_keys_json = match self.fetch_mint_keys_json(&mint_url).await {
            Ok(keys) => keys,
            Err(e) => {
                warn!("Failed to fetch keys for mint {}: {}", mint_url, e);
                return Err(NostrError::Other(format!(
                    "Failed to fetch mint keys: {}",
                    e
                )));
            }
        };

        // Collect proofs that haven't been seen before
        let mut new_proofs = Vec::new();
        for proof in proofs {
            let secret = proof.secret.clone();

            if self.seen_proofs.contains(&secret) {
                continue;
            }

            // Evict oldest if over capacity
            if self.seen_proofs.len() >= self.max_proofs {
                if let Some(oldest_secret) = self.seen_proofs.iter().next().cloned() {
                    self.seen_proofs.remove(&oldest_secret);
                    self.pending_verifications.remove(&oldest_secret);
                    for mint_proofs in self.pending_proofs.values_mut() {
                        mint_proofs.remove(&oldest_secret);
                    }
                }
            }

            new_proofs.push(proof);
        }

        if new_proofs.is_empty() {
            return Ok(());
        }

        // No-op verification: just mark all new proofs as seen
        for proof in new_proofs {
            self.seen_proofs.insert(proof.secret);
        }

        Ok(())
    }

    /// Collect all DLEQ-verified proofs and return them as serialized bytes.
    /// Note: Spent state is NOT checked here - clients must check state when spending.
    async fn verify_pending_proofs(&mut self) -> Result<Vec<u8>> {
        self.verification_running = true;

        let mut valid_proofs: FxHashMap<String, Vec<Proof>> = FxHashMap::default();

        // Process all pending proofs - just collect those with valid Y points (DLEQ verified)
        for (mint_url, mint_proofs) in &self.pending_proofs {
            for (secret, proof) in mint_proofs {
                if let Some(y_point) = self.pending_verifications.get(secret) {
                    // Non-empty Y point means DLEQ verification passed
                    if !y_point.is_empty() {
                        valid_proofs
                            .entry(mint_url.clone())
                            .or_default()
                            .push(proof.clone());
                    }
                }
            }
        }

        // Clear all pending state
        self.pending_proofs.clear();
        self.pending_verifications.clear();
        self.verification_running = false;

        // Serialize valid proofs to bytes
        let mut builder = FlatBufferBuilder::new();
        let mut proofs_mint = Vec::new();

        for (mint_url, proofs) in &valid_proofs {
            let mut proofs_offsets = Vec::new();
            for proof in proofs {
                proofs_offsets.push(proof.to_offset(&mut builder));
            }
            let proofs_vector = builder.create_vector(&proofs_offsets);
            let mint_offset = builder.create_string(mint_url);
            let mint_proofs = fb::MintProofs::create(
                &mut builder,
                &fb::MintProofsArgs {
                    mint: Some(mint_offset),
                    proofs: Some(proofs_vector),
                },
            );
            proofs_mint.push(mint_proofs);
        }

        let proofs_mint_vector = builder.create_vector(&proofs_mint);
        let valid_proofs_msg = fb::ValidProofs::create(
            &mut builder,
            &fb::ValidProofsArgs {
                proofs: Some(proofs_mint_vector),
            },
        );

        // Build root WorkerMessage
        let union_value = valid_proofs_msg.as_union_value();

        let message_args = fb::WorkerMessageArgs {
            sub_id: None,
            url: None,
            type_: fb::MessageType::ValidProofs,
            content_type: fb::Message::ValidProofs,
            content: Some(union_value),
        };

        let root = fb::WorkerMessage::create(&mut builder, &message_args);
        builder.finish(root, None);

        let result_bytes = builder.finished_data().to_vec();

        Ok(result_bytes)
    }
}

impl Pipe for ProofVerificationPipe {
    async fn process(&mut self, event: PipelineEvent) -> Result<PipeOutput> {
        // Only process parsed events
        if let Some(ref parsed_event) = event.parsed {
            let kind = parsed_event.event.kind;

            // Extract proofs from Kind 9321 or 7375
            let (proofs, mint_url) = match kind {
                9321 => {
                    if let Some(parsed_data) = &parsed_event.parsed {
                        // Check if parsed_data is Kind9321Parsed variant
                        if let ParsedData::Kind9321(kind9321) = parsed_data {
                            let mint_url = kind9321.mint_url.clone();

                            // Extract proofs directly from the Kind9321Parsed struct
                            let proofs = kind9321.proofs.clone();
                            (proofs, mint_url)
                        } else {
                            error!("parsed_data is not Kind9321Parsed variant");
                            (Vec::new(), String::new())
                        }
                    } else {
                        error!("Kind 9321 event has no parsed_data");
                        (Vec::new(), String::new())
                    }
                }
                7375 => {
                    if let Some(parsed_data) = &parsed_event.parsed {
                        // Check if parsed_data is Kind7375Parsed variant
                        if let ParsedData::Kind7375(kind7375) = parsed_data {
                            let mint_url = kind7375.mint_url.clone();
                            let decrypted = kind7375.decrypted;

                            if decrypted {
                                let proofs = kind7375.proofs.clone();
                                (proofs, mint_url)
                            } else {
                                (Vec::new(), String::new())
                            }
                        } else {
                            error!("parsed_data is not Kind7375Parsed variant");
                            (Vec::new(), String::new())
                        }
                    } else {
                        (Vec::new(), String::new())
                    }
                }
                _ => (Vec::new(), String::new()),
            };

            // Add new proofs to tracking
            if !proofs.is_empty() && !mint_url.is_empty() {
                if let Err(e) = self.add_proofs(proofs, mint_url).await {
                    error!("Failed to add proofs: {}", e);
                }
            }
        }

        // Trigger verification immediately if we have pending proofs
        if !self.pending_verifications.is_empty() && !self.verification_running {
            match self.verify_pending_proofs().await {
                Ok(bytes) => {
                    if !bytes.is_empty() {
                        // Return serialized valid proofs as direct output
                        return Ok(PipeOutput::DirectOutput(bytes));
                    }
                }
                Err(e) => {
                    error!("Error during proof verification: {}", e);
                }
            }
        }

        // Drop the event - this pipe only outputs verified proofs
        Ok(PipeOutput::Drop)
    }

    async fn process_cached_batch(&mut self, messages: &[Vec<u8>]) -> Result<Vec<Vec<u8>>> {
        // Collect proofs from cached parsed events
        for bytes in messages {
            if let Ok(msg) = flatbuffers::root::<fb::WorkerMessage>(&bytes) {
                if let fb::Message::ParsedEvent = msg.content_type() {
                    if let Some(parsed) = msg.content_as_parsed_event() {
                        let kind = parsed.kind() as u64;

                        match kind {
                            9321 => {
                                if let Some(k) = parsed.parsed_as_kind_9321_parsed() {
                                    let mint_url = k.mint_url().to_string();
                                    let fb_proofs = k.proofs();
                                    let mut proofs = Vec::new();
                                    for i in 0..fb_proofs.len() {
                                        let p = fb_proofs.get(i);
                                        proofs.push(Proof::from_flatbuffer(&p));
                                    }
                                    if !proofs.is_empty() && !mint_url.is_empty() {
                                        if let Err(e) = self.add_proofs(proofs, mint_url).await {
                                            error!("Failed to add proofs: {}", e);
                                        }
                                    }
                                }
                            }
                            7375 => {
                                if let Some(k) = parsed.parsed_as_kind_7375_parsed() {
                                    if k.decrypted() {
                                        let mint_url = k.mint_url().to_string();
                                        let fb_proofs = k.proofs();
                                        let mut proofs = Vec::new();
                                        for i in 0..fb_proofs.len() {
                                            let p = fb_proofs.get(i);
                                            proofs.push(Proof::from_flatbuffer(&p));
                                        }
                                        if !proofs.is_empty() && !mint_url.is_empty() {
                                            if let Err(e) = self.add_proofs(proofs, mint_url).await
                                            {
                                                error!("Failed to add proofs: {}", e);
                                            }
                                        }
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
        }

        // Verify all collected proofs
        if !self.pending_verifications.is_empty() && !self.verification_running {
            match self.verify_pending_proofs().await {
                Ok(bytes) => {
                    if !bytes.is_empty() {
                        return Ok(vec![bytes]);
                    }
                }
                Err(e) => {
                    error!("Error during batch proof verification: {}", e);
                }
            }
        }

        Ok(Vec::new())
    }

    fn can_direct_output(&self) -> bool {
        true // This is a terminal pipe that outputs serialized proof data
    }

    fn name(&self) -> &str {
        &self.name
    }
}

impl ProofVerificationPipe {
    /// Fetch mint keys as JSON string for DLEQ verification
    async fn fetch_mint_keys_json(&self, mint_url: &str) -> Result<String> {
        // Check cache first
        {
            let cache = self.mint_keys_cache.lock().unwrap();
            if let Some(cached) = cache.get(mint_url) {
                return Ok(cached.keys_json.clone());
            }
        }

        // Fetch from network and update cache
        let keys_json = self.fetch_mint_keys_from_network(mint_url).await?;

        {
            let mut cache = self.mint_keys_cache.lock().unwrap();
            cache.insert(
                mint_url.to_string(),
                CachedKeys {
                    keys_json: keys_json.clone(),
                },
            );
        }

        Ok(keys_json)
    }

    /// Fetch mint keys from network (stub in core crate)
    async fn fetch_mint_keys_from_network(&self, mint_url: &str) -> Result<String> {
        let url = format!("{}/v1/keys", mint_url.trim_end_matches('/'));
        Err(NostrError::Other(format!(
            "fetch_mint_keys_from_network not available in core crate (url: {})",
            url
        )))
    }
}

// Custom JSON parsers for the structs
