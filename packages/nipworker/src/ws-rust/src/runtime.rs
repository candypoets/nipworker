use crate::registry::ConnectionRegistry;
use crate::ring_buffer::ByteRingBuffer;
use crate::types::{InboundEnvelope, RelayConfig};
use crate::utils::js_interop::sleep_ms;
use js_sys::{SharedArrayBuffer, Uint8Array};
use std::collections::HashMap;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::spawn_local;

const MIN_BACKOFF_MS: u32 = 10;
const MAX_BACKOFF_MS: u32 = 1000;

#[wasm_bindgen]
pub struct WSRuntime {
    input_rings: Vec<ByteRingBuffer>,
    output_rings: Vec<ByteRingBuffer>,
    registry: ConnectionRegistry,
    sub_id_to_ring: HashMap<String, usize>,
    decoder: SimpleDecoder, // Manual UTF-8
    last_ring_index: usize,
    backoff_ms: u32,
    loop_future: Option<JsFuture>,
}

struct SimpleDecoder; // Manual decode for minimal overhead

impl SimpleDecoder {
    fn decode(&self, bytes: &Uint8Array) -> String {
        // Simple UTF-8 decode (no full lib; assume valid input)
        String::from_utf8_lossy(bytes.to_vec()).to_string()
    }
}

#[wasm_bindgen]
impl WSRuntime {
    #[wasm_bindgen(constructor)]
    pub fn new(in_rings_js: JsValue, out_rings_js: JsValue, relay_config: RelayConfig) -> Self {
        let in_rings = Array::from(&in_rings_js)
            .iter()
            .map(|v| {
                let sab: SharedArrayBuffer = v.dyn_into().unwrap();
                ByteRingBuffer::new(sab)
            })
            .collect();
        let out_rings = Array::from(&out_rings_js)
            .iter()
            .map(|v| {
                let sab: SharedArrayBuffer = v.dyn_into().unwrap();
                ByteRingBuffer::new(sab)
            })
            .collect();
        let registry = ConnectionRegistry::new(relay_config);
        let mut runtime = Self {
            input_rings: in_rings,
            output_rings: out_rings,
            registry,
            sub_id_to_ring: HashMap::new(),
            decoder: SimpleDecoder,
            last_ring_index: 0,
            backoff_ms: MIN_BACKOFF_MS,
            loop_future: None,
        };
        runtime.schedule_loop();
        runtime
    }

    pub fn wake(&mut self) {
        self.backoff_ms = MIN_BACKOFF_MS;
        if let Some(f) = self.loop_future.take() {
            // Clear if possible; simplified
        }
        self.schedule_loop();
    }

    pub fn destroy(&mut self) {
        self.loop_future = None;
        self.sub_id_to_ring.clear();
        // Disconnect all
        spawn_local(async move {
            self.registry.disconnect_all().await.unwrap();
        });
    }

    fn schedule_loop(&mut self) {
        let backoff = self.backoff_ms;
        let mut self_clone = self.clone(); // For closure
        self.loop_future = Some(JsFuture::from(sleep_ms(backoff)));
        spawn_local(async move {
            JsFuture::from(self_clone.loop_future.unwrap())
                .await
                .unwrap();
            self_clone.process_loop().await;
        });
    }

    async fn process_loop(&mut self) {
        let mut processed = 0;
        let ring_count = self.input_rings.len();
        if ring_count == 0 {
            self.backoff_ms = std::cmp::min(self.backoff_ms * 2, MAX_BACKOFF_MS);
            self.schedule_loop();
            return;
        }

        let mut made_progress = true;
        while made_progress {
            made_progress = false;
            for i in 0..ring_count {
                let idx = (self.last_ring_index + i) % ring_count;
                let ring = &mut self.input_rings[idx];
                if let Some(record) = ring.read() {
                    made_progress = true;
                    processed += 1;

                    let envelope_str = self.decoder.decode(&record);
                    let envelope = self.extract_envelope(&envelope_str);
                    if let Some(env) = envelope {
                        spawn_local(async move {
                            self.registry
                                .send_to_relays(
                                    js_object! { "relays" => env.relays, "frames" => env.frames },
                                    None,
                                    None,
                                )
                                .await
                                .unwrap();
                        });
                    }
                    self.last_ring_index = (idx + 1) % ring_count;
                }
            }
        }

        self.backoff_ms = if processed > 0 {
            MIN_BACKOFF_MS
        } else {
            std::cmp::min(self.backoff_ms * 2, MAX_BACKOFF_MS)
        };
        self.schedule_loop();
    }

    fn extract_envelope(&self, s: &str) -> Option<InboundEnvelope> {
        // Manual scan: Find {"relays":[...],"frames":[...]}
        // Assume format; slice arrays without JSON.parse
        // Simplified: Find positions of "relays" and "frames", extract quoted strings
        let relays_start = s.find(r#""relays":"#)?;
        let frames_start = s.find(r#""frames":"#)?;
        let mut relays = Vec::new();
        let mut frames = Vec::new();
        // Extract from arrays (basic parser: find quoted strings in [ ])
        // Implementation: Iterate chars, collect between " "
        Some(InboundEnvelope { relays, frames }) // Full impl would scan for [ "url" , "url" ]
    }

    fn hash_sub_id(&self, sub_id: &str) -> usize {
        let target = if sub_id.contains('_') {
            sub_id.split('_').nth(1).unwrap_or("")
        } else {
            sub_id
        };
        let mut hash = 0i32;
        for c in target.chars() {
            hash = (hash << 5) - hash + c as i32;
        }
        hash.abs() as usize % self.output_rings.len()
    }

    fn get_out_ring_for_sub_id(&mut self, sub_id: &str) -> &ByteRingBuffer {
        if let Some(&idx) = self.sub_id_to_ring.get(sub_id) {
            &self.output_rings[idx]
        } else {
            let idx = self.hash_sub_id(sub_id);
            self.sub_id_to_ring.insert(sub_id.to_string(), idx);
            &self.output_rings[idx]
        }
    }

    fn write_envelope(&self, output_ring: &ByteRingBuffer, url: &str, raw_text: &str) {
        // Manual byte packing: url len (u16), url bytes, raw len (u32), raw bytes
        let url_bytes = url.as_bytes();
        let raw_bytes = raw_text.as_bytes();
        let total_len = 2 + url_bytes.len() as u32 + 4 + raw_bytes.len() as u32;
        let mut out = Uint8Array::new(total_len as u32);
        let view = DataView::new(&out.buffer(), 0, total_len as usize);
        let mut o = 0;
        view.set_uint16(o, url_bytes.len() as u16, false);
        o += 2;
        out.set(&Uint8Array::from(url_bytes), o);
        o += url_bytes.len() as u32;
        view.set_uint32(o, raw_bytes.len() as u32, false);
        o += 4;
        out.set(&Uint8Array::from(raw_bytes), o);
        output_ring.write(&out);
    }

    fn extract_sub_id_fast(s: &str) -> Option<String> {
        // Same manual scan as in connection.rs
        // (Copy the impl from extract_sub_id)
        None // See connection.rs for full
    }
}
