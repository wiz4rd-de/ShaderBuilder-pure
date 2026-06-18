// Hand-written equivalent of the `mix_blend` parity fixture (#44).
//
// The graph: screen-UV CustomSnippet -> Sample(Source) as `a`, a const blue
// vec4 as `b`, a const float 0.25 as `t` -> Mix(a, b, t) -> Output. `mix(a,b,t)`
// is operand-order-sensitive: it returns `a*(1-t)+b*t`, so swapping a/b (or
// using a wrong t) moves pixels. With t=0.25 the result is 75% source + 25%
// blend, which varies across the red/green source split. This shader computes
// the SAME `mix(texture(Source, vTexCoord), vec4(0.0,0.0,1.0,1.0), 0.25)` so the
// two renders are byte-identical on a given adapter.
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
    vec4 blend = vec4(0.0, 0.0, 1.0, 1.0);
    FragColor = mix(color, blend, 0.25);
}
