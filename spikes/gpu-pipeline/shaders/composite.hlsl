cbuffer Params : register(b0)
{
    uint Width;
    uint Height;
    uint AlphaWidth;
    uint AlphaHeight;
    float BackgroundY;
    float BackgroundU;
    float BackgroundV;
    float Unused;
};

StructuredBuffer<uint> Nv12 : register(t0);
RWStructuredBuffer<uint> Output : register(u0);
RWStructuredBuffer<uint> Alpha : register(u1);

uint ReadByte(uint index)
{
    return (Nv12[index >> 2] >> ((index & 3) * 8)) & 0xFF;
}

float ReadAlpha(uint x, uint y)
{
    uint index = y * AlphaWidth + x;
    return saturate(f16tof32((Alpha[index >> 1] >> ((index & 1) * 16)) & 0xFFFF));
}

float SampleAlpha(float u, float v)
{
    float fx = clamp(u * AlphaWidth - 0.5, 0.0, AlphaWidth - 1.0);
    float fy = clamp(v * AlphaHeight - 0.5, 0.0, AlphaHeight - 1.0);
    uint x0 = (uint)fx;
    uint y0 = (uint)fy;
    uint x1 = min(x0 + 1, AlphaWidth - 1);
    uint y1 = min(y0 + 1, AlphaHeight - 1);
    float tx = fx - x0;
    float ty = fy - y0;
    return lerp(lerp(ReadAlpha(x0, y0), ReadAlpha(x1, y0), tx), lerp(ReadAlpha(x0, y1), ReadAlpha(x1, y1), tx), ty);
}

[numthreads(16, 16, 1)]
void main(uint3 id : SV_DispatchThreadID)
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
            float alpha = SampleAlpha((column + 0.5) / Width, (id.y + 0.5) / Height);
            value = lerp(BackgroundY, ReadByte(id.y * Width + column), alpha);
        }
        else
        {
            uint chromaRow = id.y - Height;
            uint chromaColumn = column / 2;
            float alpha = SampleAlpha((chromaColumn + 0.5) / (Width / 2), (chromaRow + 0.5) / (Height / 2));
            float background = (column & 1) == 0 ? BackgroundU : BackgroundV;
            value = lerp(background, ReadByte(Width * Height + chromaRow * Width + column), alpha);
        }
        packed |= ((uint)(value + 0.5) & 0xFF) << (8 * i);
    }
    Output[id.y * elementsPerRow + id.x] = packed;
}
