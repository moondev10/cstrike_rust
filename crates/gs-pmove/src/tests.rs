use gs_protocol::messages::MoveVars;
use gs_protocol::structs::UserCmd;
use gs_world::{ClipNode, Plane, World};

use crate::{PlayerMove, PmContext, StuckState, TextureTypes, buttons, flags, movetype, pm_move};
use crate::trace::Physent;

/// A world whose floor is the plane z = 0 (clip hulls pre-expanded like a compiled map).
fn flat_world() -> World {
    let floor = |dist: f32| Plane { normal: [0.0, 0.0, 1.0], dist, kind: 2 };
    let node = |plane: i32| ClipNode { plane, children: [-1, -2] };
    // Plane 0: point hull, 1: stand (36), 2: large (32), 3: duck (18). A trailing unused node keeps
    // `first < last` for every hull (an empty hull is treated as all-empty by the engine).
    let planes = vec![floor(0.0), floor(36.0), floor(32.0), floor(18.0)];
    let hull0 = vec![node(0), node(0)];
    let clipnodes = vec![node(1), node(2), node(3), node(3)];
    World::from_clip_data(planes, hull0, clipnodes, vec![([0, 0, 1, 2], [-4096.0; 3], [4096.0; 3])])
}

fn movevars() -> MoveVars {
    MoveVars {
        gravity: 800.0,
        stopspeed: 75.0,
        maxspeed: 320.0,
        spectatormaxspeed: 500.0,
        accelerate: 5.0,
        airaccelerate: 10.0,
        wateraccelerate: 10.0,
        friction: 4.0,
        edgefriction: 2.0,
        waterfriction: 1.0,
        entgravity: 1.0,
        bounce: 1.0,
        stepsize: 18.0,
        maxvelocity: 2000.0,
        footsteps: true,
        ..MoveVars::default()
    }
}

fn standing_player() -> PlayerMove {
    PlayerMove {
        multiplayer: true,
        origin: [0.0, 0.0, 36.0],
        view_ofs: [0.0, 0.0, 17.0],
        movetype: movetype::WALK,
        friction: 1.0,
        gravity: 1.0,
        maxspeed: 320.0,
        clientmaxspeed: 250.0,
        flags: flags::ONGROUND,
        onground: 0,
        physents: vec![Physent { model: Some(0), solid: 4, ..Physent::default() }],
        movevars: movevars(),
        runfuncs: true,
        ..PlayerMove::default()
    }
}

fn run(p: &mut PlayerMove, world: &World, cmd: UserCmd) {
    let textures = TextureTypes::default();
    let mut stuck = StuckState::default();
    let mut rng = |lo: i32, _hi: i32| lo;
    let mut ctx = PmContext { world, textures: &textures, stuck: &mut stuck, random: &mut rng, float_time: 0.0, sounds: Vec::new() };
    p.cmd = cmd;
    p.frametime = (f64::from(cmd.msec) / 1000.0) as f32;
    pm_move(p, &mut ctx, false);
}

fn cmd(msec: u8, forward: f32, buttons: i32) -> UserCmd {
    UserCmd { msec, forwardmove: forward, buttons: buttons as u16, ..UserCmd::default() }
}

#[test]
fn standing_still_stays_on_the_ground() {
    let world = flat_world();
    let mut p = standing_player();
    for _ in 0..10 {
        run(&mut p, &world, cmd(10, 0.0, 0));
    }
    assert_eq!(p.origin, [0.0, 0.0, 36.0]);
    assert_eq!(p.velocity, [0.0; 3]);
    assert_ne!(p.flags & flags::ONGROUND, 0);
}

#[test]
fn first_walk_frame_accelerates_by_accel_times_wishspeed() {
    let world = flat_world();
    let mut p = standing_player();
    run(&mut p, &world, cmd(10, 400.0, buttons::FORWARD));
    // forwardmove 400 is clipped to clientmaxspeed 250; accel = ((5 · 0.01f) · 250) · 1 in f32
    let accel = (5.0f32 * 0.01f32) * 250.0;
    assert_eq!(p.velocity, [accel, 0.0, 0.0]);
    assert_eq!(p.origin, [accel * 0.01f32 + 0.0, 0.0, 36.0]);
}

#[test]
fn walking_reaches_the_maxspeed_terminal_velocity() {
    let world = flat_world();
    let mut p = standing_player();
    for _ in 0..300 {
        run(&mut p, &world, cmd(10, 400.0, buttons::FORWARD));
    }
    // Friction removes 4% per 10 ms frame, acceleration tops the speed back up to the wish speed.
    assert!(p.velocity[0] > 240.0 && p.velocity[0] <= 250.0, "{:?}", p.velocity);
    assert_eq!(p.origin[2], 36.0);
}

