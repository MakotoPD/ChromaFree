#define WIN32_LEAN_AND_MEAN
#define NOMINMAX

#include <windows.h>
#include <mfapi.h>
#include <mfvirtualcamera.h>

#include <cstdio>

#include <wil/com.h>
#include <wil/resource.h>
#include <wil/result.h>

#include "../source/clsid.h"

int wmain()
{
    auto com = wil::CoInitializeEx(COINIT_MULTITHREADED);
    auto hr = MFStartup(MF_VERSION);
    if (FAILED(hr))
    {
        fwprintf(stderr, L"MFStartup failed: 0x%08X\n", static_cast<unsigned>(hr));
        return 1;
    }
    auto shutdown = wil::scope_exit([] { MFShutdown(); });

    wil::com_ptr_nothrow<IMFVirtualCamera> camera;
    hr = MFCreateVirtualCamera(
        MFVirtualCameraType_SoftwareCameraSource,
        MFVirtualCameraLifetime_Session,
        MFVirtualCameraAccess_CurrentUser,
        BgcamSpikeCameraName,
        BgcamSpikeCameraClsidString,
        nullptr,
        0,
        &camera);
    if (FAILED(hr))
    {
        fwprintf(stderr, L"MFCreateVirtualCamera failed: 0x%08X\n", static_cast<unsigned>(hr));
        return 1;
    }

    hr = camera->Start(nullptr);
    if (FAILED(hr))
    {
        fwprintf(stderr, L"IMFVirtualCamera::Start failed: 0x%08X (is the DLL registered from a location readable by LocalService?)\n", static_cast<unsigned>(hr));
        return 1;
    }

    wprintf(L"Virtual camera \"%s\" is running. Press Enter to remove it.\n", BgcamSpikeCameraName);
    getwchar();

    hr = camera->Remove();
    wprintf(L"Remove: 0x%08X\n", static_cast<unsigned>(hr));
    return SUCCEEDED(hr) ? 0 : 1;
}
