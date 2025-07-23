// import { openDB } from 'idb';
import { init_nostr_client } from "nostr-worker/nostr_worker.js";

// Initialize WASM and set up the worker
const initWorker = async () => {
  try {
    // Initialize the WASM module
    // init_worker() is called automatically via #[wasm_bindgen(start)]
    //
    //

    console.log("WASM worker module initialized successfully");

    // const
    // await init();
    return await init_nostr_client();
  } catch (error) {
    console.log("oops");
    console.error("Failed to initialize WASM worker module:", error);
    throw error;
  }
};

const initialized = initWorker();

self.onmessage = async (event) => {
  // const { type, data } = event.data;
  const client = await initialized;

  client.handle_message(event.data);
};
