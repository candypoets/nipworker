#include "NipworkerReactNativeTransport.h"

#include <algorithm>
#include <atomic>
#include <condition_variable>
#include <cstdint>
#include <cstdlib>
#include <cstring>
#include <iostream>
#include <mutex>
#include <set>
#include <sstream>
#include <stdexcept>
#include <string>
#include <thread>
#include <unordered_map>
#include <utility>
#include <vector>

namespace transport = nipworker::react_native;

namespace {

struct Failure : std::runtime_error {
	using std::runtime_error::runtime_error;
};

#define REQUIRE(condition)                                                                     \
	do {                                                                                         \
		if (!(condition)) {                                                                        \
			std::ostringstream message;                                                              \
			message << __FILE__ << ':' << __LINE__ << ": requirement failed: " #condition;          \
			throw Failure(message.str());                                                             \
		}                                                                                          \
	} while (false)

class ReleaseTracker {
public:
	static transport::OwnedPacket packet(uint64_t id, size_t size) {
		auto* bytes = new uint8_t[size];
		std::fill(bytes, bytes + size, uint8_t{0});
		if (size >= sizeof(id)) {
			std::memcpy(bytes, &id, sizeof(id));
		} else if (size > 0) {
			bytes[0] = static_cast<uint8_t>(id);
		}
		{
			std::lock_guard<std::mutex> lock(mutex_);
			ids_[bytes] = id;
			releases_[id] = 0;
		}
		return transport::OwnedPacket(bytes, size, &ReleaseTracker::release);
	}

	static uint64_t id(const transport::OwnedPacket& packet) {
		uint64_t value = 0;
		if (packet.data() != nullptr && packet.size() >= sizeof(value)) {
			std::memcpy(&value, packet.data(), sizeof(value));
		} else if (packet.data() != nullptr && packet.size() > 0) {
			value = packet.data()[0];
		}
		return value;
	}

	static size_t releases(uint64_t id) {
		std::lock_guard<std::mutex> lock(mutex_);
		return releases_[id];
	}

	static void reset() {
		std::lock_guard<std::mutex> lock(mutex_);
		REQUIRE(ids_.empty());
		releases_.clear();
	}

private:
	static void release(uint8_t* data, size_t) noexcept {
		uint64_t id = 0;
		{
			std::lock_guard<std::mutex> lock(mutex_);
			auto found = ids_.find(data);
			if (found == ids_.end()) {
				std::abort();
			}
			id = found->second;
			ids_.erase(found);
			releases_[id] += 1;
			if (releases_[id] != 1) {
				std::abort();
			}
		}
		delete[] data;
	}

	static std::mutex mutex_;
	static std::unordered_map<uint8_t*, uint64_t> ids_;
	static std::unordered_map<uint64_t, size_t> releases_;
};

std::mutex ReleaseTracker::mutex_;
std::unordered_map<uint8_t*, uint64_t> ReleaseTracker::ids_;
std::unordered_map<uint64_t, size_t> ReleaseTracker::releases_;

class FakeScheduler {
public:
	void schedule(transport::Generation generation) {
		std::lock_guard<std::mutex> lock(mutex_);
		tasks_.push_back(generation);
	}

	size_t size() const {
		std::lock_guard<std::mutex> lock(mutex_);
		return tasks_.size();
	}

