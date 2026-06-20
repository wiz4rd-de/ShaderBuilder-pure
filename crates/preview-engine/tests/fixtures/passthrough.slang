// Passthrough control shader (Phase 1 exit E2E).
//
// Samples the Source texture at the unmodified vTexCoord, so the output should
// reproduce the (possibly downsampled) input image within filtering tolerance.
// It declares the full canonical builtin UBO so it shares the exact bind-group
// layout the renderer expects (see `preview-engine::uniforms`); separate
// texture2D + sampler per wgpu's binding model (NOT combined sampler2D).
#version 450

layout(std140, set = 0, binding = 0) uniform UBO {
    mat4 MVP;
    vec4 SourceSize;
    vec4 OriginalSize;
    vec4 OutputSize;
    uint FrameCount;
} global;

#pragma stage vertex
layout(location = 0) in vec4 Position;
layout(location = 1) in vec2 TexCoord;
layout(location = 0) out vec2 vTexCoord;
void main() {
    gl_Position = global.MVP * Position;
    vTexCoord = TexCoord;
}

#pragma stage fragment
layout(location = 0) in vec2 vTexCoord;
layout(location = 0) out vec4 FragColor;
layout(set = 0, binding = 1) uniform texture2D Source;
layout(set = 0, binding = 2) uniform sampler Smp;
void main() {
    FragColor = texture(sampler2D(Source, Smp), vTexCoord);
}
