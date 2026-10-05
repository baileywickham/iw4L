//! Dev-only "vibe check" capture: presents frames at a fixed simulated
//! timestep and writes every presented frame plus per-entity presentation
//! metrics, so an offline reviewer (human or model) can judge motion.
//!
//! `IW4L_FRAME_DUMP=<dir>[,fps]` turns it on. `IW4L_FRAME_DUMP_FROM=replay|live`
//! (default `replay`: only while a demo plays), `IW4L_FRAME_DUMP_WIDTH` (default
//! 960, `0` = native), `IW4L_FRAME_DUMP_MAX` (frames), `IW4L_FRAME_DUMP_SKIP_MS`
//! (simulated ms to let pass before the first frame), `IW4L_FRAME_DUMP_NO_PNG=1`
//! (metrics only). Output: `<dir>/NNNNN.png`, `frames.csv`, `entities.csv`,
//! `capture.json`. Analysis lives in `tools/specops/vibe/`.

use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use assets::LoadingScreen;
use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, ScreenshotCaptured};
use bevy::tasks::IoTaskPool;
use bevy::time::TimeUpdateStrategy;
use frame::{AppScreen, HasWorld, LaunchIdentity};
use render_frontend::prepare::scene::cull::DpvsFrameStats;
use render_scene::{HostGfxScene, WorldScriptModelInstance};

use super::capture::{CaptureFrameFacts, capture_wait};

const MAX_ENCODES_IN_FLIGHT: usize = 6;
const MOVE_CLIP_SPEED: f32 = 10.0;

static ENCODES_IN_FLIGHT: AtomicUsize = AtomicUsize::new(0);
static FRAMES_WRITTEN: AtomicUsize = AtomicUsize::new(0);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DumpFrom {
    Replay,
    Live,
}

#[derive(Resource)]
pub struct FrameDump {
    dir: PathBuf,
    fps: f32,
    width: u32,
    max_frames: Option<u32>,
    skip_ms: f32,
    png: bool,
    from: DumpFrom,
    state: DumpState,
    frame: u32,
    sim_ms: f64,
    settled: u32,
    readbacks_owed: Arc<AtomicUsize>,
    last_wall: Option<Instant>,
    frames_csv: Option<std::io::BufWriter<std::fs::File>>,
    entities_csv: Option<std::io::BufWriter<std::fs::File>>,
    first_tick: Option<u32>,
    last_tick: Option<u32>,
    resolution: (u32, u32),
    skeletons: HashMap<String, Option<Arc<Skeleton>>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DumpState {
    Waiting,
    Capturing,
    Draining,
    Done,
}

struct Skeleton {
    dobj: xmodel_runtime::DObj,
    bind: Vec<Vec3>,
    wrists: Option<(usize, usize)>,
    head: Option<usize>,
}

impl FrameDump {
    pub fn from_env() -> Option<Self> {
        let raw = std::env::var("IW4L_FRAME_DUMP").ok()?;
        let mut parts = raw.split(',');
        let dir = PathBuf::from(parts.next()?.trim());
        if dir.as_os_str().is_empty() {
            return None;
        }
        let fps = parts
            .next()
            .and_then(|s| s.trim().parse::<f32>().ok())
            .filter(|f| *f > 0.0 && *f <= 240.0)
            .unwrap_or(30.0);
        let env_num = |key: &str| {
            std::env::var(key)
                .ok()
                .and_then(|s| s.trim().parse::<f64>().ok())
        };
        let from = match std::env::var("IW4L_FRAME_DUMP_FROM").as_deref() {
            Ok("live") => DumpFrom::Live,
            _ => DumpFrom::Replay,
        };
        Some(Self {
            dir,
            fps,
            width: env_num("IW4L_FRAME_DUMP_WIDTH").map_or(960, |w| w as u32),
            max_frames: env_num("IW4L_FRAME_DUMP_MAX").map(|n| n as u32),
            skip_ms: env_num("IW4L_FRAME_DUMP_SKIP_MS").unwrap_or(0.0) as f32,
            png: std::env::var("IW4L_FRAME_DUMP_NO_PNG").as_deref() != Ok("1"),
            from,
            state: DumpState::Waiting,
            frame: 0,
            sim_ms: 0.0,
            settled: 0,
            readbacks_owed: Arc::new(AtomicUsize::new(0)),
            last_wall: None,
            frames_csv: None,
            entities_csv: None,
            first_tick: None,
            last_tick: None,
            resolution: (0, 0),
            skeletons: HashMap::new(),
        })
    }

