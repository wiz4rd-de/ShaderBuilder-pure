#version 450

#pragma name two_snippets
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

void snippet_snipA(in vec4 in_a, out vec4 out_a)
{
    float t = 0.5;
    out_a = vec4(in_a.r + t, in_a.g, in_a.b, in_a.a);
}

void snippet_snipB(in vec4 in_b, out vec4 out_b)
{
    float t = 0.25;
    out_b = vec4(in_b.r, in_b.g + t, in_b.b, in_b.a);
}

void main()
{
    vec2 t0 = vec2(0.5, 0.5);
    vec4 t1 = texture(Source, t0);
    vec4 snippet_snipA_out_a;
    snippet_snipA(t1, snippet_snipA_out_a);
    vec4 t2 = snippet_snipA_out_a;
    vec4 snippet_snipB_out_b;
    snippet_snipB(t1, snippet_snipB_out_b);
    vec4 t3 = snippet_snipB_out_b;
    vec4 t4 = (t2 + t3);
    FragColor = t4;
}
