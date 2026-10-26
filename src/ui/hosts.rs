use std::collections::HashSet;

use gpui_kit::component::{
    button::{Button, ButtonVariants},
    input::{Input, InputEvent, InputState},
    list::ListItem,
    menu::{ContextMenuExt, PopupMenuItem},
    tooltip::Tooltip,
    ActiveTheme, Disableable, Icon, IconName, Sizable,
};
use gpui_kit::{prelude::*, *};

use crate::config::{HostLibrary, HostNodeId};
use crate::shortcuts::{self, MoveHostNode};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum HostId {
    Saved(String),
}

#[derive(Clone, Debug)]
pub enum HostAction {
    Connect(HostId),
    Edit(String),
    Duplicate(String),
    Delete(String),
    NewHost(Option<String>),
    CreateFolder(Option<String>),
    RenameFolder(String),
    DeleteFolder(String),
    ChooseMove(HostNodeId),
    Move {
        node: HostNodeId,
        target: Option<String>,
    },
    ImportSshConfig,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct TreeRow {
    node: HostNodeId,
    label: String,
    path: String,
    depth: usize,
    expanded: bool,
    parent: Option<String>,
}

fn visible_rows(library: &HostLibrary, collapsed: &HashSet<String>, query: &str) -> Vec<TreeRow> {
    fn children(
        library: &HostLibrary,
        collapsed: &HashSet<String>,
        query: &str,
        parent: Option<&str>,
        depth: usize,
        path: &str,
    ) -> Vec<TreeRow> {
        let mut rows = Vec::new();
        let mut folders: Vec<_> = library
            .folders
            .iter()
            .filter(|folder| folder.parent_id.as_deref() == parent)
            .collect();
        folders.sort_by_key(|folder| (folder.name.to_lowercase(), folder.id.clone()));
        for folder in folders {
            let full_path = if path.is_empty() {
                folder.name.clone()
            } else {
                format!("{path} / {}", folder.name)
            };
            let expanded = !query.is_empty() || !collapsed.contains(&folder.id);
            let descendants = if expanded {
                children(
                    library,
                    collapsed,
                    query,
                    Some(&folder.id),
                    depth + 1,
                    &full_path,
                )
            } else {
                Vec::new()
            };
            if query.is_empty()
                || full_path.to_lowercase().contains(query)
                || !descendants.is_empty()
            {
                rows.push(TreeRow {
                    node: HostNodeId::Folder(folder.id.clone()),
                    label: folder.name.clone(),
                    path: full_path,
                    depth,
                    expanded,
                    parent: folder.parent_id.clone(),
                });
                rows.extend(descendants);
            }
        }
        let mut profiles: Vec<_> = library
            .sessions
            .iter()
            .filter(|profile| profile.folder_id.as_deref() == parent)
            .filter(|profile| {
                format!(
                    "{} {} {} {}",
                    path, profile.name, profile.host, profile.username
                )
                .to_lowercase()
                .contains(query)
            })
            .collect();
        profiles.sort_by_key(|profile| (profile.name.to_lowercase(), profile.id.clone()));
        rows.extend(profiles.into_iter().map(|profile| TreeRow {
            node: HostNodeId::Host(profile.id.clone()),
            label: profile.name.clone(),
            path: format!(
                "{}{}{} · {}@{}:{}",
                path,
                if path.is_empty() { "" } else { " / " },
                profile.name,
                profile.username,
                profile.host,
                profile.port
            ),
            depth,
            expanded: false,
            parent: profile.folder_id.clone(),
        }));
        rows
    }
    children(library, collapsed, query, None, 0, "")
}

#[derive(Clone)]
struct HostDrag {
    node: HostNodeId,
}

struct HostDragPreview {
    label: String,
    folder: bool,
}

impl Render for HostDragPreview {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .gap_2()
            .px_3()
            .py_2()
            .max_w(px(260.))
            .rounded_md()
            .border_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().popover)
            .text_color(cx.theme().foreground)
            .text_sm()
            .shadow_md()
            .child(
                Icon::new(if self.folder {
                    IconName::Folder
                } else {
                    IconName::SquareTerminal
                })
                .size_3p5(),
            )
            .child(div().truncate().child(self.label.clone()))
    }
}

pub struct HostsView {
    library: HostLibrary,
    search: Entity<InputState>,
    active: Option<HostId>,
    selected: Option<HostNodeId>,
    collapsed: HashSet<String>,
    focus: FocusHandle,
    scroll: ScrollHandle,
    busy: bool,
    _subscription: Subscription,
}

