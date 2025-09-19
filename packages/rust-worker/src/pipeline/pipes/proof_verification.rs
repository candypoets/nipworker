use super::super::*;
use crate::{generated::nostr::fb, parsed_event::ParsedData, types::proof::Proof, NostrError};

type Result<T> = std::result::Result<T, NostrError>;
use crate::utils::json::BaseJsonParser;
use flatbuffers::FlatBufferBuilder;
use gloo_net;
use hex;
use k256::{elliptic_curve::sec1::ToEncodedPoint, PublicKey};
use rustc_hash::{FxHashMap, FxHashSet};
use sha2::{Digest, Sha256};
use std::fmt::Write;
use tracing::{debug, error, info, warn};

#[derive(Debug, Clone)]
struct CheckStateRequest {
    ys: Vec<String>,
}

impl CheckStateRequest {
    pub fn new(ys: Vec<String>) -> Self {
        Self { ys }
    }

    pub fn to_json(&self) -> String {
        let mut result = String::with_capacity(self.calculate_json_size());

        result.push('{');
        result.push_str("\"Ys\":[");

        for (i, y) in self.ys.iter().enumerate() {
            if i > 0 {
                result.push(',');
            }
            result.push('"');
            Self::escape_string_to(&mut result, y);
            result.push('"');
        }

        result.push_str("]}");
        result
    }

    #[inline(always)]
    fn calculate_json_size(&self) -> usize {
        10 + // {"Ys":[]}
        self.ys.iter().map(|y| y.len() * 2 + 4).sum::<usize>() // Escaped strings + quotes + commas
    }

    #[inline(always)]
    fn escape_string_to(result: &mut String, s: &str) {
        for ch in s.chars() {
            match ch {
                '\\' => result.push_str("\\\\"),
                '"' => result.push_str("\\\""),
                '\n' => result.push_str("\\n"),
                '\r' => result.push_str("\\r"),
                '\t' => result.push_str("\\t"),
                other => result.push(other),
            }
        }
    }
}

#[derive(Debug, Clone)]
struct ProofState {
    y: String,
    state: String,
    witness: Option<String>,
}

impl ProofState {
    pub fn from_json(json: &str) -> Result<Self> {
        let parser = ProofStateParser::new(json.as_bytes());
        parser.parse()
    }

    pub fn to_json(&self) -> String {
        let mut result = String::with_capacity(self.calculate_json_size());

        result.push('{');
        write!(
            result,
            "\"Y\":\"{}\",\"state\":\"{}\"",
            Self::escape_string(&self.y),
            Self::escape_string(&self.state)
        )
        .unwrap();

        if let Some(ref witness) = self.witness {
            write!(result, ",\"witness\":\"{}\"", Self::escape_string(witness)).unwrap();
        }

        result.push('}');
        result
    }

    #[inline(always)]
    fn calculate_json_size(&self) -> usize {
        20 + // Base structure
        self.y.len() * 2 + self.state.len() * 2 + // Escaping
        self.witness.as_ref().map(|w| w.len() * 2 + 13).unwrap_or(0) // ,"witness":""
    }

    #[inline(always)]
    fn escape_string(s: &str) -> String {
        if !s.contains('\\') && !s.contains('"') {
            s.to_string()
        } else {
            let mut result = String::with_capacity(s.len() + 4);
            Self::escape_string_to(&mut result, s);
            result
        }
    }

    #[inline(always)]
    fn escape_string_to(result: &mut String, s: &str) {
        for ch in s.chars() {
            match ch {
                '\\' => result.push_str("\\\\"),
                '"' => result.push_str("\\\""),
                '\n' => result.push_str("\\n"),
                '\r' => result.push_str("\\r"),
                '\t' => result.push_str("\\t"),
                other => result.push(other),
            }
        }
    }
}

#[derive(Debug, Clone)]
struct CheckStateResponse {
    states: Vec<ProofState>,
}

impl CheckStateResponse {
    pub fn from_json(json: &str) -> Result<Self> {
        let parser = CheckStateResponseParser::new(json.as_bytes());
        parser.parse()
    }

    pub fn to_json(&self) -> String {
        let mut result = String::with_capacity(self.calculate_json_size());

        result.push('{');
        result.push_str("\"states\":[");

        for (i, state) in self.states.iter().enumerate() {
            if i > 0 {
                result.push(',');
            }
            result.push_str(&state.to_json());
        }

        result.push_str("]}");
        result
    }

