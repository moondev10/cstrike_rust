//! Static weapon data (docs/re/client-sim.md Part C §3–§7, Part D §3.1–§3.2): ids, `ItemInfo`,
//! view models and event scripts.

/// CS weapon ids (`m_iId`).
pub mod id {
    pub const P228: i32 = 1;
    pub const SCOUT: i32 = 3;
    pub const HEGRENADE: i32 = 4;
    pub const XM1014: i32 = 5;
    pub const C4: i32 = 6;
    pub const MAC10: i32 = 7;
    pub const AUG: i32 = 8;
    pub const SMOKEGRENADE: i32 = 9;
    pub const ELITE: i32 = 10;
    pub const FIVESEVEN: i32 = 11;
    pub const UMP45: i32 = 12;
    pub const SG550: i32 = 13;
    pub const GALIL: i32 = 14;
    pub const FAMAS: i32 = 15;
    pub const USP: i32 = 16;
    pub const GLOCK18: i32 = 17;
    pub const AWP: i32 = 18;
    pub const MP5N: i32 = 19;
    pub const M249: i32 = 20;
    pub const M3: i32 = 21;
    pub const M4A1: i32 = 22;
    pub const TMP: i32 = 23;
    pub const G3SG1: i32 = 24;
    pub const FLASHBANG: i32 = 25;
    pub const DEAGLE: i32 = 26;
    pub const SG552: i32 = 27;
    pub const AK47: i32 = 28;
    pub const KNIFE: i32 = 29;
    pub const P90: i32 = 30;
}

/// The player's reserve-ammo mirror a weapon's `Reload` checks (`player.ammo_*`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reserve {
    None,
    Buckshot,
    Nine,
    Nato556,
    Nato556Box,
    Nato762,
    Acp45,
    Ae50,
    Mag338,
    Mm57,
    Sig357,
}

/// `ItemInfo` plus the client's model and event names.
#[derive(Debug, Clone, Copy)]
pub struct Info {
    pub max_clip: i32,
    pub flags: i32,
    pub slot: i32,
    pub position: i32,
    pub weight: i32,
    pub reserve: Reserve,
    /// `v_<name>.mdl` base name.
    pub model: &'static str,
    /// `events/<name>.sc` (empty: none).
    pub event: &'static str,
}

#[allow(clippy::too_many_arguments)]
const fn info(max_clip: i32, flags: i32, slot: i32, position: i32, weight: i32, reserve: Reserve, model: &'static str, event: &'static str) -> Info {
    Info { max_clip, flags, slot, position, weight, reserve, model, event }
}

/// `ItemInfoArray[id]` for ids 1..=30 (2 is unused).
pub fn item(id: i32) -> Option<Info> {
    use Reserve as R;
    Some(match id {
        id::P228 => info(13, 0, 1, 3, 5, R::Sig357, "p228", "p228"),
        id::SCOUT => info(10, 0, 0, 9, 30, R::Nato762, "scout", "scout"),
        id::HEGRENADE => info(-1, 24, 3, 1, 2, R::None, "hegrenade", ""),
        id::XM1014 => info(7, 0, 0, 12, 20, R::Buckshot, "xm1014", "xm1014"),
        id::C4 => info(-1, 24, 4, 3, 3, R::None, "c4", ""),
        id::MAC10 => info(30, 0, 0, 13, 25, R::Acp45, "mac10", "mac10"),
        id::AUG => info(30, 0, 0, 14, 25, R::Nato556, "aug", "aug"),
        id::SMOKEGRENADE => info(-1, 24, 3, 3, 1, R::None, "smokegrenade", ""),
        id::ELITE => info(30, 0, 1, 5, 5, R::Nine, "elite", ""),
        id::FIVESEVEN => info(20, 0, 1, 6, 5, R::Mm57, "fiveseven", "fiveseven"),
        id::UMP45 => info(25, 0, 0, 15, 25, R::Acp45, "ump45", "ump45"),
        id::SG550 => info(30, 0, 0, 16, 20, R::Nato556, "sg550", "sg550"),
        id::GALIL => info(35, 0, 0, 17, 25, R::Nato556, "galil", "galil"),
        id::FAMAS => info(25, 0, 0, 18, 75, R::Nato556, "famas", "famas"),
        id::USP => info(12, 0, 1, 4, 5, R::Acp45, "usp", "usp"),
        id::GLOCK18 => info(20, 0, 1, 2, 5, R::Nine, "glock18", "glock18"),
        id::AWP => info(10, 0, 0, 2, 30, R::Mag338, "awp", "awp"),
        id::MP5N => info(30, 0, 0, 7, 25, R::Nine, "mp5", "mp5n"),
        id::M249 => info(100, 0, 0, 4, 25, R::Nato556Box, "m249", "m249"),
        id::M3 => info(8, 0, 0, 5, 20, R::Buckshot, "m3", "m3"),
        id::M4A1 => info(30, 0, 0, 6, 25, R::Nato556, "m4a1", "m4a1"),
        id::TMP => info(30, 0, 0, 11, 25, R::Nine, "tmp", "tmp"),
        id::G3SG1 => info(20, 0, 0, 3, 20, R::Nato762, "g3sg1", "g3sg1"),
        id::FLASHBANG => info(-1, 24, 3, 2, 1, R::None, "flashbang", ""),
        id::DEAGLE => info(7, 0, 1, 1, 7, R::Ae50, "deagle", "deagle"),
        id::SG552 => info(30, 0, 0, 10, 25, R::Nato556, "sg552", "sg552"),
        id::AK47 => info(30, 0, 0, 1, 25, R::Nato762, "ak47", "ak47"),
        id::KNIFE => info(-1, 0, 2, 1, 0, R::None, "knife", ""),
        id::P90 => info(50, 0, 0, 8, 26, R::Mm57, "p90", "p90"),
        _ => return None,
    })
}

/// `ITEM_FLAG_NOAUTORELOAD`.
pub const FLAG_NOAUTORELOAD: i32 = 2;
/// Weapons without a secondary attack on the client (`HasSecondaryAttack`).
pub const NO_SECONDARY_MASK: u32 = 0x54B8_5CA2;
/// Pistols (semi-automatic re-arm on release).
pub const PISTOL_MASK: u32 = 0x0403_0C02;
