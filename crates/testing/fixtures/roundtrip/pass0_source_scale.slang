// Round-trip fixture pass 0 (#37): aliased `SOURCEPASS`, scale_type=source x2,
// nearest filter, sRGB framebuffer. Declares two `#pragma parameter` knobs, one
// of which the preset overrides. Combined `sampler2D` form (the corpus path).
//
// These fixtures exist to prove import -> export -> re-import is LOSSLESS, NOT to
// render: the harness reads the `.slang` BYTES verbatim and never compiles them.
#version 450
#pragma parameter BRIGHTNESS "Brightness" 1.0 0.0 2.0 0.01
#pragma parameter CONTRAST "Contrast" 1.0 0.5 1.5 0.01
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
void main() { FragColor = texture(Source, vTexCoord); }
