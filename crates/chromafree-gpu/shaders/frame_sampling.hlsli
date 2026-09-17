float SamplePlane(RWStructuredBuffer<float> buffer, float x, float y, uint width, uint height, uint offset, uint channels, uint channel)
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
    float a = buffer[offset + y0 * stride + x0 * channels + channel];
    float b = buffer[offset + y0 * stride + x1 * channels + channel];
    float c = buffer[offset + y1 * stride + x0 * channels + channel];
    float d = buffer[offset + y1 * stride + x1 * channels + channel];
    return lerp(lerp(a, b, tx), lerp(c, d, tx), ty);
}

float3 YuvToRgb(float luma, float u, float v, float redV, float greenU, float greenV, float blueU)
{
    float c = 1.1640625 * (luma - 16.0);
    float d = u - 128.0;
    float e = v - 128.0;
    return saturate(float3(c + redV * e, c + greenU * d + greenV * e, c + blueU * d) / 255.0);
}
