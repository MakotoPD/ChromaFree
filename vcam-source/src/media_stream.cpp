#include "pch.h"
#include "media_stream.h"
#include "offline_frame.h"

namespace
{
    constexpr int64_t HundredNanosecondsPerSecond = 10'000'000;
    constexpr int64_t RepeatAfterPeriods = 3;

    HRESULT CreateVideoType(REFGUID subtype, UINT32 bitsPerPixel, UINT32 stride, const OutputMode& mode, IMFMediaType** type)
    {
        wil::com_ptr_nothrow<IMFMediaType> result;
        RETURN_IF_FAILED(MFCreateMediaType(&result));
        RETURN_IF_FAILED(result->SetGUID(MF_MT_MAJOR_TYPE, MFMediaType_Video));
        RETURN_IF_FAILED(result->SetGUID(MF_MT_SUBTYPE, subtype));
        RETURN_IF_FAILED(MFSetAttributeSize(result.get(), MF_MT_FRAME_SIZE, mode.width, mode.height));
        RETURN_IF_FAILED(MFSetAttributeRatio(result.get(), MF_MT_FRAME_RATE, mode.fpsNumerator, mode.fpsDenominator));
        RETURN_IF_FAILED(MFSetAttributeRatio(result.get(), MF_MT_PIXEL_ASPECT_RATIO, 1, 1));
        RETURN_IF_FAILED(result->SetUINT32(MF_MT_DEFAULT_STRIDE, stride));
        RETURN_IF_FAILED(result->SetUINT32(MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive));
        RETURN_IF_FAILED(result->SetUINT32(MF_MT_ALL_SAMPLES_INDEPENDENT, TRUE));
        const auto bitrate = static_cast<uint64_t>(mode.width) * mode.height * bitsPerPixel * mode.fpsNumerator / mode.fpsDenominator;
        RETURN_IF_FAILED(result->SetUINT32(MF_MT_AVG_BITRATE, static_cast<UINT32>(std::min<uint64_t>(bitrate, UINT32_MAX))));
        *type = result.detach();
        return S_OK;
    }

    int64_t QpcToHundredNanoseconds(int64_t qpc, int64_t frequency)
    {
        return qpc / frequency * HundredNanosecondsPerSecond + qpc % frequency * HundredNanosecondsPerSecond / frequency;
    }
}

HRESULT MediaStream::Initialize(IMFMediaSource* source, DWORD streamId, std::shared_ptr<FrameChannel> channel, const OutputMode& mode)
{
    RETURN_HR_IF_NULL(E_POINTER, source);
    _source = source;
    _channel = std::move(channel);

    RETURN_IF_FAILED(SetGUID(MF_DEVICESTREAM_STREAM_CATEGORY, PINNAME_VIDEO_CAPTURE));
    RETURN_IF_FAILED(SetUINT32(MF_DEVICESTREAM_STREAM_ID, streamId));
    RETURN_IF_FAILED(SetUINT32(MF_DEVICESTREAM_FRAMESERVER_SHARED, 1));
    RETURN_IF_FAILED(SetUINT32(MF_DEVICESTREAM_ATTRIBUTE_FRAMESOURCE_TYPES, MFFrameSourceTypes_Color));
    RETURN_IF_FAILED(MFCreateEventQueue(&_queue));
    _stopWorker.create(wil::EventOptions::ManualReset);
    _pacingTimer.reset(CreateWaitableTimerExW(nullptr, nullptr, CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, TIMER_ALL_ACCESS));
    RETURN_LAST_ERROR_IF_NULL(_pacingTimer);

    std::array<wil::com_ptr_nothrow<IMFMediaType>, 3> types;
    RETURN_IF_FAILED(CreateVideoType(MFVideoFormat_NV12, 12, mode.width, mode, &types[0]));
    RETURN_IF_FAILED(CreateVideoType(MFVideoFormat_ARGB32, 32, mode.width * 4, mode, &types[1]));
    RETURN_IF_FAILED(CreateVideoType(MFVideoFormat_RGB32, 32, mode.width * 4, mode, &types[2]));
    std::array<IMFMediaType*, 3> rawTypes{ types[0].get(), types[1].get(), types[2].get() };

    RETURN_IF_FAILED(MFCreateStreamDescriptor(streamId, static_cast<DWORD>(rawTypes.size()), rawTypes.data(), &_descriptor));
    wil::com_ptr_nothrow<IMFMediaTypeHandler> handler;
    RETURN_IF_FAILED(_descriptor->GetMediaTypeHandler(&handler));
    RETURN_IF_FAILED(handler->SetCurrentMediaType(rawTypes[0]));
    return S_OK;
}

