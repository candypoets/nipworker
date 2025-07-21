use serde::{Deserialize, Serialize, Deserializer, Serializer};
use serde_json::Value;
use std::fmt;


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
            obj.get("version").and_then(|v| v.as_i64()).map(|v| v as i32)
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
        self.proof.get("amount")
            .and_then(|v| v.as_u64())
    }

    /// Get the secret from the proof if available
    pub fn secret(&self) -> Option<String> {
        self.proof.get("secret")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
    }

    /// Get the C value from the proof if available
    pub fn c(&self) -> Option<String> {
        self.proof.get("C")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
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
    pub version: Option<i32>,
}

impl ProofData {
    pub fn new(amount: u64, secret: String, c: String) -> Self {
        Self {
            amount,
            secret,
            c,
            version: None,
        }
    }

    pub fn with_version(mut self, version: i32) -> Self {
        self.version = Some(version);
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
        let proof_data = ProofData::new(
            1000,
            "test_secret".to_string(),
            "test_c_value".to_string(),
        );
        
        let proof_union = proof_data.to_proof_union();
        assert_eq!(proof_union.amount(), Some(1000));
        assert_eq!(proof_union.secret(), Some("test_secret".to_string()));
        assert_eq!(proof_union.c(), Some("test_c_value".to_string()));
        assert!(!proof_union.is_v4());
    }

    #[test]
    fn test_proof_union_v4() {
        let proof_data = ProofData::new(
            2000,
            "v4_secret".to_string(),
            "v4_c_value".to_string(),
        ).with_version(4);
        
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
        let v3_proof = ProofData::new(
            1000,
            "v3_secret".to_string(),
            "v3_c".to_string(),
        ).to_proof_union();
        
        let v4_proof = ProofData::new(
            2000,
            "v4_secret".to_string(),
            "v4_c".to_string(),
        ).with_version(4).to_proof_union();
        
        assert!(v3_proof.as_proof().is_some());
        assert!(v3_proof.as_proof_v4().is_none());
        
        assert!(v4_proof.as_proof().is_none());
        assert!(v4_proof.as_proof_v4().is_some());
    }

    #[test]
    fn test_proof_union_display() {
        let proof_union = ProofData::new(
            1000,
            "display_test".to_string(),
            "display_c".to_string(),
        ).to_proof_union();
        
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
}