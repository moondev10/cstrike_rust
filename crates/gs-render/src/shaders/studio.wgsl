// Dynamic geometry (studio models §B.11, sprites §C.2): world-space vertices with a CPU-computed
// colour; GL_MODULATE or GL_REPLACE texturing and the alpha test variants of the GL state.

struct Frame {
    view_proj: mat4x4<f32>,
    eye: vec4<f32>,
    params: vec4<f32>,
    forward: vec4<f32>,
    fog_color: vec4<f32>,
    // x: 0 off, 1 GL_LINEAR, 2 GL_EXP2; y: density; z: start; w: end
    fog: vec4<f32>,
};

@group(0) @binding(0) var<uniform> frame: Frame;
@group(1) @binding(0) var diffuse: texture_2d<f32>;
@group(3) @binding(0) var diffuse_sampler: sampler;

struct VertexIn {
    @location(0) pos: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) color: vec4<f32>,
};

struct VertexOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) world: vec3<f32>,
};

@vertex
fn vs_main(v: VertexIn) -> VertexOut {
    var out: VertexOut;
    out.clip = frame.view_proj * vec4<f32>(v.pos, 1.0);
    out.uv = v.uv;
    out.color = v.color;
    out.world = v.pos;
    return out;
}

// GL fixed-function fog on the eye-space depth (see world.wgsl).
fn fog(c: vec4<f32>, world: vec3<f32>) -> vec4<f32> {
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
    return vec4<f32>(mix(frame.fog_color.rgb, c.rgb, clamp(f, 0.0, 1.0)), c.a);
}

@fragment
fn fs_main(v: VertexOut) -> @location(0) vec4<f32> {
    return fog(textureSample(diffuse, diffuse_sampler, v.uv) * v.color, v.world);
}

// GL_GREATER 0.5
@fragment
fn fs_alpha_test(v: VertexOut) -> @location(0) vec4<f32> {
    let c = textureSample(diffuse, diffuse_sampler, v.uv) * v.color;
    if c.a <= 0.5 {
        discard;
    }
    return fog(c, v.world);
}

// GL_NOTEQUAL 0
@fragment
fn fs_alpha_nonzero(v: VertexOut) -> @location(0) vec4<f32> {
    let c = textureSample(diffuse, diffuse_sampler, v.uv) * v.color;
    if c.a == 0.0 {
        discard;
    }
    return fog(c, v.world);
}

// GL_REPLACE with GL_NOTEQUAL 0
@fragment
fn fs_replace_nonzero(v: VertexOut) -> @location(0) vec4<f32> {
    let c = textureSample(diffuse, diffuse_sampler, v.uv);
    if c.a == 0.0 {
        discard;
    }
    return fog(c, v.world);
}
