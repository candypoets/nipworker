#import "NipworkerReactNativeModule.h"

#import <jsi/jsi.h>

#include "../cpp/NipworkerReactNativeJSI.h"

#include <memory>
#include <string>

extern "C" {
#include "nipworker.h"
}

using nipworker::react_native::EngineHost;
using nipworker::react_native::RuntimeTransport;

static NSString* const NipworkerStoragePrefix = @"nipworker.";
static NSString* const NipworkerMeshProfileKey = @"nipworker.meshProfile";

@interface NipworkerReactNativeModule ()
@property(nonatomic, assign) void* engineHandle;
@property(nonatomic, assign) void* transportBox;
- (void)invalidateTransport;
@end

static std::shared_ptr<RuntimeTransport>* NipworkerTransportBox(void* box) {
	return static_cast<std::shared_ptr<RuntimeTransport>*>(box);
}

static std::string NipworkerUTF8(NSString* value) {
	return value.length > 0 ? std::string(value.UTF8String) : std::string();
}

static NSString* NipworkerRelayCSV(NSArray<NSString*>* relays) {
	NSMutableArray<NSString*>* clean = [NSMutableArray array];
	for (NSString* relay in relays ?: @[]) {
		if (![relay isKindOfClass:[NSString class]]) continue;
		NSString* trimmed = [relay stringByTrimmingCharactersInSet:NSCharacterSet.whitespaceAndNewlineCharacterSet];
		if (trimmed.length > 0 && [trimmed rangeOfString:@","].location == NSNotFound) {
			[clean addObject:trimmed];
		}
	}
	return [clean componentsJoinedByString:@","];
}

static NSString* NipworkerStorageDirectory(void) {
	NSArray<NSURL*>* urls = [[NSFileManager defaultManager]
		URLsForDirectory:NSApplicationSupportDirectory
		inDomains:NSUserDomainMask];
	NSURL* baseURL = urls.firstObject ?: [NSURL fileURLWithPath:NSTemporaryDirectory()];
	NSURL* directory = [baseURL URLByAppendingPathComponent:@"nipworker" isDirectory:YES];
	[[NSFileManager defaultManager] createDirectoryAtURL:directory
		withIntermediateDirectories:YES
		attributes:nil
		error:nil];
	return directory.path;
}

@implementation NipworkerReactNativeModule

RCT_EXPORT_MODULE(NipworkerReactNativeModule)

+ (BOOL)requiresMainQueueSetup {
	return NO;
}

RCT_EXPORT_METHOD(initEngine:(NSArray<NSString*>*)defaultRelays
	indexerRelays:(NSArray<NSString*>*)indexerRelays
	meshBLEEnabled:(BOOL)meshBLEEnabled
	logLevel:(NSString*)logLevel) {
	nipworker_set_log_level(logLevel.UTF8String);
	self.engineHandle = EngineHost::shared().configure(
		NipworkerUTF8(NipworkerStorageDirectory()),
		NipworkerUTF8(NipworkerRelayCSV(defaultRelays)),
		NipworkerUTF8(NipworkerRelayCSV(indexerRelays)),
		meshBLEEnabled
	);
	NSString* profile = [[NSUserDefaults standardUserDefaults] stringForKey:NipworkerMeshProfileKey];
	if (self.engineHandle && profile.length > 0) {
		nipworker_mesh_set_profile_json(self.engineHandle, profile.UTF8String);
	}
}

RCT_EXPORT_BLOCKING_SYNCHRONOUS_METHOD(installByteRuntime) {
	// New-architecture installation is performed by
	// installJSIBindingsWithRuntime:callInvoker:, which supplies the runtime's
	// scheduler. A raw Runtime pointer alone is deliberately insufficient.
	@synchronized(self) {
		return @(self.transportBox != nullptr);
	}
}

RCT_EXPORT_BLOCKING_SYNCHRONOUS_METHOD(startMesh) {
	return @NO;
}

RCT_EXPORT_METHOD(stopMesh) {}

RCT_EXPORT_BLOCKING_SYNCHRONOUS_METHOD(setMeshProfile:(NSString*)profileJson) {
	void* handle = self.engineHandle != nullptr
		? self.engineHandle
		: EngineHost::shared().handle();
	if (!handle || profileJson.length == 0 ||
		!nipworker_mesh_set_profile_json(handle, profileJson.UTF8String)) return @NO;
	[[NSUserDefaults standardUserDefaults] setObject:profileJson forKey:NipworkerMeshProfileKey];
	return @YES;
}

