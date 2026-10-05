/// Animscripts the engine starts on actors itself (`animscripts/%s`), the
/// cover states included: nothing in script references them, so without a
/// root an actor at a cover node runs no animscript and stands in the bind pose.
const ENGINE_ANIMSCRIPTS: [&str; 16] = [
    "animscripts/init",
    "animscripts/cover_arrival",
    "animscripts/cover_crouch",
    "animscripts/cover_left",
    "animscripts/cover_prone",
    "animscripts/cover_right",
    "animscripts/cover_stand",
    "animscripts/stop",
    "animscripts/combat",
    "animscripts/move",
    "animscripts/death",
    "animscripts/pain",
    "animscripts/flashed",
    "animscripts/grenade_cower",
    "animscripts/grenade_return_throw",
    "animscripts/scripted",
];

pub struct Iw4SpStartup {
    pub roots: Vec<String>,
    pub entries: Vec<String>,
}
impl Iw4SpStartup {
    /// `modules` is every script the zones carry; the engine reaches the aitypes,
    /// the entry animscripts and the traverse scripts by name, so they are roots
    /// when present.
    pub fn new<'a>(map: &str, modules: impl Iterator<Item = &'a str>) -> Self {
        let map = format!("maps/{map}");
        let mut roots = vec![
            "codescripts/delete".to_owned(),
            "codescripts/struct".to_owned(),
            map.clone(),
        ];
        for module in modules {
            if module.starts_with("aitype/")
                || module.starts_with("animscripts/dog/")
                || module.starts_with("animscripts/traverse/")
                || ENGINE_ANIMSCRIPTS.contains(&module)
            {
                roots.push(module.to_owned());
            }
        }
        Self {
            roots,
            entries: vec![format!("{map}::main")],
        }
    }
}
