#include "pch.h"
#include "log.h"

namespace
{
    std::wstring LogFilePath()
    {
        std::array<wchar_t, MAX_PATH> buffer{};
        if (GetEnvironmentVariableW(L"BGCAM_SPIKE_LOG_DIR", buffer.data(), static_cast<DWORD>(buffer.size())) == 0)
        {
            ExpandEnvironmentStringsW(L"%ProgramData%\\bgcam-spike", buffer.data(), static_cast<DWORD>(buffer.size()));
        }
        return std::format(L"{}\\vcam-{}.log", buffer.data(), GetCurrentProcessId());
    }

    std::wstring SidToString(PSID sid)
    {
        wil::unique_hlocal_string text;
        if (!ConvertSidToStringSidW(sid, &text))
        {
            return L"?";
        }
        return text.get();
    }

    std::vector<BYTE> TokenInformation(HANDLE token, TOKEN_INFORMATION_CLASS type)
    {
        DWORD size = 0;
        GetTokenInformation(token, type, nullptr, 0, &size);
        std::vector<BYTE> data(size);
        if (size == 0 || !GetTokenInformation(token, type, data.data(), size, &size))
        {
            data.clear();
        }
        return data;
    }

    std::wstring CreateGlobalPrivilegeState(HANDLE token)
    {
        LUID luid{};
        if (!LookupPrivilegeValueW(nullptr, SE_CREATE_GLOBAL_NAME, &luid))
        {
            return L"lookup failed";
        }
        auto data = TokenInformation(token, TokenPrivileges);
        if (data.empty())
        {
            return L"query failed";
        }
        auto privileges = reinterpret_cast<TOKEN_PRIVILEGES*>(data.data());
        for (DWORD i = 0; i < privileges->PrivilegeCount; i++)
        {
            auto& entry = privileges->Privileges[i];
            if (entry.Luid.LowPart == luid.LowPart && entry.Luid.HighPart == luid.HighPart)
            {
                return (entry.Attributes & SE_PRIVILEGE_ENABLED) ? L"present, enabled" : L"present, disabled";
            }
        }
        return L"absent";
    }
}

void LogLine(const wchar_t* format, ...)
{
    std::array<wchar_t, 2048> message{};
    va_list args;
    va_start(args, format);
    _vsnwprintf_s(message.data(), message.size(), _TRUNCATE, format, args);
    va_end(args);

    SYSTEMTIME time;
    GetLocalTime(&time);
    auto line = std::format(L"{:02}:{:02}:{:02}.{:03} [{}] {}\r\n", time.wHour, time.wMinute, time.wSecond, time.wMilliseconds, GetCurrentThreadId(), message.data());

    auto bytes = WideCharToMultiByte(CP_UTF8, 0, line.c_str(), static_cast<int>(line.size()), nullptr, 0, nullptr, nullptr);
    std::string utf8(static_cast<size_t>(bytes), '\0');
    WideCharToMultiByte(CP_UTF8, 0, line.c_str(), static_cast<int>(line.size()), utf8.data(), bytes, nullptr, nullptr);

    wil::unique_hfile file(CreateFileW(LogFilePath().c_str(), FILE_APPEND_DATA, FILE_SHARE_READ | FILE_SHARE_WRITE, nullptr, OPEN_ALWAYS, FILE_ATTRIBUTE_NORMAL, nullptr));
    if (file)
    {
        DWORD written = 0;
        WriteFile(file.get(), utf8.data(), static_cast<DWORD>(utf8.size()), &written, nullptr);
    }
}

std::wstring Win32ErrorText(DWORD error)
{
    if (error == ERROR_SUCCESS)
    {
        return L"ok";
    }
    std::array<wchar_t, 256> text{};
    FormatMessageW(FORMAT_MESSAGE_FROM_SYSTEM | FORMAT_MESSAGE_IGNORE_INSERTS, nullptr, error, 0, text.data(), static_cast<DWORD>(text.size()), nullptr);
    std::wstring result = text.data();
    while (!result.empty() && (result.back() == L'\n' || result.back() == L'\r' || result.back() == L' '))
    {
        result.pop_back();
    }
    return std::format(L"error {} ({})", error, result);
}

std::wstring ProcessDiagnostics()
{
    std::array<wchar_t, MAX_PATH> image{};
    GetModuleFileNameW(nullptr, image.data(), static_cast<DWORD>(image.size()));
    DWORD session = 0;
    ProcessIdToSessionId(GetCurrentProcessId(), &session);

    wil::unique_handle token;
    if (!OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &token))
    {
        return std::format(L"image={} pid={} session={} token=unavailable", image.data(), GetCurrentProcessId(), session);
    }

    auto user = TokenInformation(token.get(), TokenUser);
    auto integrity = TokenInformation(token.get(), TokenIntegrityLevel);
    auto appContainer = TokenInformation(token.get(), TokenIsAppContainer);

    std::wstring userSid = user.empty() ? L"?" : SidToString(reinterpret_cast<TOKEN_USER*>(user.data())->User.Sid);
    DWORD integrityRid = 0;
    if (!integrity.empty())
    {
        auto sid = reinterpret_cast<TOKEN_MANDATORY_LABEL*>(integrity.data())->Label.Sid;
        integrityRid = *GetSidSubAuthority(sid, *GetSidSubAuthorityCount(sid) - 1);
    }
    bool isAppContainer = !appContainer.empty() && *reinterpret_cast<DWORD*>(appContainer.data()) != 0;

    return std::format(
        L"image={} pid={} session={} user={} integrity=0x{:x} appcontainer={} SeCreateGlobalPrivilege={} activeConsoleSession={}",
        image.data(), GetCurrentProcessId(), session, userSid, integrityRid, isAppContainer, CreateGlobalPrivilegeState(token.get()), WTSGetActiveConsoleSessionId());
}

int64_t QpcNow()
{
    LARGE_INTEGER value;
    QueryPerformanceCounter(&value);
    return value.QuadPart;
}

double QpcToMilliseconds(int64_t ticks)
{
    static const int64_t frequency = []
    {
        LARGE_INTEGER value;
        QueryPerformanceFrequency(&value);
        return value.QuadPart;
    }();
    return static_cast<double>(ticks) * 1000.0 / static_cast<double>(frequency);
}
