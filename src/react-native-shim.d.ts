declare module 'react-native' {
	export type EventSubscription = { remove: () => void };
	export type TurboModule = Record<string, unknown>;
	export namespace CodegenTypes {
		type EventEmitter<T> = (listener: (event: T) => void) => EventSubscription;
	}

	export const NativeModules: Record<string, any>;
	export type AppStateStatus = 'active' | 'background' | 'inactive' | 'unknown' | 'extension';
	export const AppState: {
		currentState: AppStateStatus;
		addEventListener(
			type: 'change',
			listener: (state: AppStateStatus) => void
		): EventSubscription;
	};
	export const TurboModuleRegistry: {
		get<T>(name: string): T | null;
		getEnforcing<T>(name: string): T;
	};

	export class NativeEventEmitter {
		constructor(nativeModule?: any);
		addListener(eventType: string, listener: (event: any) => void): { remove: () => void };
		removeListener(eventType: string, listener: (event: any) => void): void;
	}
}
