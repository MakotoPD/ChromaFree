#pragma once

#include "attributes.h"
#include "frame_channel.h"

struct MediaStream : winrt::implements<MediaStream, AttributesBase<IMFAttributes>, IMFMediaStream2, IKsControl>
{
    STDMETHODIMP BeginGetEvent(IMFAsyncCallback* callback, IUnknown* state) override;
    STDMETHODIMP EndGetEvent(IMFAsyncResult* result, IMFMediaEvent** event) override;
    STDMETHODIMP GetEvent(DWORD flags, IMFMediaEvent** event) override;
    STDMETHODIMP QueueEvent(MediaEventType type, REFGUID extendedType, HRESULT status, const PROPVARIANT* value) override;

    STDMETHODIMP GetMediaSource(IMFMediaSource** source) override;
    STDMETHODIMP GetStreamDescriptor(IMFStreamDescriptor** descriptor) override;
    STDMETHODIMP RequestSample(IUnknown* token) override;

    STDMETHODIMP SetStreamState(MF_STREAM_STATE state) override;
    STDMETHODIMP GetStreamState(MF_STREAM_STATE* state) override;

    STDMETHODIMP_(NTSTATUS) KsProperty(PKSPROPERTY property, ULONG propertyLength, LPVOID data, ULONG dataLength, ULONG* bytesReturned) override;
    STDMETHODIMP_(NTSTATUS) KsMethod(PKSMETHOD method, ULONG methodLength, LPVOID data, ULONG dataLength, ULONG* bytesReturned) override;
    STDMETHODIMP_(NTSTATUS) KsEvent(PKSEVENT event, ULONG eventLength, LPVOID data, ULONG dataLength, ULONG* bytesReturned) override;

    HRESULT Initialize(IMFMediaSource* source, DWORD streamId, std::shared_ptr<FrameChannel> channel, const OutputMode& mode);
    HRESULT SetAllocator(IUnknown* allocator);
    HRESULT Start(IMFMediaType* type);
    HRESULT Stop();
    void Shutdown();

private:
    struct StreamFormat
    {
        uint32_t format;
        uint32_t width;
        uint32_t height;
        int64_t periodQpc;
        int64_t sampleDuration;
    };

    static constexpr size_t MaxPendingRequests = 16;

    void StopWorker();
    void Run(const StreamFormat& format);
    bool Deliver(const StreamFormat& format, bool producerAlive, const std::vector<uint8_t>& offline, uint64_t& lastFrame);

    winrt::slim_mutex _lock;
    MF_STREAM_STATE _state = MF_STREAM_STATE_STOPPED;
    std::shared_ptr<FrameChannel> _channel;
    std::thread _worker;
    wil::unique_event _stopWorker;
    wil::unique_handle _pacingTimer;
    std::array<wil::com_ptr_nothrow<IUnknown>, MaxPendingRequests> _pendingTokens;
    size_t _pendingHead = 0;
    size_t _pendingCount = 0;
    wil::com_ptr_nothrow<IMFStreamDescriptor> _descriptor;
    wil::com_ptr_nothrow<IMFMediaEventQueue> _queue;
    wil::com_ptr_nothrow<IMFMediaSource> _source;
    wil::com_ptr_nothrow<IMFVideoSampleAllocatorEx> _allocator;
    wil::com_ptr_nothrow<IMFMediaType> _currentType;
};
