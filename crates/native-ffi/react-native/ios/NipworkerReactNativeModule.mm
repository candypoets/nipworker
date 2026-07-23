#import "NipworkerReactNativeModule.h"
#if __has_include(<React/RCTBridge+Private.h>)
#import <React/RCTBridge+Private.h>
#elif __has_include(<React_Core/React/RCTBridge+Private.h>)
#import <React_Core/React/RCTBridge+Private.h>
#endif
#import <jsi/jsi.h>

#include <memory>
#include <mutex>
#include <string>
#include <utility>
#include <vector>
#include <atomic>

@interface NipworkerReactNativeModule ()
@property (nonatomic, assign) void* engineHandle;
@property (nonatomic, assign) BOOL hasListeners;
@property (nonatomic, assign) BOOL byteRuntimeInstalled;
@property (nonatomic, assign) void* byteRuntimeAddress;
- (void)emitNipworkerData:(NSDictionary *)body;
@end

extern "C" {
void* nipworker_init(void (*callback)(void* userdata, const uint8_t* ptr, size_t len), void* userdata);
void* nipworker_init_with_storage_path(void (*callback)(void* userdata, const uint8_t* ptr, size_t len), void* userdata, const char* storage_path);
void* nipworker_init_with_config(void (*callback)(void* userdata, const uint8_t* ptr, size_t len), void* userdata, const char* storage_path, const char* default_relays, const char* indexer_relays);
void* nipworker_init_with_options(void (*callback)(void* userdata, const uint8_t* ptr, size_t len), void* userdata, const char* storage_path, const char* default_relays, const char* indexer_relays, bool mesh_enabled);
void nipworker_handle_message(void* handle, const uint8_t* ptr, size_t len);
bool nipworker_subscribe_message(void* handle, const uint8_t* ptr, size_t len);
bool nipworker_publish_message(void* handle, const uint8_t* ptr, size_t len);
void nipworker_set_private_key(void* handle, const char* ptr);
void nipworker_wake(void* handle);
void nipworker_deinit(void* handle);
void nipworker_free_bytes(uint8_t* ptr, size_t len);
bool nipworker_register_subscription(void* handle, const char* sub_id, size_t buffer_size);
bool nipworker_register_publish_buffer(void* handle, const char* publish_id, size_t buffer_size);
bool nipworker_retain_subscription(void* handle, const char* sub_id);
void nipworker_release_subscription(void* handle, const char* sub_id);
uint8_t* nipworker_subscription_buffer_ptr(void* handle, const char* sub_id);
size_t nipworker_subscription_buffer_len(void* handle, const char* sub_id);
void nipworker_cleanup_subscriptions(void* handle);
bool nipworker_mesh_set_profile_json(void* handle, const char* profile_json);
bool nipworker_mesh_clear_profile(void* handle);
}

static NSString * const NipworkerEventName = @"NipworkerEvent";
static NSString * const NipworkerStoragePrefix = @"nipworker.";
static NSString * const NipworkerMeshProfileKey = @"nipworker.meshProfile";
NSNotificationName const NipworkerRuntimeDataNotification = @"NipworkerRuntimeDataNotification";
NSString * const NipworkerRuntimeDataKey = @"data";
static std::mutex NipworkerQueuedPacketsMutex;
static std::vector<std::vector<uint8_t>> NipworkerQueuedPackets;
static std::atomic_bool NipworkerByteRuntimeInstalled(false);
static void* NipworkerByteRuntimeAddress = NULL;
static void* NipworkerEngineHandle = NULL;
static NSHashTable<NipworkerReactNativeModule*>* NipworkerListenerModules;

static void NipworkerReactNativeCallbackForwarder(void* userdata, const uint8_t* ptr, size_t len);
static void* NipworkerGetEngineHandle(void* userdata, NSArray<NSString*>* defaultRelays, NSArray<NSString*>* indexerRelays, BOOL meshBLEEnabled);
static void* NipworkerGetEngineHandleDefault(void* userdata);
static void NipworkerNotifyQueuedPacket(void);
static NSString* NipworkerStorageDirectory(void);

