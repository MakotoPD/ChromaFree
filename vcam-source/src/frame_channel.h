#pragma once

struct OutputMode
{
    uint32_t width;
    uint32_t height;
    uint32_t fpsNumerator;
    uint32_t fpsDenominator;
};

struct FrameTarget
{
    BYTE* scanline0;
    LONG pitch;
    uint32_t format;
    uint32_t width;
    uint32_t height;
};

struct DeliveredFrame
{
    uint64_t number;
    int64_t qpc;
};

class FrameChannel
{
public:
    explicit FrameChannel(DWORD sessionId);
    ~FrameChannel();

    FrameChannel(const FrameChannel&) = delete;
    FrameChannel& operator=(const FrameChannel&) = delete;

    bool Open();
    bool IsOpen() const;
    std::optional<OutputMode> Mode() const;
    HANDLE FrameReadyEvent() const;

    void ConsumerStarted(uint32_t format);
    void ConsumerStopped();
    void Heartbeat();
    bool ProducerAlive() const;
    std::optional<DeliveredFrame> CopyLatest(const FrameTarget& target) const;

private:
    void Close();
    void SignalConsumerChanged() const;
    ChromaFreeFrameHeader* Header() const;

    DWORD _sessionId;
    wil::unique_handle _section;
    wil::unique_mapview_ptr<void> _view;
    wil::unique_handle _frameReady;
    wil::unique_handle _consumerChanged;
    bool _consuming = false;
};

int64_t QpcNow();
int64_t QpcFrequency();
void CopyFrame(const FrameTarget& target, const uint8_t* source);
void FillFallback(const FrameTarget& target);
