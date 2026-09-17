#include "frame_sampling.hlsli"

cbuffer Params : register(b0)
{
    uint SourceWidth;
    uint SourceHeight;
    uint TargetWidth;
    uint TargetHeight;
    uint FrameWidth;
    uint FrameHeight;
    uint Reserved0;
    uint Reserved1;
};

RWStructuredBuffer<float> Source : register(u0);
RWStructuredBuffer<float> Target : register(u1);

[numthreads(16, 16, 1)]
void to_half(uint3 id : SV_DispatchThreadID)
{
    uint width = FrameWidth / 2;
    uint height = FrameHeight / 2;
    if (id.x >= width || id.y >= height)
    {
        return;
    }
    uint x0 = id.x * 2;
    uint y0 = id.y * 2;
    float luma = Source[y0 * FrameWidth + x0] + Source[y0 * FrameWidth + x0 + 1] + Source[(y0 + 1) * FrameWidth + x0] + Source[(y0 + 1) * FrameWidth + x0 + 1];
    uint chroma = FrameWidth * FrameHeight + (id.y * width + id.x) * 2;
    uint index = (id.y * width + id.x) * 3;
    Target[index] = luma * 0.25;
    Target[index + 1] = Source[chroma];
    Target[index + 2] = Source[chroma + 1];
}

[numthreads(16, 16, 1)]
void downsample(uint3 id : SV_DispatchThreadID)
{
    if (id.x >= TargetWidth || id.y >= TargetHeight)
    {
        return;
    }
    uint x0 = id.x * 2;
    uint y0 = id.y * 2;
    uint x1 = min(x0 + 1, SourceWidth - 1);
    uint y1 = min(y0 + 1, SourceHeight - 1);
    uint target = (id.y * TargetWidth + id.x) * 3;
    for (uint c = 0; c < 3; c++)
    {
        float sum = Source[(y0 * SourceWidth + x0) * 3 + c] + Source[(y0 * SourceWidth + x1) * 3 + c] + Source[(y1 * SourceWidth + x0) * 3 + c] + Source[(y1 * SourceWidth + x1) * 3 + c];
        Target[target + c] = sum * 0.25;
    }
}

[numthreads(16, 16, 1)]
void resize(uint3 id : SV_DispatchThreadID)
{
    if (id.x >= TargetWidth || id.y >= TargetHeight)
    {
        return;
    }
    float fx = (id.x + 0.5) * SourceWidth / TargetWidth;
    float fy = (id.y + 0.5) * SourceHeight / TargetHeight;
    uint target = (id.y * TargetWidth + id.x) * 3;
    for (uint c = 0; c < 3; c++)
    {
        Target[target + c] = SamplePlane(Source, fx, fy, SourceWidth, SourceHeight, 0, 3, c);
    }
}

[numthreads(16, 16, 1)]
void tent(uint3 id : SV_DispatchThreadID)
{
    if (id.x >= TargetWidth || id.y >= TargetHeight)
    {
        return;
    }
    uint target = (id.y * TargetWidth + id.x) * 3;
    for (uint c = 0; c < 3; c++)
    {
        float sum = 0.0;
        for (int dy = -1; dy <= 1; dy++)
        {
            for (int dx = -1; dx <= 1; dx++)
            {
                uint x = (uint)clamp((int)id.x + dx, 0, (int)TargetWidth - 1);
                uint y = (uint)clamp((int)id.y + dy, 0, (int)TargetHeight - 1);
                float weight = (dx == 0 ? 2.0 : 1.0) * (dy == 0 ? 2.0 : 1.0);
                sum += weight * Source[(y * TargetWidth + x) * 3 + c];
            }
        }
        Target[target + c] = sum / 16.0;
    }
}

[numthreads(16, 16, 1)]
void from_half(uint3 id : SV_DispatchThreadID)
{
    if (id.x >= FrameWidth || id.y >= FrameHeight)
    {
        return;
    }
    uint halfWidth = FrameWidth / 2;
    uint halfHeight = FrameHeight / 2;
    Target[id.y * FrameWidth + id.x] = SamplePlane(Source, (id.x + 0.5) * 0.5, (id.y + 0.5) * 0.5, halfWidth, halfHeight, 0, 3, 0);
    if ((id.x & 1) == 0 && (id.y & 1) == 0)
    {
        uint half = (id.y / 2 * halfWidth + id.x / 2) * 3;
        uint chroma = FrameWidth * FrameHeight + (id.y / 2 * halfWidth + id.x / 2) * 2;
        Target[chroma] = Source[half + 1];
        Target[chroma + 1] = Source[half + 2];
    }
}
