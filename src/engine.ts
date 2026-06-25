import type { NostrManagerConfig } from 'src/types';
import { EngineManager } from './EngineManager';
import { getManager, setManager, setGlobalManager, NostrManagerLike } from './manager';

export * from './lib/NostrUtils';
export * from './types';
export { getManager, setManager, setGlobalManager } from './manager';
export type { NostrManagerLike } from './manager';

export { EngineManager } from './EngineManager';

/**
 * Create the single-worker WASM backend.
 */
export function createNostrManager(config?: NostrManagerConfig): NostrManagerLike {
	return new EngineManager(config);
}
