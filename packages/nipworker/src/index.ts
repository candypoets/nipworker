import { SharedBufferReader } from "src/lib/sharedBuffer";
import {
  type MainToWorkerMessage,
  type PipelineConfig,
  type Request,
  type SubscriptionConfig,
  type WorkerToMainMessage,
  type ConnectionStatus
} from "@candypoets/rust-main";

import type { AnyKind, ParsedEvent } from "src/types";

import { decode, encode } from "@msgpack/msgpack";
import type { NostrEvent } from "nostr-tools";
import type { SubscribeKind } from "src/types";

import RustWorker from "@candypoets/rust-worker/worker.js?worker";

// Re-export types for external use
export type { Request, ConnectionStatus };


// Callback for subscription events
export type SubscriptionCallback = (
  data: ParsedEvent<AnyKind>[] | number,
  type: SubscribeKind,
) => void;


export interface SubscriptionOptions {
  pipeline?: PipelineConfig;
  closeOnEose?: boolean;
  cacheFirst?: boolean;
  timeoutMs?: number;
  maxEvents?: number;
  enableOptimization?: boolean;
  skipCache?: boolean;
  force?: boolean;
  bytesPerEvent?: number;
  initialMessage?: WorkerToMainMessage;
}

/**
 * Configuration for the Nostr Manager
 */
export interface NostrManagerConfig {
  /**
   * Custom worker URL. If not provided, uses the bundled worker.
   */
  workerUrl?: string;
  /**
   * Custom worker instance. If provided, workerUrl is ignored.
   */
  worker?: Worker;
}

// const wasmReady = init(mainWasmUrl);

/**
 * Pure TypeScript NostrClient that manages worker communication and state.
 * Uses WASM utilities for heavy lifting (encoding, decoding, crypto).
 */
export class NostrManager {
  private worker: Worker;
  private subscriptions = new Map<
    string,
    {
      buffer: SharedArrayBuffer;
      options: SubscriptionOptions;
      refCount: number;
    }
  >();
  private publishes = new Map<string, {buffer: SharedArrayBuffer}>();
  private signers = new Map<string, string>(); // name -> secret key hex

  public PERPETUAL_SUBSCRIPTIONS = ["notifications", "starterpack"];

  constructor(config: NostrManagerConfig = {}) {
    this.worker = this.createWorker(config);
    this.setupWorkerListener();
  }

  private createWorker(config: NostrManagerConfig): Worker {
    return new RustWorker();
  }

  private setupWorkerListener() {
    this.worker.onmessage = async (event) => {
      // await wasmReady;
      if (event.data instanceof Uint8Array) {
        let uint8Array = event.data;
        try {
          const message: any = decode(uint8Array);
          this.handleWorkerMessage(message);
        } catch (error) {
          console.error("Failed to decode worker message:", error);
        } finally {
          // Aggressively clear memory references
          if (uint8Array) {
            uint8Array.fill(0);
            (uint8Array as any) = null;
          }
        }
      } else {
        console.log("Received non-arrayBuffer message:", event.data);
      }
    };

    this.worker.onerror = (error) => {
      console.error("Worker error:", error);
    };
  }

  private handleWorkerMessage(message: WorkerToMainMessage) {
    if ("Count" in message) {
      // this.handleSubscriptionCount(message.Count.subscription_id, message.Count.count);
    } else if ("SignedEvent" in message) {
      this.handleSignedEvent(
        message.SignedEvent.content,
        message.SignedEvent.signed_event,
      );
    } else if ("PublicKey" in message) {
      this.handlePublicKey(message.PublicKey.public_key);
    } else {
      console.warn("Unknown message type from worker:", message);
    }
  }

  private handleSignedEvent(content: string, signedEvent: any) {
    console.log("Signed event received:", content, signedEvent);
  }

  private handlePublicKey(publicKey: string) {
    console.log("Public key received:", publicKey);
  }

  private createShortId(input: string): string {
    let hash = 0;
    for (let i = 0; i < input.length; i++) {
      const char = input.charCodeAt(i);
      hash = (hash << 5) - hash + char;
      hash = hash & hash;
    }
    const shortId = Math.abs(hash).toString(36);
    return shortId.substring(0, 63);
  }

