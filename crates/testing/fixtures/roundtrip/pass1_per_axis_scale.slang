// Round-trip fixture pass 1 (#37): PER-AXIS scale — x is `absolute` (320 px),
// y is `viewport` (x1) — plus `float_framebuffer`, `repeat` wrap, linear filter,
// and `frame_count_mod`. Reads the earlier alias `SOURCEPASS` and a LUT, proving
// the per-axis scale form and the format/sampler keys round-trip.
#version 450
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
layout(set = 0, binding = 1) uniform sampler2D SOURCEPASS;
layout(set = 0, binding = 2) uniform sampler2D GRADE;
void main() {
    FragColor = mix(texture(SOURCEPASS, vTexCoord), texture(GRADE, vTexCoord), 0.5);
}
