// Brush surfaces (docs/re/render.md §A.4–A.9): texture × baked lightmap, warped water, skybox,
// and the kRender* brush entity modes.

struct Frame {
    view_proj: mat4x4<f32>,
    eye: vec4<f32>,
    // x: time (s), y: wave height, z: zmax
    params: vec4<f32>,
    forward: vec4<f32>,
    fog_color: vec4<f32>,
    // x: 0 off, 1 GL_LINEAR, 2 GL_EXP2; y: density; z: start; w: end
    fog: vec4<f32>,
};

struct Model {
    transform: mat4x4<f32>,
    // r_blend in every component
    blend: vec4<f32>,
    // rendercolor / 256 (kRenderTransColor)
    color: vec4<f32>,
};

@group(0) @binding(0) var<uniform> frame: Frame;
@group(0) @binding(1) var lightmap: texture_2d<f32>;
@group(0) @binding(2) var lightmap_sampler: sampler;
@group(0) @binding(3) var sky: texture_2d_array<f32>;
@group(0) @binding(4) var sky_sampler: sampler;

@group(1) @binding(0) var diffuse: texture_2d<f32>;
@group(3) @binding(0) var diffuse_sampler: sampler;

@group(2) @binding(0) var<uniform> model: Model;

struct VertexIn {
    @location(0) pos: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) lm: vec2<f32>,
    @location(3) aux: f32,
};

struct VertexOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) lm: vec2<f32>,
    @location(2) world: vec3<f32>,
};

@vertex
fn vs_main(in: VertexIn) -> VertexOut {
    var out: VertexOut;
    let world = (model.transform * vec4<f32>(in.pos, 1.0)).xyz;
    out.clip = frame.view_proj * vec4<f32>(world, 1.0);
    out.uv = in.uv;
    out.lm = in.lm;
    out.world = world;
    return out;
}

// turbsin[(u8)(int)x]: 8·sin(2π·i/256), index truncated toward zero then wrapped to a byte.
fn turbsin(x: f32) -> f32 {
    let i = i32(x) & 255;
    return 8.0 * sin(6.283185307179586 * f32(i) / 256.0);
}

// EmitWaterPolys: per-vertex warp of the 64-unit subdivided polygons; uv holds raw (os, ot).
@vertex
fn vs_water(in: VertexIn) -> VertexOut {
    var out: VertexOut;
    let t = frame.params.x;
    var scale = frame.params.y;
    if (frame.eye.z <= in.aux) {
        scale = -scale;
    }
    var p = in.pos;
    let w1 = (turbsin(p.x * 5.0 + t * 171.0 - p.y) + 8.0) * 0.8;
    let w2 = turbsin(t * 160.0 + p.x + p.y) + 8.0;
    p.z = p.z + scale * (w1 + w2);
    let os = in.uv.x;
    let ot = in.uv.y;
    let s = (os + turbsin((ot * 0.125 + t) * 40.74366543152521)) / 64.0;
    let tt = (ot + turbsin((os * 0.125 + t) * 40.74366543152521)) / 64.0;
    let world = (model.transform * vec4<f32>(p, 1.0)).xyz;
    out.clip = frame.view_proj * vec4<f32>(world, 1.0);
    out.uv = vec2<f32>(s, tt);
    out.lm = in.lm;
    out.world = world;
    return out;
}

// GL fixed-function fog on the eye-space depth: linear (end − z)/(end − start) or
// exp(−(density·z)²), mixing towards the fog colour.
fn fog(c: vec3<f32>, world: vec3<f32>) -> vec3<f32> {
    if (frame.fog.x == 0.0) {
        return c;
    }
    let z = abs(dot(world - frame.eye.xyz, frame.forward.xyz));
    var f: f32;
    if (frame.fog.x == 1.0) {
        f = (frame.fog.w - z) / (frame.fog.w - frame.fog.z);
    } else {
        let d = frame.fog.y * z;
        f = exp(-d * d);
    }
    return mix(frame.fog_color.rgb, c, clamp(f, 0.0, 1.0));
}

