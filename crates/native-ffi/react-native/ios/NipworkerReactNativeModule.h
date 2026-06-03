#import <React/RCTBridgeModule.h>
#import <ReactCodegen/NipworkerReactNativeSpec/NipworkerReactNativeSpec.h>
#import <ReactCommon/RCTTurboModuleWithJSIBindings.h>

@interface NipworkerReactNativeModule : NativeNipworkerReactNativeSpecBase <
	NativeNipworkerReactNativeSpec,
	RCTTurboModuleWithJSIBindings
>
@end
