#include "frame_sampling.hlsli"

cbuffer Params : register(b0)
{
    uint Width;
    uint Height;
    uint MaskWidth;
    uint MaskHeight;
    uint Mode;
    uint AlphaFromMask;
    uint Reserved0;
    uint Reserved1;
    float BackgroundY;
    float BackgroundU;
    float BackgroundV;
    float RedV;
    float GreenU;
    float GreenV;
    float BlueU;
    float Reserved2;
};

StructuredBuffer<float> ImageBackground : register(t0);
RWStructuredBuffer<float> Frame : register(u0);
RWStructuredBuffer<float> Mask : register(u1);
RWStructuredBuffer<float> BlurBackground : register(u2);
RWStructuredBuffer<uint> Output : register(u3);

static const uint ModeColor = 0;
static const uint ModeImage = 1;
static const uint ModeBlur = 2;
static const uint ModePassthrough = 3;

float SampleMask(float u, float v)
{
    [branch] if (Mode == ModePassthrough && AlphaFromMask == 0)
    {
        return 1.0;
    }
    return saturate(SamplePlane(Mask, u * MaskWidth, v * MaskHeight, MaskWidth, MaskHeight, 0, 1, 0));
}

float Background(uint index, uint channel)
{
    [branch] if (Mode == ModeImage)
    {
        return ImageBackground[index];
    }
    [branch] if (Mode == ModeBlur)
    {
        return BlurBackground[index];
    }
    return channel == 0 ? BackgroundY : (channel == 1 ? BackgroundU : BackgroundV);
}

float Blend(float foreground, uint index, uint channel, float alpha)
{
    [branch] if (Mode == ModePassthrough)
    {
        return foreground;
    }
    return lerp(Background(index, channel), foreground, alpha);
}

uint ToByte(float value)
{
    return (uint)clamp(value + 0.5, 0.0, 255.0);
}

[numthreads(16, 16, 1)]
void nv12(uint3 id : SV_DispatchThreadID)
{
    uint elementsPerRow = Width / 4;
    if (id.x >= elementsPerRow || id.y >= Height + Height / 2)
    {
        return;
    }
    uint packed = 0;
    for (uint i = 0; i < 4; i++)
    {
        uint column = id.x * 4 + i;
        float value;
        if (id.y < Height)
        {
            uint index = id.y * Width + column;
            float alpha = SampleMask((column + 0.5) / Width, (id.y + 0.5) / Height);
            value = Blend(Frame[index], index, 0, alpha);
        }
        else
        {
            uint row = id.y - Height;
            uint chromaColumn = column / 2;
            uint channel = column & 1;
            uint index = Width * Height + row * Width + column;
            float alpha = SampleMask((chromaColumn + 0.5) / (Width / 2), (row + 0.5) / (Height / 2));
            value = Blend(Frame[index], index, channel + 1, alpha);
        }
        packed |= ToByte(value) << (8 * i);
    }
    Output[id.y * elementsPerRow + id.x] = packed;
}

[numthreads(16, 16, 1)]
void bgra(uint3 id : SV_DispatchThreadID)
{
    if (id.x >= Width || id.y >= Height)
    {
        return;
    }
    uint lumaIndex = id.y * Width + id.x;
    uint chromaIndex = Width * Height + ((id.y / 2) * (Width / 2) + id.x / 2) * 2;
    float alpha = SampleMask((id.x + 0.5) / Width, (id.y + 0.5) / Height);
    float blendAlpha = AlphaFromMask != 0 ? 1.0 : alpha;
    float luma = Blend(Frame[lumaIndex], lumaIndex, 0, blendAlpha);
    float u = Blend(Frame[chromaIndex], chromaIndex, 1, blendAlpha);
    float v = Blend(Frame[chromaIndex + 1], chromaIndex + 1, 2, blendAlpha);
    float3 rgb = YuvToRgb((float)ToByte(luma), (float)ToByte(u), (float)ToByte(v), RedV, GreenU, GreenV, BlueU);
    uint outputAlpha = AlphaFromMask != 0 ? ToByte(alpha * 255.0) : 255;
    Output[lumaIndex] = ToByte(rgb.b * 255.0) | (ToByte(rgb.g * 255.0) << 8) | (ToByte(rgb.r * 255.0) << 16) | (outputAlpha << 24);
}