impl HostsView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("搜索主机或文件夹"));
        let subscription = cx.subscribe(&search, |_, _, _: &InputEvent, cx| cx.notify());
        Self {
            library: HostLibrary::default(),
            search,
            active: None,
            selected: None,
            collapsed: HashSet::new(),
            focus: cx.focus_handle().tab_stop(true),
            scroll: ScrollHandle::new(),
            busy: false,
            _subscription: subscription,
        }
    }

    pub fn set_library(&mut self, library: HostLibrary, cx: &mut Context<Self>) {
        let rows = self.rows(cx);
        let anchor = rows.get(self.scroll.top_item()).map(|row| row.node.clone());
        self.library = library;
        self.collapsed
            .retain(|id| self.library.folders.iter().any(|folder| &folder.id == id));
        if self.selected.as_ref().is_some_and(|id| !self.contains(id)) {
            self.selected = None;
        }
        if self.active.as_ref().is_some_and(|id| match id {
            HostId::Saved(id) => !self
                .library
                .sessions
                .iter()
                .any(|profile| &profile.id == id),
        }) {
            self.active = None;
        }
        if let Some(index) =
            anchor.and_then(|node| self.rows(cx).iter().position(|row| row.node == node))
        {
            self.scroll.scroll_to_top_of_item(index);
        }
        cx.notify();
    }

    pub fn focus_search(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.search.focus_handle(cx).focus(window, cx);
    }

    fn move_selected(&mut self, _: &MoveHostNode, window: &mut Window, cx: &mut Context<Self>) {
        if !self.busy && self.focus.is_focused(window) {
            if let Some(node) = self.selected.clone() {
                cx.emit(HostAction::ChooseMove(node));
            }
        }
    }

    pub fn set_busy(&mut self, busy: bool, cx: &mut Context<Self>) {
        self.busy = busy;
        cx.notify();
    }

    #[cfg(test)]
    pub(crate) fn is_busy_for_test(&self) -> bool {
        self.busy
    }

    #[cfg(test)]
    pub(crate) fn active_for_test(&self) -> Option<HostId> {
        self.active.clone()
    }

    pub fn select(&mut self, id: Option<HostId>, cx: &mut Context<Self>) {
        self.active = id;
        cx.notify();
    }

    fn contains(&self, node: &HostNodeId) -> bool {
        match node {
            HostNodeId::Host(id) => self
                .library
                .sessions
                .iter()
                .any(|profile| &profile.id == id),
            HostNodeId::Folder(id) => self.library.folders.iter().any(|folder| &folder.id == id),
        }
    }

    fn rows(&self, cx: &App) -> Vec<TreeRow> {
        visible_rows(
            &self.library,
            &self.collapsed,
            &self.search.read(cx).value().trim().to_lowercase(),
        )
    }

    fn can_move(&self, node: &HostNodeId, target: Option<&str>) -> bool {
        let current = match node {
            HostNodeId::Host(id) => self
                .library
                .sessions
                .iter()
                .find(|host| &host.id == id)
                .map(|host| host.folder_id.as_deref()),
            HostNodeId::Folder(id) => self
                .library
                .folders
                .iter()
                .find(|folder| &folder.id == id)
                .map(|folder| folder.parent_id.as_deref()),
        };
        !self.busy
            && current.is_some_and(|current| current != target)
            && self
                .library
                .clone()
                .move_node(node, target.map(str::to_owned))
                .is_ok()
    }

    fn emit_action(&self, action: HostAction, cx: &mut Context<Self>) {
        if !self.busy {
            cx.emit(action);
        }
    }

    fn drop_node(&self, drag: &HostDrag, target: Option<String>, cx: &mut Context<Self>) {
        if self.can_move(&drag.node, target.as_deref()) {
            cx.emit(HostAction::Move {
                node: drag.node.clone(),
                target,
            });
            cx.stop_propagation();
        }
    }

    fn click_row(
        &mut self,
        node: HostNodeId,
        click_count: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.busy || !self.contains(&node) {
            return;
        }
        self.focus.focus(window, cx);
        self.selected = Some(node.clone());
        match node {
            HostNodeId::Folder(id) if click_count == 1 => self.toggle_folder(&id, cx),
            HostNodeId::Host(id) if click_count == 2 => {
                cx.emit(HostAction::Connect(HostId::Saved(id)));
            }
            _ => {}
        }
        cx.notify();
    }

    fn toggle_folder(&mut self, id: &str, cx: &App) {
        if !self.search.read(cx).value().trim().is_empty() {
            return;
        }
        if !self.collapsed.remove(id) {
            self.collapsed.insert(id.to_owned());
        }
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy || !self.focus.is_focused(window) || event.keystroke.modifiers.modified() {
            return;
        }
        let rows = self.rows(cx);
        if rows.is_empty() {
            return;
        }
        let current = self
            .selected
            .as_ref()
            .and_then(|node| rows.iter().position(|row| &row.node == node));
        match event.keystroke.key.as_str() {
            "down" => {
                let index = current.map_or(0, |index| (index + 1).min(rows.len() - 1));
                self.selected = Some(rows[index].node.clone());
                self.scroll.scroll_to_item(index);
            }
            "up" => {
                let index = current.map_or(rows.len() - 1, |index| index.saturating_sub(1));
                self.selected = Some(rows[index].node.clone());
                self.scroll.scroll_to_item(index);
            }
            "home" => {
                self.selected = Some(rows[0].node.clone());
                self.scroll.scroll_to_item(0);
            }
            "end" => {
                self.selected = Some(rows[rows.len() - 1].node.clone());
                self.scroll.scroll_to_item(rows.len() - 1);
            }
            "right" => {
                if let Some(index) = current {
                    if let HostNodeId::Folder(id) = &rows[index].node {
                        if !rows[index].expanded {
                            self.collapsed.remove(id);
                        } else if let Some(child) = rows
                            .get(index + 1)
                            .filter(|child| child.depth > rows[index].depth)
                        {
                            self.selected = Some(child.node.clone());
                            self.scroll.scroll_to_item(index + 1);
                        }
                    }
                }
            }
            "left" => {
                if let Some(index) = current {
                    let row = &rows[index];
                    if matches!(row.node, HostNodeId::Folder(_))
                        && row.expanded
                        && self.search.read(cx).value().trim().is_empty()
                    {
                        if let HostNodeId::Folder(id) = &row.node {
                            self.collapsed.insert(id.clone());
                        }
                    } else if let Some(parent) = &row.parent {
                        self.selected = Some(HostNodeId::Folder(parent.clone()));
                        if let Some(index) = rows
                            .iter()
                            .position(|row| row.node == HostNodeId::Folder(parent.clone()))
                        {
                            self.scroll.scroll_to_item(index);
                        }
                    }
                }
            }
            "enter" => {
                if let Some(index) = current {
                    match &rows[index].node {
                        HostNodeId::Host(id) => {
                            cx.emit(HostAction::Connect(HostId::Saved(id.clone())));
                        }
                        HostNodeId::Folder(id) => self.toggle_folder(id, cx),
                    }
                }
            }
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }

    fn tree_item(&self, row: &TreeRow) -> ListItem {
        let folder = matches!(row.node, HostNodeId::Folder(_));
        let selector = format!("host-tree-{}", node_key(&row.node));
        let selected = self.selected.as_ref() == Some(&row.node);
        ListItem::new(SharedString::from(selector.clone()))
            .accessibility_id(selector.clone())
            .debug_selector(move || selector.clone())
            .role(accesskit::Role::TreeItem)
            .aria_selected(selected)
            .aria_level(row.depth + 1)
            .when(folder, |item| item.aria_expanded(row.expanded))
            .when(selected, |item| item.aria_active_descendant())
            .selected(selected)
            .disabled(self.busy)
            .accessibility_label(format!(
                "{}{}",
                row.path,
                if folder {
                    if row.expanded {
                        "，文件夹，已展开"
                    } else {
                        "，文件夹，已折叠"
                    }
                } else {
                    "，主机，双击或按 Enter 连接"
                }
            ))
    }

    fn tree_container(&self, cx: &App) -> Stateful<Div> {
        div()
            .id("host-tree-scroll")
            .accessibility_id("host-tree")
            .role(accesskit::Role::Tree)
            .aria_label("主机文件夹树")
            .aria_description(if self.busy {
                "正在更新主机库，暂不可操作"
            } else {
                "方向键浏览或展开文件夹，Enter 连接主机，使用移动快捷键更改位置"
            })
            .aria_keyshortcuts(shortcuts::accessibility_hint("move_host_node", cx))
            .key_context("HostsTree")
            .track_focus(&self.focus)
    }

    fn row(&self, row: TreeRow, cx: &Context<Self>) -> AnyElement {
        let folder = matches!(row.node, HostNodeId::Folder(_));
        let node = row.node.clone();
        let click_node = node.clone();
        let owner = cx.entity().downgrade();
        let preview_owner = owner.clone();
        let move_hint = shortcuts::hint("move_host_node", cx);
        let tooltip = if move_hint.is_empty() {
            row.path.clone()
        } else {
            format!("{} · 移动 {move_hint}", row.path)
        };
        let mut item = self.tree_item(&row)
            .h(px(32.))
            .min_h(px(32.))
            .pl(px(8. + row.depth as f32 * 16.))
            .pr_2()
            .py_0()
            .rounded_sm()
            .text_sm()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .min_w_0()
                    .w_full()
                    .child(
                        div().flex().items_center().w_3().flex_shrink_0().when(folder, |view| {
                            view.child(
                                Icon::new(if row.expanded {
                                    IconName::ChevronDown
                                } else {
                                    IconName::ChevronRight
                                })
                                .size_3()
                                .text_color(cx.theme().muted_foreground),
                            )
                        }),
                    )
                    .child(
                        Icon::new(if folder {
                            if row.expanded { IconName::FolderOpen } else { IconName::Folder }
                        } else {
                            IconName::SquareTerminal
                        })
                        .size_3p5()
                        .text_color(cx.theme().muted_foreground),
                    )
                    .child(div().flex_1().min_w_0().truncate().child(row.label))
                    .when(
                        matches!((&node, &self.active), (HostNodeId::Host(id), Some(HostId::Saved(active))) if id == active),
                        |view| view.child(div().w(px(5.)).h(px(5.)).rounded_full().bg(cx.theme().primary)),
                    ),
            )
            .tooltip(move |window, cx| Tooltip::new(tooltip.clone()).build(window, cx))
            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                this.click_row(click_node.clone(), event.click_count(), window, cx);
            }))
            .when(!self.busy, |item| {
                item.on_drag(HostDrag { node: node.clone() }, move |drag, _, _, cx| {
                    let label = preview_owner
                        .upgrade()
                        .and_then(|owner| {
                            let state = owner.read(cx);
                            match &drag.node {
                                HostNodeId::Host(id) => state.library.sessions.iter().find(|p| &p.id == id).map(|p| p.name.clone()),
                                HostNodeId::Folder(id) => state.library.folders.iter().find(|f| &f.id == id).map(|f| f.name.clone()),
                            }
                        })
                        .unwrap_or_default();
                    cx.new(|_| HostDragPreview {
                        label,
                        folder: matches!(drag.node, HostNodeId::Folder(_)),
                    })
                })
            });
        if let HostNodeId::Folder(id) = &node {
            let can_owner = owner.clone();
            let can_target = id.clone();
            let style_owner = owner.clone();
            let style_target = id.clone();
            let drop_target = id.clone();
            item = item
                .can_drop(move |value, _, cx| {
                    value.downcast_ref::<HostDrag>().is_some_and(|drag| {
                        can_owner.upgrade().is_some_and(|owner| {
                            owner.read(cx).can_move(&drag.node, Some(&can_target))
                        })
                    })
                })
                .drag_over::<HostDrag>(move |style, drag, _, cx| {
                    if style_owner.upgrade().is_some_and(|owner| {
                        owner.read(cx).can_move(&drag.node, Some(&style_target))
                    }) {
                        style.bg(cx.theme().selection.opacity(0.35))
                    } else {
                        style
                    }
                })
                .on_drop(cx.listener(move |this, drag: &HostDrag, _, cx| {
                    this.drop_node(drag, Some(drop_target.clone()), cx);
                }));
        }
        let busy = self.busy;
        let can_delete = match &node {
            HostNodeId::Folder(id) => self.library.clone().delete_empty_folder(id).is_ok(),
            HostNodeId::Host(_) => true,
        };
        item.context_menu(move |mut menu, _, _| {
            match &node {
                HostNodeId::Host(id) => {
                    menu = menu
                        .item(
                            action_item(
                                &owner,
                                "连接主机",
                                HostAction::Connect(HostId::Saved(id.clone())),
                            )
                            .disabled(busy),
                        )
                        .item(
                            action_item(&owner, "编辑主机", HostAction::Edit(id.clone()))
                                .disabled(busy),
                        )
                        .item(
                            action_item(&owner, "复制主机", HostAction::Duplicate(id.clone()))
                                .disabled(busy),
                        );
                }
                HostNodeId::Folder(id) => {
                    menu = menu
                        .item(
                            action_item(&owner, "新建主机", HostAction::NewHost(Some(id.clone())))
                                .disabled(busy),
                        )
                        .item(
                            action_item(
                                &owner,
                                "新建子文件夹",
                                HostAction::CreateFolder(Some(id.clone())),
                            )
                            .disabled(busy),
                        )
                        .item(
                            action_item(
                                &owner,
                                "重命名文件夹",
                                HostAction::RenameFolder(id.clone()),
                            )
                            .disabled(busy),
                        );
                }
            }
            menu = menu.separator().item(
                action_item(&owner, "移动到…", HostAction::ChooseMove(node.clone())).disabled(busy),
            );
            match &node {
                HostNodeId::Host(id) => menu.item(
                    action_item(&owner, "删除主机", HostAction::Delete(id.clone())).disabled(busy),
                ),
                HostNodeId::Folder(id) => menu.item(
                    action_item(
                        &owner,
                        if can_delete {
                            "删除空文件夹"
                        } else {
                            "文件夹非空，不能删除"
                        },
                        HostAction::DeleteFolder(id.clone()),
                    )
                    .disabled(busy || !can_delete),
                ),
            }
        })
        .into_any_element()
    }
}

