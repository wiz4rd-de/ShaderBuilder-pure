// Final pass of the multipass fixture (#32): average the aliased pass-0 output
// (`FIRST` = inverted Original) with the true `Original`. Per channel that is
// 0.5 * (1 - c) + 0.5 * c = 0.5 (≈127) for ANY source — a stable golden that
// nonetheless proves:
//   * <FIRST> resolved to pass 0's output (the alias bind, #26), and
//   * Original resolved to the true source (not pass 0's output).
// If the alias had instead bound the original (a bug), the average would be the
// source itself, not a flat 0.5.
//
// Combined `sampler2D` form (the corpus path). `FIRST` and `Original` are bound
// by their semantic names from reflection.
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
layout(set = 0, binding = 1) uniform sampler2D FIRST;
layout(set = 0, binding = 2) uniform sampler2D Original;
void main() {
    vec4 first = texture(FIRST, vTexCoord);
    vec4 orig = texture(Original, vTexCoord);
    FragColor = 0.5 * first + 0.5 * orig;
}
