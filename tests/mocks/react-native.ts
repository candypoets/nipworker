export type AppStateStatus = 'active' | 'background' | 'inactive' | 'unknown' | 'extension';

export interface TurboModule {}

export const AppState = {
	currentState: 'active' as AppStateStatus,
	addEventListener: () => ({ remove() {} })
};

export const TurboModuleRegistry = {
	get: () => null
};
