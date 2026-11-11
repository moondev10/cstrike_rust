//! Vector helpers beyond `gs_pmove::math`.

pub use gs_pmove::math::{Vec3, add, cross, dot, length, ma, normalize, scale, sub};

/// `VectorAngles`: pitch (positive up) and yaw in degrees, both in `[0, 360)`.
pub fn vector_angles(forward: Vec3) -> Vec3 {
    if forward[0] == 0.0 && forward[1] == 0.0 {
        return [if forward[2] > 0.0 { 90.0 } else { 270.0 }, 0.0, 0.0];
    }
    let mut yaw = (f64::from(forward[1]).atan2(f64::from(forward[0])) * 180.0 / std::f64::consts::PI) as f32;
    if yaw < 0.0 {
        yaw += 360.0;
    }
    let tmp = (forward[0] * forward[0] + forward[1] * forward[1]).sqrt();
    let mut pitch = (f64::from(forward[2]).atan2(f64::from(tmp)) * 180.0 / std::f64::consts::PI) as f32;
    if pitch < 0.0 {
        pitch += 360.0;
    }
    [pitch, yaw, 0.0]
}

/// `VectorVectors(forward)` → (right, up).
pub fn vector_vectors(forward: Vec3) -> (Vec3, Vec3) {
    let mut right = [forward[2], -forward[0], forward[1]];
    let d = dot(forward, right);
    right = ma(right, -d, forward);
    normalize(&mut right);
    (right, cross(right, forward))
}

/// `VectorNormalize` returning the normalized copy and the length.
pub fn normalized(v: Vec3) -> (Vec3, f32) {
    let mut n = v;
    let len = normalize(&mut n);
    (n, len)
}
