export * from "../generated/nostr/fb";


export type RequestObject = {
  ids?: string[];
  authors?: string[];
  kinds?: number[];
  tags?: Record<string, string[]>;
  since?: number;
  until?: number;
  limit?: number;
  search?: string;
  relays: string[];
  closeOnEOSE?: boolean;
  cacheFirst?: boolean;
  noOptimize?: boolean;
  count?: boolean;
  noContext?: boolean;
};

export type PipeConfig = {
  name: string;
  params?: Record<string, any>;
};

export type PipelineConfig = {
  pipes: PipeConfig[];
};

export type EventTemplate = {
  kind: number;
  content: string;
  tags: string[][];
};


export type MainToWorkerMessage =
  | { Subscribe: { subscription_id: string; requests: RequestObject[]; config?: SubscriptionConfig } }
  | { Unsubscribe: { subscription_id: string } }
  | { Publish: { publish_id: string; template: EventTemplate } }
  | { SignEvent: { template: EventTemplate } }
  | { GetPublicKey: {} }
  | { SetSigner: { signer_type: string; private_key: string } }
  | { Initialize: { buffer_key: string } };


export type SubscriptionConfig = {
  pipeline?: PipelineConfig;
  closeOnEose?: boolean;
  cacheFirst?: boolean;
  timeoutMs?: number;
  maxEvents?: number;
  enableOptimization?: boolean;
  skipCache?: boolean;
  force?: boolean;
  bytesPerEvent?: number;
};
