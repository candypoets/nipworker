import type { NostrManagerConfig } from 'src/types';
import { NostrManager } from './NostrManager';
import { getManager, setManager, setGlobalManager, NostrManagerLike } from './manager';

export * from './lib/NostrUtils';
export * from './types';
export { getManager, setManager, setGlobalManager } from './manager';
export type { NostrManagerLike } from './manager';

export { NostrManager } from './NostrManager';

/**
 * Create the legacy four-worker WASM backend.
 */
export function createNostrManager(config?: NostrManagerConfig): NostrManagerLike {
	return new NostrManager(config);
}
