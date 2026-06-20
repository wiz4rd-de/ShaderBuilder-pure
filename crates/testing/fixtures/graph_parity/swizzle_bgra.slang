// Hand-written equivalent of the `swizzle_bgra` parity fixture (#44).
//
// The graph: screen-UV CustomSnippet -> Sample(Source) -> Swizzle(.bgra) ->
// Output. A `.bgra` swizzle is operand-order-sensitive: it reverses R<->B and
// keeps G/A, so a wrong mask moves pixels (the fixed source is a left-red /
// right-green split, so swapping R and B turns the red half blue). This shader
// computes the SAME `texture(Source, vTexCoord).bgra` so the two renders are
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
    FragColor = color.bgra;
}
