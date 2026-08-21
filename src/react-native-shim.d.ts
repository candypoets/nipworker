declare module 'react-native' {
	export type EventSubscription = { remove: () => void };
	export type TurboModule = Record<string, unknown>;

	export type AppStateStatus = 'active' | 'background' | 'inactive' | 'unknown' | 'extension';
	export const AppState: {
		currentState: AppStateStatus;
		addEventListener(type: 'change', listener: (state: AppStateStatus) => void): EventSubscription;
	};
	export const TurboModuleRegistry: {
		get<T>(name: string): T | null;
		getEnforcing<T>(name: string): T;
	};
}
