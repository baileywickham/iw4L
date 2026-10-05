//! `CG_DrawDamageDirectionIndicators`: a `hit_direction` arc around the
//! screen centre pointing at where each recent hit came from.

use bevy::prelude::*;
use frame::LifeStarted;
use net::{FrameClock, LocalPresentClient, PresentedSnapshot};

use crate::draw2d::{Draw2dProvenance, Draw2dQuad};
use crate::gpu_list::{HudTessPass, TessJob};
use crate::images::{HUD_CHROME_NAMESPACE, HudImages};

const MATERIAL: &str = "hit_direction";
const SLOTS: usize = 8;
// cg_hudDamageIconWidth / Height / Offset / Time defaults.
const ICON_WIDTH: f32 = 128.0;
const ICON_HEIGHT: f32 = 64.0;
const ICON_OFFSET: f32 = 128.0;
const ICON_TIME_MS: i32 = 2000;
const UNDIRECTED: u32 = 255;

#[derive(Component)]
pub(crate) struct DamageDirectionRaster;

#[derive(Clone, Copy, Default)]
struct ViewDamage {
    time: i32,
    duration: i32,
    yaw: f32,
}

#[derive(Resource, Default)]
pub(crate) struct DamageDirections {
    slots: [ViewDamage; SLOTS],
    last_event: Option<u32>,
}

impl DamageDirections {
    /// `CG_DamageFeedback`: a new event takes the oldest slot; `yaw` is the
    /// world yaw the hit travelled along (attacker to victim).
    fn feed(&mut self, yaw_byte: u32, pitch_byte: u32, now: i32) {
        if yaw_byte == UNDIRECTED && pitch_byte == UNDIRECTED {
            return;
        }
        let yaw = yaw_byte as f32 * 360.0 / 256.0;
        let slot = self
            .slots
            .iter_mut()
            .min_by_key(|s| if s.duration == 0 { i32::MIN } else { s.time })
            .expect("slots");
        *slot = ViewDamage {
            time: now,
            duration: ICON_TIME_MS,
            yaw,
        };
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn update(
    surface: Res<crate::surface::Hud2dSurface>,
    clock: Res<FrameClock>,
    presented: Res<PresentedSnapshot>,
    local: Res<LocalPresentClient>,
    view: Option<Res<frame::ViewSubject>>,
    mut started: MessageReader<LifeStarted>,
    mut state: ResMut<DamageDirections>,
    mut pass: ResMut<HudTessPass>,
    mut hud_images: ResMut<HudImages>,
    mut images: ResMut<Assets<Image>>,
) {
    pass.damage_direction = TessJob::Hide;
    for ev in started.read() {
        if ev.client == local.0.0 {
            *state = DamageDirections::default();
        }
    }
    let Some(ps) = presented.alive_player(local.0) else {
        *state = DamageDirections::default();
        return;
    };
    let now = clock.time();
    if let Some(last) = state.last_event
        && last != ps.damage_event
        && ps.damage_count != 0
    {
        state.feed(ps.damage_yaw, ps.damage_pitch, now);
    }
    state.last_event = Some(ps.damage_event);
    if !surface.is_ready() || view.as_ref().is_some_and(|v| v.in_killcam()) {
        return;
    }
    let view_yaw = ps.viewangles[1];
    let mut quads = Vec::new();
    for slot in &mut state.slots {
        if slot.duration == 0 {
            continue;
        }
        let left = slot.time + slot.duration - now;
        if left <= 0 {
            slot.duration = 0;
            continue;
        }
        let alpha = (left as f32 / slot.duration as f32 * 2.0).min(1.0);
        // Toward the attacker, relative to the view: 0 ahead, 90 to the left.
        let rel = (slot.yaw + 180.0 - view_yaw).to_radians();
        // Clockwise screen turn of the arc's "up" so it faces the attacker.
        let deg = -rel.to_degrees();
        let (sin, cos) = deg.to_radians().sin_cos();
        let centre = [-rel.sin() * ICON_OFFSET, -rel.cos() * ICON_OFFSET];
        let xy = [[-0.5, -0.5], [0.5, -0.5], [0.5, 0.5], [-0.5, 0.5]].map(|p: [f32; 2]| {
            let x = p[0] * ICON_WIDTH;
            let y = p[1] * ICON_HEIGHT;
            let r = surface.apply_rect(
                centre[0] + x * cos - y * sin,
                centre[1] + x * sin + y * cos,
                0.0,
                0.0,
                hud_iw4::ALIGN_CENTER,
                hud_iw4::ALIGN_CENTER,
            );
            [r.x, r.y]
        });
        quads.push(Draw2dQuad {
            xy,
            st: [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
            color: [1.0, 1.0, 1.0, alpha],
            material: MATERIAL.to_owned(),
            material_namespace: HUD_CHROME_NAMESPACE,
            provenance: Draw2dProvenance::CgDraw {
                site: "damage_direction",
            },
            layer: 1,
            clip: None,
        });
    }
    if quads.is_empty()
        || hud_images
            .get(HUD_CHROME_NAMESPACE, MATERIAL, &mut images)
            .is_none()
    {
        return;
    }
    pass.damage_direction = TessJob::Quads(quads);
}