	std::vector<transport::Generation> snapshot() const {
		std::lock_guard<std::mutex> lock(mutex_);
		return tasks_;
	}

private:
	mutable std::mutex mutex_;
	std::vector<transport::Generation> tasks_;
};

std::shared_ptr<transport::DeliveryState> makeState(
	transport::Generation generation,
	transport::DeliveryLimits limits,
	FakeScheduler& scheduler
) {
	auto state = std::make_shared<transport::DeliveryState>(generation, limits);
	state->setSchedule(
		[&scheduler](transport::Generation scheduledGeneration) {
			scheduler.schedule(scheduledGeneration);
		}
	);
	return state;
}

void testTenThousandThreadedRoutesScheduleOneWakeAndDrainCompletely() {
	constexpr transport::Generation generation = 41;
	constexpr size_t eventCount = 10'000;
	constexpr size_t threadCount = 16;
	transport::DeliveryLimits limits;
	limits.maxDirtyRoutes = eventCount + 1;
	FakeScheduler scheduler;
	auto state = makeState(generation, limits, scheduler);

	std::atomic<size_t> accepted{0};
	std::vector<std::thread> producers;
	for (size_t threadIndex = 0; threadIndex < threadCount; ++threadIndex) {
		producers.emplace_back([&, threadIndex] {
			for (size_t i = threadIndex; i < eventCount; i += threadCount) {
				if (state->enqueueRoute(generation, "route-" + std::to_string(i))) {
					accepted.fetch_add(1, std::memory_order_relaxed);
				}
			}
		});
	}
	for (auto& producer : producers) producer.join();

	REQUIRE(accepted.load() == eventCount);
	REQUIRE(scheduler.size() == 1);
	auto batch = state->drain(generation);
	REQUIRE(batch.controls.empty());
	REQUIRE(batch.routes.size() == eventCount);
	std::set<std::string> routes(batch.routes.begin(), batch.routes.end());
	REQUIRE(routes.size() == eventCount);
	for (size_t i = 0; i < eventCount; ++i) {
		REQUIRE(routes.count("route-" + std::to_string(i)) == 1);
	}
	state->finishWake(generation);
	REQUIRE(scheduler.size() == 1);

	const auto stats = state->stats();
	REQUIRE(stats.receivedRoutes == eventCount);
	REQUIRE(stats.scheduledWakes == 1);
	REQUIRE(stats.droppedRoutes == 0);
	REQUIRE(stats.dirtyRoutes == 0);
}

void testDuplicateRoutesCoalesceWithoutExtraOuterWakes() {
	constexpr transport::Generation generation = 42;
	constexpr size_t eventCount = 10'000;
	constexpr size_t routeCount = 32;
	FakeScheduler scheduler;
	auto state = makeState(generation, transport::DeliveryLimits{}, scheduler);

	std::vector<std::thread> producers;
	for (size_t threadIndex = 0; threadIndex < 8; ++threadIndex) {
		producers.emplace_back([&, threadIndex] {
			for (size_t i = threadIndex; i < eventCount; i += 8) {
				state->enqueueRoute(generation, "shared-" + std::to_string(i % routeCount));
			}
		});
	}
	for (auto& producer : producers) producer.join();

	REQUIRE(scheduler.size() == 1);
	auto batch = state->drain(generation);
	REQUIRE(batch.routes.size() == routeCount);
	state->finishWake(generation);
	const auto stats = state->stats();
	REQUIRE(stats.receivedRoutes == eventCount);
	REQUIRE(stats.coalescedRoutes == eventCount - routeCount);
	REQUIRE(stats.scheduledWakes == 1);
}

void testInvalidateAndRecreateDropsLateArrivalsAndReleasesOnce() {
	ReleaseTracker::reset();
	constexpr transport::Generation oldGeneration = 100;
	constexpr transport::Generation newGeneration = 101;
	FakeScheduler oldScheduler;
	auto oldState = makeState(oldGeneration, transport::DeliveryLimits{}, oldScheduler);

	REQUIRE(oldState->enqueueControl(oldGeneration, ReleaseTracker::packet(1, 16)));
	REQUIRE(oldState->enqueueRoute(oldGeneration, "old-route"));
	REQUIRE(oldScheduler.size() == 1);
	oldState->invalidate(oldGeneration);
	REQUIRE(ReleaseTracker::releases(1) == 1);
	REQUIRE(!oldState->alive());

	REQUIRE(!oldState->enqueueControl(oldGeneration, ReleaseTracker::packet(2, 16)));
	REQUIRE(!oldState->enqueueRoute(oldGeneration, "late-same-generation"));
	REQUIRE(ReleaseTracker::releases(2) == 1);
	auto oldBatch = oldState->drain(oldGeneration);
	REQUIRE(oldBatch.controls.empty());
	REQUIRE(oldBatch.routes.empty());
	oldState->finishWake(oldGeneration);
	REQUIRE(oldState->stats().invalidatedDrops >= 2);

	FakeScheduler newScheduler;
	auto newState = makeState(newGeneration, transport::DeliveryLimits{}, newScheduler);
	REQUIRE(!newState->enqueueControl(oldGeneration, ReleaseTracker::packet(3, 16)));
	REQUIRE(!newState->enqueueRoute(oldGeneration, "stale-generation"));
	REQUIRE(ReleaseTracker::releases(3) == 1);
	REQUIRE(newState->stats().staleDrops >= 2);

	REQUIRE(newState->enqueueControl(newGeneration, ReleaseTracker::packet(4, 16)));
	REQUIRE(newState->enqueueRoute(newGeneration, "new-route"));
	REQUIRE(newScheduler.size() == 1);
	{
		auto newBatch = newState->drain(newGeneration);
		REQUIRE(newBatch.controls.size() == 1);
		REQUIRE(ReleaseTracker::id(newBatch.controls.front()) == 4);
		REQUIRE(newBatch.routes == std::vector<std::string>{"new-route"});
		newState->finishWake(newGeneration);
	}
	REQUIRE(ReleaseTracker::releases(4) == 1);
}

void testRepeatedDestroyRecreateRejectsEveryLateGeneration() {
	ReleaseTracker::reset();
	constexpr size_t cycleCount = 100;
	for (size_t cycle = 0; cycle < cycleCount; ++cycle) {
		const transport::Generation generation = 1'000 + cycle;
		FakeScheduler scheduler;
		auto state = makeState(generation, transport::DeliveryLimits{}, scheduler);
		const uint64_t acceptedId = 10'000 + cycle * 2;
		const uint64_t lateId = acceptedId + 1;

		REQUIRE(state->enqueueControl(generation, ReleaseTracker::packet(acceptedId, 16)));
		REQUIRE(state->enqueueRoute(generation, "active-" + std::to_string(cycle)));
		REQUIRE(scheduler.size() == 1);
		state->invalidate(generation);
		REQUIRE(ReleaseTracker::releases(acceptedId) == 1);
		REQUIRE(!state->enqueueControl(generation, ReleaseTracker::packet(lateId, 16)));
		REQUIRE(!state->enqueueRoute(generation, "late-" + std::to_string(cycle)));
		REQUIRE(ReleaseTracker::releases(lateId) == 1);
		REQUIRE(state->drain(generation).routes.empty());
	}
}

void testWakeClearRaceSchedulesFollowUpWithoutLoss() {
	constexpr transport::Generation generation = 200;
	FakeScheduler scheduler;
	auto state = makeState(generation, transport::DeliveryLimits{}, scheduler);
	REQUIRE(state->enqueueRoute(generation, "before-clear"));
	REQUIRE(scheduler.size() == 1);
	auto first = state->drain(generation);
	REQUIRE(first.routes == std::vector<std::string>{"before-clear"});

	std::atomic<size_t> hookCalls{0};
	state->setBeforeWakeClearHook([&] {
		if (hookCalls.fetch_add(1) == 0) {
			REQUIRE(state->enqueueRoute(generation, "during-clear"));
		}
	});
	state->finishWake(generation);
	REQUIRE(hookCalls.load() == 1);
	REQUIRE(scheduler.size() == 2);

	auto second = state->drain(generation);
	REQUIRE(second.routes == std::vector<std::string>{"during-clear"});
	state->setBeforeWakeClearHook({});
	state->finishWake(generation);
	REQUIRE(scheduler.size() == 2);
	REQUIRE(state->stats().dirtyRoutes == 0);
}

void testSchedulerInstallRaceCannotStrandBootstrapData() {
	constexpr transport::Generation generation = 250;
	FakeScheduler scheduler;
	auto state = std::make_shared<transport::DeliveryState>(
		generation,
		transport::DeliveryLimits{}
	);
	std::atomic<bool> hookRan{false};
	state->setBeforeUnscheduledClearHook([&] {
		hookRan.store(true, std::memory_order_release);
		state->setSchedule([&scheduler](transport::Generation scheduledGeneration) {
			scheduler.schedule(scheduledGeneration);
		});
	});

	REQUIRE(state->enqueueRoute(generation, "queued-before-handler"));
	REQUIRE(hookRan.load(std::memory_order_acquire));
	REQUIRE(scheduler.size() == 1);
	auto batch = state->drain(generation);
	REQUIRE(batch.routes == std::vector<std::string>{"queued-before-handler"});
	state->setBeforeUnscheduledClearHook({});
	state->finishWake(generation);
	REQUIRE(scheduler.size() == 1);
}

void testBoundedControlQueueRejectsNewestAndCountsBytes() {
	ReleaseTracker::reset();
	constexpr transport::Generation generation = 300;
	transport::DeliveryLimits limits;
	limits.maxControlPackets = 3;
	limits.maxControlBytes = 10;
	FakeScheduler scheduler;
	auto state = makeState(generation, limits, scheduler);

	REQUIRE(state->enqueueControl(generation, ReleaseTracker::packet(10, 4)));
	REQUIRE(state->enqueueControl(generation, ReleaseTracker::packet(11, 4)));
	REQUIRE(!state->enqueueControl(generation, ReleaseTracker::packet(12, 4)));
	REQUIRE(ReleaseTracker::releases(12) == 1);
	REQUIRE(scheduler.size() == 1);
	{
		auto batch = state->drain(generation);
		REQUIRE(batch.controls.size() == 2);
		REQUIRE(ReleaseTracker::id(batch.controls[0]) == 10);
		REQUIRE(ReleaseTracker::id(batch.controls[1]) == 11);
		state->finishWake(generation);
	}
	REQUIRE(ReleaseTracker::releases(10) == 1);
	REQUIRE(ReleaseTracker::releases(11) == 1);
	const auto stats = state->stats();
	REQUIRE(stats.droppedControlPackets == 1);
	REQUIRE(stats.droppedControlBytes == 4);
	REQUIRE(stats.controlBytesHighWater <= limits.maxControlBytes);
	REQUIRE(stats.queuedControlPackets == 0);
	REQUIRE(stats.queuedControlBytes == 0);

	FakeScheduler oversizedScheduler;
	auto oversized = makeState(generation + 1, limits, oversizedScheduler);
	REQUIRE(!oversized->enqueueControl(generation + 1, ReleaseTracker::packet(13, 11)));
	REQUIRE(ReleaseTracker::releases(13) == 1);
	REQUIRE(oversizedScheduler.size() == 0);
	REQUIRE(oversized->stats().droppedControlPackets == 1);
	REQUIRE(oversized->stats().droppedControlBytes == 11);
}

void testDirtyRouteLimitIsBoundedAndCounted() {
	constexpr transport::Generation generation = 400;
	transport::DeliveryLimits limits;
	limits.maxDirtyRoutes = 2;
	limits.maxDirtyRouteBytes = 14;
	limits.maxRouteBytes = 8;
	FakeScheduler scheduler;
	auto state = makeState(generation, limits, scheduler);

	REQUIRE(state->enqueueRoute(generation, "route-a"));
	REQUIRE(state->enqueueRoute(generation, "route-b"));
	REQUIRE(!state->enqueueRoute(generation, "route-c"));
	auto batch = state->drain(generation);
	REQUIRE(batch.routes.size() == 2);
	state->finishWake(generation);
	REQUIRE(state->stats().droppedRoutes == 1);
	REQUIRE(state->stats().dirtyRouteBytes == 0);
	REQUIRE(state->stats().dirtyRouteBytesHighWater == 14);

	FakeScheduler oversizedScheduler;
	auto oversized = makeState(generation + 1, limits, oversizedScheduler);
	REQUIRE(!oversized->enqueueRoute(generation + 1, "route-too-long"));
	REQUIRE(oversizedScheduler.size() == 0);
	REQUIRE(oversized->stats().droppedRoutes == 1);
}

void testOwnedPacketMoveAndDrainReleaseExactlyOnce() {
	ReleaseTracker::reset();
	constexpr transport::Generation generation = 500;
	FakeScheduler scheduler;
	auto state = makeState(generation, transport::DeliveryLimits{}, scheduler);
	{
		auto packet = ReleaseTracker::packet(20, 32);
		REQUIRE(state->enqueueControl(generation, std::move(packet)));
		REQUIRE(!packet);
		REQUIRE(ReleaseTracker::releases(20) == 0);
	}
	{
		auto batch = state->drain(generation);
		REQUIRE(batch.controls.size() == 1);
		REQUIRE(ReleaseTracker::id(batch.controls.front()) == 20);
		state->finishWake(generation);
	}
	REQUIRE(ReleaseTracker::releases(20) == 1);
}

using Test = std::pair<const char*, void (*)()>;

} // namespace

