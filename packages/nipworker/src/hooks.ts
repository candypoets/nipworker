import {  NostrEvent } from "nostr-tools";
import { nostrManager, SubscriptionOptions, type NostrManager } from ".";
import { SharedBufferReader } from "src/lib/SharedBuffer";
import type { RequestObject } from "src/types";
import { WorkerMessage } from "./generated/nostr/fb";

export function useSubscription(
  subId: string,
  requests: RequestObject[],
  callback: (message: WorkerMessage) => void = () => {},
  options: SubscriptionOptions = { closeOnEose: false },
  manager: NostrManager = nostrManager
): () => void {
  if (!subId) {
    console.warn("useSharedSubscription: No subscription ID provided");
    return () => {};
  }
  let buffer: SharedArrayBuffer | null = null;
  let lastReadPos: number = 4;
  let timeoutId: number | null = null;
  let pollInterval: number = 15; // Start at 5ms - very aggressive
  const maxInterval: number = 4000; // Max 4 seconds
  let running: boolean = true;

  let hasUnsubscribed = false;
  let hasSubscribed = false;

  const unsubscribe = (): void => {
    running = false;
    if (timeoutId !== null) {
      clearTimeout(timeoutId);
    }
    if (hasSubscribed && !hasUnsubscribed) {
      manager.unsubscribe(subId);
      hasUnsubscribed = true;
    }
  };

  buffer = manager.subscribe(subId, requests, options);

  hasSubscribed = true;

  const processEvents = (): void => {
    if (!running || !buffer) {
      if (timeoutId !== null) {
        clearTimeout(timeoutId);
      }
      return;
    }
    const result = SharedBufferReader.readMessages(buffer, lastReadPos);

    if (result.hasNewData) {
      // Found new data - reset to aggressive polling
      pollInterval = 32;


      for (const message of result.messages) {
        callback(message)
      }

      lastReadPos = result.newReadPosition;
    } else {
      // No new data - back off exponentially (faster backoff)
      pollInterval = Math.min(pollInterval * 2, maxInterval);
    }

    // Clear any existing timeout before scheduling a new one
    if (timeoutId !== null) {
      clearTimeout(timeoutId);
    }

    // Schedule next poll
    timeoutId = window.setTimeout(processEvents, pollInterval);
  };

  // Start after a minimal delay to ensure the return function is available
  timeoutId = window.setTimeout(processEvents, 0);

  return unsubscribe
}


export function usePublish(
  pubId: string,
  event: NostrEvent,
  callback: any = () => {},
  options: { trackStatus?: boolean } = { trackStatus: true }
): () => void {
  if (!pubId) {
    console.warn("usePublish: No publish ID provided");
    return () => {};
  }

  let buffer: SharedArrayBuffer | null = null;
  let lastReadPos: number = 4;
  let timeoutId: number | null = null;
  let running = true;

  const unsubscribe = (): void => {
    running = false;
    if (timeoutId !== null) {
      clearTimeout(timeoutId);
    }
  };

  buffer = nostrManager.publish(pubId, event);

  if (options.trackStatus && buffer) {
    const poll = (): void => {
      if (!running || !buffer) {
        if (timeoutId !== null) {
          clearTimeout(timeoutId);
        }
        return;
      }
      const result = SharedBufferReader.readMessages(buffer, lastReadPos);
      if (result.hasNewData) {
        result.messages.forEach((message: WorkerMessage) => {
            callback(message);
        });
        lastReadPos = result.newReadPosition;
      }
      timeoutId = window.setTimeout(poll, 50);
    };
    timeoutId = window.setTimeout(poll, 0);
  }

  return unsubscribe;
}