#[test]
fn jump_sets_cs_jump_speed_and_stamina() {
    let world = flat_world();
    let mut p = standing_player();
    run(&mut p, &world, cmd(10, 0.0, buttons::JUMP));
    assert_eq!(p.fuser[1], 1315.7894, "stamina after a jump");
    let jump = (72000.0f64).sqrt() as f32;
    // jump speed, then FixupGravityVelocity twice (in PM_Jump and after the air move)
    let half_g = |v: f32| (f64::from(v) - f64::from(800.0f32 * 0.01) * 0.5) as f32;
    assert_eq!(p.velocity[2], half_g(half_g(jump)));
    assert_eq!(p.flags & flags::ONGROUND, 0);
    // Holding jump does not jump again after landing (no auto-bunnyhop). Landing traces stop
    // DIST_EPSILON above the floor, as on real servers (origins end in .03125).
    for _ in 0..200 {
        run(&mut p, &world, cmd(10, 0.0, buttons::JUMP));
    }
    assert_eq!(p.origin[2], 36.03125);
    assert_eq!(p.velocity[2], 0.0);
}

#[test]
fn ducking_takes_400ms_and_lowers_the_origin_by_18() {
    let world = flat_world();
    let mut p = standing_player();
    let mut frames = 0;
    while p.flags & flags::DUCKING == 0 && frames < 100 {
        run(&mut p, &world, cmd(10, 0.0, buttons::DUCK));
        frames += 1;
    }
    assert_eq!(frames, 41, "duck completes once flDuckTime ≤ 600 ms");
    assert_eq!(p.usehull, 1);
    assert_eq!(p.view_ofs[2], 12.0);
    assert_eq!(p.origin[2], 18.0);
    // release: unduck immediately on open ground
    run(&mut p, &world, cmd(10, 0.0, 0));
    assert_eq!(p.usehull, 0);
    assert_eq!(p.origin[2], 36.0);
    assert_eq!(p.view_ofs[2], 17.0);
}

#[test]
fn clip_velocity_kills_tiny_components() {
    let (v, blocked) = crate::clip_velocity([10.0, 0.05, -5.0], [0.0, 0.0, 1.0], 1.0);
    assert_eq!(v, [10.0, 0.0, 0.0]);
    assert_eq!(blocked, 1);
}

/// [`flat_world`] with a step `height` units up from x = 64 on. The standing hull's floor
/// rises to z = height + 36 from x = 48 on: the box is expanded by 16 sideways and 36 down.
fn step_world(height: f32) -> World {
    let z = |dist: f32| Plane { normal: [0.0, 0.0, 1.0], dist, kind: 2 };
    let x = |dist: f32| Plane { normal: [1.0, 0.0, 0.0], dist, kind: 0 };
    let node = |plane: i32, front: i16, back: i16| ClipNode { plane, children: [front, back] };
    let (empty, solid): (i16, i16) = (-1, -2);
    // Planes 0..3 are flat_world's floors; 4/5 bound the point hull's step, 6/7 the standing hull's.
    let planes = vec![z(0.0), z(36.0), z(32.0), z(18.0), x(64.0), z(height), x(48.0), z(height + 36.0)];
    let hull0 = vec![node(0, 1, solid), node(4, 2, empty), node(5, empty, solid)];
    // Standing hull from node 0 (with the step), large and duck hulls from nodes 1 and 2 (flat).
    let clipnodes =
        vec![node(1, 4, solid), node(2, empty, solid), node(3, empty, solid), node(3, empty, solid), node(6, 5, empty), node(7, empty, solid)];
    World::from_clip_data(planes, hull0, clipnodes, vec![([0, 0, 1, 2], [-4096.0; 3], [4096.0; 3])])
}

#[test]
fn walking_climbs_a_step_up_to_stepsize_and_stops_at_a_higher_one() {
    let low = step_world(16.0);
    let mut p = standing_player();
    for _ in 0..100 {
        run(&mut p, &low, cmd(10, 400.0, buttons::FORWARD));
    }
    assert!(p.origin[0] > 64.0, "{:?}", p.origin);
    assert!((p.origin[2] - 52.0).abs() < 0.05, "standing on the 16-unit step: {:?}", p.origin);
    assert_ne!(p.flags & flags::ONGROUND, 0);

    let high = step_world(20.0);
    let mut p = standing_player();
    for _ in 0..100 {
        run(&mut p, &high, cmd(10, 400.0, buttons::FORWARD));
    }
    assert!(p.origin[0] < 48.0, "stopped by the 20-unit step (stepsize 18): {:?}", p.origin);
    assert_eq!(p.origin[2], 36.0);
}

#[test]
fn air_strafing_adds_at_most_30_units_per_second_along_the_wish_direction() {
    let world = flat_world();
    let mut p = standing_player();
    p.origin = [0.0, 0.0, 1000.0];
    p.flags = 0;
    p.onground = -1;
    p.velocity = [300.0, 0.0, 0.0];
    let strafe = UserCmd { msec: 10, sidemove: 250.0, ..UserCmd::default() };
    run(&mut p, &world, strafe);
    // airaccelerate · wishspeed · frametime = 10 · 250 · 0.01 = 25, below the cap.
    assert!((p.velocity[1].abs() - 25.0).abs() < 1e-3, "{:?}", p.velocity);
    for _ in 0..20 {
        run(&mut p, &world, strafe);
    }
    assert!((p.velocity[1].abs() - 30.0).abs() < 1e-3, "capped at 30 along the wish direction: {:?}", p.velocity);
    assert_eq!(p.velocity[0], 300.0, "no friction in the air");
    assert_eq!(p.flags & flags::ONGROUND, 0);
}

