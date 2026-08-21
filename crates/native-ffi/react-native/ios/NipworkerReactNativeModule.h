#import <Foundation/Foundation.h>

#ifdef __cplusplus
#import <React/RCTBridgeModule.h>
#import <ReactCodegen/NipworkerReactNativeSpec/NipworkerReactNativeSpec.h>
#import <ReactCommon/RCTTurboModuleWithJSIBindings.h>
#endif

NS_ASSUME_NONNULL_BEGIN

#ifdef __cplusplus
@interface NipworkerReactNativeModule : NativeNipworkerReactNativeSpecBase <
	NativeNipworkerReactNativeSpec,
	RCTTurboModuleWithJSIBindings
>
@end
#endif

NS_ASSUME_NONNULL_END
