use std::{collections::BTreeMap, fs::File, io::Read, path::PathBuf};

use gpui_kit::{DummyKeyboardMapper, KeyBinding, KeyBindingContextPredicate, Keystroke};
use serde::Deserialize;

use crate::shortcuts::{definition, ShortcutDefinition, CATALOG};

use super::ConfigManager;

pub const MAX_KEYMAP_BYTES: usize = 64 * 1024;
const MAX_BINDINGS_PER_ACTION: usize = 8;

#[derive(Clone, Debug)]
pub struct EffectiveKeymap {
    pub path: PathBuf,
    bindings: BTreeMap<String, Vec<String>>,
}

#[derive(Clone, Debug)]
pub struct ShortcutConfig {
    pub keymap: EffectiveKeymap,
    pub warning: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawKeymap {
    version: toml::Spanned<u32>,
    #[serde(default)]
    bindings: BTreeMap<String, toml::Spanned<Vec<toml::Spanned<String>>>>,
}

impl ShortcutConfig {
    pub fn defaults(path: PathBuf) -> Self {
        Self {
            keymap: EffectiveKeymap::defaults(path),
            warning: None,
        }
    }

    pub fn load(manager: &ConfigManager) -> Self {
        let path = manager.keybindings_path();
        let mut config = Self::defaults(path.clone());
        let result = (|| {
            manager
                .create_keybindings_if_missing()
                .map_err(|_| "无法创建快捷键配置文件".to_owned())?;
            let metadata = std::fs::symlink_metadata(&path)
                .map_err(|_| "无法检查快捷键配置文件".to_owned())?;
            if !metadata.is_file() {
                return Err("快捷键配置必须是普通文件".into());
            }
            let file = File::open(&path).map_err(|_| "无法读取快捷键配置文件".to_owned())?;
            let mut content = Vec::new();
            file.take((MAX_KEYMAP_BYTES + 1) as u64)
                .read_to_end(&mut content)
                .map_err(|_| "无法读取快捷键配置文件".to_owned())?;
            if content.len() > MAX_KEYMAP_BYTES {
                return Err("快捷键配置超过 64 KiB 上限".into());
            }
            let content = std::str::from_utf8(&content)
                .map_err(|_| "快捷键配置必须是有效 UTF-8".to_owned())?;
            EffectiveKeymap::parse(content, path.clone())
        })();
        match result {
            Ok(keymap) => config.keymap = keymap,
            Err(reason) => {
                config.warning = Some(format!(
                    "{}：{reason}。本次使用默认快捷键，原文件未改写；修改后重启生效。",
                    path.display()
                ));
            }
        }
        config
    }
}

impl EffectiveKeymap {
    pub fn defaults(path: PathBuf) -> Self {
        let bindings = CATALOG
            .iter()
            .map(|definition| {
                let shortcut = if cfg!(target_os = "macos") {
                    definition.macos
                } else {
                    definition.other
                };
                (
                    definition.id.to_owned(),
                    vec![normalize(shortcut).expect("built-in shortcut")],
                )
            })
            .collect();
        Self { path, bindings }
    }