  subscribe(
    subscriptionId: string,
    requests: Request[],
    options: SubscriptionOptions = {},
  ): SharedArrayBuffer {
    const subId =
      subscriptionId.length < 64
        ? subscriptionId
        : this.createShortId(subscriptionId);

    const existingSubscription = this.subscriptions.get(subId);

    if (existingSubscription) {
      existingSubscription.refCount++;
      return existingSubscription.buffer;
    }

    const defaultOptions: SubscriptionOptions = {
      closeOnEose: false,
      cacheFirst: true,
      skipCache: false,
      force: false,
      enableOptimization: true,
      ...options,
    };

    const totalLimit = requests.reduce(
      (sum, req) => sum + (req.limit || 100),
      0,
    );

    const bufferSize = SharedBufferReader.calculateBufferSize(
      totalLimit,
      options.bytesPerEvent,
    );

    let initialMessage: Uint8Array<ArrayBufferLike> = new Uint8Array();

    if(options.initialMessage) {
      initialMessage = encode(options.initialMessage);
    }

    const buffer = new SharedArrayBuffer(bufferSize + initialMessage.length);

    // Initialize the buffer (sets write position to 4)
    SharedBufferReader.initializeBuffer(buffer);

    // Write the initial message if provided
    if(initialMessage.length > 0) {
      const success = SharedBufferReader.writeMessage(buffer, initialMessage);
      if (!success) {
        console.error("Failed to write initial message to buffer");
      }
    }

    this.subscriptions.set(subId, {
      buffer,
      options: defaultOptions,
      refCount: 1,
    });

    // Convert SubscriptionOptions to SubscriptionConfig for the worker
    const config: SubscriptionConfig = {
      pipeline: defaultOptions.pipeline,
      closeOnEose: defaultOptions.closeOnEose,
      cacheFirst: defaultOptions.cacheFirst,
      timeoutMs: defaultOptions.timeoutMs,
      maxEvents: defaultOptions.maxEvents,
      enableOptimization: defaultOptions.enableOptimization,
      skipCache: defaultOptions.skipCache,
      force: defaultOptions.force,
      bytesPerEvent: defaultOptions.bytesPerEvent,
    };

    const message: MainToWorkerMessage = {
      Subscribe: {
        subscription_id: subId,
        requests: requests,
        config: config,
      },
    };

    try {
      const pack = encode(message);
      this.worker.postMessage({
        serializedMessage: pack,
        sharedBuffer: buffer,
      });

      return buffer;
    } catch (error) {
      this.subscriptions.delete(subId);
      throw error;
    }
  }

  getBuffer(subId: string): SharedArrayBuffer | undefined {
    const existingSubscription = this.subscriptions.get(subId);
    if (existingSubscription) {
      existingSubscription.refCount++;
      return existingSubscription.buffer;
    }
    return undefined;
  }

  unsubscribe(subscriptionId: string): void {
    const subId =
      subscriptionId.length < 64
        ? subscriptionId
        : this.createShortId(subscriptionId);
    const subscription = this.subscriptions.get(subId);
    if (subscription) {
      subscription.refCount--;
    }
  }

  publish(publish_id: string, event: NostrEvent): SharedArrayBuffer {

    // a publish buffer fit in 3kb
    const buffer = new SharedArrayBuffer(3072);

    // Initialize the buffer (sets write position to 4)
    SharedBufferReader.initializeBuffer(buffer);

    try {
      const template = {
        kind: event.kind,
        content: event.content,
        tags: event.tags || [],
      };

      const message: MainToWorkerMessage = {
        Publish: {
          publish_id: publish_id,
          template,
        },
      };

      const p = encode(message);

      this.worker.postMessage({ serializedMessage: p, sharedBuffer: buffer });

      this.publishes.set(publish_id, {buffer});
      return buffer;
    } catch (error) {
      console.error("Failed to publish event:", error);
      throw error;
    }
  }

  setSigner(name: string, secretKeyHex: string): void {
    const message: MainToWorkerMessage = {
      SetSigner: {
        signer_type: name,
        private_key: secretKeyHex,
      },
    };

    const pack = encode(message);
    this.worker.postMessage(pack);
    this.signers.set(name, secretKeyHex);
  }

  signEvent(event: NostrEvent) {
    const template = {
      kind: event.kind,
      content: event.content,
      tags: event.tags,
    };

    const message: MainToWorkerMessage = {
      SignEvent: {
        template: template,
      },
    };
    const pack = encode(message);
    this.worker.postMessage(pack);
  }

  getPublicKey() {
    const message: MainToWorkerMessage = {
      GetPublicKey: {},
    };
    const pack = encode(message);
    this.worker.postMessage(pack);
  }

  cleanup(): void {
    const subscriptionsToDelete: string[] = [];

    for (const [subId, subscription] of this.subscriptions.entries()) {
      if (
        subscription.refCount <= 0 &&
        !this.PERPETUAL_SUBSCRIPTIONS.includes(subId)
      ) {
        subscriptionsToDelete.push(subId);
      }
    }

    for (const subId of subscriptionsToDelete) {
      const subscription = this.subscriptions.get(subId);
      if (subscription) {
        const message: MainToWorkerMessage = {
          Unsubscribe: {
            subscription_id: subId,
          },
        };
        const pack = encode(message);
        this.worker.postMessage(pack);
        this.subscriptions.delete(subId);
      }
    }
  }
}

/**
 * Factory function to create a new NostrManager instance.
 * @param config - Configuration for the NostrManager.
 * @returns A new instance of NostrManager.
 */
export function createNostrManager(
  config: NostrManagerConfig = {},
): NostrManager {
  return new NostrManager(config);
}

/**
 * Default singleton instance of the NostrManager.
 * Useful for applications that only need one instance.
 */
export const nostrManager = new NostrManager();

export function cleanup(): void {
  nostrManager.cleanup();
}

export * from "./types";
