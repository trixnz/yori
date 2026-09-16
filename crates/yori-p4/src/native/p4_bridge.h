#pragma once

#include "rust/cxx.h"

#include <memory>

namespace yori::p4 {

struct CancellationState;
struct RawResult;

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

private:
    class Impl;
    std::unique_ptr<Impl> impl_;
};

std::unique_ptr<NativeClient> connect(
    rust::Str cwd, rust::Str port_override, RawResult& result);

} // namespace yori::p4
