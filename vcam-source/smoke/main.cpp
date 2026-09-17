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

#include "../src/clsid.h"

namespace
{
    using GetClassObjectFunction = HRESULT(STDAPICALLTYPE*)(REFCLSID, REFIID, LPVOID*);
    using Clock = std::chrono::steady_clock;

    constexpr DWORD RequestIntervalMilliseconds = 1;

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

    wil::com_ptr<IMFMediaEvent> TryGetEvent(IMFMediaEventGenerator* generator)
    {
        wil::com_ptr<IMFMediaEvent> event;
        const auto hr = generator->GetEvent(MF_EVENT_FLAG_NO_WAIT, &event);
        if (hr == MF_E_NO_EVENTS_AVAILABLE)
        {
            return nullptr;
        }
        THROW_IF_FAILED(hr);
        return event;
    }

    DWORD FindType(IMFMediaTypeHandler* handler, REFGUID subtype)
    {
        DWORD count = 0;
        THROW_IF_FAILED(handler->GetMediaTypeCount(&count));
        for (DWORD index = 0; index < count; index++)
        {
            wil::com_ptr<IMFMediaType> type;
            THROW_IF_FAILED(handler->GetMediaTypeByIndex(index, &type));
            GUID candidate{};
            THROW_IF_FAILED(type->GetGUID(MF_MT_SUBTYPE, &candidate));
            if (candidate == subtype)
            {
                return index;
            }
        }
        THROW_HR(MF_E_INVALIDMEDIATYPE);
    }

    void PrintSample(IMFSample* sample, double elapsedMs)
    {
        LONGLONG sampleTime = 0;
        THROW_IF_FAILED(sample->GetSampleTime(&sampleTime));
        const auto latencyMs = static_cast<double>(MFGetSystemTime() - sampleTime) / 10'000.0;
        wil::com_ptr<IMFMediaBuffer> buffer;
        THROW_IF_FAILED(sample->GetBufferByIndex(0, &buffer));
        auto buffer2d = buffer.query<IMF2DBuffer2>();
        BYTE* scanline0 = nullptr;
        LONG pitch = 0;
        BYTE* bufferStart = nullptr;
        DWORD length = 0;
        THROW_IF_FAILED(buffer2d->Lock2DSize(MF2DBuffer_LockFlags_Read, &scanline0, &pitch, &bufferStart, &length));
        wprintf(L"sample t=%.2f latency=%.2f b0=%u b3=%u\n", elapsedMs, latencyMs, scanline0[0], scanline0[3]);
        THROW_IF_FAILED(buffer2d->Unlock2D());
    }
}

