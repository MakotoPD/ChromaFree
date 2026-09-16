cbuffer Params : register(b0)
{
    uint SourceWidth;
    uint SourceHeight;
    uint TargetWidth;
    uint TargetHeight;
    uint Plane;
    uint Reserved0;
    uint Reserved1;
    uint Reserved2;
    float MapXX;
    float MapXY;
    float MapX0;
    float MapYX;
    float MapYY;
    float MapY0;
    float Reserved3;
    float Reserved4;
};

StructuredBuffer<uint> Source : register(t0);
RWStructuredBuffer<float> Frame : register(u0);

uint ReadByte(uint index)
{
    return (Source[index >> 2] >> ((index & 3) * 8)) & 0xFF;
}

float SampleSource(float x, float y, uint width, uint height, uint offset, uint channels, uint channel)
{
    float fx = clamp(x - 0.5, 0.0, width - 1.0);
    float fy = clamp(y - 0.5, 0.0, height - 1.0);
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
    uint width = Plane == 0 ? TargetWidth : TargetWidth / 2;
    uint height = Plane == 0 ? TargetHeight : TargetHeight / 2;
    if (id.x >= width || id.y >= height)
    {
        return;
    }
    float scale = Plane == 0 ? 1.0 : 2.0;
    float ox = (id.x + 0.5) * scale;
    float oy = (id.y + 0.5) * scale;
    float sx = MapXX * ox + MapXY * oy + MapX0;
    float sy = MapYX * ox + MapYY * oy + MapY0;
    if (Plane == 0)
    {
        Frame[id.y * TargetWidth + id.x] = SampleSource(sx, sy, SourceWidth, SourceHeight, 0, 1, 0);
        return;
    }
    uint chromaOffset = SourceWidth * SourceHeight;
    uint index = TargetWidth * TargetHeight + (id.y * width + id.x) * 2;
    Frame[index] = SampleSource(sx * 0.5, sy * 0.5, SourceWidth / 2, SourceHeight / 2, chromaOffset, 2, 0);
    Frame[index + 1] = SampleSource(sx * 0.5, sy * 0.5, SourceWidth / 2, SourceHeight / 2, chromaOffset, 2, 1);
}
