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

#import "LynxNipworkerModule.h"

@interface NipworkerLynxModule ()
@property (atomic, copy) void (^callbackBlock)(NSData*);
@property (nonatomic, assign) void* engineHandle;
@end

extern "C" {
void* nipworker_init(void (*callback)(void* userdata, const uint8_t* ptr, size_t len), void* userdata);
void nipworker_handle_message(void* handle, const uint8_t* ptr, size_t len);
void nipworker_set_private_key(void* handle, const char* ptr);
void nipworker_deinit(void* handle);
void nipworker_free_bytes(uint8_t* ptr, size_t len);
}

static void NipworkerCallbackForwarder(void* userdata, const uint8_t* ptr, size_t len) {
	NSLog(@"[Nipworker] callback fired, len=%zu", len);
	NipworkerLynxModule* module = (__bridge NipworkerLynxModule*)userdata;
	NSData* data = [NSData dataWithBytes:ptr length:len];
	nipworker_free_bytes((uint8_t*)ptr, len);
	void (^block)(NSData*) = module.callbackBlock;
	if (block) {
		dispatch_async(dispatch_get_main_queue(), ^{
			block(data);
		});
	}
}

@implementation NipworkerLynxModule

+ (NSString *)name {
	return @"NipworkerLynxModule";
}

+ (NSDictionary<NSString *, NSString *> *)methodLookup {
	return @{
		@"init": @"initEngine:",
		@"handleMessage": @"handleMessage:",
		@"setPrivateKey": @"setPrivateKey:",
		@"deinit": @"deinitModule"
	};
}

- (void)initEngine:(void (^)(NSData *))callback {
	NSLog(@"[Nipworker] initEngine called");
	self.callbackBlock = [callback copy];
	self.engineHandle = nipworker_init(NipworkerCallbackForwarder, (__bridge void*)self);
	NSLog(@"[Nipworker] initEngine engineHandle=%p", self.engineHandle);
}

- (void)handleMessage:(NSData *)data {
	NSLog(@"[Nipworker] handleMessage called, engineHandle=%p, dataLength=%lu", self.engineHandle, (unsigned long)data.length);
	if (self.engineHandle && data) {
		nipworker_handle_message(self.engineHandle, (const uint8_t*)data.bytes, data.length);
	}
}

- (void)setPrivateKey:(NSString *)secret {
	if (self.engineHandle && secret) {
		nipworker_set_private_key(self.engineHandle, [secret UTF8String]);
	}
}

- (void)deinitModule {
	if (self.engineHandle) {
		nipworker_deinit(self.engineHandle);
		self.engineHandle = NULL;
	}
	self.callbackBlock = nil;
}

@end
