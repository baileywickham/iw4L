use crate::MenuMapList;

pub fn map_label(map: &str) -> String {
    let stem = map.split_once(':').map_or(map, |(_, name)| name);
    // A Spec Ops zone shows its mission name ("BODY COUNT"), not the zone stem.
    if let Some(mission) = super::specops::SO_TIERS
        .iter()
        .flat_map(|tier| tier.missions)
        .find(|mission| mission.zone.eq_ignore_ascii_case(stem))
    {
        return mission.name.to_uppercase();
    }
    stem.trim_start_matches("mp_")
        .replace('_', " ")
        .to_uppercase()
}

pub fn map_preview(map: &str) -> String {
    match map.split_once(':').unwrap_or(("iw4", map)) {
        (_, "") => String::new(),
        ("t5", stem) => format!(
            "t5:material/menu_mp_map_select_{}_big",
            stem.trim_start_matches("mp_")
        ),
        (namespace, stem) => format!("{namespace}:material/preview_{stem}"),
    }
}

pub fn pack_maps(maps: &MenuMapList, pack: usize) -> &[String] {
    maps.0.get(pack).map_or(&[], |pack| pack.maps.as_slice())
}
