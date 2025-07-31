use super::super::*;
use crate::parser::{Kind7375Parsed, Kind9321Parsed};
use crate::types::proof::ProofUnion;
use anyhow::Result;
use gloo_net;
use gloo_timers;
use hex;
use k256::{
    elliptic_curve::{ops::Reduce, sec1::ToEncodedPoint},
    ProjectivePoint, PublicKey, Scalar, U256,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use tracing::{debug, error, info, warn};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CheckStateRequest {
    #[serde(rename = "Ys")]
    ys: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ProofState {
    #[serde(rename = "Y")]
    y: String,
    state: String, // "UNSPENT", "PENDING", "SPENT"
    witness: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CheckStateResponse {
    states: Vec<ProofState>,
}

/// Pipe that extracts proofs from Kind 9321 and 7375 events and verifies their state with mints
pub struct ProofVerificationPipe {
    seen_proofs: HashSet<String>, // secrets we've already seen
    pending_verifications: HashMap<String, String>, // secret -> Y point
    pending_proofs: HashMap<String, HashMap<String, ProofUnion>>, // mint_url -> secret -> proof
    max_proofs: usize,
    name: String,
    verification_running: bool,
}

impl ProofVerificationPipe {
    pub fn new(max_proofs: usize) -> Self {
        Self {
            seen_proofs: HashSet::new(),
            pending_verifications: HashMap::new(),
            pending_proofs: HashMap::new(),
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
    fn add_proofs(&mut self, proofs: Vec<ProofUnion>, mint_url: String) {
        for proof in proofs {
            if let Some(secret) = proof.secret() {
                // Skip if we've already seen this proof
                if self.seen_proofs.contains(&secret) {
                    info!(
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

                info!(
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
    }

    /// Check proofs with mints and return serialized valid proofs (iterative)
    async fn verify_pending_proofs(&mut self) -> Result<Vec<u8>> {
        // Set the running state
        self.verification_running = true;

        let mut valid_proofs: HashMap<String, Vec<ProofUnion>> = HashMap::new();

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
                let mut secret_to_y: HashMap<String, String> = HashMap::new();

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
                                        info!(
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
                                        info!(
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
        let mut result_bytes = Vec::new();

        for (mint_url, proofs) in &valid_proofs {
            let message = WorkerToMainMessage::Proofs {
                mint: mint_url.clone(),
                proofs: proofs.clone(),
            };
            match rmp_serde::to_vec_named(&message) {
                Ok(bytes) => {
                    result_bytes.extend(&bytes);
                    debug!(
                        "Serialized {} proofs from mint {} to {} bytes",
                        proofs.len(),
                        mint_url,
                        bytes.len()
                    );
                }
                Err(e) => {
                    error!("Failed to serialize proofs from mint {}: {}", mint_url, e);
                }
            }
        }

        debug!("Total serialized bytes: {}", result_bytes.len());
        Ok(result_bytes)
    }

    /// Make HTTP request to mint's checkstate endpoint
    async fn check_proofs_with_mint(
        &self,
        mint_url: &str,
        y_points: &[String],
    ) -> Result<Vec<ProofState>> {
        info!(
            "{}: Checking {} proofs with mint: {}",
            self.name,
            y_points.len(),
            mint_url
        );
        let url = format!("{}/v1/checkstate", mint_url.trim_end_matches('/'));

        let request = CheckStateRequest {
            ys: y_points.to_vec(),
        };

        let request_body = serde_json::to_string(&request)?;

        let response = gloo_net::http::Request::post(&url)
            .header("Content-Type", "application/json")
            .body(request_body)?
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("HTTP request failed: {:?}", e))?;

        if !response.ok() {
            return Err(anyhow::anyhow!(
                "Mint returned status: {}",
                response.status()
            ));
        }

        let response_text = response
            .text()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to read response: {:?}", e))?;

        let check_response: CheckStateResponse = serde_json::from_str(&response_text)?;
        Ok(check_response.states)
    }
}

#[async_trait(?Send)]
impl Pipe for ProofVerificationPipe {
    async fn process(&mut self, event: PipelineEvent) -> Result<PipeOutput> {
        // Only process parsed events
        if let Some(ref parsed_event) = event.parsed {
            let kind = parsed_event.event.kind.as_u64();

            // Extract proofs from Kind 9321 or 7375
            let (proofs, mint_url) = match kind {
                9321 => {
                    info!("Attempting to parse Kind 9321 event data");
                    if let Some(parsed_data) = &parsed_event.parsed {
                        // Extract fields directly from the JSON object
                        if let Some(kind9321_object) = parsed_data.as_object() {
                            let mint_url = kind9321_object
                                .get("mintUrl")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();

                            info!("Kind 9321 event - mint_url: {}", mint_url);

                            // Extract proofs from the JSON array
                            if let Some(proofs_array) =
                                kind9321_object.get("proofs").and_then(|v| v.as_array())
                            {
                                let mut proofs = Vec::new();
                                for proof_value in proofs_array {
                                    if let Ok(proof) =
                                        serde_json::from_value::<ProofUnion>(proof_value.clone())
                                    {
                                        proofs.push(proof);
                                    }
                                }
                                info!(
                                    "Successfully extracted {} proofs from Kind 9321 event",
                                    proofs.len()
                                );
                                (proofs, mint_url)
                            } else {
                                (Vec::new(), mint_url)
                            }
                        } else {
                            error!("Kind 9321 parsed_data is not a JSON object");
                            (Vec::new(), String::new())
                        }
                    } else {
                        error!("Kind 9321 event has no parsed_data");
                        (Vec::new(), String::new())
                    }
                }
                7375 => {
                    info!("Attempting to parse Kind 7375 event data");
                    if let Some(parsed_data) = &parsed_event.parsed {
                        // Extract fields directly from the JSON object
                        if let Some(kind7375_object) = parsed_data.as_object() {
                            let mint_url = kind7375_object
                                .get("mintUrl")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();

                            let decrypted = kind7375_object
                                .get("decrypted")
                                .and_then(|v| v.as_bool())
                                .unwrap_or(false);

                            info!(
                                "Kind 7375 event - mint_url: {}, decrypted: {}",
                                mint_url, decrypted
                            );

                            if decrypted {
                                // Extract proofs from the JSON array
                                if let Some(proofs_array) =
                                    kind7375_object.get("proofs").and_then(|v| v.as_array())
                                {
                                    let mut proofs = Vec::new();
                                    for proof_value in proofs_array {
                                        if let Ok(proof) = serde_json::from_value::<ProofUnion>(
                                            proof_value.clone(),
                                        ) {
                                            proofs.push(proof);
                                        }
                                    }
                                    info!(
                                        "Successfully extracted {} proofs from Kind 7375 event",
                                        proofs.len()
                                    );
                                    (proofs, mint_url)
                                } else {
                                    (Vec::new(), mint_url)
                                }
                            } else {
                                (Vec::new(), String::new())
                            }
                        } else {
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
                        info!(
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

    fn can_direct_output(&self) -> bool {
        true // This is a terminal pipe that outputs serialized proof data
    }

    fn name(&self) -> &str {
        &self.name
    }
}
