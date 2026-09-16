#include "pch.h"
#include "media_stream.h"
#include "log.h"

namespace
{
    constexpr uint64_t HundredNanosecondsPerSecond = 10'000'000;

    HRESULT CreateVideoType(REFGUID subtype, UINT32 bitsPerPixel, UINT32 stride, IMFMediaType** type)
    {
        wil::com_ptr_nothrow<IMFMediaType> result;
        RETURN_IF_FAILED(MFCreateMediaType(&result));
        RETURN_IF_FAILED(result->SetGUID(MF_MT_MAJOR_TYPE, MFMediaType_Video));
        RETURN_IF_FAILED(result->SetGUID(MF_MT_SUBTYPE, subtype));
        RETURN_IF_FAILED(MFSetAttributeSize(result.get(), MF_MT_FRAME_SIZE, spike::FrameWidth, spike::FrameHeight));
        RETURN_IF_FAILED(MFSetAttributeRatio(result.get(), MF_MT_FRAME_RATE, spike::FrameRate, 1));
        RETURN_IF_FAILED(MFSetAttributeRatio(result.get(), MF_MT_PIXEL_ASPECT_RATIO, 1, 1));
        RETURN_IF_FAILED(result->SetUINT32(MF_MT_DEFAULT_STRIDE, stride));
        RETURN_IF_FAILED(result->SetUINT32(MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive));
        RETURN_IF_FAILED(result->SetUINT32(MF_MT_ALL_SAMPLES_INDEPENDENT, TRUE));
        RETURN_IF_FAILED(result->SetUINT32(MF_MT_AVG_BITRATE, spike::FrameWidth * spike::FrameHeight * bitsPerPixel * spike::FrameRate));
        *type = result.detach();
        return S_OK;
    }
}

HRESULT MediaStream::Initialize(IMFMediaSource* source, DWORD streamId)
{
    RETURN_HR_IF_NULL(E_POINTER, source);
    _source = source;

    RETURN_IF_FAILED(SetGUID(MF_DEVICESTREAM_STREAM_CATEGORY, PINNAME_VIDEO_CAPTURE));
    RETURN_IF_FAILED(SetUINT32(MF_DEVICESTREAM_STREAM_ID, streamId));
    RETURN_IF_FAILED(SetUINT32(MF_DEVICESTREAM_FRAMESERVER_SHARED, 1));
    RETURN_IF_FAILED(SetUINT32(MF_DEVICESTREAM_ATTRIBUTE_FRAMESOURCE_TYPES, MFFrameSourceTypes_Color));
    RETURN_IF_FAILED(MFCreateEventQueue(&_queue));

    std::array<IMFMediaType*, 2> types{};
    RETURN_IF_FAILED(CreateVideoType(MFVideoFormat_NV12, 12, spike::FrameWidth, &types[0]));
    RETURN_IF_FAILED(CreateVideoType(MFVideoFormat_RGB32, 32, spike::FrameWidth * 4, &types[1]));
    auto release = wil::scope_exit([&] {
        for (auto* type : types)
        {
            type->Release();
        }
    });

    RETURN_IF_FAILED(MFCreateStreamDescriptor(streamId, static_cast<DWORD>(types.size()), types.data(), &_descriptor));
    wil::com_ptr_nothrow<IMFMediaTypeHandler> handler;
    RETURN_IF_FAILED(_descriptor->GetMediaTypeHandler(&handler));
    RETURN_IF_FAILED(handler->SetCurrentMediaType(types[0]));
    return S_OK;
}

HRESULT MediaStream::Start(IMFMediaType* type)
{
    RETURN_HR_IF(MF_E_SHUTDOWN, !_queue || !_allocator);
    if (type)
    {
        _currentType = type;
    }
    RETURN_HR_IF_NULL(MF_E_INVALIDMEDIATYPE, _currentType);

    GUID subtype{};
    RETURN_IF_FAILED(_currentType->GetGUID(MF_MT_SUBTYPE, &subtype));
    _frames.Start(subtype == MFVideoFormat_RGB32 ? OutputFormat::Rgb32 : OutputFormat::Nv12);

    RETURN_IF_FAILED(_allocator->InitializeSampleAllocator(10, _currentType.get()));
    RETURN_IF_FAILED(_queue->QueueEventParamVar(MEStreamStarted, GUID_NULL, S_OK, nullptr));
    _state = MF_STREAM_STATE_RUNNING;
    return S_OK;
}

HRESULT MediaStream::Stop()
{
    RETURN_HR_IF(MF_E_SHUTDOWN, !_queue || !_allocator);
    if (_state != MF_STREAM_STATE_STOPPED)
    {
        _frames.Stop();
    }
    RETURN_IF_FAILED(_allocator->UninitializeSampleAllocator());
    RETURN_IF_FAILED(_queue->QueueEventParamVar(MEStreamStopped, GUID_NULL, S_OK, nullptr));
    _state = MF_STREAM_STATE_STOPPED;
    return S_OK;
}

