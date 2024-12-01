use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
pub struct Config {
    pub path: Vec<String>,
    pub verbose_builds: bool,
    pub strip: bool,
    pub su_cmd: Option<String>,
    pub cache_dir: Option<String>,
}