static void NipworkerQueuePacket(const uint8_t* ptr, size_t len) {
	if (!ptr || len == 0) {
		return;
	}
	std::vector<uint8_t> packet(len);
	memcpy(packet.data(), ptr, len);
	std::lock_guard<std::mutex> lock(NipworkerQueuedPacketsMutex);
	NipworkerQueuedPackets.emplace_back(std::move(packet));
}

static std::vector<std::vector<uint8_t>> NipworkerDrainPackets() {
	std::lock_guard<std::mutex> lock(NipworkerQueuedPacketsMutex);
	std::vector<std::vector<uint8_t>> packets;
	packets.swap(NipworkerQueuedPackets);
	return packets;
}

static NSString* NipworkerRelayCSV(NSArray<NSString*>* relays) {
	NSMutableArray<NSString*>* clean = [NSMutableArray array];
	for (NSString* relay in relays ?: @[]) {
		if (![relay isKindOfClass:[NSString class]]) {
			continue;
		}
		NSString* trimmed = [relay stringByTrimmingCharactersInSet:[NSCharacterSet whitespaceAndNewlineCharacterSet]];
		if (trimmed.length > 0 && [trimmed rangeOfString:@","].location == NSNotFound) {
			[clean addObject:trimmed];
		}
	}
	return [clean componentsJoinedByString:@","];
}

static void* NipworkerGetEngineHandle(void* userdata, NSArray<NSString*>* defaultRelays, NSArray<NSString*>* indexerRelays, BOOL meshBLEEnabled) {
	@synchronized ([NipworkerReactNativeModule class]) {
		if (!NipworkerEngineHandle) {
			NSString* path = NipworkerStorageDirectory();
			NSString* defaultRelayCSV = NipworkerRelayCSV(defaultRelays);
			NSString* indexerRelayCSV = NipworkerRelayCSV(indexerRelays);
			NipworkerEngineHandle = nipworker_init_with_options(
				NipworkerReactNativeCallbackForwarder,
				userdata,
				[path UTF8String],
				[defaultRelayCSV UTF8String],
				[indexerRelayCSV UTF8String],
				meshBLEEnabled
			);
		}
		return NipworkerEngineHandle;
	}
}

static void* NipworkerGetEngineHandleDefault(void* userdata) {
	return NipworkerGetEngineHandle(userdata, @[], @[], NO);
}

static NSString* NipworkerStorageDirectory(void) {
	NSArray<NSURL*>* urls = [[NSFileManager defaultManager] URLsForDirectory:NSApplicationSupportDirectory inDomains:NSUserDomainMask];
	NSURL* baseURL = urls.firstObject;
	if (!baseURL) {
		baseURL = [NSURL fileURLWithPath:NSTemporaryDirectory()];
	}
	NSURL* dirURL = [baseURL URLByAppendingPathComponent:@"nipworker" isDirectory:YES];
	[[NSFileManager defaultManager] createDirectoryAtURL:dirURL withIntermediateDirectories:YES attributes:nil error:nil];
	return [dirURL path];
}

void *nipworker_react_native_shared_handle(void) {
	return NipworkerGetEngineHandleDefault(NULL);
}

static void NipworkerNotifyQueuedPacket(void) {
	dispatch_async(dispatch_get_main_queue(), ^{
		NSArray<NipworkerReactNativeModule*>* listeners = nil;
		@synchronized ([NipworkerReactNativeModule class]) {
			listeners = NipworkerListenerModules.allObjects;
		}
		for (NipworkerReactNativeModule* listener in listeners) {
			[listener emitNipworkerData:@{
				@"v": @1,
				@"encoding": @"queued"
			}];
		}
	});
}

class NipworkerMutableBuffer final : public facebook::jsi::MutableBuffer {
public:
	explicit NipworkerMutableBuffer(std::vector<uint8_t>&& bytes) : bytes_(std::move(bytes)) {}

	size_t size() const override {
		return bytes_.size();
	}

