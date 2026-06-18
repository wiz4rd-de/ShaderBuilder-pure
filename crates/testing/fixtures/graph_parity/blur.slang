// Hand-written equivalent of the `blur` parity fixture (#44).
//
// The graph is a 3-tap horizontal box blur: sample Source at vTexCoord offset by
// -0.125 / 0 / +0.125 in x (one 8-px source texel), then average. This shader
// computes the SAME taps and the SAME `((sL + sC) + sR) * (1/3)` reduction, with
// the 1/3 spelled as the emitter's f32 literal `0.33333334`, so the generated and
// hand-written renders are byte-identical.
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
    vec4 sL = texture(Source, vTexCoord - vec2(0.125, 0.0));
    vec4 sC = texture(Source, vTexCoord);
    vec4 sR = texture(Source, vTexCoord + vec2(0.125, 0.0));
    FragColor = ((sL + sC) + sR) * 0.33333334;
}
