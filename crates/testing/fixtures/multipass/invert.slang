// Pass 0 of the multipass fixture (#32): invert the source's RGB. Combined
// `sampler2D` form (the corpus path). Aliased as FIRST and rendered into a
// source x2 FBO so the final pass reads a real intermediate.
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
layout(set = 0, binding = 1) uniform sampler2D Source;
void main() {
    vec4 c = texture(Source, vTexCoord);
    FragColor = vec4(vec3(1.0) - c.rgb, c.a);
}
