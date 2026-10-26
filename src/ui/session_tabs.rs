use std::collections::HashMap;

#[derive(Clone, Debug)]
pub struct TabIdentity<'a> {
    pub title: &'a str,
    pub target: &'a str,
    pub ordinal: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TabLabel {
    pub label: String,
    pub instance: Option<String>,
}

pub fn labels(tabs: &[TabIdentity<'_>], opened: &HashMap<String, u64>) -> Vec<TabLabel> {
    tabs.iter()
        .map(|tab| {
            let same_name = tabs.iter().filter(|other| other.title == tab.title).count() > 1;
            let different_target = tabs
                .iter()
                .any(|other| other.title == tab.title && other.target != tab.target);
            let instance = (opened.get(tab.target).copied().unwrap_or(1) > 1)
                .then(|| format!("#{}", tab.ordinal));
            TabLabel {
                label: if same_name && different_target {
                    format!("{} · {}", tab.title, tab.target)
                } else {
                    tab.title.to_owned()
                },
                instance,
            }
        })
        .collect()
}

pub fn next_index(current: Option<usize>, count: usize, previous: bool) -> Option<usize> {
    if count == 0 {
        return None;
    }
    let Some(current) = current.filter(|index| *index < count) else {
        return Some(if previous { count - 1 } else { 0 });
    };
    Some(if previous {
        (current + count - 1) % count
    } else {
        (current + 1) % count
    })
}

pub fn inspector_width(saved: f32) -> f32 {
    if saved.is_finite() {
        saved.clamp(280., 480.)
    } else {
        320.
    }
}

pub fn sidebar_width(saved: f32, viewport: f32) -> f32 {
    let saved = if saved.is_finite() { saved } else { 200. };
    saved.clamp(180., (viewport - 440. - 40. - 2.).clamp(180., 420.))
}

pub fn inspector_is_inline(viewport: f32, sidebar: f32, inspector: f32) -> bool {
    viewport - sidebar_width(sidebar, viewport) - 40. - 2. >= 440. + inspector_width(inspector)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cycling_handles_empty_single_and_wrap() {
        assert_eq!(next_index(None, 0, false), None);
        assert_eq!(next_index(Some(0), 1, false), Some(0));
        assert_eq!(next_index(Some(0), 3, true), Some(2));
        assert_eq!(next_index(Some(2), 3, false), Some(0));
        assert_eq!(next_index(Some(99), 3, false), Some(0));
        assert_eq!(next_index(None, 3, true), Some(2));
    }

    #[test]
    fn repeated_targets_keep_instance_numbers_after_close() {
        let opened = HashMap::from([("user@fixture:22".into(), 3)]);
        let tabs = [TabIdentity {
            title: "dev",
            target: "user@fixture:22",
            ordinal: 3,
        }];
        assert_eq!(labels(&tabs, &opened)[0].instance.as_deref(), Some("#3"));
        assert_eq!(labels(&tabs, &opened)[0].label, "dev");
    }

    #[test]
    fn duplicate_names_with_distinct_targets_include_target() {
        let tabs = [
            TabIdentity {
                title: "dev",
                target: "a@one:22",
                ordinal: 1,
            },
            TabIdentity {
                title: "dev",
                target: "b@two:22",
                ordinal: 1,
            },
        ];
        let result = labels(&tabs, &HashMap::new());
        assert_eq!(result[0].label, "dev · a@one:22");
        assert_eq!(result[1].label, "dev · b@two:22");
    }

    #[test]
    fn inspector_replaces_body_when_three_columns_do_not_fit() {
        assert!(inspector_is_inline(1280., 200., 320.));
        assert!(!inspector_is_inline(860., 200., 320.));
        assert!(!inspector_is_inline(1280., 420., 480.));
        assert!(sidebar_width(420., 860.) + 440. + 40. + 2. <= 860.);
        assert_eq!(inspector_width(f32::NAN), 320.);
        assert_eq!(inspector_width(900.), 480.);
    }
}
