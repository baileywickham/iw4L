//! Single-player objective presentation: the typewritten objective line in the
//! top-left game message window (`Con_TypewriterPrint`) and the onscreen
//! objective pointer with its distance (`objective_onscreen`, "189m").

use std::collections::HashMap;

use asset_game::MenuCatalog;
use assets::PreparedLocalizedStrings;
use bevy::prelude::*;
use net::{FrameClock, LocalPresentClient, PresentedSnapshot};
use sim::{CompassObjective, ObjectiveMessage, ObjectiveState};

use crate::draw2d::{
    Draw2dCmd, Draw2dList, Draw2dOp, Draw2dProvenance, TextRunFx, tessellate_fonts,
};
use crate::gaps::{GapCause, HudPresentationGaps};
use crate::gpu_list::{HudTessPass, TessJob};
use crate::images::HudImages;

#[derive(Component)]
pub(crate) struct SpObjectivesRaster;

/// `con_typewriterPrintSpeed`, `con_typewriterDecayStartTime`, `con_typewriterDecayDuration`.
const TYPEWRITER_LETTER_MS: i32 = 50;
const TYPEWRITER_DECAY_START_MS: i32 = 6000;
const TYPEWRITER_DECAY_MS: i32 = 700;
/// `con_typewriterColorGlowUpdated` / `Completed` / `Failed`.
const GLOW_UPDATED: [f32; 4] = [0.0, 0.6, 0.18, 1.0];
const GLOW_COMPLETED: [f32; 4] = [0.0, 0.3, 0.8, 1.0];
const GLOW_FAILED: [f32; 4] = [0.8, 0.0, 0.0, 1.0];
/// The SP game message window: top left of the safe area, objective font.
const MESSAGE_MENU: &str = "gamemessages";
const MESSAGE_X: f32 = 6.0;
const MESSAGE_Y: f32 = 24.0;
const MESSAGE_FONT: i32 = 6;
const MESSAGE_TEXT_SCALE: f32 = 0.3;

const ONSCREEN_ICON: &str = "objective_onscreen";
const ONSCREEN_ICON_SIZE: f32 = 16.0;
/// `objectiveFontSize` (a `smallfont` scale), `objectiveTextOffsetY`.
const ONSCREEN_TEXT_SCALE: f32 = 0.3;
const ONSCREEN_TEXT_OFFSET_Y: f32 = -5.33;
/// `waypointDistScaleRangeMin` / `Max` / `Smallest`.
const DIST_SCALE_MIN: f32 = 1000.0;
const DIST_SCALE_MAX: f32 = 3000.0;
const DIST_SCALE_SMALLEST: f32 = 0.8;
const INCHES_TO_METERS: f32 = 0.0254;
const ONSCREEN_FONT: &str = "fonts/smallfont";

fn hide(pass: &mut HudTessPass) {
    pass.sp_objectives = TessJob::Hide;
}

