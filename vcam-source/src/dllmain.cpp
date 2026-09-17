#include "pch.h"
#include "clsid.h"
#include "media_source.h"
#include "log.h"

DEFINE_GUID(MF_FRAMESERVER_CLIENTCONTEXT_CLIENTPID, 0x5f8d322e, 0x0fe4, 0x43e4, 0x9e, 0x50, 0xd8, 0x3e, 0xcd, 0x9f, 0xc2, 0xb8);

namespace
{
    HMODULE g_module = nullptr;

    DWORD SessionOf(DWORD processId)
    {
        DWORD session = 0;
        return ProcessIdToSessionId(processId, &session) ? session : 0;
    }

    std::string ProcessName(DWORD processId)
    {
        wil::unique_handle process(OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, FALSE, processId));
        std::array<wchar_t, MAX_PATH> path{};
        auto size = static_cast<DWORD>(path.size());
        if (!process || !QueryFullProcessImageNameW(process.get(), 0, path.data(), &size))
        {
            return "?";
        }
        const std::wstring_view full(path.data(), size);
        const auto name = full.substr(full.find_last_of(L'\\') + 1);
        std::string narrow;
        for (const auto character : name)
        {
            narrow.push_back(character < 128 ? static_cast<char>(character) : '?');
        }
        return narrow;
    }

    DWORD ProducerSession(IMFAttributes* attributes)
    {
        UINT32 clientPid = 0;
        if (SUCCEEDED(attributes->GetUINT32(MF_FRAMESERVER_CLIENTCONTEXT_CLIENTPID, &clientPid)) && clientPid)
        {
            if (const auto session = SessionOf(clientPid))
            {
                return session;
            }
        }
        if (const auto session = SessionOf(GetCurrentProcessId()))
        {
            return session;
        }
        return WTSGetActiveConsoleSessionId();
    }
}

struct Activator : winrt::implements<Activator, AttributesBase<IMFActivate>>
{
    HRESULT Initialize()
    {
        RETURN_IF_FAILED(SetUINT32(MF_VIRTUALCAMERA_PROVIDE_ASSOCIATED_CAMERA_SOURCES, 1));
        return SetGUID(MFT_TRANSFORM_CLSID_Attribute, CLSID_ChromaFreeCamera);
    }

    STDMETHODIMP ActivateObject(REFIID iid, void** object) override
    {
        RETURN_HR_IF_NULL(E_POINTER, object);
        *object = nullptr;
        winrt::slim_lock_guard lock(_lock);
        RETURN_HR_IF(MF_E_SHUTDOWN, _detached);
        if (!_source)
        {
            try
            {
                UINT32 clientPid = 0;
                LOG_IF_FAILED(GetUINT32(MF_FRAMESERVER_CLIENTCONTEXT_CLIENTPID, &clientPid));
                const auto session = ProducerSession(this);
                LogEvent("activate for client pid %u (%s), producer session %lu", clientPid, ProcessName(clientPid ? clientPid : GetCurrentProcessId()).c_str(), session);
                auto source = winrt::make_self<MediaSource>();
                RETURN_IF_FAILED(source->Initialize(this, session));
                _source = std::move(source);
            }
            CATCH_RETURN();
        }
        return _source->QueryInterface(iid, object);
    }

    STDMETHODIMP ShutdownObject() override
    {
        return S_OK;
    }

    STDMETHODIMP DetachObject() override
    {
        winrt::slim_lock_guard lock(_lock);
        _source = nullptr;
        _detached = true;
        return S_OK;
    }

private:
    winrt::slim_mutex _lock;
    winrt::com_ptr<MediaSource> _source;
    bool _detached = false;
};

namespace winrt
{
    template <> inline bool is_guid_of<AttributesBase<IMFActivate>>(guid const& id) noexcept
    {
        return is_guid_of<IMFActivate, IMFAttributes>(id);
    }
}

struct ClassFactory : winrt::implements<ClassFactory, IClassFactory>
{
    STDMETHODIMP CreateInstance(IUnknown* outer, REFIID iid, void** result) noexcept override
    {
        RETURN_HR_IF_NULL(E_POINTER, result);
        *result = nullptr;
        RETURN_HR_IF(CLASS_E_NOAGGREGATION, outer != nullptr);
        try
        {
            auto activator = winrt::make_self<Activator>();
            RETURN_IF_FAILED(activator->Initialize());
            return activator->QueryInterface(iid, result);
        }
        CATCH_RETURN();
    }

    STDMETHODIMP LockServer(BOOL) noexcept override
    {
        return S_OK;
    }
};

BOOL APIENTRY DllMain(HMODULE module, DWORD reason, LPVOID)
{
    if (reason == DLL_PROCESS_ATTACH)
    {
        g_module = module;
        DisableThreadLibraryCalls(module);
    }
    return TRUE;
}

STDAPI DllCanUnloadNow()
{
    if (winrt::get_module_lock())
    {
        return S_FALSE;
    }
    winrt::clear_factory_cache();
    return S_OK;
}

STDAPI DllGetClassObject(REFCLSID clsid, REFIID iid, LPVOID* object)
{
    RETURN_HR_IF_NULL(E_POINTER, object);
    *object = nullptr;
    RETURN_HR_IF(CLASS_E_CLASSNOTAVAILABLE, clsid != CLSID_ChromaFreeCamera);
    try
    {
        return winrt::make<ClassFactory>().as<IUnknown>()->QueryInterface(iid, object);
    }
    CATCH_RETURN();
}

STDAPI DllRegisterServer()
{
    try
    {
        const auto modulePath = wil::GetModuleFileNameW<std::wstring>(g_module);
        const auto keyPath = std::format(L"Software\\Classes\\CLSID\\{}\\InprocServer32", ChromaFreeCameraClsidString);
        wil::unique_hkey key;
        RETURN_IF_WIN32_ERROR(RegCreateKeyExW(HKEY_LOCAL_MACHINE, keyPath.c_str(), 0, nullptr, 0, KEY_WRITE, nullptr, &key, nullptr));
        RETURN_IF_WIN32_ERROR(RegSetValueExW(key.get(), nullptr, 0, REG_SZ, reinterpret_cast<const BYTE*>(modulePath.c_str()), static_cast<DWORD>((modulePath.size() + 1) * sizeof(wchar_t))));
        constexpr wchar_t threadingModel[] = L"Both";
        RETURN_IF_WIN32_ERROR(RegSetValueExW(key.get(), L"ThreadingModel", 0, REG_SZ, reinterpret_cast<const BYTE*>(threadingModel), sizeof(threadingModel)));
        return S_OK;
    }
    CATCH_RETURN();
}

STDAPI DllUnregisterServer()
{
    const auto keyPath = std::format(L"Software\\Classes\\CLSID\\{}", ChromaFreeCameraClsidString);
    const auto status = RegDeleteTreeW(HKEY_LOCAL_MACHINE, keyPath.c_str());
    RETURN_HR_IF(HRESULT_FROM_WIN32(status), status != ERROR_SUCCESS && status != ERROR_FILE_NOT_FOUND);
    return S_OK;
}
