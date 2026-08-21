#include "NipworkerReactNativeTransport.h"

#include <atomic>
#include <chrono>
#include <cstdint>
#include <cstring>
#include <iostream>
#include <memory>
#include <string>
#include <sys/resource.h>
#include <thread>
#include <vector>

namespace transport = nipworker::react_native;

namespace {

void releasePacket(uint8_t* data, size_t) noexcept {
	delete[] data;
}

transport::OwnedPacket packet(uint64_t id, size_t size) {
	auto* data = new uint8_t[size];
	std::memset(data, 0, size);
	if (size >= sizeof(id)) std::memcpy(data, &id, sizeof(id));
	return transport::OwnedPacket(data, size, &releasePacket);
}

double elapsedMilliseconds(std::chrono::steady_clock::time_point start) {
	return std::chrono::duration<double, std::milli>(std::chrono::steady_clock::now() - start)
		.count();
}

long maximumRssKilobytes() {
	rusage usage{};
	return getrusage(RUSAGE_SELF, &usage) == 0 ? usage.ru_maxrss : -1;
}

} // namespace

int main(int argc, char** argv) {
	const size_t eventCount = argc > 1 ? std::stoull(argv[1]) : 1'000'000;
	const size_t threadCount = argc > 2 ? std::stoull(argv[2]) : 16;
	const size_t controlCount = argc > 3 ? std::stoull(argv[3]) : 100'000;
	constexpr size_t routeCount = 256;
	constexpr size_t controlSize = 64;
	constexpr transport::Generation generation = 900;

	transport::DeliveryLimits limits;
	limits.maxDirtyRoutes = routeCount + 1;
	limits.maxControlPackets = controlCount + 1;
	limits.maxControlBytes = (controlCount + 1) * controlSize;
	std::atomic<uint64_t> schedulerCalls{0};
	auto state = std::make_shared<transport::DeliveryState>(generation, limits);
	state->setSchedule(
		[&](transport::Generation) { schedulerCalls.fetch_add(1, std::memory_order_relaxed); }
	);

	const auto routeStart = std::chrono::steady_clock::now();
	std::vector<std::thread> producers;
	for (size_t threadIndex = 0; threadIndex < threadCount; ++threadIndex) {
		producers.emplace_back([&, threadIndex] {
			for (size_t i = threadIndex; i < eventCount; i += threadCount) {
				state->enqueueRoute(generation, "route-" + std::to_string(i % routeCount));
			}
		});
	}
	for (auto& producer : producers) producer.join();
	const double routeEnqueueMs = elapsedMilliseconds(routeStart);
	const auto routeDrainStart = std::chrono::steady_clock::now();
	auto routeBatch = state->drain(generation);
	state->finishWake(generation);
	const double routeDrainMs = elapsedMilliseconds(routeDrainStart);
	const auto routeStats = state->stats();

	const auto controlStart = std::chrono::steady_clock::now();
	for (size_t i = 0; i < controlCount; ++i) {
		state->enqueueControl(generation, packet(i, controlSize));
	}
	const double controlEnqueueMs = elapsedMilliseconds(controlStart);
	const auto controlDrainStart = std::chrono::steady_clock::now();
	auto controlBatch = state->drain(generation);
	state->finishWake(generation);
	const double controlDrainMs = elapsedMilliseconds(controlDrainStart);
	const auto stats = state->stats();

	const double routeThroughput = eventCount / (routeEnqueueMs / 1000.0);
	const double controlThroughput = controlCount / (controlEnqueueMs / 1000.0);
	const auto controlScheduledWakes = stats.scheduledWakes - routeStats.scheduledWakes;
	const auto acceptedControlPackets = controlBatch.controls.size();
	const double modeledWakeReduction = eventCount == 0
		? 0.0
		: (1.0 - static_cast<double>(routeStats.scheduledWakes) / static_cast<double>(eventCount)) *
			100.0;
	std::cout << '{'
		<< "\"eventCount\":" << eventCount << ','
		<< "\"threadCount\":" << threadCount << ','
		<< "\"routeCount\":" << routeCount << ','
		<< "\"routeEnqueueMs\":" << routeEnqueueMs << ','
		<< "\"routeDrainMs\":" << routeDrainMs << ','
		<< "\"routeEventsPerSecond\":" << routeThroughput << ','
		<< "\"routesDrained\":" << routeBatch.routes.size() << ','
		<< "\"routeScheduledWakes\":" << routeStats.scheduledWakes << ','
		<< "\"legacyModeledRouteWakes\":" << eventCount << ','
		<< "\"legacyModeledJniByteArrayAllocations\":" << eventCount << ','
		<< "\"legacyModeledPayloadCopies\":" << (eventCount * 2) << ','
		<< "\"newRoutePayloadCopies\":0,"
		<< "\"modeledRouteWakeReductionPercent\":" << modeledWakeReduction << ','
		<< "\"controlCount\":" << controlCount << ','
		<< "\"controlSize\":" << controlSize << ','
		<< "\"controlEnqueueMs\":" << controlEnqueueMs << ','
		<< "\"controlDrainMs\":" << controlDrainMs << ','
		<< "\"controlPacketsPerSecond\":" << controlThroughput << ','
		<< "\"controlsDrained\":" << controlBatch.controls.size() << ','
		<< "\"acceptedControlPackets\":" << acceptedControlPackets << ','
		<< "\"controlScheduledWakes\":" << controlScheduledWakes << ','
		<< "\"scheduledWakes\":" << stats.scheduledWakes << ','
		<< "\"schedulerCalls\":" << schedulerCalls.load() << ','
		<< "\"receivedControls\":" << stats.receivedControls << ','
		<< "\"droppedControlPackets\":" << stats.droppedControlPackets << ','
		<< "\"droppedControlBytes\":" << stats.droppedControlBytes << ','
		<< "\"controlBytesHighWater\":" << stats.controlBytesHighWater << ','
		<< "\"maximumRssKb\":" << maximumRssKilobytes()
		<< "}\n";
	return 0;
}