/// A one-bone studio model with a 40×4×4 hitbox along the bone's x axis (group 3), a second
/// small one far off (group 7), and no animation data.
fn stick_model(flags: i32) -> std::sync::Arc<gs_studio::StudioModel> {
    use gs_studio::model::{Bone, HitBox, SeqDesc};
    let bone = Bone { name: "root".into(), parent: -1, controller: [-1; 6], value: [0.0; 6], scale: [0.0; 6] };
    let seq = SeqDesc {
        label: "idle".into(),
        fps: 30.0,
        flags: 0,
        activity: 0,
        events: Vec::new(),
        numframes: 1,
        motiontype: 0,
        motionbone: 0,
        linearmovement: [0.0; 3],
        bbmin: [0.0; 3],
        bbmax: [0.0; 3],
        numblends: 1,
        animindex: 0,
        seqgroup: 0,
    };
    std::sync::Arc::new(gs_studio::StudioModel {
        name: "stick.mdl".into(),
        eyeposition: [0.0; 3],
        min: [0.0; 3],
        max: [0.0; 3],
        bbmin: [0.0; 3],
        bbmax: [0.0; 3],
        flags,
        bones: vec![bone],
        controllers: Vec::new(),
        hitboxes: vec![
            HitBox { bone: 0, group: 3, mins: [-20.0, -2.0, -2.0], maxs: [20.0, 2.0, 2.0] },
            HitBox { bone: 0, group: 7, mins: [200.0, 200.0, -1.0], maxs: [201.0, 201.0, 1.0] },
        ],
        sequences: vec![seq],
        seqgroups: Vec::new(),
        bodyparts: Vec::new(),
        attachments: Vec::new(),
        textures: Vec::new(),
        skins: Vec::new(),
        data: std::sync::Arc::from(vec![0u8; 12]),
        group_data: Vec::new(),
    })
}

fn stick_physent(flags: i32, yaw: f32) -> Physent {
    use crate::studio_hull::StudioPhys;
    let model = stick_model(flags);
    Physent {
        origin: [0.0, 0.0, 100.0],
        studiomodel: Some(StudioPhys { model, frame: 0.0, sequence: 0, controller: [0; 4], blending: [0; 2] }),
        mins: [-30.0, -30.0, -30.0],
        maxs: [30.0, 30.0, 30.0],
        info: 5,
        angles: [0.0, yaw, 0.0],
        solid: 3,
        ..Physent::default()
    }
}

#[test]
fn point_traces_hit_the_rotated_hitbox_and_studio_box_traces_the_bbox() {
    use crate::trace::{player_trace, trace_flags};
    let world = flat_world();
    let world_pe = Physent { model: Some(0), solid: 4, ..Physent::default() };
    // Rotated 90 degrees the stick lies along y: a trace down x = 10 misses it, one down y = 10
    // stops on its top face (z = 102, backed off by DIST_EPSILON) with the hitbox's group.
    let near = |z: f32, face: f32| (z - (face + 0.03125)).abs() < 1e-3;
    let physents = [world_pe.clone(), stick_physent(0, 90.0)];
    let miss = player_trace(&world, &physents, 2, [10.0, 0.0, 200.0], [10.0, 0.0, 50.0], trace_flags::NORMAL, -1);
    assert_eq!(miss.ent, -1);
    let hit = player_trace(&world, &physents, 2, [0.0, 10.0, 200.0], [0.0, 10.0, 50.0], trace_flags::NORMAL, -1);
    assert_eq!((hit.ent, hit.hitgroup), (1, 3));
    assert!(near(hit.endpos[2], 102.0), "{:?}", hit.endpos);
    // PM_STUDIO_BOX keeps the bounding box for the point hull.
    let boxed = player_trace(&world, &physents, 2, [10.0, 0.0, 200.0], [10.0, 0.0, 50.0], trace_flags::STUDIO_BOX, -1);
    assert_eq!(boxed.ent, 1);
    assert!(near(boxed.endpos[2], 130.0), "{:?}", boxed.endpos);
    // The standing hull uses hitboxes only for STUDIO_TRACE_HITBOX models, grown by half the hull.
    let stand = player_trace(&world, &physents, 0, [0.0, 10.0, 300.0], [0.0, 10.0, 50.0], trace_flags::NORMAL, -1);
    assert!(near(stand.endpos[2], 166.0), "the box, 130 + 36: {:?}", stand.endpos);
    let flagged = [world_pe, stick_physent(crate::studio_hull::STUDIO_TRACE_HITBOX, 90.0)];
    let hb = player_trace(&world, &flagged, 0, [0.0, 10.0, 300.0], [0.0, 10.0, 50.0], trace_flags::NORMAL, -1);
    assert!(near(hb.endpos[2], 138.0), "the hitbox, 102 + 36: {:?}", hb.endpos);
}
