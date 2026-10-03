use bevy_ecs::prelude::Resource;

#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LocalPlayerProfile {
    pub percent_complete_sp: u8,
    pub percent_complete_mp: u8,
    pub percent_complete_so: u8,
}

impl LocalPlayerProfile {
    pub const FIELDS: [&str; 3] = [
        "percentcompletesp",
        "percentcompletemp",
        "percentcompleteso",
    ];

    pub fn get(&self, name: &str) -> Option<u8> {
        if name.eq_ignore_ascii_case(Self::FIELDS[0]) {
            Some(self.percent_complete_sp)
        } else if name.eq_ignore_ascii_case(Self::FIELDS[1]) {
            Some(self.percent_complete_mp)
        } else if name.eq_ignore_ascii_case(Self::FIELDS[2]) {
            Some(self.percent_complete_so)
        } else {
            None
        }
    }

    pub fn set(&mut self, name: &str, value: u8) -> bool {
        let slot = if name.eq_ignore_ascii_case(Self::FIELDS[0]) {
            &mut self.percent_complete_sp
        } else if name.eq_ignore_ascii_case(Self::FIELDS[1]) {
            &mut self.percent_complete_mp
        } else if name.eq_ignore_ascii_case(Self::FIELDS[2]) {
            &mut self.percent_complete_so
        } else {
            return false;
        };
        *slot = value;
        true
    }
}

/// Single-player profile fields (`missionSOHighestDifficulty`, best times `s0`…`s22`, …) keyed
/// by lowercased name; values keep their script text form.
#[derive(Resource, Clone, Debug, Default, PartialEq, Eq)]
pub struct SpProfile(pub std::collections::BTreeMap<String, String>);

impl SpProfile {
    /// Fields holding one digit per level; they stay strings in script.
    pub const DIGIT_FIELDS: [&str; 2] = ["missionsohighestdifficulty", "missionhighestdifficulty"];

    pub fn get(&self, name: &str) -> Option<&str> {
        self.0.get(&name.to_ascii_lowercase()).map(String::as_str)
    }

    pub fn set(&mut self, name: &str, value: impl Into<String>) {
        self.0.insert(name.to_ascii_lowercase(), value.into());
    }

    /// Stars of the Spec Ops level at `index`: its digit minus one, as `_endmission` counts.
    pub fn so_stars(&self, index: usize) -> u8 {
        self.get(Self::DIGIT_FIELDS[0])
            .and_then(|digits| digits.as_bytes().get(index).copied())
            .filter(u8::is_ascii_digit)
            .map_or(0, |digit| (digit - b'0').saturating_sub(1))
    }

    /// Best time in milliseconds of the Spec Ops level at `index` (`sp/specOpsTable.csv`
    /// column 9 names the field `s<index>`).
    pub fn so_best_time(&self, index: usize) -> Option<u32> {
        self.get(&format!("s{index}"))
            .and_then(|text| text.trim().parse::<u32>().ok())
            .filter(|ms| *ms > 0)
    }
}
