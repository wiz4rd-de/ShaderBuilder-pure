// Orphan parameter-override fixture body (FINDING A1).
//
// This pass declares a `#pragma parameter` for REAL_KNOB inline, but declares
// NOTHING for HEADER_KNOB. In a real preset HEADER_KNOB's pragma would live in an
// `#include`d header the importer does not resolve; here we simply omit it, which
// is the same condition the importer sees: the pragma scan finds no declaration
// for HEADER_KNOB, so the preset's bare `HEADER_KNOB = 0.625` override binds to no
// scanned pragma — it is the "orphan" the importer must preserve, not drop.
//
// (No `#include` is used so the body compiles cleanly through the fuzz/golden
// harness; the import-side pragma scan reads only this file's own declarations,
// which is exactly what produces the orphan.)
//
// LOCAL fixture, NOT a RetroArch reference; the harness reads this as bytes.
#version 450
#pragma parameter REAL_KNOB "Real knob" 0.5 0.0 1.0 0.01
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
