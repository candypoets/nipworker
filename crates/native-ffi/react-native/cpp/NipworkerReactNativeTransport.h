#pragma once

#include <atomic>
#include <cstddef>
#include <cstdint>
#include <deque>
#include <functional>
#include <memory>
#include <mutex>
#include <string>
#include <unordered_set>
#include <vector>

namespace nipworker::react_native {

using Generation = std::uint64_t;

struct OwnedPacket {
	using Release = void (*)(std::uint8_t*, std::size_t) noexcept;

	OwnedPacket() noexcept = default;
	OwnedPacket(std::uint8_t* data, std::size_t size, Release release) noexcept;
	OwnedPacket(const OwnedPacket&) = delete;
	OwnedPacket& operator=(const OwnedPacket&) = delete;
	OwnedPacket(OwnedPacket&& other) noexcept;
	OwnedPacket& operator=(OwnedPacket&& other) noexcept;
	~OwnedPacket();

	std::uint8_t* data() const noexcept { return data_; }
	std::size_t size() const noexcept { return size_; }
	explicit operator bool() const noexcept { return data_ != nullptr && size_ != 0; }
	void reset() noexcept;

private:
	std::uint8_t* data_ = nullptr;
	std::size_t size_ = 0;
	Release release_ = nullptr;
};

struct DeliveryLimits {
	std::size_t maxControlPackets = 4096;
	std::size_t maxControlBytes = 8 * 1024 * 1024;
	std::size_t maxDirtyRoutes = 16384;
	std::size_t maxDirtyRouteBytes = 2 * 1024 * 1024;
	std::size_t maxRouteBytes = 1024;
};

struct DeliveryStats {
	std::uint64_t receivedRoutes = 0;
	std::uint64_t receivedControls = 0;
	std::uint64_t coalescedRoutes = 0;
	std::uint64_t scheduledWakes = 0;
	std::uint64_t executedWakes = 0;
	std::uint64_t droppedControlPackets = 0;
	std::uint64_t droppedControlBytes = 0;
	std::uint64_t droppedRoutes = 0;
	std::uint64_t staleDrops = 0;
	std::uint64_t invalidatedDrops = 0;
	std::size_t queuedControlPackets = 0;
	std::size_t queuedControlBytes = 0;
	std::size_t dirtyRoutes = 0;
	std::size_t dirtyRouteBytes = 0;
	std::size_t dirtyRouteBytesHighWater = 0;
	std::size_t controlBytesHighWater = 0;
};

struct DrainBatch {
	std::vector<std::string> routes;
	std::vector<OwnedPacket> controls;
};

// Thread-safe, runtime-independent delivery state. Platform adapters provide
// the scheduler; all queue and wake coalescing semantics are shared.
class DeliveryState final : public std::enable_shared_from_this<DeliveryState> {
public:
	using Schedule = std::function<void(Generation)>;
	using BeforeWakeClearHook = std::function<void()>;
	using BeforeUnscheduledClearHook = std::function<void()>;

	explicit DeliveryState(Generation generation, DeliveryLimits limits = {});

	void setSchedule(Schedule schedule);
	bool enqueueRoute(Generation sourceGeneration, std::string route);
	bool enqueueControl(Generation sourceGeneration, OwnedPacket packet);
	DrainBatch drain(Generation generation);
	void finishWake(Generation generation);
	void invalidate(Generation generation);
	DeliveryStats stats() const;
	Generation generation() const noexcept { return generation_; }
	bool alive() const noexcept { return alive_.load(std::memory_order_acquire); }

	// Deterministic race seam used by the host stress suite. Production leaves it unset.
	void setBeforeWakeClearHook(BeforeWakeClearHook hook);
	void setBeforeUnscheduledClearHook(BeforeUnscheduledClearHook hook);

private:
	bool accepts(Generation sourceGeneration, OwnedPacket* packet = nullptr);
	bool hasPending() const;
	void requestWake();

	const Generation generation_;
	const DeliveryLimits limits_;
	std::atomic_bool alive_{true};
	std::atomic_bool wakeScheduled_{false};
	mutable std::mutex mutex_;
	Schedule schedule_;
	BeforeWakeClearHook beforeWakeClearHook_;
	BeforeUnscheduledClearHook beforeUnscheduledClearHook_;
	std::deque<std::string> dirtyRouteOrder_;
	std::unordered_set<std::string> dirtyRouteSet_;
	std::size_t dirtyRouteBytes_ = 0;
	std::deque<OwnedPacket> controlPackets_;
	DeliveryStats stats_;
};

} // namespace nipworker::react_native
