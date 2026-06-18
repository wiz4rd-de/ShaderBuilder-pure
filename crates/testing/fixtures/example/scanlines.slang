#version 450

// Example preset — pass 1: CRT SCANLINES.
//
// Modulates the (already curvature-warped) image from pass 0 with a vertical
// sine pattern locked to the source's vertical resolution, so each scan row gets
// a soft dark gap like a CRT's raster lines. A weight knob fades the effect.
//
// Second pass of the bundled "CRT Scanlines + Curvature" example project
// (docs/example-project.md). Plain whole-pass slang: compiles, previews, exports.

layout(push_constant) uniform Push
{
	vec4 SourceSize;
	vec4 OriginalSize;
	vec4 OutputSize;
	uint FrameCount;
	float SCANLINE_WEIGHT;
	float BRIGHTNESS;
} params;

#pragma parameter SCANLINE_WEIGHT "Scanline Strength" 0.40 0.0 1.0 0.05
#pragma parameter BRIGHTNESS "Brightness Boost" 1.15 0.5 2.0 0.05

layout(std140, set = 0, binding = 0) uniform UBO
{
	mat4 MVP;
} global;

#define PI 3.141592654

#pragma stage vertex
layout(location = 0) in vec4 Position;
layout(location = 1) in vec2 TexCoord;
layout(location = 0) out vec2 vTexCoord;

void main()
{
	gl_Position = global.MVP * Position;
	vTexCoord = TexCoord;
}

#pragma stage fragment
layout(location = 0) in vec2 vTexCoord;
layout(location = 0) out vec4 FragColor;
layout(set = 0, binding = 2) uniform sampler2D Source;

void main()
{
	vec3 color = texture(Source, vTexCoord).rgb * params.BRIGHTNESS;

	// One dark band per source row.
	float lines = sin(vTexCoord.y * params.SourceSize.y * PI);
	float scan = 1.0 - params.SCANLINE_WEIGHT * (lines * lines);

	FragColor = vec4(color * scan, 1.0);
}
