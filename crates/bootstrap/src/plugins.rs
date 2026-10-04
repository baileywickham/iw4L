use assets::AssetPlugin;
use audio::AudioPlugin;
use bevy::{
    log::LogPlugin,
    prelude::*,
    render::{
        RenderPlugin as BevyRenderPlugin,
        pipelined_rendering::PipelinedRenderingPlugin,
        settings::{RenderCreation, WgpuFeatures, WgpuSettings},
    },
};
use bots::BotsPlugin;
use console::ConsolePlugin;
use frame::RuntimeRole;
use hud::HudPlugin;
use net::NetPlugin;
use render::RenderPlugin;
use replay::ReplayPlugin;
use session::SessionPlugin;
use ui::UiPlugin;

pub fn add_runtime_plugins(app: &mut App) {
    add_runtime_plugins_with_role(app, RuntimeRole::Listen);
}

pub fn add_runtime_plugins_with_role(app: &mut App, role: RuntimeRole) {
    let net = match role {
        RuntimeRole::Listen => NetPlugin::listen(),
        RuntimeRole::Dedicated => NetPlugin::dedicated(),
        RuntimeRole::Client => NetPlugin::client(),
        RuntimeRole::Replay => NetPlugin::replay(),
    };
    if crate::bench::enabled() {
        // Unfocused benchmarks must not inherit the window runner's 60 Hz sleep.
        app.insert_resource(bevy::winit::WinitSettings::continuous());
    }
    app.insert_resource(audio::AudioRuntime::new(
        role != RuntimeRole::Dedicated && !audio::AudioSilent::active(),
    ));
    app.add_plugins(AssetPlugin)
        .add_plugins(UiPlugin)
        .add_plugins(ConsolePlugin)
        .add_plugins(net)
        .add_plugins((BotsPlugin, HudPlugin))
        .add_plugins(AudioPlugin)
        .add_plugins(ReplayPlugin)
        .add_plugins(RenderPlugin)
        .add_plugins(SessionPlugin);

    app.edit_schedule(Update, |schedule| {
        schedule.set_executor(bevy::ecs::schedule::SingleThreadedExecutor::new());
    });

    app.edit_schedule(First, |schedule| {
        schedule.set_executor(bevy::ecs::schedule::SingleThreadedExecutor::new());
    });
    app.edit_schedule(PreUpdate, |schedule| {
        schedule.set_executor(bevy::ecs::schedule::SingleThreadedExecutor::new());
    });
    app.edit_schedule(PostUpdate, |schedule| {
        schedule.set_executor(bevy::ecs::schedule::SingleThreadedExecutor::new());
    });
    app.edit_schedule(Last, |schedule| {
        schedule.set_executor(bevy::ecs::schedule::SingleThreadedExecutor::new());
    });

    if let Some(render_app) = app.get_sub_app_mut(bevy::render::RenderApp) {
        render_app.add_systems(
            bevy::render::Render,
            report_unpresented_window
                .after(bevy::render::view::prepare_windows)
                .in_set(bevy::render::RenderSystems::PrepareViews),
        );
        render_app.edit_schedule(bevy::render::renderer::RenderGraph, |schedule| {
            schedule.set_executor(bevy::ecs::schedule::SingleThreadedExecutor::new());
        });
        render_app.edit_schedule(bevy::core_pipeline::Core3d, |schedule| {
            schedule.set_executor(bevy::ecs::schedule::SingleThreadedExecutor::new());
        });
        render_app.edit_schedule(bevy::core_pipeline::Core2d, |schedule| {
            schedule.set_executor(bevy::ecs::schedule::SingleThreadedExecutor::new());
        });
        render_app.edit_schedule(bevy::render::Render, |schedule| {
            schedule.set_executor(bevy::ecs::schedule::SingleThreadedExecutor::new());
        });
        render_app.edit_schedule(bevy::render::ExtractSchedule, |schedule| {
            schedule.set_executor(bevy::ecs::schedule::SingleThreadedExecutor::new());
        });
        if cfg!(target_os = "macos") {
            // AppKit lets only the main thread create the Metal layer behind
            // the NSView. The single-threaded `Render` schedule would run
            // bevy's `create_surfaces` on the render thread when rendering is
            // pipelined, so the surface is created (and configured) here, in
            // extraction, which always runs on the main thread. `Render`'s own
            // copy then finds it configured and only reconfigures on a resize.
            render_app.add_systems(
                bevy::render::ExtractSchedule,
                bevy::render::view::create_surfaces
                    .run_if(bevy::render::view::need_surface_configuration)
                    .after(bevy::render::camera::extract_cameras),
            );
        }
    }
}

