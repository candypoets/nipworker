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
  let running: boolean = true;
  let hasUnsubscribed = false;
  let hasSubscribed = false;

  const unsubscribe = (): void => {
    running = false;
    if (hasSubscribed && !hasUnsubscribed) {
      manager.removeEventListener(`subscription:${subId}`, processEvents);
      manager.unsubscribe(subId);
      hasUnsubscribed = true;
    }
  };
  subId = manager.createShortId(subId);
  buffer = manager.subscribe(subId, requests, options);

  hasSubscribed = true;

  const processEvents = (): void => {
    if (!running || !buffer) return;
    const result = SharedBufferReader.readMessages(buffer, lastReadPos);
    if (result.hasNewData) {
      for (const message of result.messages) {
        callback(message)
      }
      lastReadPos = result.newReadPosition;
    }
  };

  manager.addEventListener(`subscription:${subId}`, processEvents)

  queueMicrotask(processEvents);

  return unsubscribe
}


export function usePublish(
  pubId: string,
  event: NostrEvent,
  callback: (message: WorkerMessage) => void = () => {},
  options: { trackStatus?: boolean } = { trackStatus: true }
): () => void {
  if (!pubId) {
    console.warn("usePublish: No publish ID provided");
    return () => {};
  }

  let buffer: SharedArrayBuffer | null = null;
  let lastReadPos: number = 4;
  let running = true;

  const unsubscribe = (): void => {
    running = false;
    nostrManager.removeEventListener(`publish:${pubId}`, processEvents);
  };

  buffer = nostrManager.publish(pubId, event);

  const processEvents = (): void => {
    if (!running || !buffer) {
      return;
    }
    const result = SharedBufferReader.readMessages(buffer, lastReadPos);
    if (result.hasNewData) {
      result.messages.forEach((message: WorkerMessage) => {
          console.log(message);
          callback(message);
      });
      lastReadPos = result.newReadPosition;
    }
  };

  if (options.trackStatus) {
      nostrManager.addEventListener(`publish:${pubId}`, processEvents);
      queueMicrotask(processEvents);
    }

  return unsubscribe;
}
