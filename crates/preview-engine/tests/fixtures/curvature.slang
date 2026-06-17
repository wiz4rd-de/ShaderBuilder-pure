// One-pass barrel/pincushion curvature (warp) shader — the Phase 1 exit fixture.
//
// Remaps the sampling coordinate so the image bulges outward (barrel
// distortion): centered coords `p` are pushed away from the center by a radial
// factor, so the corners pull inward toward the center and the framebuffer
// corners fall outside the source and read black. This is a clear, non-no-op
// warp that is most visible in the corner regions — exactly what the E2E test
// asserts on.
//
// Convention (matches `preview-engine::uniforms` + the renderer bind group):
//   * full canonical builtin UBO at set=0 binding=0 (we use SourceSize for the
//     aspect correction so the warp is radially symmetric on non-square images),
//   * separate texture2D + sampler at bindings 1/2 (NOT combined sampler2D),
//   * standard MVP passthrough vertex stage.
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
void main() {
    gl_Position = global.MVP * Position;
    vTexCoord = TexCoord;
}

#pragma stage fragment
layout(location = 0) in vec2 vTexCoord;
layout(location = 0) out vec4 FragColor;
layout(set = 0, binding = 1) uniform texture2D Source;
layout(set = 0, binding = 2) uniform sampler Smp;

// Barrel-distortion strength. Large enough that the corners warp visibly.
const float K = 0.25;

void main() {
    // To centered coordinates in [-1, 1].
    vec2 p = vTexCoord * 2.0 - 1.0;

    // Aspect correction: scale x by the source aspect ratio so the radial term
    // is isotropic on non-square images, then undo it after distorting.
    float aspect = global.SourceSize.x * global.SourceSize.w; // w/h (w * 1/h)
    p.x *= aspect;

    // Radial barrel distortion: push samples outward by 1 + K*r^2.
    p *= 1.0 + K * dot(p, p);

    p.x /= aspect;

    // Back to texture space.
    vec2 uv = p * 0.5 + 0.5;

    // Outside the source -> black, so the warped corners darken/shift clearly.
    if (uv.x < 0.0 || uv.x > 1.0 || uv.y < 0.0 || uv.y > 1.0) {
        FragColor = vec4(0.0, 0.0, 0.0, 1.0);
    } else {
        FragColor = texture(sampler2D(Source, Smp), uv);
    }
}