fn node_key(node: &HostNodeId) -> String {
    match node {
        HostNodeId::Folder(id) => format!("folder-{id}"),
        HostNodeId::Host(id) => format!("host-{id}"),
    }
}

fn action_item(
    owner: &WeakEntity<HostsView>,
    label: &'static str,
    action: HostAction,
) -> PopupMenuItem {
    let owner = owner.clone();
    PopupMenuItem::new(label).on_click(move |_, _, cx| {
        let _ = owner.update(cx, |this, cx| this.emit_action(action.clone(), cx));
    })
}

impl EventEmitter<HostAction> for HostsView {}

impl Render for HostsView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let rows = self.rows(cx);
        let empty = rows.is_empty();
        let library_empty = self.library.folders.is_empty() && self.library.sessions.is_empty();
        let owner = cx.entity().downgrade();
        let can_owner = owner.clone();
        let style_owner = owner.clone();
        let busy = self.busy;
        div()
            .id("hosts-sidebar")
            .flex()
            .flex_col()
            .w_full()
            .h_full()
            .min_w_0()
            .bg(cx.theme().sidebar)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .px_3()
                    .pt_3()
                    .pb_2()
                    .flex_shrink_0()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child("主机"),
                            )
                            .child(
                                div()
                                    .flex()
                                    .gap_1()
                                    .child(
                                        Button::new("host-new-folder")
                                            .ghost()
                                            .xsmall()
                                            .icon(IconName::Folder)
                                            .tooltip("新建文件夹")
                                            .disabled(busy)
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.emit_action(HostAction::CreateFolder(None), cx)
                                            })),
                                    )
                                    .child(
                                        Button::new("host-new-profile")
                                            .ghost()
                                            .xsmall()
                                            .icon(IconName::Plus)
                                            .tooltip("新建主机")
                                            .disabled(busy)
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.emit_action(HostAction::NewHost(None), cx)
                                            })),
                                    ),
                            ),
                    )
                    .child(
                        Input::new(&self.search)
                            .id("host-search")
                            .cleanable(true)
                            .w_full(),
                    )
                    .child(
                        Button::new("host-import-ssh-config")
                            .debug_selector(|| "host-import-ssh-config".into())
                            .ghost()
                            .small()
                            .label("从 SSH Config 导入")
                            .w_full()
                            .disabled(busy)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.emit_action(HostAction::ImportSshConfig, cx)
                            })),
                    ),
            )
            .child(
                ListItem::new("host-tree-root")
                    .debug_selector(|| "host-tree-root".into())
                    .h(px(30.))
                    .mx_2()
                    .px_2()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .accessibility_label("根目录，可将主机或文件夹拖到此处")
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(Icon::new(IconName::Folder).size_3())
                            .child("根目录"),
                    )
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.selected = None;
                        this.focus.focus(window, cx);
                        cx.notify();
                    }))
                    .can_drop(move |value, _, cx| {
                        value.downcast_ref::<HostDrag>().is_some_and(|drag| {
                            can_owner
                                .upgrade()
                                .is_some_and(|owner| owner.read(cx).can_move(&drag.node, None))
                        })
                    })
                    .drag_over::<HostDrag>(move |style, drag, _, cx| {
                        if style_owner
                            .upgrade()
                            .is_some_and(|owner| owner.read(cx).can_move(&drag.node, None))
                        {
                            style.bg(cx.theme().selection.opacity(0.35))
                        } else {
                            style
                        }
                    })
                    .on_drop(
                        cx.listener(|this, drag: &HostDrag, _, cx| this.drop_node(drag, None, cx)),
                    )
                    .context_menu(move |menu, _, _| {
                        menu.item(
                            action_item(&owner, "新建主机", HostAction::NewHost(None))
                                .disabled(busy),
                        )
                        .item(
                            action_item(&owner, "新建文件夹", HostAction::CreateFolder(None))
                                .disabled(busy),
                        )
                    }),
            )
            .child(
                self.tree_container(cx)
                    .on_action(cx.listener(Self::move_selected))
                    .on_key_down(cx.listener(Self::on_key_down))
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .px_2()
                    .pb_2()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .children(rows.into_iter().map(|row| self.row(row, cx)))
                    .when(empty, |view| {
                        view.child(
                            div()
                                .p_3()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child(if library_empty {
                                    "新建主机或导入 SSH Config，开始整理连接。"
                                } else {
                                    "没有匹配的主机或文件夹。"
                                }),
                        )
                    }),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::{node_key, visible_rows, HostAction, HostDrag, HostId, HostsView};
    use crate::config::{Folder, HostLibrary, HostNodeId, SessionConfig};
    use gpui_kit::{AppContext, Focusable, KeyDownEvent, Keystroke, TestAppContext};
    use std::{cell::RefCell, collections::HashSet, rc::Rc};

    fn library() -> HostLibrary {
        HostLibrary {
            folders: vec![
                Folder {
                    id: "prod".into(),
                    name: "生产".into(),
                    parent_id: None,
                },
                Folder {
                    id: "db".into(),
                    name: "数据库".into(),
                    parent_id: Some("prod".into()),
                },
                Folder {
                    id: "empty".into(),
                    name: "Empty".into(),
                    parent_id: None,
                },
            ],
            sessions: vec![
                SessionConfig {
                    id: "nested".into(),
                    name: "Database".into(),
                    host: "db.test.invalid".into(),
                    username: "Deploy".into(),
                    folder_id: Some("db".into()),
                    ..Default::default()
                },
                SessionConfig {
                    id: "root".into(),
                    name: "Root shell".into(),
                    host: "root.test.invalid".into(),
                    username: "tester".into(),
                    ..Default::default()
                },
            ],
        }
    }

    #[test]
    fn empty_folders_and_nested_hosts_have_stable_folder_first_rows() {
        let rows = visible_rows(&library(), &HashSet::new(), "");
        assert_eq!(
            rows.iter()
                .map(|row| node_key(&row.node))
                .collect::<Vec<_>>(),
            [
                "folder-empty",
                "folder-prod",
                "folder-db",
                "host-nested",
                "host-root"
            ]
        );
        assert_eq!(rows[3].depth, 2);
        assert!(rows[0].expanded);
    }

    #[test]
    fn search_retains_ancestors_without_mutating_collapsed_state() {
        let collapsed = HashSet::from(["prod".into(), "db".into()]);
        assert_eq!(visible_rows(&library(), &collapsed, "").len(), 3);
        for query in ["db.test", "deploy", "数据库", "生产 / 数据库"] {
            let rows = visible_rows(&library(), &collapsed, query);
            assert_eq!(
                rows.iter()
                    .map(|row| node_key(&row.node))
                    .collect::<Vec<_>>(),
                ["folder-prod", "folder-db", "host-nested"]
            );
        }
        assert_eq!(collapsed.len(), 2);
        assert_eq!(visible_rows(&library(), &collapsed, "").len(), 3);
        assert!(visible_rows(&library(), &collapsed, "missing").is_empty());
    }

    #[gpui::test]
    fn selection_keyboard_connection_and_moves_are_independent(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (window, hosts) = cx.update(|cx| {
            gpui_kit::open_window(Default::default(), cx, |window, cx| {
                cx.new(|cx| HostsView::new(window, cx))
            })
            .unwrap()
        });
        let actions = Rc::new(RefCell::new(Vec::new()));
        let collected = actions.clone();
        let _subscription = cx.update(|cx| {
            cx.subscribe(&hosts, move |_, action: &HostAction, _| {
                collected.borrow_mut().push(action.clone())
            })
        });
        cx.update_window(window, |_, window, cx| {
            hosts.update(cx, |hosts, cx| {
                hosts.set_library(library(), cx);
                hosts.select(Some(HostId::Saved("root".into())), cx);
                hosts.click_row(HostNodeId::Host("nested".into()), 1, window, cx);
                assert_eq!(hosts.active, Some(HostId::Saved("root".into())));
                assert_eq!(hosts.selected, Some(HostNodeId::Host("nested".into())));
                hosts.on_key_down(
                    &KeyDownEvent {
                        keystroke: Keystroke::parse("enter").unwrap(),
                        is_held: false,
                        prefer_character_input: false,
                    },
                    window,
                    cx,
                );
                hosts.drop_node(
                    &HostDrag {
                        node: HostNodeId::Host("nested".into()),
                    },
                    None,
                    cx,
                );
                hosts.drop_node(
                    &HostDrag {
                        node: HostNodeId::Folder("prod".into()),
                    },
                    Some("db".into()),
                    cx,
                );
                hosts.drop_node(
                    &HostDrag {
                        node: HostNodeId::Folder("prod".into()),
                    },
                    Some("prod".into()),
                    cx,
                );
                hosts.set_busy(true, cx);
                hosts.click_row(HostNodeId::Host("root".into()), 2, window, cx);
                hosts.drop_node(
                    &HostDrag {
                        node: HostNodeId::Host("nested".into()),
                    },
                    Some("empty".into()),
                    cx,
                );
                assert_eq!(hosts.library.sessions[0].folder_id.as_deref(), Some("db"));
            });
        })
        .unwrap();
        cx.run_until_parked();
        let actions = actions.borrow();
        assert_eq!(actions.len(), 2);
        assert!(matches!(&actions[0], HostAction::Connect(HostId::Saved(id)) if id == "nested"));
        assert!(
            matches!(&actions[1], HostAction::Move { node: HostNodeId::Host(id), target: None } if id == "nested")
        );
    }

    #[gpui::test]
    fn pointer_drag_moves_once_without_connecting_and_rejects_cycles(cx: &mut TestAppContext) {
        use gpui_kit::{point, px, MouseButton};

        cx.update(gpui_kit::init);
        let (hosts, cx) = cx.add_window_view(HostsView::new);
        let actions = Rc::new(RefCell::new(Vec::new()));
        let collected = actions.clone();
        let _subscription = cx.update(|_, cx| {
            cx.subscribe(&hosts, move |_, action: &HostAction, _| {
                collected.borrow_mut().push(action.clone());
            })
        });
        cx.update(|window, cx| {
            hosts.update(cx, |hosts, cx| hosts.set_library(library(), cx));
            window.draw(cx).clear(cx);
        });
        let source = cx
            .debug_bounds("host-tree-host-nested")
            .expect("nested host")
            .center();
        let target = cx
            .debug_bounds("host-tree-folder-empty")
            .expect("empty folder")
            .center();
        cx.simulate_mouse_down(source, MouseButton::Left, Default::default());
        cx.simulate_mouse_move(
            source + point(px(16.), px(0.)),
            MouseButton::Left,
            Default::default(),
        );
        cx.simulate_mouse_move(target, MouseButton::Left, Default::default());
        cx.simulate_mouse_up(target, MouseButton::Left, Default::default());
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert!(
            matches!(actions.borrow().as_slice(), [HostAction::Move { node: HostNodeId::Host(id), target: Some(target) }] if id == "nested" && target == "empty")
        );

        let source = cx.debug_bounds("host-tree-folder-prod").unwrap().center();
        let target = cx.debug_bounds("host-tree-folder-db").unwrap().center();
        cx.simulate_mouse_down(source, MouseButton::Left, Default::default());
        cx.simulate_mouse_move(
            source + point(px(16.), px(0.)),
            MouseButton::Left,
            Default::default(),
        );
        cx.simulate_mouse_move(target, MouseButton::Left, Default::default());
        cx.simulate_mouse_up(target, MouseButton::Left, Default::default());
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert_eq!(actions.borrow().len(), 1);
        for (source, target, expected) in [
            ("host-tree-folder-db", "host-tree-folder-empty", 2),
            ("host-tree-host-nested", "host-tree-root", 3),
        ] {
            let source = cx.debug_bounds(source).unwrap().center();
            let target = cx.debug_bounds(target).unwrap().center();
            cx.simulate_mouse_down(source, MouseButton::Left, Default::default());
            cx.simulate_mouse_move(
                source + point(px(16.), px(0.)),
                MouseButton::Left,
                Default::default(),
            );
            cx.simulate_mouse_move(target, MouseButton::Left, Default::default());
            cx.simulate_mouse_up(target, MouseButton::Left, Default::default());
            cx.update(|window, cx| window.draw(cx).clear(cx));
            assert_eq!(actions.borrow().len(), expected);
        }
        assert!(
            matches!(&actions.borrow()[1], HostAction::Move { node: HostNodeId::Folder(id), target: Some(target) } if id == "db" && target == "empty")
        );
        assert!(
            matches!(&actions.borrow()[2], HostAction::Move { node: HostNodeId::Host(id), target: None } if id == "nested")
        );
        cx.update(|_, cx| {
            hosts.update(cx, |hosts, cx| {
                assert!(!hosts.collapsed.contains("prod"));
                hosts.set_busy(true, cx);
            })
        });
        let source = cx.debug_bounds("host-tree-host-nested").unwrap().center();
        let target = cx.debug_bounds("host-tree-root").unwrap().center();
        cx.simulate_mouse_down(source, MouseButton::Left, Default::default());
        cx.simulate_mouse_move(
            source + point(px(16.), px(0.)),
            MouseButton::Left,
            Default::default(),
        );
        cx.simulate_mouse_move(target, MouseButton::Left, Default::default());
        cx.simulate_mouse_up(target, MouseButton::Left, Default::default());
        assert_eq!(actions.borrow().len(), 3);
    }

    #[gpui::test]
    fn raw_keyboard_navigation_preserves_search_expansion_and_opens_move_dialog(
        cx: &mut TestAppContext,
    ) {
        use gpui_kit::PlatformInput;

        cx.update(|cx| {
            gpui_kit::init(cx);
            crate::shortcuts::init(
                crate::config::ShortcutConfig::defaults("/fixture/keybindings.toml".into()),
                cx,
            );
        });
        let (hosts, cx) = cx.add_window_view(HostsView::new);
        let actions = Rc::new(RefCell::new(Vec::new()));
        let collected = actions.clone();
        let _subscription = cx.update(|_, cx| {
            cx.subscribe(&hosts, move |_, action: &HostAction, _| {
                collected.borrow_mut().push(action.clone())
            })
        });
        cx.update(|window, cx| {
            hosts.update(cx, |hosts, cx| {
                hosts.set_library(library(), cx);
                hosts.selected = Some(HostNodeId::Folder("prod".into()));
                hosts.collapsed.insert("prod".into());
                hosts.focus.focus(window, cx);
            });
            window.draw(cx).clear(cx);
        });
        for key in [
            "right",
            "right",
            "right",
            "enter",
            if cfg!(target_os = "macos") {
                "cmd-shift-m"
            } else {
                "ctrl-alt-shift-m"
            },
        ] {
            cx.update(|window, cx| {
                window.dispatch_event(
                    PlatformInput::KeyDown(KeyDownEvent {
                        keystroke: Keystroke::parse(key).unwrap(),
                        is_held: false,
                        prefer_character_input: false,
                    }),
                    cx,
                );
                window.draw(cx).clear(cx);
            });
        }
        assert!(
            matches!(actions.borrow().as_slice(), [HostAction::Connect(HostId::Saved(id)), HostAction::ChooseMove(HostNodeId::Host(moved))] if id == "nested" && moved == id)
        );
        cx.update(|window, cx| {
            hosts.update(cx, |hosts, cx| {
                hosts.collapsed.insert("prod".into());
                hosts.collapsed.insert("db".into());
                hosts
                    .search
                    .update(cx, |input, cx| input.set_value("deploy", window, cx));
                hosts.click_row(HostNodeId::Folder("prod".into()), 1, window, cx);
                hosts.on_key_down(
                    &KeyDownEvent {
                        keystroke: Keystroke::parse("left").unwrap(),
                        is_held: false,
                        prefer_character_input: false,
                    },
                    window,
                    cx,
                );
                assert!(hosts.collapsed.contains("prod"));
                assert!(hosts.collapsed.contains("db"));
                assert_eq!(hosts.rows(cx).len(), 3);
                hosts
                    .search
                    .update(cx, |input, cx| input.set_value("", window, cx));
                assert_eq!(hosts.rows(cx).len(), 3);
            })
        });
    }

    #[gpui::test]
    fn root_tab_enters_tree_from_search_and_shift_tab_returns(cx: &mut TestAppContext) {
        use gpui_kit::{PlatformInput, Window};

        cx.update(gpui_kit::init);
        let (window, hosts) = cx.update(|cx| {
            gpui_kit::open_window(Default::default(), cx, |window, cx| {
                cx.new(|cx| HostsView::new(window, cx))
            })
            .unwrap()
        });
        let raw_key = |window: &mut Window, cx: &mut gpui_kit::App, key: &str| {
            window.dispatch_event(
                PlatformInput::KeyDown(KeyDownEvent {
                    keystroke: Keystroke::parse(key).unwrap(),
                    is_held: false,
                    prefer_character_input: false,
                }),
                cx,
            );
            window.draw(cx).clear(cx);
        };
        cx.update_window(window, |_, window, cx| {
            hosts.update(cx, |hosts, cx| {
                hosts.set_library(library(), cx);
                hosts.search.read(cx).focus_handle(cx).focus(window, cx);
            });
            window.draw(cx).clear(cx);
            assert!(hosts
                .read(cx)
                .search
                .read(cx)
                .focus_handle(cx)
                .is_focused(window));
            raw_key(window, cx, "tab");
            assert!(!hosts.read(cx).focus.is_focused(window));
            raw_key(window, cx, "tab");
            assert!(
                hosts.read(cx).focus.is_focused(window),
                "tree must be reachable through normal Tab order"
            );
            raw_key(window, cx, "down");
            raw_key(window, cx, "down");
            raw_key(window, cx, "right");
            assert_eq!(
                hosts.read(cx).selected,
                Some(HostNodeId::Folder("db".into()))
            );
            raw_key(window, cx, "shift-tab");
            assert!(!hosts.read(cx).focus.is_focused(window));
            raw_key(window, cx, "shift-tab");
            assert!(hosts
                .read(cx)
                .search
                .read(cx)
                .focus_handle(cx)
                .is_focused(window));
        })
        .unwrap();
    }

    #[gpui::test]
    fn accessible_tree_exposes_stable_rows_and_keyboard_active_descendant(cx: &mut TestAppContext) {
        use gpui_kit::PlatformInput;

        cx.update(gpui_kit::init);
        let (window, hosts) = cx.update(|cx| {
            gpui_kit::open_window(Default::default(), cx, |window, cx| {
                cx.new(|cx| HostsView::new(window, cx))
            })
            .unwrap()
        });
        cx.update_window(window, |_, window, cx| {
            hosts.update(cx, |hosts, cx| {
                hosts.set_library(library(), cx);
                hosts.search.read(cx).focus_handle(cx).focus(window, cx);
            });
            window.draw(cx).clear(cx);
            for key in ["tab", "tab", "down", "down", "right", "right"] {
                window.dispatch_event(
                    PlatformInput::KeyDown(KeyDownEvent {
                        keystroke: Keystroke::parse(key).unwrap(),
                        is_held: false,
                        prefer_character_input: false,
                    }),
                    cx,
                );
                window.draw(cx).clear(cx);
            }
            hosts.update(cx, |hosts, cx| {
                use gpui_kit::{accesskit, Element, IntoElement, RenderOnce};

                let tree = hosts.tree_container(cx);
                assert_eq!(tree.a11y_role(), Some(accesskit::Role::Tree));
                assert!(tree.id().is_some());
                let mut tree_node = accesskit::Node::new(accesskit::Role::Tree);
                tree.write_a11y_info(&mut tree_node);
                assert_eq!(tree_node.author_id(), Some("host-tree"));
                assert_eq!(tree_node.label(), Some("主机文件夹树"));
                for row in hosts.rows(cx) {
                    let item = hosts.tree_item(&row).render(window, cx).into_element();
                    assert_eq!(item.a11y_role(), Some(accesskit::Role::TreeItem));
                    let expected_id = format!("host-tree-{}", node_key(&row.node));
                    assert_eq!(
                        item.id(),
                        Some(gpui_kit::ElementId::Name(expected_id.clone().into()))
                    );
                    let mut node = accesskit::Node::new(accesskit::Role::TreeItem);
                    item.write_a11y_info(&mut node);
                    assert_eq!(node.author_id(), Some(expected_id.as_str()));
                    assert!(node.label().unwrap().contains(&row.label));
                    assert_eq!(node.level(), Some(row.depth + 1));
                    assert_eq!(
                        node.is_selected(),
                        Some(row.node == HostNodeId::Host("nested".into()))
                    );
                    assert_eq!(
                        node.is_expanded(),
                        matches!(row.node, HostNodeId::Folder(_)).then_some(true)
                    );
                }
                assert_eq!(hosts.selected, Some(HostNodeId::Host("nested".into())));
                assert!(hosts.focus.is_focused(window));
            });
            window.dispatch_event(
                PlatformInput::KeyDown(KeyDownEvent {
                    keystroke: Keystroke::parse("left").unwrap(),
                    is_held: false,
                    prefer_character_input: false,
                }),
                cx,
            );
            window.draw(cx).clear(cx);
            window.dispatch_event(
                PlatformInput::KeyDown(KeyDownEvent {
                    keystroke: Keystroke::parse("left").unwrap(),
                    is_held: false,
                    prefer_character_input: false,
                }),
                cx,
            );
            window.draw(cx).clear(cx);
            hosts.update(cx, |hosts, cx| {
                use gpui_kit::{accesskit, Element, IntoElement, RenderOnce};

                let rows = hosts.rows(cx);
                let row = rows
                    .iter()
                    .find(|row| row.node == HostNodeId::Folder("db".into()))
                    .unwrap();
                let item = hosts.tree_item(row).render(window, cx).into_element();
                let mut node = accesskit::Node::new(accesskit::Role::TreeItem);
                item.write_a11y_info(&mut node);
                assert_eq!(node.is_selected(), Some(true));
                assert_eq!(node.is_expanded(), Some(false));
                assert_eq!(node.level(), Some(2));
                assert_eq!(hosts.selected, Some(row.node.clone()));
                hosts.set_busy(true, cx);
                let mut tree = accesskit::Node::new(accesskit::Role::Tree);
                hosts.tree_container(cx).write_a11y_info(&mut tree);
                assert_eq!(tree.description(), Some("正在更新主机库，暂不可操作"));
            });
        })
        .unwrap();
    }

    #[gpui::test]
    fn root_custom_move_binding_replaces_old_key_and_excludes_search(cx: &mut TestAppContext) {
        use gpui_kit::PlatformInput;

        cx.update(|cx| {
            gpui_kit::init(cx);
            crate::shortcuts::init(
                crate::config::ShortcutConfig {
                    keymap: crate::config::EffectiveKeymap::parse(
                        "version = 1\n[bindings]\nmove_host_node = ['f13']\n",
                        "/fixture/keybindings.toml".into(),
                    )
                    .unwrap(),
                    warning: None,
                },
                cx,
            );
        });
        let (window, hosts) = cx.update(|cx| {
            gpui_kit::open_window(Default::default(), cx, |window, cx| {
                cx.new(|cx| HostsView::new(window, cx))
            })
            .unwrap()
        });
        let actions = Rc::new(RefCell::new(Vec::new()));
        let collected = actions.clone();
        let _subscription = cx.update(|cx| {
            cx.subscribe(&hosts, move |_, action: &HostAction, _| {
                collected.borrow_mut().push(action.clone())
            })
        });
        cx.update_window(window, |_, window, cx| {
            hosts.update(cx, |hosts, cx| {
                hosts.set_library(library(), cx);
                hosts.selected = Some(HostNodeId::Host("nested".into()));
                hosts.focus.focus(window, cx);
            });
            let press = |key: &str, window: &mut gpui_kit::Window, cx: &mut gpui_kit::App| {
                window.draw(cx).clear(cx);
                window.dispatch_event(
                    PlatformInput::KeyDown(KeyDownEvent {
                        keystroke: Keystroke::parse(key).unwrap(),
                        is_held: false,
                        prefer_character_input: false,
                    }),
                    cx,
                );
                window.draw(cx).clear(cx);
            };
            press(
                if cfg!(target_os = "macos") {
                    "cmd-shift-m"
                } else {
                    "ctrl-alt-shift-m"
                },
                window,
                cx,
            );
            assert!(actions.borrow().is_empty());
            press("f13", window, cx);
            hosts.update(cx, |hosts, cx| {
                let mut node = gpui_kit::accesskit::Node::new(gpui_kit::accesskit::Role::Tree);
                use gpui_kit::Element;
                hosts.tree_container(cx).write_a11y_info(&mut node);
                assert_eq!(node.keyboard_shortcut(), Some("F13"));
                hosts.focus_search(window, cx);
            });
            press("f13", window, cx);
        })
        .unwrap();
        cx.run_until_parked();
        assert!(
            matches!(actions.borrow().as_slice(), [HostAction::ChooseMove(HostNodeId::Host(id))] if id == "nested")
        );
    }

    #[gpui::test]
    fn refreshed_library_keeps_valid_selection_and_collapse_ids(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (_, hosts) = cx.update(|cx| {
            gpui_kit::open_window(Default::default(), cx, |window, cx| {
                cx.new(|cx| HostsView::new(window, cx))
            })
            .unwrap()
        });
        cx.update(|cx| {
            hosts.update(cx, |hosts, cx| {
                hosts.set_library(library(), cx);
                hosts.selected = Some(HostNodeId::Host("nested".into()));
                hosts.collapsed.insert("prod".into());
                let mut updated = library();
                updated
                    .move_node(&HostNodeId::Host("nested".into()), None)
                    .unwrap();
                updated.rename_folder("prod", "Production".into()).unwrap();
                hosts.set_library(updated, cx);
                assert_eq!(hosts.selected, Some(HostNodeId::Host("nested".into())));
                assert!(hosts.collapsed.contains("prod"));
                let mut removed = library();
                removed.sessions.retain(|profile| profile.id != "nested");
                hosts.set_library(removed, cx);
                assert!(hosts.selected.is_none());
            })
        });
    }
}
