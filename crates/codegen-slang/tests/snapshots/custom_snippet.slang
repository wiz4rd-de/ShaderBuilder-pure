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

void main()
{
    float t0 = 1.25;
    vec2 t1 = vec2(0.5, 0.5);
    vec4 t2 = texture(Source, t1);
#define in_color t2
#define gain t0
    vec4 out_color;
    out_color = clamp(in_color * gain, 0.0, 1.0);
#undef in_color
#undef gain
    vec4 t3 = out_color;
    FragColor = t3;
}
