//
//  NipworkerLynxModule.mm
//  Lynx native module wrapper for libnipworker_native_ffi.
//
//  The host app must link libnipworker_native_ffi.a (iOS static library)
//  built from the Rust native-ffi crate.
//
//  Registration:
//    [globalConfig registerModule:NipworkerLynxModule.class];
//

#import <Foundation/Foundation.h>
#import <Lynx/LynxModule.h>

extern "C" {
void* nipworker_init(void (*callback)(const uint8_t* ptr, size_t len));
void nipworker_handle_message(void* handle, const uint8_t* ptr, size_t len);
void nipworker_set_private_key(void* handle, const char* ptr);
void nipworker_deinit(void* handle);
void nipworker_free_bytes(uint8_t* ptr, size_t len);
}

static void (^_gCallbackBlock)(NSData*) = nil;

static void NipworkerCallbackForwarder(const uint8_t* ptr, size_t len) {
	NSData* data = [NSData dataWithBytes:ptr length:len];
	nipworker_free_bytes(const_cast<uint8_t*>(ptr), len);
	if (_gCallbackBlock) {
		_gCallbackBlock(data);
	}
}

@interface NipworkerLynxModule : NSObject <LynxModule>
{
	void* _engineHandle;
}
@end

@implementation NipworkerLynxModule

+ (NSString *)name {
	return @"NipworkerLynxModule";
}

+ (NSDictionary<NSString *, NSString *> *)methodLookup {
	return @{
		@"init": @"init:",
		@"handleMessage": @"handleMessage:",
		@"setPrivateKey": @"setPrivateKey:",
		@"deinit": @"deinitModule"
	};
}

- (void)init:(void (^)(NSData *))callback {
	_gCallbackBlock = [callback copy];
	_engineHandle = nipworker_init(NipworkerCallbackForwarder);
}

- (void)handleMessage:(NSData *)data {
	if (_engineHandle && data) {
		nipworker_handle_message(_engineHandle, (const uint8_t*)data.bytes, data.length);
	}
}

- (void)setPrivateKey:(NSString *)secret {
	if (_engineHandle && secret) {
		nipworker_set_private_key(_engineHandle, [secret UTF8String]);
	}
}

- (void)deinitModule {
	if (_engineHandle) {
		nipworker_deinit(_engineHandle);
		_engineHandle = NULL;
	}
	_gCallbackBlock = nil;
}

@end
