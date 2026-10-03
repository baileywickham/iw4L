//! Spec Ops mission select data from SP `common.ff` `sp/specOpsTable.csv`: column 0 is the
//! profile index (digit of `missionSOHighestDifficulty`, best-time field `s<index>`), 13 the tier,
//! 14 whether the mission takes a difficulty (0: stars come from the run itself). Tier unlocks
//! are the `so_alpha`…`so_echo` rows' column 5. Order inside a tier is the MW2 menu order.

pub struct SoTier {
    pub label_key: &'static str,
    pub label: &'static str,
    pub unlock: u32,
    pub missions: &'static [SoMission],
}

pub struct SoMission {
    pub zone: &'static str,
    pub index: usize,
    pub name: &'static str,
    pub picks_skill: bool,
}

const fn m(zone: &'static str, index: usize, name: &'static str, picks_skill: bool) -> SoMission {
    SoMission {
        zone,
        index,
        name,
        picks_skill,
    }
}

pub const SO_TIERS: [SoTier; 5] = [
    SoTier {
        label_key: "SPECIAL_OPS_SO_BUTTON_ALPHA_CAPS",
        label: "ALPHA",
        unlock: 0,
        missions: &[
            m("so_killspree_trainer", 22, "The Pit", false),
            m("so_rooftop_contingency", 13, "Sniper Fi", true),
            m("so_killspree_favela", 3, "O Cristo Redentor", true),
            m("so_forest_contingency", 7, "Evasion", true),
            m("so_crossing_so_bridge", 10, "Suspension", true),
        ],
    },
    SoTier {
        label_key: "SPECIAL_OPS_SO_BUTTON_BETA_CAPS",
        label: "BRAVO",
        unlock: 4,
        missions: &[
            m("so_ac130_co_hunted", 2, "Overwatch", true),
            m("so_killspree_invasion", 1, "Body Count", true),
            m("so_defuse_favela_escape", 15, "Bomb Squad", true),
            m("so_snowrace1_cliffhanger", 11, "Race", false),
            m("so_chopper_invasion", 21, "Big Brother", true),
        ],
    },
    SoTier {
        label_key: "SPECIAL_OPS_SO_BUTTON_CHARLIE_CAPS",
        label: "CHARLIE",
        unlock: 8,
        missions: &[
            m("so_hidden_so_ghillies", 20, "Hidden", true),
            m("so_showers_gulag", 0, "Breach & Clear", true),
            m("so_snowrace2_cliffhanger", 12, "Time Trial", true),
            m("so_defense_invasion", 5, "Homeland Security", true),
            m("so_intel_boneyard", 18, "Snatch", true),
        ],
    },
    SoTier {
        label_key: "SPECIAL_OPS_SO_BUTTON_DELTA_CAPS",
        label: "DELTA",
        unlock: 20,
        missions: &[
            m("so_download_arcadia", 8, "Wardriving", true),
            m("so_demo_so_bridge", 9, "Wreckage", true),
            m("so_sabotage_cliffhanger", 14, "Acceptable Losses", true),
            m("so_escape_airport", 6, "Terminal", true),
            m("so_takeover_estate", 16, "Estate Takedown", true),
        ],
    },
    SoTier {
        label_key: "SPECIAL_OPS_SO_BUTTON_ECHO_CAPS",
        label: "ECHO",
        unlock: 40,
        missions: &[
            m("so_assault_oilrig", 4, "Wetwork", true),
            m("so_juggernauts_favela", 19, "High Explosive", true),
            m("so_takeover_oilrig", 17, "Armor Piercing", true),
        ],
    },
];

pub const SO_MAX_ROWS: usize = 5;

/// `g_gameskill` values the Spec Ops menu offers (`_gameskill`: 1 normal, 2 hardened, 3 veteran).
pub const SO_SKILLS: [(i32, &str, &str); 3] = [
    (1, "MENU_REGULAR", "REGULAR"),
    (2, "MENU_HARDENED", "HARDENED"),
    (3, "MENU_VETERAN", "VETERAN"),
];

pub fn so_name_key(zone: &str) -> String {
    format!("SPECIAL_OPS_{}", zone.to_ascii_uppercase())
}

pub fn so_desc_key(zone: &str) -> String {
    format!("SPECIAL_OPS_{}_DESC", zone.to_ascii_uppercase())
}

pub fn so_total_stars(profile: &sim::SpProfile) -> u32 {
    SO_TIERS
        .iter()
        .flat_map(|tier| tier.missions)
        .map(|mission| u32::from(profile.so_stars(mission.index)))
        .sum()
}

pub fn so_time(ms: u32) -> String {
    let centis = ms / 10;
    format!(
        "{}:{:02}.{:02}",
        centis / 6000,
        centis / 100 % 60,
        centis % 100
    )
}
