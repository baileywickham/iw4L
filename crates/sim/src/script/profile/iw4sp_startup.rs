pub struct Iw4SpStartup {
    pub roots: Vec<String>,
    pub entries: Vec<String>,
}
impl Iw4SpStartup {
    pub fn new(map: &str) -> Self {
        let map = format!("maps/{map}");
        Self {
            roots: vec![
                "codescripts/delete".to_owned(),
                "codescripts/struct".to_owned(),
                map.clone(),
            ],
            entries: vec![format!("{map}::main")],
        }
    }
}