    pub fn parse(content: &str, path: PathBuf) -> Result<Self, String> {
        if content.len() > MAX_KEYMAP_BYTES {
            return Err("快捷键配置超过 64 KiB 上限".into());
        }
        let raw: RawKeymap = toml::from_str(content).map_err(|error: toml::de::Error| {
            located(
                content,
                error.span().map(|span| span.start),
                "无效 TOML 或配置结构",
            )
        })?;
        if *raw.version.get_ref() != 1 {
            return Err(located(
                content,
                Some(raw.version.span().start),
                "快捷键配置版本必须为 1",
            ));
        }
        let mut keymap = Self::defaults(path);
        for (id, values) in &raw.bindings {
            let position = Some(values.span().start);
            let action = definition(id).ok_or_else(|| located(content, position, "未知动作名"))?;
            if values.get_ref().len() > MAX_BINDINGS_PER_ACTION {
                return Err(located(content, position, "每个动作最多允许 8 个快捷键"));
            }
            let mut resolved = Vec::new();
            for value in values.get_ref() {
                let position = Some(value.span().start);
                let shortcut = normalize(value.get_ref())
                    .map_err(|reason| located(content, position, reason))?;
                load_binding(action, &shortcut)
                    .map_err(|_| located(content, position, "无效快捷键"))?;
                if matches!(
                    action.scope,
                    crate::shortcuts::ShortcutScope::Workspace
                        | crate::shortcuts::ShortcutScope::Global
                ) && !action.non_input
                    && input_reserved(&shortcut)
                {
                    return Err(located(
                        content,
                        position,
                        "该快捷键保留给输入框编辑，请选择其他按键",
                    ));
                }
                if resolved.contains(&shortcut) {
                    return Err(located(content, position, "同一动作包含重复快捷键"));
                }
                resolved.push(shortcut);
            }
            keymap.bindings.insert(id.clone(), resolved);
        }
        for (index, action) in CATALOG.iter().enumerate() {
            for other in &CATALOG[index + 1..] {
                if action.scope.overlaps(other.scope)
                    && keymap
                        .bindings_for(action.id)
                        .iter()
                        .any(|binding| keymap.bindings_for(other.id).contains(binding))
                {
                    let position = raw
                        .bindings
                        .get(action.id)
                        .or_else(|| raw.bindings.get(other.id))
                        .map(|values| values.span().start);
                    return Err(located(
                        content,
                        position,
                        "快捷键与重叠作用范围内的另一动作冲突",
                    ));
                }
            }
        }
        Ok(keymap)
    }

    pub fn bindings_for(&self, id: &str) -> &[String] {
        self.bindings.get(id).map(Vec::as_slice).unwrap_or(&[])
    }

    pub fn hint(&self, id: &str) -> String {
        self.bindings_for(id)
            .iter()
            .map(|shortcut| display_shortcut(shortcut))
            .collect::<Vec<_>>()
            .join(" / ")
    }

