#include "pch.h"
#include "clsid.h"
#include "log.h"
#include "media_source.h"

DEFINE_GUID(MF_FRAMESERVER_CLIENTCONTEXT_CLIENTPID, 0x5f8d322e, 0x0fe4, 0x43e4, 0x9e, 0x50, 0xd8, 0x3e, 0xcd, 0x9f, 0xc2, 0xb8);

namespace
{
    HMODULE g_module = nullptr;

    std::wstring ProcessImageName(DWORD pid)
    {
        wil::unique_handle process(OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, FALSE, pid));
        if (!process)
        {
            return Win32ErrorText(GetLastError());
        }
        std::array<wchar_t, MAX_PATH> name{};
        auto size = static_cast<DWORD>(name.size());
        return QueryFullProcessImageNameW(process.get(), 0, name.data(), &size) ? std::wstring(name.data()) : Win32ErrorText(GetLastError());
    }
}

struct Activator : winrt::implements<Activator, AttributesBase<IMFActivate>>
{
    HRESULT Initialize()
    {
        _source = winrt::make_self<MediaSource>();
        RETURN_IF_FAILED(SetUINT32(MF_VIRTUALCAMERA_PROVIDE_ASSOCIATED_CAMERA_SOURCES, 1));
        RETURN_IF_FAILED(SetGUID(MFT_TRANSFORM_CLSID_Attribute, CLSID_ChromaFreeSpikeCamera));
        return _source->Initialize(this);
    }

    STDMETHODIMP ActivateObject(REFIID iid, void** object) override
    {
        RETURN_HR_IF_NULL(E_POINTER, object);
        *object = nullptr;
        UINT32 clientPid = 0;
        if (SUCCEEDED(GetUINT32(MF_FRAMESERVER_CLIENTCONTEXT_CLIENTPID, &clientPid)) && clientPid)
        {
            LogLine(L"activate for client pid=%u image=%s", clientPid, ProcessImageName(clientPid).c_str());
        }
        RETURN_HR_IF(MF_E_SHUTDOWN, !_source);
        return _source->QueryInterface(iid, object);
    }

    STDMETHODIMP ShutdownObject() override
    {
        return S_OK;
    }

    STDMETHODIMP DetachObject() override
    {
        _source = nullptr;
        return S_OK;
    }

private:
    winrt::com_ptr<MediaSource> _source;
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
        LogLine(L"create instance %s", ProcessDiagnostics().c_str());
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
    RETURN_HR_IF(CLASS_E_CLASSNOTAVAILABLE, clsid != CLSID_ChromaFreeSpikeCamera);
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
        const auto keyPath = std::format(L"Software\\Classes\\CLSID\\{}\\InprocServer32", ChromaFreeSpikeCameraClsidString);
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
    const auto keyPath = std::format(L"Software\\Classes\\CLSID\\{}", ChromaFreeSpikeCameraClsidString);
    const auto status = RegDeleteTreeW(HKEY_LOCAL_MACHINE, keyPath.c_str());
    RETURN_HR_IF(HRESULT_FROM_WIN32(status), status != ERROR_SUCCESS && status != ERROR_FILE_NOT_FOUND);
    return S_OK;
}
