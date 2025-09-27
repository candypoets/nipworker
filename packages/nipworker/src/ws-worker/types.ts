export enum ConnectionStatus {
  Idle = 'idle',
  Connecting = 'connecting',
  Ready = 'ready',
  Closing = 'closing',
  Closed = 'closed',
  Error = 'error',
}

export enum MsgKind {
  Unknown = 0,
  Event = 1,
  Eose = 2,
  Ok = 3,
  Closed = 4,
  Notice = 5,
  Auth = 6,
}

  // MsgKind is defined in FlatBuffers schema; import from generated code
  // export type { MsgKind } from '../fb/worker_messages_generated';

export interface RelayConfig {
  connectTimeoutMs?: number;
  writeTimeoutMs?: number;
  retryBaseMs?: number;
  retryMaxMs?: number;
  retryMultiplier?: number;
  retryJitter?: number;
  retry?: {
    baseMs: number;
    maxMs: number;
    multiplier: number;
    jitter: number;
  };
  idleTimeoutMs?: number;
}

export interface RelayStats {
  sent: number;
  received: number;
  reconnects: number;
  lastActivity: number; // timestamp
  uptime?: number;
  dropped: number; // for ring buffer overwrites
}

export interface InboundEnvelope {
  relays: string[];
  frames: string[];
}

export interface WorkerLine {
  relay: {
    url: string;
  };
  kind: number; // MsgKind from generated
  sub_id?: string;
  raw: Uint8Array; // UTF-8 bytes
}

export type FrameCallback = (frame: string) => void;
export type MessageHandler = (url: string, kind: MsgKind, subId: string | null, rawText: string) => void;
