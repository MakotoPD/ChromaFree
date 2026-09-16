#include "pch.h"
#include "frame_source.h"
#include "log.h"

namespace
{
    constexpr double ProducerTimeoutMs = 1000.0;
    constexpr double StatsIntervalMs = 5000.0;
    constexpr int SeqlockAttempts = 8;

    void WriteNv12(const OutputTarget& target, const uint8_t* nv12)
    {
        const auto width = spike::FrameWidth;
        const auto height = spike::FrameHeight;
        for (uint32_t y = 0; y < height; y++)
        {
            memcpy(target.scanline0 + static_cast<ptrdiff_t>(y) * target.pitch, nv12 + y * width, width);
        }
        const auto* chroma = nv12 + width * height;
        auto* chromaTarget = target.scanline0 + static_cast<ptrdiff_t>(height) * target.pitch;
        for (uint32_t y = 0; y < height / 2; y++)
        {
            memcpy(chromaTarget + static_cast<ptrdiff_t>(y) * target.pitch, chroma + y * width, width);
        }
    }

    uint8_t ClampByte(int value)
    {
        return static_cast<uint8_t>(std::clamp(value, 0, 255));
    }

    void WriteRgb32(const OutputTarget& target, const uint8_t* nv12)
    {
        const auto width = spike::FrameWidth;
        const auto height = spike::FrameHeight;
        for (uint32_t y = 0; y < height; y++)
        {
            auto* row = target.scanline0 + static_cast<ptrdiff_t>(y) * target.pitch;
            const auto* luma = nv12 + y * width;
            const auto* chroma = nv12 + width * height + (y / 2) * width;
            for (uint32_t x = 0; x < width; x++)
            {
                const int c = 298 * (luma[x] - 16);
                const int d = chroma[x & ~1u] - 128;
                const int e = chroma[(x & ~1u) + 1] - 128;
                row[x * 4] = ClampByte((c + 516 * d + 128) >> 8);
                row[x * 4 + 1] = ClampByte((c - 100 * d - 208 * e + 128) >> 8);
                row[x * 4 + 2] = ClampByte((c + 409 * e + 128) >> 8);
                row[x * 4 + 3] = 255;
            }
        }
    }

    uint8_t AlphaForColumn(uint32_t x)
    {
        const auto band = x * 3 / spike::FrameWidth;
        return band == 0 ? 0 : (band == 1 ? 128 : 255);
    }

    void ApplyAlphaBands(const OutputTarget& target)
    {
        for (uint32_t y = 0; y < spike::FrameHeight; y++)
        {
            auto* row = target.scanline0 + static_cast<ptrdiff_t>(y) * target.pitch;
            for (uint32_t x = 0; x < spike::FrameWidth; x++)
            {
                row[x * 4 + 3] = AlphaForColumn(x);
            }
        }
    }

    void WriteFrame(const OutputTarget& target, const uint8_t* nv12)
    {
        switch (target.format)
        {
        case OutputFormat::Nv12:
            WriteNv12(target, nv12);
            break;
        case OutputFormat::Rgb32:
            WriteRgb32(target, nv12);
            break;
        case OutputFormat::Argb32:
            WriteRgb32(target, nv12);
            ApplyAlphaBands(target);
            break;
        }
    }

    const wchar_t* FormatName(OutputFormat format)
    {
        switch (format)
        {
        case OutputFormat::Nv12:
            return L"NV12";
        case OutputFormat::Rgb32:
            return L"RGB32";
        default:
            return L"ARGB32";
        }
    }
}

void FrameSource::Start(OutputFormat format)
{
    _format = format;
    _frame = 0;
    _stats = {};
    _stats.windowStartQpc = QpcNow();
    _pattern.assign(spike::Nv12Bytes, 0);
    LogLine(L"stream start format=%s %s", FormatName(format), ProcessDiagnostics().c_str());
    OpenChannels();
}

void FrameSource::Stop()
{
    LogStatsIfDue(true);
    for (auto* channel : { &_dllChannel, &_appChannel })
    {
        if (*channel)
        {
            channel->Header()->flags.fetch_and(~spike::ConsumerActive);
        }
        *channel = SharedChannel{};
    }
    _pattern.clear();
    _pattern.shrink_to_fit();
    LogLine(L"stream stop");
}

void FrameSource::OpenChannels()
{
    wil::unique_handle probe(OpenFileMappingW(FILE_MAP_READ, FALSE, spike::AppProbeGlobalName));
    LogLine(L"[A] open %s: %s", spike::AppProbeGlobalName, Win32ErrorText(probe ? ERROR_SUCCESS : GetLastError()).c_str());

    SharedChannel localAttempt;
    localAttempt.label = L"[B]";
    OpenChannel(localAttempt, std::format(L"Local\\{}", spike::AppSectionBaseName), std::format(L"Local\\{}", spike::AppEventBaseName));

    _appChannel.label = L"[E]";
    const auto session = WTSGetActiveConsoleSessionId();
    OpenChannel(_appChannel, std::format(L"Session\\{}\\{}", session, spike::AppSectionBaseName), std::format(L"Session\\{}\\{}", session, spike::AppEventBaseName));

    _dllChannel.label = L"[C]";
    CreateChannel(_dllChannel, spike::DllSectionName, spike::DllEventName);
}