    #[inline(always)]
    fn calculate_json_size(&self) -> usize {
        15 + // {"states":[]}
        self.states.iter().map(|s| s.calculate_json_size() + 1).sum::<usize>() // States + commas
    }
}

/// Pipe that extracts proofs from Kind 9321 and 7375 events and verifies their state with mints
pub struct ProofVerificationPipe {
    seen_proofs: FxHashSet<String>, // secrets we've already seen
    pending_verifications: FxHashMap<String, String>, // secret -> Y point
    pending_proofs: FxHashMap<String, FxHashMap<String, Proof>>, // mint_url -> secret -> proof
    max_proofs: usize,
    name: String,
    verification_running: bool,
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
        }
    }

    /// Compute Y point from secret using cashu-ts compatible hash_to_curve implementation
    fn compute_y_point(&self, secret: &str) -> String {
        const DOMAIN_SEPARATOR: &[u8] = b"Secp256k1_HashToCurve_Cashu_";

        // First hash: DOMAIN_SEPARATOR || secret
        let mut hasher = Sha256::new();
        hasher.update(DOMAIN_SEPARATOR);
        hasher.update(secret.as_bytes());
        let msg_hash = hasher.finalize();

        // Counter loop to find valid point
        const MAX_ITERATIONS: u32 = 65536; // 2^16
        for counter in 0..MAX_ITERATIONS {
            let mut hasher = Sha256::new();
            hasher.update(&msg_hash);
            hasher.update(&counter.to_le_bytes()); // little endian as per spec
            let hash = hasher.finalize();

            // Try to create point with 0x02 prefix (compressed)
            let mut point_bytes = Vec::with_capacity(33);
            point_bytes.push(0x02);
            point_bytes.extend_from_slice(&hash);

            // Try to parse as a valid secp256k1 point
            if let Ok(public_key) = PublicKey::from_sec1_bytes(&point_bytes) {
                let encoded_point = public_key.to_encoded_point(true);
                return hex::encode(encoded_point.as_bytes());
            }
        }

        panic!("No valid point found after 65536 iterations");
    }

    /// Add proofs to tracking, with deduplication
    fn add_proofs(&mut self, proofs: Vec<Proof>, mint_url: String) {
        for proof in proofs {
            let secret = proof.secret.clone();
            // Skip if we've already seen this proof
            if self.seen_proofs.contains(&secret) {
                debug!(
                    "{}: Skipping duplicate proof with secret {}",
                    self.name,
                    &secret[..8.min(secret.len())]
                );
                continue;
            }

            // Enforce max proofs limit
            if self.seen_proofs.len() >= self.max_proofs {
                // Remove oldest proof (simple cleanup)
                if let Some(oldest_secret) = self.seen_proofs.iter().next().cloned() {
                    self.seen_proofs.remove(&oldest_secret);
                    self.pending_verifications.remove(&oldest_secret);

                    // Remove from pending_proofs
                    for mint_proofs in self.pending_proofs.values_mut() {
                        mint_proofs.remove(&oldest_secret);
                    }
                }
            }

            debug!(
                "Adding new proof to verification queue: secret={}, mint={}",
                &secret[..8.min(secret.len())],
                mint_url
            );

            // Compute Y point and add to pending verifications
            let y_point = self.compute_y_point(&secret);
            self.pending_verifications.insert(secret.clone(), y_point);

            // Add to pending proofs by mint
            self.pending_proofs
                .entry(mint_url.clone())
                .or_default()
                .insert(secret.clone(), proof);

            // Mark as seen
            self.seen_proofs.insert(secret);
        }
    }

    /// Check proofs with mints and return serialized valid proofs (iterative)
    async fn verify_pending_proofs(&mut self) -> Result<Vec<u8>> {
        // Set the running state
        self.verification_running = true;

        let mut valid_proofs: FxHashMap<String, Vec<Proof>> = FxHashMap::default();

        // Keep processing until no more pending proofs
        loop {
            // Take a snapshot of current pending proofs to process
            let current_pending = self.pending_proofs.clone();

            if current_pending.is_empty() {
                break;
            }

            let mut made_progress = false;

            for (mint_url, mint_proofs) in &current_pending {
                if mint_proofs.is_empty() {
                    continue;
                }

                // Get Y points for this mint's proofs
                let mut y_points = Vec::new();
                let mut secret_to_y: FxHashMap<String, String> = FxHashMap::default();

                for secret in mint_proofs.keys() {
                    if let Some(y_point) = self.pending_verifications.get(secret) {
                        y_points.push(y_point.clone());
                        secret_to_y.insert(secret.clone(), y_point.clone());
                    }
                }

                if y_points.is_empty() {
                    continue;
                }

                match self.check_proofs_with_mint(mint_url, &y_points).await {
                    Ok(states) => {
                        // Process the states and collect valid proofs
                        for state in &states {
                            // Find the secret by Y point
                            let mut found_secret = None;
                            for (secret, y_point) in &secret_to_y {
                                if y_point == &state.y {
                                    found_secret = Some(secret.clone());
                                    break;
                                }
                            }

                            if let Some(secret) = found_secret {
                                match state.state.as_str() {
                                    "SPENT" => {
                                        debug!(
                                            "Proof {} is SPENT, dropping from mint {}",
                                            &secret[..8.min(secret.len())],
                                            mint_url
                                        );
                                        self.pending_verifications.remove(&secret);
                                        // Remove from pending proofs
                                        if let Some(mint_proofs) =
                                            self.pending_proofs.get_mut(mint_url)
                                        {
                                            mint_proofs.remove(&secret);
                                        }
                                        made_progress = true;
                                    }
                                    "UNSPENT" => {
                                        debug!(
                                            "Proof {} is UNSPENT, collecting for output from mint {}",
                                            &secret[..8.min(secret.len())],
                                            mint_url
                                        );
                                        // Collect this valid proof
                                        if let Some(proof) = mint_proofs.get(&secret) {
                                            valid_proofs
                                                .entry(mint_url.clone())
                                                .or_default()
                                                .push(proof.clone());
                                        }
                                        // Remove from pending verification
                                        self.pending_verifications.remove(&secret);
                                        // Remove from pending proofs
                                        if let Some(mint_proofs) =
                                            self.pending_proofs.get_mut(mint_url)
                                        {
                                            mint_proofs.remove(&secret);
                                        }
                                        made_progress = true;
                                    }
                                    "PENDING" => {
                                        debug!(
                                            "Proof {} is PENDING, keeping for later check",
                                            &secret[..8.min(secret.len())]
                                        );
                                        // Keep in all tracking for next check
                                    }
                                    unknown => {
                                        warn!("Unknown proof state: {}", unknown);
                                    }
                                }
                            }
                        }
                    }
                    Err(e) => {
                        warn!("Failed to check proofs with mint {}: {}", mint_url, e);
                        // Remove from current pending to avoid immediate retry
                        self.pending_proofs.remove(mint_url);
                        // Remove from pending verifications temporarily
                        for secret in mint_proofs.keys() {
                            self.pending_verifications.remove(secret);
                        }
                        made_progress = true;
                    }
                }
            }

            // Clean up empty mint entries
            self.pending_proofs.retain(|_, proofs| !proofs.is_empty());

            // If we didn't make any progress, break to avoid infinite loop
            if !made_progress {
                break;
            }
        }

        // No more pending proofs, set verification_running to false
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
        tracing::info!("Union value for ValidProofs created");

        let message_args = fb::WorkerMessageArgs {
            type_: fb::MessageType::ValidProofs,
            content_type: fb::Message::ValidProofs,
            content: Some(union_value),
        };
        tracing::info!(
            "WorkerMessage args - content is Some: {}",
            message_args.content.is_some()
        );

        let root = fb::WorkerMessage::create(&mut builder, &message_args);
        builder.finish(root, None);

        let result_bytes = builder.finished_data().to_vec();

        debug!("Total serialized bytes: {}", result_bytes.len());
        Ok(result_bytes)
    }

    /// Make HTTP request to mint's checkstate endpoint
    async fn check_proofs_with_mint(
        &self,
        mint_url: &str,
        y_points: &[String],
    ) -> Result<Vec<ProofState>> {
        debug!(
            "{}: Checking {} proofs with mint: {}",
            self.name,
            y_points.len(),
            mint_url
        );
        let url = format!("{}/v1/checkstate", mint_url.trim_end_matches('/'));

        let request = CheckStateRequest::new(y_points.to_vec());

        // ✅ UPDATED: Use custom JSON serialization instead of serde_json
        let request_body = request.to_json();

        let response = gloo_net::http::Request::post(&url)
            .header("Content-Type", "application/json")
            .body(request_body)?
            .send()
            .await
            .map_err(|e| NostrError::Other(format!("HTTP request failed: {:?}", e)))?;

        if !response.ok() {
            return Err(NostrError::Other(format!(
                "Mint returned status: {}",
                response.status()
            )));
        }

        let response_text = response
            .text()
            .await
            .map_err(|e| NostrError::Other(format!("Failed to read response: {:?}", e)))?;

        info!("Raw mint response: {}", response_text);

        // ✅ UPDATED: Use custom JSON parsing instead of serde_json
        let check_response: CheckStateResponse = CheckStateResponse::from_json(&response_text)?;
        Ok(check_response.states)
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
                    debug!("Attempting to parse Kind 9321 event data");
                    if let Some(parsed_data) = &parsed_event.parsed {
                        // Check if parsed_data is Kind9321Parsed variant
                        if let ParsedData::Kind9321(kind9321) = parsed_data {
                            let mint_url = kind9321.mint_url.clone();

                            debug!("Kind 9321 event - mint_url: {}", mint_url);

                            // Extract proofs directly from the Kind9321Parsed struct
                            let proofs = kind9321.proofs.clone();

                            debug!(
                                "Successfully extracted {} proofs from Kind 9321 event",
                                proofs.len()
                            );
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
                    debug!("Attempting to parse Kind 7375 event data");
                    if let Some(parsed_data) = &parsed_event.parsed {
                        // Check if parsed_data is Kind7375Parsed variant
                        if let ParsedData::Kind7375(kind7375) = parsed_data {
                            let mint_url = kind7375.mint_url.clone();
                            let decrypted = kind7375.decrypted;

                            debug!(
                                "Kind 7375 event - mint_url: {}, decrypted: {}",
                                mint_url, decrypted
                            );

                            if decrypted {
                                debug!(
                                    "Successfully extracted {} proofs from Kind 7375 event",
                                    kind7375.proofs.len()
                                );
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
                self.add_proofs(proofs, mint_url);
            }
        }

        // Trigger verification immediately if we have pending proofs
        if !self.pending_verifications.is_empty() && !self.verification_running {
            match self.verify_pending_proofs().await {
                Ok(bytes) => {
                    if !bytes.is_empty() {
                        debug!(
                            "{}: Returning valid proofs as output: {} bytes",
                            self.name,
                            bytes.len()
                        );
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
                                        self.add_proofs(proofs, mint_url);
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
                                            self.add_proofs(proofs, mint_url);
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
                        debug!(
                            "{}: Returning valid proofs from batch: {} bytes",
                            self.name,
                            bytes.len()
                        );
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

// Custom JSON parsers for the structs
enum ProofStateParserData<'a> {
    Borrowed(&'a [u8]),
    Owned(Vec<u8>),
}

struct ProofStateParser<'a> {
    data: ProofStateParserData<'a>,
}

impl<'a> ProofStateParser<'a> {
    #[inline(always)]
    fn new(bytes: &'a [u8]) -> Self {
        Self {
            data: ProofStateParserData::Borrowed(bytes),
        }
    }

    #[inline(always)]
    fn parse(mut self) -> Result<ProofState> {
        // Get the bytes to parse
        let bytes = match &self.data {
            ProofStateParserData::Borrowed(b) => *b,
            ProofStateParserData::Owned(v) => v.as_slice(),
        };

        // Handle escaped JSON if needed
        let mut parser = if let Some(unescaped) = BaseJsonParser::unescape_if_needed(bytes)
            .map_err(|e| NostrError::Other(format!("Failed to unescape JSON: {:?}", e)))?
        {
            // Use the unescaped data
            self.data = ProofStateParserData::Owned(unescaped);
            match &self.data {
                ProofStateParserData::Owned(v) => BaseJsonParser::new(v.as_slice()),
                _ => unreachable!(),
            }
        } else {
            BaseJsonParser::new(bytes)
        };

        parser.skip_whitespace();
        parser.expect_byte(b'{')?;

        let mut y = String::new();
        let mut state = String::new();
        let mut witness = None;

        while parser.pos < parser.bytes.len() {
            parser.skip_whitespace();
            if parser.peek() == b'}' {
                parser.pos += 1;
                break;
            }

            let key = parser.parse_string()?;
            parser.skip_whitespace();
            parser.expect_byte(b':')?;
            parser.skip_whitespace();

            match key {
                "Y" => {
                    y = parser
                        .parse_string_unescaped()
                        .map_err(|e| NostrError::Other(format!("Failed to parse Y: {:?}", e)))?
                }
                "state" => {
                    state = parser
                        .parse_string_unescaped()
                        .map_err(|e| NostrError::Other(format!("Failed to parse state: {:?}", e)))?
                }
                "witness" => {
                    parser.skip_whitespace();
                    match parser.peek() {
                        b'"' => {
                            // It's a string: parse and unescape
                            witness = Some(parser.parse_string_unescaped().map_err(|e| {
                                NostrError::Other(format!(
                                    "Failed to parse witness string: {:?}",
                                    e
                                ))
                            })?);
                        }
                        b'n' => {
                            // Handle null
                            parser.skip_null()?;
                            witness = None;
                        }
                        _ => {
                            // Skip other types (e.g., objects, arrays, numbers, booleans)
                            parser.skip_value()?;
                            witness = None;
                            warn!("Witness field is not a string or null, skipping: unexpected byte '{}'", parser.peek() as char);
                        }
                    }
                }
                _ => parser.skip_value()?,
            }

            parser.skip_comma_or_end()?;
        }

        if y.is_empty() || state.is_empty() {
            return Err(NostrError::Other(
                "Missing required fields in ProofState".to_string(),
            ));
        }

        Ok(ProofState { y, state, witness })
    }
}

enum CheckStateResponseParserData<'a> {
    Borrowed(&'a [u8]),
    Owned(Vec<u8>),
}

struct CheckStateResponseParser<'a> {
    data: CheckStateResponseParserData<'a>,
}

impl<'a> CheckStateResponseParser<'a> {
    #[inline(always)]
    fn new(bytes: &'a [u8]) -> Self {
        Self {
            data: CheckStateResponseParserData::Borrowed(bytes),
        }
    }

    #[inline(always)]
    fn parse(mut self) -> Result<CheckStateResponse> {
        // Get the bytes to parse
        let bytes = match &self.data {
            CheckStateResponseParserData::Borrowed(b) => *b,
            CheckStateResponseParserData::Owned(v) => v.as_slice(),
        };

        // Handle escaped JSON if needed
        let mut parser = if let Some(unescaped) = BaseJsonParser::unescape_if_needed(bytes)
            .map_err(|e| NostrError::Other(format!("Failed to unescape JSON: {:?}", e)))?
        {
            // Use the unescaped data
            self.data = CheckStateResponseParserData::Owned(unescaped);
            match &self.data {
                CheckStateResponseParserData::Owned(v) => BaseJsonParser::new(v.as_slice()),
                _ => unreachable!(),
            }
        } else {
            BaseJsonParser::new(bytes)
        };

        parser.skip_whitespace();
        parser.expect_byte(b'{')?;

        let mut states = Vec::new();

        while parser.pos < parser.bytes.len() {
            parser.skip_whitespace();
            if parser.peek() == b'}' {
                parser.pos += 1;
                break;
            }

            let key = parser.parse_string()?;
            parser.skip_whitespace();
            parser.expect_byte(b':')?;
            parser.skip_whitespace();

            match key {
                "states" => {
                    parser.expect_byte(b'[')?;
                    while parser.pos < parser.bytes.len() {
                        parser.skip_whitespace();
                        if parser.peek() == b']' {
                            parser.pos += 1;
                            break;
                        }

                        let state_json = parser.parse_raw_json_value()?;
                        states.push(ProofState::from_json(state_json)?);
                        parser.skip_comma_or_end()?;
                    }
                }
                _ => parser.skip_value()?,
            }

            parser.skip_comma_or_end()?;
        }

        Ok(CheckStateResponse { states })
    }
}
