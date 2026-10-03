use std::{fs, path::PathBuf};

use bevy::prelude::*;
use sim::{LocalPlayerProfile, SpProfile};

/// Spec Ops / single-player profile fields live beside `profile.cfg`.
const SP_PROFILE_FILE: &str = "specops.cfg";

#[derive(Resource, Default)]
pub(crate) struct ProfilePersistence {
    path: Option<PathBuf>,
    saved: LocalPlayerProfile,
    sp_path: Option<PathBuf>,
    sp_saved: SpProfile,
}

pub(crate) fn load(
    identity: Res<ui::LaunchIdentity>,
    role: Res<frame::RuntimeRole>,
    mut profile: ResMut<LocalPlayerProfile>,
    mut sp_profile: ResMut<SpProfile>,
    authority: Option<ResMut<net::AuthorityWorld>>,
    mut persistence: ResMut<ProfilePersistence>,
) {
    if *role != frame::RuntimeRole::Listen {
        return;
    }
    let path = std::env::var_os("IW4L_PROFILE_PATH")
        .map(PathBuf::from)
        .or_else(|| {
            crate::user_settings::settings_path(&identity.artifacts)
                .map(|path| path.with_file_name("profile.cfg"))
        });
    if let Some(path) = path {
        match fs::read_to_string(&path) {
            Ok(source) => match parse(&source) {
                Ok(loaded) => *profile = loaded,
                Err(error) => warn!("could not parse {}: {error}", path.display()),
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => warn!("could not read {}: {error}", path.display()),
        }
        let sp_path = path.with_file_name(SP_PROFILE_FILE);
        match fs::read_to_string(&sp_path) {
            Ok(source) => *sp_profile = parse_sp(&source),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => warn!("could not read {}: {error}", sp_path.display()),
        }
        diag::info!(
            Console,
            "profile: {} spec ops fields from {}",
            sp_profile.0.len(),
            sp_path.display()
        );
        persistence.sp_path = Some(sp_path);
        persistence.path = Some(path);
    } else {
        warn!("no HOME or XDG_CONFIG_HOME; local player profile is session-only");
    }
    persistence.saved = *profile;
    persistence.sp_saved = sp_profile.clone();
    if let Some(mut authority) = authority {
        authority.0.set_local_player_profile(*profile);
        authority.0.set_sp_profile(sp_profile.clone());
    }
}

pub(crate) fn save(
    role: Res<frame::RuntimeRole>,
    authority: Option<Res<net::AuthorityWorld>>,
    mut profile: ResMut<LocalPlayerProfile>,
    mut sp_profile: ResMut<SpProfile>,
    mut persistence: ResMut<ProfilePersistence>,
) {
    if *role != frame::RuntimeRole::Listen {
        return;
    }
    if let Some(authority) = authority {
        let current = authority.0.local_player_profile();
        if *profile != current {
            *profile = current;
        }
        let current = authority.0.sp_profile();
        if *sp_profile != *current {
            *sp_profile = current.clone();
        }
    }
    if *sp_profile != persistence.sp_saved
        && let Some(path) = persistence.sp_path.as_ref()
    {
        match write_atomic(path, &serialize_sp(&sp_profile)) {
            Ok(()) => {
                diag::info!(Console, "profile: saved {}", path.display());
                persistence.sp_saved = sp_profile.clone();
            }
            Err(error) => warn!("could not save {}: {error}", path.display()),
        }
    }
    if *profile == persistence.saved {
        return;
    }
    let Some(path) = persistence.path.as_ref() else {
        return;
    };
    let result = write_atomic(path, &serialize(&profile));
    match result {
        Ok(()) => persistence.saved = *profile,
        Err(error) => warn!("could not save {}: {error}", path.display()),
    }
}

fn parse(source: &str) -> Result<LocalPlayerProfile, String> {
    let mut profile = LocalPlayerProfile::default();
    for line in source.lines().map(str::trim) {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (name, value) = line
            .split_once('=')
            .ok_or_else(|| "expected a profile field=value line".to_owned())?;
        let value = value
            .trim()
            .parse::<u8>()
            .map_err(|_| format!("invalid byte for {}", name.trim()))?;
        if !profile.set(name.trim(), value) {
            return Err(format!("unknown profile field {}", name.trim()));
        }
    }
    Ok(profile)
}

fn serialize(profile: &LocalPlayerProfile) -> String {
    let mut source = String::from("# iw4l local player profile v1\n");
    for name in LocalPlayerProfile::FIELDS {
        source.push_str(&format!("{name}={}\n", profile.get(name).unwrap()));
    }
    source
}

fn parse_sp(source: &str) -> SpProfile {
    let mut profile = SpProfile::default();
    for line in source.lines().map(str::trim) {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((name, value)) = line.split_once('=') {
            profile.set(name.trim(), value.trim());
        }
    }
    profile
}

fn serialize_sp(profile: &SpProfile) -> String {
    let mut source = String::from("# iw4l spec ops profile v1\n");
    for (name, value) in &profile.0 {
        let value: String = value.chars().filter(|ch| !ch.is_control()).collect();
        source.push_str(&format!("{name}={value}\n"));
    }
    source
}

fn write_atomic(path: &std::path::Path, contents: &str) -> std::io::Result<()> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension("cfg.tmp");
    fs::write(&temporary, contents)?;
    fs::rename(temporary, path)
}