RCT_EXPORT_BLOCKING_SYNCHRONOUS_METHOD(clearMeshProfile) {
	[[NSUserDefaults standardUserDefaults] removeObjectForKey:NipworkerMeshProfileKey];
	void* handle = self.engineHandle != nullptr
		? self.engineHandle
		: EngineHost::shared().handle();
	return @(handle && nipworker_mesh_clear_profile(handle));
}

RCT_EXPORT_METHOD(handleMessage:(NSArray<NSNumber*>*)bytes) {
	if (bytes.count == 0) return;
	NSMutableData* data = [NSMutableData dataWithLength:bytes.count];
	auto* output = static_cast<std::uint8_t*>(data.mutableBytes);
	for (NSUInteger index = 0; index < bytes.count; index++) {
		output[index] = bytes[index].unsignedCharValue;
	}
	if (auto* handle = EngineHost::shared().handle()) {
		nipworker_handle_message(handle, output, data.length);
	}
}

RCT_EXPORT_METHOD(setPrivateKey:(NSString*)secret) {
	if (auto* handle = EngineHost::shared().handle(); handle && secret.length > 0) {
		nipworker_set_private_key(handle, secret.UTF8String);
	}
}
RCT_EXPORT_METHOD(clearSigner) {
	if (auto* handle = EngineHost::shared().handle()) nipworker_clear_signer(handle);
}
RCT_EXPORT_METHOD(removeSigner) {
	if (auto* handle = EngineHost::shared().handle()) nipworker_remove_signer(handle);
}
RCT_EXPORT_METHOD(wake) {
	if (auto* handle = EngineHost::shared().handle()) nipworker_wake(handle);
}

RCT_EXPORT_BLOCKING_SYNCHRONOUS_METHOD(getStorageItem:(NSString*)key) {
	if (!key) return (id)kCFNull;
	return [[NSUserDefaults standardUserDefaults]
		stringForKey:[NipworkerStoragePrefix stringByAppendingString:key]] ?: (id)kCFNull;
}

RCT_EXPORT_BLOCKING_SYNCHRONOUS_METHOD(setStorageItem:(NSString*)key value:(NSString*)value) {
	if (!key || !value) return @NO;
	[[NSUserDefaults standardUserDefaults]
		setObject:value
		forKey:[NipworkerStoragePrefix stringByAppendingString:key]];
	return @([[NSUserDefaults standardUserDefaults] synchronize]);
}

RCT_EXPORT_BLOCKING_SYNCHRONOUS_METHOD(removeStorageItem:(NSString*)key) {
	if (!key) return @NO;
	[[NSUserDefaults standardUserDefaults]
		removeObjectForKey:[NipworkerStoragePrefix stringByAppendingString:key]];
	return @([[NSUserDefaults standardUserDefaults] synchronize]);
}

- (void)deinitEngine {
	[self invalidateTransport];
	EngineHost::shared().deinit();
	self.engineHandle = nullptr;
}

- (void)invalidate {
	[self invalidateTransport];
	EngineHost::shared().deinit();
	self.engineHandle = nullptr;
}

- (void)invalidateTransport {
	@synchronized(self) {
		if (self.transportBox) {
			auto* box = NipworkerTransportBox(self.transportBox);
			if (*box) (*box)->invalidate();
			delete box;
			self.transportBox = nullptr;
		}
	}
}

- (void)installJSIBindingsWithRuntime:(facebook::jsi::Runtime&)runtime
	callInvoker:(const std::shared_ptr<facebook::react::CallInvoker>&)callInvoker {
	@synchronized(self) {
		if (self.transportBox) return;
		auto transport = RuntimeTransport::create(callInvoker);
		if (!transport->install(runtime)) return;
		self.transportBox = new std::shared_ptr<RuntimeTransport>(std::move(transport));
	}
}

- (std::shared_ptr<facebook::react::TurboModule>)getTurboModule:
	(const facebook::react::ObjCTurboModule::InitParams&)params {
	return std::make_shared<facebook::react::NativeNipworkerReactNativeSpecJSI>(params);
}

@end
