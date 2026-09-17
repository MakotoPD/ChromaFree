#include "pch.h"
#include "log.h"

namespace
{
    constexpr LONGLONG MaxLogBytes = 1024 * 1024;

    std::wstring LogPath()
    {
        std::array<wchar_t, MAX_PATH> buffer{};
        const auto length = ExpandEnvironmentStringsW(L"%ProgramData%\\ChromaFree\\vcam-source.log", buffer.data(), static_cast<DWORD>(buffer.size()));
        return length == 0 || length > buffer.size() ? std::wstring{} : std::wstring(buffer.data());
    }

    winrt::slim_mutex& LogLock()
    {
        static winrt::slim_mutex lock;
        return lock;
    }
}

void LogEvent(const char* format, ...)
{
    const auto path = LogPath();
    if (path.empty())
    {
        return;
    }
    std::array<char, 512> message{};
    va_list arguments;
    va_start(arguments, format);
    _vsnprintf_s(message.data(), message.size(), _TRUNCATE, format, arguments);
    va_end(arguments);

    SYSTEMTIME time;
    GetLocalTime(&time);
    const auto line = std::format("{:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:03} pid={} {}\r\n", time.wYear, time.wMonth, time.wDay, time.wHour, time.wMinute,
                                  time.wSecond, time.wMilliseconds, GetCurrentProcessId(), message.data());

    winrt::slim_lock_guard lock(LogLock());
    WIN32_FILE_ATTRIBUTE_DATA attributes{};
    const bool tooLarge = GetFileAttributesExW(path.c_str(), GetFileExInfoStandard, &attributes) &&
                          ((static_cast<LONGLONG>(attributes.nFileSizeHigh) << 32) | attributes.nFileSizeLow) > MaxLogBytes;
    wil::unique_hfile file(CreateFileW(path.c_str(), tooLarge ? GENERIC_WRITE : FILE_APPEND_DATA, FILE_SHARE_READ | FILE_SHARE_WRITE, nullptr,
                                       tooLarge ? CREATE_ALWAYS : OPEN_ALWAYS, FILE_ATTRIBUTE_NORMAL, nullptr));
    if (file)
    {
        DWORD written = 0;
        WriteFile(file.get(), line.data(), static_cast<DWORD>(line.size()), &written, nullptr);
    }
}
