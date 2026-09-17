#include "frame_sampling.hlsli"

cbuffer Params : register(b0)
{
    uint FrameWidth;
    uint FrameHeight;
    uint ModelWidth;
    uint ModelHeight;
    float RedV;
    float GreenU;
    float GreenV;
    float BlueU;
};

RWStructuredBuffer<float> Frame : register(u0);
RWStructuredBuffer<uint> HalfTensor : register(u1);
RWStructuredBuffer<float> FloatTensor : register(u2);

float3 ModelPixel(uint x, uint y)
{
    float fx = (x + 0.5) * FrameWidth / ModelWidth;
    float fy = (y + 0.5) * FrameHeight / ModelHeight;
    uint chromaOffset = FrameWidth * FrameHeight;
    float luma = SamplePlane(Frame, fx, fy, FrameWidth, FrameHeight, 0, 1, 0);
    float u = SamplePlane(Frame, fx * 0.5, fy * 0.5, FrameWidth / 2, FrameHeight / 2, chromaOffset, 2, 0);
    float v = SamplePlane(Frame, fx * 0.5, fy * 0.5, FrameWidth / 2, FrameHeight / 2, chromaOffset, 2, 1);
    return YuvToRgb(luma, u, v, RedV, GreenU, GreenV, BlueU);
}

[numthreads(16, 16, 1)]
void planar_half(uint3 id : SV_DispatchThreadID)
{
    uint pairs = ModelWidth / 2;
    if (id.x >= pairs || id.y >= ModelHeight)
    {
        return;
    }
    uint red = 0;
    uint green = 0;
    uint blue = 0;
    for (uint i = 0; i < 2; i++)
    {
        float3 rgb = ModelPixel(id.x * 2 + i, id.y);
        uint shift = 16 * i;
        red |= f32tof16(rgb.r) << shift;
        green |= f32tof16(rgb.g) << shift;
        blue |= f32tof16(rgb.b) << shift;
    }
    uint plane = ModelWidth * ModelHeight / 2;
    uint element = id.y * pairs + id.x;
    HalfTensor[element] = red;
    HalfTensor[plane + element] = green;
    HalfTensor[2 * plane + element] = blue;
}

[numthreads(16, 16, 1)]
void interleaved_float(uint3 id : SV_DispatchThreadID)
{
    if (id.x >= ModelWidth || id.y >= ModelHeight)
    {
        return;
    }
    float3 rgb = ModelPixel(id.x, id.y);
    uint index = (id.y * ModelWidth + id.x) * 3;
    FloatTensor[index] = rgb.r;
    FloatTensor[index + 1] = rgb.g;
    FloatTensor[index + 2] = rgb.b;
}