    fn open(&mut self) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        let mut frames =
            std::io::BufWriter::new(std::fs::File::create(self.dir.join("frames.csv"))?);
        writeln!(
            frames,
            "frame,sim_ms,tick,wall_ms,cam_x,cam_y,cam_z,cam_yaw,cam_pitch,actors,items,width,height"
        )?;
        let mut entities =
            std::io::BufWriter::new(std::fs::File::create(self.dir.join("entities.csv"))?);
        writeln!(
            entities,
            "frame,sim_ms,tick,entnum,kind,model,x,y,z,yaw,u,v,u_top,v_top,on_screen,drawn,dist,\
             tree,leaves,weight_sum,move_w,anim_speed,top_clip,top_w,pose,arm_span,bind_span,\
             pose_dev,head_z,ground_gap"
        )?;
        self.frames_csv = Some(frames);
        self.entities_csv = Some(entities);
        Ok(())
    }

    fn write_manifest(&mut self, zone: &str, ended: &str) {
        if let Some(csv) = self.frames_csv.as_mut() {
            let _ = csv.flush();
        }
        if let Some(csv) = self.entities_csv.as_mut() {
            let _ = csv.flush();
        }
        let manifest = serde_json::json!({
            "zone": zone,
            "fps": self.fps,
            "frames": self.frame,
            "frames_written": FRAMES_WRITTEN.load(Ordering::Relaxed),
            "png": self.png,
            "width": self.width,
            "resolution": [self.resolution.0, self.resolution.1],
            "from": format!("{:?}", self.from).to_lowercase(),
            "first_tick": self.first_tick,
            "last_tick": self.last_tick,
            "sim_ms": self.sim_ms,
            "ended": ended,
        });
        if let Ok(text) = serde_json::to_string_pretty(&manifest) {
            let _ = std::fs::write(self.dir.join("capture.json"), text);
        }
    }
}

pub(crate) fn register_frame_dump(app: &mut App) {
    let Some(dump) = FrameDump::from_env() else {
        return;
    };
    let step = Duration::from_secs_f64(1.0 / f64::from(dump.fps));
    diag::info!(
        Launch,
        "frame dump: {} at {} fps (fixed step {:?}, from {:?})",
        dump.dir.display(),
        dump.fps,
        step,
        dump.from
    );
    app.insert_resource(TimeUpdateStrategy::ManualDuration(step))
        .insert_resource(dump)
        .add_systems(Last, frame_dump);
}

#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct DumpFacts<'w> {
    screen: Option<Res<'w, AppScreen>>,
    loading: Option<Res<'w, LoadingScreen>>,
    has_world: Option<Res<'w, HasWorld>>,
    stats: Option<Res<'w, DpvsFrameStats>>,
    working: Option<Res<'w, render_gpu::ColourWorkingSet>>,
    replay: Option<Res<'w, replay::ReplayPlayback>>,
    identity: Option<Res<'w, LaunchIdentity>>,
}

#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct DumpScene<'w, 's> {
    presented: Option<Res<'w, net::PresentedSnapshot>>,
    gfx: Option<Res<'w, HostGfxScene>>,
    catalog: Option<Res<'w, asset_world::MapXModelSceneCatalog>>,
    xanims: Option<Res<'w, assets::PreparedXAnims>>,
    authority: Option<Res<'w, net::AuthorityWorld>>,
    prediction: Option<Res<'w, net::ClientPredictionState>>,
    view: Option<Res<'w, render_scene::PreparedSceneView>>,
    owners: Query<
        'w,
        's,
        (
            &'static WorldScriptModelInstance,
            &'static Transform,
            &'static Visibility,
        ),
    >,
}

