cbuffer Params : register(b0)
{
    uint Width;
    uint Height;
    uint UseHistory;
    uint Reserved0;
    float Keep;
    float EdgeLow;
    float EdgeHigh;
    float Reserved1;
};

StructuredBuffer<float> UploadedAlpha : register(t0);
RWStructuredBuffer<uint> HalfAlpha : register(u0);
RWStructuredBuffer<float> History : register(u1);
RWStructuredBuffer<float> Mask : register(u2);
RWStructuredBuffer<float> FloatAlpha : register(u3);

void Refine(uint index, float alpha)
{
    float value = saturate(alpha);
    [branch] if (UseHistory != 0)
    {
        value = lerp(value, History[index], Keep);
        History[index] = value;
    }
    float t = saturate((value - EdgeLow) / max(EdgeHigh - EdgeLow, 1e-6));
    Mask[index] = t * t * (3.0 - 2.0 * t);
}

[numthreads(16, 16, 1)]
void from_half(uint3 id : SV_DispatchThreadID)
{
    if (id.x >= Width || id.y >= Height)
    {
        return;
    }
    uint index = id.y * Width + id.x;
    Refine(index, f16tof32((HalfAlpha[index >> 1] >> ((index & 1) * 16)) & 0xFFFF));
}

[numthreads(16, 16, 1)]
void from_float(uint3 id : SV_DispatchThreadID)
{
    if (id.x >= Width || id.y >= Height)
    {
        return;
    }
    uint index = id.y * Width + id.x;
    Refine(index, FloatAlpha[index]);
}

[numthreads(16, 16, 1)]
void from_upload(uint3 id : SV_DispatchThreadID)
{
    if (id.x >= Width || id.y >= Height)
    {
        return;
    }
    uint index = id.y * Width + id.x;
    Refine(index, UploadedAlpha[index]);
}
