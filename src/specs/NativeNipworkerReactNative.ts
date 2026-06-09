import type { CodegenTypes, TurboModule } from 'react-native';
import { TurboModuleRegistry } from 'react-native';

export interface Spec extends TurboModule {
	initEngine(defaultRelays: Array<string>, indexerRelays: Array<string>): void;
	handleMessage(bytes: Array<number>): void;
	installByteRuntime(): boolean;
	wake(): void;
	setPrivateKey(secret: string): void;
	getStorageItem(key: string): string | null;
	setStorageItem(key: string, value: string): boolean;
	removeStorageItem(key: string): boolean;
	deinitEngine(): void;

	readonly onData: CodegenTypes.EventEmitter<Readonly<{ data: Array<number> }>>;
}

export default TurboModuleRegistry.get<Spec>('NipworkerReactNativeModule');
