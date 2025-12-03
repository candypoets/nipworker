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

/// P2PK witness
#[derive(Debug, Clone, PartialEq)]
pub struct P2PKWitness {
    /// An array of signatures in hex format
    pub signatures: Option<Vec<String>>,
}

/// HTLC witness
#[derive(Debug, Clone, PartialEq)]
pub struct HTLCWitness {
    /// preimage
    pub preimage: String,
    /// An array of signatures in hex format
    pub signatures: Option<Vec<String>>,
}

/// Witness enum
#[derive(Debug, Clone, PartialEq)]
pub enum Witness {
    String(String),
    P2PK(P2PKWitness),
    HTLC(HTLCWitness),
}

impl P2PKWitness {
    pub fn from_json(json: &str) -> Result<Self> {
        let mut parser = BaseJsonParser::new(json.as_bytes());
        parser.skip_whitespace();
        parser.expect_byte(b'{')?;
        let mut signatures = None;
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
                "signatures" => {
                    if parser.peek() == b'n' {
                        parser.expect_byte(b'n')?;
                        parser.expect_byte(b'u')?;
                        parser.expect_byte(b'l')?;
                        parser.expect_byte(b'l')?;
                        signatures = None;
                    } else {
                        signatures = Some(Self::parse_string_array(&mut parser)?);
                    }
                }
                _ => parser.skip_value()?,
            }
            parser.skip_comma_or_end()?;
        }
        Ok(P2PKWitness { signatures })
    }

    pub fn to_json(&self) -> String {
        let mut result = String::new();
        result.push('{');
        if let Some(ref sigs) = self.signatures {
            result.push_str(r#""signatures":["#);
            for (i, sig) in sigs.iter().enumerate() {
                if i > 0 {
                    result.push(',');
                }
                result.push('"');
                Self::escape_string_to(&mut result, sig);
                result.push('"');
            }
            result.push(']');
        } else {
            result.push_str(r#""signatures":null"#);
        }
        result.push('}');
        result
    }

    fn parse_string_array(parser: &mut BaseJsonParser) -> Result<Vec<String>> {
        parser.expect_byte(b'[')?;
        let mut array = Vec::new();
        while parser.pos < parser.bytes.len() {
            parser.skip_whitespace();
            if parser.peek() == b']' {
                parser.pos += 1;
                break;
            }
            let value = parser.parse_string_unescaped()?;
            array.push(value);
            parser.skip_comma_or_end()?;
        }
        Ok(array)
    }

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

impl HTLCWitness {
    pub fn from_json(json: &str) -> Result<Self> {
        let mut parser = BaseJsonParser::new(json.as_bytes());
        parser.skip_whitespace();
        parser.expect_byte(b'{')?;
        let mut preimage = String::new();
        let mut signatures = None;
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
                "preimage" => preimage = parser.parse_string_unescaped()?,
                "signatures" => {
                    if parser.peek() == b'n' {
                        parser.expect_byte(b'n')?;
                        parser.expect_byte(b'u')?;
                        parser.expect_byte(b'l')?;
                        parser.expect_byte(b'l')?;
                        signatures = None;
                    } else {
                        signatures = Some(Self::parse_string_array(&mut parser)?);
                    }
                }
                _ => parser.skip_value()?,
            }
            parser.skip_comma_or_end()?;
        }
        if preimage.is_empty() {
            return Err(ParserError::InvalidFormat("Missing preimage".to_string()));
        }
        Ok(HTLCWitness {
            preimage,
            signatures,
        })
    }

    pub fn to_json(&self) -> String {
        let mut result = String::new();
        result.push('{');
        result.push_str(r#""preimage":""#);
        Self::escape_string_to(&mut result, &self.preimage);
        result.push('"');
        if let Some(ref sigs) = self.signatures {
            result.push_str(r#","signatures":["#);
            for (i, sig) in sigs.iter().enumerate() {
                if i > 0 {
                    result.push(',');
                }
                result.push('"');
                Self::escape_string_to(&mut result, sig);
                result.push('"');
            }
            result.push(']');
        } else {
            result.push_str(r#","signatures":null"#);
        }
        result.push('}');
        result
    }

    fn parse_string_array(parser: &mut BaseJsonParser) -> Result<Vec<String>> {
        parser.expect_byte(b'[')?;
        let mut array = Vec::new();
        while parser.pos < parser.bytes.len() {
            parser.skip_whitespace();
            if parser.peek() == b']' {
                parser.pos += 1;
                break;
            }
            let value = parser.parse_string_unescaped()?;
            array.push(value);
            parser.skip_comma_or_end()?;
        }
        Ok(array)
    }

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

impl Witness {
    pub fn from_json(json: &str) -> Result<Self> {
        let mut parser = BaseJsonParser::new(json.as_bytes());
        parser.skip_whitespace();
        if parser.peek() == b'"' {
            let s = parser.parse_string_unescaped()?;
            Ok(Witness::String(s))
        } else if parser.peek() == b'{' {
            parser.expect_byte(b'{')?;
            let mut preimage = None;
            let mut signatures = None;
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
                    "preimage" => preimage = Some(parser.parse_string_unescaped()?),
                    "signatures" => {
                        if parser.peek() == b'n' {
                            parser.expect_byte(b'n')?;
                            parser.expect_byte(b'u')?;
                            parser.expect_byte(b'l')?;
                            parser.expect_byte(b'l')?;
                            signatures = None;
                        } else {
                            signatures = Some(Self::parse_string_array(&mut parser)?);
                        }
                    }
                    _ => parser.skip_value()?,
                }
                parser.skip_comma_or_end()?;
            }
            if let Some(preimage) = preimage {
                Ok(Witness::HTLC(HTLCWitness {
                    preimage,
                    signatures,
                }))
            } else {
                Ok(Witness::P2PK(P2PKWitness { signatures }))
            }
        } else {
            Err(ParserError::InvalidFormat(
                "Invalid Witness JSON".to_string(),
            ))
        }
    }

    pub fn to_json(&self) -> String {
        match self {
            Witness::String(s) => {
                let mut result = String::new();
                result.push('"');
                Self::escape_string_to(&mut result, s);
                result.push('"');
                result
            }
            Witness::P2PK(p) => p.to_json(),
            Witness::HTLC(h) => h.to_json(),
        }
    }

    fn parse_string_array(parser: &mut BaseJsonParser) -> Result<Vec<String>> {
        parser.expect_byte(b'[')?;
        let mut array = Vec::new();
        while parser.pos < parser.bytes.len() {
            parser.skip_whitespace();
            if parser.peek() == b']' {
                parser.pos += 1;
                break;
            }
            let value = parser.parse_string_unescaped()?;
            array.push(value);
            parser.skip_comma_or_end()?;
        }
        Ok(array)
    }

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
    pub witness: Option<Witness>,
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
            witness: None,
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
            dleq: proof.dleq().map(|d| DleqProof {
                e: d.e().to_string(),
                s: d.s().to_string(),
                r: Some(d.r().to_string()),
            }),
            witness: None,
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

        // DLEQ proof (if present)
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

        // Witness union
        let (witness, witness_type) = self
            .witness
            .as_ref()
            .map(|w| match w {
                Witness::String(s) => {
                    // Wrap the string into the WitnessString table
                    let s_off = builder.create_string(s);
                    let ws = fb::WitnessString::create(
                        builder,
                        &fb::WitnessStringArgs { value: Some(s_off) },
                    );
                    (Some(ws.as_union_value()), fb::Witness::WitnessString)
                }
                Witness::P2PK(p) => {
                    let signatures = p.signatures.as_ref().map(|sigs| {
                        let sig_offsets: Vec<_> =
                            sigs.iter().map(|sig| builder.create_string(sig)).collect();
                        builder.create_vector(&sig_offsets)
                    });
                    let p2pk =
                        fb::P2PKWitness::create(builder, &fb::P2PKWitnessArgs { signatures });
                    (Some(p2pk.as_union_value()), fb::Witness::P2PKWitness)
                }
                Witness::HTLC(h) => {
                    let preimage = builder.create_string(&h.preimage);
                    let signatures = h.signatures.as_ref().map(|sigs| {
                        let sig_offsets: Vec<_> =
                            sigs.iter().map(|sig| builder.create_string(sig)).collect();
                        builder.create_vector(&sig_offsets)
                    });
                    let htlc = fb::HTLCWitness::create(
                        builder,
                        &fb::HTLCWitnessArgs {
                            preimage: Some(preimage),
                            signatures,
                        },
                    );
                    (Some(htlc.as_union_value()), fb::Witness::HTLCWitness)
                }
            })
            .unwrap_or((None, fb::Witness::NONE));

        if let Some(ref w) = self.witness {}
        if let Some(ref d) = self.dleq {}

        // IMPORTANT: keep witness + witness_type; don't overwrite proof_args
        let proof_args = fb::ProofArgs {
            amount: self.amount,
            id,
            secret: Some(secret),
            c: Some(c),
            dleq,
            witness,
            witness_type,
            version: 0,
        };

        fb::Proof::create(builder, &proof_args)
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
enum DleqProofParserData<'a> {
    Borrowed(&'a [u8]),
    Owned(Vec<u8>),
}

struct DleqProofParser<'a> {
    data: DleqProofParserData<'a>,
}

impl<'a> DleqProofParser<'a> {
    #[inline(always)]
    fn new(bytes: &'a [u8]) -> Self {
        Self {
            data: DleqProofParserData::Borrowed(bytes),
        }
    }

    #[inline(always)]
    fn parse(mut self) -> Result<DleqProof> {
        // Get the bytes to parse
        let bytes = match &self.data {
            DleqProofParserData::Borrowed(b) => *b,
            DleqProofParserData::Owned(v) => v.as_slice(),
        };

        // Handle escaped JSON if needed
        let mut parser = if let Some(unescaped) = BaseJsonParser::unescape_if_needed(bytes)? {
            // Use the unescaped data
            self.data = DleqProofParserData::Owned(unescaped);
            match &self.data {
                DleqProofParserData::Owned(v) => BaseJsonParser::new(v.as_slice()),
                _ => unreachable!(),
            }
        } else {
            BaseJsonParser::new(bytes)
        };

        parser.skip_whitespace();
        parser.expect_byte(b'{')?;

        let mut e = String::new();
        let mut s = String::new();
        let mut r = None;

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
                "e" => e = parser.parse_string_unescaped()?,
                "s" => s = parser.parse_string_unescaped()?,
                "r" => r = Some(parser.parse_string_unescaped()?),
                _ => parser.skip_value()?,
            }

            parser.skip_comma_or_end()?;
        }

        if e.is_empty() || s.is_empty() {
            return Err(ParserError::InvalidFormat(
                "Missing required fields in DleqProof".to_string(),
            ));
        }

        Ok(DleqProof { e, s, r })
    }
}