HRESULT MediaStream::Start(IMFMediaType* type)
{
    StopWorker();
    winrt::slim_lock_guard lock(_lock);
    RETURN_HR_IF(MF_E_SHUTDOWN, !_queue || !_allocator);
    if (type)
    {
        _currentType = type;
    }
    RETURN_HR_IF_NULL(MF_E_INVALIDMEDIATYPE, _currentType);

    GUID subtype{};
    RETURN_IF_FAILED(_currentType->GetGUID(MF_MT_SUBTYPE, &subtype));
    UINT32 width = 0;
    UINT32 height = 0;
    RETURN_IF_FAILED(MFGetAttributeSize(_currentType.get(), MF_MT_FRAME_SIZE, &width, &height));
    UINT32 numerator = 0;
    UINT32 denominator = 0;
    RETURN_IF_FAILED(MFGetAttributeRatio(_currentType.get(), MF_MT_FRAME_RATE, &numerator, &denominator));
    RETURN_HR_IF(MF_E_INVALIDMEDIATYPE, numerator == 0 || denominator == 0);

    const auto frequency = QpcFrequency();
    const StreamFormat format{
        subtype == MFVideoFormat_NV12 ? static_cast<uint32_t>(CHROMAFREE_FORMAT_NV12) : static_cast<uint32_t>(CHROMAFREE_FORMAT_BGRA),
        width,
        height,
        frequency * denominator / numerator,
        HundredNanosecondsPerSecond * denominator / numerator,
    };

    RETURN_IF_FAILED(_allocator->InitializeSampleAllocator(10, _currentType.get()));
    RETURN_IF_FAILED(_queue->QueueEventParamVar(MEStreamStarted, GUID_NULL, S_OK, nullptr));
    _state = MF_STREAM_STATE_RUNNING;
    _stopWorker.ResetEvent();
    try
    {
        _worker = std::thread([this, format] { Run(format); });
    }
    CATCH_RETURN();
    return S_OK;
}

void MediaStream::StopWorker()
{
    if (!_worker.joinable())
    {
        return;
    }
    _stopWorker.SetEvent();
    _worker.join();
    _channel->ConsumerStopped();
    winrt::slim_lock_guard lock(_lock);
    for (auto& token : _pendingTokens)
    {
        token.reset();
    }
    _pendingHead = 0;
    _pendingCount = 0;
}

HRESULT MediaStream::Stop()
{
    StopWorker();
    winrt::slim_lock_guard lock(_lock);
    RETURN_HR_IF(MF_E_SHUTDOWN, !_queue || !_allocator);
    RETURN_IF_FAILED(_allocator->UninitializeSampleAllocator());
    RETURN_IF_FAILED(_queue->QueueEventParamVar(MEStreamStopped, GUID_NULL, S_OK, nullptr));
    _state = MF_STREAM_STATE_STOPPED;
    return S_OK;
}

HRESULT MediaStream::SetAllocator(IUnknown* allocator)
{
    RETURN_HR_IF_NULL(E_POINTER, allocator);
    winrt::slim_lock_guard lock(_lock);
    _allocator.reset();
    return allocator->QueryInterface(&_allocator);
}

void MediaStream::Shutdown()
{
    StopWorker();
    winrt::slim_lock_guard lock(_lock);
    _state = MF_STREAM_STATE_STOPPED;
    if (_queue)
    {
        LOG_IF_FAILED(_queue->Shutdown());
        _queue.reset();
    }
    _descriptor.reset();
    _source.reset();
    _allocator.reset();
}

