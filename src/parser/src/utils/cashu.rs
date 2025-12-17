use crate::utils::json::BaseJsonParser;
use crate::NostrError;
use shared::types::proof::{DleqProof, Proof};

use gloo_net::http::Request;
use hex;
use k256::elliptic_curve::sec1::FromEncodedPoint;
use k256::elliptic_curve::PrimeField;
use rustc_hash::FxHashMap;
use sha2::{Digest, Sha256};

use k256::{
    elliptic_curve::sec1::ToEncodedPoint, AffinePoint, EncodedPoint, ProjectivePoint, Scalar,
};
use tracing::info;

type Result<T> = std::result::Result<T, NostrError>;

//
// Public API: filter proofs with valid DLEQ by querying the mint keys
//

pub async fn filter_valid_dleq_proofs_with_mint(
    mint_url: &str,
    proofs: &Vec<Proof>,
) -> Result<Vec<Proof>> {
    let keys_map = fetch_mint_keys_map(mint_url).await?;
    let mut valid = Vec::with_capacity(proofs.len());
    for p in proofs.into_iter() {
        if verify_proof_dleq_with_keys(p, &keys_map) {
            valid.push(p.clone());
        }
    }
    Ok(valid)
}

//
// Core verification logic per NUT-12
//

fn verify_proof_dleq_with_keys(proof: &Proof, keys_map: &FxHashMap<u64, String>) -> bool {
    let dleq: &DleqProof = match &proof.dleq {
        Some(d) => d,
        None => return false,
    };

    let key_hex = match keys_map.get(&proof.amount) {
        Some(k) => k,
        None => return false,
    };

    let a = match point_from_hex_unchecked(key_hex) {
        Some(p) => p,
        None => return false,
    };

    let c = match point_from_hex_unchecked(&proof.c) {
        Some(p) => p,
        None => return false,
    };

    let r = match parse_scalar_hex(&dleq.r.as_deref().unwrap_or("00")) {
        Some(s) => s,
        None => return false,
    };
    let e = match parse_scalar_hex(&dleq.e) {
        Some(s) => s,
        None => return false,
    };
    let s = match parse_scalar_hex(&dleq.s) {
        Some(sv) => sv,
        None => return false,
    };

    // Y = hash_to_curve(secret) (NUT-00)
    let y = match hash_to_curve_point(proof.secret.as_bytes()) {
        Some(p) => p,
        None => return false,
    };

    // Reblind to reconstruct Carol-side B' and C' (NUT-12):
    // B' = Y + r*G
    // C' = C + r*A
    let r_g = ProjectivePoint::from(AffinePoint::GENERATOR) * r;

    let bp = y + r_g;

    let cp = c + (a * r);

    // Verify Chaum-Pedersen DLEQ (NUT-12):
    // R1 = s*G - e*A
    // R2 = s*B' - e*C'
    // e' = H(R1, R2, A, C')  (uncompressed hex concatenation)
    let s_g = ProjectivePoint::from(AffinePoint::GENERATOR) * s;

    let e_a = a * e;

    let r1 = s_g + (-e_a);

    let s_b = bp * s;

    let e_c = cp * e;

    let r2 = s_b + (-e_c);

    let e_prime = compute_challenge_e_nut12(&r1, &r2, &a, &cp);

    let valid = e == e_prime;
    valid
}

//
// Mint keys fetching and JSON parsing without serde_json
//

async fn fetch_mint_keys_map(mint_url: &str) -> Result<FxHashMap<u64, String>> {
    let url = format!("{}/v1/keys", mint_url.trim_end_matches('/'));

    let response = Request::get(&url)
        .send()
        .await
        .map_err(|e| NostrError::Other(format!("HTTP request failed: {:?}", e)))?;

    if !response.ok() {
        return Err(NostrError::Other(format!(
            "Mint returned status: {}",
            response.status()
        )));
    }

    let text = response
        .text()
        .await
        .map_err(|e| NostrError::Other(format!("Failed to read response: {:?}", e)))?;

    parse_mint_keys_map(&text)
}

// Supports both common shapes:
// A) { "keys": { "1": "02...", "2": "03...", ... } }
// B) { "keysets": [ { "keys": { "1": "02...", ... } }, ... ] }
fn parse_mint_keys_map(json: &str) -> Result<FxHashMap<u64, String>> {
    let mut parser = BaseJsonParser::new(json.as_bytes());
    parser.skip_whitespace();
    parser.expect_byte(b'{')?;

    let mut out: FxHashMap<u64, String> = FxHashMap::default();

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
            "keys" => parse_keys_object_into(&mut parser, &mut out)?,
            "keysets" => {
                parser.expect_byte(b'[')?;
                while parser.pos < parser.bytes.len() {
                    parser.skip_whitespace();
                    if parser.peek() == b']' {
                        parser.pos += 1;
                        break;
                    }
                    let keyset_json = parser.parse_raw_json_value()?;
                    parse_keyset_keys_into(keyset_json, &mut out)?;
                    parser.skip_comma_or_end()?;
                }
            }
            _ => parser.skip_value()?,
        }

        parser.skip_comma_or_end()?;
    }

    if out.is_empty() {
        return Err(NostrError::Other("No keys found in mint response".into()));
    }
    Ok(out)
}

