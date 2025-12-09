import {
	CounterPipeConfig,
	CounterPipeConfigT,
	KindFilterPipeConfig,
	KindFilterPipeConfigT,
	NpubLimiterPipeConfig,
	NpubLimiterPipeConfigT,
	ParsePipeConfig,
	ParsePipeConfigT,
	Pipe,
	PipeConfig,
	PipelineConfigT,
	PipeT,
	SaveToDbPipeConfig,
	SaveToDbPipeConfigT,
	SerializeEventsPipeConfig,
	SerializeEventsPipeConfigT
} from '../generated/nostr/fb';

export * from '../generated/nostr/fb';

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
	noCache?: boolean;
	maxRelays?: number;
};

// export type PipeConfig = {
//   name: string;
//   params?: Record<string, any>;
// };

export type EventTemplate = {
	kind: number;
	content: string;
	tags: string[][];
};

export type SubscriptionConfig = {
	pipeline?: PipeT[];
	closeOnEose?: boolean;
	cacheFirst?: boolean;
	timeoutMs?: number;
	maxEvents?: number;
	enableOptimization?: boolean;
	skipCache?: boolean;
	force?: boolean;
	bytesPerEvent?: number;
	isSlow?: boolean;
};
