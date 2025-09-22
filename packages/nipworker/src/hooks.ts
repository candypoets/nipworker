import {  NostrEvent } from "nostr-tools";
import { RequestObject, type SubscriptionConfig, type NostrManager, nostrManager } from ".";
import { SharedBufferReader } from "src/lib/SharedBuffer";
import { WorkerMessage } from "./generated/nostr/fb";

export function useSubscription(
  subId: string,
  requests: RequestObject[],
  callback: (message: WorkerMessage) => void = () => {},
  options: SubscriptionConfig = { closeOnEose: false },
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

  manager.ready.then(() => {
    buffer = manager.subscribe(subId, requests, options)
    hasSubscribed = true;
  });


  const processEvents = (): void => {
    if (!running || !buffer) return;
    let result = SharedBufferReader.readMessages(buffer, lastReadPos);
    while (result.hasNewData) {
      for (const message of result.messages) {
        callback(message)
      }
      lastReadPos = result.newReadPosition;
      result = SharedBufferReader.readMessages(buffer, lastReadPos);
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
  options: { trackStatus?: boolean } = { trackStatus: true },
  manager: NostrManager = nostrManager
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
    manager.removeEventListener(`publish:${pubId}`, processEvents);
  };

  manager.ready.then(() => {
    buffer = manager.publish(pubId, event);
  });

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
      manager.addEventListener(`publish:${pubId}`, processEvents);
      queueMicrotask(processEvents);
    }

  return unsubscribe;
}