fn objective_text(
    objective: &CompassObjective,
    strings: &PreparedLocalizedStrings,
) -> Option<String> {
    let template = crate::hudelem::resolve_hud_text(strings, &objective.text)?;
    let mut text = template;
    for (i, arg) in objective.text_args.iter().enumerate() {
        let arg = crate::hudelem::resolve_hud_text(strings, arg).unwrap_or_else(|| arg.clone());
        text = text.replace(&format!("&&{}", i + 1), &arg);
    }
    Some(text)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn update(
    surface: Res<crate::surface::Hud2dSurface>,
    catalog: Option<Res<MenuCatalog>>,
    strings: Option<Res<PreparedLocalizedStrings>>,
    presented: Res<PresentedSnapshot>,
    local: Res<LocalPresentClient>,
    cg_clock: Res<FrameClock>,
    input: Res<frame::HudInputView>,
    mut pass: ResMut<HudTessPass>,
    mut gaps: ResMut<HudPresentationGaps>,
    mut hud_images: ResMut<HudImages>,
    mut images: ResMut<Assets<Image>>,
    cameras: Query<(&Camera, &GlobalTransform), With<Camera3d>>,
    view: Option<Res<frame::ViewSubject>>,
    mut logged: Local<bool>,
) {
    hide(&mut pass);
    if !surface.is_ready() || input.script_menu_open || view.is_some_and(|v| v.in_killcam()) {
        return;
    }
    let Some(snapshot) = presented.snapshot() else {
        return;
    };
    if snapshot.meta.kind != gamemode_iw4::GameModeKind::SpecOps {
        return;
    }
    let Some(ps) = presented.alive_player(local.0) else {
        return;
    };
    let (Some(catalog), Some(strings)) = (catalog.as_ref(), strings.as_ref()) else {
        return;
    };
    let objectives = &snapshot.meta.objectives.compass;
    if objectives.is_empty() {
        return;
    }
    let cg_time = cg_clock.time();
    let mut cmds = Vec::new();
    let mut fonts = HashMap::new();

    let window = catalog.get(MESSAGE_MENU).and_then(|menu| {
        menu.items
            .iter()
            .find(|item| item.item_type == hud_iw4::ITEM_TYPE_GAME_MESSAGE_WINDOW)
            .map(|item| (menu, item))
    });
    let (x, y, horz, vert, font_enum, text_scale) = match window {
        Some((menu, item)) => (
            menu.rect.x + item.rect.x,
            menu.rect.y + item.rect.y,
            i32::from(menu.rect.horz_align),
            i32::from(menu.rect.vert_align),
            item.font_enum,
            item.text_scale,
        ),
        None => (
            MESSAGE_X,
            MESSAGE_Y,
            hud_iw4::ALIGN_USER_MIN,
            hud_iw4::ALIGN_USER_MIN,
            MESSAGE_FONT,
            MESSAGE_TEXT_SCALE,
        ),
    };
    if !*logged {
        *logged = true;
        diag::info!(
            Ui,
            "hud: sp objective window {} at {x} {y} align {horz} {vert} font {font_enum} scale {text_scale}",
            if window.is_some() {
                MESSAGE_MENU
            } else {
                "fallback"
            }
        );
    }
    let mut lines: Vec<(&CompassObjective, String)> = objectives
        .iter()
        .filter(|o| o.message != ObjectiveMessage::None && o.message_ms > 0)
        .filter_map(|o| Some((o, objective_text(o, strings)?)))
        .filter(|(o, text)| {
            let len = hud_iw4::seh_print_strlen(text);
            let decay = TYPEWRITER_DECAY_START_MS.max(TYPEWRITER_LETTER_MS * len);
            cg_time.wrapping_sub(o.message_ms) <= decay + TYPEWRITER_DECAY_MS
        })
        .collect();
    lines.sort_by_key(|(o, _)| o.message_ms);
    let font_name =
        hud_iw4::ui_get_font_handle(font_enum, surface.scale_virtual_to_real()[1], text_scale);
    if !lines.is_empty() {
        match catalog.font(font_name) {
            Some(font) => {
                let material = asset_core::AssetRef::bare_name(&font.material).to_owned();
                if hud_images
                    .get(crate::images::HUD_CHROME_NAMESPACE, &material, &mut images)
                    .is_none()
                {
                    gaps.raise(GapCause::FontAtlasMissing {
                        image: hud_images.zone_image_name(&material).map(str::to_owned),
                        material: material.clone(),
                    });
                }
                let nscale = hud_iw4::normalized_text_scale(font.pixel_height, text_scale);
                let line_h = hud_iw4::ui_text_height(text_scale);
                for (i, (objective, text)) in lines.into_iter().enumerate() {
                    let glow = match objective.message {
                        ObjectiveMessage::Completed => GLOW_COMPLETED,
                        ObjectiveMessage::Failed => GLOW_FAILED,
                        _ => GLOW_UPDATED,
                    };
                    let len = hud_iw4::seh_print_strlen(&text);
                    let rect =
                        surface.apply_rect(x, y + line_h * i as f32, nscale, nscale, horz, vert);
                    cmds.push(Draw2dCmd {
                        material_namespace: crate::images::HUD_CHROME_NAMESPACE,
                        x: (rect.x + 0.5).floor(),
                        y: (rect.y + 0.5).floor(),
                        w: rect.w,
                        h: rect.h,
                        s0: 0.0,
                        t0: 0.0,
                        s1: 1.0,
                        t1: 1.0,
                        color: [1.0; 4],
                        material: material.clone(),
                        op: Draw2dOp::TextRun {
                            font: font_name.to_owned(),
                            scale: nscale,
                            text,
                            loc_key: objective.text.clone(),
                            style: crate::draw2d::TEXT_STYLE_HUDELEM,
                            fx: Some(TextRunFx {
                                scene_time: cg_time,
                                fx: hud_iw4::TextPulseFx {
                                    birth_time: objective.message_ms.min(cg_time),
                                    letter_time: TYPEWRITER_LETTER_MS,
                                    decay_start_time: TYPEWRITER_DECAY_START_MS
                                        .max(TYPEWRITER_LETTER_MS * len),
                                    decay_duration: TYPEWRITER_DECAY_MS,
                                },
                            }),
                            glow: crate::chrome::text_run_glow(font, glow),
                        },
                        provenance: Draw2dProvenance::CgDraw {
                            site: "sp_objective_typewriter",
                        },
                        layer: 1,
                    });
                }
                fonts.insert(font_name.to_owned(), font);
            }
            None => gaps.raise(GapCause::FontMissing {
                name: font_name.to_owned(),
            }),
        }
    }

    let camera = cameras.iter().find(|(c, _)| c.is_active);
    let small = catalog.font(ONSCREEN_FONT);
    let icon_ok = hud_images
        .get(
            crate::images::HUD_CHROME_NAMESPACE,
            ONSCREEN_ICON,
            &mut images,
        )
        .is_some();
    if let (Some((camera, transform)), Some(font)) = (camera, small) {
        let eye = transform.translation();
        let forward = *transform.forward();
        let scale = surface.scale_virtual_to_real()[1];
        let material = asset_core::AssetRef::bare_name(&font.material).to_owned();
        let nscale = hud_iw4::normalized_text_scale(font.pixel_height, ONSCREEN_TEXT_SCALE);
        for objective in objectives
            .iter()
            .filter(|o| o.state == ObjectiveState::Current && o.origin != [0.0; 3])
        {
            let target = Vec3::from_array(objective.origin);
            if (target - eye).dot(forward) <= 0.0 {
                continue;
            }
            let Ok(pos) = camera.world_to_viewport(transform, target) else {
                continue;
            };
            if !(0.0..=surface.width()).contains(&pos.x)
                || !(0.0..=surface.height()).contains(&pos.y)
            {
                continue;
            }
            let dist = Vec3::from_array(ps.origin).distance(target);
            let t = ((dist - DIST_SCALE_MIN) / (DIST_SCALE_MAX - DIST_SCALE_MIN)).clamp(0.0, 1.0);
            let dist_scale = 1.0 + (DIST_SCALE_SMALLEST - 1.0) * t;
            let size = ONSCREEN_ICON_SIZE * dist_scale * scale;
            if icon_ok {
                cmds.push(Draw2dCmd {
                    material_namespace: crate::images::HUD_CHROME_NAMESPACE,
                    x: pos.x - size * 0.5,
                    y: pos.y - size * 0.5,
                    w: size,
                    h: size,
                    s0: 0.0,
                    t0: 0.0,
                    s1: 1.0,
                    t1: 1.0,
                    color: [1.0; 4],
                    material: ONSCREEN_ICON.to_owned(),
                    op: Draw2dOp::StretchPic,
                    provenance: Draw2dProvenance::Objective,
                    layer: 1,
                });
            }
            let text = format!("{}m", (dist * INCHES_TO_METERS).round() as i32);
            let glyph = nscale * scale * dist_scale;
            let width = crate::chrome::text_width(font, &text) as f32 * glyph;
            cmds.push(Draw2dCmd {
                material_namespace: crate::images::HUD_CHROME_NAMESPACE,
                x: (pos.x - width * 0.5 + 0.5).floor(),
                y: (pos.y - size * 0.5 + ONSCREEN_TEXT_OFFSET_Y * scale + 0.5).floor(),
                w: glyph,
                h: glyph,
                s0: 0.0,
                t0: 0.0,
                s1: 1.0,
                t1: 1.0,
                color: [1.0; 4],
                material: material.clone(),
                op: Draw2dOp::TextRun {
                    font: ONSCREEN_FONT.to_owned(),
                    scale: glyph,
                    text,
                    loc_key: String::new(),
                    style: crate::draw2d::TEXT_STYLE_HUDELEM,
                    fx: None,
                    glow: None,
                },
                provenance: Draw2dProvenance::Objective,
                layer: 1,
            });
        }
        fonts.insert(ONSCREEN_FONT.to_owned(), font);
    }
    if cmds.is_empty() {
        return;
    }
    let (quads, _) = tessellate_fonts(&Draw2dList { cmds }, &fonts);
    if !quads.is_empty() {
        pass.sp_objectives = TessJob::Quads(quads);
    }
}
