// Hand-written equivalent of the `builtin_framecount` parity fixture (#44).
//
// The graph: screen-UV CustomSnippet -> Sample(Source) as the color; a
// Builtin(FrameCount) -> Mul(FrameCount, 0.25) -> Fract -> a scalar `phase`;
// then Mul(color, phase) -> Output. FrameCount is a `uint` in the params block
// but typed `int` in the IR, so the emitter reads it through an `int(...)` cast;
// this fixture renders at frame_index = 2 (FrameCount == 2) so the cast and the
// live counter VALUE both matter: phase = fract(2 * 0.25) = fract(0.5) = 0.5, so
// the output is the source at half brightness. A wrong cast (or a baked/zero
// FrameCount) would change the value and move pixels. This shader computes the
// SAME `texture(Source, vTexCoord) * fract(int(params.FrameCount) * 0.25)` so the
// two renders are byte-identical on a given adapter.
#version 450

layout(push_constant) uniform Push
{
    uint FrameCount;
} params;

#pragma format R8G8B8A8_UNORM

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
    float phase = fract(int(params.FrameCount) * 0.25);
    FragColor = color * phase;
}
