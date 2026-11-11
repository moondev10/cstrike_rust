//! Typed server→client message payloads (docs/re/net-messages.md §3, §5, §9).

use crate::structs::EventArgs;

#[derive(Debug, Clone, PartialEq)]
pub struct ServerInfo {
    pub protocol: i32,
    pub spawncount: i32,
    /// Map CRC after `COM_UnMunge3`.
    pub map_crc: i32,
    pub client_dll_md5: [u8; 16],
    pub maxclients: u8,
    /// 0-based; the local player's entity is `playernum + 1`.
    pub playernum: u8,
    pub deathmatch: u8,
    pub gamedir: String,
    pub hostname: String,
    /// e.g. `maps/de_dust2.bsp`.
    pub map_model: String,
    pub mapcycle: String,
    pub extra: u8,
}

/// `movevars_t` in wire order (docs/re/net-messages.md §3.4).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MoveVars {
    pub gravity: f32,
    pub stopspeed: f32,
    pub maxspeed: f32,
    pub spectatormaxspeed: f32,
    pub accelerate: f32,
    pub airaccelerate: f32,
    pub wateraccelerate: f32,
    pub friction: f32,
    pub edgefriction: f32,
    pub waterfriction: f32,
    pub entgravity: f32,
    pub bounce: f32,
    pub stepsize: f32,
    pub maxvelocity: f32,
    pub zmax: f32,
    pub wave_height: f32,
    pub footsteps: bool,
    pub rollangle: f32,
    pub rollspeed: f32,
    pub skycolor: [f32; 3],
    pub skyvec: [f32; 3],
    pub skyname: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SoundName {
    /// Index into the sound precache list.
    Precache(u16),
    /// `!<sentence name>` for `cl.sentence[index]`.
    Sentence(u16),
    /// `!#<n>`: numbered sentence `n`.
    SentenceNumber(u16),
}

pub mod sound_flags {
    pub const VOLUME: u32 = 1;
    pub const ATTENUATION: u32 = 2;
    pub const LARGE_INDEX: u32 = 4;
    pub const PITCH: u32 = 8;
    pub const SENTENCE: u32 = 16;
    pub const STOP: u32 = 32;
    pub const CHANGE_VOL: u32 = 64;
    pub const CHANGE_PITCH: u32 = 128;
    pub const SPAWNING: u32 = 256;
}

