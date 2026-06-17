// Round-trip fixture pass 3 (#37): the FINAL pass — scale_type=viewport x1, the
// `mirrored_repeat` wrap mode, and a reference to the second LUT `BORDER`. Proves
// the viewport scale type and the last wrap-mode variant round-trip.
#version 450
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
layout(set = 0, binding = 2) uniform sampler2D BORDER;
void main() {
    FragColor = mix(texture(Source, vTexCoord), texture(BORDER, vTexCoord), 0.25);
}
