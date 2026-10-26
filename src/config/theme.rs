use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Theme {
    pub name: String,
    #[serde(default)]
    pub ui: UiColors,
    #[serde(default)]
    pub terminal: TerminalColorPalette,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct UiColors {
    pub background: [u8; 3],
    pub foreground: [u8; 3],
    pub tab_active_bg: [u8; 3],
    pub tab_inactive_bg: [u8; 3],
    pub panel_bg: [u8; 3],
    pub selection_bg: [u8; 4],
    pub statusbar_bg: [u8; 3],
    pub accent: [u8; 3],
    pub error: [u8; 3],
    #[serde(default = "default_border")]
    pub border: [u8; 3],
    #[serde(default = "default_muted_foreground")]
    pub muted_foreground: [u8; 3],
    #[serde(default = "default_success")]
    pub success: [u8; 3],
    #[serde(default = "default_warning")]
    pub warning: [u8; 3],
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct TerminalColorPalette {
    #[serde(default = "default_ansi_colors")]
    pub ansi: [[u8; 3]; 16],
    pub fg: [u8; 3],
    pub bg: [u8; 3],
    pub cursor: [u8; 3],
}

#[derive(Debug, Clone, Copy)]
pub struct BuiltinTheme {
    pub id: &'static str,
    pub label: &'static str,
    pub is_light: bool,
    ui: UiColors,
    terminal: TerminalColorPalette,
}

const fn color(hex: u32) -> [u8; 3] {
    [(hex >> 16) as u8, (hex >> 8) as u8, hex as u8]
}

const fn ansi(values: [u32; 16]) -> [[u8; 3]; 16] {
    let mut result = [[0; 3]; 16];
    let mut i = 0;
    while i < 16 {
        result[i] = color(values[i]);
        i += 1;
    }
    result
}

pub const BUILTIN_THEMES: [BuiltinTheme; 4] = [
    BuiltinTheme {
        id: "dark",
        label: "Tokyo Night",
        is_light: false,
        ui: UiColors {
            background: color(0x1a1b26),
            foreground: color(0xc0caf5),
            tab_active_bg: color(0x24283b),
            tab_inactive_bg: color(0x16161e),
            panel_bg: color(0x16161e),
            selection_bg: [0x7a, 0xa2, 0xf7, 0x48],
            statusbar_bg: color(0x1a1b26),
            accent: color(0x7aa2f7),
            error: color(0xf7768e),
            border: color(0x292e42),
            muted_foreground: color(0x9aa5ce),
            success: color(0x9ece6a),
            warning: color(0xe0af68),
        },
        terminal: TerminalColorPalette {
            ansi: ansi([
                0x15161e, 0xf7768e, 0x9ece6a, 0xe0af68, 0x7aa2f7, 0xbb9af7, 0x7dcfff, 0xa9b1d6,
                0x565f89, 0xff899d, 0xb9f27c, 0xf5c77e, 0x89b4ff, 0xc8a6ff, 0x89ddff, 0xc0caf5,
            ]),
            fg: color(0xc0caf5),
            bg: color(0x1a1b26),
            cursor: color(0xc0caf5),
        },
    },
    BuiltinTheme {
        id: "catppuccin-mocha",
        label: "Catppuccin Mocha",
        is_light: false,
        ui: UiColors {
            background: color(0x1e1e2e),
            foreground: color(0xcdd6f4),
            tab_active_bg: color(0x313244),
            tab_inactive_bg: color(0x181825),
            panel_bg: color(0x181825),
            selection_bg: [0xcb, 0xa6, 0xf7, 0x40],
            statusbar_bg: color(0x1e1e2e),
            accent: color(0xcba6f7),
            error: color(0xf38ba8),
            border: color(0x313244),
            muted_foreground: color(0xa6adc8),
            success: color(0xa6e3a1),
            warning: color(0xf9e2af),
        },
        terminal: TerminalColorPalette {
            ansi: ansi([
                0x45475a, 0xf38ba8, 0xa6e3a1, 0xf9e2af, 0x89b4fa, 0xf5c2e7, 0x94e2d5, 0xbac2de,
                0x585b70, 0xf38ba8, 0xa6e3a1, 0xf9e2af, 0x89b4fa, 0xf5c2e7, 0x94e2d5, 0xa6adc8,
            ]),
            fg: color(0xcdd6f4),
            bg: color(0x1e1e2e),
            cursor: color(0xf5e0dc),
        },
    },
    BuiltinTheme {
        id: "nord",
        label: "Nord",
        is_light: false,
        ui: UiColors {
            background: color(0x2e3440),
            foreground: color(0xeceff4),
            tab_active_bg: color(0x3b4252),
            tab_inactive_bg: color(0x292e39),
            panel_bg: color(0x292e39),
            selection_bg: [0x88, 0xc0, 0xd0, 0x40],
            statusbar_bg: color(0x2e3440),
            accent: color(0x88c0d0),
            error: color(0xe08a93),
            border: color(0x434c5e),
            muted_foreground: color(0xb8c1d1),
            success: color(0xa3be8c),
            warning: color(0xebcb8b),
        },
        terminal: TerminalColorPalette {
            ansi: ansi([
                0x3b4252, 0xbf616a, 0xa3be8c, 0xebcb8b, 0x81a1c1, 0xb48ead, 0x88c0d0, 0xe5e9f0,
                0x4c566a, 0xbf616a, 0xa3be8c, 0xebcb8b, 0x81a1c1, 0xb48ead, 0x8fbcbb, 0xeceff4,
            ]),
            fg: color(0xeceff4),
            bg: color(0x2e3440),
            cursor: color(0xd8dee9),
        },
    },
    BuiltinTheme {
        id: "light",
        label: "Catppuccin Latte",
        is_light: true,
        ui: UiColors {
            background: color(0xeff1f5),
            foreground: color(0x4c4f69),
            tab_active_bg: color(0xffffff),
            tab_inactive_bg: color(0xe6e9ef),
            panel_bg: color(0xe6e9ef),
            selection_bg: [0x1e, 0x66, 0xf5, 0x30],
            statusbar_bg: color(0xeff1f5),
            accent: color(0x1e66f5),
            error: color(0xd20f39),
            border: color(0xccd0da),
            muted_foreground: color(0x5c5f77),
            success: color(0x3b7021),
            warning: color(0x8a5800),
        },
        terminal: TerminalColorPalette {
            ansi: ansi([
                0x4c4f69, 0xd20f39, 0x3b7021, 0x8a5800, 0x1d5fe3, 0x8839ef, 0x087b88, 0x6c6f85,
                0x5c5f77, 0xb80c32, 0x3b7021, 0x8a5800, 0x1d5fe3, 0x8839ef, 0x087b88, 0x7c7f93,
            ]),
            fg: color(0x4c4f69),
            bg: color(0xeff1f5),
            cursor: color(0x4c4f69),
        },
    },
];

fn default_ansi_colors() -> [[u8; 3]; 16] {
    BUILTIN_THEMES[0].terminal.ansi
}

fn default_border() -> [u8; 3] {
    BUILTIN_THEMES[0].ui.border
}

fn default_muted_foreground() -> [u8; 3] {
    BUILTIN_THEMES[0].ui.muted_foreground
}

fn default_success() -> [u8; 3] {
    BUILTIN_THEMES[0].ui.success
}

fn default_warning() -> [u8; 3] {
    BUILTIN_THEMES[0].ui.warning
}

impl Default for UiColors {
    fn default() -> Self {
        BUILTIN_THEMES[0].ui
    }
}

impl Default for TerminalColorPalette {
    fn default() -> Self {
        BUILTIN_THEMES[0].terminal
    }
}

impl Default for Theme {
    fn default() -> Self {
        Self::default_dark()
    }
}

impl Theme {
    pub fn builtin(name: &str) -> Option<Self> {
        BUILTIN_THEMES
            .iter()
            .find(|preset| preset.id == name)
            .map(|preset| Self {
                name: preset.id.into(),
                ui: preset.ui,
                terminal: preset.terminal,
            })
    }

    pub fn for_name(name: &str) -> Self {
        Self::builtin(name).unwrap_or_default()
    }

    pub fn is_light(&self) -> bool {
        BUILTIN_THEMES
            .iter()
            .any(|preset| preset.id == self.name && preset.is_light)
    }

    pub fn default_dark() -> Self {
        Self::builtin("dark").unwrap()
    }

    pub fn default_light() -> Self {
        Self::builtin("light").unwrap()
    }

    pub fn terminal_fg(&self) -> [u8; 3] {
        self.terminal.fg
    }

    pub fn terminal_bg(&self) -> [u8; 3] {
        self.terminal.bg
    }

    pub fn selection_bg(&self) -> [u8; 4] {
        self.ui.selection_bg
    }

    pub fn indexed_color(&self, idx: u8) -> [u8; 3] {
        match idx {
            0..=15 => self.terminal.ansi[idx as usize],
            16..=231 => {
                let n = idx - 16;
                let component = |value: u8| if value == 0 { 0 } else { value * 40 + 55 };
                [component(n / 36), component((n / 6) % 6), component(n % 6)]
            }
            232..=255 => {
                let gray = (idx - 232) * 10 + 8;
                [gray; 3]
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn contrast(a: [u8; 3], b: [u8; 3]) -> f64 {
        let luminance = |rgb: [u8; 3]| {
            rgb.into_iter()
                .zip([0.2126, 0.7152, 0.0722])
                .map(|(channel, weight)| {
                    let value = f64::from(channel) / 255.;
                    let linear = if value <= 0.04045 {
                        value / 12.92
                    } else {
                        ((value + 0.055) / 1.055).powf(2.4)
                    };
                    linear * weight
                })
                .sum::<f64>()
        };
        let a = luminance(a);
        let b = luminance(b);
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }

    #[test]
    fn builtins_have_unique_ids_names_and_readable_palettes() {
        let mut ids = std::collections::HashSet::new();
        let mut labels = std::collections::HashSet::new();
        for preset in BUILTIN_THEMES {
            assert!(ids.insert(preset.id));
            assert!(labels.insert(preset.label));
            let theme = Theme::builtin(preset.id).unwrap();
            assert_eq!(theme.is_light(), preset.is_light);
            assert!(
                contrast(theme.ui.foreground, theme.ui.background) >= 4.5,
                "{} text",
                preset.id
            );
            assert!(
                contrast(theme.ui.muted_foreground, theme.ui.panel_bg) >= 4.5,
                "{} secondary",
                preset.id
            );
            assert!(
                contrast(theme.terminal.ansi[4], theme.terminal.bg) >= 4.5,
                "{} blue",
                preset.id
            );
            assert!(
                contrast(theme.terminal.cursor, theme.terminal.bg) >= 3.,
                "{} cursor",
                preset.id
            );
            assert!(
                contrast(theme.ui.accent, theme.ui.background) >= 3.,
                "{} focus",
                preset.id
            );
            for index in 16..=255 {
                assert_eq!(
                    theme.indexed_color(index),
                    Theme::default_dark().indexed_color(index)
                );
            }
        }
        assert_eq!(Theme::default_dark().name, "dark");
        assert_eq!(Theme::default_light().name, "light");
        assert_eq!(Theme::for_name("missing").name, "dark");
        assert!(Theme::builtin("missing").is_none());
    }

    #[test]
    fn older_theme_documents_accept_new_optional_semantic_colors() {
        let mut document = toml::Value::try_from(Theme::default_dark()).unwrap();
        let ui = document.get_mut("ui").unwrap().as_table_mut().unwrap();
        for field in ["border", "muted_foreground", "success", "warning"] {
            ui.remove(field);
        }
        let theme: Theme = document.try_into().unwrap();
        assert_eq!(theme.ui.border, default_border());
        assert_eq!(theme.ui.muted_foreground, default_muted_foreground());
        assert_eq!(theme.terminal.ansi, default_ansi_colors());
    }
}
