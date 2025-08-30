#!/bin/bash

# Memory monitoring script for debugging build issues
set -e

echo "🔍 Build Memory Monitor"
echo "======================"

# Function to get memory info in a portable way
get_memory_info() {
    local prefix="$1"
    echo "$prefix Memory Status:"

    if command -v free >/dev/null 2>&1; then
        # Linux
        free -h | grep -E "(Mem:|Swap:)" | sed 's/^/  /'
        echo "  Available: $(free -h | awk '/^Mem:/ {print $7}')"
    elif command -v vm_stat >/dev/null 2>&1; then
        # macOS
        local page_size=$(vm_stat | grep "page size" | awk '{print $8}')
        local free_pages=$(vm_stat | grep "Pages free:" | awk '{print $3}' | sed 's/\.//')
        local active_pages=$(vm_stat | grep "Pages active:" | awk '{print $3}' | sed 's/\.//')
        local inactive_pages=$(vm_stat | grep "Pages inactive:" | awk '{print $3}' | sed 's/\.//')

        echo "  Free Memory: $((free_pages * page_size / 1024 / 1024)) MB"
        echo "  Active Memory: $((active_pages * page_size / 1024 / 1024)) MB"
        echo "  Inactive Memory: $((inactive_pages * page_size / 1024 / 1024)) MB"
    else
        echo "  Memory info not available"
    fi
    echo ""
}

# Function to get process info
get_process_info() {
    local prefix="$1"
    echo "$prefix Process Info:"

    if command -v ps >/dev/null 2>&1; then
        # Get top memory consumers
        echo "  Top 5 memory consumers:"
        if [[ "$OSTYPE" == "darwin"* ]]; then
            # macOS
            ps aux | sort -k4 -nr | head -6 | tail -5 | awk '{printf "    %s: %s%% CPU, %s%% MEM\n", $11, $3, $4}' 2>/dev/null || true
        else
            # Linux
            ps aux --sort=-%mem | head -6 | tail -5 | awk '{printf "    %s: %s%% CPU, %s%% MEM\n", $11, $3, $4}' 2>/dev/null || true
        fi
    fi
    echo ""
}

# Function to get disk space
get_disk_info() {
    local prefix="$1"
    echo "$prefix Disk Usage:"

    if command -v df >/dev/null 2>&1; then
        df -h . | tail -1 | awk '{printf "  Available: %s (%.0f%% used)\n", $4, ($3/($3+$4)*100)}'
    fi
    echo ""
}

# Function to monitor build command
monitor_build() {
    local cmd="$@"
    local start_time=$(date +%s)

    echo "🚀 Starting monitored build: $cmd"
    echo "Start time: $(date)"
    echo ""

    get_memory_info "[START]"
    get_process_info "[START]"
    get_disk_info "[START]"

    # Start background monitoring
    local monitor_pid=""
    if [ "${ENABLE_CONTINUOUS_MONITORING:-false}" = "true" ]; then
        (
            while true; do
                sleep 30
                echo "--- Memory check at $(date) ---"
                get_memory_info "[DURING]"
                if command -v free >/dev/null 2>&1; then
                    local mem_usage=$(free | awk '/^Mem:/ {printf "%.1f", ($3/$2) * 100.0}')
                    if (( $(echo "$mem_usage > 85" | bc -l) )); then
                        echo "⚠️  WARNING: High memory usage detected: ${mem_usage}%"
                    fi
                fi
            done
        ) &
        monitor_pid=$!
    fi

    # Run the actual build command
    local exit_code=0
    if ! eval "$cmd"; then
        exit_code=$?
        echo ""
        echo "❌ Build failed with exit code: $exit_code"
    else
        echo ""
        echo "✅ Build completed successfully"
    fi

    # Stop background monitoring
    if [ -n "$monitor_pid" ]; then
        kill $monitor_pid 2>/dev/null || true
        wait $monitor_pid 2>/dev/null || true
    fi

    local end_time=$(date +%s)
    local duration=$((end_time - start_time))

    echo ""
    echo "🏁 Build finished"
    echo "End time: $(date)"
    echo "Duration: ${duration} seconds"
    echo ""

    get_memory_info "[END]"
    get_process_info "[END]"
    get_disk_info "[END]"

    # Check for common memory issues
    echo "🔍 Post-build Analysis:"
    if command -v dmesg >/dev/null 2>&1 && dmesg | tail -50 | grep -qi "killed process\|out of memory\|oom"; then
        echo "  ⚠️  OOM (Out of Memory) events detected in system log"
        dmesg | tail -20 | grep -i "killed process\|out of memory\|oom" | sed 's/^/    /' || true
    else
        echo "  ✅ No obvious OOM events detected"
    fi

    if [ -f "pkg/rust_worker_bg.wasm" ]; then
        local wasm_size=$(wc -c < pkg/rust_worker_bg.wasm)
        local wasm_mb=$((wasm_size / 1024 / 1024))
        echo "  📦 Final WASM size: ${wasm_size} bytes (${wasm_mb} MB)"
        if [ $wasm_mb -gt 10 ]; then
            echo "    ⚠️  Large WASM file detected - consider optimizing dependencies"
        fi
    fi

    echo ""
    return $exit_code
}

# Function to show usage
show_usage() {
    echo "Usage: $0 [OPTIONS] COMMAND"
    echo ""
    echo "Options:"
    echo "  --continuous-monitor    Enable continuous memory monitoring during build"
    echo "  --help                  Show this help message"
    echo ""
    echo "Examples:"
    echo "  $0 npm run build"
    echo "  $0 --continuous-monitor wasm-pack build --release"
    echo "  $0 ./build.sh"
    echo ""
    echo "Environment variables:"
    echo "  ENABLE_CONTINUOUS_MONITORING=true  Enable continuous monitoring"
}

# Parse command line arguments
CONTINUOUS_MONITORING=false
while [[ $# -gt 0 ]]; do
    case $1 in
        --continuous-monitor)
            export ENABLE_CONTINUOUS_MONITORING=true
            shift
            ;;
        --help)
            show_usage
            exit 0
            ;;
        *)
            break
            ;;
    esac
done

# Check if we have a command to run
if [ $# -eq 0 ]; then
    echo "Error: No command specified"
    echo ""
    show_usage
    exit 1
fi

# Install bc if available for math operations
if ! command -v bc >/dev/null 2>&1; then
    echo "📝 Note: 'bc' not available, some calculations may be skipped"
    echo ""
fi

# Run the monitored build
monitor_build "$@"