	uint8_t* data() override {
		return bytes_.data();
	}

private:
	std::vector<uint8_t> bytes_;
};

class NipworkerExternalMutableBuffer final : public facebook::jsi::MutableBuffer {
public:
	NipworkerExternalMutableBuffer(void* engineHandle, std::string subId, uint8_t* ptr, size_t len)
		: engineHandle_(engineHandle), subId_(std::move(subId)), ptr_(ptr), len_(len) {}

	~NipworkerExternalMutableBuffer() override {
		if (engineHandle_ && !subId_.empty()) {
			nipworker_release_subscription(engineHandle_, subId_.c_str());
		}
	}

	size_t size() const override {
		return len_;
	}

	uint8_t* data() override {
		return ptr_;
	}

private:
	void* engineHandle_;
	std::string subId_;
	uint8_t* ptr_;
	size_t len_;
};

static std::shared_ptr<NipworkerExternalMutableBuffer> NipworkerCreatePinnedBuffer(
	void* engineHandle,
	const std::string& subId,
	uint8_t* ptr,
	size_t len
) {
	if (!engineHandle || subId.empty() || !ptr || len == 0 ||
		!nipworker_retain_subscription(engineHandle, subId.c_str())) {
		return nullptr;
	}
	return std::make_shared<NipworkerExternalMutableBuffer>(engineHandle, subId, ptr, len);
}

