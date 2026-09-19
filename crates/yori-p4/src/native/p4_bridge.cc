#include "p4_bridge.h"
#include "yori-p4/src/lib.rs.h"

#include <p4/clientapi.h>
#include <p4/p4libs.h>

#include <cstdint>
#include <cstring>
#include <limits>
#include <memory>
#include <mutex>
#include <string>
#include <utility>
#include <vector>

namespace yori::p4 {
namespace {

constexpr int kLibraryFlags = P4LIBRARIES_INIT_ALL;

std::mutex libraries_mutex;
std::size_t library_users = 0;
thread_local std::size_t active_threads = 0;
thread_local std::size_t active_clients = 0;

rust::String rust_string(const char* data, int length) {
    if (data == nullptr || length <= 0) {
        return rust::String();
    }

    return rust::String(data, static_cast<std::size_t>(length));
}

void append_bytes(rust::Vec<std::uint8_t>& destination, const char* data, int length) {
    if (data == nullptr || length <= 0) {
        return;
    }

    const auto* begin = reinterpret_cast<const std::uint8_t*>(data);
    destination.reserve(destination.size() + static_cast<std::size_t>(length));
    for (int index = 0; index < length; ++index) {
        destination.push_back(begin[index]);
    }
}

void append_message(RawResult& result, int severity, int generic, const char* text, int length) {
    RawMessage message;
    message.severity = severity;
    message.generic = generic;
    append_bytes(message.text, text, length);
    result.messages.push_back(std::move(message));
}

void append_internal_error(RawResult& result, const char* text) {
    append_message(
        result,
        E_FAILED,
        0,
        text,
        static_cast<int>(std::strlen(text)));
}

void append_error(RawResult* result, Error* error) {
    if (result == nullptr || !error->Test()) {
        return;
    }

    StrBuf formatted;
    error->Fmt(&formatted, EF_PLAIN);
    append_message(
        *result,
        error->GetSeverity(),
        error->GetGeneric(),
        formatted.Text(),
        formatted.Length());
}

bool acquire_libraries(RawResult& result) {
    const std::scoped_lock lock(libraries_mutex);

    if (library_users == 0) {
        Error error;
        P4Libraries::Initialize(kLibraryFlags, &error);
        if (error.Test()) {
            append_error(&result, &error);

            Error shutdown_error;
            P4Libraries::Shutdown(kLibraryFlags, &shutdown_error);
            append_error(&result, &shutdown_error);
            return false;
        }
    }

    ++library_users;
    return true;
}

void release_libraries(RawResult* result) {
    const std::scoped_lock lock(libraries_mutex);

    if (library_users == 0) {
        if (result != nullptr) {
            append_internal_error(*result, "P4API library shutdown was not paired with initialization");
        }
        return;
    }

    --library_users;
    if (library_users != 0) {
        return;
    }

    Error error;
    P4Libraries::Shutdown(kLibraryFlags, &error);
    append_error(result, &error);
}

class CancellationKeepAlive final : public KeepAlive {
public:
    explicit CancellationKeepAlive(const CancellationState& cancellation)
        : cancellation_(cancellation) {}

    int IsAlive() override {
        return cancellation_requested(cancellation_) ? 0 : 1;
    }

private:
    const CancellationState& cancellation_;
};

class CaptureClientUser final : public ClientUser {
public:
    explicit CaptureClientUser(RawResult& result) : result_(result) {}

    void HandleError(Error* error) override {
        append_error(&result_, error);
    }

    void Message(Error* error) override {
        append_error(&result_, error);
    }

    void OutputError(const char* text) override {
        const std::string value = text == nullptr ? std::string() : std::string(text);
        append_message(result_, E_FAILED, 0, value.data(), static_cast<int>(value.size()));
    }

    void OutputInfo(char, const char* text) override {
        const std::string value = text == nullptr ? std::string() : std::string(text);
        append_message(result_, E_INFO, 0, value.data(), static_cast<int>(value.size()));
    }

    void OutputBinary(const char* data, int length) override {
        append_output(data, length);
    }

    void OutputText(const char* data, int length) override {
        append_output(data, length);
    }

    void OutputStat(StrDict* variables) override {
        RawRecord record;
        StrRef name;
        StrRef value;

        for (int index = 0; variables->GetVar(index, name, value); ++index) {
            RawField field;
            field.name = rust_string(name.Text(), name.Length());
            append_bytes(field.value, value.Text(), value.Length());
            record.fields.push_back(std::move(field));
        }

        result_.records.push_back(std::move(record));
    }

private:
    void append_output(const char* data, int length) {
        append_bytes(result_.output, data, length);
    }

