use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use serde::{Deserialize, Deserializer, Serialize};

use super::SessionConfig;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Folder {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub parent_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum HostNodeId {
    Folder(String),
    Host(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ImportOrigin {
    pub source_path: PathBuf,
    pub alias: String,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct HostLibrary {
    pub folders: Vec<Folder>,
    pub sessions: Vec<SessionConfig>,
}

#[derive(Deserialize)]
struct StoredSession {
    #[serde(default)]
    group: String,
    #[serde(flatten)]
    profile: SessionConfig,
}

#[derive(Default, Deserialize)]
struct StoredLibrary {
    #[serde(default)]
    folders: Vec<Folder>,
    #[serde(default)]
    sessions: Vec<StoredSession>,
}

impl<'de> Deserialize<'de> for HostLibrary {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let stored = StoredLibrary::deserialize(deserializer)?;
        let mut library = Self {
            folders: stored.folders,
            sessions: Vec::with_capacity(stored.sessions.len()),
        };
        for mut session in stored.sessions {
            if session.profile.folder_id.is_none() && !session.group.is_empty() {
                let existing = library
                    .folders
                    .iter()
                    .find(|folder| folder.parent_id.is_none() && folder.name == session.group);
                let id = match existing {
                    Some(folder) => folder.id.clone(),
                    None => {
                        let id = legacy_folder_id(&session.group);
                        if library.folders.iter().any(|folder| folder.id == id)
                            || library.sessions.iter().any(|profile| profile.id == id)
                        {
                            return Err(serde::de::Error::custom("Legacy folder ID collision"));
                        }
                        library.folders.push(Folder {
                            id: id.clone(),
                            name: session.group,
                            parent_id: None,
                        });
                        id
                    }
                };
                session.profile.folder_id = Some(id);
            }
            library.sessions.push(session.profile);
        }
        library.validate().map_err(serde::de::Error::custom)?;
        Ok(library)
    }
}

fn legacy_folder_id(label: &str) -> String {
    use std::fmt::Write;

    let mut id = String::from("legacy-group-");
    for byte in label.as_bytes() {
        write!(id, "{byte:02x}").unwrap();
    }
    id
}

impl HostLibrary {
    pub fn validate(&self) -> Result<(), String> {
        let mut ids = HashSet::new();
        let mut siblings = HashSet::new();
        let mut folders = HashMap::new();
        for folder in &self.folders {
            if folder.id.trim().is_empty() || !ids.insert(folder.id.as_str()) {
                return Err("Folder IDs must be unique and nonempty".into());
            }
            if folder.name.is_empty() {
                return Err("Folder names must not be empty".into());
            }
            if !siblings.insert((folder.parent_id.as_deref(), folder.name.as_str())) {
                return Err("Folders with the same parent must have different names".into());
            }
            folders.insert(folder.id.as_str(), folder);
        }
        for folder in &self.folders {
            let mut visited = HashSet::new();
            let mut current = Some(folder.id.as_str());
            while let Some(id) = current {
                if !visited.insert(id) {
                    return Err("Folder hierarchy contains a cycle".into());
                }
                current = folders
                    .get(id)
                    .ok_or_else(|| "Folder parent does not exist".to_owned())?
                    .parent_id
                    .as_deref();
            }
        }
        let mut origins = HashSet::new();
        for session in &self.sessions {
            if session.id.trim().is_empty() || !ids.insert(session.id.as_str()) {
                return Err("Host and folder IDs must be unique and nonempty".into());
            }
            if session
                .folder_id
                .as_deref()
                .is_some_and(|id| !folders.contains_key(id))
            {
                return Err("Host folder does not exist".into());
            }
            if let Some(origin) = &session.import_origin {
                if origin.source_path.as_os_str().is_empty() || origin.alias.trim().is_empty() {
                    return Err("Import origin requires a source path and alias".into());
                }
                if !origins.insert(origin) {
                    return Err("Imported hosts must have unique source aliases".into());
                }
            }
        }
        Ok(())
    }

    pub fn folder_path(&self, id: &str) -> Option<String> {
        let mut parts = Vec::new();
        let mut visited = HashSet::new();
        let mut current = Some(id);
        while let Some(id) = current {
            if !visited.insert(id) {
                return None;
            }
            let folder = self.folders.iter().find(|folder| folder.id == id)?;
            parts.push(folder.name.as_str());
            current = folder.parent_id.as_deref();
        }
        parts.reverse();
        Some(parts.join(" / "))
    }

    pub fn folder_options(&self) -> Vec<(Option<String>, String)> {
        let mut children: HashMap<Option<&str>, Vec<&Folder>> = HashMap::new();
        for folder in &self.folders {
            children
                .entry(folder.parent_id.as_deref())
                .or_default()
                .push(folder);
        }
        for folders in children.values_mut() {
            folders.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.id.cmp(&b.id)));
        }
        let mut result = vec![(None, "根目录".into())];
        let mut stack: Vec<_> = children
            .get(&None)
            .into_iter()
            .flatten()
            .rev()
            .map(|folder| (*folder, folder.name.clone()))
            .collect();
        let mut visited = HashSet::new();
        while let Some((folder, path)) = stack.pop() {
            if !visited.insert(folder.id.as_str()) {
                continue;
            }
            result.push((Some(folder.id.clone()), path.clone()));
            if let Some(nested) = children.get(&Some(folder.id.as_str())) {
                stack.extend(
                    nested
                        .iter()
                        .rev()
                        .map(|child| (*child, format!("{path} / {}", child.name))),
                );
            }
        }
        result
    }

    pub fn create_folder(
        &mut self,
        name: String,
        parent_id: Option<String>,
    ) -> Result<String, String> {
        let mut updated = self.clone();
        let id = uuid::Uuid::new_v4().to_string();
        updated.folders.push(Folder {
            id: id.clone(),
            name: name.trim().to_owned(),
            parent_id,
        });
        updated.validate()?;
        *self = updated;
        Ok(id)
    }

    pub fn rename_folder(&mut self, id: &str, name: String) -> Result<(), String> {
        let mut updated = self.clone();
        let folder = updated
            .folders
            .iter_mut()
            .find(|folder| folder.id == id)
            .ok_or_else(|| "Folder does not exist".to_owned())?;
        folder.name = name.trim().to_owned();
        updated.validate()?;
        *self = updated;
        Ok(())
    }

    pub fn move_node(&mut self, node: &HostNodeId, target: Option<String>) -> Result<(), String> {
        let mut updated = self.clone();
        if target
            .as_ref()
            .is_some_and(|id| !updated.folders.iter().any(|folder| folder.id == *id))
        {
            return Err("Destination folder does not exist".into());
        }
        match node {
            HostNodeId::Folder(id) => {
                let folder = updated
                    .folders
                    .iter_mut()
                    .find(|folder| folder.id == *id)
                    .ok_or_else(|| "Folder does not exist".to_owned())?;
                folder.parent_id = target;
            }
            HostNodeId::Host(id) => {
                let session = updated
                    .sessions
                    .iter_mut()
                    .find(|session| session.id == *id)
                    .ok_or_else(|| "Host does not exist".to_owned())?;
                session.folder_id = target;
            }
        }
        updated.validate()?;
        *self = updated;
        Ok(())
    }

    pub fn delete_empty_folder(&mut self, id: &str) -> Result<(), String> {
        if !self.folders.iter().any(|folder| folder.id == id) {
            return Err("Folder does not exist".into());
        }
        if self
            .folders
            .iter()
            .any(|folder| folder.parent_id.as_deref() == Some(id))
            || self
                .sessions
                .iter()
                .any(|session| session.folder_id.as_deref() == Some(id))
        {
            return Err("Only empty folders can be deleted".into());
        }
        let mut updated = self.clone();
        updated.folders.retain(|folder| folder.id != id);
        updated.validate()?;
        *self = updated;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(id: &str, name: &str, parent: Option<&str>) -> Folder {
        Folder {
            id: id.into(),
            name: name.into(),
            parent_id: parent.map(str::to_owned),
        }
    }

    #[test]
    fn legacy_groups_migrate_deterministically_without_normalizing_labels() {
        let text = r#"[[sessions]]
id = "a"
group = " 运维/PROD "
[[sessions]]
id = "b"
group = " 运维/PROD "
[[sessions]]
id = "c"
group = "运维/PROD"
[[sessions]]
id = "root"
"#;
        let first: HostLibrary = toml::from_str(text).unwrap();
        let second: HostLibrary = toml::from_str(text).unwrap();
        assert_eq!(first.folders.len(), 2);
        assert_eq!(first.folders, second.folders);
        assert_eq!(first.folders[0].name, " 运维/PROD ");
        assert_eq!(
            first.folders[0].id,
            "legacy-group-20e8bf90e7bbb42f50524f4420"
        );
        assert_eq!(first.sessions[0].folder_id, first.sessions[1].folder_id);
        assert_ne!(first.sessions[0].folder_id, first.sessions[2].folder_id);
        assert!(first.sessions[3].folder_id.is_none());
        let encoded = toml::to_string(&first).unwrap();
        let value: toml::Value = toml::from_str(&encoded).unwrap();
        assert!(value["sessions"][0].get("group").is_none());
        let decoded: HostLibrary = toml::from_str(&encoded).unwrap();
        assert_eq!(decoded.folders, first.folders);
    }

    #[test]
    fn explicit_folders_win_and_matching_root_folder_is_reused() {
        let library: HostLibrary = toml::from_str(
            r#"[[folders]]
id = "existing"
name = "Production"
[[folders]]
id = "nested"
name = "Nested"
parent_id = "existing"
[[sessions]]
id = "a"
group = "Production"
[[sessions]]
id = "b"
folder_id = "nested"
group = "ignored"
"#,
        )
        .unwrap();
        assert_eq!(library.folders.len(), 2);
        assert_eq!(library.sessions[0].folder_id.as_deref(), Some("existing"));
        assert_eq!(library.sessions[1].folder_id.as_deref(), Some("nested"));
        assert_eq!(
            library.folder_path("nested").as_deref(),
            Some("Production / Nested")
        );
    }

    #[test]
    fn empty_and_folders_only_documents_round_trip() {
        let empty: HostLibrary = toml::from_str("").unwrap();
        assert!(empty.folders.is_empty());
        assert!(empty.sessions.is_empty());
        let mut library = empty;
        let root = library.create_folder("Zebra".into(), None).unwrap();
        let child = library
            .create_folder("Child".into(), Some(root.clone()))
            .unwrap();
        let alpha = library.create_folder("Alpha".into(), None).unwrap();
        let decoded: HostLibrary = toml::from_str(&toml::to_string(&library).unwrap()).unwrap();
        assert_eq!(decoded.folders, library.folders);
        assert_eq!(
            decoded.folder_options(),
            vec![
                (None, "根目录".into()),
                (Some(alpha), "Alpha".into()),
                (Some(root), "Zebra".into()),
                (Some(child), "Zebra / Child".into()),
            ]
        );
        let only: HostLibrary =
            toml::from_str("[[folders]]\nid = 'one'\nname = 'Empty'\n").unwrap();
        assert!(only.sessions.is_empty());
        assert_eq!(only.folders.len(), 1);
    }

    #[test]
    fn invalid_references_cycles_names_and_ids_fail_closed() {
        let invalid_folders = [
            vec![folder("one", "One", Some("missing"))],
            vec![folder("one", "One", Some("one"))],
            vec![
                folder("one", "One", Some("two")),
                folder("two", "Two", Some("one")),
            ],
            vec![folder("same", "One", None), folder("same", "Two", None)],
            vec![folder("one", "Same", None), folder("two", "Same", None)],
            vec![folder("", "Name", None)],
            vec![folder("one", "", None)],
        ];
        for folders in invalid_folders {
            let library = HostLibrary {
                folders,
                sessions: Vec::new(),
            };
            assert!(library.validate().is_err());
            assert!(toml::from_str::<HostLibrary>(&toml::to_string(&library).unwrap()).is_err());
        }
        let mut library = HostLibrary::default();
        library.sessions.push(SessionConfig {
            id: "host".into(),
            folder_id: Some("missing".into()),
            ..Default::default()
        });
        assert!(library.validate().is_err());
        library.sessions[0].folder_id = None;
        library.sessions.push(library.sessions[0].clone());
        assert!(library.validate().is_err());
        library.sessions.pop();
        library.folders.push(folder("host", "Collision", None));
        assert!(library.validate().is_err());
        let text = "[[folders]]\nid = 'legacy-group-61'\nname = 'Different'\n[[sessions]]\nid = 'host'\ngroup = 'a'\n";
        assert!(toml::from_str::<HostLibrary>(text).is_err());
    }

    #[test]
    fn rejected_folder_operations_do_not_mutate_the_library() {
        let mut library = HostLibrary::default();
        let root = library.create_folder("Root".into(), None).unwrap();
        let child = library
            .create_folder("Child".into(), Some(root.clone()))
            .unwrap();
        let other = library.create_folder("Child".into(), None).unwrap();
        let before = toml::to_string(&library).unwrap();
        assert!(library.create_folder("Root".into(), None).is_err());
        assert!(library.create_folder(" ".into(), None).is_err());
        assert!(library
            .create_folder("Missing".into(), Some("missing".into()))
            .is_err());
        assert!(library.rename_folder(&root, "Child".into()).is_err());
        assert!(library
            .move_node(&HostNodeId::Folder(root.clone()), Some(child.clone()))
            .is_err());
        assert!(library
            .move_node(&HostNodeId::Folder(root.clone()), Some(root.clone()))
            .is_err());
        assert!(library
            .move_node(&HostNodeId::Folder(child.clone()), None)
            .is_err());
        assert!(library
            .move_node(&HostNodeId::Folder(other), Some("missing".into()))
            .is_err());
        assert!(library.delete_empty_folder(&root).is_err());
        assert_eq!(toml::to_string(&library).unwrap(), before);
        library.rename_folder(&child, "Renamed".into()).unwrap();
        library
            .move_node(&HostNodeId::Folder(child.clone()), None)
            .unwrap();
        library.delete_empty_folder(&root).unwrap();
        assert_eq!(library.folder_path(&child).as_deref(), Some("Renamed"));
    }

    #[test]
    fn host_moves_keep_identity_authentication_and_origin() {
        let mut library = HostLibrary::default();
        let target = library.create_folder("Target".into(), None).unwrap();
        let profile = SessionConfig {
            id: "host".into(),
            name: "Server".into(),
            host: "server.example".into(),
            username: "admin".into(),
            remember_credentials: true,
            import_origin: Some(ImportOrigin {
                source_path: "/fixture/config".into(),
                alias: "server".into(),
            }),
            ..Default::default()
        };
        library.sessions.push(profile.clone());
        library
            .move_node(&HostNodeId::Host("host".into()), Some(target.clone()))
            .unwrap();
        assert_eq!(library.sessions[0].id, profile.id);
        assert!(crate::config::ConfigManager::same_credential_target(
            &profile,
            &library.sessions[0]
        ));
        assert_eq!(library.sessions[0].import_origin, profile.import_origin);
        assert!(library.sessions[0].remember_credentials);
        assert!(library.delete_empty_folder(&target).is_err());
        library
            .move_node(&HostNodeId::Host("host".into()), None)
            .unwrap();
        library.delete_empty_folder(&target).unwrap();
    }
}
