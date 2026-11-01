//! Collapsed sidebar nodes, kept in `<config dir>/collapsed.toml` (PLAN.md section 17) as
//! `collapsed = ["<instance>", "<instance>/space/<id>", "<instance>/category/<id>",
//! "<instance>/dms"]`.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use hermes_client_core::InstanceId;
use hermes_client_core::proto::id::Id;
use serde::{Deserialize, Serialize};

pub const FILE: &str = "collapsed.toml";

/// A node of the sidebar tree that can be collapsed.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Node {
    Instance(InstanceId),
    Space(InstanceId, Id),
    Category(InstanceId, Id),
    Dms(InstanceId),
}

impl Node {
    pub fn key(&self) -> String {
        match self {
            Node::Instance(inst) => inst.clone(),
            Node::Space(inst, id) => format!("{inst}/space/{id}"),
            Node::Category(inst, id) => format!("{inst}/category/{id}"),
            Node::Dms(inst) => format!("{inst}/dms"),
        }
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct File {
    #[serde(default)]
    collapsed: BTreeSet<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Collapsed {
    keys: BTreeSet<String>,
    /// `None`: kept in memory only (tests).
    path: Option<PathBuf>,
}

impl Collapsed {
    /// Loads `<dir>/collapsed.toml`; a missing or broken file means nothing is collapsed.
    pub fn load(dir: &Path) -> Collapsed {
        let path = dir.join(FILE);
        let keys = match std::fs::read_to_string(&path) {
            Ok(text) => match toml::from_str::<File>(&text) {
                Ok(file) => file.collapsed,
                Err(err) => {
                    tracing::warn!("{}: {err}", path.display());
                    BTreeSet::new()
                }
            },
            Err(_) => BTreeSet::new(),
        };
        Collapsed {
            keys,
            path: Some(path),
        }
    }

    pub fn is_collapsed(&self, node: &Node) -> bool {
        self.keys.contains(&node.key())
    }

    /// Flips `node` and saves the file.
    pub fn toggle(&mut self, node: &Node) -> std::io::Result<()> {
        let key = node.key();
        if !self.keys.remove(&key) {
            self.keys.insert(key);
        }
        self.save()
    }

    fn save(&self) -> std::io::Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let text = toml::to_string(&File {
            collapsed: self.keys.clone(),
        })
        .map_err(std::io::Error::other)?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toggles_are_saved_and_loaded() {
        let dir = std::env::temp_dir().join(format!("hermes-gui-{}-collapsed", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let space = Node::Space("h".into(), Id(5));
        let dms = Node::Dms("h".into());

        let mut collapsed = Collapsed::load(&dir);
        assert!(!collapsed.is_collapsed(&space));
        collapsed.toggle(&space).unwrap();
        collapsed.toggle(&dms).unwrap();
        collapsed.toggle(&dms).unwrap();

        let text = std::fs::read_to_string(dir.join(FILE)).unwrap();
        assert_eq!(text.trim(), r#"collapsed = ["h/space/5"]"#);
        let loaded = Collapsed::load(&dir);
        assert!(loaded.is_collapsed(&space));
        assert!(!loaded.is_collapsed(&dms));

        std::fs::write(dir.join(FILE), "broken [").unwrap();
        assert!(!Collapsed::load(&dir).is_collapsed(&space));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
