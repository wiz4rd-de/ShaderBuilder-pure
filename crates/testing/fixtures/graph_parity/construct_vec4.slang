// Hand-written equivalent of the `construct_vec4` parity fixture (#44).
//
// The graph: screen-UV CustomSnippet -> Sample(Source) -> Swizzle(.rgb) ->
// Construct(vec4) from (the vec3 rgb, a const float alpha 1.0) -> Output. The
// construct packs a vec3 + a scalar into a vec4 in that exact operand order, so
// a wrong constructor argument order/grouping moves pixels. This shader computes
// the SAME `vec4(texture(Source, vTexCoord).rgb, 1.0)` so the two renders are
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
    vec3 rgb = color.rgb;
    FragColor = vec4(rgb, 1.0);
}
