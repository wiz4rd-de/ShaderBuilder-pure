// Hand-written equivalent of the `color_invert` parity fixture (#44).
//
// The graph: screen-UV CustomSnippet -> Sample(Source) -> (vec4(1) - color)
// -> Output. This shader computes the SAME arithmetic the emitter generates
// (`vec4(1.0, 1.0, 1.0, 1.0) - texture(Source, vTexCoord)`, same operand order)
// so the generated and hand-written renders are byte-identical on a given
// adapter. The single-pass `.slangp` the harness wraps both in is identical
// (viewport scale, linear filter), so only the `.slang` source is under test.
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
    FragColor = vec4(1.0, 1.0, 1.0, 1.0) - color;
}
