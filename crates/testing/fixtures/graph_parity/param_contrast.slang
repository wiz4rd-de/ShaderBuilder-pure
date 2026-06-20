// Hand-written equivalent of the `param_contrast` parity fixture (#44).
//
// The graph applies a `#pragma parameter`-driven contrast to the sampled color:
// `((color - 0.5) * CONTRAST) + 0.5`, with CONTRAST read live from the params
// block. This shader declares the SAME `#pragma parameter` and computes the SAME
// expression (same operand order, the 0.5 reused for both the subtract and the
// add) so the generated and hand-written renders are byte-identical under any
// CONTRAST value — including the live override the param-driven test pushes.
#version 450

layout(push_constant) uniform Push
{
    float CONTRAST;
} params;

#pragma parameter CONTRAST "Contrast" 1.3 0.0 2.0 0.01

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
    float mid = 0.5;
    vec4 color = texture(Source, vTexCoord);
    vec4 centered = color - mid;
    vec4 scaled = centered * params.CONTRAST;
    FragColor = scaled + mid;
}
