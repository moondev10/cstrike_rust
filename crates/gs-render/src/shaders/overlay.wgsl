// The 2D overlay (HUD sprites, fills, VGUI2 glyphs) in screen pixels, origin top-left
// (docs/re/client-hud.md §2.3).

struct Screen {
    // xy: width, height in pixels
    size: vec4<f32>,
};

@group(0) @binding(0) var<uniform> screen: Screen;
@group(1) @binding(0) var tex: texture_2d<f32>;
@group(3) @binding(0) var tex_sampler: sampler;

struct VertexIn {
    @location(0) pos: vec2<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) color: vec4<f32>,
};

struct VertexOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
};

@vertex
fn vs_main(in: VertexIn) -> VertexOut {
    var out: VertexOut;
    out.clip = vec4<f32>(in.pos.x / screen.size.x * 2.0 - 1.0, 1.0 - in.pos.y / screen.size.y * 2.0, 0.0, 1.0);
    out.uv = in.uv;
    out.color = in.color;
    return out;
}

// Untextured fills.
@fragment
fn fs_flat(in: VertexOut) -> @location(0) vec4<f32> {
    return in.color;
}

// GL_MODULATE.
@fragment
fn fs_textured(in: VertexOut) -> @location(0) vec4<f32> {
    return textureSample(tex, tex_sampler, in.uv) * in.color;
}

// SPR_DrawHoles: alpha test (GL_NOTEQUAL 0).
@fragment
fn fs_holes(in: VertexOut) -> @location(0) vec4<f32> {
    let c = textureSample(tex, tex_sampler, in.uv) * in.color;
    if (c.a == 0.0) {
        discard;
    }
    return c;
}
