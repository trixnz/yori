#pragma once

#include "rust/cxx.h"

#include <cstdint>
#include <memory>

namespace yori::p4 {

struct CancellationState;
struct RawResult;

class NativeThread {
public:
    explicit NativeThread(RawResult& result);
    ~NativeThread();

    NativeThread(const NativeThread&) = delete;
    NativeThread& operator=(const NativeThread&) = delete;

    [[nodiscard]] bool ready() const;
    void shutdown(RawResult& result);

private:
    void shutdown(RawResult* result);

    class Impl;
    std::unique_ptr<Impl> impl_;
};

class NativeClient {
public:
    NativeClient(rust::Str cwd, rust::Str port_override, RawResult& result);
    ~NativeClient();

    NativeClient(const NativeClient&) = delete;
    NativeClient& operator=(const NativeClient&) = delete;

    void run(rust::Str command,
             rust::Slice<const rust::String> arguments,
             const CancellationState& cancellation,
             RawResult& result);

    [[nodiscard]] bool connected() const;
    void close(RawResult& result);

private:
    class Impl;
    std::unique_ptr<Impl> impl_;
};

std::unique_ptr<NativeThread> start_thread(RawResult& result);
std::unique_ptr<NativeClient> connect(
    rust::Str cwd, rust::Str port_override, RawResult& result);
void capture_diagnostic(rust::Slice<const std::uint8_t> diagnostic, RawResult& result);

} // namespace yori::p4