bool FrameSource::OpenChannel(SharedChannel& channel, const std::wstring& sectionName, const std::wstring& eventName)
{
    channel.section.reset(OpenFileMappingW(FILE_MAP_READ | FILE_MAP_WRITE, FALSE, sectionName.c_str()));
    LogLine(L"%s open section %s: %s", channel.label.c_str(), sectionName.c_str(), Win32ErrorText(channel.section ? ERROR_SUCCESS : GetLastError()).c_str());
    if (!channel.section)
    {
        return false;
    }

    channel.view.reset(MapViewOfFile(channel.section.get(), FILE_MAP_READ | FILE_MAP_WRITE, 0, 0, spike::SectionBytes));
    if (!channel.view)
    {
        LogLine(L"%s map view: %s", channel.label.c_str(), Win32ErrorText(GetLastError()).c_str());
        channel.section.reset();
        return false;
    }

    channel.frameEvent.reset(OpenEventW(SYNCHRONIZE, FALSE, eventName.c_str()));
    LogLine(L"%s open event %s: %s", channel.label.c_str(), eventName.c_str(), Win32ErrorText(channel.frameEvent ? ERROR_SUCCESS : GetLastError()).c_str());
    channel.Header()->flags.fetch_or(spike::ConsumerActive);
    channel.Header()->consumerHeartbeatQpc = QpcNow();
    return true;
}

bool FrameSource::CreateChannel(SharedChannel& channel, const wchar_t* sectionName, const wchar_t* eventName)
{
    wil::unique_hlocal_security_descriptor descriptor;
    if (!ConvertStringSecurityDescriptorToSecurityDescriptorW(spike::SharedObjectSddl, SDDL_REVISION_1, &descriptor, nullptr))
    {
        LogLine(L"%s security descriptor: %s", channel.label.c_str(), Win32ErrorText(GetLastError()).c_str());
        return false;
    }
    SECURITY_ATTRIBUTES attributes{ sizeof(attributes), descriptor.get(), FALSE };

    channel.section.reset(CreateFileMappingW(INVALID_HANDLE_VALUE, &attributes, PAGE_READWRITE, 0, spike::SectionBytes, sectionName));
    const auto createError = GetLastError();
    LogLine(L"%s create section %s: %s", channel.label.c_str(), sectionName, Win32ErrorText(channel.section ? (createError == ERROR_ALREADY_EXISTS ? createError : ERROR_SUCCESS) : createError).c_str());
    if (!channel.section)
    {
        return false;
    }

    channel.view.reset(MapViewOfFile(channel.section.get(), FILE_MAP_READ | FILE_MAP_WRITE, 0, 0, spike::SectionBytes));
    if (!channel.view)
    {
        LogLine(L"%s map view: %s", channel.label.c_str(), Win32ErrorText(GetLastError()).c_str());
        channel.section.reset();
        return false;
    }

    auto* header = channel.Header();
    if (createError != ERROR_ALREADY_EXISTS)
    {
        header->magic = spike::Magic;
        header->version = spike::ProtocolVersion;
        header->width = spike::FrameWidth;
        header->height = spike::FrameHeight;
    }

    channel.frameEvent.reset(CreateEventW(&attributes, FALSE, FALSE, eventName));
    LogLine(L"%s create event %s: %s", channel.label.c_str(), eventName, Win32ErrorText(channel.frameEvent ? ERROR_SUCCESS : GetLastError()).c_str());
    header->flags.fetch_or(spike::ConsumerActive);
    header->consumerHeartbeatQpc = QpcNow();
    return true;
}

HRESULT FrameSource::Fill(IMFSample* sample)
{
    wil::com_ptr_nothrow<IMFMediaBuffer> buffer;
    RETURN_IF_FAILED(sample->GetBufferByIndex(0, &buffer));
    auto buffer2d = buffer.try_query<IMF2DBuffer2>();
    RETURN_HR_IF_NULL(E_NOINTERFACE, buffer2d);

    BYTE* scanline0 = nullptr;
    LONG pitch = 0;
    BYTE* bufferStart = nullptr;
    DWORD bufferLength = 0;
    RETURN_IF_FAILED(buffer2d->Lock2DSize(MF2DBuffer_LockFlags_Write, &scanline0, &pitch, &bufferStart, &bufferLength));
    auto unlock = wil::scope_exit([&] { buffer2d->Unlock2D(); });

    const OutputTarget target{ scanline0, pitch, _format };
    _stats.requests++;
    const auto copyStart = QpcNow();
    const bool delivered = TryDeliverFrom(_dllChannel, target) || TryDeliverFrom(_appChannel, target);
    if (!delivered)
    {
        DrawPattern();
        WriteFrame(target, _pattern.data());
        _stats.patternFrames++;
    }
    const auto copyMs = QpcToMilliseconds(QpcNow() - copyStart);
    _stats.copySumMs += copyMs;
    _stats.copyMaxMs = std::max(_stats.copyMaxMs, copyMs);
    _frame++;
    LogStatsIfDue(false);
    return S_OK;
}

