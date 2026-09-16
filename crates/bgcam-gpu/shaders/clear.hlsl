cbuffer Params : register(b0)
{
    uint Count;
};

RWStructuredBuffer<uint> Target : register(u0);

[numthreads(256, 1, 1)]
void main(uint3 id : SV_DispatchThreadID)
{
    if (id.x >= Count)
    {
        return;
    }
    Target[id.x] = 0;
}
