#include "pch.h"
#include "offline_frame.h"

namespace
{
    constexpr wchar_t OfflineImageName[] = L"offline.png";

    std::wstring ModuleDirectory()
    {
        HMODULE module = nullptr;
        if (!GetModuleHandleExW(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
                                reinterpret_cast<LPCWSTR>(&LoadOfflineFrame), &module))
        {
            return {};
        }
        auto path = wil::GetModuleFileNameW<std::wstring>(module);
        const auto separator = path.find_last_of(L'\\');
        return separator == std::wstring::npos ? std::wstring{} : path.substr(0, separator + 1);
    }

    HRESULT DecodeBgra(const std::wstring& path, UINT width, UINT height, std::vector<uint8_t>& pixels)
    {
        auto factory = wil::CoCreateInstanceNoThrow<IWICImagingFactory>(CLSID_WICImagingFactory);
        RETURN_HR_IF_NULL(E_NOINTERFACE, factory);
        wil::com_ptr_nothrow<IWICBitmapDecoder> decoder;
        RETURN_IF_FAILED(factory->CreateDecoderFromFilename(path.c_str(), nullptr, GENERIC_READ, WICDecodeMetadataCacheOnDemand, &decoder));
        wil::com_ptr_nothrow<IWICBitmapFrameDecode> frame;
        RETURN_IF_FAILED(decoder->GetFrame(0, &frame));
        UINT sourceWidth = 0;
        UINT sourceHeight = 0;
        RETURN_IF_FAILED(frame->GetSize(&sourceWidth, &sourceHeight));
        RETURN_HR_IF(E_INVALIDARG, sourceWidth == 0 || sourceHeight == 0);

        const auto scale = std::min(static_cast<double>(width) / sourceWidth, static_cast<double>(height) / sourceHeight);
        const auto scaledWidth = std::clamp(static_cast<UINT>(std::lround(sourceWidth * scale)), UINT{ 1 }, width);
        const auto scaledHeight = std::clamp(static_cast<UINT>(std::lround(sourceHeight * scale)), UINT{ 1 }, height);
        wil::com_ptr_nothrow<IWICBitmapScaler> scaler;
        RETURN_IF_FAILED(factory->CreateBitmapScaler(&scaler));
        RETURN_IF_FAILED(scaler->Initialize(frame.get(), scaledWidth, scaledHeight, WICBitmapInterpolationModeHighQualityCubic));

        wil::com_ptr_nothrow<IWICFormatConverter> converter;
        RETURN_IF_FAILED(factory->CreateFormatConverter(&converter));
        RETURN_IF_FAILED(converter->Initialize(scaler.get(), GUID_WICPixelFormat32bppBGRA, WICBitmapDitherTypeNone, nullptr, 0.0, WICBitmapPaletteTypeCustom));
        std::vector<uint8_t> scaled(static_cast<size_t>(scaledWidth) * scaledHeight * 4);
        RETURN_IF_FAILED(converter->CopyPixels(nullptr, scaledWidth * 4, static_cast<UINT>(scaled.size()), scaled.data()));

        pixels.resize(static_cast<size_t>(width) * height * 4);
        for (size_t offset = 0; offset < pixels.size(); offset += 4)
        {
            std::copy_n(scaled.data(), 4, pixels.data() + offset);
        }
        const auto left = (width - scaledWidth) / 2;
        const auto top = (height - scaledHeight) / 2;
        for (UINT y = 0; y < scaledHeight; y++)
        {
            std::copy_n(scaled.data() + static_cast<size_t>(y) * scaledWidth * 4, static_cast<size_t>(scaledWidth) * 4,
                        pixels.data() + (static_cast<size_t>(top + y) * width + left) * 4);
        }
        return S_OK;
    }

    uint8_t Clamp(int value)
    {
        return static_cast<uint8_t>(std::clamp(value, 0, 255));
    }

    std::vector<uint8_t> BgraToNv12(const std::vector<uint8_t>& bgra, uint32_t width, uint32_t height)
    {
        const bool hd = height >= 720;
        const int yr = hd ? 47 : 66;
        const int yg = hd ? 157 : 129;
        const int yb = hd ? 16 : 25;
        const int ur = hd ? -26 : -38;
        const int ug = hd ? -87 : -74;
        const int vg = hd ? -102 : -94;
        const int vb = hd ? -10 : -18;
        std::vector<uint8_t> nv12(static_cast<size_t>(width) * height * 3 / 2);
        const auto chroma = nv12.data() + static_cast<size_t>(width) * height;
        for (uint32_t y = 0; y < height; y++)
        {
            for (uint32_t x = 0; x < width; x++)
            {
                const auto* pixel = bgra.data() + (static_cast<size_t>(y) * width + x) * 4;
                const int alpha = pixel[3];
                const int b = pixel[0] * alpha / 255;
                const int g = pixel[1] * alpha / 255;
                const int r = pixel[2] * alpha / 255;
                nv12[static_cast<size_t>(y) * width + x] = Clamp(((yr * r + yg * g + yb * b + 128) >> 8) + 16);
                if ((x & 1) == 0 && (y & 1) == 0)
                {
                    const auto offset = static_cast<size_t>(y / 2) * width + x;
                    chroma[offset] = Clamp(((ur * r + ug * g + 112 * b + 128) >> 8) + 128);
                    chroma[offset + 1] = Clamp(((112 * r + vg * g + vb * b + 128) >> 8) + 128);
                }
            }
        }
        return nv12;
    }
}

std::vector<uint8_t> LoadOfflineFrame(uint32_t format, uint32_t width, uint32_t height)
{
    const auto directory = ModuleDirectory();
    if (directory.empty() || width == 0 || height == 0)
    {
        return {};
    }
    const auto path = directory + OfflineImageName;
    if (GetFileAttributesW(path.c_str()) == INVALID_FILE_ATTRIBUTES)
    {
        return {};
    }
    const auto com = CoInitializeEx(nullptr, COINIT_MULTITHREADED);
    auto uninitialize = wil::scope_exit([&] {
        if (SUCCEEDED(com))
        {
            CoUninitialize();
        }
    });
    std::vector<uint8_t> bgra;
    if (FAILED_LOG(DecodeBgra(path, width, height, bgra)))
    {
        return {};
    }
    return format == CHROMAFREE_FORMAT_NV12 ? BgraToNv12(bgra, width, height) : bgra;
}
