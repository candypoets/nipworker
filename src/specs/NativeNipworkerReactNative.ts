import type { CodegenTypes, TurboModule } from 'react-native';
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
	getStorageItem(key: string): string | null;
	setStorageItem(key: string, value: string): boolean;
	removeStorageItem(key: string): boolean;
	deinitEngine(): void;

	readonly onData: CodegenTypes.EventEmitter<
		Readonly<{
			v: number;
			encoding: string;
			data?: Array<number>;
		}>
	>;
}

export default TurboModuleRegistry.get<Spec>('NipworkerReactNativeModule');