/// On macOS, wgpu's Metal surface hands out no drawable while the window is
/// occluded (fully covered, on another Space, minimized, or the screen locked):
/// Bevy's `prepare_windows` swallows `CurrentSurfaceTexture::Occluded`, the
/// window camera gets no `ViewTarget`, and no view node records. The world
/// spawn's GPU gate needs a recorded colour pass (`working_hits > 0`), so it
/// waits with nothing else in the log to say why. A window already occluded at
/// creation sends no `WindowOccluded`, hence the check on the surface itself.
fn report_unpresented_window(
    windows: Res<bevy::render::view::ExtractedWindows>,
    mut unpresented: Local<bool>,
) {
    let Some(window) = windows.primary.and_then(|entity| windows.get(&entity)) else {
        return;
    };
    let now = window.swap_chain_texture_view.is_none();
    if now == *unpresented {
        return;
    }
    *unpresented = now;
    if now {
        diag::warn!(
            Launch,
            "window surface: no frame to draw (macOS: window occluded or screen locked) — nothing renders, and world spawn's GPU gate waits until the window is visible"
        );
    } else {
        diag::info!(Launch, "window surface: presenting again");
    }
}

pub fn assemble_listen_app() -> App {
    let mut app = App::new();
    app.add_plugins(default_plugins_with_quiet_log(WindowPlugin {
        primary_window: None,
        ..default()
    }));
    add_runtime_plugins(&mut app);
    app
}

/// `IW4L_WINDOW_POS=x,y` places the window in physical pixels (two clients side by side).
fn window_position() -> Option<IVec2> {
    let text = std::env::var("IW4L_WINDOW_POS").ok()?;
    let (x, y) = text.split_once(',')?;
    Some(IVec2::new(x.trim().parse().ok()?, y.trim().parse().ok()?))
}

pub fn default_plugins_with_quiet_log(mut window: WindowPlugin) -> bevy::app::PluginGroupBuilder {
    if let Some(primary) = window.primary_window.as_mut() {
        primary.desired_maximum_frame_latency = core::num::NonZeroU32::new(frame_latency());
        if std::env::var("IW4L_WINDOW_ON_TOP").is_ok_and(|v| v == "1") {
            primary.window_level = bevy::window::WindowLevel::AlwaysOnTop;
        }
        if let Some(at) = window_position() {
            primary.position = bevy::window::WindowPosition::At(at);
        }
    }
    let mut wgpu = WgpuSettings::default();
    wgpu.features |= WgpuFeatures::TEXTURE_FORMAT_16BIT_NORM
        | WgpuFeatures::TEXTURE_COMPRESSION_BC
        | WgpuFeatures::POLYGON_MODE_LINE
        | WgpuFeatures::TEXTURE_BINDING_ARRAY
        | WgpuFeatures::SAMPLED_TEXTURE_AND_STORAGE_BUFFER_ARRAY_NON_UNIFORM_INDEXING
        | WgpuFeatures::PARTIALLY_BOUND_BINDING_ARRAY;
    let plugins = DefaultPlugins
        .set(window)
        .set(LogPlugin {
            filter: "warn,iw4l=info".into(),
            level: bevy::log::Level::WARN,
            ..default()
        })
        .set(BevyRenderPlugin {
            render_creation: RenderCreation::Automatic(Box::new(wgpu)),
            ..default()
        });
    if pipelined_rendering() {
        plugins
    } else {
        plugins.disable::<PipelinedRenderingPlugin>()
    }
}

const PIPELINED_RENDERING_ENV: &str = "IW4L_PIPELINED_RENDERING";

/// Overlap rendering with the next main frame. Extraction remains the ownership
/// boundary; the bounded render channel permits one outstanding frame.
/// Set IW4L_PIPELINED_RENDERING=0 for synchronous presentation.
///
/// On macOS the window surface is created during extraction (main thread,
/// see `add_runtime_plugins_with_role`): AppKit only lets the main thread
/// touch the NSView, and the single-threaded `Render` schedule would otherwise
/// create it on the render thread and panic in `raw-window-metal`.
fn pipelined_rendering() -> bool {
    match std::env::var_os(PIPELINED_RENDERING_ENV) {
        None => true,
        Some(_) => perf::switch(PIPELINED_RENDERING_ENV),
    }
}

const FRAME_LATENCY_ENV: &str = "IW4L_FRAME_LATENCY";

/// How many frames the surface is asked to let the CPU run ahead of the GPU.
///
/// wgpu calls this a hint and the backend is free to clamp it: on Vulkan it is
/// bound to the number of swapchain images, so a run that asked for two did
/// not necessarily get two, and only a measurement says which. One is the
/// default because it is what the runtime shipped; the variable exists so the
/// other arm needs no rebuild, and the manifest records the number that was
/// asked for — never the number the driver granted, which this process cannot
/// read back.
///
/// The value is a count, not a switch: anything unparseable or zero is the
/// default, and says so rather than silently picking an arm. Read once, so the
/// window and the manifest cannot disagree and the complaint is made once.
pub(crate) fn frame_latency() -> u32 {
    const DEFAULT: u32 = 1;
    static FRAMES: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    *FRAMES.get_or_init(|| {
        let Some(asked) = std::env::var_os(FRAME_LATENCY_ENV) else {
            return DEFAULT;
        };
        match asked.to_str().map(str::trim).and_then(|v| v.parse().ok()) {
            Some(frames) if frames > 0 => frames,
            _ => {
                diag::warn!(
                    Launch,
                    "{FRAME_LATENCY_ENV}={} is not a frame count; using {DEFAULT}",
                    asked.to_string_lossy(),
                );
                DEFAULT
            }
        }
    })
}