static void NipworkerInstallByteRuntime(facebook::jsi::Runtime& runtime, void* engineHandle) {
	facebook::jsi::Object byteRuntime(runtime);
	byteRuntime.setProperty(
		runtime,
		"init",
		facebook::jsi::Function::createFromHostFunction(
			runtime,
			facebook::jsi::PropNameID::forAscii(runtime, "init"),
			0,
			[](facebook::jsi::Runtime& runtime, const facebook::jsi::Value&, const facebook::jsi::Value*, size_t) {
				return facebook::jsi::Value::undefined();
			}
		)
	);
	byteRuntime.setProperty(
		runtime,
		"handleMessage",
		facebook::jsi::Function::createFromHostFunction(
			runtime,
			facebook::jsi::PropNameID::forAscii(runtime, "handleMessage"),
			1,
			[engineHandle](facebook::jsi::Runtime& runtime, const facebook::jsi::Value&, const facebook::jsi::Value* args, size_t count) {
				if (count < 1 || !args[0].isObject() || !args[0].asObject(runtime).isArrayBuffer(runtime)) {
					return facebook::jsi::Value::undefined();
				}
				facebook::jsi::ArrayBuffer buffer = args[0].asObject(runtime).getArrayBuffer(runtime);
				nipworker_handle_message(engineHandle, buffer.data(runtime), buffer.size(runtime));
				return facebook::jsi::Value::undefined();
			}
		)
	);
	byteRuntime.setProperty(
		runtime,
		"setPrivateKey",
		facebook::jsi::Function::createFromHostFunction(
			runtime,
			facebook::jsi::PropNameID::forAscii(runtime, "setPrivateKey"),
			1,
			[engineHandle](facebook::jsi::Runtime& runtime, const facebook::jsi::Value&, const facebook::jsi::Value* args, size_t count) {
				if (count < 1 || !args[0].isString()) {
					return facebook::jsi::Value::undefined();
				}
				std::string secret = args[0].asString(runtime).utf8(runtime);
				nipworker_set_private_key(engineHandle, secret.c_str());
				return facebook::jsi::Value::undefined();
			}
		)
	);
	byteRuntime.setProperty(
		runtime,
		"wake",
		facebook::jsi::Function::createFromHostFunction(
			runtime,
			facebook::jsi::PropNameID::forAscii(runtime, "wake"),
			0,
			[engineHandle](facebook::jsi::Runtime&, const facebook::jsi::Value&, const facebook::jsi::Value*, size_t) {
				nipworker_wake(engineHandle);
				return facebook::jsi::Value::undefined();
			}
		)
	);
	byteRuntime.setProperty(
		runtime,
		"registerSubscription",
		facebook::jsi::Function::createFromHostFunction(
			runtime,
			facebook::jsi::PropNameID::forAscii(runtime, "registerSubscription"),
			2,
			[engineHandle](facebook::jsi::Runtime& runtime, const facebook::jsi::Value&, const facebook::jsi::Value* args, size_t count) {
				if (count < 2 || !args[0].isString() || !args[1].isNumber()) {
					return facebook::jsi::Value(false);
				}
				std::string subId = args[0].asString(runtime).utf8(runtime);
				auto bufferSize = static_cast<size_t>(args[1].asNumber());
				return facebook::jsi::Value(nipworker_register_subscription(engineHandle, subId.c_str(), bufferSize));
			}
		)
	);
	byteRuntime.setProperty(
		runtime,
		"registerPublishBuffer",
		facebook::jsi::Function::createFromHostFunction(
			runtime,
			facebook::jsi::PropNameID::forAscii(runtime, "registerPublishBuffer"),
			2,
			[engineHandle](facebook::jsi::Runtime& runtime, const facebook::jsi::Value&, const facebook::jsi::Value* args, size_t count) {
				if (count < 2 || !args[0].isString() || !args[1].isNumber()) {
					return facebook::jsi::Value(false);
				}
				std::string publishId = args[0].asString(runtime).utf8(runtime);
				auto bufferSize = static_cast<size_t>(args[1].asNumber());
				return facebook::jsi::Value(nipworker_register_publish_buffer(engineHandle, publishId.c_str(), bufferSize));
			}
		)
	);
	byteRuntime.setProperty(
		runtime,
		"subscribe",
		facebook::jsi::Function::createFromHostFunction(
			runtime,
			facebook::jsi::PropNameID::forAscii(runtime, "subscribe"),
			2,
			[engineHandle](facebook::jsi::Runtime& runtime, const facebook::jsi::Value&, const facebook::jsi::Value* args, size_t count) {
				if (count < 2 || !args[0].isObject() || !args[0].asObject(runtime).isArrayBuffer(runtime) || !args[1].isString()) {
					return facebook::jsi::Value::undefined();
				}
				facebook::jsi::ArrayBuffer message = args[0].asObject(runtime).getArrayBuffer(runtime);
				std::string subId = args[1].asString(runtime).utf8(runtime);
				if (!nipworker_subscribe_message(engineHandle, message.data(runtime), message.size(runtime))) {
					return facebook::jsi::Value::undefined();
				}
				uint8_t* ptr = nipworker_subscription_buffer_ptr(engineHandle, subId.c_str());
				size_t len = nipworker_subscription_buffer_len(engineHandle, subId.c_str());
				if (!ptr || len == 0) {
					return facebook::jsi::Value::undefined();
				}
				auto nativeBuffer = NipworkerCreatePinnedBuffer(engineHandle, subId, ptr, len);
				if (!nativeBuffer) {
					nipworker_release_subscription(engineHandle, subId.c_str());
					return facebook::jsi::Value::undefined();
				}
				facebook::jsi::ArrayBuffer buffer(runtime, std::move(nativeBuffer));
				return facebook::jsi::Value(runtime, std::move(buffer));
			}
		)
	);
	byteRuntime.setProperty(
		runtime,
		"publish",
		facebook::jsi::Function::createFromHostFunction(
			runtime,
			facebook::jsi::PropNameID::forAscii(runtime, "publish"),
			2,
			[engineHandle](facebook::jsi::Runtime& runtime, const facebook::jsi::Value&, const facebook::jsi::Value* args, size_t count) {
				if (count < 2 || !args[0].isObject() || !args[0].asObject(runtime).isArrayBuffer(runtime) || !args[1].isString()) {
					return facebook::jsi::Value::undefined();
				}
				facebook::jsi::ArrayBuffer message = args[0].asObject(runtime).getArrayBuffer(runtime);
				std::string publishId = args[1].asString(runtime).utf8(runtime);
				if (!nipworker_publish_message(engineHandle, message.data(runtime), message.size(runtime))) {
					return facebook::jsi::Value::undefined();
				}
				uint8_t* ptr = nipworker_subscription_buffer_ptr(engineHandle, publishId.c_str());
				size_t len = nipworker_subscription_buffer_len(engineHandle, publishId.c_str());
				if (!ptr || len == 0) {
					return facebook::jsi::Value::undefined();
				}
				auto nativeBuffer = NipworkerCreatePinnedBuffer(engineHandle, publishId, ptr, len);
				if (!nativeBuffer) {
					nipworker_release_subscription(engineHandle, publishId.c_str());
					return facebook::jsi::Value::undefined();
				}
				facebook::jsi::ArrayBuffer buffer(runtime, std::move(nativeBuffer));
				return facebook::jsi::Value(runtime, std::move(buffer));
			}
		)
	);
	byteRuntime.setProperty(
		runtime,
		"retainSubscription",
		facebook::jsi::Function::createFromHostFunction(
			runtime,
			facebook::jsi::PropNameID::forAscii(runtime, "retainSubscription"),
			1,
			[engineHandle](facebook::jsi::Runtime& runtime, const facebook::jsi::Value&, const facebook::jsi::Value* args, size_t count) {
				if (count < 1 || !args[0].isString()) {
					return facebook::jsi::Value(false);
				}
				std::string subId = args[0].asString(runtime).utf8(runtime);
				return facebook::jsi::Value(nipworker_retain_subscription(engineHandle, subId.c_str()));
			}
		)
	);
	byteRuntime.setProperty(
		runtime,
		"retainSubscriptionBuffer",
		facebook::jsi::Function::createFromHostFunction(
			runtime,
			facebook::jsi::PropNameID::forAscii(runtime, "retainSubscriptionBuffer"),
			1,
			[engineHandle](facebook::jsi::Runtime& runtime, const facebook::jsi::Value&, const facebook::jsi::Value* args, size_t count) {
				if (count < 1 || !args[0].isString()) {
					return facebook::jsi::Value::undefined();
				}
				std::string subId = args[0].asString(runtime).utf8(runtime);
				if (!nipworker_retain_subscription(engineHandle, subId.c_str())) {
					return facebook::jsi::Value::undefined();
				}
				uint8_t* ptr = nipworker_subscription_buffer_ptr(engineHandle, subId.c_str());
				size_t len = nipworker_subscription_buffer_len(engineHandle, subId.c_str());
				if (!ptr || len == 0) {
					nipworker_release_subscription(engineHandle, subId.c_str());
					return facebook::jsi::Value::undefined();
				}
				auto nativeBuffer = NipworkerCreatePinnedBuffer(engineHandle, subId, ptr, len);
				if (!nativeBuffer) {
					nipworker_release_subscription(engineHandle, subId.c_str());
					return facebook::jsi::Value::undefined();
				}
				facebook::jsi::ArrayBuffer buffer(runtime, std::move(nativeBuffer));
				return facebook::jsi::Value(runtime, std::move(buffer));
			}
		)
	);
	byteRuntime.setProperty(
		runtime,
		"releaseSubscription",
		facebook::jsi::Function::createFromHostFunction(
			runtime,
			facebook::jsi::PropNameID::forAscii(runtime, "releaseSubscription"),
			1,
			[engineHandle](facebook::jsi::Runtime& runtime, const facebook::jsi::Value&, const facebook::jsi::Value* args, size_t count) {
				if (count < 1 || !args[0].isString()) {
					return facebook::jsi::Value::undefined();
				}
				std::string subId = args[0].asString(runtime).utf8(runtime);
				nipworker_release_subscription(engineHandle, subId.c_str());
				return facebook::jsi::Value::undefined();
			}
		)
	);
	byteRuntime.setProperty(
		runtime,
		"getSubscriptionBuffer",
		facebook::jsi::Function::createFromHostFunction(
			runtime,
			facebook::jsi::PropNameID::forAscii(runtime, "getSubscriptionBuffer"),
			1,
			[engineHandle](facebook::jsi::Runtime& runtime, const facebook::jsi::Value&, const facebook::jsi::Value* args, size_t count) {
				if (count < 1 || !args[0].isString()) {
					return facebook::jsi::Value::undefined();
				}
				std::string subId = args[0].asString(runtime).utf8(runtime);
				uint8_t* ptr = nipworker_subscription_buffer_ptr(engineHandle, subId.c_str());
				size_t len = nipworker_subscription_buffer_len(engineHandle, subId.c_str());
				if (!ptr || len == 0) {
					return facebook::jsi::Value::undefined();
				}
				auto nativeBuffer = NipworkerCreatePinnedBuffer(engineHandle, subId, ptr, len);
				if (!nativeBuffer) {
					return facebook::jsi::Value::undefined();
				}
				facebook::jsi::ArrayBuffer buffer(runtime, std::move(nativeBuffer));
				return facebook::jsi::Value(runtime, std::move(buffer));
			}
		)
	);
	byteRuntime.setProperty(
		runtime,
		"cleanupSubscriptions",
		facebook::jsi::Function::createFromHostFunction(
			runtime,
			facebook::jsi::PropNameID::forAscii(runtime, "cleanupSubscriptions"),
			0,
			[engineHandle](facebook::jsi::Runtime&, const facebook::jsi::Value&, const facebook::jsi::Value*, size_t) {
				nipworker_cleanup_subscriptions(engineHandle);
				return facebook::jsi::Value::undefined();
			}
		)
	);
	byteRuntime.setProperty(
		runtime,
		"deinit",
		facebook::jsi::Function::createFromHostFunction(
			runtime,
			facebook::jsi::PropNameID::forAscii(runtime, "deinit"),
			0,
			[engineHandle](facebook::jsi::Runtime&, const facebook::jsi::Value&, const facebook::jsi::Value*, size_t) {
				nipworker_deinit(engineHandle);
				return facebook::jsi::Value::undefined();
			}
		)
	);
	byteRuntime.setProperty(
		runtime,
		"drain",
		facebook::jsi::Function::createFromHostFunction(
			runtime,
			facebook::jsi::PropNameID::forAscii(runtime, "drain"),
			0,
			[](facebook::jsi::Runtime& runtime, const facebook::jsi::Value&, const facebook::jsi::Value*, size_t) {
				auto packets = NipworkerDrainPackets();
				facebook::jsi::Array output(runtime, packets.size());
				for (size_t i = 0; i < packets.size(); i++) {
					auto nativeBuffer = std::make_shared<NipworkerMutableBuffer>(std::move(packets[i]));
					facebook::jsi::ArrayBuffer buffer(runtime, std::move(nativeBuffer));
					output.setValueAtIndex(runtime, i, std::move(buffer));
				}
				return output;
			}
		)
	);

	runtime.global().setProperty(
		runtime,
		"__nipworkerReactNativeByteRuntime",
		std::move(byteRuntime)
	);
}