enum ProofParserData<'a> {
    Borrowed(&'a [u8]),
    Owned(Vec<u8>),
}

struct ProofParser<'a> {
    data: ProofParserData<'a>,
}

impl<'a> ProofParser<'a> {
    #[inline(always)]
    fn new(bytes: &'a [u8]) -> Self {
        Self {
            data: ProofParserData::Borrowed(bytes),
        }
    }

    #[inline(always)]
    fn parse(mut self) -> Result<Proof> {
        // Get the bytes to parse
        let bytes = match &self.data {
            ProofParserData::Borrowed(b) => *b,
            ProofParserData::Owned(v) => v.as_slice(),
        };

        // Handle escaped JSON if needed
        let mut parser = if let Some(unescaped) = BaseJsonParser::unescape_if_needed(bytes)? {
            // Use the unescaped data
            self.data = ProofParserData::Owned(unescaped);
            match &self.data {
                ProofParserData::Owned(v) => BaseJsonParser::new(v.as_slice()),
                _ => unreachable!(),
            }
        } else {
            BaseJsonParser::new(bytes)
        };

        parser.skip_whitespace();
        parser.expect_byte(b'{')?;

        let mut amount = 0u64;
        let mut secret = String::new();
        let mut c = String::new();
        let mut id = None;
        let mut version = None;
        let mut dleq = None;
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
                "amount" => amount = parser.parse_u64()?,
                "secret" => secret = parser.parse_string_unescaped()?,
                "C" => c = parser.parse_string_unescaped()?,
                "id" => id = Some(parser.parse_string_unescaped()?),
                "version" => version = Some(parser.parse_i32()?),
                "dleq" => dleq = Some(DleqProof::from_json(parser.parse_raw_json_value()?)?),
                "witness" => witness = Some(Witness::from_json(parser.parse_raw_json_value()?)?),
                _ => parser.skip_value()?,
            }

