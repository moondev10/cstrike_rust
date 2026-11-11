//! GoldSrc view conventions: X forward, Y left, Z up; angles are (pitch down, yaw left, roll) in degrees.

use glam::camera::rh::{proj::directx, view};
use glam::{Mat4, Vec3};

pub const NUM_LIGHTSTYLES: usize = 64;
/// `d_lightstylevalue` of a style with the default pattern `m`.
pub const NORMAL_STYLE: i32 = 264;
pub const Z_NEAR: f32 = 4.0;
/// `sv_zmax` default.
pub const DEFAULT_ZMAX: f32 = 4096.0;

/// Everything that varies per rendered frame.
#[derive(Debug, Clone)]
pub struct View {
    pub origin: Vec3,
    pub angles: Vec3,
    /// `default_fov`/`fov`: horizontal degrees, widened for aspect ratios above 1.5.
    pub fov: f32,
    /// Client time in seconds (`cl.time`), drives water warp and texture animation.
    pub time: f64,
    /// `d_lightstylevalue` per style (`'m'` = 264).
    pub light_styles: [i32; NUM_LIGHTSTYLES],
    /// Far plane: `movevars.zmax`.
    pub zfar: f32,
    /// `movevars.waveHeight`.
    pub wave_height: f32,
    pub fog: Fog,
}

/// The frame's fog (docs/re/render.md §A.7.9).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Fog {
    /// The client's `tri_Fog` + `tri_FogParams` (GL_EXP2, sky excluded) while on: colour 0..1
    /// and density.
    pub user: Option<([f32; 3], f32)>,
    /// `cl.waterlevel > 2`: the engine's linear underwater fog over the opaque scene.
    pub underwater: bool,
}

impl View {
    pub fn new(origin: Vec3, angles: Vec3) -> Self {
        Self {
            origin,
            angles,
            fov: 90.0,
            time: 0.0,
            light_styles: [NORMAL_STYLE; NUM_LIGHTSTYLES],
            zfar: DEFAULT_ZMAX,
            wave_height: 0.0,
            fog: Fog::default(),
        }
    }
}

/// `AngleVectors`: forward, right, up.
pub fn angle_vectors(angles: Vec3) -> (Vec3, Vec3, Vec3) {
    let (sp, cp) = angles.x.to_radians().sin_cos();
    let (sy, cy) = angles.y.to_radians().sin_cos();
    let (sr, cr) = angles.z.to_radians().sin_cos();
    let forward = Vec3::new(cp * cy, cp * sy, -sp);
    let right = Vec3::new(-sr * sp * cy + cr * sy, -sr * sp * sy - cr * cy, -sr * cp);
    let up = Vec3::new(cr * sp * cy + sr * sy, cr * sp * sy - sr * cy, cr * cp);
    (forward, right, up)
}

/// `R_SetFrustum` field of view: `(fov_x, fov_y)` in degrees for a viewport.
pub fn field_of_view(fov: f32, width: u32, height: u32) -> (f32, f32) {
    let (w, h) = (width.max(1) as f32, height.max(1) as f32);
    let mut fov_x = fov;
    if w / h > 1.5 {
        fov_x = 2.0 * ((fov_x.to_radians() * 0.5).tan() * (w / h * 0.75)).atan().to_degrees();
    }
    if !(1.0..=179.0).contains(&fov_x) {
        fov_x = 90.0;
    }
    let fov_y = (h / (w / (fov_x / 360.0 * std::f32::consts::PI).tan())).atan() * 360.0 / std::f32::consts::PI;
    (fov_x, fov_y)
}

pub fn view_projection(view: &View, width: u32, height: u32) -> Mat4 {
    let (forward, _, up) = angle_vectors(view.angles);
    let (_, fov_y) = field_of_view(view.fov, width, height);
    let aspect = width.max(1) as f32 / height.max(1) as f32;
    let proj = directx::perspective(fov_y.to_radians(), aspect, Z_NEAR, view.zfar.max(Z_NEAR + 1.0));
    proj * view::look_to_mat4(view.origin, forward, up)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn yaw_90_looks_along_positive_y_and_right_is_positive_x() {
        let (f, r, u) = angle_vectors(Vec3::new(0.0, 90.0, 0.0));
        assert!((f - Vec3::Y).length() < 1e-6, "{f}");
        assert!((r - Vec3::X).length() < 1e-6, "{r}");
        assert!((u - Vec3::Z).length() < 1e-6, "{u}");
    }

    #[test]
    fn widescreen_widens_horizontal_fov_and_keeps_4_3_vertical() {
        let (x43, y43) = field_of_view(90.0, 1024, 768);
        assert!((x43 - 90.0).abs() < 1e-4);
        let (x169, y169) = field_of_view(90.0, 1920, 1080);
        assert!(x169 > 106.0 && x169 < 106.4, "{x169}");
        assert!((y169 - y43).abs() < 1e-3, "vertical fov is preserved: {y169} vs {y43}");
        let (x54, _) = field_of_view(90.0, 1280, 1024);
        assert!((x54 - 90.0).abs() < 1e-4, "aspect <= 1.5 keeps the horizontal fov");
    }

    #[test]
    fn point_ahead_projects_to_screen_center() {
        let view = View::new(Vec3::new(10.0, 20.0, 30.0), Vec3::new(0.0, 45.0, 0.0));
        let (f, _, _) = angle_vectors(view.angles);
        let clip = view_projection(&view, 1600, 900) * (view.origin + f * 100.0).extend(1.0);
        let ndc = clip.truncate() / clip.w;
        assert!(ndc.x.abs() < 1e-5 && ndc.y.abs() < 1e-5 && (0.0..=1.0).contains(&ndc.z), "{ndc}");
    }
}
