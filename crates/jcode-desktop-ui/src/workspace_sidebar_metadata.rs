//! Only panel metadata visible in the sidebar participates in its retention.
//! Streaming text, selection and transcript scrolling do not change these
//! fields and must not invalidate the entire session list.
use super::*;

pub(super) struct SidebarPanelMetadata {
    pub session_id: String,
    pub title: gpui::SharedString,
    pub working_dir: Option<String>,
    voice: bool,
    activity: Option<gpui::AnyView>,
    todos: Option<(usize, usize)>,
}

impl SidebarPanelMetadata {
    fn from_panel(panel: &Panel) -> Self {
        Self {
            session_id: panel.session_id.clone(),
            title: panel.title.clone(),
            working_dir: panel.working_dir.clone(),
            voice: panel.supports_voice(),
            activity: panel.sidebar_activity(),
            todos: panel.latest_todo_progress(),
        }
    }

    fn matches(&self, panel: &Panel) -> bool {
        self.session_id == panel.session_id
            && self.title == panel.title
            && self.working_dir == panel.working_dir
            && self.voice == panel.supports_voice()
            && self.activity == panel.sidebar_activity()
            && self.todos == panel.latest_todo_progress()
    }

    pub fn supports_voice(&self) -> bool {
        self.voice
    }
    pub fn sidebar_activity(&self) -> Option<gpui::AnyView> {
        self.activity.clone()
    }
    pub fn latest_todo_progress(&self) -> Option<(usize, usize)> {
        self.todos
    }
}

impl Workspace {
    /// Read live panels at the workspace boundary, not inside SidebarChrome.
    /// Reading an entity is a tracked rendering dependency in GPUI. Publishing
    /// only when metadata differs lets text deltas redraw the conversation
    /// without rebuilding sidebar layout, rows and project groups. The map is
    /// bounded to current slots and never retains transcripts or old panels.
    pub(super) fn refresh_sidebar_panel_metadata(&mut self, cx: &mut Context<Self>) {
        let unchanged = self
            .sidebar_panel_metadata
            .as_ref()
            .is_some_and(|metadata| {
                let metadata = metadata.read(cx);
                metadata.len() == self.slots.len()
                    && self.slots.iter().all(|slot| {
                        metadata
                            .get(&slot.panel.entity_id())
                            .is_some_and(|old| old.matches(slot.panel.read(cx)))
                    })
            });
        if unchanged {
            return;
        }
        let metadata = self
            .slots
            .iter()
            .map(|slot| {
                (
                    slot.panel.entity_id(),
                    SidebarPanelMetadata::from_panel(slot.panel.read(cx)),
                )
            })
            .collect();
        if let Some(entity) = &self.sidebar_panel_metadata {
            entity.update(cx, |current, cx| {
                *current = metadata;
                cx.notify();
            });
        } else {
            self.sidebar_panel_metadata = Some(cx.new(|_| metadata));
        }
    }
}