static void NipworkerReactNativeCallbackForwarder(void* userdata, const uint8_t* ptr, size_t len) {
	NipworkerReactNativeModule* module = (__bridge NipworkerReactNativeModule*)userdata;
	NSData* data = [NSData dataWithBytes:ptr length:len];
	nipworker_free_bytes((uint8_t*)ptr, len);

	dispatch_async(dispatch_get_main_queue(), ^{
		[[NSNotificationCenter defaultCenter] postNotificationName:NipworkerRuntimeDataNotification
															object:nil
														  userInfo:@{ NipworkerRuntimeDataKey: data }];
	});

	if (module.byteRuntimeInstalled || NipworkerByteRuntimeInstalled.load()) {
		NipworkerQueuePacket((const uint8_t*)data.bytes, data.length);
		NipworkerNotifyQueuedPacket();
		return;
	}

	NSMutableArray<NSNumber*>* bytes = [NSMutableArray arrayWithCapacity:len];
	const uint8_t* rawBytes = (const uint8_t*)data.bytes;
	for (NSUInteger i = 0; i < len; i++) {
		[bytes addObject:@(rawBytes[i])];
	}

	dispatch_async(dispatch_get_main_queue(), ^{
		[module emitNipworkerData:@{
			@"v": @1,
			@"encoding": @"bytes",
			@"data": bytes
		}];
	});
}

