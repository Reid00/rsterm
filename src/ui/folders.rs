use gpui_kit::component::{
    button::{Button, ButtonVariants},
    input::{Input, InputEvent, InputState},
    select::{SearchableVec, Select, SelectEvent, SelectItem, SelectState},
    ActiveTheme, Disableable, IndexPath,
};
use gpui_kit::{prelude::*, *};

use crate::{
    config::{HostLibrary, HostNodeId},
    ui::connection::{field, row},
};

#[derive(Clone, Debug)]
pub enum FolderFormAction {
    Save(String),
    Cancel,
}

pub struct FolderForm {
    name: Entity<InputState>,
    pub error: Option<String>,
    pub busy: bool,
    _subscription: Subscription,
}

impl FolderForm {
    pub fn new(name: &str, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let name = field(name, "文件夹名称", false, window, cx);
        let subscription = cx.subscribe(&name, |this, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::PressEnter { .. }) {
                this.save(cx);
            }
        });
        Self {
            name,
            error: None,
            busy: false,
            _subscription: subscription,
        }
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let name = self.name.read(cx).value().trim().to_owned();
        if name.is_empty() {
            self.error = Some("请填写文件夹名称".into());
        } else {
            self.error = None;
            self.busy = true;
            cx.emit(FolderFormAction::Save(name));
        }
        cx.notify();
    }
}

impl EventEmitter<FolderFormAction> for FolderForm {}

impl Render for FolderForm {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap_3()
            .w_full()
            .child(row(
                "文件夹名称",
                Input::new(&self.name).id("folder-name").disabled(self.busy),
            ))
            .when_some(self.error.clone(), |view, error| {
                view.child(div().text_sm().text_color(cx.theme().danger).child(error))
            })
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap_2()
                    .child(
                        Button::new("folder-cancel")
                            .ghost()
                            .label("取消")
                            .disabled(self.busy)
                            .on_click(cx.listener(|this, _, _, cx| {
                                if !this.busy {
                                    cx.emit(FolderFormAction::Cancel);
                                }
                            })),
                    )
                    .child(
                        Button::new("folder-save")
                            .primary()
                            .label(if self.busy { "保存中…" } else { "保存" })
                            .disabled(self.busy)
                            .on_click(cx.listener(|this, _, _, cx| this.save(cx))),
                    ),
            )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct FolderDestination {
    id: Option<String>,
    label: String,
}

impl SelectItem for FolderDestination {
    type Value = Option<String>;

    fn title(&self) -> SharedString {
        self.label.clone().into()
    }

    fn value(&self) -> &Self::Value {
        &self.id
    }
}

fn destinations(library: &HostLibrary, node: &HostNodeId) -> Vec<FolderDestination> {
    let current = match node {
        HostNodeId::Host(id) => library
            .sessions
            .iter()
            .find(|host| &host.id == id)
            .map(|host| host.folder_id.as_ref()),
        HostNodeId::Folder(id) => library
            .folders
            .iter()
            .find(|folder| &folder.id == id)
            .map(|folder| folder.parent_id.as_ref()),
    };
    library
        .folder_options()
        .into_iter()
        .filter(|(id, _)| {
            current.is_some_and(|current| current != id.as_ref())
                && library.clone().move_node(node, id.clone()).is_ok()
        })
        .map(|(id, label)| FolderDestination { id, label })
        .collect()
}

#[derive(Clone, Debug)]
pub enum MoveFormAction {
    Move {
        node: HostNodeId,
        target: Option<String>,
    },
    Cancel,
}

pub struct MoveForm {
    library: HostLibrary,
    node: HostNodeId,
    target: Entity<SelectState<SearchableVec<FolderDestination>>>,
    pub error: Option<String>,
    pub busy: bool,
    _subscription: Subscription,
}

impl MoveForm {
    pub fn new(
        library: &HostLibrary,
        node: HostNodeId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let options = destinations(library, &node);
        let selected = if options.is_empty() {
            None
        } else {
            Some(IndexPath::new(0))
        };
        let target =
            cx.new(|cx| SelectState::new(SearchableVec::new(options), selected, window, cx));
        let subscription = cx.subscribe(&target, |_, _, _: &SelectEvent<_>, cx| cx.notify());
        Self {
            library: library.clone(),
            node,
            target,
            error: None,
            busy: false,
            _subscription: subscription,
        }
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let Some(target) = self.target.read(cx).selected_value().cloned() else {
            self.error = Some("没有可用的目标文件夹".into());
            cx.notify();
            return;
        };
        if let Err(error) = self.library.clone().move_node(&self.node, target.clone()) {
            self.error = Some(error);
        } else {
            self.error = None;
            self.busy = true;
            cx.emit(MoveFormAction::Move {
                node: self.node.clone(),
                target,
            });
        }
        cx.notify();
    }
}

impl EventEmitter<MoveFormAction> for MoveForm {}

