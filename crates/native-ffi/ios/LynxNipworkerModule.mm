//
//  NipworkerLynxModule.mm
//  Lynx native module wrapper for libnipworker_native_ffi.
//
//  CRITICAL: Lynx native module callbacks are ONE-SHOT. After the first
//  invocation, the callback is erased from Lynx's internal map. To receive
//  multiple events, we buffer events in pendingEvents and re-register the
//  callback from JS after each invocation.
//
//  Registration:
//    [globalConfig registerModule:NipworkerLynxModule.class];
//

#import "LynxNipworkerModule.h"

@interface NipworkerLynxModule ()
@property (atomic, copy) void (^callbackBlock)(NSData*);
@property (nonatomic, assign) void* engineHandle;
@property (nonatomic, strong) NSMutableArray<NSData*> *pendingEvents;
@property (nonatomic, strong) NSLock *pendingLock;
@end

extern "C" {
void* nipworker_init(void (*callback)(void* userdata, const uint8_t* ptr, size_t len), void* userdata);
void nipworker_handle_message(void* handle, const uint8_t* ptr, size_t len);
void nipworker_set_private_key(void* handle, const char* ptr);
void nipworker_deinit(void* handle);
void nipworker_free_bytes(uint8_t* ptr, size_t len);
}

static void NipworkerCallbackForwarder(void* userdata, const uint8_t* ptr, size_t len) {
	NipworkerLynxModule* module = (__bridge NipworkerLynxModule*)userdata;
	NSData* data = [NSData dataWithBytes:ptr length:len];
	nipworker_free_bytes((uint8_t*)ptr, len);
	
	void (^block)(NSData*) = nil;
	[module.pendingLock lock];
	block = module.callbackBlock;
	if (block) {
		module.callbackBlock = nil;
	} else {
		// No active callback — queue for later
		if (!module.pendingEvents) {
			module.pendingEvents = [NSMutableArray array];
		}
		[module.pendingEvents addObject:data];
		NSUInteger queueSize = module.pendingEvents.count;
		[module.pendingLock unlock];
		NSLog(@"[Nipworker] callbackBlock nil, queued event len=%zu (queue size=%lu)", len, (unsigned long)queueSize);
		return;
	}
	[module.pendingLock unlock];
	
	if (block) {
		// Parse subId from payload for logging: [4-byte len][subId][4-byte len][payload]
		NSString *subId = @"unknown";
		if (data.length >= 4) {
			uint32_t subIdLen = 0;
			[data getBytes:&subIdLen length:4];
			if (subIdLen > 0 && 4 + subIdLen <= data.length) {
				NSData *subIdData = [data subdataWithRange:NSMakeRange(4, subIdLen)];
				subId = [[NSString alloc] initWithData:subIdData encoding:NSUTF8StringEncoding];
			}
		}
		NSLog(@"[Nipworker] dispatching event to JS, len=%zu, subId=%@", len, subId);
		// Always dispatch to main queue to avoid calling Lynx callbacks from
		// the Rust background thread, which causes corruption on large payloads.
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
		@"deinit": @"deinitModule",
		@"testPing": @"testPing",
		@"getCallbackStatus": @"getCallbackStatus"
	};
}

- (instancetype)init {
	self = [super init];
	if (self) {
		_pendingEvents = [NSMutableArray array];
		_pendingLock = [[NSLock alloc] init];
	}
	return self;
}

- (void)initEngine:(void (^)(NSData *))callback {
	NSLog(@"[Nipworker] initEngine called");
	
	[self.pendingLock lock];
	self.callbackBlock = [callback copy];
	[self.pendingLock unlock];
	
	// Flush queued events asynchronously on the main queue to avoid deep
	// recursion when JS re-registers inside the callback handler.
	dispatch_async(dispatch_get_main_queue(), ^{
		[self flushQueuedEvents];
	});
	
	if (!self.engineHandle) {
		self.engineHandle = nipworker_init(NipworkerCallbackForwarder, (__bridge void*)self);
		NSLog(@"[Nipworker] initEngine engineHandle=%p", self.engineHandle);
	}
}

- (void)flushQueuedEvents {
	void (^block)(NSData*) = nil;
	NSData *data = nil;
	
	[self.pendingLock lock];
	if (self.pendingEvents.count > 0 && self.callbackBlock) {
		data = [self.pendingEvents firstObject];
		[self.pendingEvents removeObjectAtIndex:0];
		block = self.callbackBlock;
		self.callbackBlock = nil;
	}
	[self.pendingLock unlock];
	
	if (block && data) {
		block(data);
	}
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
	[self.pendingLock lock];
	self.callbackBlock = nil;
	[self.pendingEvents removeAllObjects];
	[self.pendingLock unlock];
}

- (void)testPing {
    NSLog(@"[Nipworker] testPing called, callbackBlock=%@", self.callbackBlock ? @"set" : @"nil");
    
    void (^block)(NSData*) = nil;
    NSData *data = nil;
    
    [self.pendingLock lock];
    block = self.callbackBlock;
    if (block) {
        self.callbackBlock = nil;
        
        NSString *subId = @"test";
        // Build a ~2000 byte payload to test large-data bridge handling
        NSMutableString *payloadBuilder = [NSMutableString stringWithString:@"hello"];
        while (payloadBuilder.length < 2000) {
            [payloadBuilder appendString:@"_this_is_a_test_payload_for_large_data_transfer_through_lynx_bridge"];
        }
        NSString *payload = [payloadBuilder substringToIndex:2000];
        
        NSData *subIdData = [subId dataUsingEncoding:NSUTF8StringEncoding];
        NSData *payloadData = [payload dataUsingEncoding:NSUTF8StringEncoding];
        
        NSMutableData *testData = [NSMutableData data];
        uint32_t subIdLen = (uint32_t)subIdData.length;
        [testData appendBytes:&subIdLen length:4];
        [testData appendData:subIdData];
        uint32_t payloadLen = (uint32_t)payloadData.length;
        [testData appendBytes:&payloadLen length:4];
        [testData appendData:payloadData];
        data = testData;
    }
    [self.pendingLock unlock];
    
    if (block && data) {
        dispatch_async(dispatch_get_main_queue(), ^{
            block(data);
        });
    }
}

- (NSString *)getCallbackStatus {
    [self.pendingLock lock];
    NSUInteger queueSize = self.pendingEvents.count;
    BOOL hasBlock = (self.callbackBlock != nil);
    [self.pendingLock unlock];
    return [NSString stringWithFormat:@"callbackBlock_%@,queue=%lu",
            hasBlock ? @"set" : @"nil", (unsigned long)queueSize];
}

@end
