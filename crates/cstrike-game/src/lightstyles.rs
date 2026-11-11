//! Light style patterns for local map viewing (on a server they arrive via `svc_lightstyle`).

use gs_render::camera::NUM_LIGHTSTYLES;
use gs_render::lightmap::animate_styles;

/// Styles the CS game DLL sets in `CWorld::Precache` (cs.so).
pub const CS_DEFAULT_STYLES: [(usize, &str); 14] = [
    (0, "m"),
    (1, "mmnmmommommnonmmonqnmmo"),
    (2, "abcdefghijklmnopqrstuvwxyzyxwvutsrqponmlkjihgfedcba"),
    (3, "mmmmmaaaaammmmmaaaaaabcdefgabcdefg"),
    (4, "mamamamamama"),
    (5, "jklmnopqrstuvwxyzyxwvutsrqponmlkj"),
    (6, "nmonqnmomnmomomno"),
    (7, "mmmaaaabcdefgmmmmaaaammmaamm"),
    (8, "mmmaaammmaaammmabcdefaaaammmmabcdefmmmaaaa"),
    (9, "aaaaaaaazzzzzzzz"),
    (10, "mmamammmmammamamaaamammma"),
    (11, "abcdefghijklmnopqrrqponmlkjihgfedcba"),
    (12, "mmnnmmnnnmmnn"),
    (63, "a"),
];

#[derive(Debug, Clone)]
pub struct LightStyles {
    patterns: Vec<String>,
}

impl Default for LightStyles {
    fn default() -> Self {
        let mut patterns = vec![String::new(); NUM_LIGHTSTYLES];
        for (i, p) in CS_DEFAULT_STYLES {
            patterns[i] = p.to_owned();
        }
        Self { patterns }
    }
}

impl LightStyles {
    /// `d_lightstylevalue` for every style at `time`.
    pub fn values(&self, time: f64) -> [i32; NUM_LIGHTSTYLES] {
        animate_styles(&self.patterns, time)
    }
}
