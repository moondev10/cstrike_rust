//! Vector helpers with the exact f32 association of the original (docs/re/movement.md §A.6,
//! §B.11). No fused multiply-add anywhere.

pub type Vec3 = [f32; 3];

pub const ORIGIN: Vec3 = [0.0; 3];
/// `M_PI*2/360` as the double the binaries use.
const DEG2RAD: f64 = 0.017_453_292_519_943_295;

/// `(a0*b0 + a1*b1) + a2*b2`.
#[inline]
pub fn dot(a: Vec3, b: Vec3) -> f32 {
    (a[0] * b[0] + a[1] * b[1]) + a[2] * b[2]
}

/// `Length`: `sqrt(((0 + x*x) + y*y) + z*z)` (f64 sqrt rounded equals f32 sqrt).
#[inline]
pub fn length(v: Vec3) -> f32 {
    (((0.0 + v[0] * v[0]) + v[1] * v[1]) + v[2] * v[2]).sqrt()
}

/// `VectorNormalize`: returns the length; a zero vector is left unchanged.
#[inline]
pub fn normalize(v: &mut Vec3) -> f32 {
    let len = ((v[0] * v[0] + v[1] * v[1]) + v[2] * v[2]).sqrt();
    if len != 0.0 {
        let il = 1.0 / len;
        for c in v.iter_mut() {
            *c *= il;
        }
    }
    len
}

/// `VectorMA`: `a + s*b`.
#[inline]
pub fn ma(a: Vec3, s: f32, b: Vec3) -> Vec3 {
    [a[0] + s * b[0], a[1] + s * b[1], a[2] + s * b[2]]
}

#[inline]
pub fn scale(v: Vec3, s: f32) -> Vec3 {
    [v[0] * s, v[1] * s, v[2] * s]
}

#[inline]
pub fn add(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

#[inline]
pub fn sub(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

/// `CrossProduct`.
#[inline]
pub fn cross(a: Vec3, b: Vec3) -> Vec3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

fn sin_cos(angle: f32) -> (f32, f32) {
    let a = (f64::from(angle) * DEG2RAD) as f32;
    (f64::from(a).sin() as f32, f64::from(a).cos() as f32)
}

/// `AngleVectors(angles)` → (forward, right, up).
pub fn angle_vectors(angles: Vec3) -> (Vec3, Vec3, Vec3) {
    let (sy, cy) = sin_cos(angles[1]);
    let (sp, cp) = sin_cos(angles[0]);
    let (sr, cr) = sin_cos(angles[2]);
    let forward = [cp * cy, cp * sy, -sp];
    let right = [cr * sy - (sr * sp) * cy, (-cr) * cy - (sr * sp) * sy, -(sr * cp)];
    let up = [(cr * sp) * cy + sr * sy, (cr * sp) * sy - sr * cy, cr * cp];
    (forward, right, up)
}

/// `AngleVectorsTranspose(angles)` → (forward, right, up) of the transposed rotation.
pub fn angle_vectors_transpose(angles: Vec3) -> (Vec3, Vec3, Vec3) {
    let (sy, cy) = sin_cos(angles[1]);
    let (sp, cp) = sin_cos(angles[0]);
    let (sr, cr) = sin_cos(angles[2]);
    let (spr, cpr) = (sr * sp, cr * sp);
    let forward = [cp * cy, spr * cy - cr * sy, cpr * cy - (-sy) * sr];
    let right = [cp * sy, spr * sy + cr * cy, cpr * sy - sr * cy];
    let up = [-sp, sr * cp, cr * cp];
    (forward, right, up)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn angle_vectors_basis_for_yaw_90() {
        let (f, r, u) = angle_vectors([0.0, 90.0, 0.0]);
        assert!(f[0].abs() < 1e-7 && (f[1] - 1.0).abs() < 1e-7 && f[2] == 0.0);
        assert!((r[0] - 1.0).abs() < 1e-7 && r[1].abs() < 1e-7, "right of +y is +x: {r:?}");
        assert_eq!(u[2], 1.0);
    }

    #[test]
    fn transpose_is_the_inverse_rotation() {
        let a = [10.0, 35.0, -20.0];
        let (f, r, u) = angle_vectors(a);
        let (tf, tr, tu) = angle_vectors_transpose(a);
        // rotation rows (f, -r, u) times transpose columns gives identity
        let v = [3.0, -2.0, 5.0];
        let local = [dot(v, f), -dot(v, r), dot(v, u)];
        let back = [dot(tf, local), dot(tr, local), dot(tu, local)];
        for i in 0..3 {
            assert!((back[i] - v[i]).abs() < 1e-4, "{back:?}");
        }
    }
}
