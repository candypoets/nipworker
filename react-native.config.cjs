module.exports = {
	dependency: {
		platforms: {
			android: {
				sourceDir: './crates/native-ffi/react-native/android',
				packageImportPath:
					'import com.candypoets.nipworker.reactnative.NipworkerReactNativePackage;',
				packageInstance: 'new NipworkerReactNativePackage()'
			}
		}
	}
};