    pub fn accessibility_hint(&self, id: &str) -> String {
        self.bindings_for(id)
            .iter()
            .map(|shortcut| {
                let key = Keystroke::parse(shortcut).expect("validated shortcut");
                let mut parts = Vec::new();
                if key.modifiers.control {
                    parts.push("Control".to_owned());
                }
                if key.modifiers.alt {
                    parts.push("Alt".to_owned());
                }
                if key.modifiers.shift {
                    parts.push("Shift".to_owned());
                }
                if key.modifiers.platform {
                    parts.push("Meta".to_owned());
                }
                parts.push(key.key.to_uppercase());
                parts.join("+")
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    pub fn gpui_bindings(&self) -> Vec<KeyBinding> {
        CATALOG
            .iter()
            .flat_map(|action| {
                self.bindings_for(action.id)
                    .iter()
                    .map(|shortcut| load_binding(action, shortcut).expect("validated shortcut"))
            })
            .collect()
    }
}

pub fn default_keybindings_file() -> String {
    let mut content = String::from(
        "# RSTerm 快捷键；修改后重启生效。\n\
         # 省略动作沿用内置值；数组替换该动作全部默认键；[] 禁用。\n\
         # 单步按键使用 - 分隔修饰键；不支持多步组合键。\n\
         # secondary 在 macOS 是 Cmd，其他平台是 Ctrl，仍受终端安全校验限制。\n\
         # 保留 Tab、Shift-Tab、Ctrl-C 及基础终端控制/输入编辑键。\n\
         # 所有动作在应用弹窗打开时暂停；终端动作仅作用于终端。\n\
         version = 1\n\n[bindings]\n",
    );
    for action in CATALOG {
        content.push_str(&format!("# {} · {}\n", action.label, action.scope_label()));
        let shortcut = if cfg!(target_os = "macos") {
            action.macos
        } else {
            action.other
        };
        content.push_str(&format!("{} = [\"{}\"]\n", action.id, shortcut));
    }
    content
}

fn load_binding(action: &ShortcutDefinition, shortcut: &str) -> Result<KeyBinding, ()> {
    let predicate = action
        .predicate()
        .map(KeyBindingContextPredicate::parse)
        .transpose()
        .map_err(|_| ())?
        .map(std::rc::Rc::new);
    KeyBinding::load(
        shortcut,
        (action.action)(),
        predicate,
        false,
        None,
        &DummyKeyboardMapper,
    )
    .map_err(|_| ())
}

fn located(content: &str, position: Option<usize>, reason: &str) -> String {
    match position {
        Some(position) => {
            let before = &content.as_bytes()[..position.min(content.len())];
            let line = before.iter().filter(|byte| **byte == b'\n').count() + 1;
            let column = before.len()
                - before
                    .iter()
                    .rposition(|byte| *byte == b'\n')
                    .map_or(0, |position| position + 1)
                + 1;
            format!("第 {line} 行，第 {column} 列：{reason}")
        }
        None => reason.to_owned(),
    }
}

fn normalize(source: &str) -> Result<String, &'static str> {
    if source.is_empty()
        || source.len() > 128
        || !source.is_ascii()
        || source.chars().any(char::is_whitespace)
        || source.contains("->")
    {
        return Err("快捷键必须是非空、单步的按键表达式");
    }
    let mut parts = source.split('-').collect::<Vec<_>>();
    let mut seen = std::collections::HashSet::new();
    let last_modifier = if source.ends_with("--") {
        parts.len() - 2
    } else {
        parts.len() - 1
    };
    for part in &mut parts[..last_modifier] {
        *part = match part.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => "ctrl",
            "alt" | "option" => "alt",
            "shift" => "shift",
            "cmd" | "command" | "meta" | "super" | "win" => "cmd",
            "secondary" if cfg!(target_os = "macos") => "cmd",
            "secondary" => "ctrl",
            _ => return Err("未知修饰键"),
        };
        if !seen.insert(*part) {
            return Err("重复修饰键");
        }
    }
    let source = parts.join("-");
    let mut key = Keystroke::parse(&source).map_err(|_| "无效快捷键")?;
    key.key = match key.key.as_str() {
        "esc" => "escape".into(),
        "return" => "enter".into(),
        "pgup" => "pageup".into(),
        "pgdown" | "pgdn" => "pagedown".into(),
        other => other.into(),
    };
    let shifted_symbol = "!@#$%^&*()_+{}|:\"<>?~".contains(key.key.as_str());
    let mut safe_modifiers = key.modifiers;
    safe_modifiers.shift |= shifted_symbol;
    if key.modifiers.shift {
        if let Some(symbol) = match key.key.as_str() {
            "1" => Some("!"),
            "2" => Some("@"),
            "3" => Some("#"),
            "4" => Some("$"),
            "5" => Some("%"),
            "6" => Some("^"),
            "7" => Some("&"),
            "8" => Some("*"),
            "9" => Some("("),
            "0" => Some(")"),
            "-" => Some("_"),
            "=" => Some("+"),
            "[" => Some("{"),
            "]" => Some("}"),
            "\\" => Some("|"),
            ";" => Some(":"),
            "'" => Some("\""),
            "," => Some("<"),
            "." => Some(">"),
            "/" => Some("?"),
            "`" => Some("~"),
            _ => None,
        } {
            key.key = symbol.into();
            key.modifiers.shift = false;
        } else if shifted_symbol {
            key.modifiers.shift = false;
        }
    }
    let function_number = key
        .key
        .strip_prefix('f')
        .and_then(|number| number.parse::<u8>().ok())
        .filter(|number| (1..=24).contains(number));
    if let Some(number) = function_number {
        key.key = format!("f{number}");
    }
    let function_key = function_number.is_some();
    let printable = key.key.len() == 1
        && key.key.as_bytes()[0].is_ascii_graphic()
        && key.key.as_bytes()[0] != 127;
    let tab_navigation =
        key.key == "tab" && key.modifiers.control && !key.modifiers.alt && !key.modifiers.platform;
    let digit_navigation = key.key.len() == 1
        && matches!(key.key.as_bytes()[0], b'1'..=b'9')
        && key.modifiers.alt
        && !key.modifiers.shift
        && !key.modifiers.control
        && !key.modifiers.platform;
    let safe_modified = printable
        && ((cfg!(target_os = "macos") && safe_modifiers.platform)
            || (safe_modifiers.control && safe_modifiers.shift));
    if !(function_key || tab_navigation || digit_navigation || safe_modified) {
        return Err("该按键会占用普通输入或终端控制键；请使用 Cmd、安全复合修饰键或功能键");
    }
    Ok(key.unparse())
}

fn input_reserved(shortcut: &str) -> bool {
    let Ok(key) = Keystroke::parse(shortcut) else {
        return true;
    };
    let modifiers = key.modifiers;
    #[cfg(target_os = "macos")]
    {
        if modifiers.platform && !modifiers.control && !modifiers.alt {
            return match key.key.as_str() {
                "a" | "c" | "v" | "x" | "[" | "]" | "." => !modifiers.shift,
                "z" | "f" => true,
                _ => false,
            };
        }
        modifiers.control
            && modifiers.shift
            && !modifiers.platform
            && !modifiers.alt
            && matches!(key.key.as_str(), "a" | "e")
    }
    #[cfg(not(target_os = "macos"))]
    {
        !modifiers.platform
            && !modifiers.alt
            && modifiers.control
            && !modifiers.shift
            && matches!(
                key.key.as_str(),
                "a" | "c" | "v" | "x" | "z" | "y" | "f" | "h" | "[" | "]" | "."
            )
    }
}

pub fn display_shortcut(source: &str) -> String {
    let Ok(key) = Keystroke::parse(source) else {
        return String::new();
    };
    let mut parts = Vec::new();
    if key.modifiers.control {
        parts.push("Ctrl".to_owned());
    }
    if key.modifiers.alt {
        parts.push("Alt".to_owned());
    }
    if key.modifiers.shift {
        parts.push("Shift".to_owned());
    }
    if key.modifiers.platform {
        parts.push(
            if cfg!(target_os = "macos") {
                "⌘"
            } else {
                "Super"
            }
            .into(),
        );
    }
    parts.push(match key.key.as_str() {
        "tab" => "Tab".into(),
        other => other.to_uppercase(),
    });
    parts.join("+")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(content: &str) -> Result<EffectiveKeymap, String> {
        EffectiveKeymap::parse(content, "/fixture/keybindings.toml".into())
    }

    #[test]
    fn generated_defaults_round_trip_and_match_the_catalog() {
        let map = parse(&default_keybindings_file()).unwrap();
        for action in CATALOG {
            assert_eq!(map.bindings_for(action.id).len(), 1);
        }
        assert_eq!(map.gpui_bindings().len(), CATALOG.len());
    }

    #[test]
    fn overrides_replace_defaults_and_empty_arrays_disable() {
        let map = parse(
            "version = 1\n[bindings]\nnext_tab = ['f13', 'ctrl-shift-f14']\nshow_history = []\n",
        )
        .unwrap();
        assert_eq!(map.bindings_for("next_tab"), ["f13", "ctrl-shift-f14"]);
        assert!(map.bindings_for("show_history").is_empty());
        assert!(!map.bindings_for("close_tab").is_empty());
        assert_eq!(map.gpui_bindings().len(), CATALOG.len());
    }

    #[test]
    fn shifted_punctuation_normalizes_to_native_key_events() {
        for (source, expected) in [
            ("ctrl-shift-,", "ctrl-<"),
            ("ctrl-alt-shift-/", "ctrl-alt-?"),
            ("ctrl-shift-]", "ctrl-}"),
            ("ctrl-}", "ctrl-}"),
        ] {
            assert_eq!(normalize(source).unwrap(), expected);
            assert!(parse(&format!("version = 1\n[bindings]\nnext_tab = [{source:?}]")).is_ok());
        }
        #[cfg(target_os = "macos")]
        {
            assert_eq!(normalize("cmd-shift-]").unwrap(), "cmd-}");
            assert!(parse("version = 1\n[bindings]\nnext_tab = ['cmd-shift-]']").is_ok());
        }
        for action in CATALOG {
            assert!(normalize(action.other).is_ok(), "{}", action.id);
        }
    }

    #[test]
    fn aliases_case_and_shifted_symbols_cannot_hide_collisions() {
        for values in [
            "['ctrl-tab', 'control-tab']",
            "['f13', 'F13']",
            "['f13', 'f013']",
            "['ctrl-shift-/', 'control-shift-?']",
        ] {
            assert!(parse(&format!("version = 1\n[bindings]\nnext_tab = {values}")).is_err());
        }
        #[cfg(target_os = "macos")]
        for values in [
            "['cmd-n', 'secondary-n']",
            "['command-N', 'cmd-shift-n']",
            "['cmd-shift-]', 'cmd-}']",
        ] {
            assert!(parse(&format!("version = 1\n[bindings]\nnext_tab = {values}")).is_err());
        }
        assert!(
            parse("version = 1\n[bindings]\nnext_tab = ['f13']\nprevious_tab = ['f13']").is_err()
        );
        assert!(
            parse("version = 1\n[bindings]\nnext_tab = ['f13']\nterminal_copy = ['f13']").is_err()
        );
        assert!(parse(
            "version = 1\n[bindings]\nterminal_copy = ['f13']\nmove_host_node = ['f13']"
        )
        .is_ok());
    }

    #[test]
    fn dangerous_input_sequences_unknown_actions_and_limits_are_rejected() {
        for value in [
            "",
            "a",
            "shift-a",
            "ctrl-c",
            "ctrl-a",
            "alt-x",
            "tab",
            "shift-tab",
            "enter",
            "cmd-enter",
            "ctrl-shift-left",
            "cmd-backspace",
            "fn",
            "cmd-madeup",
            "cmd-ctrl-cmd-a",
            "cmd-x cmd-y",
            "cmd-a->secret",
        ] {
            assert!(
                parse(&format!("version = 1\n[bindings]\nnext_tab = [{value:?}]")).is_err(),
                "{value}"
            );
        }
        for content in [
            "version = 2",
            "[bindings]",
            "version = 1\n[bindings]\nsecret_unknown_action = []",
            "version = 1\n[bindings]\nnext_tab = 'secret_wrong_type'",
            "version = 1\nunknown = 'secret_unknown_field'",
            "version = 1\n[bindings]\nnext_tab = ['f13', 'f14', 'f15', 'f16', 'f17', 'f18', 'f19', 'f20', 'f21']",
        ] {
            let error = parse(content).unwrap_err();
            assert!(!error.contains("secret"));
        }
        assert!(parse(&"#".repeat(MAX_KEYMAP_BYTES + 1)).is_err());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn workspace_overrides_cannot_shadow_input_editing() {
        for shortcut in [
            "cmd-a",
            "cmd-c",
            "cmd-v",
            "cmd-x",
            "cmd-z",
            "cmd-shift-z",
            "cmd-f",
            "cmd-shift-f",
            "cmd-[",
            "cmd-]",
            "cmd-.",
            "ctrl-shift-a",
            "ctrl-shift-e",
        ] {
            assert!(parse(&format!(
                "version = 1\n[bindings]\nnext_tab = [{shortcut:?}]"
            ))
            .is_err());
        }
        assert!(parse("version = 1\n[bindings]\nterminal_copy = ['cmd-z']").is_ok());
    }

    #[test]
    fn parse_diagnostics_have_position_but_never_source_values() {
        let error = parse("version = 1\n[bindings]\nnext_tab = ['secret-input']").unwrap_err();
        assert!(error.contains("第 3 行"));
        assert!(!error.contains("secret-input"));
        let error = parse("version = 1\n[bindings]\nnext_tab = secret-input").unwrap_err();
        assert!(error.contains("第 3 行"));
        assert!(!error.contains("secret-input"));
    }
}