impl Render for MoveForm {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let has_target = self.target.read(cx).selected_value().is_some();
        div()
            .flex()
            .flex_col()
            .gap_3()
            .w_full()
            .child(row(
                "目标文件夹",
                Select::new(&self.target)
                    .placeholder("选择目标文件夹")
                    .disabled(self.busy)
                    .w_full(),
            ))
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(if has_target {
                        "只更改主机库中的位置，不影响已打开的连接。"
                    } else {
                        "没有可用的目标。请先新建文件夹，或选择其他项目。"
                    }),
            )
            .when_some(self.error.clone(), |view, error| {
                view.child(div().text_sm().text_color(cx.theme().danger).child(error))
            })
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap_2()
                    .child(
                        Button::new("move-cancel")
                            .ghost()
                            .label("取消")
                            .disabled(self.busy)
                            .on_click(cx.listener(|this, _, _, cx| {
                                if !this.busy {
                                    cx.emit(MoveFormAction::Cancel);
                                }
                            })),
                    )
                    .child(
                        Button::new("move-save")
                            .primary()
                            .label(if self.busy { "移动中…" } else { "移动" })
                            .disabled(self.busy || !has_target)
                            .on_click(cx.listener(|this, _, _, cx| this.save(cx))),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::{destinations, FolderForm, FolderFormAction, MoveForm, MoveFormAction};
    use crate::config::{Folder, HostLibrary, HostNodeId, SessionConfig};
    use gpui_kit::{AppContext, TestAppContext};
    use std::{cell::RefCell, rc::Rc};

    fn library() -> HostLibrary {
        HostLibrary {
            folders: vec![
                Folder {
                    id: "a".into(),
                    name: "A".into(),
                    parent_id: None,
                },
                Folder {
                    id: "b".into(),
                    name: "B".into(),
                    parent_id: Some("a".into()),
                },
                Folder {
                    id: "c".into(),
                    name: "C".into(),
                    parent_id: None,
                },
            ],
            sessions: vec![SessionConfig {
                id: "host".into(),
                folder_id: Some("b".into()),
                ..Default::default()
            }],
        }
    }

    #[test]
    fn move_destinations_exclude_noop_self_and_descendants() {
        let library = library();
        let folder = destinations(&library, &HostNodeId::Folder("a".into()));
        assert_eq!(
            folder
                .iter()
                .map(|target| target.id.as_deref())
                .collect::<Vec<_>>(),
            [Some("c")]
        );
        let host = destinations(&library, &HostNodeId::Host("host".into()));
        assert_eq!(
            host.iter()
                .map(|target| target.id.as_deref())
                .collect::<Vec<_>>(),
            [None, Some("a"), Some("c")]
        );
        assert!(destinations(&library, &HostNodeId::Host("missing".into())).is_empty());
    }

    #[gpui::test]
    fn folder_name_validation_and_busy_guard_emit_one_trimmed_save(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (window, form) = cx.update(|cx| {
            gpui_kit::open_window(Default::default(), cx, |window, cx| {
                cx.new(|cx| FolderForm::new("  ", window, cx))
            })
            .unwrap()
        });
        let actions = Rc::new(RefCell::new(Vec::new()));
        let collected = actions.clone();
        let _subscription = cx.update(|cx| {
            cx.subscribe(&form, move |_, action: &FolderFormAction, _| {
                collected.borrow_mut().push(action.clone())
            })
        });
        cx.update_window(window, |_, window, cx| {
            form.update(cx, |form, cx| {
                form.save(cx);
                assert!(form.error.is_some());
                assert!(!form.busy);
                form.name.update(cx, |input, cx| {
                    input.set_value("  工作 / Work  ", window, cx)
                });
                form.save(cx);
                form.save(cx);
            })
        })
        .unwrap();
        cx.run_until_parked();
        assert!(
            matches!(actions.borrow().as_slice(), [FolderFormAction::Save(name)] if name == "工作 / Work")
        );
    }

    #[gpui::test]
    fn move_form_emits_stable_destination_without_changing_library(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let library = library();
        let (window, form) = cx.update(|cx| {
            gpui_kit::open_window(Default::default(), cx, |window, cx| {
                cx.new(|cx| MoveForm::new(&library, HostNodeId::Host("host".into()), window, cx))
            })
            .unwrap()
        });
        let actions = Rc::new(RefCell::new(Vec::new()));
        let collected = actions.clone();
        let _subscription = cx.update(|cx| {
            cx.subscribe(&form, move |_, action: &MoveFormAction, _| {
                collected.borrow_mut().push(action.clone())
            })
        });
        cx.update_window(window, |_, _, cx| {
            form.update(cx, |form, cx| {
                form.save(cx);
                form.save(cx);
                assert_eq!(form.library.sessions[0].folder_id.as_deref(), Some("b"));
            })
        })
        .unwrap();
        cx.run_until_parked();
        assert!(
            matches!(actions.borrow().as_slice(), [MoveFormAction::Move { node: HostNodeId::Host(id), target: None }] if id == "host")
        );
    }
}
