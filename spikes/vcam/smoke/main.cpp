#define WIN32_LEAN_AND_MEAN
#define NOMINMAX

#include <windows.h>
#include <mfapi.h>
#include <mferror.h>
#include <mfidl.h>

#include <chrono>
#include <cstdio>
#include <string>

#include <wil/com.h>
#include <wil/resource.h>
#include <wil/result.h>

#include "../source/clsid.h"
#include "../source/spike_protocol.h"

namespace
{
    using GetClassObjectFunction = HRESULT(STDAPICALLTYPE*)(REFCLSID, REFIID, LPVOID*);

    wil::com_ptr<IMFMediaEvent> WaitForEvent(IMFMediaEventGenerator* generator, MediaEventType wanted)
    {
        for (;;)
        {
            wil::com_ptr<IMFMediaEvent> event;
            THROW_IF_FAILED(generator->GetEvent(0, &event));
            MediaEventType type = MEUnknown;
            THROW_IF_FAILED(event->GetType(&type));
            HRESULT status = S_OK;
            THROW_IF_FAILED(event->GetStatus(&status));
            THROW_IF_FAILED(status);
            if (type == wanted)
            {
                return event;
            }
        }
    }

    struct FormatResult
    {
        int samples = 0;
        double requestMs = 0;
        BYTE firstByte = 0;
        BYTE lastFirstByte = 0;
        LONG pitch = 0;
    };

    FormatResult StreamFormat(IMFMediaSource* source, DWORD typeIndex, int frames)
    {
        wil::com_ptr<IMFPresentationDescriptor> presentation;
        THROW_IF_FAILED(source->CreatePresentationDescriptor(&presentation));
        BOOL selected = FALSE;
        wil::com_ptr<IMFStreamDescriptor> streamDescriptor;
        THROW_IF_FAILED(presentation->GetStreamDescriptorByIndex(0, &selected, &streamDescriptor));
        THROW_IF_FAILED(presentation->SelectStream(0));
        wil::com_ptr<IMFMediaTypeHandler> handler;
        THROW_IF_FAILED(streamDescriptor->GetMediaTypeHandler(&handler));
        wil::com_ptr<IMFMediaType> type;
        THROW_IF_FAILED(handler->GetMediaTypeByIndex(typeIndex, &type));
        THROW_IF_FAILED(handler->SetCurrentMediaType(type.get()));

        PROPVARIANT start{};
        THROW_IF_FAILED(source->Start(presentation.get(), nullptr, &start));
        auto newStream = WaitForEvent(source, MENewStream);
        wil::unique_prop_variant value;
        THROW_IF_FAILED(newStream->GetValue(&value));
        auto stream = wil::com_query<IMFMediaStream>(value.punkVal);
        WaitForEvent(source, MESourceStarted);
        WaitForEvent(stream.get(), MEStreamStarted);

        FormatResult result;
        for (int i = 0; i < frames; i++)
        {
            const auto begin = std::chrono::steady_clock::now();
            THROW_IF_FAILED(stream->RequestSample(nullptr));
            auto event = WaitForEvent(stream.get(), MEMediaSample);
            result.requestMs += std::chrono::duration<double, std::milli>(std::chrono::steady_clock::now() - begin).count();

            wil::unique_prop_variant sampleValue;
            THROW_IF_FAILED(event->GetValue(&sampleValue));
            auto sample = wil::com_query<IMFSample>(sampleValue.punkVal);
            wil::com_ptr<IMFMediaBuffer> buffer;
            THROW_IF_FAILED(sample->GetBufferByIndex(0, &buffer));
            auto buffer2d = buffer.query<IMF2DBuffer2>();
            BYTE* scanline0 = nullptr;
            BYTE* bufferStart = nullptr;
            DWORD length = 0;
            THROW_IF_FAILED(buffer2d->Lock2DSize(MF2DBuffer_LockFlags_Read, &scanline0, &result.pitch, &bufferStart, &length));
            if (i == 0)
            {
                result.firstByte = scanline0[0];
            }
            result.lastFirstByte = scanline0[spike::FrameWidth / 2];
            THROW_IF_FAILED(buffer2d->Unlock2D());
            result.samples++;
            Sleep(1000 / spike::FrameRate);
        }

        THROW_IF_FAILED(source->Stop());
        WaitForEvent(source, MESourceStopped);
        result.requestMs /= std::max(result.samples, 1);
        return result;
    }
}

int wmain(int argc, wchar_t** argv)
{
    try
    {
        const std::wstring dllPath = argc > 1 ? argv[1] : L"vcam-spike.dll";
        const int frames = argc > 2 ? _wtoi(argv[2]) : 90;
        auto com = wil::CoInitializeEx(COINIT_MULTITHREADED);
        THROW_IF_FAILED(MFStartup(MF_VERSION));
        auto shutdown = wil::scope_exit([] { MFShutdown(); });

        wil::unique_hmodule module(LoadLibraryW(dllPath.c_str()));
        THROW_LAST_ERROR_IF_NULL(module);
        auto getClassObject = reinterpret_cast<GetClassObjectFunction>(GetProcAddress(module.get(), "DllGetClassObject"));
        THROW_LAST_ERROR_IF_NULL(getClassObject);

        {
            wil::com_ptr<IClassFactory> factory;
            THROW_IF_FAILED(getClassObject(CLSID_BgcamSpikeCamera, IID_IClassFactory, factory.put_void()));
            wil::com_ptr<IMFActivate> activate;
            THROW_IF_FAILED(factory->CreateInstance(nullptr, IID_PPV_ARGS(&activate)));
            wil::com_ptr<IMFMediaSource> source;
            THROW_IF_FAILED(activate->ActivateObject(IID_PPV_ARGS(&source)));

            wil::com_ptr<IMFVideoSampleAllocatorEx> allocator;
            THROW_IF_FAILED(MFCreateVideoSampleAllocatorEx(IID_PPV_ARGS(&allocator)));
            THROW_IF_FAILED(source.query<IMFSampleAllocatorControl>()->SetDefaultAllocator(0, allocator.get()));

            int failures = 0;
            for (DWORD typeIndex : { 0ul, 1ul })
            {
                const auto result = StreamFormat(source.get(), typeIndex, frames);
                const bool ok = result.samples == frames;
                failures += ok ? 0 : 1;
                wprintf(L"%s %s: samples=%d/%d request avg=%.2fms pitch=%ld first pixel byte=%u last center byte=%u\n",
                    ok ? L"ok  " : L"FAIL", typeIndex == 0 ? L"NV12 " : L"RGB32", result.samples, frames, result.requestMs, result.pitch,
                    result.firstByte, result.lastFirstByte);
            }

            THROW_IF_FAILED(source->Shutdown());
            THROW_IF_FAILED(activate->ShutdownObject());
            if (failures)
            {
                return 1;
            }
        }
        return 0;
    }
    catch (const wil::ResultException& e)
    {
        fwprintf(stderr, L"FAIL: 0x%08X\n", static_cast<unsigned>(e.GetErrorCode()));
        return 1;
    }
}