#[derive(Debug, Clone, PartialEq)]
pub struct Sound {
    pub flags: u32,
    /// 0..1.
    pub volume: f32,
    pub attenuation: f32,
    pub channel: u8,
    pub entity: u16,
    pub sound: SoundName,
    pub origin: [f32; 3],
    pub pitch: u8,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StaticSound {
    pub origin: [f32; 3],
    pub sound: SoundName,
    pub volume: f32,
    pub attenuation: f32,
    pub entity: i16,
    pub pitch: u8,
    pub flags: u8,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Resource {
    /// 0 sound, 1 skin, 2 model, 3 decal, 4 generic, 5 eventscript, 6 world.
    pub kind: u8,
    pub name: String,
    pub index: u16,
    pub download_size: u32,
    pub flags: u8,
    pub md5: Option<[u8; 16]>,
    pub reserved: Option<[u8; 32]>,
}

pub mod resource_flags {
    pub const FATAL_IF_MISSING: u8 = 1;
    pub const WAS_MISSING: u8 = 2;
    pub const CUSTOM: u8 = 4;
    pub const CHECKFILE: u8 = 0x80;
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ResourceList {
    pub resources: Vec<Resource>,
    /// Indices into `resources` the server wants consistency answers for.
    pub consistency: Vec<u16>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Customization {
    pub player: u8,
    pub resource: Resource,
}

/// One `svc_event` entry as sent. Argument/delay reuse across entries and the packet-entity
/// fill-in belong to the client (`CL_ParseEvent`, docs/re/client-sim.md Part E §2.5).
#[derive(Debug, Clone, PartialEq)]
pub struct WireEvent {
    pub index: u16,
    /// Entity position in the current packet; `None` drops the event.
    pub packet_index: Option<u16>,
    /// Delta from zero, when sent.
    pub args: Option<EventArgs>,
    pub delay: Option<f32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ping {
    pub slot: u8,
    pub ping: u16,
    pub loss: u8,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Hltv {
    Active,
    Status { clients: i32, proxies: i16, slots: u16, total_viewers: i32, total_slots: i32, total_relays: u16 },
    Listen(String),
    Unknown(u8),
}

/// A static (`MAKE_STATIC`) entity: the payload the original client fails to read.
#[derive(Debug, Clone, PartialEq)]
pub struct StaticEntity {
    pub modelindex: i16,
    pub sequence: u8,
    pub frame: u8,
    pub colormap: i16,
    pub skin: u8,
    pub origin: [f32; 3],
    pub angles: [f32; 3],
    pub rendermode: u8,
    pub renderamt: u8,
    pub rendercolor: [u8; 3],
    pub renderfx: u8,
}

/// Shared beam parameters of `TE_BEAM*` messages.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BeamParams {
    pub model: i16,
    pub start_frame: u8,
    /// Frames per second (wire ×0.1).
    pub frame_rate: f32,
    /// Seconds (wire ×0.1).
    pub life: f32,
    pub width: f32,
    pub noise: f32,
    pub color: [u8; 3],
    pub brightness: f32,
    pub scroll_speed: f32,
}

/// `svc_temp_entity` payloads (docs/re/net-messages.md §9). Scales are already applied.
#[derive(Debug, Clone, PartialEq)]
pub enum TempEntity {
    BeamPoints { start: [f32; 3], end: [f32; 3], beam: BeamParams },
    BeamEntPoint { start_ent: i16, end: [f32; 3], beam: BeamParams },
    Gunshot { origin: [f32; 3] },
    Explosion { origin: [f32; 3], sprite: i16, scale: f32, frame_rate: u8, flags: u8 },
    TarExplosion { origin: [f32; 3] },
    Smoke { origin: [f32; 3], sprite: i16, scale: f32, frame_rate: u8 },
    Tracer { start: [f32; 3], end: [f32; 3] },
    Lightning { start: [f32; 3], end: [f32; 3], life: f32, width: f32, amplitude: f32, model: i16 },
    BeamEnts { start_ent: i16, end_ent: i16, beam: BeamParams },
    Sparks { origin: [f32; 3] },
    LavaSplash { origin: [f32; 3] },
    Teleport { origin: [f32; 3] },
    Explosion2 { origin: [f32; 3], color_start: u8, color_count: u8 },
    BspDecal { origin: [f32; 3], decal: i16, entity: i16, model: i16 },
    Implosion { origin: [f32; 3], radius: u8, count: u8, life: f32 },
    SpriteTrail { start: [f32; 3], end: [f32; 3], sprite: i16, count: u8, life: f32, scale: f32, velocity: f32, randomness: f32 },
    Sprite { origin: [f32; 3], sprite: i16, scale: f32, brightness: u8 },
    BeamSprite { start: [f32; 3], end: [f32; 3], beam_model: i16, sprite_model: i16 },
    BeamTorus { center: [f32; 3], axis: [f32; 3], beam: BeamParams },
    BeamDisk { center: [f32; 3], axis: [f32; 3], beam: BeamParams },
    BeamCylinder { center: [f32; 3], axis: [f32; 3], beam: BeamParams },
    BeamFollow { entity: i16, model: i16, life: f32, width: u8, color: [u8; 3], brightness: u8 },
    GlowSprite { origin: [f32; 3], model: i16, life: f32, scale: f32, brightness: u8 },
    BeamRing { start_ent: i16, end_ent: i16, beam: BeamParams },
    StreakSplash { origin: [f32; 3], dir: [f32; 3], color: u8, count: i16, speed: i16, velocity_range: i16 },
    DLight { origin: [f32; 3], radius: f32, color: [u8; 3], life: f32, decay: f32 },
    ELight { entity: i16, origin: [f32; 3], radius: f32, color: [u8; 3], life: f32, decay: f32 },
    TextMessage(TextMessage),
    Line { start: [f32; 3], end: [f32; 3], life: f32, color: [u8; 3] },
    Box { mins: [f32; 3], maxs: [f32; 3], life: f32, color: [u8; 3] },
    KillBeam { entity: i16 },
    LargeFunnel { origin: [f32; 3], sprite: i16, flags: i16 },
    BloodStream { origin: [f32; 3], dir: [f32; 3], color: u8, speed: u8 },
    ShowLine { start: [f32; 3], end: [f32; 3] },
    Blood { origin: [f32; 3], dir: [f32; 3], color: u8, speed: u8 },
    Decal { origin: [f32; 3], texture: u16, entity: i16 },
    Fizz { entity: i16, sprite: i16, density: u8 },
    Model { origin: [f32; 3], velocity: [f32; 3], yaw: f32, model: i16, bounce_sound: u8, life: f32 },
    ExplodeModel { origin: [f32; 3], velocity: f32, model: i16, count: i16, life: f32 },
    BreakModel { origin: [f32; 3], size: [f32; 3], velocity: [f32; 3], random_velocity: f32, model: i16, count: u8, life: f32, flags: u8 },
    GunshotDecal { origin: [f32; 3], entity: i16, decal: u8 },
    SpriteSpray { origin: [f32; 3], velocity: [f32; 3], sprite: i16, count: u8, speed: f32, noise: u8 },
    ArmorRicochet { origin: [f32; 3], scale: f32 },
    PlayerDecal { player: u8, origin: [f32; 3], entity: i16, decal: u8 },
    Bubbles { mins: [f32; 3], maxs: [f32; 3], height: f32, model: i16, count: u8, speed: f32 },
    BubbleTrail { mins: [f32; 3], maxs: [f32; 3], height: f32, model: i16, count: u8, speed: f32 },
    BloodSprite { origin: [f32; 3], sprite: i16, drop_sprite: i16, color: u8, scale: u8 },
    WorldDecal { origin: [f32; 3], texture: u16 },
    Projectile { origin: [f32; 3], velocity: [f32; 3], model: i16, life: u8, owner: u8 },
    Spray { origin: [f32; 3], dir: [f32; 3], model: i16, count: u8, speed: u8, noise: u8, rendermode: u8 },
    PlayerSprites { player: i16, model: i16, count: u8, variance: u8 },
    ParticleBurst { origin: [f32; 3], radius: i16, color: u8, duration: f32 },
    FireField { origin: [f32; 3], radius: i16, model: i16, count: u8, flags: u8, duration: f32 },
    PlayerAttachment { entity: u8, vertical_offset: f32, model: i16, life: f32 },
    KillPlayerAttachments { entity: u8 },
    MultiGunshot { origin: [f32; 3], dir: [f32; 3], noise: [f32; 2], count: u8, decal: u8 },
    UserTracer { origin: [f32; 3], velocity: [f32; 3], life: f32, color: u8, length: f32 },
}

#[derive(Debug, Clone, PartialEq)]
pub struct TextMessage {
    pub channel: u8,
    /// -1..1 screen fraction, -1 = centered.
    pub x: f32,
    pub y: f32,
    pub effect: u8,
    pub color1: [u8; 4],
    pub color2: [u8; 4],
    pub fade_in: f32,
    pub fade_out: f32,
    pub hold: f32,
    pub fx_time: f32,
    pub text: String,
}

/// One parsed server message. Entity, client-data and baseline updates are applied to the
/// [`crate::ProtocolState`] frame ring; their variants only signal that it happened.
#[derive(Debug, Clone, PartialEq)]
pub enum ServerMessage {
    Disconnect(String),
    /// `svc_event`.
    Events(Vec<WireEvent>),
    /// `svc_event_reliable`.
    ReliableEvent { index: u16, args: EventArgs, delay: f32 },
    Version(i32),
    SetView(i16),
    Sound(Sound),
    Time(f32),
    Print(String),
    StuffText(String),
    SetAngle([f32; 3]),
    ServerInfo(ServerInfo),
    LightStyle { index: u8, pattern: String },
    UpdateUserInfo { slot: u8, userid: i32, info: String, hash: [u8; 16] },
    DeltaDescription(String),
    ClientData,
    StopSound { entity: u16, channel: u8 },
    Pings(Vec<Ping>),
    Particle { origin: [f32; 3], dir: [f32; 3], count: u16, color: u8 },
    SpawnStatic(StaticEntity),
    SpawnBaseline,
    TempEntity(TempEntity),
    SetPause(bool),
    SignonNum(u8),
    CenterPrint(String),
    SpawnStaticSound(StaticSound),
    Intermission,
    Finale(String),
    CdTrack { track: u8, loop_track: u8 },
    Restore { save_name: String, maps: Vec<String> },
    Cutscene(String),
    WeaponAnim { sequence: u8, body: u8 },
    DecalName { index: u8, name: String },
    RoomType(i16),
    AddAngle(f32),
    NewUserMsg { id: u8, size: i16, name: String },
    PacketEntities { delta_from: Option<u8>, count: u16 },
    Choke,
    ResourceList(ResourceList),
    NewMoveVars(MoveVars),
    ResourceRequest { spawncount: i32, start_index: i32 },
    Customization(Customization),
    CrosshairAngle { pitch: f32, yaw: f32 },
    SoundFade([u8; 4]),
    FileTransferFailed(String),
    Hltv(Hltv),
    Director(Vec<u8>),
    VoiceInit { codec: String, quality: u8 },
    VoiceData { sender: u8, data: Vec<u8> },
    SendExtraInfo { fallback_dir: String, cheats: bool },
    TimeScale(f32),
    ResourceLocation(String),
    SendCvarValue(String),
    SendCvarValue2 { request_id: i32, name: String },
    Exec { mode: u8, class: Option<u8> },
    UserMessage { id: u8, name: String, data: Vec<u8> },
}
