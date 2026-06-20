// Hand-written equivalent of the `div_scale` parity fixture (#44).
//
// The graph: screen-UV CustomSnippet -> Sample(Source) as `a`, a const float
// 2.0 as `b` -> Div(a, b) -> Output. Division is operand-order-sensitive
// (`a / b` != `b / a`): dividing the source by 2.0 halves every channel, which
// varies across the red/green source split. A swapped numerator/denominator
// would compute `2.0 / color` and move pixels far. This shader computes the SAME
// `texture(Source, vTexCoord) / 2.0` so the two renders are byte-identical on a
// given adapter.
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
    FragColor = color / 2.0;
}
