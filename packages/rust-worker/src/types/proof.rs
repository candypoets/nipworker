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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_proof_union_creation() {
        let proof_data =
            ProofData::new(1000, "test_secret".to_string(), "test_c_value".to_string());

        let proof_union = proof_data.to_proof_union();
        assert_eq!(proof_union.amount(), Some(1000));
        assert_eq!(proof_union.secret(), Some("test_secret".to_string()));
        assert_eq!(proof_union.c(), Some("test_c_value".to_string()));
        assert!(!proof_union.is_v4());
    }

    #[test]
    fn test_proof_union_v4() {
        let proof_data =
            ProofData::new(2000, "v4_secret".to_string(), "v4_c_value".to_string()).with_version(4);

        let proof_union = proof_data.to_proof_union();
        assert_eq!(proof_union.amount(), Some(2000));
        assert!(proof_union.is_v4());
        assert_eq!(proof_union.version(), Some(4));
    }

    #[test]
    fn test_proof_union_serialization() {
        let proof_data = ProofData::new(
            1500,
            "serialization_test".to_string(),
            "serialization_c".to_string(),
        );

        let proof_union = proof_data.to_proof_union();
        let json = serde_json::to_string(&proof_union).unwrap();

        let deserialized: ProofUnion = serde_json::from_str(&json).unwrap();
        assert_eq!(proof_union.amount(), deserialized.amount());
        assert_eq!(proof_union.secret(), deserialized.secret());
        assert_eq!(proof_union.c(), deserialized.c());
    }

    #[test]
    fn test_proof_union_from_json() {
        let json = r#"{"amount":1000,"secret":"test","C":"test_c","version":4}"#;
        let proof_union: ProofUnion = serde_json::from_str(json).unwrap();

        assert_eq!(proof_union.amount(), Some(1000));
        assert_eq!(proof_union.secret(), Some("test".to_string()));
        assert_eq!(proof_union.c(), Some("test_c".to_string()));
        assert!(proof_union.is_v4());
    }

    #[test]
    fn test_proof_union_as_proof_methods() {
        let v3_proof =
            ProofData::new(1000, "v3_secret".to_string(), "v3_c".to_string()).to_proof_union();

        let v4_proof = ProofData::new(2000, "v4_secret".to_string(), "v4_c".to_string())
            .with_version(4)
            .to_proof_union();

        assert!(v3_proof.as_proof().is_some());
        assert!(v3_proof.as_proof_v4().is_none());

        assert!(v4_proof.as_proof().is_none());
        assert!(v4_proof.as_proof_v4().is_some());
    }

    #[test]
    fn test_proof_union_display() {
        let proof_union = ProofData::new(1000, "display_test".to_string(), "display_c".to_string())
            .to_proof_union();

        let display = format!("{}", proof_union);
        assert!(display.contains("ProofUnion"));
        assert!(display.contains("1000"));
    }

    #[test]
    fn test_proof_union_default() {
        let proof_union = ProofUnion::default();
        assert!(proof_union.amount().is_none());
        assert!(proof_union.secret().is_none());
        assert!(proof_union.c().is_none());
        assert!(!proof_union.is_v4());
    }

    #[test]
    fn test_dleq_proof() {
        let dleq = DleqProof {
            e: "challenge_value".to_string(),
            s: "response_value".to_string(),
            r: Some("blinding_factor".to_string()),
        };

        let proof_data =
            ProofData::new(1000, "dleq_secret".to_string(), "dleq_c".to_string()).with_dleq(dleq);

        let proof_union = proof_data.to_proof_union();
        assert!(proof_union.has_dleq());
        let retrieved_dleq = proof_union.dleq().unwrap();
        assert_eq!(retrieved_dleq.e, "challenge_value");
        assert_eq!(retrieved_dleq.s, "response_value");
        assert_eq!(retrieved_dleq.r, Some("blinding_factor".to_string()));
    }

    #[test]
    fn test_p2pk_locked_proof() {
        let signatures = vec!["sig1".to_string(), "sig2".to_string()];
        let proof_data = ProofData::new(2000, "p2pk_secret".to_string(), "p2pk_c".to_string())
            .with_p2pk_signatures(signatures.clone());

        let proof_union = proof_data.to_proof_union();
        assert!(proof_union.is_locked());
        assert_eq!(proof_union.p2pk_signatures(), Some(signatures));
    }

    #[test]
    fn test_htlc_proof() {
        let proof_data = ProofData::new(1500, "htlc_secret".to_string(), "htlc_c".to_string())
            .with_htlc_preimage("preimage_value".to_string());

        let proof_union = proof_data.to_proof_union();
        assert!(proof_union.has_htlc_preimage());
        assert_eq!(
            proof_union.htlc_preimage(),
            Some("preimage_value".to_string())
        );
    }

    #[test]
    fn test_proof_with_id() {
        let proof_data = ProofData::new(1000, "test_secret".to_string(), "test_c".to_string())
            .with_id("keyset_123".to_string());

        let proof_union = proof_data.to_proof_union();
        assert_eq!(proof_union.id(), Some("keyset_123".to_string()));
    }
}