            parser.skip_comma_or_end()?;
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
            witness,
        })
    }
}

enum TokenContentParserData<'a> {
    Borrowed(&'a [u8]),
    Owned(Vec<u8>),
}

struct TokenContentParser<'a> {
    data: TokenContentParserData<'a>,
}

impl<'a> TokenContentParser<'a> {
    #[inline(always)]
    fn new(bytes: &'a [u8]) -> Self {
        Self {
            data: TokenContentParserData::Borrowed(bytes),
        }
    }

    #[inline(always)]
    fn parse(mut self) -> Result<TokenContent> {
        // Get the bytes to parse
        let bytes = match &self.data {
            TokenContentParserData::Borrowed(b) => *b,
            TokenContentParserData::Owned(v) => v.as_slice(),
        };

        // Handle escaped JSON if needed
        let mut parser = if let Some(unescaped) = BaseJsonParser::unescape_if_needed(bytes)? {
            // Use the unescaped data
            self.data = TokenContentParserData::Owned(unescaped);
            match &self.data {
                TokenContentParserData::Owned(v) => BaseJsonParser::new(v.as_slice()),
                _ => unreachable!(),
            }
        } else {
            BaseJsonParser::new(bytes)
        };

        parser.skip_whitespace();
        parser.expect_byte(b'{')?;

        let mut mint = String::new();
        let mut proofs = Vec::new();
        let mut del = None;

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
                "mint" => {
                    mint = parser.parse_string_unescaped()?;
                }
                "proofs" => {
                    proofs = self.parse_proofs(&mut parser)?;
                }
                "del" => {
                    del = Some(self.parse_string_array(&mut parser)?);
                }
                _ => {
                    parser.skip_value()?;
                }
            }