fn frame_dump(
    mut commands: Commands,
    mut dump: ResMut<FrameDump>,
    facts: DumpFacts,
    scene: DumpScene,
    time: Res<Time>,
    mut exit: MessageWriter<AppExit>,
) {
    let zone = facts
        .identity
        .as_ref()
        .map_or_else(String::new, |i| i.zone.clone());
    match dump.state {
        DumpState::Done => return,
        DumpState::Draining => {
            if dump.readbacks_owed.load(Ordering::Relaxed) == 0
                && ENCODES_IN_FLIGHT.load(Ordering::Relaxed) == 0
            {
                dump.state = DumpState::Done;
                let ended = if facts.replay.as_ref().is_some_and(|r| r.ended) {
                    "replay_end"
                } else {
                    "max_frames"
                };
                dump.write_manifest(&zone, ended);
                diag::info!(
                    Launch,
                    "frame dump: done, {} frames ({} written) in {}",
                    dump.frame,
                    FRAMES_WRITTEN.load(Ordering::Relaxed),
                    dump.dir.display()
                );
                exit.write(AppExit::Success);
            }
            return;
        }
        _ => {}
    }

    let capture_facts = CaptureFrameFacts {
        screen: facts
            .screen
            .as_deref()
            .copied()
            .unwrap_or(AppScreen::MainMenu),
        loading_overlay: facts.loading.is_some(),
        has_world: facts.has_world.as_ref().is_some_and(|w| w.0),
        submitted_batches: facts.stats.as_ref().map_or(0, |s| s.submitted_batches),
        g0_world: facts
            .stats
            .as_ref()
            .map_or(0, |s| s.g0_world_surfs.len() as u32),
    };
    let pipelines_ready = !capture_facts.has_world
        || facts
            .working
            .as_ref()
            .is_some_and(|set| set.hits > 0 && set.pipeline_not_ready == 0);
    dump.settled = if capture_facts.content_up() && pipelines_ready {
        dump.settled.saturating_add(1)
    } else {
        0
    };
    let source_live = match dump.from {
        DumpFrom::Replay => facts.replay.as_ref().is_some_and(|r| !r.ended),
        DumpFrom::Live => capture_facts.screen == AppScreen::InGame,
    };
    let replay_ended = dump.from == DumpFrom::Replay
        && dump.state == DumpState::Capturing
        && facts.replay.as_ref().is_none_or(|r| r.ended);
    let max_hit = dump.max_frames.is_some_and(|max| dump.frame >= max);
    if replay_ended || max_hit {
        diag::info!(
            Launch,
            "frame dump: capture ended after {} frames",
            dump.frame
        );
        dump.state = DumpState::Draining;
        return;
    }
    let ready = source_live && capture_wait(capture_facts, dump.settled).is_none();
    if dump.state == DumpState::Waiting {
        if !ready {
            return;
        }
        if dump.skip_ms > 0.0 {
            dump.skip_ms -= time.delta_secs() * 1000.0;
            return;
        }
        if let Err(error) = dump.open() {
            diag::error!(Launch, "frame dump: {}: {error}", dump.dir.display());
            dump.state = DumpState::Done;
            return;
        }
        diag::info!(Launch, "frame dump: capturing into {}", dump.dir.display());
        arm_encode_drain_at_exit();
        dump.state = DumpState::Capturing;
    } else if !ready {
        return;
    }

    while ENCODES_IN_FLIGHT.load(Ordering::Relaxed) >= MAX_ENCODES_IN_FLIGHT {
        std::thread::sleep(Duration::from_millis(2));
    }

    let frame = dump.frame;
    dump.frame += 1;
    dump.sim_ms += f64::from(time.delta_secs()) * 1000.0;
    let now = Instant::now();
    let wall_ms = dump
        .last_wall
        .map_or(0.0, |last| now.duration_since(last).as_secs_f64() * 1000.0);
    dump.last_wall = Some(now);

    if dump.png {
        spawn_frame_readback(&mut commands, &dump, frame);
    }
    sample_frame(&mut dump, frame, wall_ms, &scene);
    // A `play` run leaves through `std::process::exit` the moment the demo
    // ends, so what is on disk has to be whole at every frame.
    if let Some(csv) = dump.frames_csv.as_mut() {
        let _ = csv.flush();
    }
    if let Some(csv) = dump.entities_csv.as_mut() {
        let _ = csv.flush();
    }
    if frame.is_multiple_of(30) {
        dump.write_manifest(&zone, "running");
    }
}