bool FrameSource::TryDeliverFrom(SharedChannel& channel, const OutputTarget& target)
{
    if (!channel)
    {
        return false;
    }
    auto* header = channel.Header();
    const auto now = QpcNow();
    header->consumerHeartbeatQpc = now;
    header->flags.fetch_or(spike::ConsumerActive);

    if (channel.frameEvent && WaitForSingleObject(channel.frameEvent.get(), 0) == WAIT_OBJECT_0)
    {
        channel.eventsSeen++;
    }

    if (header->magic != spike::Magic || header->version != spike::ProtocolVersion || header->width != spike::FrameWidth || header->height != spike::FrameHeight)
    {
        return false;
    }
    if ((header->flags.load() & spike::ProducerActive) == 0 || QpcToMilliseconds(now - header->producerHeartbeatQpc.load()) > ProducerTimeoutMs)
    {
        return false;
    }

    const auto* frameData = static_cast<const uint8_t*>(channel.view.get()) + spike::HeaderBytes;
    for (int attempt = 0; attempt < SeqlockAttempts; attempt++)
    {
        const auto before = header->sequence.load();
        if (before & 1)
        {
            _stats.seqlockRetries++;
            YieldProcessor();
            continue;
        }
        const auto frameNumber = header->frameNumber;
        const auto frameQpc = header->frameQpc;
        WriteFrame(target, frameData);
        if (header->sequence.load() != before)
        {
            _stats.seqlockRetries++;
            continue;
        }

        if (frameNumber == channel.lastFrameNumber)
        {
            _stats.repeatedFrames++;
        }
        else
        {
            const auto latencyMs = QpcToMilliseconds(QpcNow() - frameQpc);
            _stats.latencySamples++;
            _stats.latencySumMs += latencyMs;
            _stats.latencyMaxMs = std::max(_stats.latencyMaxMs, latencyMs);
        }
        channel.lastFrameNumber = frameNumber;
        channel.framesDelivered++;
        return true;
    }
    return false;
}

void FrameSource::DrawPattern()
{
    const auto width = spike::FrameWidth;
    const auto height = spike::FrameHeight;
    const auto offset = static_cast<uint32_t>(_frame * 6);
    const auto barX = static_cast<uint32_t>((_frame * 12) % width);
    for (uint32_t y = 0; y < height; y++)
    {
        auto* row = _pattern.data() + y * width;
        for (uint32_t x = 0; x < width; x++)
        {
            const bool stripe = ((x + y + offset) / 40) % 2 == 0;
            const bool bar = x >= barX && x < barX + 24;
            row[x] = bar ? 235 : (stripe ? 170 : 70);
        }
    }
    auto* chroma = _pattern.data() + width * height;
    for (uint32_t i = 0; i < width * height / 2; i += 2)
    {
        chroma[i] = 170;
        chroma[i + 1] = 100;
    }
}

void FrameSource::LogStatsIfDue(bool force)
{
    const auto now = QpcNow();
    const auto elapsedMs = QpcToMilliseconds(now - _stats.windowStartQpc);
    if (!force && elapsedMs < StatsIntervalMs)
    {
        return;
    }
    const auto requests = std::max<uint64_t>(_stats.requests, 1);
    LogLine(
        L"stats %.1fs format=%s requests=%llu fps=%.1f pattern=%llu dll[C] frames=%llu events=%llu app[E] frames=%llu events=%llu repeated=%llu retries=%llu latency avg=%.2fms max=%.2fms copy avg=%.2fms max=%.2fms",
        elapsedMs / 1000.0, FormatName(_format), _stats.requests, _stats.requests * 1000.0 / std::max(elapsedMs, 1.0), _stats.patternFrames,
        _dllChannel.framesDelivered, _dllChannel.eventsSeen, _appChannel.framesDelivered, _appChannel.eventsSeen,
        _stats.repeatedFrames, _stats.seqlockRetries,
        _stats.latencySamples ? _stats.latencySumMs / _stats.latencySamples : 0.0, _stats.latencyMaxMs,
        _stats.copySumMs / requests, _stats.copyMaxMs);
    for (auto* channel : { &_dllChannel, &_appChannel })
    {
        channel->framesDelivered = 0;
        channel->eventsSeen = 0;
    }
    _stats = {};
    _stats.windowStartQpc = now;
}