int main() {
	const std::vector<Test> tests = {
		{"10k threaded route burst", &testTenThousandThreadedRoutesScheduleOneWakeAndDrainCompletely},
		{"duplicate route coalescing", &testDuplicateRoutesCoalesceWithoutExtraOuterWakes},
		{"invalidate and recreate", &testInvalidateAndRecreateDropsLateArrivalsAndReleasesOnce},
		{"repeated destroy and recreate", &testRepeatedDestroyRecreateRejectsEveryLateGeneration},
		{"wake clear race", &testWakeClearRaceSchedulesFollowUpWithoutLoss},
		{"scheduler install race", &testSchedulerInstallRaceCannotStrandBootstrapData},
		{"bounded control saturation", &testBoundedControlQueueRejectsNewestAndCountsBytes},
		{"bounded dirty routes", &testDirtyRouteLimitIsBoundedAndCounted},
		{"owned packet release once", &testOwnedPacketMoveAndDrainReleaseExactlyOnce},
	};

	for (const auto& [name, test] : tests) {
		try {
			test();
			std::cout << "PASS " << name << '\n';
		} catch (const std::exception& error) {
			std::cerr << "FAIL " << name << ": " << error.what() << '\n';
			return 1;
		}
	}
	std::cout << "PASS native delivery transport (" << tests.size() << " tests)\n";
	return 0;
}
