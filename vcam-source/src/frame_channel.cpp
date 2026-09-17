#include "pch.h"
#include "frame_channel.h"
#include "log.h"

namespace
{
    template <class T> T Load(const T& field)
    {
        const volatile T* pointer = &field;
        return *pointer;
    }

    LONG* AsLong(uint32_t& field)
    {
        return reinterpret_cast<LONG*>(&field);
    }

    size_t FrameBytes(uint32_t format, uint32_t width, uint32_t height)
    {
        const auto pixels = static_cast<size_t>(width) * height;
        switch (format)
        {
        case CHROMAFREE_FORMAT_NV12:
            return pixels * 3 / 2;
        case CHROMAFREE_FORMAT_BGRA:
            return pixels * 4;
        default:
            return 0;
        }
    }

    const uint8_t* FrameData(const ChromaFreeFrameHeader* header)
    {
        return reinterpret_cast<const uint8_t*>(header) + CHROMAFREE_HEADER_SIZE;
    }

    uint8_t* Row(const FrameTarget& target, uint32_t row)
    {
        return target.scanline0 + static_cast<ptrdiff_t>(row) * target.pitch;
    }

    std::wstring SessionName(DWORD sessionId, const wchar_t* name)
    {
        return std::format(L"Session\\{}\\{}", sessionId, name);
    }
}

int64_t QpcNow()
{
    LARGE_INTEGER value;
    QueryPerformanceCounter(&value);
    return value.QuadPart;
}

int64_t QpcFrequency()
{
    LARGE_INTEGER value;
    QueryPerformanceFrequency(&value);
    return value.QuadPart;
}

void CopyFrame(const FrameTarget& target, const uint8_t* source)
{
    if (target.format == CHROMAFREE_FORMAT_NV12)
    {
        for (uint32_t y = 0; y < target.height + target.height / 2; y++)
        {
            memcpy(Row(target, y), source + static_cast<size_t>(y) * target.width, target.width);
        }
        return;
    }
    const auto rowBytes = static_cast<size_t>(target.width) * 4;
    for (uint32_t y = 0; y < target.height; y++)
    {
        memcpy(Row(target, y), source + y * rowBytes, rowBytes);
    }
}

void FillFallback(const FrameTarget& target)
{
    if (target.format == CHROMAFREE_FORMAT_NV12)
    {
        for (uint32_t y = 0; y < target.height; y++)
        {
            memset(Row(target, y), 16, target.width);
        }
        for (uint32_t y = target.height; y < target.height + target.height / 2; y++)
        {
            memset(Row(target, y), 128, target.width);
        }
        return;
    }
    for (uint32_t y = 0; y < target.height; y++)
    {
        auto* row = Row(target, y);
        for (uint32_t x = 0; x < target.width; x++)
        {
            row[x * 4] = 0;
            row[x * 4 + 1] = 0;
            row[x * 4 + 2] = 0;
            row[x * 4 + 3] = 255;
        }
    }
}

FrameChannel::FrameChannel(DWORD sessionId) : _sessionId(sessionId)
{
}

FrameChannel::~FrameChannel()
{
    ConsumerStopped();
    Close();
}

bool FrameChannel::IsOpen() const
{
    return _view != nullptr;
}

ChromaFreeFrameHeader* FrameChannel::Header() const
{
    return static_cast<ChromaFreeFrameHeader*>(_view.get());
}

HANDLE FrameChannel::FrameReadyEvent() const
{
    return _frameReady.get();
}

void FrameChannel::Close()
{
    if (_slot && IsOpen())
    {
        InterlockedAnd(AsLong(Header()->reader_slots), ~static_cast<LONG>(1u << *_slot));
    }
    _slot.reset();
    _consumerChanged.reset();
    _frameReady.reset();
    _view.reset();
    _section.reset();
}

std::optional<uint32_t> FrameChannel::ClaimSlot()
{
    auto* slots = AsLong(Header()->reader_slots);
    for (uint32_t slot = 0; slot < CHROMAFREE_READER_SLOTS; slot++)
    {
        const LONG bit = static_cast<LONG>(1u << slot);
        if ((InterlockedOr(slots, bit) & bit) == 0)
        {
            return slot;
        }
    }
    return std::nullopt;
}

bool FrameChannel::Open()
{
    if (IsOpen())
    {
        return true;
    }
    _section.reset(OpenFileMappingW(FILE_MAP_READ | FILE_MAP_WRITE, FALSE, SessionName(_sessionId, CHROMAFREE_SECTION_NAME).c_str()));
    if (!_section)
    {
        const auto error = GetLastError();
        if (error != _lastOpenError)
        {
            LogEvent("shared memory in session %lu not available, error %lu", _sessionId, error);
            _lastOpenError = error;
        }
        return false;
    }
    _view.reset(MapViewOfFile(_section.get(), FILE_MAP_READ | FILE_MAP_WRITE, 0, 0, CHROMAFREE_SECTION_SIZE));
    const auto* header = Header();
    const bool compatible = header && Load(header->magic) == CHROMAFREE_MAGIC && Load(header->version) == CHROMAFREE_PROTOCOL_VERSION &&
                            Load(header->header_size) == CHROMAFREE_HEADER_SIZE;
    if (!compatible)
    {
        LogEvent("shared memory in session %lu has protocol version %u, expected %u", _sessionId, header ? Load(header->version) : 0,
                 CHROMAFREE_PROTOCOL_VERSION);
        Close();
        return false;
    }
    _slot = ClaimSlot();
    const auto readyName = _slot ? std::format(L"{}{}", CHROMAFREE_READER_EVENT_PREFIX, *_slot) : std::wstring(CHROMAFREE_FRAME_READY_EVENT_NAME);
    _frameReady.reset(OpenEventW(SYNCHRONIZE, FALSE, SessionName(_sessionId, readyName.c_str()).c_str()));
    _consumerChanged.reset(OpenEventW(EVENT_MODIFY_STATE, FALSE, SessionName(_sessionId, CHROMAFREE_CONSUMER_CHANGED_EVENT_NAME).c_str()));
    if (!_frameReady || !_consumerChanged)
    {
        LogEvent("events in session %lu not available, error %lu", _sessionId, GetLastError());
        Close();
        return false;
    }
    _lastOpenError = ERROR_SUCCESS;
    LogEvent("shared memory opened in session %lu, reader slot %d", _sessionId, _slot ? static_cast<int>(*_slot) : -1);
    return true;
}

