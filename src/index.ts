import type { NostrManagerConfig } from 'src/types';
import { EngineManager } from './EngineManager';
import { NostrManager } from './NostrManager';
import type { NostrManagerLike } from './manager';

export * from './lib/NostrUtils';
export * from './types';
export * from './generated/nostr/fb';
export { getManager, setManager, setGlobalManager } from './manager';
export type { NostrManagerLike } from './manager';

export { EngineManager } from './EngineManager';
export { NostrManager } from './NostrManager';

/**
 * Create the appropriate web backend.
 *
 * React Native builds should import from `@candypoets/nipworker/react-native`.
 */
export function createNostrManager(config?: NostrManagerConfig): NostrManagerLike {
	if (config?.engine) {
		return new EngineManager(config);
	}
	return new NostrManager(config);
}