int wmain(int argc, wchar_t** argv)
{
    try
    {
        if (argc < 4)
        {
            fwprintf(stderr, L"usage: vcam-source-smoke <dll> <nv12|argb32> <seconds>\n");
            return 2;
        }
        const std::wstring dllPath = argv[1];
        const bool argb = std::wstring(argv[2]) == L"argb32";
        const auto duration = std::chrono::duration<double>(_wtof(argv[3]));

        auto com = wil::CoInitializeEx(COINIT_MULTITHREADED);
        THROW_IF_FAILED(MFStartup(MF_VERSION));
        auto shutdown = wil::scope_exit([] { MFShutdown(); });

        wil::unique_hmodule module(LoadLibraryW(dllPath.c_str()));
        THROW_LAST_ERROR_IF_NULL(module);
        auto getClassObject = reinterpret_cast<GetClassObjectFunction>(GetProcAddress(module.get(), "DllGetClassObject"));
        THROW_LAST_ERROR_IF_NULL(getClassObject);

        wil::com_ptr<IClassFactory> factory;
        THROW_IF_FAILED(getClassObject(CLSID_ChromaFreeCamera, IID_IClassFactory, factory.put_void()));
        wil::com_ptr<IMFActivate> activate;
        THROW_IF_FAILED(factory->CreateInstance(nullptr, IID_PPV_ARGS(&activate)));
        wil::com_ptr<IMFMediaSource> source;
        THROW_IF_FAILED(activate->ActivateObject(IID_PPV_ARGS(&source)));

        wil::com_ptr<IMFVideoSampleAllocatorEx> allocator;
        THROW_IF_FAILED(MFCreateVideoSampleAllocatorEx(IID_PPV_ARGS(&allocator)));
        THROW_IF_FAILED(source.query<IMFSampleAllocatorControl>()->SetDefaultAllocator(0, allocator.get()));

        wil::com_ptr<IMFPresentationDescriptor> presentation;
        THROW_IF_FAILED(source->CreatePresentationDescriptor(&presentation));
        BOOL selected = FALSE;
        wil::com_ptr<IMFStreamDescriptor> streamDescriptor;
        THROW_IF_FAILED(presentation->GetStreamDescriptorByIndex(0, &selected, &streamDescriptor));
        THROW_IF_FAILED(presentation->SelectStream(0));
        wil::com_ptr<IMFMediaTypeHandler> handler;
        THROW_IF_FAILED(streamDescriptor->GetMediaTypeHandler(&handler));
        wil::com_ptr<IMFMediaType> type;
        THROW_IF_FAILED(handler->GetMediaTypeByIndex(FindType(handler.get(), argb ? MFVideoFormat_ARGB32 : MFVideoFormat_NV12), &type));
        THROW_IF_FAILED(handler->SetCurrentMediaType(type.get()));
        UINT32 width = 0;
        UINT32 height = 0;
        THROW_IF_FAILED(MFGetAttributeSize(type.get(), MF_MT_FRAME_SIZE, &width, &height));
        UINT32 numerator = 0;
        UINT32 denominator = 0;
        THROW_IF_FAILED(MFGetAttributeRatio(type.get(), MF_MT_FRAME_RATE, &numerator, &denominator));
        wprintf(L"type width=%u height=%u fps=%u/%u\n", width, height, numerator, denominator);
        fflush(stdout);

        PROPVARIANT start{};
        THROW_IF_FAILED(source->Start(presentation.get(), nullptr, &start));
        auto newStream = WaitForEvent(source.get(), MENewStream);
        wil::unique_prop_variant value;
        THROW_IF_FAILED(newStream->GetValue(&value));
        auto stream = wil::com_query<IMFMediaStream>(value.punkVal);
        WaitForEvent(source.get(), MESourceStarted);
        WaitForEvent(stream.get(), MEStreamStarted);

        const auto begin = Clock::now();
        uint64_t requests = 0;
        uint64_t samples = 0;
        while (Clock::now() - begin < duration)
        {
            THROW_IF_FAILED(stream->RequestSample(nullptr));
            requests++;
            while (auto event = TryGetEvent(stream.get()))
            {
                MediaEventType eventType = MEUnknown;
                THROW_IF_FAILED(event->GetType(&eventType));
                if (eventType != MEMediaSample)
                {
                    continue;
                }
                wil::unique_prop_variant sampleValue;
                THROW_IF_FAILED(event->GetValue(&sampleValue));
                PrintSample(wil::com_query<IMFSample>(sampleValue.punkVal).get(), std::chrono::duration<double, std::milli>(Clock::now() - begin).count());
                samples++;
            }
            fflush(stdout);
            Sleep(RequestIntervalMilliseconds);
        }

        THROW_IF_FAILED(source->Stop());
        WaitForEvent(source.get(), MESourceStopped);
        THROW_IF_FAILED(source->Shutdown());
        THROW_IF_FAILED(activate->ShutdownObject());
        wprintf(L"summary samples=%llu requests=%llu\n", samples, requests);
        return 0;
    }
    catch (const wil::ResultException& e)
    {
        fwprintf(stderr, L"FAIL: 0x%08X\n", static_cast<unsigned>(e.GetErrorCode()));
        return 1;
    }
}
