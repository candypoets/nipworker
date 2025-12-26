use k256::schnorr::SigningKey;
use serde_json::json;
use shared::types::{Event, EventId, Keys, UnsignedEvent};
use shared::SabRing;
use signature::hazmat::{PrehashSigner, PrehashVerifier};
use std::cell::RefCell;
use std::rc::Rc;
use tracing::{info, warn};
use wasm_bindgen::prelude::*;

pub struct Transport {
    ws_req: Rc<RefCell<SabRing>>,
    relays: Vec<String>,
    app_name: Option<String>,
    client_keys: Keys,
    client_pubkey_hex: String,
}

impl Transport {
    pub fn new(
        ws_req: Rc<RefCell<SabRing>>,
        relays: Vec<String>,
        app_name: Option<String>,
        client_keys: Keys,
    ) -> Self {
        let client_pubkey_hex = client_keys.public_key().to_hex();
        Self {
            ws_req,
            relays,
            app_name,
            client_keys,
            client_pubkey_hex,
        }
    }

    pub fn open_req_subscription(&self, sub_id: &str, unix_time: u32) {
        let filter = json!({
            "kinds": [24133],
            "#p": [self.client_pubkey_hex]
        })
        .to_string();

        let frame = format!(r#"["REQ","{}",{}]"#, sub_id, filter);
        self.publish_frames(&[frame]);
    }

    pub fn send_close(&self, sub_id: &str) {
        let frame = format!(r#"["CLOSE","{}"]"#, sub_id);
        self.publish_frames(&[frame]);
    }

    pub fn publish_nip46_event(
        &self,
        encrypted_content: &str,
        remote_pubkey: &str,
        unix_time: u32,
    ) -> Result<(), JsValue> {
        let mut tags = vec![vec!["p".to_string(), remote_pubkey.to_string()]];
        if let Some(app) = &self.app_name {
            tags.push(vec!["client".to_string(), app.clone()]);
        }

        let kind: u32 = 24133;

        let unsigned_event = UnsignedEvent::new(
            &self.client_pubkey_hex,
            kind as u16,
            encrypted_content.to_string(),
            tags,
        )
        .map_err(|e| JsValue::from_str(&format!("Failed to create unsigned event: {}", e)))?;

        let mut event = Event {
            id: EventId([0u8; 32]),
            pubkey: unsigned_event.pubkey,
            created_at: unix_time as u64,
            kind: unsigned_event.kind,
            tags: unsigned_event.tags,
            content: unsigned_event.content,
            sig: String::new(),
        };

        // Compute the event ID
        let event_id_hex = shared::nostr_crypto::compute_event_id(
            &event.pubkey,
            event.created_at,
            event.kind,
            &event.tags,
            &event.content,
        );
        event.id = shared::types::EventId::from_hex(&event_id_hex)
            .map_err(|e| JsValue::from_str(&format!("Failed to parse event ID: {}", e)))?;

        let secret_key = self
            .client_keys
            .secret_key()
            .map_err(|e| JsValue::from_str(&format!("Failed to get secret key: {}", e)))?;

        let signing_key = SigningKey::from_bytes(&secret_key.0)
            .map_err(|e| JsValue::from_str(&format!("Failed to create signing key: {}", e)))?;

        let verifying_key = signing_key.verifying_key();

        let signature = signing_key
            .sign_prehash(&event.id.to_bytes())
            .map_err(|e| JsValue::from_str(&format!("Schnorr prehash sign failed: {}", e)))?;

        verifying_key
            .verify_prehash(&event.id.to_bytes(), &signature)
            .map_err(|e| {
                JsValue::from_str(&format!("Local Schnorr prehash verify failed: {}", e))
            })?;

        event.sig = hex::encode(signature.to_bytes());

        let frame = format!(r#"["EVENT",{}]"#, event.to_json());
        self.publish_frames(&[frame]);
        Ok(())
    }

    pub fn publish_frames(&self, frames: &[String]) {
        info!(
            "[nip46] publish_frames: sending {} frames to relays: {:?}",
            frames.len(),
            self.relays
        );
        let env = json!({
            "relays": self.relays,
            "frames": frames,
        });

        match serde_json::to_vec(&env) {
            Ok(buf) => {
                let ok = self.ws_req.borrow_mut().write(&buf);
                if ok {
                    info!(
                        "[nip46] publish_frames: successfully wrote {} bytes to ws_req ring",
                        buf.len()
                    );
                } else {
                    warn!(
                        "[nip46] ws_req ring full, dropped {} frame(s)",
                        frames.len()
                    );
                }
            }
            Err(e) => warn!("[nip46] failed to serialize Envelope: {}", e),
        }
    }
}