            parser.skip_comma_or_end()?;
        }

        if mint.is_empty() {
            return Err(ParserError::InvalidFormat("Missing mint field".to_string()));
        }

        Ok(TokenContent { mint, proofs, del })
    }

    #[inline(always)]
    fn parse_proofs(&self, parser: &mut BaseJsonParser) -> Result<Vec<Proof>> {
        parser.expect_byte(b'[')?;
        let mut proofs = Vec::new();

        while parser.pos < parser.bytes.len() {
            parser.skip_whitespace();
            if parser.peek() == b']' {
                parser.pos += 1;
                break;
            }

            let proof_json = parser.parse_raw_json_value()?;
            proofs.push(Proof::from_json(proof_json)?);
            parser.skip_comma_or_end()?;
        }

        Ok(proofs)
    }

    #[inline(always)]
    fn parse_string_array(&self, parser: &mut BaseJsonParser) -> Result<Vec<String>> {
        parser.expect_byte(b'[')?;
        let mut array = Vec::new();

        while parser.pos < parser.bytes.len() {
            parser.skip_whitespace();
            if parser.peek() == b']' {
                parser.pos += 1;
                break;
            }

            let value = parser.parse_string()?.to_string();
            array.push(value);
            parser.skip_comma_or_end()?;
        }

        Ok(array)
    }
}
