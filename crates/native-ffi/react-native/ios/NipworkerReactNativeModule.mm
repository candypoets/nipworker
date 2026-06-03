#import "NipworkerReactNativeModule.h"
#if __has_include(<React/RCTBridge+Private.h>)
#import <React/RCTBridge+Private.h>
#elif __has_include(<React_Core/React/RCTBridge+Private.h>)
#import <React_Core/React/RCTBridge+Private.h>
#endif
#import <jsi/jsi.h>

#include <memory>
#include <mutex>
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
void nipworker_handle_message(void* handle, const uint8_t* ptr, size_t len);
void nipworker_set_private_key(void* handle, const char* ptr);
void nipworker_deinit(void* handle);
void nipworker_free_bytes(uint8_t* ptr, size_t len);
}

static NSString * const NipworkerEventName = @"NipworkerEvent";
static NSString * const NipworkerStoragePrefix = @"nipworker.";
static std::mutex NipworkerQueuedPacketsMutex;
static std::vector<std::vector<uint8_t>> NipworkerQueuedPackets;
static std::atomic_bool NipworkerByteRuntimeInstalled(false);
static void* NipworkerByteRuntimeAddress = NULL;
static void* NipworkerEngineHandle = NULL;
static NSHashTable<NipworkerReactNativeModule*>* NipworkerListenerModules;

static void NipworkerReactNativeCallbackForwarder(void* userdata, const uint8_t* ptr, size_t len);
static void* NipworkerGetEngineHandle(void* userdata);
static void NipworkerNotifyQueuedPacket(void);

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

static void* NipworkerGetEngineHandle(void* userdata) {
	@synchronized ([NipworkerReactNativeModule class]) {
		if (!NipworkerEngineHandle) {
			NipworkerEngineHandle = nipworker_init(NipworkerReactNativeCallbackForwarder, userdata);
		}
		return NipworkerEngineHandle;
	}
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

RCT_REMAP_METHOD(init, initEngine) {
	self.engineHandle = NipworkerGetEngineHandle((__bridge void*)self);
}

RCT_EXPORT_BLOCKING_SYNCHRONOUS_METHOD(installByteRuntime) {
	self.engineHandle = NipworkerGetEngineHandle((__bridge void*)self);
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
		self.engineHandle = NipworkerGetEngineHandle((__bridge void*)self);
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
