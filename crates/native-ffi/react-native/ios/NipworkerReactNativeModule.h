#import <React/RCTBridgeModule.h>
#import <ReactCodegen/NipworkerReactNativeSpec/NipworkerReactNativeSpec.h>
#import <ReactCommon/RCTTurboModuleWithJSIBindings.h>

NS_ASSUME_NONNULL_BEGIN

FOUNDATION_EXPORT NSNotificationName const NipworkerRuntimeDataNotification;
FOUNDATION_EXPORT NSString * const NipworkerRuntimeDataKey;
FOUNDATION_EXPORT void *nipworker_react_native_shared_handle(void);

@interface NipworkerRuntime : NSObject
+ (void *)sharedHandle;
+ (void *)sharedHandleWithDefaultRelays:(NSArray<NSString *> *)defaultRelays
                          indexerRelays:(NSArray<NSString *> *)indexerRelays
                               userdata:(void *)userdata;
+ (void)handleMessage:(NSData *)data;
+ (void)setPrivateKey:(NSString *)secret;
+ (void)wake;
@end

@interface NipworkerReactNativeModule : NativeNipworkerReactNativeSpecBase <
	NativeNipworkerReactNativeSpec,
	RCTTurboModuleWithJSIBindings
>
@end

NS_ASSUME_NONNULL_END