uint64_t FrameChannel::LatestFrameNumber() const
{
    return IsOpen() ? Load(Header()->frame_number) : 0;
}

std::optional<OutputMode> FrameChannel::Mode() const
{
    if (!IsOpen())
    {
        return std::nullopt;
    }
    const auto* header = Header();
    const OutputMode mode{ Load(header->output_width), Load(header->output_height), Load(header->output_fps_numerator), Load(header->output_fps_denominator) };
    const bool valid = mode.width >= 2 && mode.height >= 2 && mode.width % 2 == 0 && mode.height % 2 == 0 && mode.width <= CHROMAFREE_MAX_WIDTH &&
                       mode.height <= CHROMAFREE_MAX_HEIGHT && mode.fpsNumerator > 0 && mode.fpsDenominator > 0;
    return valid ? std::optional(mode) : std::nullopt;
}

void FrameChannel::SignalConsumerChanged() const
{
    LOG_IF_WIN32_BOOL_FALSE(SetEvent(_consumerChanged.get()));
}

void FrameChannel::ConsumerStarted(uint32_t format, uint32_t width, uint32_t height)
{
    if (!IsOpen() || _consuming)
    {
        return;
    }
    auto* header = Header();
    InterlockedExchange(AsLong(header->consumer_width), static_cast<LONG>(width));
    InterlockedExchange(AsLong(header->consumer_height), static_cast<LONG>(height));
    InterlockedExchange(AsLong(header->consumer_format), static_cast<LONG>(format));
    InterlockedIncrement(AsLong(header->consumer_count));
    InterlockedOr(AsLong(header->consumer_flags), CHROMAFREE_CONSUMER_ACTIVE);
    Heartbeat();
    _consuming = true;
    SignalConsumerChanged();
}

void FrameChannel::ConsumerStopped()
{
    if (!IsOpen() || !_consuming)
    {
        return;
    }
    auto* header = Header();
    auto* count = AsLong(header->consumer_count);
    LONG previous = 0;
    do
    {
        previous = *count;
    } while (InterlockedCompareExchange(count, previous > 0 ? previous - 1 : 0, previous) != previous);
    if (previous <= 1)
    {
        InterlockedAnd(AsLong(header->consumer_flags), ~static_cast<LONG>(CHROMAFREE_CONSUMER_ACTIVE));
    }
    _consuming = false;
    SignalConsumerChanged();
}

void FrameChannel::Heartbeat()
{
    if (IsOpen())
    {
        InterlockedExchange64(&Header()->consumer_heartbeat_qpc, QpcNow());
    }
}

bool FrameChannel::ProducerAlive() const
{
    if (!IsOpen())
    {
        return false;
    }
    const auto* header = Header();
    const auto heartbeat = Load(header->producer_heartbeat_qpc);
    const auto timeout = std::max<int64_t>(Load(header->qpc_frequency), 1) * CHROMAFREE_HEARTBEAT_TIMEOUT_MS / 1000;
    return (Load(header->producer_flags) & CHROMAFREE_PRODUCER_ACTIVE) != 0 && heartbeat != 0 && QpcNow() - heartbeat <= timeout;
}

std::optional<DeliveredFrame> FrameChannel::CopyLatest(const FrameTarget& target) const
{
    if (!IsOpen())
    {
        return std::nullopt;
    }
    const auto* header = Header();
    const auto expectedBytes = FrameBytes(target.format, target.width, target.height);
    for (uint32_t attempt = 0; attempt < CHROMAFREE_SEQLOCK_ATTEMPTS; attempt++)
    {
        const auto before = Load(header->sequence);
        if (before == 0)
        {
            return std::nullopt;
        }
        if (before & 1)
        {
            YieldProcessor();
            continue;
        }
        const bool matches = Load(header->frame_format) == target.format && Load(header->frame_width) == target.width &&
                             Load(header->frame_height) == target.height && Load(header->frame_size) == expectedBytes &&
                             expectedBytes <= CHROMAFREE_FRAME_CAPACITY;
        const DeliveredFrame frame{ Load(header->frame_number), Load(header->frame_qpc) };
        if (matches)
        {
            CopyFrame(target, FrameData(header));
        }
        if (Load(header->sequence) != before)
        {
            continue;
        }
        return matches ? std::optional(frame) : std::nullopt;
    }
    return std::nullopt;
}
