//! Main-menu Spec Ops flow: tier tabs, mission rows with stars and best times, the difficulty
//! popup, and the solo launch / co-op lobby hand-off. Menus: `specops_select`, `specops_launch`.

use bevy::prelude::*;
use bevy::tasks::{AsyncComputeTaskPool, Task, futures_lite::future};
use frame::{UiMenuDvars, UiMenuRequest};
use ui::frontend::rules::host_rules;
use ui::frontend::specops::{
    SO_MAX_ROWS, SO_SKILLS, SO_TIERS, SoMission, so_desc_key, so_name_key, so_time, so_total_stars,
};

use crate::{CommandSpec, ConsoleCommand, ConsoleRegistry};

const DESC_LINES: usize = 3;
const DESC_WIDTH: usize = 46;

pub(crate) struct SpecOpsMenuState {
    tier: usize,
    hover: usize,
    picked: Option<(usize, usize)>,
    skill: i32,
    strings: Option<asset_game::LocalizeCatalog>,
    strings_task: Option<Task<Option<asset_game::LocalizeCatalog>>>,
}

impl Default for SpecOpsMenuState {
    fn default() -> Self {
        Self {
            tier: 0,
            hover: 0,
            picked: None,
            skill: 1,
            strings: None,
            strings_task: None,
        }
    }
}

impl SpecOpsMenuState {
    fn text<'a>(&'a self, key: &str, fallback: &'a str) -> &'a str {
        self.strings
            .as_ref()
            .and_then(|strings| strings.text(key))
            .filter(|text| !text.is_empty())
            .unwrap_or(fallback)
    }

    fn mission(&self, tier: usize, row: usize) -> Option<&'static SoMission> {
        SO_TIERS.get(tier)?.missions.get(row)
    }
}

pub(crate) fn register(registry: &mut ConsoleRegistry) {
    for name in [
        "ui_so_open",
        "ui_so_tier",
        "ui_so_hover",
        "ui_so_pick",
        "ui_so_skill",
        "ui_so_start",
        "ui_so_host",
    ] {
        if registry.resolve(name).is_none() {
            registry.register(CommandSpec::new(name));
        }
    }
}

/// SP mission names and descriptions sit in SP `common.ff`; walk it once, off the frame.
fn start_strings(state: &mut SpecOpsMenuState, identity: Option<&ui::LaunchIdentity>) {
    if state.strings.is_some() || state.strings_task.is_some() {
        return;
    }
    let Some(root) = identity.map(|identity| identity.games_root.clone()) else {
        return;
    };
    state.strings_task = Some(AsyncComputeTaskPool::get().spawn(async move {
        let games = asset_transport::GamesRoot(root);
        let started = std::time::Instant::now();
        let common = asset_transport::find_zone_file(&games, "iw4:so_killspree_trainer")
            .and_then(|mission| asset_transport::find_zone_for_tree(&mission.path, "common"))
            .and_then(|common| asset_game::load_localize_catalog(&common.path));
        match common {
            Ok(catalog) => {
                diag::info!(
                    Ui,
                    "specops menu: {} SP localize keys in {} ms",
                    catalog.len(),
                    started.elapsed().as_millis()
                );
                Some(catalog)
            }
            Err(error) => {
                diag::warn!(Ui, "specops menu: SP localize: {error}");
                None
            }
        }
    }));
}

fn arg_index(command: &ConsoleCommand, limit: usize, what: &str) -> Result<usize, String> {
    command
        .args
        .first()
        .and_then(|arg| arg.parse::<usize>().ok())
        .filter(|index| *index < limit)
        .ok_or_else(|| format!("Invalid {what}"))
}

