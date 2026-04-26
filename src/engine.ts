import type { NostrManagerConfig } from 'src/types';
import { EngineManager } from './EngineManager';
import { NativeBackend } from './NativeBackend';
import {
	getManager,
	setManager,
	setGlobalManager,
	NostrManagerLike,
	hasLynxNativeModule,
	hasNativeModule
} from './manager';

export * from './lib/NostrUtils';
export * from './types';
export {
	getManager,
	setManager,
	setGlobalManager,
	hasLynxNativeModule,
	hasNativeModule
} from './manager';
export type { NostrManagerLike } from './manager';

export { EngineManager } from './EngineManager';
export { NativeBackend } from './NativeBackend';

/**
 * Create the appropriate backend for the current runtime environment.
 *
 * Detection order:
 * 1. LynxJS native module available -> NativeBackend
 * 2. Otherwise                     -> EngineManager (single WASM worker)
 */
export function createNostrManager(config?: NostrManagerConfig): NostrManagerLike {
	if (hasLynxNativeModule()) {
		return new NativeBackend(config);
	}
	return new EngineManager(config);
}
