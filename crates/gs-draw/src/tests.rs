use gs_formats::sprite::{SpriteFormat, SpriteFrame, SpriteFrameSlot, SpriteType};

use super::*;

struct Loader;

impl SpriteLoader for Loader {
    fn load_sprite(&mut self, name: &str) -> Option<(Arc<Sprite>, u32)> {
        if name.contains("missing") {
            return None;
        }
        let frame = |w: u32, h: u32| {
            SpriteFrameSlot::Single(SpriteFrame { width: w, height: h, up: 0.0, down: 0.0, left: 0.0, right: 0.0, pixels: vec![0; (w * h) as usize] })
        };
        let sprite = Sprite {
            kind: SpriteType::ParallelUpright,
            format: SpriteFormat::Additive,
            max_width: 64,
            max_height: 32,
            beam_length: 0.0,
            sync_type: 0,
            palette: vec![[0; 3]; 256],
            frames: vec![frame(64, 32), frame(16, 16)],
        };
        Some((Arc::new(sprite), 7))
    }
    fn load_image(&mut self, _name: &str) -> Option<Image> {
        None
    }
}

fn screen() -> Screen {
    let mut s = Screen::new(Scheme::default(), Localize::default());
    s.width = 800;
    s.height = 600;
    s
}

#[test]
fn sprite_handles_are_one_based_and_shared_by_name() {
    let mut s = screen();
    let a = s.spr_load("sprites/640hud1.spr", &mut Loader);
    let b = s.spr_load("SPRITES/640HUD1.SPR", &mut Loader);
    assert_eq!(a, SprHandle(1));
    assert_eq!(a, b);
    assert_eq!(s.spr_load("sprites/missing.spr", &mut Loader), SprHandle(0));
    assert_eq!(s.spr_frames(a), 2);
    assert_eq!(s.spr_size(a, 1), (16, 16));
    assert_eq!(s.spr_size(a, 9), (64, 32), "out-of-range frames read frame 0");
}

#[test]
fn sprite_rects_crop_with_half_texel_insets() {
    let mut s = screen();
    let h = s.spr_load("sprites/640hud1.spr", &mut Loader);
    s.spr_set(h, 255, 160, 0);
    s.spr_draw_additive(0, 10, 20, Some(Rect { left: 8, right: 24, top: 0, bottom: 16 }));
    s.spr_draw_additive(0, 10, 20, Some(Rect { left: 30, right: 10, top: 0, bottom: 16 }));
    s.spr_draw_additive(0, 900, 20, None);
    assert_eq!(s.list.len(), 2, "draws starting off-screen are dropped");
    let crop = s.list[0];
    assert_eq!(crop.rect, [10.5, 20.5, 26.5, 36.5]);
    assert_eq!(crop.uv, [8.5 / 64.0, 0.5 / 32.0, 23.5 / 64.0, 15.5 / 32.0]);
    assert_eq!(crop.color, [1.0, 160.0 / 255.0, 0.0, 1.0]);
    assert_eq!(crop.texture, Texture::Sprite { texture: 7, frame: 0 });
    assert_eq!(crop.blend, Blend::Additive);
    assert_eq!(s.list[1].uv, [0.0, 0.0, 1.0, 1.0], "an inverted rect draws the whole frame");
}

#[test]
fn a_bad_handle_keeps_the_current_sprite_and_colour() {
    let mut s = screen();
    let h = s.spr_load("sprites/640hud1.spr", &mut Loader);
    s.spr_set(h, 10, 20, 30);
    s.spr_set(SprHandle(42), 255, 255, 255);
    s.spr_draw_holes(1, 0, 0, None);
    assert_eq!(s.list[0].color, [10.0 / 255.0, 20.0 / 255.0, 30.0 / 255.0, 1.0]);
    assert_eq!(s.list[0].rect, [0.5, 0.5, 16.5, 16.5]);
}

#[test]
fn scissor_clamps_to_the_screen_and_skips_fills() {
    let mut s = screen();
    let h = s.spr_load("sprites/640hud1.spr", &mut Loader);
    s.spr_set(h, 255, 255, 255);
    s.enable_scissor(-10, 590, 2000, 50);
    s.spr_draw(0, 0, 0, None);
    s.fill_rgba(0, 0, 4, 4, 255, 0, 0, 128);
    s.disable_scissor();
    s.spr_draw(0, 0, 0, None);
    assert_eq!(s.list[0].scissor, Some([0, 590, 800, 10]));
    assert_eq!(s.list[1].scissor, None);
    assert_eq!(s.list[1].blend, Blend::AlphaAdditive);
    assert_eq!(s.list[2].scissor, None);
}
