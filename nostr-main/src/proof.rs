use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;
use std::fmt;

/// DLEQ (Discrete Log Equality) proof for offline signature validation (NUT-12)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DleqProof {
    pub e: String, // Challenge
    pub s: String, // Response
    #[serde(skip_serializing_if = "Option::is_none")]
    pub r: Option<String>, // Blinding factor (for user-to-user transfers)
}

/// ProofUnion represents either a cashu.Proof or cashu.ProofV4
/// This matches the Go type from types/proof.go
#[derive(Debug, Clone, PartialEq)]
pub struct ProofUnion {
    version: Option<i32>,
    proof: Value,
}

impl ProofUnion {
    pub fn new(proof_json: &str) -> Result<ProofUnion, anyhow::Error> {
        let proof: Value = serde_json::from_str(proof_json)?;

        Ok(ProofUnion::from_value(proof))
    }

    /// Create a ProofUnion from a serde_json::Value
    pub fn from_value(proof: Value) -> Self {
        // Try to determine version from the proof data
        let version = if let Some(obj) = proof.as_object() {
            obj.get("version")
                .and_then(|v| v.as_i64())
                .map(|v| v as i32)
        } else {
            None
        };

        Self { version, proof }
    }

    /// Check if this is a V4 proof
    pub fn is_v4(&self) -> bool {
        self.version == Some(4)
    }

    /// Get the version of the proof
    pub fn version(&self) -> Option<i32> {
        self.version
    }

    /// Get the proof as a JSON string
    pub fn to_json(&self) -> Result<String, anyhow::Error> {
        Ok(serde_json::to_string(&self.proof)?)
    }

    /// Get the proof value (internal use)
    pub fn get_proof(&self) -> &Value {
        &self.proof
    }

    /// Try to get the proof as a regular Proof (version 3 or unspecified)
    pub fn as_proof(&self) -> Option<String> {
        if !self.is_v4() {
            serde_json::to_string(&self.proof).ok()
        } else {
            None
        }
    }

    /// Try to get the proof as a ProofV4
    pub fn as_proof_v4(&self) -> Option<String> {
        if self.is_v4() {
            serde_json::to_string(&self.proof).ok()
        } else {
            None
        }
    }

    /// Get the amount from the proof if available
    pub fn amount(&self) -> Option<u64> {
        self.proof.get("amount").and_then(|v| v.as_u64())
    }

    /// Get the secret from the proof if available
    pub fn secret(&self) -> Option<String> {
        self.proof
            .get("secret")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
    }

    /// Get the C value from the proof if available
    pub fn c(&self) -> Option<String> {
        self.proof
            .get("C")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
    }

    /// Get the keyset ID from the proof if available
    pub fn id(&self) -> Option<String> {
        self.proof
            .get("id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
    }

    /// Get the DLEQ proof if available
    pub fn dleq(&self) -> Option<DleqProof> {
        self.proof
            .get("dleq")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
    }

    /// Check if this proof has a DLEQ proof
    pub fn has_dleq(&self) -> bool {
        self.proof.get("dleq").is_some()
    }

    /// Get P2PK signatures if available
    pub fn p2pk_signatures(&self) -> Option<Vec<String>> {
        self.proof
            .get("p2pksigs")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
    }

    /// Check if this proof is locked (has P2PK signatures)
    pub fn is_locked(&self) -> bool {
        self.proof.get("p2pksigs").is_some()
    }

    /// Get HTLC preimage if available
    pub fn htlc_preimage(&self) -> Option<String> {
        self.proof
            .get("htlcpreimage")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
    }

    /// Check if this proof has an HTLC preimage
    pub fn has_htlc_preimage(&self) -> bool {
        self.proof.get("htlcpreimage").is_some()
    }

    /// Create a new ProofUnion with a specific version
    pub fn with_version(proof_json: &str, version: i32) -> Result<ProofUnion, anyhow::Error> {
        let mut proof: Value = serde_json::from_str(proof_json)?;

        // Set the version in the proof object
        if let Some(obj) = proof.as_object_mut() {
            obj.insert("version".to_string(), Value::Number(version.into()));
        }

        Ok(ProofUnion {
            version: Some(version),
            proof,
        })
    }
}

impl Serialize for ProofUnion {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.proof.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ProofUnion {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let proof = Value::deserialize(deserializer)?;
        Ok(ProofUnion::from_value(proof))
    }
}

impl fmt::Display for ProofUnion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "ProofUnion(version: {:?}, amount: {:?})",
            self.version,
            self.amount()
        )
    }
}

impl Default for ProofUnion {
    fn default() -> Self {
        Self {
            version: None,
            proof: Value::Object(serde_json::Map::new()),
        }
    }
}

/// Helper struct for creating proof test data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProofData {
    pub amount: u64,
    pub secret: String,
    #[serde(rename = "C")]
    pub c: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dleq: Option<DleqProof>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub p2pksigs: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub htlcpreimage: Option<String>,
}

impl ProofData {
    pub fn new(amount: u64, secret: String, c: String) -> Self {
        Self {
            amount,
            secret,
            c,
            id: None,
            version: None,
            dleq: None,
            p2pksigs: None,
            htlcpreimage: None,
        }
    }

    pub fn with_version(mut self, version: i32) -> Self {
        self.version = Some(version);
        self
    }

    pub fn with_id(mut self, id: String) -> Self {
        self.id = Some(id);
        self
    }

    pub fn with_dleq(mut self, dleq: DleqProof) -> Self {
        self.dleq = Some(dleq);
        self
    }

    pub fn with_p2pk_signatures(mut self, sigs: Vec<String>) -> Self {
        self.p2pksigs = Some(sigs);
        self
    }

    pub fn with_htlc_preimage(mut self, preimage: String) -> Self {
        self.htlcpreimage = Some(preimage);
        self
    }

    pub fn to_proof_union(&self) -> ProofUnion {
        let value = serde_json::to_value(self).unwrap();
        ProofUnion::from_value(value)
    }
}