#[cfg(unix)]
fn arm_encode_drain_at_exit() {
    use std::sync::atomic::AtomicBool;
    static ARMED: AtomicBool = AtomicBool::new(false);
    if ARMED.swap(true, Ordering::SeqCst) {
        return;
    }
    unsafe extern "C" {
        fn atexit(callback: extern "C" fn()) -> i32;
    }
    extern "C" fn drain() {
        let until = Instant::now() + Duration::from_secs(30);
        while ENCODES_IN_FLIGHT.load(Ordering::Relaxed) > 0 && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    let _ = unsafe { atexit(drain) };
}

#[cfg(not(unix))]
fn arm_encode_drain_at_exit() {}

fn spawn_frame_readback(commands: &mut Commands, dump: &FrameDump, frame: u32) {
    let path = dump.dir.join(format!("{frame:05}.png"));
    let width = dump.width;
    let owed = dump.readbacks_owed.clone();
    owed.fetch_add(1, Ordering::Relaxed);
    commands.spawn(Screenshot::primary_window()).observe(
        move |mut captured: On<ScreenshotCaptured>| {
            let image = std::mem::replace(
                &mut captured.event_mut().image,
                Image::new_uninit(
                    bevy::render::render_resource::Extent3d {
                        width: 1,
                        height: 1,
                        depth_or_array_layers: 1,
                    },
                    bevy::render::render_resource::TextureDimension::D2,
                    bevy::render::render_resource::TextureFormat::Rgba8Unorm,
                    bevy::asset::RenderAssetUsages::empty(),
                ),
            );
            owed.fetch_sub(1, Ordering::Relaxed);
            ENCODES_IN_FLIGHT.fetch_add(1, Ordering::Relaxed);
            let path = path.clone();
            IoTaskPool::get()
                .spawn(async move {
                    if let Err(error) = encode_frame(&path, image, width) {
                        diag::error!(Launch, "frame dump: {}: {error}", path.display());
                    } else {
                        FRAMES_WRITTEN.fetch_add(1, Ordering::Relaxed);
                    }
                    ENCODES_IN_FLIGHT.fetch_sub(1, Ordering::Relaxed);
                })
                .detach();
        },
    );
}

fn encode_frame(path: &std::path::Path, image: Image, width: u32) -> Result<(), String> {
    let rgb = image
        .try_into_dynamic()
        .map_err(|error| error.to_string())?
        .to_rgb8();
    let rgb = if width > 0 && rgb.width() > width {
        let height = (u64::from(rgb.height()) * u64::from(width) / u64::from(rgb.width())) as u32;
        image::imageops::resize(
            &rgb,
            width,
            height.max(1),
            image::imageops::FilterType::Triangle,
        )
    } else {
        rgb
    };
    let file = std::fs::File::create(path).map_err(|error| error.to_string())?;
    let encoder = image::codecs::png::PngEncoder::new_with_quality(
        std::io::BufWriter::new(file),
        image::codecs::png::CompressionType::Fast,
        image::codecs::png::FilterType::Sub,
    );
    image::ImageEncoder::write_image(
        encoder,
        rgb.as_raw(),
        rgb.width(),
        rgb.height(),
        image::ExtendedColorType::Rgb8,
    )
    .map_err(|error| error.to_string())
}

/// The view the scene was actually drawn with (`PreparedSceneView`: the fly
/// host times the lens, which are not a Bevy hierarchy).
struct Projector {
    clip_from_world: Mat4,
    eye: Vec3,
}

impl Projector {
    fn uv(&self, world: Vec3) -> Option<Vec2> {
        let clip = self.clip_from_world * world.extend(1.0);
        if clip.w <= 1.0 {
            return None;
        }
        let ndc = clip.truncate() / clip.w;
        Some(Vec2::new((ndc.x + 1.0) * 0.5, (1.0 - ndc.y) * 0.5))
    }
}

fn csv_name(name: &str) -> String {
    name.replace([',', '"', '\n'], "_")
}

fn sample_frame(dump: &mut FrameDump, frame: u32, wall_ms: f64, scene: &DumpScene) {
    let snapshot = scene.presented.as_ref().and_then(|p| p.snapshot());
    let tick = snapshot.map(|s| s.tick.0);
    if let Some(tick) = tick {
        dump.first_tick.get_or_insert(tick);
        dump.last_tick = Some(tick);
    }
    let projector = scene.view.as_ref().filter(|v| v.ready).map(|view| {
        dump.resolution = (view.rt_w.max(0) as u32, view.rt_h.max(0) as u32);
        Projector {
            clip_from_world: view.clip_from_world,
            eye: view.eye,
        }
    });
    let world = scene
        .authority
        .as_ref()
        .map(|a| &a.0)
        .or_else(|| scene.prediction.as_ref().map(|p| p.0.world()));
    // Shot mask through the bullet path: brushes, terrain, static models and
    // script-model collision (the carrier's own model left out), which is what
    // a body or a dropped weapon visibly rests on.
    let ground_gap = |origin: Vec3, own: Option<sim::ScriptModelId>| -> f32 {
        let Some(world) = world else {
            return f32::NAN;
        };
        let start = origin + Vec3::Z * 16.0;
        let end = origin - Vec3::Z * 512.0;
        let outcome = world.bullet_trace(
            sim::bullet_collision::BulletTraceQuery {
                start: start.to_array(),
                end: end.to_array(),
                mask: sim::bullet_collision::MASK_SHOT,
                ignore: None,
                ignore_hit: None,
                ignore_model: own,
            },
            Some(&[]),
        );
        match outcome {
            sim::bullet_collision::TraceOutcome::Hit { end, .. } => origin.z - end[2],
            _ => f32::NAN,
        }
    };
    let sim_ms = dump.sim_ms;
    let tick_s = tick.map_or(String::new(), |t| t.to_string());
    let mut rows: Vec<String> = Vec::new();
    let mut actors = 0u32;
    let mut items = 0u32;

    for (owner, transform, visibility) in &scene.owners {
        let Some(catalog) = scene.catalog.as_deref() else {
            break;
        };
        let state = &owner.dobj_state;
        let key = state
            .composition
            .models
            .iter()
            .map(|m| format!("{}@{}", m.model, m.attach_tag.as_deref().unwrap_or("")))
            .collect::<Vec<_>>()
            .join("+");
        let skeleton = dump
            .skeletons
            .entry(key)
            .or_insert_with(|| build_skeleton(catalog, state))
            .clone();
        let humanoid = skeleton.as_ref().is_some_and(|s| s.wrists.is_some());
        if !humanoid && state.tree.is_none() {
            continue;
        }
        actors += 1;
        let entnum = owner.gentity_number.map(u32::from);
        let origin = transform.translation;
        let fwd = transform.rotation * Vec3::X;
        let yaw = fwd.y.atan2(fwd.x).to_degrees();
        let drawn = *visibility != Visibility::Hidden
            && entnum.is_none_or(|e| {
                scene
                    .gfx
                    .as_ref()
                    .is_none_or(|g| !g.scene.scene_ent_skips_draw(e))
            });

        let anim = anim_summary(state, scene.xanims.as_deref());
        let mut pose_label = "none";
        let (mut arm_span, mut bind_span, mut pose_dev, mut head_z) =
            (f32::NAN, f32::NAN, f32::NAN, f32::NAN);
        let mut top_world = origin + Vec3::Z * 72.0;
        if let Some(skeleton) = skeleton.as_ref() {
            let posed = state
                .resolve_request(|name| {
                    scene
                        .xanims
                        .as_ref()?
                        .0
                        .clip(asset_core::AssetNamespace::Iw4, name)
                })
                .map_err(|_| "resolve_err")
                .and_then(|request| {
                    xmodel_runtime::pose_dobj(&skeleton.dobj, &request, Mat4::IDENTITY)
                        .map_err(|_| "pose_err")
                });
            match posed {
                Ok(world) => {
                    pose_label = if state.tree.is_some() { "tree" } else { "bind" };
                    let points: Vec<Vec3> = world.iter().map(|m| m.w_axis.truncate()).collect();
                    let n = points.len().min(skeleton.bind.len()).max(1);
                    pose_dev = points
                        .iter()
                        .zip(&skeleton.bind)
                        .map(|(a, b)| a.distance(*b))
                        .sum::<f32>()
                        / n as f32;
                    if let Some((le, ri)) = skeleton.wrists {
                        arm_span = points[le].distance(points[ri]);
                        bind_span = skeleton.bind[le].distance(skeleton.bind[ri]);
                    }
                    if let Some(head) = skeleton.head {
                        head_z = points[head].z;
                        top_world = transform.transform_point(points[head] + Vec3::Z * 6.0);
                    }
                }
                Err(label) => pose_label = label,
            }
        }
        let uv = projector.as_ref().and_then(|p| p.uv(origin));
        let uv_top = projector.as_ref().and_then(|p| p.uv(top_world));
        let on_screen = uv.zip(uv_top).is_some_and(|(a, b)| {
            (0.0..=1.0).contains(&a.x) && b.y <= 1.0 && a.y >= 0.0 && (0.0..=1.0).contains(&b.x)
        });
        let dist = projector
            .as_ref()
            .map_or(f32::NAN, |p| p.eye.distance(origin));
        let model = state
            .composition
            .models
            .first()
            .map_or(String::new(), |m| csv_name(&m.model));
        let kind = if humanoid { "actor" } else { "animated" };
        rows.push(format!(
            "{frame},{sim_ms:.1},{tick_s},{},{kind},{model},{:.2},{:.2},{:.2},{yaw:.2},{},{},{},{},{},{},{dist:.1},{},{},{:.3},{:.3},{:.1},{},{:.3},{pose_label},{arm_span:.2},{bind_span:.2},{pose_dev:.2},{head_z:.2},{:.2}",
            entnum.map_or(String::new(), |e| e.to_string()),
            origin.x,
            origin.y,
            origin.z,
            uv.map_or(String::new(), |v| format!("{:.4}", v.x)),
            uv.map_or(String::new(), |v| format!("{:.4}", v.y)),
            uv_top.map_or(String::new(), |v| format!("{:.4}", v.x)),
            uv_top.map_or(String::new(), |v| format!("{:.4}", v.y)),
            u8::from(on_screen),
            u8::from(drawn),
            u8::from(state.tree.is_some()),
            anim.leaves,
            anim.weight_sum,
            anim.move_w,
            anim.anim_speed,
            csv_name(&anim.top_clip),
            anim.top_w,
            ground_gap(origin, owner.authority_owner.and_then(|o| o.script_model())),
        ));
    }

    if let Some(snapshot) = snapshot {
        for es in snapshot
            .meta
            .entities
            .iter()
            .filter(|es| es.e_type == entity_iw4::ET_ITEM && es.e_flags & 0x20 == 0)
        {
            items += 1;
            let entnum = u32::try_from(es.number).unwrap_or(0);
            let presented = scene.gfx.as_ref().and_then(|g| {
                g.scene
                    .scene_dobj(entnum)
                    .map(|d| d.origin)
                    .or_else(|| g.scene.scene_model(entnum).map(|m| m.origin))
            });
            let drawn = presented.is_some();
            let origin = Vec3::from_array(presented.unwrap_or(es.tr_base));
            let uv = projector.as_ref().and_then(|p| p.uv(origin));
            let uv_top = projector
                .as_ref()
                .and_then(|p| p.uv(origin + Vec3::Z * 12.0));
            let on_screen =
                uv.is_some_and(|a| (0.0..=1.0).contains(&a.x) && (0.0..=1.0).contains(&a.y));
            let dist = projector
                .as_ref()
                .map_or(f32::NAN, |p| p.eye.distance(origin));
            rows.push(format!(
                "{frame},{sim_ms:.1},{tick_s},{entnum},item,weapon{},{:.2},{:.2},{:.2},{:.2},{},{},{},{},{},{},{dist:.1},0,0,0,0,0,,0,none,,,,,{:.2}",
                es.index,
                origin.x,
                origin.y,
                origin.z,
                es.apos_tr_base[1],
                uv.map_or(String::new(), |v| format!("{:.4}", v.x)),
                uv.map_or(String::new(), |v| format!("{:.4}", v.y)),
                uv_top.map_or(String::new(), |v| format!("{:.4}", v.x)),
                uv_top.map_or(String::new(), |v| format!("{:.4}", v.y)),
                u8::from(on_screen),
                u8::from(drawn),
                ground_gap(origin, None),
            ));
        }
    }

    let (cam, cam_yaw, cam_pitch) = scene.view.as_ref().filter(|v| v.ready).map_or(
        (Vec3::splat(f32::NAN), f32::NAN, f32::NAN),
        |view| {
            let fwd = view.forward;
            (
                view.eye,
                fwd.y.atan2(fwd.x).to_degrees(),
                (-fwd.z).asin().to_degrees(),
            )
        },
    );
    let (w, h) = dump.resolution;
    if let Some(csv) = dump.frames_csv.as_mut() {
        let _ = writeln!(
            csv,
            "{frame},{sim_ms:.1},{tick_s},{wall_ms:.2},{:.2},{:.2},{:.2},{cam_yaw:.2},{cam_pitch:.2},{actors},{items},{w},{h}",
            cam.x, cam.y, cam.z
        );
    }
    if let Some(csv) = dump.entities_csv.as_mut() {
        for row in rows {
            let _ = writeln!(csv, "{row}");
        }
    }
}

fn build_skeleton(
    catalog: &asset_world::MapXModelSceneCatalog,
    state: &xmodel_runtime::DObjSemanticState,
) -> Option<Arc<Skeleton>> {
    let (specs, _) =
        render_anim::occupancy::script_model::collect_presented_models(catalog, state)?;
    let dobj = xmodel_runtime::DObj::build(&specs).ok()?;
    let bind = xmodel_runtime::pose_dobj(
        &dobj,
        &xmodel_runtime::DObjPoseRequest::bind_pose(),
        Mat4::IDENTITY,
    )
    .ok()?
    .iter()
    .map(|m| m.w_axis.truncate())
    .collect();
    let wrists = dobj.find("j_wrist_le").zip(dobj.find("j_wrist_ri"));
    let head = dobj.find("j_head");
    Some(Arc::new(Skeleton {
        dobj,
        bind,
        wrists,
        head,
    }))
}

#[derive(Default)]
struct AnimSummary {
    leaves: u32,
    weight_sum: f32,
    move_w: f32,
    anim_speed: f32,
    top_clip: String,
    top_w: f32,
}

/// Effective leaf weights (product up the parent chain, additive subtrees
/// left out), the share on clips that carry root motion, and the
/// weight-averaged root speed those clips would move the body at.
fn anim_summary(
    state: &xmodel_runtime::DObjSemanticState,
    xanims: Option<&assets::PreparedXAnims>,
) -> AnimSummary {
    let mut out = AnimSummary::default();
    let Some(tree) = state.tree.as_ref() else {
        return out;
    };
    let nodes = &tree.nodes;
    let mut speed_sum = 0.0;
    for node in nodes {
        if node.kind != xmodel_runtime::XAnimSemanticNodeKind::Leaf {
            continue;
        }
        let mut weight = node.state.weight;
        let mut parent = node.parent;
        let mut additive = false;
        let mut guard = 0;
        while let Some(p) = parent {
            let Some(up) = nodes.get(usize::from(p.0)) else {
                break;
            };
            if up.kind == xmodel_runtime::XAnimSemanticNodeKind::Additive {
                additive = true;
            }
            weight *= up.state.weight;
            parent = up.parent;
            guard += 1;
            if guard > 64 {
                break;
            }
        }
        if additive || weight <= 0.01 {
            continue;
        }
        out.leaves += 1;
        out.weight_sum += weight;
        let clip_name = node.clip.as_deref().unwrap_or("");
        if weight > out.top_w {
            out.top_w = weight;
            out.top_clip = clip_name.to_owned();
        }
        let speed = xanims
            .and_then(|x| x.0.clip(asset_core::AssetNamespace::Iw4, clip_name))
            .map_or(0.0, |clip| clip.move_speed() * node.state.rate.abs());
        if speed > MOVE_CLIP_SPEED {
            out.move_w += weight;
            speed_sum += speed * weight;
        }
    }
    if out.move_w > 0.0 {
        out.anim_speed = speed_sum / out.move_w;
    }
    out
}