// Rendermode 0: TMU0 base texture MODULATE TMU1 lightmap.
@fragment
fn fs_lit(in: VertexOut) -> @location(0) vec4<f32> {
    let tex = textureSample(diffuse, diffuse_sampler, in.uv);
    let lm = textureSample(lightmap, lightmap_sampler, in.lm);
    return vec4<f32>(fog(tex.rgb * lm.rgb, in.world), tex.a);
}

// kRenderTransAlpha: lit, glAlphaFunc(GL_GREATER, gl_alphamin = 0.25).
@fragment
fn fs_lit_alpha_test(in: VertexOut) -> @location(0) vec4<f32> {
    let tex = textureSample(diffuse, diffuse_sampler, in.uv);
    let lm = textureSample(lightmap, lightmap_sampler, in.lm);
    if (tex.a <= 0.25) {
        discard;
    }
    return vec4<f32>(fog(tex.rgb * lm.rgb, in.world), tex.a);
}

// kRenderTransColor: untextured rendercolor/256 with alpha r_blend.
@fragment
fn fs_trans_color(in: VertexOut) -> @location(0) vec4<f32> {
    return vec4<f32>(fog(model.color.rgb, in.world), model.blend.x);
}

// kRenderTransTexture / kRenderGlow on brushes: fullbright texture, colour (1,1,1,r_blend).
@fragment
fn fs_trans_texture(in: VertexOut) -> @location(0) vec4<f32> {
    let tex = textureSample(diffuse, diffuse_sampler, in.uv);
    return vec4<f32>(fog(tex.rgb, in.world), tex.a * model.blend.x);
}

// kRenderTransAdd: fullbright texture × r_blend added to the framebuffer.
@fragment
fn fs_additive(in: VertexOut) -> @location(0) vec4<f32> {
    let tex = textureSample(diffuse, diffuse_sampler, in.uv);
    return vec4<f32>(fog(tex.rgb * model.blend.x, in.world), 1.0);
}

// Decals: decal texture MODULATE the surface lightmap, alpha blended, GL_NOTEQUAL 0 test.
@fragment
fn fs_decal(in: VertexOut) -> @location(0) vec4<f32> {
    let tex = textureSample(diffuse, diffuse_sampler, in.uv);
    if (tex.a == 0.0) {
        discard;
    }
    let lm = textureSample(lightmap, lightmap_sampler, in.lm);
    return vec4<f32>(fog(tex.rgb * lm.rgb, in.world), tex.a);
}

// Turbulent surfaces are fullbright.
@fragment
fn fs_water(in: VertexOut) -> @location(0) vec4<f32> {
    let tex = textureSample(diffuse, diffuse_sampler, in.uv);
    return vec4<f32>(fog(tex.rgb, in.world), model.blend.x);
}

// Sky polygons show the skybox in the view direction (faces rt, bk, lf, ft, up, dn), using the
// Quake skybox st_to_vec mapping and GoldSrc's 1/512 edge clamp.
@fragment
fn fs_sky(in: VertexOut) -> @location(0) vec4<f32> {
    let d = in.world - frame.eye.xyz;
    let a = abs(d);
    var layer: i32;
    var st: vec2<f32>;
    if (a.x >= a.y && a.x >= a.z) {
        if (d.x > 0.0) { layer = 0; st = vec2<f32>(-d.y, d.z) / a.x; } else { layer = 2; st = vec2<f32>(d.y, d.z) / a.x; }
    } else if (a.y >= a.z) {
        if (d.y > 0.0) { layer = 1; st = vec2<f32>(d.x, d.z) / a.y; } else { layer = 3; st = vec2<f32>(-d.x, d.z) / a.y; }
    } else {
        if (d.z > 0.0) { layer = 4; st = vec2<f32>(-d.y, -d.x) / a.z; } else { layer = 5; st = vec2<f32>(-d.y, d.x) / a.z; }
    }
    let u = clamp((st.x + 1.0) * 0.5, 1.0 / 512.0, 511.0 / 512.0);
    let v = 1.0 - clamp((st.y + 1.0) * 0.5, 1.0 / 512.0, 511.0 / 512.0);
    return vec4<f32>(textureSample(sky, sky_sampler, vec2<f32>(u, v), layer).rgb, 1.0);
}
