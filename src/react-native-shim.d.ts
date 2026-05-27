declare module 'react-native' {
	export const NativeModules: Record<string, any>;

	export class NativeEventEmitter {
		constructor(nativeModule?: any);
		addListener(eventType: string, listener: (event: any) => void): { remove: () => void };
		removeListener(eventType: string, listener: (event: any) => void): void;
	}
}