void MediaStream::Run(const StreamFormat& format)
{
    const auto offline = LoadOfflineFrame(format.format, format.width, format.height);
    const auto frequency = QpcFrequency();
    auto lastOpenAttempt = QpcNow();
    if (_channel->Open())
    {
        _channel->ConsumerStarted(format.format);
    }

    auto deadline = QpcNow();
    for (;;)
    {
        auto now = QpcNow();
        if (!_channel->IsOpen() && now - lastOpenAttempt >= frequency)
        {
            lastOpenAttempt = now;
            if (_channel->Open())
            {
                _channel->ConsumerStarted(format.format);
            }
        }

        LARGE_INTEGER due{};
        due.QuadPart = -QpcToHundredNanoseconds(std::max<int64_t>(deadline - now, 0), frequency);
        if (!SetWaitableTimerEx(_pacingTimer.get(), &due, 0, nullptr, nullptr, nullptr, 0))
        {
            LOG_LAST_ERROR();
            return;
        }
        std::array<HANDLE, 3> handles{ _stopWorker.get(), _pacingTimer.get(), _channel->FrameReadyEvent() };
        const DWORD handleCount = handles[2] ? 3 : 2;
        const auto result = WaitForMultipleObjects(handleCount, handles.data(), FALSE, INFINITE);
        if (result == WAIT_OBJECT_0 || result == WAIT_FAILED)
        {
            LOG_LAST_ERROR_IF(result == WAIT_FAILED);
            return;
        }

        _channel->Heartbeat();
        now = QpcNow();
        const bool newFrame = result == WAIT_OBJECT_0 + 2;
        if (!newFrame && now < deadline)
        {
            continue;
        }
        const bool producerAlive = _channel->ProducerAlive();
        if (!Deliver(format, producerAlive, offline))
        {
            deadline = now + format.periodQpc;
        }
        else if (producerAlive)
        {
            deadline = now + format.periodQpc * RepeatAfterPeriods;
        }
        else
        {
            deadline = std::max(deadline, now - format.periodQpc) + format.periodQpc;
        }
    }
}

bool MediaStream::Deliver(const StreamFormat& format, bool producerAlive, const std::vector<uint8_t>& offline)
{
    winrt::slim_lock_guard lock(_lock);
    if (_pendingCount == 0 || !_allocator || !_queue || _state != MF_STREAM_STATE_RUNNING)
    {
        return false;
    }

    wil::com_ptr_nothrow<IMFSample> sample;
    if (FAILED_LOG(_allocator->AllocateSample(&sample)))
    {
        return false;
    }
    wil::com_ptr_nothrow<IMFMediaBuffer> buffer;
    if (FAILED_LOG(sample->GetBufferByIndex(0, &buffer)))
    {
        return false;
    }
    auto buffer2d = buffer.try_query<IMF2DBuffer2>();
    BYTE* scanline0 = nullptr;
    LONG pitch = 0;
    BYTE* bufferStart = nullptr;
    DWORD bufferLength = 0;
    if (!buffer2d || FAILED_LOG(buffer2d->Lock2DSize(MF2DBuffer_LockFlags_Write, &scanline0, &pitch, &bufferStart, &bufferLength)))
    {
        return false;
    }

    const FrameTarget target{ scanline0, pitch, format.format, format.width, format.height };
    const auto frame = producerAlive ? _channel->CopyLatest(target) : std::nullopt;
    if (!frame)
    {
        if (offline.empty())
        {
            FillFallback(target);
        }
        else
        {
            CopyFrame(target, offline.data());
        }
    }
    LOG_IF_FAILED(buffer2d->Unlock2D());

    const auto sampleTime = frame ? QpcToHundredNanoseconds(frame->qpc, QpcFrequency()) : MFGetSystemTime();
    auto token = std::move(_pendingTokens[_pendingHead]);
    _pendingHead = (_pendingHead + 1) % MaxPendingRequests;
    _pendingCount--;

    if (FAILED_LOG(sample->SetSampleTime(sampleTime)) || FAILED_LOG(sample->SetSampleDuration(format.sampleDuration)))
    {
        return false;
    }
    if (token && FAILED_LOG(sample->SetUnknown(MFSampleExtension_Token, token.get())))
    {
        return false;
    }
    return SUCCEEDED_LOG(_queue->QueueEventParamUnk(MEMediaSample, GUID_NULL, S_OK, sample.get()));
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
    RETURN_HR_IF(MF_E_MEDIA_SOURCE_WRONGSTATE, _state != MF_STREAM_STATE_RUNNING);
    if (_pendingCount < MaxPendingRequests)
    {
        _pendingTokens[(_pendingHead + _pendingCount) % MaxPendingRequests] = token;
        _pendingCount++;
    }
    return S_OK;
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