@implementation NipworkerRuntime

+ (void *)sharedHandle {
	return NipworkerGetEngineHandleDefault(NULL);
}

+ (void *)sharedHandleWithDefaultRelays:(NSArray<NSString *> *)defaultRelays
                          indexerRelays:(NSArray<NSString *> *)indexerRelays
                         meshBLEEnabled:(BOOL)meshBLEEnabled
                               userdata:(void *)userdata {
	return NipworkerGetEngineHandle(userdata, defaultRelays, indexerRelays, meshBLEEnabled);
}

+ (void)handleMessage:(NSData *)data {
	void* handle = NipworkerGetEngineHandleDefault(NULL);
	if (handle && data.length > 0) {
		nipworker_handle_message(handle, (const uint8_t*)data.bytes, data.length);
	}
}

+ (void)setPrivateKey:(NSString *)secret {
	void* handle = NipworkerGetEngineHandleDefault(NULL);
	if (handle && secret) {
		nipworker_set_private_key(handle, [secret UTF8String]);
	}
}

+ (void)wake {
	void* handle = NipworkerGetEngineHandleDefault(NULL);
	if (handle) {
		nipworker_wake(handle);
	}
}

@end

@implementation NipworkerReactNativeModule

RCT_EXPORT_MODULE(NipworkerReactNativeModule)

