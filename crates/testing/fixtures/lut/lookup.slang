// The LUT fixture's only pass (#32, #27/§7): output the LUT `PAL` sampled across
// the screen, ignoring the source entirely. With a 2x2 nearest LUT the output is
// the four LUT texels in the four screen quadrants — a deterministic golden that
// proves the LUT bound by name with real content and its own sampler.
//
// Combined `sampler2D` form (the corpus path). `PAL` is neither Source/Original
// /Pass*/History*, so it classifies as a LUT and binds the registered texture.
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
layout(set = 0, binding = 1) uniform sampler2D PAL;
void main() { FragColor = texture(PAL, vTexCoord); }