pub(crate) fn route(
    mut events: MessageReader<ConsoleCommand>,
    mut commands: Commands,
    mut dvars: ResMut<UiMenuDvars>,
    mut menus: MessageWriter<UiMenuRequest>,
    mut transition: ResMut<session::SessionSwapRequest>,
    maps: Res<ui::MenuMapList>,
    profile: Res<sim::SpProfile>,
    mp_strings: Option<Res<asset_game::LocalizeCatalog>>,
    identity: Option<Res<ui::LaunchIdentity>>,
    mut state: Local<SpecOpsMenuState>,
    mut echo: crate::feature_dispatch::ConsoleEcho,
) {
    if let Some(task) = state.strings_task.as_mut()
        && let Some(done) = future::block_on(future::poll_once(task))
    {
        state.strings_task = None;
        state.strings = Some(done.unwrap_or_default());
    }
    for command in events.read() {
        if !command.name.starts_with("ui_so_") {
            continue;
        }
        let result = (|| -> Result<(), String> {
            match command.name.as_str() {
                "ui_so_open" => {
                    start_strings(&mut state, identity.as_deref());
                    state.hover = 0;
                    dvars.set("ui_so_status", "");
                }
                "ui_so_tier" => {
                    state.tier = arg_index(command, SO_TIERS.len(), "tier")?;
                    state.hover = 0;
                }
                "ui_so_hover" => {
                    let row = arg_index(command, SO_MAX_ROWS, "mission row")?;
                    if state.mission(state.tier, row).is_some() {
                        state.hover = row;
                    }
                }
                "ui_so_pick" => {
                    let row = arg_index(command, SO_MAX_ROWS, "mission row")?;
                    let mission = state
                        .mission(state.tier, row)
                        .ok_or("Mission is unavailable")?;
                    if !maps.contains(&format!("iw4:{}", mission.zone)) {
                        return Err(format!("{} is not installed", mission.zone));
                    }
                    state.hover = row;
                    state.picked = Some((state.tier, row));
                    dvars.set("ui_so_launch_status", "");
                    menus.write(UiMenuRequest::Open("specops_launch".into()));
                }
                "ui_so_skill" => {
                    let skill = command
                        .args
                        .first()
                        .and_then(|arg| arg.parse::<i32>().ok())
                        .filter(|skill| SO_SKILLS.iter().any(|(value, ..)| value == skill))
                        .ok_or("Invalid difficulty")?;
                    state.skill = skill;
                }
                "ui_so_start" | "ui_so_host" => {
                    let (tier, row) = state.picked.ok_or("No mission selected")?;
                    let mission = state.mission(tier, row).ok_or("Mission is unavailable")?;
                    let map = format!("iw4:{}", mission.zone);
                    if !maps.contains(&map) {
                        return Err(format!("{} is not installed", mission.zone));
                    }
                    let mode = sim::HostGameModeSelection::from_token("so")
                        .ok_or("Spec Ops mode is unavailable")?;
                    dvars.set("g_gameskill", state.skill.to_string());
                    dvars.set("ui_mapname", map.clone());
                    dvars.set("ui_gametype", mode.token());
                    if command.name == "ui_so_host" {
                        menus.write(UiMenuRequest::Close("specops_launch".into()));
                        echo.write(format!(
                            "menu: spec ops co-op lobby {map} g_gameskill={}",
                            state.skill
                        ));
                        return Ok(());
                    }
                    commands.insert_resource(host_rules(&dvars));
                    let id = transition
                        .request_zone(map.clone())
                        .map_err(|error| error.to_string())?;
                    commands.insert_resource(mode);
                    menus.write(UiMenuRequest::Close("specops_launch".into()));
                    menus.write(UiMenuRequest::Close("specops_select".into()));
                    echo.write(format!(
                        "menu: starting {map} so g_gameskill={} (swap #{id})",
                        state.skill
                    ));
                }
                _ => {}
            }
            Ok(())
        })();
        if let Err(error) = result {
            let status = if matches!(command.name.as_str(), "ui_so_start" | "ui_so_host") {
                "ui_so_launch_status"
            } else {
                "ui_so_status"
            };
            dvars.set(status, &error);
            echo.write(format!("menu: {error}"));
        }
    }
    publish(&state, &mut dvars, &maps, &profile, mp_strings.as_deref());
}

fn total_label(total: u32) -> String {
    let max = SO_TIERS
        .iter()
        .map(|tier| tier.missions.len())
        .sum::<usize>()
        * 3;
    format!("STARS  {total} / {max}")
}

fn star_label(stars: u8) -> String {
    let stars = usize::from(stars.min(3));
    format!("{}{}", "*".repeat(stars), "-".repeat(3 - stars))
}

fn wrap(text: &str) -> [String; DESC_LINES] {
    let mut lines: [String; DESC_LINES] = Default::default();
    let mut line = 0;
    for word in text.split_whitespace() {
        if !lines[line].is_empty() && lines[line].len() + 1 + word.len() > DESC_WIDTH {
            line += 1;
            if line == DESC_LINES {
                lines[DESC_LINES - 1].push_str("...");
                break;
            }
        }
        if !lines[line].is_empty() {
            lines[line].push(' ');
        }
        lines[line].push_str(word);
    }
    lines
}

