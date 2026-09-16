cbuffer Params : register(b0)
{
    uint SourceWidth;
    uint SourceHeight;
    uint TargetWidth;
    uint TargetHeight;
    float RedV;
    float GreenU;
    float GreenV;
    float BlueU;
};

StructuredBuffer<uint> Nv12 : register(t0);
RWStructuredBuffer<uint> Tensor : register(u0);

uint ReadByte(uint index)
{
    return (Nv12[index >> 2] >> ((index & 3) * 8)) & 0xFF;
}

float SamplePlane(float x, float y, uint width, uint height, uint offset, uint channels, uint channel)
{
    float fx = clamp(x, 0.0, width - 1.0);
    float fy = clamp(y, 0.0, height - 1.0);
    uint x0 = (uint)fx;
    uint y0 = (uint)fy;
    uint x1 = min(x0 + 1, width - 1);
    uint y1 = min(y0 + 1, height - 1);
    float tx = fx - x0;
    float ty = fy - y0;
    uint stride = width * channels;
    float a = ReadByte(offset + y0 * stride + x0 * channels + channel);
    float b = ReadByte(offset + y0 * stride + x1 * channels + channel);
    float c = ReadByte(offset + y1 * stride + x0 * channels + channel);
    float d = ReadByte(offset + y1 * stride + x1 * channels + channel);
    return lerp(lerp(a, b, tx), lerp(c, d, tx), ty);
}

[numthreads(16, 16, 1)]
void main(uint3 id : SV_DispatchThreadID)
{
    uint pairs = TargetWidth / 2;
    if (id.x >= pairs || id.y >= TargetHeight)
    {
        return;
    }
    uint red = 0;
    uint green = 0;
    uint blue = 0;
    uint chromaOffset = SourceWidth * SourceHeight;
    for (uint i = 0; i < 2; i++)
    {
        float sx = (id.x * 2 + i + 0.5) * SourceWidth / TargetWidth - 0.5;
        float sy = (id.y + 0.5) * SourceHeight / TargetHeight - 0.5;
        float cx = (sx + 0.5) * 0.5 - 0.5;
        float cy = (sy + 0.5) * 0.5 - 0.5;
        float luma = SamplePlane(sx, sy, SourceWidth, SourceHeight, 0, 1, 0);
        float u = SamplePlane(cx, cy, SourceWidth / 2, SourceHeight / 2, chromaOffset, 2, 0);
        float v = SamplePlane(cx, cy, SourceWidth / 2, SourceHeight / 2, chromaOffset, 2, 1);
        float c = 1.1640625 * (luma - 16.0);
        float d = u - 128.0;
        float e = v - 128.0;
        float3 rgb = saturate(float3(c + RedV * e, c + GreenU * d + GreenV * e, c + BlueU * d) / 255.0);
        uint shift = 16 * i;
        red |= f32tof16(rgb.r) << shift;
        green |= f32tof16(rgb.g) << shift;
        blue |= f32tof16(rgb.b) << shift;
    }
    uint plane = TargetWidth * TargetHeight / 2;
    uint element = id.y * pairs + id.x;
    Tensor[element] = red;
    Tensor[plane + element] = green;
    Tensor[2 * plane + element] = blue;
}
