import type { TurboModule } from 'react-native';
import { TurboModuleRegistry } from 'react-native';

export interface Spec extends TurboModule {
	initEngine(
		defaultRelays: Array<string>,
		indexerRelays: Array<string>,
		meshBLEEnabled: boolean,
		logLevel: string
	): void;
	handleMessage(bytes: Array<number>): void;
	installByteRuntime(): boolean;
	startMesh(): boolean;
	stopMesh(): void;
	setMeshProfile(profileJson: string): boolean;
	clearMeshProfile(): boolean;
	wake(): void;
	setPrivateKey(secret: string): void;
	clearSigner(): void;
	removeSigner(): void;
	getStorageItem(key: string): string | null;
	setStorageItem(key: string, value: string): boolean;
	removeStorageItem(key: string): boolean;
	deinitEngine(): void;
}

export default TurboModuleRegistry.get<Spec>('NipworkerReactNativeModule');
