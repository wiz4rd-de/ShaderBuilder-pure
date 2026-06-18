// Hand-written equivalent of the `snippet_channel_swap` parity fixture (#44).
//
// The graph: screen-UV -> Sample(Source) -> a CustomSnippet that swaps R and B
// (`vec4(in_color.b, in_color.g, in_color.r, in_color.a)`) -> Output. This shader
// computes the SAME channel swap so the two renders are byte-identical, exercising
// the CustomSnippet op end-to-end.
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
    vec4 in_color = texture(Source, vTexCoord);
    FragColor = vec4(in_color.b, in_color.g, in_color.r, in_color.a);
}
