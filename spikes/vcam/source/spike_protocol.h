#pragma once

#include <atomic>
#include <cstdint>

namespace spike
{
    inline constexpr uint32_t Magic = 0x4D414347;
    inline constexpr uint32_t ProtocolVersion = 1;
    inline constexpr uint32_t FrameWidth = 1280;
    inline constexpr uint32_t FrameHeight = 720;
    inline constexpr uint32_t FrameRate = 30;
    inline constexpr uint32_t Nv12Bytes = FrameWidth * FrameHeight * 3 / 2;
    inline constexpr uint32_t HeaderBytes = 64;
    inline constexpr uint32_t SectionBytes = HeaderBytes + Nv12Bytes;

    inline constexpr uint32_t ConsumerActive = 1;
    inline constexpr uint32_t ProducerActive = 2;

    inline constexpr wchar_t AppProbeGlobalName[] = L"Global\\bgcam-spike-app-probe";
    inline constexpr wchar_t AppSectionBaseName[] = L"bgcam-spike-app-section";
    inline constexpr wchar_t AppEventBaseName[] = L"bgcam-spike-app-event";
    inline constexpr wchar_t DllSectionName[] = L"Global\\bgcam-spike-dll-section";
    inline constexpr wchar_t DllEventName[] = L"Global\\bgcam-spike-dll-event";
    inline constexpr wchar_t SharedObjectSddl[] = L"D:P(A;;GA;;;SY)(A;;GA;;;LS)(A;;GA;;;IU)";

    struct Header
    {
        uint32_t magic;
        uint32_t version;
        uint32_t width;
        uint32_t height;
        std::atomic<uint64_t> sequence;
        uint64_t frameNumber;
        int64_t frameQpc;
        std::atomic<uint32_t> flags;
        uint32_t reserved;
        std::atomic<int64_t> producerHeartbeatQpc;
        std::atomic<int64_t> consumerHeartbeatQpc;
    };

    static_assert(sizeof(Header) == HeaderBytes);
    static_assert(offsetof(Header, sequence) == 16);
    static_assert(offsetof(Header, flags) == 40);
    static_assert(offsetof(Header, consumerHeartbeatQpc) == 56);
}