+ (BOOL)requiresMainQueueSetup {
	return NO;
}

- (NSArray<NSString *> *)supportedEvents {
	return @[NipworkerEventName];
}

- (void)emitNipworkerData:(NSDictionary *)body {
	if (!_eventEmitterCallback) {
		return;
	}
	[self emitOnData:body];
}

- (void)setEventEmitterCallback:(EventEmitterCallbackWrapper *)eventEmitterCallbackWrapper {
	[super setEventEmitterCallback:eventEmitterCallbackWrapper];
	@synchronized ([NipworkerReactNativeModule class]) {
		if (!NipworkerListenerModules) {
			NipworkerListenerModules = [NSHashTable weakObjectsHashTable];
		}
		[NipworkerListenerModules addObject:self];
	}
}

- (void)startObserving {
	self.hasListeners = YES;
	@synchronized ([NipworkerReactNativeModule class]) {
		if (!NipworkerListenerModules) {
			NipworkerListenerModules = [NSHashTable weakObjectsHashTable];
		}
		[NipworkerListenerModules addObject:self];
	}
}

- (void)stopObserving {
	self.hasListeners = NO;
	@synchronized ([NipworkerReactNativeModule class]) {
		[NipworkerListenerModules removeObject:self];
	}
}

RCT_EXPORT_METHOD(initEngine:(NSArray<NSString *> *)defaultRelays indexerRelays:(NSArray<NSString *> *)indexerRelays meshBLEEnabled:(BOOL)meshBLEEnabled) {
	self.engineHandle = [NipworkerRuntime sharedHandleWithDefaultRelays:defaultRelays
													   indexerRelays:indexerRelays
													  meshBLEEnabled:meshBLEEnabled
														userdata:(__bridge void*)self];
	NSString* profile = [[NSUserDefaults standardUserDefaults] stringForKey:NipworkerMeshProfileKey];
	if (self.engineHandle && profile.length > 0) {
		nipworker_mesh_set_profile_json(self.engineHandle, profile.UTF8String);
	}
}

