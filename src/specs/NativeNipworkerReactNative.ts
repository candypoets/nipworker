import type { CodegenTypes, TurboModule } from 'react-native';
import { TurboModuleRegistry } from 'react-native';

export interface Spec extends TurboModule {
	init(): void;
	handleMessage(bytes: Array<number>): void;
	installByteRuntime(): boolean;
	setPrivateKey(secret: string): void;
	getStorageItem(key: string): string | null;
	setStorageItem(key: string, value: string): boolean;
	removeStorageItem(key: string): boolean;
	deinit(): void;

	readonly onData: CodegenTypes.EventEmitter<Readonly<{ data: Array<number> }>>;
}

export default TurboModuleRegistry.get<Spec>('NipworkerReactNative');
