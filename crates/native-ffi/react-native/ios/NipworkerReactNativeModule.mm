#import "NipworkerReactNativeModule.h"

@interface NipworkerReactNativeModule ()
@property (nonatomic, assign) void* engineHandle;
@property (nonatomic, assign) BOOL hasListeners;
@end

extern "C" {
void* nipworker_init(void (*callback)(void* userdata, const uint8_t* ptr, size_t len), void* userdata);
void nipworker_handle_message(void* handle, const uint8_t* ptr, size_t len);
void nipworker_set_private_key(void* handle, const char* ptr);
void nipworker_deinit(void* handle);
void nipworker_free_bytes(uint8_t* ptr, size_t len);
}

static NSString * const NipworkerEventName = @"NipworkerEvent";
static NSString * const NipworkerStoragePrefix = @"nipworker.";

static void NipworkerReactNativeCallbackForwarder(void* userdata, const uint8_t* ptr, size_t len) {
	NipworkerReactNativeModule* module = (__bridge NipworkerReactNativeModule*)userdata;
	NSData* data = [NSData dataWithBytes:ptr length:len];
	nipworker_free_bytes((uint8_t*)ptr, len);

	NSMutableArray<NSNumber*>* bytes = [NSMutableArray arrayWithCapacity:len];
	const uint8_t* rawBytes = (const uint8_t*)data.bytes;
	for (NSUInteger i = 0; i < len; i++) {
		[bytes addObject:@(rawBytes[i])];
	}

	dispatch_async(dispatch_get_main_queue(), ^{
		if (!module.hasListeners) {
			return;
		}
		[module sendEventWithName:NipworkerEventName
							 body:@{
								 @"v": @1,
								 @"encoding": @"bytes",
								 @"data": bytes
							 }];
	});
}

@implementation NipworkerReactNativeModule

RCT_EXPORT_MODULE(NipworkerReactNativeModule)

+ (BOOL)requiresMainQueueSetup {
	return NO;
}

- (NSArray<NSString *> *)supportedEvents {
	return @[NipworkerEventName];
}

- (void)startObserving {
	self.hasListeners = YES;
}

- (void)stopObserving {
	self.hasListeners = NO;
}

RCT_REMAP_METHOD(init, initEngine) {
	if (!self.engineHandle) {
		self.engineHandle = nipworker_init(NipworkerReactNativeCallbackForwarder, (__bridge void*)self);
	}
}

RCT_EXPORT_METHOD(handleMessage:(NSArray<NSNumber *> *)bytes) {
	if (self.engineHandle && bytes) {
		NSMutableData* data = [NSMutableData dataWithLength:bytes.count];
		uint8_t* rawBytes = (uint8_t*)data.mutableBytes;
		for (NSUInteger i = 0; i < bytes.count; i++) {
			rawBytes[i] = (uint8_t)(bytes[i].unsignedCharValue);
		}
		nipworker_handle_message(self.engineHandle, (const uint8_t*)data.bytes, data.length);
	}
}

RCT_EXPORT_METHOD(setPrivateKey:(NSString *)secret) {
	if (self.engineHandle && secret) {
		nipworker_set_private_key(self.engineHandle, [secret UTF8String]);
	}
}

RCT_EXPORT_BLOCKING_SYNCHRONOUS_METHOD(getStorageItem:(NSString *)key) {
	if (!key) {
		return (id)kCFNull;
	}
	NSString *storageKey = [NipworkerStoragePrefix stringByAppendingString:key];
	NSString *value = [[NSUserDefaults standardUserDefaults] stringForKey:storageKey];
	return value ?: (id)kCFNull;
}

RCT_EXPORT_BLOCKING_SYNCHRONOUS_METHOD(setStorageItem:(NSString *)key value:(NSString *)value) {
	if (!key || !value) {
		return @NO;
	}
	NSString *storageKey = [NipworkerStoragePrefix stringByAppendingString:key];
	[[NSUserDefaults standardUserDefaults] setObject:value forKey:storageKey];
	return @([[NSUserDefaults standardUserDefaults] synchronize]);
}

RCT_EXPORT_BLOCKING_SYNCHRONOUS_METHOD(removeStorageItem:(NSString *)key) {
	if (!key) {
		return @NO;
	}
	NSString *storageKey = [NipworkerStoragePrefix stringByAppendingString:key];
	[[NSUserDefaults standardUserDefaults] removeObjectForKey:storageKey];
	return @([[NSUserDefaults standardUserDefaults] synchronize]);
}

RCT_REMAP_METHOD(deinit, deinitEngine) {
	if (self.engineHandle) {
		nipworker_deinit(self.engineHandle);
		self.engineHandle = NULL;
	}
}

- (void)invalidate {
	[self deinitEngine];
}

@end
