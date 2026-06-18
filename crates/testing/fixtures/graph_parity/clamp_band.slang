// Hand-written equivalent of the `clamp_band` parity fixture (#44).
//
// The graph: screen-UV CustomSnippet -> Sample(Source) as `x`, a const float
// 0.25 as `lo`, a const float 0.65 as `hi` -> Clamp(x, lo, hi) -> Output.
// `clamp(x, lo, hi)` is operand-order-sensitive (lo before hi); with the source
// channels spanning 0.078..0.784 the clamp band [0.25, 0.65] genuinely moves
// pixels (low channels clamp up, high channels clamp down) and a swapped lo/hi
// would invert/break it. This shader computes the SAME
// `clamp(texture(Source, vTexCoord), 0.25, 0.65)` so the two renders are
// byte-identical on a given adapter.
#version 450

layout(std140, set = 0, binding = 0) uniform UBO
{
    mat4 MVP;
} global;

#pragma stage vertex
layout(location = 0) in vec4 Position;
layout(location = 1) in vec2 TexCoord;
layout(location = 0) out vec2 vTexCoord;
void main()
{
    gl_Position = global.MVP * Position;
    vTexCoord = TexCoord;
}

#pragma stage fragment
layout(location = 0) in vec2 vTexCoord;
layout(location = 0) out vec4 FragColor;
layout(set = 0, binding = 2) uniform sampler2D Source;
void main()
{
    vec4 color = texture(Source, vTexCoord);
    FragColor = clamp(color, 0.25, 0.65);
}