fn publish(
    state: &SpecOpsMenuState,
    dvars: &mut UiMenuDvars,
    maps: &ui::MenuMapList,
    profile: &sim::SpProfile,
    mp_strings: Option<&asset_game::LocalizeCatalog>,
) {
    let total = so_total_stars(profile);
    dvars.set("ui_so_total", total_label(total));
    for (index, tier) in SO_TIERS.iter().enumerate() {
        let label = state.text(tier.label_key, tier.label);
        let label = if total < tier.unlock {
            format!("{label} ({})", tier.unlock)
        } else {
            label.to_owned()
        };
        dvars.set(&format!("ui_so_tab_{index}"), label);
        dvars.set(
            &format!("ui_so_tab_tint_{index}"),
            if index == state.tier { "1" } else { "0.45" },
        );
    }
    let tier = &SO_TIERS[state.tier];
    let locked = total < tier.unlock;
    dvars.set(
        "ui_so_tier_note",
        if locked {
            format!(
                "LOCKED: {} stars unlock this tier; stars earned here are not saved",
                tier.unlock
            )
        } else {
            String::new()
        },
    );
    for row in 0..SO_MAX_ROWS {
        let mission = tier.missions.get(row);
        let installed = mission.is_some_and(|m| maps.contains(&format!("iw4:{}", m.zone)));
        dvars.set(
            &format!("ui_so_{row}_visible"),
            if mission.is_some() { "1" } else { "0" },
        );
        dvars.set(
            &format!("ui_so_{row}_name"),
            mission.map_or(String::new(), |m| {
                state.text(&so_name_key(m.zone), m.name).to_uppercase()
            }),
        );
        dvars.set(
            &format!("ui_so_{row}_stars"),
            mission.map_or(String::new(), |m| match installed {
                true => star_label(profile.so_stars(m.index)),
                false => "N/A".into(),
            }),
        );
        dvars.set(
            &format!("ui_so_{row}_time"),
            mission
                .and_then(|m| profile.so_best_time(m.index))
                .map_or(String::new(), so_time),
        );
    }
    let detail = state.mission(state.tier, state.hover);
    dvars.set(
        "ui_so_detail_name",
        detail.map_or(String::new(), |m| {
            state.text(&so_name_key(m.zone), m.name).to_uppercase()
        }),
    );
    let desc = detail.map_or(String::new(), |m| {
        state.text(&so_desc_key(m.zone), "").to_owned()
    });
    for (index, line) in wrap(&desc).into_iter().enumerate() {
        dvars.set(&format!("ui_so_detail_desc_{index}"), line);
    }
    dvars.set(
        "ui_so_detail_info",
        detail.map_or(String::new(), |m| {
            let best = profile.so_best_time(m.index).map_or("--".into(), so_time);
            format!(
                "STARS {} / 3     BEST TIME {best}",
                profile.so_stars(m.index)
            )
        }),
    );
    dvars.set(
        "ui_so_detail_status",
        match detail {
            Some(m) if !maps.contains(&format!("iw4:{}", m.zone)) => {
                "Mission zone or its base map is not installed"
            }
            Some(m) if !m.picks_skill => "Stars come from your performance, not difficulty",
            _ => "",
        },
    );
    let picked = state
        .picked
        .and_then(|(tier, row)| state.mission(tier, row));
    dvars.set(
        "ui_so_pick_name",
        picked.map_or(String::new(), |m| {
            state.text(&so_name_key(m.zone), m.name).to_uppercase()
        }),
    );
    dvars.set(
        "ui_so_pick_note",
        match picked {
            Some(m) if !m.picks_skill => "Stars come from your performance on this mission.",
            _ => "Stars earned: Regular 1, Hardened 2, Veteran 3.",
        },
    );
    for (index, (skill, key, fallback)) in SO_SKILLS.iter().enumerate() {
        let label = mp_strings
            .and_then(|strings| strings.text(key))
            .or_else(|| state.strings.as_ref().and_then(|strings| strings.text(key)))
            .unwrap_or(fallback)
            .to_uppercase();
        dvars.set(
            &format!("ui_so_skill_{index}"),
            if *skill == state.skill {
                format!("> {label} <")
            } else {
                label
            },
        );
    }
}