HRESULT MediaStream::SetAllocator(IUnknown* allocator)
{
    RETURN_HR_IF_NULL(E_POINTER, allocator);
    _allocator.reset();
    return allocator->QueryInterface(&_allocator);
}

void MediaStream::Shutdown()
{
    if (_state != MF_STREAM_STATE_STOPPED)
    {
        _frames.Stop();
        _state = MF_STREAM_STATE_STOPPED;
    }
    if (_queue)
    {
        LOG_IF_FAILED(_queue->Shutdown());
        _queue.reset();
    }
    _descriptor.reset();
    _source.reset();
    _allocator.reset();
}

STDMETHODIMP MediaStream::BeginGetEvent(IMFAsyncCallback* callback, IUnknown* state)
{
    winrt::slim_lock_guard lock(_lock);
    RETURN_HR_IF(MF_E_SHUTDOWN, !_queue);
    return _queue->BeginGetEvent(callback, state);
}

STDMETHODIMP MediaStream::EndGetEvent(IMFAsyncResult* result, IMFMediaEvent** event)
{
    winrt::slim_lock_guard lock(_lock);
    RETURN_HR_IF(MF_E_SHUTDOWN, !_queue);
    return _queue->EndGetEvent(result, event);
}

STDMETHODIMP MediaStream::GetEvent(DWORD flags, IMFMediaEvent** event)
{
    wil::com_ptr_nothrow<IMFMediaEventQueue> queue;
    {
        winrt::slim_lock_guard lock(_lock);
        RETURN_HR_IF(MF_E_SHUTDOWN, !_queue);
        queue = _queue;
    }
    return queue->GetEvent(flags, event);
}

STDMETHODIMP MediaStream::QueueEvent(MediaEventType type, REFGUID extendedType, HRESULT status, const PROPVARIANT* value)
{
    winrt::slim_lock_guard lock(_lock);
    RETURN_HR_IF(MF_E_SHUTDOWN, !_queue);
    return _queue->QueueEventParamVar(type, extendedType, status, value);
}

STDMETHODIMP MediaStream::GetMediaSource(IMFMediaSource** source)
{
    RETURN_HR_IF_NULL(E_POINTER, source);
    *source = nullptr;
    RETURN_HR_IF(MF_E_SHUTDOWN, !_source);
    return _source.copy_to(source);
}

STDMETHODIMP MediaStream::GetStreamDescriptor(IMFStreamDescriptor** descriptor)
{
    RETURN_HR_IF_NULL(E_POINTER, descriptor);
    *descriptor = nullptr;
    RETURN_HR_IF(MF_E_SHUTDOWN, !_descriptor);
    return _descriptor.copy_to(descriptor);
}

STDMETHODIMP MediaStream::RequestSample(IUnknown* token)
{
    winrt::slim_lock_guard lock(_lock);
    RETURN_HR_IF(MF_E_SHUTDOWN, !_allocator || !_queue);

    wil::com_ptr_nothrow<IMFSample> sample;
    RETURN_IF_FAILED(_allocator->AllocateSample(&sample));
    RETURN_IF_FAILED(sample->SetSampleTime(MFGetSystemTime()));
    RETURN_IF_FAILED(sample->SetSampleDuration(HundredNanosecondsPerSecond / spike::FrameRate));
    RETURN_IF_FAILED(_frames.Fill(sample.get()));
    if (token)
    {
        RETURN_IF_FAILED(sample->SetUnknown(MFSampleExtension_Token, token));
    }
    return _queue->QueueEventParamUnk(MEMediaSample, GUID_NULL, S_OK, sample.get());
}

STDMETHODIMP MediaStream::SetStreamState(MF_STREAM_STATE state)
{
    if (_state == state)
    {
        return S_OK;
    }
    switch (state)
    {
    case MF_STREAM_STATE_PAUSED:
        RETURN_HR_IF(MF_E_INVALID_STATE_TRANSITION, _state != MF_STREAM_STATE_RUNNING);
        _state = state;
        return S_OK;
    case MF_STREAM_STATE_RUNNING:
        return Start(nullptr);
    case MF_STREAM_STATE_STOPPED:
        return Stop();
    default:
        return MF_E_INVALID_STATE_TRANSITION;
    }
}

STDMETHODIMP MediaStream::GetStreamState(MF_STREAM_STATE* state)
{
    RETURN_HR_IF_NULL(E_POINTER, state);
    *state = _state;
    return S_OK;
}

STDMETHODIMP_(NTSTATUS) MediaStream::KsProperty(PKSPROPERTY, ULONG, LPVOID, ULONG, ULONG*)
{
    return HRESULT_FROM_WIN32(ERROR_SET_NOT_FOUND);
}

STDMETHODIMP_(NTSTATUS) MediaStream::KsMethod(PKSMETHOD, ULONG, LPVOID, ULONG, ULONG*)
{
    return HRESULT_FROM_WIN32(ERROR_SET_NOT_FOUND);
}

STDMETHODIMP_(NTSTATUS) MediaStream::KsEvent(PKSEVENT, ULONG, LPVOID, ULONG, ULONG*)
{
    return HRESULT_FROM_WIN32(ERROR_SET_NOT_FOUND);
}
