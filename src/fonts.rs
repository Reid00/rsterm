use gpui_kit::{font, Font, FontFallbacks, FontFeatures, Window};

pub fn terminal_font(preferred: &str, window: &Window) -> Font {
    resolve_terminal_font(preferred, &window.text_system().all_font_names())
}

fn resolve_terminal_font(preferred: &str, installed: &[String]) -> Font {
    let family = [preferred, "Menlo", "SF Mono", "Monaco", "monospace"]
        .into_iter()
        .find(|candidate| {
            installed
                .iter()
                .any(|name| name.eq_ignore_ascii_case(candidate))
        })
        .unwrap_or("Menlo");
    let nerd_font = installed
        .iter()
        .find(|name| name.eq_ignore_ascii_case("JetBrainsMono Nerd Font Mono"))
        .or_else(|| {
            installed
                .iter()
                .find(|name| name.ends_with(" Nerd Font Mono"))
        });
    let mut fallbacks = Vec::new();
    if let Some(nerd_font) = nerd_font {
        fallbacks.push(nerd_font.clone());
    }
    fallbacks.extend([
        "Menlo".into(),
        "PingFang SC".into(),
        "Hiragino Sans".into(),
        "Apple Color Emoji".into(),
    ]);
    Font {
        features: FontFeatures::disable_ligatures(),
        fallbacks: Some(FontFallbacks::from_fonts(fallbacks)),
        ..font(family.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::resolve_terminal_font;

    #[test]
    fn preferred_primary_is_preserved_and_jetbrains_mono_fallback_is_prioritized() {
        let installed = [
            "Custom Mono",
            "FiraCode Nerd Font Mono",
            "JetBrainsMono Nerd Font Propo",
            "JetBrainsMono Nerd Font Mono",
            "Menlo",
        ]
        .map(String::from);
        let font = resolve_terminal_font("Custom Mono", &installed);

        assert_eq!(font.family.as_ref(), "Custom Mono");
        assert_eq!(
            font.fallbacks.as_ref().unwrap().fallback_list(),
            &[
                "JetBrainsMono Nerd Font Mono",
                "Menlo",
                "PingFang SC",
                "Hiragino Sans",
                "Apple Color Emoji",
            ]
        );
    }

    #[test]
    fn another_installed_nerd_mono_family_is_used_instead_of_propo() {
        let installed = [
            "Menlo",
            "JetBrainsMono Nerd Font Propo",
            "FiraCode Nerd Font Mono",
        ]
        .map(String::from);
        let font = resolve_terminal_font("Menlo", &installed);

        assert_eq!(font.family.as_ref(), "Menlo");
        assert_eq!(
            font.fallbacks.as_ref().unwrap().fallback_list()[0],
            "FiraCode Nerd Font Mono"
        );
    }

    #[test]
    fn absent_nerd_mono_family_is_not_added_to_fallbacks() {
        let installed = ["Menlo", "JetBrainsMono Nerd Font Propo"].map(String::from);
        let font = resolve_terminal_font("Menlo", &installed);

        assert_eq!(
            font.fallbacks.as_ref().unwrap().fallback_list(),
            &["Menlo", "PingFang SC", "Hiragino Sans", "Apple Color Emoji"]
        );
    }

    #[test]
    fn missing_preferred_font_keeps_system_family_resolution_order() {
        let families = ["Menlo", "SF Mono", "Monaco", "monospace"];
        for (index, expected) in families.iter().enumerate() {
            let installed: Vec<_> = families[index..]
                .iter()
                .map(|family| (*family).to_owned())
                .collect();
            let font = resolve_terminal_font("Unavailable", &installed);

            assert_eq!(font.family.as_ref(), *expected);
        }
        assert_eq!(
            resolve_terminal_font("Unavailable", &[]).family.as_ref(),
            "Menlo"
        );
    }
}
