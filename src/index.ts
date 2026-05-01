import type { NostrManagerConfig } from 'src/types';
import { EngineManager } from './EngineManager';
import { NostrManager } from './NostrManager';
import { getManager, setManager, setGlobalManager, NostrManagerLike } from './manager';

export * from './lib/NostrUtils';
export * from './types';
export { getManager, setManager, setGlobalManager } from './manager';
export type { NostrManagerLike } from './manager';

export { EngineManager } from './EngineManager';
export { NostrManager } from './NostrManager';

/**
 * Create the appropriate web backend.
 *
 * Native/Lynx builds should import from `@candypoets/nipworker/native`.
 */
export function createNostrManager(config?: NostrManagerConfig): NostrManagerLike {
	if (config?.engine) {
		return new EngineManager(config);
	}
	return new NostrManager(config);
}
