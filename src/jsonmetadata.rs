use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::Path;

type JsonMap = HashMap<String, serde_json::Value>;

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct JsonMetadata {
    pub parent: String,
    pub visible_name: String,

    #[serde(flatten)]
    extra: JsonMap,
}

impl JsonMetadata {
    pub fn new(visible_name: &str, parent: &str) -> JsonMetadata {
        JsonMetadata {
            parent: parent.to_string(),
            visible_name: visible_name.to_string(),
            extra: JsonMap::new(),
        }
    }

    pub fn from_file<P: AsRef<Path>>(path: P) -> io::Result<JsonMetadata>{
        Ok(serde_json::from_str(&fs::read_to_string(&path)?)?)
    }

    pub fn save_file<P: AsRef<Path>>(&self, path: P) -> io::Result<()>{
        fs::write(path, serde_json::to_vec(&self)?)
    }
}
