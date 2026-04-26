import type { NostrManagerConfig } from 'src/types';
import { NostrManager } from './NostrManager';
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

export { NostrManager } from './NostrManager';
export { NativeBackend } from './NativeBackend';

/**
 * Create the appropriate backend for the current runtime environment.
 *
 * Detection order:
 * 1. LynxJS native module available -> NativeBackend
 * 2. Otherwise                     -> NostrManager (legacy 4-worker WASM)
 */
export function createNostrManager(config?: NostrManagerConfig): NostrManagerLike {
	if (hasLynxNativeModule()) {
		return new NativeBackend(config);
	}
	return new NostrManager(config);
}
