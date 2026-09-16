#include "p4_bridge.h"
#include "yori-p4/src/lib.rs.h"

#include <p4/clientapi.h>

#include <cstdint>
#include <string>
#include <utility>
#include <vector>

namespace yori::p4 {
namespace {

rust::String rust_string(const char* data, int length) {
    if (data == nullptr || length <= 0) {
        return rust::String();
    }

    return rust::String(data, static_cast<std::size_t>(length));
}

void append_message(RawResult& result, int severity, int generic, const char* text, int length) {
    RawMessage message;
    message.severity = severity;
    message.generic = generic;
    message.text = rust_string(text, length);
    result.messages.push_back(std::move(message));
}

void append_error(RawResult& result, Error* error) {
    StrBuf formatted;
    error->Fmt(&formatted, EF_PLAIN);
    append_message(
        result,
        error->GetSeverity(),
        error->GetGeneric(),
        formatted.Text(),
        formatted.Length());
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
        append_error(result_, error);
    }

    void Message(Error* error) override {
        append_error(result_, error);
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

            const auto length = static_cast<std::size_t>(value.Length());
            const auto* begin = reinterpret_cast<const std::uint8_t*>(value.Text());
            field.value.reserve(length);
            for (std::size_t offset = 0; offset < length; ++offset) {
                field.value.push_back(begin[offset]);
            }

            record.fields.push_back(std::move(field));
        }

        result_.records.push_back(std::move(record));
    }

private:
    void append_output(const char* data, int length) {
        if (data == nullptr || length <= 0) {
            return;
        }

        const auto* begin = reinterpret_cast<const std::uint8_t*>(data);
        result_.output.reserve(result_.output.size() + static_cast<std::size_t>(length));
        for (int index = 0; index < length; ++index) {
            result_.output.push_back(begin[index]);
        }
    }

    RawResult& result_;
};

} // namespace

class NativeClient::Impl {
public:
    ClientApi client;
    bool initialized = false;
};

NativeClient::NativeClient(rust::Str cwd, rust::Str port_override, RawResult& result)
    : impl_(std::make_unique<Impl>()) {
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
        append_error(result, &error);
        return;
    }

    impl_->initialized = true;
}

NativeClient::~NativeClient() {
    if (!impl_->initialized) {
        return;
    }

    Error error;
    impl_->client.Final(&error);
}

void NativeClient::run(rust::Str command,
                       rust::Slice<const rust::String> arguments,
                       const CancellationState& cancellation,
                       RawResult& result) {
    if (!impl_->initialized) {
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

    impl_->client.SetBreak(&keep_alive);
    impl_->client.SetArgv(
        static_cast<int>(argument_pointers.size()),
        argument_pointers.empty() ? nullptr : argument_pointers.data());
    impl_->client.Run(command_name.c_str(), &user);
    impl_->client.SetBreak(nullptr);
}

bool NativeClient::connected() const {
    return impl_->initialized;
}

std::unique_ptr<NativeClient> connect(
    rust::Str cwd, rust::Str port_override, RawResult& result) {
    return std::make_unique<NativeClient>(cwd, port_override, result);
}

} // namespace yori::p4