RCT_EXPORT_BLOCKING_SYNCHRONOUS_METHOD(installByteRuntime) {
	self.engineHandle = NipworkerGetEngineHandleDefault((__bridge void*)self);
	if (NipworkerByteRuntimeInstalled.load()) {
		self.byteRuntimeInstalled = YES;
		self.byteRuntimeAddress = NipworkerByteRuntimeAddress;
		return @YES;
	}
	id bridge = [self respondsToSelector:@selector(bridge)] ? [self performSelector:@selector(bridge)] : nil;
	RCTCxxBridge* cxxBridge = (RCTCxxBridge*)bridge;
	if (![cxxBridge isKindOfClass:[RCTCxxBridge class]] || !cxxBridge.runtime) {
		return @NO;
	}

	facebook::jsi::Runtime& runtime = *reinterpret_cast<facebook::jsi::Runtime*>(cxxBridge.runtime);
	if (self.byteRuntimeInstalled && self.byteRuntimeAddress == &runtime) {
		return @YES;
	}
	NipworkerInstallByteRuntime(runtime, self.engineHandle);
	self.byteRuntimeInstalled = YES;
	self.byteRuntimeAddress = &runtime;
	NipworkerByteRuntimeAddress = &runtime;
	NipworkerByteRuntimeInstalled.store(true);
	return @YES;
}

// The CocoaPods React Native target does not own the Swift CoreBluetooth
// adapter. iOS hosts attach it to this shared handle via NostrManager.reactNativeShared().
RCT_EXPORT_BLOCKING_SYNCHRONOUS_METHOD(startMesh) {
	return @NO;
}

RCT_EXPORT_METHOD(stopMesh) {}

RCT_EXPORT_BLOCKING_SYNCHRONOUS_METHOD(setMeshProfile:(NSString *)profileJson) {
	if (!self.engineHandle || profileJson.length == 0 ||
		!nipworker_mesh_set_profile_json(self.engineHandle, profileJson.UTF8String)) {
		return @NO;
	}
	[[NSUserDefaults standardUserDefaults] setObject:profileJson forKey:NipworkerMeshProfileKey];
	return @YES;
}

RCT_EXPORT_BLOCKING_SYNCHRONOUS_METHOD(clearMeshProfile) {
	[[NSUserDefaults standardUserDefaults] removeObjectForKey:NipworkerMeshProfileKey];
	return @(self.engineHandle && nipworker_mesh_clear_profile(self.engineHandle));
}

RCT_EXPORT_METHOD(handleMessage:(NSArray<NSNumber *> *)bytes) {
	if (self.engineHandle && bytes) {
		NSMutableData* data = [NSMutableData dataWithLength:bytes.count];
		uint8_t* rawBytes = (uint8_t*)data.mutableBytes;
		for (NSUInteger i = 0; i < bytes.count; i++) {
			rawBytes[i] = (uint8_t)(bytes[i].unsignedCharValue);
		}
		[NipworkerRuntime handleMessage:data];
	}
}

RCT_EXPORT_METHOD(setPrivateKey:(NSString *)secret) {
	[NipworkerRuntime setPrivateKey:secret];
}

RCT_EXPORT_METHOD(wake) {
	[NipworkerRuntime wake];
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

- (void)deinitEngine {
	if (self.engineHandle) {
		self.engineHandle = NULL;
	}
}

- (void)invalidate {
	[self deinitEngine];
}

- (void)installJSIBindingsWithRuntime:(facebook::jsi::Runtime &)runtime
                          callInvoker:(const std::shared_ptr<facebook::react::CallInvoker> &)callInvoker {
	if (!self.engineHandle) {
		self.engineHandle = NipworkerGetEngineHandleDefault((__bridge void*)self);
	}
	if (self.byteRuntimeInstalled && self.byteRuntimeAddress == &runtime) {
		return;
	}
	NipworkerInstallByteRuntime(runtime, self.engineHandle);
	self.byteRuntimeInstalled = YES;
	self.byteRuntimeAddress = &runtime;
	NipworkerByteRuntimeAddress = &runtime;
	NipworkerByteRuntimeInstalled.store(true);
}

- (std::shared_ptr<facebook::react::TurboModule>)getTurboModule:
	(const facebook::react::ObjCTurboModule::InitParams &)params {
	return std::make_shared<facebook::react::NativeNipworkerReactNativeSpecJSI>(params);
}

@end
