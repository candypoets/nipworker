use crate::nostr::NostrTags;
use crate::parser::{ParserError, Result};
use crate::utils::json::BaseJsonParser;
use flatbuffers::{FlatBufferBuilder, WIPOffset};
use std::fmt::Write;

use crate::generated::nostr::fb;

/// DLEQ (Discrete Log Equality) proof for offline signature validation (NUT-12)
#[derive(Debug, Clone, PartialEq)]
pub struct DleqProof {
    pub e: String,         // Challenge
    pub s: String,         // Response
    pub r: Option<String>, // Blinding factor (for user-to-user transfers)
}

impl DleqProof {
    /// Parse DleqProof from JSON string
    pub fn from_json(json: &str) -> Result<Self> {
        let parser = DleqProofParser::new(json.as_bytes());
        parser.parse()
    }

    /// Serialize DleqProof to JSON string
    pub fn to_json(&self) -> String {
        let mut result = String::with_capacity(self.calculate_json_size());

        result.push('{');
        write!(
            result,
            r#""e":"{}","s":"{}""#,
            Self::escape_string(&self.e),
            Self::escape_string(&self.s)
        )
        .unwrap();

        if let Some(ref r) = self.r {
            result.push_str(r#","r":""#);
            Self::escape_string_to(&mut result, r);
            result.push('"');
        }

        result.push('}');
        result
    }

    #[inline(always)]
    fn calculate_json_size(&self) -> usize {
        15 + // Base structure {"e":"","s":""}
        self.e.len() * 2 + self.s.len() * 2 + // Escaping
        self.r.as_ref().map(|r| r.len() * 2 + 8).unwrap_or(0) // ,"r":"" + escaping
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

/// Helper struct for creating proof test data
#[derive(Debug, Clone)]
pub struct Proof {
    pub amount: u64,
    pub secret: String,
    pub c: String,
    pub id: Option<String>,
    pub version: Option<i32>,
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

    pub fn from_flatbuffer(proof: &fb::Proof<'_>) -> Self {
        Self {
            amount: proof.amount(),
            secret: proof.secret().to_string(),
            c: proof.c().to_string(),
            id: Some(proof.id().to_string()),
            version: if proof.version() != 0 {
                Some(proof.version() as i32)
            } else {
                None
            },
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

    /// Parse Proof from JSON string
    pub fn from_json(json: &str) -> Result<Self> {
        let parser = ProofParser::new(json.as_bytes());
        parser.parse()
    }

    /// Serialize Proof to JSON string
    pub fn to_json(&self) -> String {
        let mut result = String::with_capacity(self.calculate_json_size());

        result.push('{');

        // Required fields
        write!(
            result,
            r#""amount":{},"secret":"{}","C":"{}""#,
            self.amount,
            Self::escape_string(&self.secret),
            Self::escape_string(&self.c)
        )
        .unwrap();

        // Optional fields
        if let Some(ref id) = self.id {
            result.push_str(r#","id":""#);
            Self::escape_string_to(&mut result, id);
            result.push('"');
        }

        if let Some(version) = self.version {
            write!(result, r#","version":{}"#, version).unwrap();
        }

        if let Some(ref dleq) = self.dleq {
            result.push_str(r#","dleq":"#);
            result.push_str(&dleq.to_json());
        }

        result.push('}');
        result
    }

    #[inline(always)]
    fn calculate_json_size(&self) -> usize {
        let mut size = 50; // Base structure with required fields

        // Required fields
        size += self.secret.len() * 2 + self.c.len() * 2; // Escaping

        // Optional fields
        if let Some(ref id) = self.id {
            size += 8 + id.len() * 2; // "id":"" + escaping
        }
        if self.version.is_some() {
            size += 15; // "version":number
        }
        if let Some(ref dleq) = self.dleq {
            size += 8 + dleq.calculate_json_size(); // "dleq":{}
        }

        size
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

/// Cashu TokenContent for Nostr kind 7375 events
#[derive(Debug, Clone)]
pub struct TokenContent {
    pub mint: String,
    pub proofs: Vec<Proof>,
    pub del: Option<Vec<String>>,
}

impl TokenContent {
    /// Parse TokenContent from JSON string
    pub fn from_json(json: &str) -> Result<Self> {
        let parser = TokenContentParser::new(json.as_bytes());
        parser.parse()
    }

    /// Serialize TokenContent to JSON string
    pub fn to_json(&self) -> String {
        let mut result = String::with_capacity(self.calculate_json_size());

        result.push('{');

        // Add mint
        result.push_str(r#""mint":""#);
        Self::escape_string_to(&mut result, &self.mint);
        result.push('"');

        // Add proofs
        result.push_str(r#","proofs":["#);
        for (i, proof) in self.proofs.iter().enumerate() {
            if i > 0 {
                result.push(',');
            }
            result.push_str(&proof.to_json());
        }
        result.push(']');

        // Add del if present
        if let Some(ref del) = self.del {
            result.push_str(r#","del":["#);
            for (i, id) in del.iter().enumerate() {
                if i > 0 {
                    result.push(',');
                }
                result.push('"');
                Self::escape_string_to(&mut result, id);
                result.push('"');
            }
            result.push(']');
        }

        result.push('}');
        result
    }

    #[inline(always)]
    fn calculate_json_size(&self) -> usize {
        let mut size = 20; // Base JSON structure

        // Mint field
        size += 10 + self.mint.len() * 2; // "mint":"" + escaping

        // Proofs array
        size += 12; // "proofs":[]
        for proof in &self.proofs {
            size += proof.calculate_json_size() + 1; // +1 for comma
        }

        // Del array (if present)
        if let Some(ref del) = self.del {
            size += 8; // "del":[]
            for id in del {
                size += id.len() * 2 + 4; // Escaped string + quotes + comma
            }
        }

        size
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

// Specific parsers that use the base parser
struct DleqProofParser<'a>(BaseJsonParser<'a>);

impl<'a> DleqProofParser<'a> {
    #[inline(always)]
    fn new(bytes: &'a [u8]) -> Self {
        Self(BaseJsonParser::new(bytes))
    }

    #[inline(always)]
    fn parse(mut self) -> Result<DleqProof> {
        self.0.skip_whitespace();
        self.0.expect_byte(b'{')?;

        let mut e = String::new();
        let mut s = String::new();
        let mut r = None;

        while self.0.pos < self.0.bytes.len() {
            self.0.skip_whitespace();
            if self.0.peek() == b'}' {
                self.0.pos += 1;
                break;
            }

            let key = self.0.parse_string()?;
            self.0.skip_whitespace();
            self.0.expect_byte(b':')?;
            self.0.skip_whitespace();

            match key {
                "e" => e = self.0.parse_string()?.to_string(),
                "s" => s = self.0.parse_string()?.to_string(),
                "r" => r = Some(self.0.parse_string()?.to_string()),
                _ => self.0.skip_value()?,
            }

            self.0.skip_comma_or_end()?;
        }

        if e.is_empty() || s.is_empty() {
            return Err(ParserError::InvalidFormat(
                "Missing required fields in DleqProof".to_string(),
            ));
        }

        Ok(DleqProof { e, s, r })
    }
}

struct ProofParser<'a>(BaseJsonParser<'a>);

impl<'a> ProofParser<'a> {
    #[inline(always)]
    fn new(bytes: &'a [u8]) -> Self {
        Self(BaseJsonParser::new(bytes))
    }

    #[inline(always)]
    fn parse(mut self) -> Result<Proof> {
        self.0.skip_whitespace();
        self.0.expect_byte(b'{')?;

        let mut amount = 0u64;
        let mut secret = String::new();
        let mut c = String::new();
        let mut id = None;
        let mut version = None;
        let mut dleq = None;

        while self.0.pos < self.0.bytes.len() {
            self.0.skip_whitespace();
            if self.0.peek() == b'}' {
                self.0.pos += 1;
                break;
            }

            let key = self.0.parse_string()?;
            self.0.skip_whitespace();
            self.0.expect_byte(b':')?;
            self.0.skip_whitespace();

            match key {
                "amount" => amount = self.0.parse_u64()?,
                "secret" => secret = self.0.parse_string()?.to_string(),
                "C" => c = self.0.parse_string()?.to_string(),
                "id" => id = Some(self.0.parse_string()?.to_string()),
                "version" => version = Some(self.0.parse_i32()?),
                "dleq" => dleq = Some(DleqProof::from_json(self.0.parse_raw_json_value()?)?),
                _ => self.0.skip_value()?,
            }

            self.0.skip_comma_or_end()?;
        }

        if secret.is_empty() || c.is_empty() {
            return Err(ParserError::InvalidFormat(
                "Missing required fields in Proof".to_string(),
            ));
        }

        Ok(Proof {
            amount,
            secret,
            c,
            id,
            version,
            dleq,
        })
    }
}

struct TokenContentParser<'a>(BaseJsonParser<'a>);

impl<'a> TokenContentParser<'a> {
    #[inline(always)]
    fn new(bytes: &'a [u8]) -> Self {
        Self(BaseJsonParser::new(bytes))
    }

    #[inline(always)]
    fn parse(mut self) -> Result<TokenContent> {
        self.0.skip_whitespace();
        self.0.expect_byte(b'{')?;

        let mut mint = String::new();
        let mut proofs = Vec::new();
        let mut del = None;

        while self.0.pos < self.0.bytes.len() {
            self.0.skip_whitespace();
            if self.0.peek() == b'}' {
                self.0.pos += 1;
                break;
            }

            let key = self.0.parse_string()?;
            self.0.skip_whitespace();
            self.0.expect_byte(b':')?;
            self.0.skip_whitespace();

            match key {
                "mint" => {
                    let mint_str = self.0.parse_string()?;
                    mint = mint_str.to_string();
                }
                "proofs" => {
                    proofs = self.parse_proofs()?;
                }
                "del" => {
                    del = Some(self.parse_string_array()?);
                }
                _ => {
                    self.0.skip_value()?;
                }
            }

            self.0.skip_comma_or_end()?;
        }

        if mint.is_empty() {
            return Err(ParserError::InvalidFormat("Missing mint field".to_string()));
        }

        Ok(TokenContent { mint, proofs, del })
    }

    #[inline(always)]
    fn parse_proofs(&mut self) -> Result<Vec<Proof>> {
        self.0.expect_byte(b'[')?;
        let mut proofs = Vec::new();

        while self.0.pos < self.0.bytes.len() {
            self.0.skip_whitespace();
            if self.0.peek() == b']' {
                self.0.pos += 1;
                break;
            }

            let proof_json = self.0.parse_raw_json_value()?;
            proofs.push(Proof::from_json(proof_json)?);
            self.0.skip_comma_or_end()?;
        }

        Ok(proofs)
    }

    #[inline(always)]
    fn parse_string_array(&mut self) -> Result<Vec<String>> {
        self.0.expect_byte(b'[')?;
        let mut array = Vec::new();

        while self.0.pos < self.0.bytes.len() {
            self.0.skip_whitespace();
            if self.0.peek() == b']' {
                self.0.pos += 1;
                break;
            }

            let value = self.0.parse_string()?.to_string();
            array.push(value);
            self.0.skip_comma_or_end()?;
        }

        Ok(array)
    }
}
