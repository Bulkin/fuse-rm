use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::Path;

type JsonMap = HashMap<String, serde_json::Value>;

#[derive(Serialize, Deserialize, Debug, Clone)]
enum DocType {
    CollectionType,
    DocumentType,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct JsonMetadata {
    pub parent: String,
    pub visible_name: String,
    r#type: DocType,

    #[serde(flatten)]
    extra: JsonMap,
}

impl JsonMetadata {
    fn new(visible_name: &str,
           parent: &str,
           doctype: DocType) -> JsonMetadata {
        JsonMetadata {
            parent: parent.to_string(),
            visible_name: visible_name.to_string(),
            r#type: doctype,
            extra: JsonMap::new(),
        }
    }

    pub fn new_file(visible_name: &str, parent: &str) -> JsonMetadata {
        JsonMetadata::new(visible_name, parent, DocType::DocumentType)
    }

    pub fn new_dir(visible_name: &str, parent: &str) -> JsonMetadata {
        JsonMetadata::new(visible_name, parent, DocType::CollectionType)
    }

    pub fn from_file<P: AsRef<Path>>(path: P) -> io::Result<JsonMetadata>{
        Ok(serde_json::from_str(&fs::read_to_string(&path)?)?)
    }

    pub fn save_file<P: AsRef<Path>>(&self, path: P) -> io::Result<()>{
        fs::write(path, serde_json::to_vec(&self)?)
    }
}
