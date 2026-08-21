#include "NipworkerReactNativeTransport.h"

#include <algorithm>
#include <utility>

namespace nipworker::react_native {

OwnedPacket::OwnedPacket(std::uint8_t* data, std::size_t size, Release release) noexcept
	: data_(data), size_(size), release_(release) {}

OwnedPacket::OwnedPacket(OwnedPacket&& other) noexcept
	: data_(other.data_), size_(other.size_), release_(other.release_) {
	other.data_ = nullptr;
	other.size_ = 0;
	other.release_ = nullptr;
}

OwnedPacket& OwnedPacket::operator=(OwnedPacket&& other) noexcept {
	if (this == &other) return *this;
	reset();
	data_ = other.data_;
	size_ = other.size_;
	release_ = other.release_;
	other.data_ = nullptr;
	other.size_ = 0;
	other.release_ = nullptr;
	return *this;
}

OwnedPacket::~OwnedPacket() {
	reset();
}

void OwnedPacket::reset() noexcept {
	if (data_ != nullptr && release_ != nullptr) release_(data_, size_);
	data_ = nullptr;
	size_ = 0;
	release_ = nullptr;
}

DeliveryState::DeliveryState(Generation generation, DeliveryLimits limits)
	: generation_(generation), limits_(limits) {}

void DeliveryState::setSchedule(Schedule schedule) {
	{
		std::lock_guard<std::mutex> lock(mutex_);
		schedule_ = std::move(schedule);
	}
	if (hasPending()) requestWake();
}

bool DeliveryState::accepts(Generation sourceGeneration, OwnedPacket* packet) {
	if (!alive()) {
		std::lock_guard<std::mutex> lock(mutex_);
		stats_.invalidatedDrops++;
		if (packet) {
			stats_.droppedControlPackets++;
			stats_.droppedControlBytes += packet->size();
		}
		return false;
	}
	if (sourceGeneration != generation_) {
		std::lock_guard<std::mutex> lock(mutex_);
		stats_.staleDrops++;
		if (packet) {
			stats_.droppedControlPackets++;
			stats_.droppedControlBytes += packet->size();
		}
		return false;
	}
	return true;
}

bool DeliveryState::enqueueRoute(Generation sourceGeneration, std::string route) {
	if (!accepts(sourceGeneration) || route.empty()) return false;
	const auto routeBytes = route.size();
	{
		std::lock_guard<std::mutex> lock(mutex_);
		if (!alive_.load(std::memory_order_relaxed)) {
			stats_.invalidatedDrops++;
			return false;
		}
		stats_.receivedRoutes++;
		if (dirtyRouteSet_.find(route) != dirtyRouteSet_.end()) {
			stats_.coalescedRoutes++;
			return true;
		}
		if (routeBytes > limits_.maxRouteBytes || routeBytes > limits_.maxDirtyRouteBytes ||
			dirtyRouteOrder_.size() >= limits_.maxDirtyRoutes ||
			dirtyRouteBytes_ > limits_.maxDirtyRouteBytes - routeBytes) {
			// Production registration uses the same route/count limits, so every
			// Rust-emitted route has reserved capacity. This branch protects malformed
			// or out-of-contract native input without making the queue unbounded.
			stats_.droppedRoutes++;
			return false;
		}
		dirtyRouteSet_.insert(route);
		dirtyRouteOrder_.emplace_back(std::move(route));
		dirtyRouteBytes_ += routeBytes;
		stats_.dirtyRoutes = dirtyRouteOrder_.size();
		stats_.dirtyRouteBytes = dirtyRouteBytes_;
		stats_.dirtyRouteBytesHighWater =
			std::max(stats_.dirtyRouteBytesHighWater, dirtyRouteBytes_);
	}
	requestWake();
	return true;
}

bool DeliveryState::enqueueControl(Generation sourceGeneration, OwnedPacket packet) {
	if (!packet || !accepts(sourceGeneration, &packet)) return false;
	const auto packetSize = packet.size();
	{
		std::lock_guard<std::mutex> lock(mutex_);
		if (!alive_.load(std::memory_order_relaxed)) {
			stats_.invalidatedDrops++;
			stats_.droppedControlPackets++;
			stats_.droppedControlBytes += packetSize;
			return false;
		}
		stats_.receivedControls++;
		if (packetSize > limits_.maxControlBytes || limits_.maxControlPackets == 0) {
			stats_.droppedControlPackets++;
			stats_.droppedControlBytes += packetSize;
			return false;
		}
		// Preserve accepted FIFO/causality for signer and auth responses. Reject
		// the incoming control when the bounded queue is saturated.
		if (controlPackets_.size() >= limits_.maxControlPackets ||
			stats_.queuedControlBytes > limits_.maxControlBytes - packetSize) {
			stats_.droppedControlPackets++;
			stats_.droppedControlBytes += packetSize;
			return false;
		}
		controlPackets_.emplace_back(std::move(packet));
		stats_.queuedControlPackets = controlPackets_.size();
		stats_.queuedControlBytes += packetSize;
		stats_.controlBytesHighWater =
			std::max(stats_.controlBytesHighWater, stats_.queuedControlBytes);
	}
	requestWake();
	return true;
}

DrainBatch DeliveryState::drain(Generation generation) {
	DrainBatch batch;
	if (!alive() || generation != generation_) return batch;
	std::lock_guard<std::mutex> lock(mutex_);
	batch.routes.reserve(dirtyRouteOrder_.size());
	while (!dirtyRouteOrder_.empty()) {
		batch.routes.emplace_back(std::move(dirtyRouteOrder_.front()));
		dirtyRouteOrder_.pop_front();
	}
	dirtyRouteSet_.clear();
	dirtyRouteBytes_ = 0;
	batch.controls.reserve(controlPackets_.size());
	while (!controlPackets_.empty()) {
		batch.controls.emplace_back(std::move(controlPackets_.front()));
		controlPackets_.pop_front();
	}
	stats_.queuedControlPackets = 0;
	stats_.queuedControlBytes = 0;
	stats_.dirtyRoutes = 0;
	stats_.dirtyRouteBytes = 0;
	return batch;
}

void DeliveryState::finishWake(Generation generation) {
	if (generation != generation_) return;
	BeforeWakeClearHook hook;
	{
		std::lock_guard<std::mutex> lock(mutex_);
		stats_.executedWakes++;
		hook = beforeWakeClearHook_;
	}
	if (hook) hook();
	wakeScheduled_.store(false, std::memory_order_release);
	// An arrival before the clear observed wakeScheduled=true. Recheck after
	// clearing so it cannot be stranded; a concurrent arrival after the clear
	// wins the same CAS and the two paths coalesce.
	if (alive() && hasPending()) requestWake();
}

void DeliveryState::invalidate(Generation generation) {
	if (generation != generation_) return;
	if (!alive_.exchange(false, std::memory_order_acq_rel)) return;
	std::lock_guard<std::mutex> lock(mutex_);
	stats_.invalidatedDrops += dirtyRouteOrder_.size() + controlPackets_.size();
	for (const auto& packet : controlPackets_) {
		stats_.droppedControlPackets++;
		stats_.droppedControlBytes += packet.size();
	}
	dirtyRouteOrder_.clear();
	dirtyRouteSet_.clear();
	dirtyRouteBytes_ = 0;
	controlPackets_.clear();
	stats_.queuedControlPackets = 0;
	stats_.queuedControlBytes = 0;
	stats_.dirtyRoutes = 0;
	stats_.dirtyRouteBytes = 0;
	schedule_ = {};
	wakeScheduled_.store(false, std::memory_order_release);
}

DeliveryStats DeliveryState::stats() const {
	std::lock_guard<std::mutex> lock(mutex_);
	return stats_;
}

void DeliveryState::setBeforeWakeClearHook(BeforeWakeClearHook hook) {
	std::lock_guard<std::mutex> lock(mutex_);
	beforeWakeClearHook_ = std::move(hook);
}

void DeliveryState::setBeforeUnscheduledClearHook(BeforeUnscheduledClearHook hook) {
	std::lock_guard<std::mutex> lock(mutex_);
	beforeUnscheduledClearHook_ = std::move(hook);
}

bool DeliveryState::hasPending() const {
	std::lock_guard<std::mutex> lock(mutex_);
	return !dirtyRouteOrder_.empty() || !controlPackets_.empty();
}

void DeliveryState::requestWake() {
	if (!alive()) return;
	bool expected = false;
	if (!wakeScheduled_.compare_exchange_strong(
			expected,
			true,
			std::memory_order_acq_rel,
			std::memory_order_acquire)) {
		return;
	}
	Schedule schedule;
	{
		std::lock_guard<std::mutex> lock(mutex_);
		schedule = schedule_;
		if (schedule) stats_.scheduledWakes++;
	}
	if (!schedule) {
		BeforeUnscheduledClearHook hook;
		{
			std::lock_guard<std::mutex> lock(mutex_);
			hook = beforeUnscheduledClearHook_;
		}
		if (hook) hook();
		wakeScheduled_.store(false, std::memory_order_release);
		// setSchedule may have raced after the empty scheduler was observed and
		// seen wakeScheduled=true. Recheck both scheduler and pending state after
		// the clear so bootstrap data cannot be stranded.
		bool shouldRetry = false;
		{
			std::lock_guard<std::mutex> lock(mutex_);
			shouldRetry = alive_.load(std::memory_order_relaxed) &&
				static_cast<bool>(schedule_) &&
				(!dirtyRouteOrder_.empty() || !controlPackets_.empty());
		}
		if (shouldRetry) requestWake();
		return;
	}
	try {
		schedule(generation_);
	} catch (...) {
		wakeScheduled_.store(false, std::memory_order_release);
		// Treat scheduler rejection like the normal wake-completion race: an
		// arrival may have observed the scheduled bit while invokeAsync threw.
		// Re-arm once a scheduler is still installed so pending work is never
		// stranded. Runtime invalidation removes the scheduler and stops retries.
		bool shouldRetry = false;
		{
			std::lock_guard<std::mutex> lock(mutex_);
			shouldRetry = alive_.load(std::memory_order_relaxed) &&
				static_cast<bool>(schedule_) &&
				(!dirtyRouteOrder_.empty() || !controlPackets_.empty());
		}
		if (shouldRetry) requestWake();
	}
}

} // namespace nipworker::react_native
