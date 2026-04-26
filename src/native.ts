/**
 * Native-only entry point for @candypoets/nipworker.
 *
 * This module exports ONLY the NativeBackend and shared utilities.
 * It contains NO WASM imports, NO worker URLs, and NO web-specific code.
 *
 * Use this entry point in LynxJS / React Native / other native environments
 * to avoid bundling wasm-bindgen glue code that crashes QuickJS.
 *
 * @example
 * import { createNostrManager, setManager } from '@candypoets/nipworker/native';
 * const backend = createNostrManager();
 * setManager(backend);
 */

export { NativeBackend } from './NativeBackend';
export {
	getManager,
	setManager,
	setGlobalManager,
	hasLynxNativeModule,
	hasNativeModule
} from './manager';
export type { NostrManagerLike } from './manager';
export * from './lib/NostrUtils';
export * from './types';

import { NativeBackend } from './NativeBackend';
import type { NostrManagerConfig } from './types';
import type { NostrManagerLike } from './manager';

/**
 * Create a NativeBackend instance.
 *
 * In native builds this always returns NativeBackend. If you need web WASM
 * backends, import from `@candypoets/nipworker` instead.
 */
export function createNostrManager(config?: NostrManagerConfig): NostrManagerLike {
	return new NativeBackend(config);
}
