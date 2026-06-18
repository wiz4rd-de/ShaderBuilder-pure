#version 450

#pragma name snippet
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

void snippet_snippet(in vec4 in_color, in float gain, out vec4 out_color)
{
    out_color = clamp(in_color * gain, 0.0, 1.0);
}

void main()
{
    float t0 = 1.25;
    vec2 t1 = vec2(0.5, 0.5);
    vec4 t2 = texture(Source, t1);
    vec4 snippet_snippet_out_color;
    snippet_snippet(t2, t0, snippet_snippet_out_color);
    vec4 t3 = snippet_snippet_out_color;
    FragColor = t3;
}
