// The feedback fixture's only pass (#32, #24/§4): out = 0.5*Source +
// 0.5*PassFeedback0, where PassFeedback0 is THIS pass's output from the PREVIOUS
// frame. Over successive frames the output decays toward the source (a leaky
// accumulator). At a fixed frame_index the rendered value is deterministic.
//
// SEPARATE texture2D + sampler form (wgpu-native): Source and the feedback twin
// share the one sampler at binding 2, exactly as the engine's feedback tests do.
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
void main() { gl_Position = global.MVP * Position; vTexCoord = TexCoord; }
#pragma stage fragment
layout(location = 0) in vec2 vTexCoord;
layout(location = 0) out vec4 FragColor;
layout(set = 0, binding = 1) uniform texture2D Source;
layout(set = 0, binding = 2) uniform sampler Smp;
layout(set = 0, binding = 3) uniform texture2D PassFeedback0;
void main() {
    vec4 s = texture(sampler2D(Source, Smp), vTexCoord);
    vec4 f = texture(sampler2D(PassFeedback0, Smp), vTexCoord);
    FragColor = 0.5 * s + 0.5 * f;
}