    RawResult& result_;
};

} // namespace

class NativeThread::Impl {
public:
    bool libraries_initialized = false;
    bool thread_initialized = false;
};

NativeThread::NativeThread(RawResult& result) : impl_(std::make_unique<Impl>()) {
    if (!acquire_libraries(result)) {
        return;
    }
    impl_->libraries_initialized = true;

    Error error;
    {
        const std::scoped_lock lock(libraries_mutex);
        P4Libraries::InitializeThread(kLibraryFlags, &error);
    }
    if (error.Test()) {
        append_error(&result, &error);

        Error shutdown_error;
        {
            const std::scoped_lock lock(libraries_mutex);
            P4Libraries::ShutdownThread(kLibraryFlags, &shutdown_error);
        }
        append_error(&result, &shutdown_error);
        release_libraries(&result);
        impl_->libraries_initialized = false;
        return;
    }

    impl_->thread_initialized = true;
    ++active_threads;
}

NativeThread::~NativeThread() {
    shutdown(nullptr);
}

bool NativeThread::ready() const {
    return impl_->libraries_initialized && impl_->thread_initialized;
}

void NativeThread::shutdown(RawResult& result) {
    shutdown(&result);
}

void NativeThread::shutdown(RawResult* result) {
    if (!impl_->libraries_initialized) {
        return;
    }

    if (active_clients != 0) {
        if (result != nullptr) {
            append_internal_error(
                *result,
                "P4API thread shutdown requires all native clients to be destroyed first");
        }
        return;
    }

    if (impl_->thread_initialized) {
        Error error;
        {
            const std::scoped_lock lock(libraries_mutex);
            P4Libraries::ShutdownThread(kLibraryFlags, &error);
        }
        append_error(result, &error);
        impl_->thread_initialized = false;
        --active_threads;
    }

    release_libraries(result);
    impl_->libraries_initialized = false;
}

class NativeClient::Impl {
public:
    ClientApi client;
    bool initialized = false;
};

NativeClient::NativeClient(rust::Str cwd, rust::Str port_override, RawResult& result)
    : impl_(nullptr) {
    if (active_threads == 0) {
        append_internal_error(result, "P4API client creation requires worker-thread initialization");
        return;
    }

    const std::scoped_lock lock(libraries_mutex);
    impl_ = std::make_unique<Impl>();
    ++active_clients;

    const std::string working_directory(cwd.data(), cwd.size());
    impl_->client.SetProtocol("tag", "");
    impl_->client.SetCwd(working_directory.c_str());

    if (!port_override.empty()) {
        const std::string server_address(port_override.data(), port_override.size());
        impl_->client.SetPort(server_address.c_str());
    }

    Error error;
    impl_->client.Init(&error);
    if (error.Test()) {
        append_error(&result, &error);
        return;
    }

    impl_->initialized = true;
}

NativeClient::~NativeClient() {
    if (impl_ == nullptr) {
        return;
    }

    {
        const std::scoped_lock lock(libraries_mutex);
        if (impl_->initialized) {
            Error error;
            impl_->client.Final(&error);
            impl_->initialized = false;
        }
        impl_.reset();
    }

    --active_clients;
}

void NativeClient::close(RawResult& result) {
    if (impl_ == nullptr || !impl_->initialized) {
        return;
    }

    const std::scoped_lock lock(libraries_mutex);
    Error error;
    impl_->client.Final(&error);
    impl_->initialized = false;
    append_error(&result, &error);
}

void NativeClient::run(rust::Str command,
                       rust::Slice<const rust::String> arguments,
                       const CancellationState& cancellation,
                       RawResult& result) {
    if (impl_ == nullptr || !impl_->initialized) {
        return;
    }

    std::vector<std::string> owned_arguments;
    owned_arguments.reserve(arguments.size());
    for (const auto& argument : arguments) {
        owned_arguments.emplace_back(argument.data(), argument.size());
    }

    std::vector<char*> argument_pointers;
    argument_pointers.reserve(owned_arguments.size());
    for (auto& argument : owned_arguments) {
        argument_pointers.push_back(argument.data());
    }

    CaptureClientUser user(result);
    CancellationKeepAlive keep_alive(cancellation);
    const std::string command_name(command.data(), command.size());

    const std::scoped_lock lock(libraries_mutex);
    impl_->client.SetBreak(&keep_alive);
    impl_->client.SetArgv(
        static_cast<int>(argument_pointers.size()),
        argument_pointers.empty() ? nullptr : argument_pointers.data());
    impl_->client.Run(command_name.c_str(), &user);
    impl_->client.SetBreak(nullptr);
}

bool NativeClient::connected() const {
    return impl_ != nullptr && impl_->initialized;
}

std::unique_ptr<NativeThread> start_thread(RawResult& result) {
    return std::make_unique<NativeThread>(result);
}

std::unique_ptr<NativeClient> connect(
    rust::Str cwd, rust::Str port_override, RawResult& result) {
    return std::make_unique<NativeClient>(cwd, port_override, result);
}

void capture_diagnostic(rust::Slice<const std::uint8_t> diagnostic, RawResult& result) {
    if (diagnostic.size() > static_cast<std::size_t>(std::numeric_limits<int>::max())) {
        append_internal_error(result, "P4API diagnostic exceeded the supported length");
        return;
    }

    append_message(
        result,
        E_FAILED,
        0,
        reinterpret_cast<const char*>(diagnostic.data()),
        static_cast<int>(diagnostic.size()));
}

} // namespace yori::p4
