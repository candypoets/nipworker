import { nostrManager, SubscriptionOptions } from ".";
import { SharedBufferReader } from "src/lib/sharedBuffer";
import type { WorkerToMainMessage, Request } from "src/types";

export function useSubscription(
  subId: string,
  requests: Request[],
  callback: any = () => {},
  options: SubscriptionOptions = { closeOnEose: false },
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
      nostrManager.unsubscribe(subId);
      hasUnsubscribed = true;
    }
  };

  buffer = nostrManager.subscribe(subId, requests, options);

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
      pollInterval = 5;

      result.messages.forEach((message: WorkerToMainMessage) => {
        if ("SubscriptionEvent" in message) {
          if (message.SubscriptionEvent.event_type === "BUFFER_FULL" as any) {
            callback([], "BUFFER_FULL");
            unsubscribe()
          } else {
            message.SubscriptionEvent.event_data.forEach((event) => {
              callback(event, message.SubscriptionEvent.event_type);
            });
          }
        } else if ("Eose" in message) {
          if (options.closeOnEose) {
            unsubscribe();
          }
          callback(message.Eose.data, "EOSE");
        } else if ("Eoce" in message) {
          callback([], "EOCE");
        } else if ("Proofs" in message) {
          callback(message.Proofs);
        } else if ("Count" in message) {
          callback([message.Count], "Count")
        }
      });
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
