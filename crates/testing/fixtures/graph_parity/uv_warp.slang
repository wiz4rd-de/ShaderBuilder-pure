// Hand-written equivalent of the `uv_warp` parity fixture (#44).
//
// The graph: a CustomSnippet warps the per-fragment UV toward the center
// (`c + (vTexCoord - c) * 0.8`) and feeds it to Sample(Source) -> Output. This
// shader computes the SAME warp + sample so the two renders are byte-identical.
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
    vec2 c = vec2(0.5, 0.5);
    vec2 warped = c + (vTexCoord - c) * 0.8;
    FragColor = texture(Source, warped);
}
