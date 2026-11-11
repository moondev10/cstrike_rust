//! Counter-Strike client DLL behaviour (client.dll/client.so), rebuilt on top of the engine
//! client in `gs-client`: view calculation (`V_CalcRefdef`), the studio model renderer's
//! animation side (`CGameStudioModelRenderer`) and the event handlers (`ev_cs`).

pub mod cam;
pub mod cvars;
pub mod events;
pub mod hud;
pub mod input;
pub mod menu;
pub mod models;
pub mod particleman;
pub mod view;
pub mod weapons;
pub mod weather;

/// The client DLL's commands in registration order (`InitInput`, `CAM_Init`, `IN_Init`,
/// `CHud::Init` and its elements; audit-commands.md §1.3): each goes to the head of the engine's
/// command list with flag 1. The czero-only `credits`/`buy_preset`/`buy_preset_edit` are absent.
pub const COMMANDS: &[&str] = &[
    // InitInput.
    "+moveup", "-moveup", "+movedown", "-movedown", "+left", "-left", "+right", "-right", "+forward", "-forward", "+back", "-back",
    "+lookup", "-lookup", "+lookdown", "-lookdown", "+strafe", "-strafe", "+moveleft", "-moveleft", "+moveright", "-moveright",
    "+speed", "-speed", "+attack", "-attack", "+attack2", "-attack2", "+use", "-use", "+jump", "-jump", "impulse", "+klook",
    "-klook", "+mlook", "-mlook", "+jlook", "-jlook", "+duck", "-duck", "+reload", "-reload", "+alt1", "-alt1", "+score",
    "-score", "+showscores", "-showscores", "+graph", "-graph", "+break", "-break",
    // CAM_Init.
    "+campitchup", "-campitchup", "+campitchdown", "-campitchdown", "+camyawleft", "-camyawleft", "+camyawright",
    "-camyawright", "+camin", "-camin", "+camout", "-camout", "thirdperson", "firstperson", "+cammousemove", "-cammousemove",
    "+camdistance", "-camdistance", "snapto",
    // IN_Init.
    "force_centerview", "joyadvancedupdate",
    // CHud::Init, CHudNightVision, CHudAmmo, CHudHealth, CHudSayText, CHudSpectator, CVoiceStatus.
    "+commandmenu", "-commandmenu", "gunsmoke", "autobuy", "rebuy", "buy", "+nvgadjust", "-nvgadjust", "slot1", "slot2", "slot3",
    "slot4", "slot5", "slot6", "slot7", "slot8", "slot9", "slot10", "cancelselect", "invnext", "invprev", "adjust_crosshair",
    "trackplayer", "clearplayers", "drawradar", "hideradar", "hud_saytext", "spec_mode", "spec_toggleinset", "spec_decal",
    "spec_help", "spec_menu", "togglescores", "spec_drawnames", "spec_drawcone", "spec_drawstatus", "spec_autodirector",
    "voice_showbanned",
];
