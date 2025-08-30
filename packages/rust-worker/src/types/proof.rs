use flatbuffers::{FlatBufferBuilder, WIPOffset};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;
use std::fmt;

use crate::generated::nostr::fb;

/// DLEQ (Discrete Log Equality) proof for offline signature validation (NUT-12)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DleqProof {
    pub e: String, // Challenge
    pub s: String, // Response
    #[serde(skip_serializing_if = "Option::is_none")]
    pub r: Option<String>, // Blinding factor (for user-to-user transfers)
}

/// Helper struct for creating proof test data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Proof {
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
}

impl Proof {
    pub fn new(amount: u64, secret: String, c: String) -> Self {
        Self {
            amount,
            secret,
            c,
            id: None,
            version: None,
            dleq: None,
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

    pub fn to_offset<'a, A: flatbuffers::Allocator + 'a>(
        &self,
        builder: &mut FlatBufferBuilder<'a, A>,
    ) -> WIPOffset<fb::Proof<'a>> {
        let id = self.id.as_ref().map(|id| builder.create_string(id));
        let secret = builder.create_string(&self.secret);
        let c = builder.create_string(&self.c);

        // Build DLEQ proof if present
        let dleq = self.dleq.as_ref().map(|d| {
            let e = builder.create_string(&d.e);
            let s = builder.create_string(&d.s);
            let r = d.r.as_ref().map(|r| builder.create_string(r));

            let dleq_args = fb::DLEQProofArgs {
                e: Some(e),
                s: Some(s),
                r,
            };
            fb::DLEQProof::create(builder, &dleq_args)
        });

        let proof_args = fb::ProofArgs {
            amount: self.amount,
            id,
            secret: Some(secret),
            c: Some(c),
            dleq,
            version: 0,
        };

        return fb::Proof::create(builder, &proof_args);
    }
}
