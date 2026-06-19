#version 450

// Example preset — pass 0: CRT screen CURVATURE (barrel distortion).
//
// Warps the source UVs outward from the centre so the image bulges like an old
// curved CRT tube, then samples the source through the warped coordinate. UVs
// that fall off the (now curved) screen are darkened to a black border.
//
// This is the first pass of the bundled "CRT Scanlines + Curvature" example
// project shipped with ShaderBuilder (docs/example-project.md). It is authored
// as plain whole-pass slang so it compiles, previews, and exports unchanged.

layout(push_constant) uniform Push
{
	vec4 SourceSize;
	vec4 OriginalSize;
	vec4 OutputSize;
	uint FrameCount;
	float CURVATURE;
	float CORNER_SMOOTH;
} params;

#pragma parameter CURVATURE "Screen Curvature" 0.10 0.0 0.40 0.01
#pragma parameter CORNER_SMOOTH "Corner Vignette" 0.03 0.0 0.20 0.01

layout(std140, set = 0, binding = 0) uniform UBO
{
	mat4 MVP;
} global;

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

// Barrel-distort a centred UV by `amount`.
vec2 curve(vec2 uv, float amount)
{
	uv = uv * 2.0 - 1.0;
	vec2 offset = abs(uv.yx) / vec2(6.0, 5.0);
	uv = uv + uv * offset * offset * amount * 10.0;
	return uv * 0.5 + 0.5;
}

void main()
{
	vec2 uv = curve(vTexCoord, params.CURVATURE);

	// Darken the corners and anything that warped off-screen.
	vec2 edge = smoothstep(vec2(0.0), vec2(params.CORNER_SMOOTH + 0.001), uv)
	          * smoothstep(vec2(0.0), vec2(params.CORNER_SMOOTH + 0.001), vec2(1.0) - uv);
	float vignette = clamp(edge.x * edge.y, 0.0, 1.0);

	vec3 color = texture(Source, clamp(uv, 0.0, 1.0)).rgb;
	color *= vignette;

	FragColor = vec4(color, 1.0);
}
