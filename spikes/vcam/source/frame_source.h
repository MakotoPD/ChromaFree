#pragma once

#include "spike_protocol.h"

enum class OutputFormat
{
    Nv12,
    Rgb32,
    Argb32,
};

struct OutputTarget
{
    BYTE* scanline0;
    LONG pitch;
    OutputFormat format;
};

struct SharedChannel
{
    std::wstring label;
    wil::unique_handle section;
    wil::unique_handle frameEvent;
    wil::unique_mapview_ptr<void> view;
    uint64_t lastFrameNumber = UINT64_MAX;
    uint64_t framesDelivered = 0;
    uint64_t eventsSeen = 0;

    spike::Header* Header() const { return static_cast<spike::Header*>(view.get()); }
    explicit operator bool() const { return view != nullptr; }
};

struct DeliveryStats
{
    uint64_t requests = 0;
    uint64_t patternFrames = 0;
    uint64_t repeatedFrames = 0;
    uint64_t seqlockRetries = 0;
    uint64_t latencySamples = 0;
    double latencySumMs = 0;
    double latencyMaxMs = 0;
    double copySumMs = 0;
    double copyMaxMs = 0;
    int64_t windowStartQpc = 0;
};

class FrameSource
{
public:
    void Start(OutputFormat format);
    void Stop();
    HRESULT Fill(IMFSample* sample);

private:
    void OpenChannels();
    bool OpenChannel(SharedChannel& channel, const std::wstring& sectionName, const std::wstring& eventName);
    bool CreateChannel(SharedChannel& channel, const wchar_t* sectionName, const wchar_t* eventName);
    bool TryDeliverFrom(SharedChannel& channel, const OutputTarget& target);
    void DrawPattern();
    void LogStatsIfDue(bool force);

    OutputFormat _format = OutputFormat::Nv12;
    SharedChannel _dllChannel;
    SharedChannel _appChannel;
    std::vector<uint8_t> _pattern;
    uint64_t _frame = 0;
    DeliveryStats _stats;
};
