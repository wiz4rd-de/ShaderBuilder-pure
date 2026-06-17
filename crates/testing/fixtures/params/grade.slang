// Parameter + multi-LUT import fixture (#35). One pass that declares SEVERAL
// `#pragma parameter` knobs and samples TWO LUTs (`GRADE`, `BORDER`) with
// differing per-texture sampler settings. The params are not referenced in the
// GLSL body (they live in the engine-generated parameter block, like RetroArch),
// so the pass compiles and renders deterministically through the fuzz/golden
// harness — while exercising the importer's pragma-parameter extraction and the
// reconcile-with-overrides path (see params.slangp).
//
// LOCAL fixture, NOT a RetroArch reference. Both LUTs reuse the tiny committed
// 2x2 PNG so the fixture is self-contained.
#version 450
#pragma parameter BRIGHTNESS "Brightness" 1.0 0.0 2.0 0.01
#pragma parameter CONTRAST "Contrast" 1.0 0.5 1.5 0.01
#pragma parameter SATURATION "Saturation" 1.0 0.0 2.0 0.05
#pragma parameter GAMMA "Gamma" 2.2 1.0 3.0 0.1
layout(std140, set = 0, binding = 0) uniform UBO { mat4 MVP; } global;
#pragma stage vertex
layout(location = 0) in vec4 Position;
layout(location = 1) in vec2 TexCoord;
layout(location = 0) out vec2 vTexCoord;
void main() { gl_Position = global.MVP * Position; vTexCoord = TexCoord; }
#pragma stage fragment
layout(location = 0) in vec2 vTexCoord;
layout(location = 0) out vec4 FragColor;
layout(set = 0, binding = 1) uniform sampler2D Source;
layout(set = 0, binding = 2) uniform sampler2D GRADE;
layout(set = 0, binding = 3) uniform sampler2D BORDER;
void main() {
    vec4 src = texture(Source, vTexCoord);
    vec4 grade = texture(GRADE, vTexCoord);
    vec4 border = texture(BORDER, vTexCoord);
    FragColor = mix(src * grade, border, 0.25);
}