fn parse_keys_object_into(
    parser: &mut BaseJsonParser,
    out: &mut FxHashMap<u64, String>,
) -> Result<()> {
    parser.expect_byte(b'{')?;
    while parser.pos < parser.bytes.len() {
        parser.skip_whitespace();
        if parser.peek() == b'}' {
            parser.pos += 1;
            break;
        }
        let amount_key = parser.parse_string()?;
        parser.skip_whitespace();
        parser.expect_byte(b':')?;
        parser.skip_whitespace();
        let pk_hex = parser.parse_string_unescaped()?;
        if let Ok(amount) = amount_key.parse::<u64>() {
            out.insert(amount, pk_hex);
        } else {
            parser.skip_value()?;
        }
        parser.skip_comma_or_end()?;
    }
    Ok(())
}

fn parse_keyset_keys_into(keyset_json: &str, out: &mut FxHashMap<u64, String>) -> Result<()> {
    let mut parser = BaseJsonParser::new(keyset_json.as_bytes());
    parser.skip_whitespace();
    parser.expect_byte(b'{')?;
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
            "keys" => parse_keys_object_into(&mut parser, out)?,
            _ => parser.skip_value()?,
        }
        parser.skip_comma_or_end()?;
    }
    Ok(())
}

//
// Curve helpers (NUT-00 + NUT-12 exact encodings)
//

fn parse_scalar_hex(s: &str) -> Option<Scalar> {
    let s = s.strip_prefix("0x").unwrap_or(s).trim();
    let bytes = hex::decode(s).ok()?;
    let mut be = [0u8; 32];
    if bytes.len() > 32 {
        be.copy_from_slice(&bytes[bytes.len() - 32..]);
    } else {
        be[32 - bytes.len()..].copy_from_slice(&bytes);
    }
    // Use from_repr (no reduction); practically always succeeds for 32-byte values from SHA-256 or valid scalars
    Option::<Scalar>::from(Scalar::from_repr(be.into()))
}

fn point_from_hex_unchecked(hex_str: &str) -> Option<ProjectivePoint> {
    let s = hex_str.strip_prefix("0x").unwrap_or(hex_str).trim();
    let bytes = hex::decode(s).ok()?;
    let ep = EncodedPoint::from_bytes(&bytes).ok()?;
    let aff_opt: Option<AffinePoint> = AffinePoint::from_encoded_point(&ep).into();
    aff_opt.map(ProjectivePoint::from)
}

fn point_to_uncompressed_hex(p: &ProjectivePoint) -> String {
    let aff = AffinePoint::from(*p);
    let ep = aff.to_encoded_point(false); // uncompressed 65 bytes
    hex::encode(ep.as_bytes())
}

// NUT-12: e = SHA256(hex(uncompressed(R1)) || hex(uncompressed(R2)) || hex(uncompressed(A)) || hex(uncompressed(C')))
fn compute_challenge_e_nut12(
    R1: &ProjectivePoint,
    R2: &ProjectivePoint,
    A: &ProjectivePoint,
    Cp: &ProjectivePoint,
) -> Scalar {
    let r1_hex = point_to_uncompressed_hex(R1);
    let r2_hex = point_to_uncompressed_hex(R2);
    let a_hex = point_to_uncompressed_hex(A);
    let c_hex = point_to_uncompressed_hex(Cp);

    let mut hasher = Sha256::new();
    hasher.update(r1_hex.as_bytes());
    hasher.update(r2_hex.as_bytes());
    hasher.update(a_hex.as_bytes());
    hasher.update(c_hex.as_bytes());
    let digest = hasher.finalize();

    let mut be = [0u8; 32];
    be.copy_from_slice(&digest);
    // Use from_repr instead of reduced
    Option::<Scalar>::from(Scalar::from_repr(be.into()))
        .expect("digest should almost always be < n for secp256k1")
}

// NUT-00 hash_to_curve (Cashu)
// Y = PublicKey('02' || SHA256(SHA256(DOMAIN_SEPARATOR||x) || counter_le))
// Find first valid point
fn hash_to_curve_point(secret: &[u8]) -> Option<ProjectivePoint> {
    const DOMAIN_SEPARATOR: &[u8] = b"Secp256k1_HashToCurve_Cashu_";

    let mut h = Sha256::new();
    h.update(DOMAIN_SEPARATOR);
    h.update(secret);
    let msg_hash = h.finalize();

    for counter in 0u32..=u32::MAX {
        let mut hh = Sha256::new();
        hh.update(&msg_hash);
        hh.update(&counter.to_le_bytes());
        let hash = hh.finalize();

        let mut bytes = [0u8; 33];
        bytes[0] = 0x02;
        bytes[1..].copy_from_slice(&hash);

        if let Ok(ep) = EncodedPoint::from_bytes(&bytes[..]) {
            let aff_opt: Option<AffinePoint> = AffinePoint::from_encoded_point(&ep).into();
            if let Some(aff) = aff_opt {
                return Some(ProjectivePoint::from(aff));
            }
        }
        if counter == 65535 {
            break;
        }
    }
    None
}
