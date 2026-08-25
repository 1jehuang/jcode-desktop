//! Panel: one Jcode session as a spatial card with a live transcript.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use base64::Engine as _;
use gpui::{
    Animation, AnimationExt, App, Context, Entity, FocusHandle, Focusable, FontWeight, ImageSource,
    ScrollHandle, SharedString, StyledImage, Window, div, img, point, prelude::*, px, relative,
};
use jcode_desktop_api::HostHandle;
use jcode_sdk::ApiEvent;
use serde::{Deserialize, Serialize};

use crate::commands::{help_markdown, registered_command};
use crate::harness::{Bridge, Command, SessionOperation};
use crate::input::{PromptInput, PromptInputSnapshot};
use crate::markdown;
use crate::terminal::TerminalPanel;
use crate::theme::Theme;

fn command_unavailable_message(input: &str) -> String {
    let name = input.split_whitespace().next().unwrap_or(input);
    match registered_command(name) {
        Some(command) => format!(
            "`{name}` is a Jcode command, but its {} behavior is not available in Desktop yet.",
            command.help.to_ascii_lowercase()
        ),
        None => format!("Unknown command: `{name}`. Type `/help` for available commands."),
    }
}

/// One transcript entry, in display order.
#[derive(Debug, Clone)]
pub enum Item {
    User(String),
    Image(TranscriptImage),
    Assistant(String),
    Reasoning(String),
    Tool {
        call_id: String,
        name: String,
        /// Raw tool arguments as streamed, used for the one-line summary and
        /// the expanded detail.
        input: String,
        output: String,
        done: bool,
        error: Option<String>,
    },
    /// Live state for work detached by a tool call. Unlike the originating
    /// `bg`/`bash` row, this continues to change while the agent waits.
    BackgroundTask {
        task_id: String,
        label: String,
        summary: String,
        percent: Option<f32>,
        done: bool,
    },
    Todos(TodoCardPayload),
    Error(String),
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct TodoCardPayload {
    #[serde(default)]
    todos: Vec<TodoCardItem>,
    #[serde(default)]
    plan: TodoCardPlan,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct TodoCardPlan {
    user_intention: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct TodoCardItem {
    content: String,
    status: String,
    #[serde(default)]
    group: Option<String>,
    #[serde(default)]
    confidence: Option<serde_json::Value>,
    #[serde(default)]
    completion_confidence: Option<serde_json::Value>,
    #[serde(default)]
    blocked_by: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct TranscriptImage {
    media_type: String,
    data: String,
    label: Option<String>,
    preview: Option<Arc<gpui::Image>>,
}

static NEXT_TRANSCRIPT_IMAGE_ID: AtomicU64 = AtomicU64::new(1);

impl TranscriptImage {
    fn new(media_type: String, data: String, label: Option<String>) -> Self {
        let preview = base64::engine::general_purpose::STANDARD
            .decode(&data)
            .ok()
            .and_then(|bytes| {
                Some(Arc::new(gpui::Image {
                    format: gpui::ImageFormat::from_mime_type(&media_type)?,
                    bytes,
                    id: NEXT_TRANSCRIPT_IMAGE_ID.fetch_add(1, Ordering::Relaxed),
                }))
            });
        Self {
            media_type,
            data,
            label,
            preview,
        }
    }

    fn from_rendered(image: jcode_sdk::RenderedImage) -> Self {
        Self::new(image.media_type, image.data, image.label)
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct PanelSnapshot {
    pub session_id: String,
    pub title: String,
    pub working_dir: Option<String>,
    pub draft: PromptInputSnapshot,
    pub scroll_x: f32,
    pub scroll_y: f32,
    pub stick_to_bottom: bool,
    pub terminal_resource_id: Option<u64>,
}

pub struct Panel {
    pub session_id: String,
    pub title: SharedString,
    pub working_dir: Option<String>,
    pub status: String,
    pub connection_phase: String,
    /// Model id serving this session, e.g. `gpt-5.6-sol`.
    pub model: Option<String>,
    /// Provider display name, e.g. `openai` or `anthropic`.
    pub provider: Option<String>,
    /// Credential route for the current model, e.g. `oauth` or `api key`.
    pub auth_method: Option<String>,
    /// Reasoning effort, e.g. `high`, when the provider exposes it.
    pub reasoning_effort: Option<String>,
    /// Latest token usage: (input, output, cache_read) from the last update.
    token_usage: Option<(u64, u64, u64)>,
    pub items: Vec<Item>,
    /// Streaming assistant text accumulates here until the turn ends.
    streaming_text: String,
    streaming_reasoning: String,
    pub input: Entity<PromptInput>,
    pub focus_handle: FocusHandle,
    scroll: ScrollHandle,
    stick_to_bottom: bool,
    /// A detached reload offset cannot be applied until asynchronous history
    /// has rebuilt the scroll region. Painting the empty panel clamps it to 0.
    pending_history_scroll: Option<(f32, f32)>,
    bridge: Bridge,
    history_loaded: bool,
    /// Tool rows the user expanded, keyed by call id.
    expanded_tools: HashSet<String>,
    /// Reasoning rows the user expanded, keyed by transcript index.
    expanded_reasoning: HashSet<usize>,
    pending_users: VecDeque<usize>,
    accepted_users: HashMap<usize, Instant>,
    /// Newly received tool calls, keyed by call id, while their entrance runs.
    arriving_tools: HashMap<String, Instant>,
    terminal: Option<Entity<TerminalPanel>>,
    unfinished_work: Option<Vec<crate::harness::UnfinishedSession>>,
    /// A read-only source file opened from the workspace file browser.
    code_file: Option<CodeFile>,
    model_picker_open: bool,
    available_models: Vec<String>,
    model_logo_providers: HashMap<String, String>,
}

struct CodeFile {
    path: std::path::PathBuf,
    contents: Result<String, String>,
}

impl Panel {
    pub(crate) fn sidebar_runtime_status(&self) -> &str {
        &self.status
    }

    pub fn new(
        session_id: String,
        title: Option<String>,
        working_dir: Option<String>,
        bridge: Bridge,
        cx: &mut Context<Self>,
    ) -> Self {
        let send_bridge = bridge.clone();
        let send_session = session_id.clone();
        let input = cx.new(|cx| {
            PromptInput::new(
                cx,
                "message jcode...",
                move |content, images, _window, _app| {
                    send_bridge.send(Command::Send {
                        session_id: send_session.clone(),
                        content,
                        images,
                    });
                },
            )
        });
        // Local echo is appended by the workspace when submit fires; simplest
        // is to observe our own input entity... but the closure above has no
        // panel access. Instead the workspace routes sends through the panel.
        let display_title = title
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| short_id(&session_id));
        Self {
            session_id,
            title: display_title.into(),
            working_dir,
            status: "idle".into(),
            connection_phase: String::new(),
            model: None,
            provider: None,
            auth_method: None,
            reasoning_effort: None,
            token_usage: None,
            items: demo_items(),
            streaming_text: String::new(),
            streaming_reasoning: String::new(),
            input,
            focus_handle: cx.focus_handle(),
            scroll: ScrollHandle::new(),
            stick_to_bottom: true,
            pending_history_scroll: None,
            bridge,
            history_loaded: false,
            expanded_tools: HashSet::new(),
            expanded_reasoning: HashSet::new(),
            pending_users: VecDeque::new(),
            accepted_users: HashMap::new(),
            arriving_tools: HashMap::new(),
            terminal: None,
            unfinished_work: None,
            code_file: None,
            model_picker_open: false,
            available_models: Vec::new(),
            model_logo_providers: HashMap::new(),
        }
    }

    pub fn new_code_file(path: std::path::PathBuf, bridge: Bridge, cx: &mut Context<Self>) -> Self {
        const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;
        let title = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
        let contents = std::fs::metadata(&path)
            .map_err(|error| error.to_string())
            .and_then(|metadata| {
                if metadata.len() > MAX_FILE_BYTES {
                    Err(format!(
                        "file is too large to preview ({} bytes)",
                        metadata.len()
                    ))
                } else {
                    std::fs::read_to_string(&path).map_err(|error| {
                        if error.kind() == std::io::ErrorKind::InvalidData {
                            "binary files cannot be previewed".to_string()
                        } else {
                            error.to_string()
                        }
                    })
                }
            });
        let working_dir = path.parent().map(|parent| parent.display().to_string());
        let mut panel = Self::new(
            format!("file://{}", path.display()),
            Some(title),
            working_dir,
            bridge,
            cx,
        );
        panel.items.clear();
        panel.code_file = Some(CodeFile { path, contents });
        panel
    }

    pub fn new_terminal(
        working_dir: Option<String>,
        bridge: Bridge,
        host: HostHandle,
        resource_id: Option<u64>,
        cx: &mut Context<Self>,
    ) -> Self {
        let terminal = cx.new(|cx| TerminalPanel::new(working_dir.clone(), resource_id, host, cx));
        let mut panel = Self::new(
            "terminal".into(),
            Some("terminal".into()),
            working_dir,
            bridge,
            cx,
        );
        panel.terminal = Some(terminal);
        panel
    }

    pub fn new_unfinished_work(
        sessions: Vec<crate::harness::UnfinishedSession>,
        bridge: Bridge,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut panel = Self::new(
            "unfinished-work".into(),
            Some("unfinished work".into()),
            None,
            bridge,
            cx,
        );
        panel.unfinished_work = Some(sessions);
        panel
    }

    pub fn set_unfinished_work(
        &mut self,
        sessions: Vec<crate::harness::UnfinishedSession>,
        cx: &mut Context<Self>,
    ) {
        if self.unfinished_work.as_ref() != Some(&sessions) {
            self.unfinished_work = Some(sessions);
            cx.notify();
        }
    }

    pub fn snapshot(&self, cx: &App) -> PanelSnapshot {
        let offset = self.scroll.offset();
        PanelSnapshot {
            session_id: self.session_id.clone(),
            title: self.title.to_string(),
            working_dir: self.working_dir.clone(),
            draft: self.input.read(cx).snapshot(),
            scroll_x: f32::from(offset.x),
            scroll_y: f32::from(offset.y),
            stick_to_bottom: self.stick_to_bottom,
            terminal_resource_id: self
                .terminal
                .as_ref()
                .and_then(|terminal| terminal.read(cx).resource_id()),
        }
    }

    pub fn restore_snapshot(&mut self, snapshot: PanelSnapshot, cx: &mut Context<Self>) {
        self.title = snapshot.title.into();
        self.working_dir = snapshot.working_dir;
        self.stick_to_bottom = snapshot.stick_to_bottom;
        self.pending_history_scroll =
            (!snapshot.stick_to_bottom).then_some((snapshot.scroll_x, snapshot.scroll_y));
        self.scroll
            .set_offset(point(px(snapshot.scroll_x), px(snapshot.scroll_y)));
        self.input
            .update(cx, |input, cx| input.restore(snapshot.draft, cx));
    }

    /// Wire the input's submit to also echo locally. Called once after
    /// creation, when we have the panel entity.
    pub fn connect_input(panel: &Entity<Panel>, cx: &mut App) {
        let weak = panel.downgrade();
        panel.update(cx, |this, cx| {
            let bridge = this.bridge.clone();
            let session_id = this.session_id.clone();
            let cancel_weak = weak.clone();
            let change_weak = weak.clone();
            this.input = cx.new(|cx| {
                PromptInput::new(
                    cx,
                    "message jcode...",
                    move |content, images, _window, app| {
                        let echoed_images = images.clone();
                        if let Some(panel) = weak.upgrade() {
                            panel.update(app, |this, cx| {
                                if images.is_empty() && this.handle_slash_command(&content, cx) {
                                    return;
                                }
                                bridge.send(Command::Send {
                                    session_id: session_id.clone(),
                                    content: content.clone(),
                                    images,
                                });
                                let index = this.items.len();
                                this.items.push(Item::User(content));
                                this.items.extend(echoed_images.into_iter().map(
                                    |(media_type, data)| {
                                        Item::Image(TranscriptImage::new(media_type, data, None))
                                    },
                                ));
                                this.pending_users.push_back(index);
                                this.stick_to_bottom = true;
                                this.scroll.scroll_to_bottom();
                                cx.notify();
                            });
                        }
                    },
                )
                .with_on_change(move |content, app| {
                    if let Some(panel) = change_weak.upgrade() {
                        panel.update(app, |this, cx| {
                            if content == "/model" {
                                this.open_model_picker(cx);
                            } else if this.model_picker_open && !content.starts_with("/model ") {
                                this.close_model_picker(cx);
                            }
                        });
                    }
                })
                .with_on_overlay_cancel(move |app| {
                    let mut handled = false;
                    if let Some(panel) = cancel_weak.upgrade() {
                        panel.update(app, |this, cx| {
                            if this.model_picker_open {
                                this.close_model_picker(cx);
                                handled = true;
                            } else if this.status != "idle"
                                || !this.connection_phase.is_empty()
                                || !this.streaming_text.is_empty()
                                || !this.streaming_reasoning.is_empty()
                                || !this.pending_users.is_empty()
                            {
                                this.bridge.send(Command::Cancel {
                                    session_id: this.session_id.clone(),
                                });
                                handled = true;
                            }
                            cx.notify();
                        });
                    }
                    handled
                })
            });
        });
    }

    fn handle_slash_command(&mut self, content: &str, cx: &mut Context<Self>) -> bool {
        let trimmed = content.trim();
        if let Some(model) = trimmed
            .strip_prefix("/model ")
            .map(str::trim)
            .filter(|model| !model.is_empty())
        {
            self.close_model_picker(cx);
            self.bridge.send(Command::SetModel {
                session_id: self.session_id.clone(),
                model: model.to_string(),
            });
            self.items
                .push(Item::Assistant(format!("Switching model to `{model}`…")));
        } else if let Some(effort) = trimmed.strip_prefix("/effort ").map(str::trim) {
            const EFFORTS: &[&str] = &["none", "minimal", "low", "medium", "high", "xhigh", "max"];
            if EFFORTS.contains(&effort) {
                self.run_session_operation(
                    SessionOperation::SetEffort(effort.to_string()),
                    format!("Reasoning effort set to `{effort}`."),
                );
            } else {
                self.items.push(Item::Error(format!(
                    "Usage: `/effort <{}>`",
                    EFFORTS.join("|")
                )));
            }
        } else if let Some(title) = trimmed.strip_prefix("/rename ").map(str::trim) {
            if title.is_empty() {
                self.items.push(Item::Error(
                    "Usage: `/rename <session name>` or `/rename --clear`.".into(),
                ));
            } else {
                let title = (title != "--clear").then(|| title.to_string());
                self.run_session_operation(SessionOperation::Rename(title), "Session renamed.");
            }
        } else if let Some(target) = trimmed.strip_prefix("/rewind ").map(str::trim) {
            if target == "undo" {
                self.run_session_operation(SessionOperation::RewindUndo, "Rewind undone.");
            } else if let Ok(index) = target.parse::<usize>() {
                if index == 0 {
                    self.items
                        .push(Item::Error("Rewind numbering starts at 1.".into()));
                } else {
                    self.run_session_operation(
                        SessionOperation::Rewind(index - 1),
                        format!("Rewound to message {index}."),
                    );
                }
            } else {
                self.items.push(Item::Error(
                    "Usage: `/rewind <message number|undo>`.".into(),
                ));
            }
        } else {
            match trimmed {
                "/cancel" => {
                    self.bridge.send(Command::Cancel {
                        session_id: self.session_id.clone(),
                    });
                    self.items
                        .push(Item::Assistant("Cancellation requested.".into()));
                }
                "/cls" | "/clear-view" => {
                    self.items.clear();
                    self.streaming_text.clear();
                    self.streaming_reasoning.clear();
                }
                "/clear" => self.run_session_operation(
                    SessionOperation::Clear,
                    "Conversation history cleared.",
                ),
                "/compact" => self.run_session_operation(
                    SessionOperation::Compact,
                    "Context compacted.",
                ),
                "/help" | "/commands" | "/?" => {
                    self.items.push(Item::Assistant(help_markdown()))
                }
                "/todos" | "/todo" => {
                    if let Some(payload) = self.latest_todo_payload() {
                        self.items.push(Item::Todos(payload));
                    } else {
                        self.items.push(Item::Todos(TodoCardPayload::default()));
                    }
                }
                "/model" | "/models" => self.open_model_picker(cx),
                "/effort" => self.items.push(Item::Assistant(
                    "Usage: `/effort <none|minimal|low|medium|high|xhigh|max>`.".into(),
                )),
                "/rename" => self.items.push(Item::Error(
                    "Usage: `/rename <session name>` or `/rename --clear`.".into(),
                )),
                "/rewind" => self.items.push(Item::Assistant(
                    "Use `/rewind <message number>` to rewind, or `/rewind undo` to restore.".into(),
                )),
                "/commit" => self.submit_command_prompt(
                    "Make interactive, logical commits for the current uncommitted work. Inspect git state first, group related changes into coherent commits, preserve unrelated work, validate appropriately, and report the commits created plus remaining changes.",
                ),
                "/commit-push" | "/commit-and-push" => self.submit_command_prompt(
                    "Make logical commits for the current uncommitted work, preserving unrelated work and validating appropriately. Then push to the tracking branch without force-pushing, and report the commits and push result.",
                ),
                _ if trimmed.starts_with('/') => self.items.push(Item::Error(format!(
                    "{}",
                    command_unavailable_message(trimmed)
                ))),
                _ => return false,
            }
        }
        self.stick_to_bottom = true;
        self.scroll.scroll_to_bottom();
        cx.notify();
        true
    }

    fn latest_todo_payload(&self) -> Option<TodoCardPayload> {
        self.items.iter().rev().find_map(|item| match item {
            Item::Todos(payload) => Some(payload.clone()),
            Item::Tool {
                name,
                output,
                done: true,
                error: None,
                ..
            } if name == "todo" => parse_todo_tool_output(output),
            _ => None,
        })
    }

    fn open_model_picker(&mut self, cx: &mut Context<Self>) {
        if self.available_models.is_empty() {
            self.items.push(Item::Error(
                "No models are available yet. Wait for the session to connect, then try `/model` again.".into(),
            ));
            return;
        }
        self.model_picker_open = true;
        let input = self.input.clone();
        cx.defer(move |cx| {
            input.update(cx, |input, cx| {
                input.set_content("/model ".to_string(), cx);
                input.set_command_palette_visible(false, cx);
            });
        });
    }

    fn close_model_picker(&mut self, cx: &mut Context<Self>) {
        self.model_picker_open = false;
        let input = self.input.clone();
        cx.defer(move |cx| {
            input.update(cx, |input, cx| input.set_command_palette_visible(true, cx));
        });
    }

    fn select_model(&mut self, model: String, cx: &mut Context<Self>) {
        self.close_model_picker(cx);
        let input = self.input.clone();
        cx.defer(move |cx| {
            input.update(cx, |input, cx| input.set_content(String::new(), cx));
        });
        self.bridge.send(Command::SetModel {
            session_id: self.session_id.clone(),
            model: model.clone(),
        });
        self.items
            .push(Item::Assistant(format!("Switching model to `{model}`…")));
        self.stick_to_bottom = true;
        self.scroll.scroll_to_bottom();
        cx.notify();
    }

    fn render_model_picker(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let rows = self.input.read(cx).model_picker_rows();
        let list = div()
            .id("model-picker-list")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .children(
                rows.into_iter()
                    .enumerate()
                    .map(|(index, (model, selected))| {
                        let chosen = model.clone();
                        let provider = self
                            .model_logo_providers
                            .get(&model)
                            .map(String::as_str)
                            .unwrap_or_else(|| model_logo_provider(&model, ""));
                        let logo: gpui::AnyElement = match crate::accounts::logo(provider) {
                            Some(bytes) => gpui::svg()
                                .data(bytes)
                                .size(px(18.0))
                                .flex_none()
                                .text_color(if selected {
                                    Theme::global().TEXT
                                } else {
                                    Theme::global().TEXT_DIM
                                })
                                .into_any_element(),
                            None => div()
                                .size(px(18.0))
                                .flex_none()
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded_sm()
                                .bg(Theme::global().INLINE_CODE_BG)
                                .text_size(px(10.0))
                                .text_color(if selected {
                                    Theme::global().TEXT
                                } else {
                                    Theme::global().TEXT_DIM
                                })
                                .child(crate::accounts::lettermark(&model))
                                .into_any_element(),
                        };
                        div()
                            .id(("model-picker-row", index))
                            .debug_selector(move || format!("model-picker-row-{index}").into())
                            .flex()
                            .items_center()
                            .justify_between()
                            .px_4()
                            .py_2()
                            .cursor_pointer()
                            .bg(if selected {
                                Theme::global().USER_BG
                            } else {
                                Theme::global().PANEL_BG
                            })
                            .text_color(if selected {
                                Theme::global().TEXT
                            } else {
                                Theme::global().TEXT_DIM
                            })
                            .hover(|el| {
                                el.bg(Theme::global().HEADER_BG)
                                    .text_color(Theme::global().TEXT)
                            })
                            .on_mouse_down(
                                gpui::MouseButton::Left,
                                cx.listener(move |this, _event, _window, cx| {
                                    this.select_model(chosen.clone(), cx);
                                }),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_3()
                                    .child(
                                        div()
                                            .debug_selector(move || {
                                                format!("model-picker-logo-{index}")
                                            })
                                            .child(logo),
                                    )
                                    .child(
                                        div().font_family(Theme::global().FONT_MONO).child(model),
                                    ),
                            )
                            .when(selected, |el| {
                                el.child(div().text_color(Theme::global().ACCENT).child("●"))
                            })
                    }),
            );

        div()
            .id("model-picker-overlay")
            .debug_selector(|| "model-picker-overlay".into())
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(gpui::rgba(0x080b10d9))
            .occlude()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .overflow_hidden()
                    .with_animation(
                        "model-picker-dialog",
                        Animation::new(crate::transition::MODAL_DURATION),
                        |el, delta| el.opacity(delta),
                    )
                    .child(
                        div()
                            .debug_selector(|| "model-picker-dialog".into())
                            .w(px(620.0))
                            .max_w(relative(0.88))
                            .h(px(410.0))
                            .flex()
                            .flex_col()
                            .rounded_lg()
                            .border_1()
                            .border_color(Theme::global().PANEL_BORDER_FOCUS)
                            .bg(Theme::global().PANEL_BG)
                            .shadow_lg()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .px_4()
                                    .py_3()
                                    .border_b_1()
                                    .border_color(Theme::global().PANEL_BORDER)
                                    .child(
                                        div()
                                            .flex()
                                            .flex_col()
                                            .gap_1()
                                            .child(div().text_size(px(15.0)).child("Choose a model"))
                                            .child(
                                                div()
                                                    .text_size(px(10.5))
                                                    .text_color(Theme::global().TEXT_FAINT)
                                                    .child("type to filter  ·  ↑↓ move  ·  enter select"),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .cursor_pointer()
                                            .text_color(Theme::global().TEXT_DIM)
                                            .hover(|el| el.text_color(Theme::global().TEXT))
                                            .on_mouse_down(
                                                gpui::MouseButton::Left,
                                                cx.listener(|this, _event, _window, cx| {
                                                    this.close_model_picker(cx);
                                                    this.input.update(cx, |input, cx| {
                                                        input.set_content(String::new(), cx)
                                                    });
                                                    cx.notify();
                                                }),
                                            )
                                            .child("esc"),
                                    ),
                            )
                            .child(list),
                    ),
            )
            .into_any_element()
    }

    fn run_session_operation(&mut self, operation: SessionOperation, message: impl Into<String>) {
        self.bridge.send(Command::SessionOperation {
            session_id: self.session_id.clone(),
            operation,
        });
        self.items.push(Item::Assistant(message.into()));
    }

    fn submit_command_prompt(&mut self, prompt: &str) {
        self.bridge.send(Command::Send {
            session_id: self.session_id.clone(),
            content: prompt.to_string(),
            images: Vec::new(),
        });
        let index = self.items.len();
        self.items.push(Item::User(prompt.to_string()));
        self.pending_users.push_back(index);
    }

    pub fn load_history(
        &mut self,
        messages: Vec<jcode_sdk::HistoryMessage>,
        images: Vec<jcode_sdk::RenderedImage>,
        cx: &mut Context<Self>,
    ) {
        if self.history_loaded {
            // Reattaching a session fetches history again. The runtime may have
            // completed the active turn while its event stream was unavailable,
            // so reconcile the newest assistant message instead of discarding
            // the refresh and leaving the locally echoed prompt unanswered.
            if let Some(response) = messages
                .iter()
                .rev()
                .find(|message| message.role == "assistant" && !message.content.trim().is_empty())
                .map(|message| message.content.as_str())
            {
                self.recover_response(response);
                if self.stick_to_bottom {
                    self.scroll.scroll_to_bottom();
                }
                cx.notify();
            }
            return;
        }
        self.history_loaded = true;
        let mut images_by_prompt: HashMap<usize, Vec<jcode_sdk::RenderedImage>> = HashMap::new();
        let mut trailing_images = Vec::new();
        for image in images {
            match &image.anchor {
                Some(jcode_sdk::RenderedImageAnchor::UserPrompt { ordinal }) => {
                    images_by_prompt.entry(*ordinal).or_default().push(image);
                }
                _ => trailing_images.push(image),
            }
        }
        let mut items = Vec::with_capacity(messages.len() + trailing_images.len());
        let mut user_ordinal = 0;
        for message in messages {
            match message.role.as_str() {
                "user" => {
                    items.push(Item::User(message.content));
                    if let Some(images) = images_by_prompt.remove(&user_ordinal) {
                        items.extend(
                            images
                                .into_iter()
                                .map(|image| Item::Image(TranscriptImage::from_rendered(image))),
                        );
                    }
                    user_ordinal += 1;
                }
                "assistant" => {
                    if !message.content.trim().is_empty() {
                        items.push(Item::Assistant(message.content));
                    }
                }
                _ => {}
            }
        }
        for images in images_by_prompt.into_values() {
            trailing_images.extend(images);
        }
        items.extend(
            trailing_images
                .into_iter()
                .map(|image| Item::Image(TranscriptImage::from_rendered(image))),
        );
        // History goes first; anything echoed locally before it arrived is
        // appended, minus the duplicate the server already knows about.
        let mut existing = std::mem::take(&mut self.items);
        existing.retain(|item| match item {
            Item::User(text) => !matches!(items.last(), Some(Item::User(last)) if last == text),
            _ => true,
        });
        let history_len = items.len();
        self.pending_users = self
            .pending_users
            .drain(..)
            .map(|index| index + history_len)
            .collect();
        self.accepted_users = std::mem::take(&mut self.accepted_users)
            .into_iter()
            .map(|(index, at)| (index + history_len, at))
            .collect();
        items.append(&mut existing);
        self.items = items;
        if let Some((x, y)) = self.pending_history_scroll.take() {
            self.scroll.set_offset(point(px(x), px(y)));
        } else if self.stick_to_bottom {
            self.scroll.scroll_to_bottom();
        }
        cx.notify();
    }

    fn recover_response(&mut self, response: &str) {
        if self.streaming_text == response {
            return;
        }
        if !self.streaming_text.is_empty() && response.starts_with(&self.streaming_text) {
            self.streaming_text = response.to_string();
            return;
        }

        if let Some(existing) = self.items.iter_mut().rev().find_map(|item| match item {
            Item::Assistant(text) => Some(text),
            _ => None,
        }) {
            if existing == response {
                return;
            }
            if response.starts_with(existing.as_str()) {
                *existing = response.to_string();
                return;
            }
        }

        self.flush_reasoning();
        self.flush_streaming();
        self.items.push(Item::Assistant(response.to_string()));
    }

    /// Apply a streaming event addressed to this session.
    pub fn apply(&mut self, event: &ApiEvent, cx: &mut Context<Self>) {
        match event {
            ApiEvent::MessageAccepted { .. } => {
                acknowledge_next(
                    &mut self.pending_users,
                    &mut self.accepted_users,
                    Instant::now(),
                );
            }
            ApiEvent::TextDelta { text, .. } => {
                self.flush_reasoning();
                self.streaming_text.push_str(text);
            }
            ApiEvent::ReasoningDelta { text, .. } => {
                self.streaming_reasoning.push_str(text);
            }
            ApiEvent::ReasoningDone { .. } => {
                self.flush_reasoning();
            }
            ApiEvent::ToolStart { call_id, name, .. } => {
                self.flush_streaming();
                self.arriving_tools.insert(call_id.clone(), Instant::now());
                self.items.push(Item::Tool {
                    call_id: call_id.clone(),
                    name: name.clone(),
                    input: String::new(),
                    output: String::new(),
                    done: false,
                    error: None,
                });
            }
            ApiEvent::ToolInputDelta { call_id, delta, .. } => {
                if let Some(Item::Tool { input, .. }) = self.find_tool(call_id) {
                    input.push_str(delta);
                }
            }
            ApiEvent::ToolDone {
                call_id,
                name,
                output,
                error,
                ..
            } => {
                if let Some(Item::Tool {
                    done,
                    error: slot,
                    output: output_slot,
                    ..
                }) = self.find_tool(call_id)
                {
                    *done = true;
                    *slot = error.clone();
                    *output_slot = output.clone();
                } else {
                    self.arriving_tools.insert(call_id.clone(), Instant::now());
                    self.items.push(Item::Tool {
                        call_id: call_id.clone(),
                        name: name.clone(),
                        input: String::new(),
                        output: output.clone(),
                        done: true,
                        error: error.clone(),
                    });
                }
            }
            ApiEvent::BackgroundProgress {
                task_id,
                label,
                summary,
                percent,
                done,
                ..
            } => {
                if let Some(Item::BackgroundTask {
                    label: current_label,
                    summary: current_summary,
                    percent: current_percent,
                    done: current_done,
                    ..
                }) = self.items.iter_mut().rev().find(|item| {
                    matches!(item, Item::BackgroundTask { task_id: id, .. } if id == task_id)
                }) {
                    *current_label = label.clone();
                    *current_summary = summary.clone();
                    *current_percent = *percent;
                    *current_done = *done;
                } else {
                    self.items.push(Item::BackgroundTask {
                        task_id: task_id.clone(),
                        label: label.clone(),
                        summary: summary.clone(),
                        percent: *percent,
                        done: *done,
                    });
                }
            }
            ApiEvent::SidePaneImages { images, .. } => {
                for image in images.iter().cloned() {
                    self.insert_rendered_image(image);
                }
            }
            ApiEvent::TurnDone { .. } => {
                self.flush_reasoning();
                self.flush_streaming();
                self.connection_phase.clear();
            }
            ApiEvent::SessionStatus { status, .. } => {
                self.status = status.clone();
                if status == "idle" {
                    self.flush_reasoning();
                    self.flush_streaming();
                    self.connection_phase.clear();
                }
            }
            ApiEvent::ConnectionPhase { phase, .. } => {
                self.connection_phase = phase.clone();
            }
            ApiEvent::ModelInfo {
                provider, model, ..
            } => {
                if provider.is_some() {
                    self.provider = provider.clone();
                }
                // Only an actual switch invalidates the auth method: the route
                // catalog keyed it by model, but effort broadcasts repeat the
                // current model and must not wipe a still-correct label.
                if model.is_some() && *model != self.model {
                    self.model = model.clone();
                    self.auth_method = None;
                }
            }
            ApiEvent::RuntimeInfo {
                provider,
                model,
                routes,
                ..
            } => {
                if provider.is_some() {
                    self.provider = provider.clone();
                }
                if model.is_some() {
                    self.model = model.clone();
                }
                self.auth_method = auth_method_for_model(self.model.as_deref(), routes);
                let models = available_model_names(routes);
                self.model_logo_providers = available_model_logo_providers(routes);
                self.available_models = models.clone();
                self.input
                    .update(cx, |input, cx| input.set_command_models(models, cx));
            }
            ApiEvent::TokenUsage {
                input,
                output,
                cache_read_input,
                ..
            } => {
                self.token_usage = Some((*input, *output, cache_read_input.unwrap_or(0)));
            }
            ApiEvent::SessionRenamed { display_title, .. } => {
                self.title = display_title.clone().into();
            }
            ApiEvent::Error { message, .. } => {
                self.flush_streaming();
                self.items.push(Item::Error(message.clone()));
            }
            _ => {}
        }
        if self.stick_to_bottom {
            self.scroll.scroll_to_bottom();
        }
        cx.notify();
    }

    fn flush_streaming(&mut self) {
        if !self.streaming_text.trim().is_empty() {
            self.items
                .push(Item::Assistant(std::mem::take(&mut self.streaming_text)));
        } else {
            self.streaming_text.clear();
        }
    }

    fn find_tool(&mut self, call_id: &str) -> Option<&mut Item> {
        // The legacy harness protocol streams `tool_input` without an id. The
        // bridge preserves that fact as an empty call_id, so associate those
        // deltas with the most recent active call. Exact ids still win for
        // modern providers and for overlapping completed calls.
        if call_id.is_empty() {
            return self
                .items
                .iter_mut()
                .rev()
                .find(|item| matches!(item, Item::Tool { done: false, .. }));
        }
        self.items.iter_mut().rev().find(
            |item| matches!(item, Item::Tool { call_id: existing, .. } if existing == call_id),
        )
    }

    fn insert_rendered_image(&mut self, image: jcode_sdk::RenderedImage) {
        if self.items.iter().any(|item| {
            matches!(item, Item::Image(existing)
                if existing.media_type == image.media_type && existing.data == image.data)
        }) {
            return;
        }
        let insertion = match image.anchor.as_ref() {
            Some(jcode_sdk::RenderedImageAnchor::ToolCall { id }) => self
                .items
                .iter()
                .rposition(|item| matches!(item, Item::Tool { call_id, .. } if call_id == id))
                .map(|index| index + 1),
            _ => None,
        }
        .unwrap_or(self.items.len());
        self.items.insert(
            insertion,
            Item::Image(TranscriptImage::from_rendered(image)),
        );
    }

    fn flush_reasoning(&mut self) {
        if !self.streaming_reasoning.trim().is_empty() {
            let target_index = self
                .items
                .last()
                .filter(|item| matches!(item, Item::Reasoning(_)))
                .map(|_| self.items.len() - 1)
                .unwrap_or(self.items.len());
            // Live reasoning is shown in full as it arrives. Keep that disclosure
            // state when the sentinel row becomes a settled transcript item so
            // the card does not suddenly collapse at the end of the turn.
            let was_expanded = self.expanded_reasoning.remove(&(usize::MAX - 1))
                || self.streaming_reasoning.chars().count() > 200;
            if was_expanded {
                self.expanded_reasoning.insert(target_index);
            }
            append_reasoning(
                &mut self.items,
                std::mem::take(&mut self.streaming_reasoning),
            );
        } else {
            self.streaming_reasoning.clear();
        }
    }

    pub fn is_busy(&self) -> bool {
        self.status != "idle" || !self.streaming_text.is_empty()
    }

    pub fn message_failed(&mut self, message: String, cx: &mut Context<Self>) {
        self.flush_reasoning();
        self.flush_streaming();
        self.items.push(Item::Error(message));
        self.status = "idle".into();
        self.connection_phase.clear();
        self.stick_to_bottom = true;
        self.scroll.scroll_to_bottom();
        cx.notify();
    }

    /// A one-line footer describing what the session is doing, when that is
    /// not simply "idle". Provider phase wins over the coarse status because
    /// it is what tells the user progress is still happening.
    fn status_line(&self) -> Option<String> {
        if !self.connection_phase.is_empty() {
            return Some(self.connection_phase.clone());
        }
        if self.status != "idle" {
            return Some(self.status.replace('_', " "));
        }
        None
    }

    fn render_item(
        &self,
        index: usize,
        item: &Item,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        match item {
            Item::User(text) => {
                let now = Instant::now();
                let pending = self.pending_users.contains(&index);
                let (offset, opacity, animating) = self
                    .accepted_users
                    .get(&index)
                    .map(|at| crate::ack::motion(*at, now))
                    .unwrap_or((
                        0.0,
                        if pending {
                            crate::ack::PENDING_TONE
                        } else {
                            1.0
                        },
                        false,
                    ));
                if animating {
                    window.request_animation_frame();
                }
                div()
                    .flex()
                    .flex_col()
                    .ml(px(offset))
                    .opacity(opacity)
                    .bg(Theme::global().USER_BG)
                    .rounded_md()
                    .px_3()
                    .py_2()
                    .text_color(Theme::global().TEXT_USER)
                    .child(markdown::render(text, window))
                    .into_any_element()
            }
            Item::Image(image) => {
                let label = image
                    .label
                    .clone()
                    .unwrap_or_else(|| "image read by model".to_string());
                div()
                    .debug_selector(|| "transcript-image".into())
                    .flex()
                    .flex_col()
                    .items_start()
                    .gap_1()
                    .rounded_md()
                    .p_2()
                    .bg(Theme::global().USER_BG)
                    .when_some(image.preview.clone(), |el, preview| {
                        el.child(
                            img(ImageSource::Image(preview))
                                .w_full()
                                .h(px(320.0))
                                .object_fit(gpui::ObjectFit::Contain)
                                .rounded_md(),
                        )
                    })
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(Theme::global().TEXT_DIM)
                            .child(if image.preview.is_some() {
                                label
                            } else {
                                format!("{label} (could not display {})", image.media_type)
                            }),
                    )
                    .into_any_element()
            }
            Item::Assistant(text) => div()
                .debug_selector(|| "assistant-response".into())
                .px_1()
                .text_color(Theme::global().TEXT)
                .child(markdown::render(text, window))
                // Streaming updates already repaint this row as text arrives. A
                // repeating GPUI animation here would repaint every settled
                // markdown row at display rate between chunks.
                .when(index == usize::MAX, |el| {
                    el.child(
                        div()
                            .text_size(px(12.0))
                            .text_color(Theme::global().ACCENT)
                            .child("▍"),
                    )
                })
                .into_any_element(),
            Item::Reasoning(text) => {
                let expanded = self.expanded_reasoning.contains(&index);
                // Let the live card grow with the complete thought. Once settled,
                // older compact cards can still be expanded on demand.
                let live = index == usize::MAX - 1;
                let body: String = if expanded || live {
                    text.clone()
                } else {
                    condense(text, 200)
                };
                let long = text.chars().count() > 200;
                div()
                    .id(("reasoning", index))
                    .flex()
                    // Reasoning belongs to the scrolling transcript. It should
                    // retain its content height rather than being flex-squashed
                    // into a fixed-looking card when the transcript overflows.
                    .flex_none()
                    .flex_col()
                    .gap_0p5()
                    .px_2()
                    .py_1p5()
                    .rounded_md()
                    .bg(if live {
                        Theme::global().ACCENT_DIM
                    } else {
                        Theme::global().REASONING_BG
                    })
                    .when(long && !live, |el| {
                        el.cursor_pointer().on_mouse_down(
                            gpui::MouseButton::Left,
                            cx.listener(move |this, _event, _window, cx| {
                                if !this.expanded_reasoning.remove(&index) {
                                    this.expanded_reasoning.insert(index);
                                }
                                cx.notify();
                            }),
                        )
                    })
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .gap_1p5()
                            .items_center()
                            .text_size(px(10.0))
                            .text_color(if live {
                                Theme::global().TEXT_DIM
                            } else {
                                Theme::global().TEXT_FAINT
                            })
                            .child(if live {
                                // Incoming reasoning events provide the repaint clock;
                                // avoid a second, unbounded full-panel animation.
                                div().child("● thinking…").into_any_element()
                            } else {
                                div().child("thinking").into_any_element()
                            })
                            .when(long && !live, |el| {
                                el.child(if expanded { "show less" } else { "show all" })
                            }),
                    )
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(Theme::global().REASONING)
                            .italic()
                            .line_height(relative(1.45))
                            .child(body),
                    )
                    .into_any_element()
            }
            Item::Todos(payload) => render_todo_card(payload).into_any_element(),
            Item::BackgroundTask {
                task_id: _,
                label,
                summary,
                percent,
                done,
            } => {
                let progress = percent.map(|value| (value / 100.0).clamp(0.0, 1.0));
                div()
                    .id(("background-task", index))
                    .debug_selector(|| "background-task-card".into())
                    .flex()
                    .flex_none()
                    .flex_col()
                    .gap_1p5()
                    .rounded_md()
                    .border_1()
                    .border_color(Theme::global().TOOL_BORDER)
                    .bg(Theme::global().TOOL_BG)
                    .px_2p5()
                    .py_2()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .flex_none()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(if *done {
                                        Theme::global().OK
                                    } else {
                                        Theme::global().WARN
                                    })
                                    .child(if *done { "✓" } else { "●" }),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(Theme::global().TOOL_TEXT)
                                    .child(label.clone()),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .font_family(Theme::global().FONT_MONO)
                                    .text_size(px(10.0))
                                    .text_color(Theme::global().TEXT_FAINT)
                                    .child(if *done { "finished" } else { "background" }),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(11.5))
                            .text_color(Theme::global().TEXT_DIM)
                            .child(summary.clone()),
                    )
                    .when_some(progress, |el, progress| {
                        el.child(
                            div()
                                .w_full()
                                .h(px(4.0))
                                .rounded_full()
                                .bg(Theme::global().TOOL_BORDER)
                                .child(div().h_full().w(relative(progress)).rounded_full().bg(
                                    if *done {
                                        Theme::global().OK
                                    } else {
                                        Theme::global().ACCENT
                                    },
                                )),
                        )
                    })
                    .into_any_element()
            }
            Item::Tool {
                call_id,
                name,
                input,
                output,
                done,
                error,
            } => {
                let (offset, opacity, animating) = self
                    .arriving_tools
                    .get(call_id)
                    .map(|started_at| {
                        crate::transition::arrival_motion(
                            *started_at,
                            Instant::now(),
                            crate::transition::policy(crate::transition::Transition::ToolArrival)
                                .duration,
                        )
                    })
                    .unwrap_or((0.0, 1.0, false));
                if animating {
                    window.request_animation_frame();
                }
                if name == "todo"
                    && *done
                    && error.is_none()
                    && let Some(payload) = parse_todo_tool_output(output)
                {
                    return div()
                        .ml(px(offset))
                        .opacity(opacity)
                        .child(render_todo_card(&payload))
                        .into_any_element();
                }
                let status = match (done, error) {
                    (false, _) => div()
                        .w(px(12.0))
                        .flex_none()
                        .text_color(Theme::global().WARN)
                        // Tool events notify the panel on meaningful progress. A
                        // perpetual spinner otherwise reparses and repaints the
                        // complete transcript while a long-running tool is quiet.
                        .child("●")
                        .into_any_element(),
                    (true, None) => div()
                        .w(px(12.0))
                        .flex_none()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(Theme::global().OK)
                        .child("✓")
                        .into_any_element(),
                    (true, Some(_)) => div()
                        .w(px(12.0))
                        .flex_none()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(Theme::global().ERROR)
                        .child("×")
                        .into_any_element(),
                };
                let expanded = self.expanded_tools.contains(call_id);
                let summary = tool_summary(input);
                let detail = tool_detail(name, input, output);
                let has_detail = !detail.is_empty();
                let output_lines = output.lines().filter(|l| !l.trim().is_empty()).count();
                let call_id = call_id.clone();
                div()
                    .id(("tool", index))
                    .debug_selector(|| "tool-card".into())
                    .flex()
                    .ml(px(offset))
                    .opacity(opacity)
                    // Transcript rows live in a fixed-height flex column. Once
                    // it overflows, flex items shrink by default, which can
                    // squash tool cards instead of letting the column scroll.
                    .flex_none()
                    .flex_col()
                    .rounded_md()
                    .bg(Theme::global().TOOL_BG)
                    .border_1()
                    .border_color(Theme::global().TOOL_BORDER)
                    .overflow_hidden()
                    .child(
                        div()
                            .debug_selector(|| "tool-header".into())
                            .flex()
                            .flex_row()
                            .gap_2()
                            .items_center()
                            .px_2()
                            .py_1()
                            .text_size(px(11.5))
                            .when(has_detail, |el| {
                                el.cursor_pointer().on_mouse_down(
                                    gpui::MouseButton::Left,
                                    cx.listener(move |this, _event, _window, cx| {
                                        if !this.expanded_tools.remove(&call_id) {
                                            this.expanded_tools.insert(call_id.clone());
                                        }
                                        cx.notify();
                                    }),
                                )
                            })
                            .child(status)
                            .child(
                                div()
                                    .flex_none()
                                    .font_family(Theme::global().FONT_MONO)
                                    .text_size(px(10.0))
                                    .text_color(Theme::global().TEXT_FAINT)
                                    .child(name.clone()),
                            )
                            .when(!summary.is_empty(), |el| {
                                el.child(
                                    div()
                                        .debug_selector(|| "tool-summary".into())
                                        .flex_1()
                                        .min_w_0()
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(Theme::global().TOOL_TEXT)
                                        .child(summary),
                                )
                            })
                            .when(!*done, |el| {
                                el.child(
                                    div()
                                        .flex_none()
                                        .text_size(px(10.0))
                                        .text_color(Theme::global().TEXT_FAINT)
                                        .child("running"),
                                )
                            })
                            // Collapsed finished calls hint at how much output
                            // is hiding behind the expander.
                            .when(*done && !expanded && output_lines > 1, |el| {
                                el.child(
                                    div()
                                        .debug_selector(|| "tool-output-size".into())
                                        .flex_none()
                                        .text_size(px(10.0))
                                        .text_color(Theme::global().TEXT_FAINT)
                                        .child(format!("{output_lines} lines")),
                                )
                            })
                            .when(has_detail, |el| {
                                el.child(
                                    div()
                                        .flex_none()
                                        .text_size(px(10.0))
                                        .text_color(Theme::global().TEXT_FAINT)
                                        .child(if expanded { "▾" } else { "▸" }),
                                )
                            }),
                    )
                    .when(expanded && has_detail, |el| {
                        el.child(
                            div()
                                .debug_selector(|| "tool-detail".into())
                                .border_t_1()
                                .border_color(Theme::global().TOOL_BORDER)
                                .bg(Theme::global().CODE_BG)
                                .px_2p5()
                                .py_1p5()
                                .font_family(Theme::global().FONT_MONO)
                                .text_size(px(11.5))
                                .line_height(relative(1.45))
                                .text_color(Theme::global().CODE_TEXT)
                                .child(detail),
                        )
                    })
                    .children(error.clone().map(|message| {
                        div()
                            .px_2()
                            .py_1()
                            .border_t_1()
                            .border_color(Theme::global().TOOL_BORDER)
                            .text_size(px(11.5))
                            .font_family(Theme::global().FONT_MONO)
                            .text_color(Theme::global().ERROR)
                            .child(condense(&strip_ansi(&message), 300))
                    }))
                    .into_any_element()
            }
            Item::Error(message) => div()
                .flex()
                .flex_row()
                .gap_2()
                .items_start()
                .px_2p5()
                .py_1p5()
                .rounded_md()
                .bg(Theme::global().ERROR_BG)
                .border_1()
                .border_color(Theme::global().TOOL_BORDER)
                .text_size(px(12.0))
                .text_color(Theme::global().ERROR)
                .child(div().flex_none().child("!"))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .line_height(relative(1.45))
                        .child(message.clone()),
                )
                .into_any_element(),
        }
    }

    pub fn focus_input(&self, window: &mut Window, cx: &mut App) {
        let handle = self.input_focus_handle(cx);
        window.focus(&handle, cx);
    }

    pub fn input_focus_handle(&self, cx: &App) -> FocusHandle {
        self.terminal
            .as_ref()
            .map(|terminal| terminal.read(cx).focus_handle(cx))
            .unwrap_or_else(|| self.input.read(cx).focus_handle.clone())
    }

    #[cfg(test)]
    pub fn test_terminal_contents(&self, cx: &App) -> Option<String> {
        self.terminal
            .as_ref()
            .map(|terminal| terminal.read(cx).screen_contents())
    }

    /// The transcript's vertical scroll offset, so tests can prove a gesture
    /// did or did not scroll it.
    #[cfg(test)]
    pub fn test_scroll_offset_y(&self) -> gpui::Pixels {
        self.scroll.offset().y
    }
}

/// Keep provider-level reasoning segments in one visual card. Some providers
/// emit `ReasoningDone` between segments even though they belong to the same
/// uninterrupted thinking phase.
fn append_reasoning(items: &mut Vec<Item>, text: String) {
    if let Some(Item::Reasoning(existing)) = items.last_mut() {
        append_reasoning_text(existing, &text);
    } else {
        items.push(Item::Reasoning(text));
    }
}

impl Render for Panel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(terminal) = &self.terminal {
            return div()
                .size_full()
                .track_focus(&self.focus_handle)
                .child(terminal.clone())
                .into_any_element();
        }
        if let Some(file) = &self.code_file {
            let path = file.path.display().to_string();
            let mut body = div()
                .id("code-file-contents")
                .debug_selector(|| "code-file-contents".into())
                .flex_1()
                .min_h_0()
                .overflow_scroll()
                .font_family(Theme::global().FONT_MONO)
                .text_size(px(12.0))
                .py_2();
            match &file.contents {
                Ok(contents) => {
                    for (index, line) in contents.lines().enumerate() {
                        body = body.child(
                            div()
                                .flex()
                                .min_w_full()
                                .child(
                                    div()
                                        .w(px(52.0))
                                        .flex_none()
                                        .pr_3()
                                        .text_align(gpui::TextAlign::Right)
                                        .text_color(Theme::global().CODE_GUTTER)
                                        .child((index + 1).to_string()),
                                )
                                .child(
                                    div()
                                        .pr_4()
                                        .whitespace_nowrap()
                                        .text_color(Theme::global().CODE_TEXT)
                                        .child(if line.is_empty() {
                                            " ".to_string()
                                        } else {
                                            line.to_owned()
                                        }),
                                ),
                        );
                    }
                    if contents.is_empty() {
                        body = body.child(
                            div()
                                .px_4()
                                .text_color(Theme::global().TEXT_DIM)
                                .child("empty file"),
                        );
                    }
                }
                Err(error) => {
                    body = body.child(
                        div()
                            .p_4()
                            .text_color(Theme::global().ERROR)
                            .child(error.clone()),
                    );
                }
            }
            return div()
                .size_full()
                .flex()
                .flex_col()
                .overflow_hidden()
                .track_focus(&self.focus_handle)
                .child(
                    div()
                        .flex_none()
                        .px_3()
                        .py_2()
                        .border_b_1()
                        .border_color(Theme::global().CODE_BORDER)
                        .bg(Theme::global().CODE_HEADER_BG)
                        .font_family(Theme::global().FONT_MONO)
                        .text_size(px(11.0))
                        .text_color(Theme::global().TEXT_DIM)
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .child(path),
                )
                .child(body)
                .into_any_element();
        }
        if let Some(sessions) = &self.unfinished_work {
            let mut list = div()
                .id("unfinished-work-list")
                .debug_selector(|| "unfinished-work-list".into())
                .size_full()
                .overflow_y_scroll()
                .p_4()
                .flex()
                .flex_col()
                .gap_3()
                .child(div().text_size(px(18.)).font_weight(gpui::FontWeight::SEMIBOLD).child("Unfinished work"))
                .child(div().text_size(px(11.)).text_color(Theme::global().TEXT_DIM).child("Todos left in closed sessions. Open the session from the sidebar to continue."));
            if sessions.is_empty() {
                list = list.child(
                    div()
                        .mt_4()
                        .text_color(Theme::global().TEXT_DIM)
                        .child("You are all caught up."),
                );
            }
            for (index, session) in sessions.iter().enumerate() {
                let mut card = div()
                    .id(("unfinished-session", index))
                    .debug_selector(move || format!("unfinished-session-{index}").into())
                    .p_3()
                    .rounded_lg()
                    .border_1()
                    .border_color(Theme::global().PANEL_BORDER)
                    .bg(Theme::global().HEADER_BG)
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .child(session.title.clone()),
                    );
                if let Some(dir) = &session.working_dir {
                    card = card.child(
                        div()
                            .text_size(px(10.))
                            .text_color(Theme::global().TEXT_FAINT)
                            .child(dir.clone()),
                    );
                }
                for todo in &session.todos {
                    let marker = if todo.status.eq_ignore_ascii_case("in_progress") {
                        "◐"
                    } else {
                        "○"
                    };
                    card = card.child(
                        div()
                            .flex()
                            .gap_2()
                            .text_size(px(12.))
                            .child(marker)
                            .child(todo.content.clone()),
                    );
                }
                list = list.child(card);
            }
            return div()
                .size_full()
                .track_focus(&self.focus_handle)
                .child(list)
                .into_any_element();
        }
        let scroll_handle = self.scroll.clone();
        let streaming = !self.streaming_text.is_empty();
        let mut transcript = div()
            .id(if streaming {
                "transcript-with-response"
            } else {
                "transcript"
            })
            // Tagged so render tests can assert a streamed response painted.
            .debug_selector(move || {
                if streaming {
                    "transcript-with-response".into()
                } else {
                    "transcript".into()
                }
            })
            .flex()
            .flex_col()
            .gap_2p5()
            .size_full()
            .px_3()
            .py_2p5()
            .text_size(px(13.5))
            .overflow_y_scroll()
            // Vertical deltas only: horizontal two-finger swipes pan the
            // workspace canvas instead of nudging the transcript.
            .restrict_scroll_to_axis()
            .track_scroll(&self.scroll)
            // Reading history must win over following the stream: scrolling
            // up releases stick-to-bottom, and returning near the bottom
            // re-engages it.
            .on_scroll_wheel(cx.listener(
                move |this, event: &gpui::ScrollWheelEvent, window, cx| {
                    let delta = event.delta.pixel_delta(window.line_height()).y;
                    if delta > px(0.) {
                        if this.stick_to_bottom {
                            this.stick_to_bottom = false;
                            cx.notify();
                        }
                    } else if delta < px(0.) && !this.stick_to_bottom {
                        let distance = scroll_handle.max_offset().y + scroll_handle.offset().y;
                        if distance <= px(48.) {
                            this.stick_to_bottom = true;
                            this.scroll.scroll_to_bottom();
                            cx.notify();
                        }
                    }
                },
            ));

        // Live rows are appended after the settled ones and share the same
        // renderer, so a streaming turn looks identical to a finished one.
        // Todo state is persistent session chrome rather than transcript history.
        // Keep only the latest snapshot pinned above the scroller instead of
        // leaving stale cards interspersed through the conversation.
        let pinned_todo = self
            .latest_todo_payload()
            .filter(|payload| !payload.todos.is_empty());
        let latest_prompt = self.items.iter().rev().find_map(|item| match item {
            Item::User(prompt) if !prompt.trim().is_empty() => Some(prompt.clone()),
            _ => None,
        });
        let mut rows: Vec<(usize, Item)> = self
            .items
            .iter()
            .cloned()
            .enumerate()
            .filter(|(_, item)| {
                !matches!(item, Item::Todos(_))
                    && !matches!(item, Item::Tool { name, .. } if name == "todo")
            })
            .collect();
        if !self.streaming_reasoning.is_empty() {
            rows.push((
                usize::MAX - 1,
                Item::Reasoning(self.streaming_reasoning.clone()),
            ));
        }
        if !self.streaming_text.is_empty() {
            rows.push((usize::MAX, Item::Assistant(self.streaming_text.clone())));
        }
        let rows = coalesce_reasoning_rows(rows);

        let mut previous: Option<&'static str> = None;
        for (index, item) in &rows {
            let role = role_of(item);
            // Group consecutive rows from the same speaker: only the first
            // gets a caption, and tool runs sit tight under their turn.
            let show_label = role.is_some() && role != previous;
            previous = role.or(previous);
            let element = self.render_item(*index, item, window, cx);
            transcript = transcript.child(match (show_label, role) {
                (true, Some(label)) => role_caption(label, element),
                _ => element,
            });
        }

        if rows.is_empty() {
            transcript = transcript.child(
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .items_center()
                    .justify_center()
                    .child(
                        div()
                            .text_color(Theme::global().TEXT_DIM)
                            .text_size(px(13.0))
                            .child("no messages yet"),
                    )
                    .child(
                        div()
                            .text_color(Theme::global().TEXT_FAINT)
                            .text_size(px(11.0))
                            .child("type below to start this session"),
                    ),
            );
        }

        let status_line = self.status_line();
        let meta_line = meta_line(
            self.working_dir.as_deref(),
            self.model.as_deref(),
            self.provider.as_deref(),
            self.auth_method.as_deref(),
            self.reasoning_effort.as_deref(),
            self.token_usage,
        )
        .map(|text| format!("{text}  ·  {}", crate::build_info::label()))
        .unwrap_or_else(crate::build_info::label);

        let show_jump_chip = !self.stick_to_bottom;
        let model_picker = self.model_picker_open.then(|| self.render_model_picker(cx));
        let session_title = custom_session_title(&self.session_id, self.title.as_ref())
            .map(|title| SharedString::from(title.to_owned()));

        div()
            .flex()
            .flex_col()
            .size_full()
            .relative()
            .overflow_hidden()
            .track_focus(&self.focus_handle)
            .children(session_title.map(|title| {
                div()
                    .debug_selector(|| "panel-session-title".into())
                    .px_3()
                    .pt_2()
                    .pb_1()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_size(px(12.0))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(Theme::global().TEXT_DIM)
                    .child(title)
            }))
            .children(pinned_todo.map(|payload| {
                div()
                    .debug_selector(|| "pinned-todo-card".into())
                    .flex_none()
                    .px_3()
                    .pt_1()
                    .children(latest_prompt.map(|prompt| {
                        div()
                            .debug_selector(|| "pinned-latest-prompt".into())
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .pb_1()
                            .text_size(px(11.5))
                            .text_color(Theme::global().TEXT_DIM)
                            .child(prompt)
                    }))
                    .child(render_todo_card(&payload))
            }))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .relative()
                    .child(transcript)
                    .child(crate::scrollbar::vertical(
                        &self.scroll,
                        "transcript-scrollbar",
                    ))
                    // Detached from the live end: one tap catches back up.
                    .when(show_jump_chip, |el| {
                        el.child(
                            div()
                                .id("jump-to-latest")
                                .debug_selector(|| "jump-to-latest".into())
                                .absolute()
                                .bottom_2()
                                .right_3()
                                .px_2p5()
                                .py_1()
                                .rounded_md()
                                .bg(Theme::global().HEADER_BG)
                                .border_1()
                                .border_color(Theme::global().PANEL_BORDER)
                                .text_size(px(10.5))
                                .font_family(Theme::global().FONT_MONO)
                                .text_color(Theme::global().TEXT_DIM)
                                .cursor_pointer()
                                .hover(|el| el.text_color(Theme::global().TEXT))
                                .occlude()
                                .on_mouse_down(
                                    gpui::MouseButton::Left,
                                    cx.listener(|this, _event, _window, cx| {
                                        this.stick_to_bottom = true;
                                        this.scroll.scroll_to_bottom();
                                        cx.notify();
                                    }),
                                )
                                .child("↓ latest"),
                        )
                    }),
            )
            .children(status_line.map(|text| {
                // Session events repaint this footer when its state changes. A
                // repeating GPUI animation here forces the complete transcript
                // to lay out and paint at display rate for the entire request.
                let dot = div()
                    .debug_selector(|| "panel-status-pulse".into())
                    .size(px(5.0))
                    .flex_none()
                    .rounded_full()
                    .bg(Theme::global().ACCENT);
                div()
                    .flex()
                    .flex_row()
                    .gap_1p5()
                    .items_center()
                    .px_3()
                    .py_1()
                    .text_size(px(10.5))
                    .font_family(Theme::global().FONT_MONO)
                    .text_color(Theme::global().TEXT_FAINT)
                    .child(dot)
                    .child(text)
            }))
            // Session identity: where it runs, what serves it, and how full
            // the context is. Always present, so the user never has to ask
            // "which model is this?" mid-conversation.
            .child({
                div()
                    // Tagged so a render test can prove the footer painted,
                    // not just that meta_line() produced a string.
                    .debug_selector(|| "panel-meta".into())
                    .px_3()
                    .py_1()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_size(px(10.0))
                    .font_family(Theme::global().FONT_MONO)
                    .text_color(Theme::global().TEXT_FAINT)
                    .child(meta_line)
            })
            // Input
            .child(div().px_2().py_2().child(self.input.clone()))
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|this, _event, window, cx| {
                    this.focus_input(window, cx);
                    cx.notify();
                }),
            )
            .children(model_picker)
            .into_any_element()
    }
}

/// The generated short id is useful as a fallback elsewhere, but it is not a
/// meaningful title to repeat above the conversation.
fn custom_session_title<'a>(session_id: &str, title: &'a str) -> Option<&'a str> {
    let title = title.trim();
    (!title.is_empty() && title != short_id(session_id)).then_some(title)
}

/// Defensive render-time grouping for transcripts assembled from more than one
/// event source. Streaming normally merges reasoning as it arrives, but a
/// reconnect or provider boundary can leave adjacent reasoning items behind.
/// They are one uninterrupted visual phase and should therefore paint as one
/// card. Keep the first index so expansion state remains stable.
fn coalesce_reasoning_rows(rows: Vec<(usize, Item)>) -> Vec<(usize, Item)> {
    let mut grouped: Vec<(usize, Item)> = Vec::with_capacity(rows.len());
    for (index, item) in rows {
        match (grouped.last_mut(), item) {
            (Some((_, Item::Reasoning(existing))), Item::Reasoning(text)) => {
                append_reasoning_text(existing, &text);
            }
            (_, item) => grouped.push((index, item)),
        }
    }
    grouped
}

fn append_reasoning_text(existing: &mut String, text: &str) {
    let separated = existing
        .chars()
        .next_back()
        .is_some_and(char::is_whitespace)
        || text.chars().next().is_some_and(char::is_whitespace);
    if !separated {
        existing.push_str("\n\n");
    }
    existing.push_str(text);
}

impl Focusable for Panel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

fn short_id(session_id: &str) -> String {
    let tail: String = session_id
        .chars()
        .rev()
        .take(8)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("session {tail}")
}

/// Which speaker a row belongs to, or None for rows that carry no caption.
fn role_of(item: &Item) -> Option<&'static str> {
    match item {
        Item::User(_) => Some("you"),
        Item::Assistant(_) => Some("jcode"),
        Item::Image(_)
        | Item::Reasoning(_)
        | Item::Tool { .. }
        | Item::BackgroundTask { .. }
        | Item::Todos(_)
        | Item::Error(_) => None,
    }
}

/// A labelled role row: a small caption above the message body, so a long
/// transcript stays scannable without heavyweight avatars.
fn role_caption(label: &'static str, body: gpui::AnyElement) -> gpui::AnyElement {
    div()
        .flex()
        // Restored history alternates labelled user and assistant rows. If
        // these wrappers may shrink, flex layout compresses the whole history
        // into the viewport and leaves the transcript with nothing to scroll.
        .flex_none()
        .flex_col()
        .gap_1()
        .child(
            div()
                .text_size(px(10.0))
                .text_color(Theme::global().TEXT_FAINT)
                .font_family(Theme::global().FONT_MONO)
                .child(label),
        )
        .child(body)
        .into_any_element()
}

/// The credential route serving `model`, phrased for humans. The route
/// catalog's `api_method` values are stable ids like `openai-oauth` or
/// `anthropic-api-key`; the footer says "oauth" or "api key".
fn auth_method_for_model(
    model: Option<&str>,
    routes: &[jcode_sdk::ModelRouteInfo],
) -> Option<String> {
    let model = model?;
    let route = routes.iter().find(|route| route.model == model)?;
    let method = route.api_method.to_lowercase();
    Some(if method.contains("oauth") {
        "oauth".to_string()
    } else if method.contains("api-key") || method.contains("api_key") {
        "api key".to_string()
    } else {
        method
    })
}

fn available_model_names(routes: &[jcode_sdk::ModelRouteInfo]) -> Vec<String> {
    let mut models: Vec<String> = routes
        .iter()
        .filter(|route| route.available)
        .map(|route| route.model.clone())
        .collect();
    models.sort();
    models.dedup();
    models
}

fn available_model_logo_providers(routes: &[jcode_sdk::ModelRouteInfo]) -> HashMap<String, String> {
    routes
        .iter()
        .filter(|route| route.available)
        .map(|route| {
            (
                route.model.clone(),
                model_logo_provider(&route.model, &route.api_method).to_string(),
            )
        })
        .collect()
}

fn model_logo_provider<'a>(model: &str, api_method: &'a str) -> &'a str {
    let method = api_method.to_ascii_lowercase();
    for provider in [
        "anthropic",
        "openai",
        "gemini",
        "google",
        "copilot",
        "openrouter",
        "bedrock",
        "azure",
        "cursor",
        "antigravity",
        "xai",
        "mistral",
        "deepseek",
        "kimi",
        "zai",
        "groq",
        "perplexity",
        "cerebras",
        "minimax",
        "ollama",
    ] {
        if method.contains(provider) {
            return match provider {
                "anthropic" => "anthropic-api",
                "openai" => "openai",
                other => other,
            };
        }
    }

    let model = model.to_ascii_lowercase();
    if model.starts_with("claude") {
        "anthropic-api"
    } else if model.starts_with("gpt") || model.starts_with("o1") || model.starts_with("o3") {
        "openai"
    } else if model.starts_with("gemini") {
        "gemini"
    } else if model.starts_with("grok") {
        "xai"
    } else if model.starts_with("mistral") || model.starts_with("codestral") {
        "mistral"
    } else if model.starts_with("deepseek") {
        "deepseek"
    } else {
        api_method
    }
}

/// The identity footer: directory, model (provider, auth, effort), and
/// context usage. Absent parts are simply omitted, so the line never shows
/// placeholders.
fn meta_line(
    working_dir: Option<&str>,
    model: Option<&str>,
    provider: Option<&str>,
    auth_method: Option<&str>,
    reasoning_effort: Option<&str>,
    token_usage: Option<(u64, u64, u64)>,
) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    if let Some(dir) = working_dir.filter(|dir| !dir.is_empty()) {
        parts.push(compact_dir(dir));
    }
    if let Some(model) = model {
        let mut qualifiers: Vec<&str> = Vec::new();
        if let Some(provider) = provider {
            qualifiers.push(provider);
        }
        if let Some(auth) = auth_method {
            qualifiers.push(auth);
        }
        if let Some(effort) = reasoning_effort {
            qualifiers.push(effort);
        }
        if qualifiers.is_empty() {
            parts.push(model.to_string());
        } else {
            parts.push(format!("{model} ({})", qualifiers.join(", ")));
        }
    }
    if let Some((input, output, cache_read)) = token_usage {
        let used = input + output + cache_read;
        match model.and_then(context_window_for_model) {
            Some(window) => {
                let percent = (used as f64 / window as f64 * 100.0).min(100.0);
                parts.push(format!(
                    "{} / {} ({percent:.0}%)",
                    format_tokens(used),
                    format_tokens(window)
                ));
            }
            None => parts.push(format!("{} tokens", format_tokens(used))),
        }
    }
    if parts.is_empty() {
        return None;
    }
    Some(parts.join("  ·  "))
}

/// Shorten a home-relative path for the footer.
fn compact_dir(path: &str) -> String {
    match std::env::var("HOME").ok() {
        Some(home) if path == home => "~".to_string(),
        Some(home) => match path.strip_prefix(&format!("{home}/")) {
            Some(relative) => format!("~/{relative}"),
            None => path.to_string(),
        },
        None => path.to_string(),
    }
}

/// Best-effort context window by model family. The harness API does not carry
/// the provider's exact window, so this mirrors jcode's own fallbacks for the
/// families the user actually runs; unknown models show raw token counts.
fn context_window_for_model(model: &str) -> Option<u64> {
    let m = model.to_lowercase();
    if m.starts_with("gpt-5.3-codex-spark") {
        return Some(128_000);
    }
    if m.contains("chat") && m.starts_with("gpt-5") {
        return Some(128_000);
    }
    if m.starts_with("gpt-5.4") {
        return Some(1_000_000);
    }
    if m.starts_with("gpt-5") {
        return Some(272_000);
    }
    if m.starts_with("claude-")
        || m.starts_with("fable")
        || m.contains("opus")
        || m.contains("sonnet")
        || m.contains("haiku")
    {
        return Some(200_000);
    }
    if m.starts_with("gemini-") {
        return Some(1_000_000);
    }
    None
}

/// `12500` -> `12.5k`, `1048576` -> `1.0m`; small counts stay exact.
fn format_tokens(count: u64) -> String {
    if count >= 1_000_000 {
        format!("{:.1}m", count as f64 / 1_000_000.0)
    } else if count >= 1_000 {
        format!("{:.1}k", count as f64 / 1_000.0)
    } else {
        count.to_string()
    }
}

/// Collapse whitespace and clip to a readable length.
fn condense(text: &str, limit: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= limit {
        return flat;
    }
    let clipped: String = flat.chars().take(limit).collect();
    format!("{}…", clipped.trim_end())
}

/// Like `condense`, but keeps the end: while reasoning streams, the newest
/// words are the ones worth reading.
fn condense_tail(text: &str, limit: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let count = flat.chars().count();
    if count <= limit {
        return flat;
    }
    let clipped: String = flat.chars().skip(count - limit).collect();
    format!("…{}", clipped.trim_start())
}

/// Drop ANSI escape sequences (colors, cursor moves) so terminal output
/// renders as text instead of garbage.
fn strip_ansi(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            output.push(c);
            continue;
        }
        match chars.peek() {
            // CSI: ESC [ ... final byte in @-~
            Some('[') => {
                chars.next();
                for c in chars.by_ref() {
                    if ('@'..='~').contains(&c) {
                        break;
                    }
                }
            }
            // OSC: ESC ] ... BEL or ESC \
            Some(']') => {
                chars.next();
                while let Some(c) = chars.next() {
                    if c == '\u{7}' {
                        break;
                    }
                    if c == '\u{1b}' && chars.peek() == Some(&'\\') {
                        chars.next();
                        break;
                    }
                }
            }
            // Two-character sequences like ESC ( B.
            Some(_) => {
                chars.next();
            }
            None => {}
        }
    }
    output
}

/// Parse the todo tool's concatenated output format. The tool emits the item
/// array first, followed by optional `Plan:` and `Goals:` JSON sections.
fn parse_todo_tool_output(output: &str) -> Option<TodoCardPayload> {
    let mut stream =
        serde_json::Deserializer::from_str(output.trim_start()).into_iter::<Vec<TodoCardItem>>();
    let todos = stream.next()?.ok()?;
    let remainder = output
        .trim_start()
        .get(stream.byte_offset()..)?
        .trim_start();
    let plan = remainder
        .strip_prefix("Plan:")
        .and_then(|json| {
            serde_json::Deserializer::from_str(json.trim_start())
                .into_iter::<TodoCardPlan>()
                .next()
                .and_then(Result::ok)
        })
        .unwrap_or_default();
    Some(TodoCardPayload { todos, plan })
}

fn semantic_value(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(value) => value.replace('_', " "),
        serde_json::Value::Number(value) => format!("{value}%"),
        _ => String::new(),
    }
}

fn todo_status_color(todo: &TodoCardItem) -> gpui::Rgba {
    if !todo.blocked_by.is_empty() && todo.status != "completed" {
        Theme::global().WARN
    } else {
        match todo.status.as_str() {
            "completed" => Theme::global().OK,
            "in_progress" => Theme::global().ACCENT,
            "cancelled" => Theme::global().ERROR,
            _ => Theme::global().TEXT_FAINT,
        }
    }
}

fn render_todo_marker(todo: &TodoCardItem) -> impl IntoElement {
    let color = todo_status_color(todo);
    div()
        .flex_none()
        .w(px(13.0))
        .h(px(13.0))
        .rounded_full()
        .border_1()
        .border_color(color)
        .p(px(2.0))
        .when(todo.status == "completed", |marker| {
            marker.child(div().size_full().rounded_full().bg(color))
        })
        .when(todo.status == "in_progress", |marker| {
            marker.child(div().size_full().rounded_full().bg(Theme::global().ACCENT))
        })
}

fn render_todo_card(payload: &TodoCardPayload) -> impl IntoElement {
    let total = payload.todos.len();
    let completed = payload
        .todos
        .iter()
        .filter(|todo| todo.status == "completed")
        .count();
    let progress = if total == 0 {
        0.0
    } else {
        completed as f32 / total as f32
    };

    let mut groups: Vec<(Option<&str>, Vec<&TodoCardItem>)> = Vec::new();
    for todo in &payload.todos {
        let group = todo
            .group
            .as_deref()
            .map(str::trim)
            .filter(|g| !g.is_empty());
        if let Some((_, items)) = groups.iter_mut().find(|(known, _)| *known == group) {
            items.push(todo);
        } else {
            groups.push((group, vec![todo]));
        }
    }
    groups.sort_by_key(|(group, _)| group.is_none());

    let mut body = div()
        .id("todo-card-body-scroll")
        .debug_selector(|| "todo-card-body".into())
        .flex()
        .flex_col()
        .gap_1()
        .max_h(px(132.0))
        .overflow_y_scroll();
    if payload.todos.is_empty() {
        body = body.child(
            div()
                .py_1()
                .text_size(px(12.0))
                .text_color(Theme::global().TEXT_DIM)
                .child("No tasks yet. Jcode will populate them as work is planned."),
        );
    } else {
        for (group, todos) in groups {
            let done = todos
                .iter()
                .filter(|todo| todo.status == "completed")
                .count();
            let mut section = div().flex().flex_col().gap_0p5();
            if group.is_some() || payload.todos.iter().any(|todo| todo.group.is_some()) {
                section = section.child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .text_size(px(11.0))
                        .child(
                            div()
                                .min_w_0()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(Theme::global().ACCENT_MUTED)
                                .child(group.unwrap_or("Other").to_string()),
                        )
                        .child(
                            div()
                                .text_color(Theme::global().TEXT_FAINT)
                                .child(format!("{done}/{}", todos.len())),
                        ),
                );
            }
            for todo in todos {
                let confidence = if todo.status == "completed" {
                    todo.completion_confidence
                        .as_ref()
                        .or(todo.confidence.as_ref())
                } else {
                    todo.confidence.as_ref()
                };
                let blocked = !todo.blocked_by.is_empty() && todo.status != "completed";
                section = section.child(
                    div()
                        .debug_selector(|| "todo-row".into())
                        .flex()
                        .items_start()
                        .gap_1p5()
                        .child(render_todo_marker(todo))
                        .child(div().flex_1().min_w_0().flex().flex_col().gap_0p5().when(
                            blocked || confidence.is_some(),
                            |content| {
                                content
                                    .child(
                                        div()
                                            .min_w_0()
                                            .overflow_hidden()
                                            .whitespace_nowrap()
                                            .text_ellipsis()
                                            .text_size(px(12.5))
                                            .text_color(if todo.status == "completed" {
                                                Theme::global().TEXT_DIM
                                            } else {
                                                Theme::global().TEXT
                                            })
                                            .child(todo.content.clone()),
                                    )
                                    .child(
                                        div()
                                            .flex()
                                            .flex_wrap()
                                            .gap_1()
                                            .text_size(px(9.5))
                                            .when(blocked, |meta| {
                                                meta.child(
                                                    div()
                                                        .rounded_sm()
                                                        .px_1()
                                                        .bg(Theme::global().ERROR_BG)
                                                        .text_color(Theme::global().WARN)
                                                        .child("blocked"),
                                                )
                                            })
                                            .when_some(confidence, |meta, confidence| {
                                                meta.child(
                                                    div()
                                                        .text_color(Theme::global().TEXT_FAINT)
                                                        .child(semantic_value(confidence)),
                                                )
                                            }),
                                    )
                            },
                        )),
                );
            }
            body = body.child(section);
        }
    }

    div()
        .debug_selector(|| "todo-card".into())
        .flex()
        .flex_none()
        .flex_col()
        .gap_1()
        .rounded_md()
        .border_1()
        .border_color(Theme::global().TOOL_BORDER)
        .bg(Theme::global().TOOL_BG)
        .px_2()
        .py_1p5()
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(Theme::global().HEADING)
                        .child("Tasks"),
                )
                .child(
                    div()
                        .text_size(px(10.5))
                        .text_color(Theme::global().TEXT_DIM)
                        .child(format!("{completed} of {total} complete")),
                ),
        )
        .child(
            div()
                .h(px(3.0))
                .w_full()
                .rounded_full()
                .bg(Theme::global().CODE_BORDER)
                .overflow_hidden()
                .child(
                    div()
                        .h_full()
                        .w(relative(progress))
                        .rounded_full()
                        .bg(Theme::global().OK),
                ),
        )
        .when_some(payload.plan.user_intention.clone(), |card, intention| {
            card.child(
                div()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .text_size(px(11.5))
                    .text_color(Theme::global().TEXT_DIM)
                    .child(intention),
            )
        })
        .child(body)
}

/// The human-readable intent of a tool call, with a useful argument fallback.
fn tool_summary(input: &str) -> String {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(input) else {
        return condense(input, 90);
    };
    const PREFERRED: &[&str] = &[
        "intent",
        "command",
        "query",
        "file_path",
        "path",
        "pattern",
        "url",
        "prompt",
        "content",
        "task",
        "action",
    ];
    for key in PREFERRED {
        if let Some(found) = value.get(*key).and_then(json_scalar) {
            if !found.trim().is_empty() {
                let found = condense(&found, 90);
                return found;
            }
        }
    }
    condense(input, 90)
}

fn json_scalar(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::String(text) => Some(text.clone()),
        serde_json::Value::Number(number) => Some(number.to_string()),
        serde_json::Value::Bool(flag) => Some(flag.to_string()),
        _ => None,
    }
}

/// Expanded tool detail: the exact invocation plus a clipped output tail.
fn tool_detail(name: &str, input: &str, output: &str) -> String {
    let arguments = if input.trim().is_empty() {
        "{}"
    } else {
        input.trim()
    };
    let pretty = serde_json::from_str::<serde_json::Value>(arguments)
        .ok()
        .and_then(|value| serde_json::to_string_pretty(&value).ok())
        .unwrap_or_else(|| arguments.to_string());
    let mut parts = vec![format!(
        "Tool call\n{}\n{}",
        name.trim(),
        clip_lines(&pretty, 40)
    )];
    if !output.trim().is_empty() {
        parts.push(format!(
            "Output\n{}",
            clip_lines(strip_ansi(output).trim(), 40)
        ));
    }
    parts.join("\n\n")
}

/// Keep a block short from the top, noting how much was hidden.
fn acknowledge_next(
    pending: &mut VecDeque<usize>,
    accepted: &mut HashMap<usize, Instant>,
    now: Instant,
) {
    if let Some(index) = pending.pop_front() {
        accepted.insert(index, now);
    }
}

/// Keep a block short by keeping its head and tail: for command output the
/// end (results, errors) usually matters more than the middle.
fn clip_lines(text: &str, max_lines: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() <= max_lines {
        return text.to_string();
    }
    let head = max_lines * 2 / 3;
    let tail = max_lines - head;
    let hidden = lines.len() - head - tail;
    let mut kept = lines[..head].join("\n");
    kept.push_str(&format!("\n… {hidden} lines hidden …\n"));
    kept.push_str(&lines[lines.len() - tail..].join("\n"));
    kept
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn restored_scroll_is_not_replaced_when_history_reattaches(cx: &mut gpui::TestAppContext) {
        let (workspace, vcx) = cx.add_window_view(|_, cx| {
            let mut workspace =
                crate::workspace::Workspace::for_test(crate::learning::Coach::new(), cx);
            workspace.push_test_panel("session-a", cx);
            workspace
        });
        let panel = workspace
            .read_with(vcx, |workspace, _| workspace.test_panel(0))
            .expect("panel exists");
        let mut saved = panel.read_with(vcx, |panel, cx| panel.snapshot(cx));
        saved.scroll_y = -137.0;
        saved.stick_to_bottom = false;

        panel.update(vcx, |panel, cx| {
            panel.restore_snapshot(saved, cx);
            panel.load_history(Vec::new(), Vec::new(), cx);
            assert_eq!(f32::from(panel.scroll.offset().y), -137.0);
            assert!(!panel.stick_to_bottom);
        });
    }

    #[gpui::test]
    fn restored_alternating_history_still_overflows_and_scrolls(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| crate::bind_workspace_keys(cx));
        let (workspace, vcx) = cx.add_window_view(|_, cx| {
            let mut workspace =
                crate::workspace::Workspace::for_test(crate::learning::Coach::new(), cx);
            workspace.push_test_panel("session-a", cx);
            workspace
        });
        let panel = workspace
            .read_with(vcx, |workspace, _| workspace.test_panel(0))
            .expect("panel exists");

        // Reload restores the scroll handle before Watch returns history. Paint
        // that empty intermediate state, matching the real asynchronous handoff.
        panel.update(vcx, |panel, cx| {
            panel.items.clear();
            let mut snapshot = panel.snapshot(cx);
            snapshot.scroll_y = -137.0;
            snapshot.stick_to_bottom = false;
            panel.restore_snapshot(snapshot, cx);
            cx.notify();
        });
        vcx.run_until_parked();

        panel.update(vcx, |panel, cx| {
            let history = (0..80)
                .map(|index| jcode_sdk::HistoryMessage {
                    role: if index % 2 == 0 { "user" } else { "assistant" }.into(),
                    content: format!("restored message {index}"),
                })
                .collect();
            panel.load_history(history, Vec::new(), cx);
        });
        vcx.run_until_parked();
        panel.update(vcx, |_panel, cx| cx.notify());
        vcx.run_until_parked();

        assert!(
            vcx.debug_bounds("transcript-scrollbar").is_some(),
            "restored alternating history must produce scrollable overflow"
        );
        let before = panel.read_with(vcx, |panel, _| panel.scroll.offset().y);
        let transcript = vcx.debug_bounds("transcript").expect("transcript painted");
        vcx.simulate_event(gpui::ScrollWheelEvent {
            position: transcript.center(),
            // Physical mouse wheels arrive as non-precise line deltas, unlike
            // the precise pixel delta used by a touchpad.
            delta: gpui::ScrollDelta::Lines(gpui::point(0.0, 3.0)),
            modifiers: gpui::Modifiers::default(),
            touch_phase: gpui::TouchPhase::Moved,
        });
        vcx.run_until_parked();
        let after = panel.read_with(vcx, |panel, _| panel.scroll.offset().y);
        assert_ne!(after, before, "wheel input must move restored history");
    }

    #[test]
    fn message_acceptance_promotes_the_oldest_pending_prompt() {
        let now = Instant::now();
        let mut pending = VecDeque::from([4, 7]);
        let mut accepted = HashMap::new();
        acknowledge_next(&mut pending, &mut accepted, now);
        assert_eq!(pending, VecDeque::from([7]));
        assert_eq!(accepted.get(&4), Some(&now));
    }

    #[test]
    fn tool_summary_prefers_intent_over_implementation_details() {
        assert_eq!(
            tool_summary(r#"{"intent":"look","command":"cargo test"}"#),
            "look"
        );
        assert_eq!(tool_summary(r#"{"command":"cargo test"}"#), "cargo test");
        assert_eq!(tool_summary(r#"{"other":1}"#), r#"{"other":1}"#);
    }

    #[test]
    fn condense_flattens_and_clips() {
        assert_eq!(condense("a\n  b\tc", 90), "a b c");
        assert_eq!(condense("abcdef", 3), "abc…");
    }

    #[test]
    fn panel_header_only_shows_a_meaningful_session_title() {
        let session_id = "01JABCDEF0123456789";
        let fallback = short_id(session_id);

        assert_eq!(
            custom_session_title(session_id, "Plan the release"),
            Some("Plan the release")
        );
        assert_eq!(custom_session_title(session_id, &fallback), None);
        assert_eq!(custom_session_title(session_id, "  "), None);
    }

    #[test]
    fn tool_detail_pretty_prints_and_clips() {
        let detail = tool_detail("bash", r#"{"a":1}"#, "line1\nline2");
        assert!(detail.starts_with("Tool call\nbash\n"));
        assert!(detail.contains("\"a\": 1"));
        assert!(detail.contains("\n\nOutput\nline1\nline2"));
        assert!(detail.contains("line2"));
        let long: String = (0..50).map(|n| format!("l{n}\n")).collect();
        let clipped = tool_detail("bash", "{}", &long);
        assert!(clipped.contains("Tool call\nbash\n{}"));
        assert!(clipped.contains("lines hidden"));
        // The tail survives: results and errors live at the end of output.
        assert!(clipped.contains("l49"));
        assert!(clipped.contains("l0"));
    }

    #[test]
    fn reasoning_tail_keeps_the_newest_words() {
        assert_eq!(condense_tail("short", 200), "short");
        let tail = condense_tail(&"word ".repeat(100), 20);
        assert!(tail.starts_with('…'));
        assert!(tail.ends_with("word"));
    }

    #[gpui::test]
    fn full_live_reasoning_stays_expanded_after_it_settles(cx: &mut gpui::TestAppContext) {
        let (workspace, vcx) = cx.add_window_view(|_, cx| {
            let mut workspace =
                crate::workspace::Workspace::for_test(crate::learning::Coach::new(), cx);
            workspace.push_test_panel("session-a", cx);
            workspace
        });
        let panel = workspace
            .read_with(vcx, |workspace, _| workspace.test_panel(0))
            .expect("panel exists");

        panel.update(vcx, |panel, _| {
            panel.items.clear();
            let complete_thought = "a complete train of thought ".repeat(12);
            panel.streaming_reasoning = complete_thought.clone();
            panel.flush_reasoning();

            assert!(panel.expanded_reasoning.contains(&0));
            assert!(!panel.expanded_reasoning.contains(&(usize::MAX - 1)));
            assert!(matches!(panel.items.as_slice(), [Item::Reasoning(text)] if text == &complete_thought));
        });
    }

    #[test]
    fn adjacent_reasoning_segments_share_one_visual_block() {
        let mut items = vec![Item::Reasoning("first thought".into())];
        append_reasoning(&mut items, "second thought".into());

        assert_eq!(items.len(), 1);
        assert!(matches!(
            &items[0],
            Item::Reasoning(text) if text == "first thought\n\nsecond thought"
        ));
    }

    #[test]
    fn adjacent_reasoning_rows_are_coalesced_before_rendering() {
        let rows = vec![
            (4, Item::Reasoning("first thought".into())),
            (5, Item::Reasoning("second thought".into())),
            (usize::MAX - 1, Item::Reasoning("live thought".into())),
        ];

        let grouped = coalesce_reasoning_rows(rows);
        assert_eq!(grouped.len(), 1);
        assert!(matches!(
            &grouped[0],
            (4, Item::Reasoning(text))
                if text == "first thought\n\nsecond thought\n\nlive thought"
        ));
    }

    #[test]
    fn reasoning_after_another_item_starts_a_new_block() {
        let mut items = vec![Item::Assistant("answer".into())];
        append_reasoning(&mut items, "new thought".into());

        assert_eq!(items.len(), 2);
        assert!(matches!(&items[1], Item::Reasoning(text) if text == "new thought"));
    }

    #[test]
    fn ansi_escapes_are_stripped_from_tool_output() {
        assert_eq!(strip_ansi("\u{1b}[1;32mok\u{1b}[0m done"), "ok done");
        assert_eq!(strip_ansi("\u{1b}]0;title\u{7}text"), "text");
        assert_eq!(strip_ansi("plain"), "plain");
        assert!(tool_detail("bash", "{}", "\u{1b}[31mred\u{1b}[0m").contains("red"));
        assert!(!tool_detail("bash", "{}", "\u{1b}[31mred\u{1b}[0m").contains('\u{1b}'));
    }

    fn route(model: &str, api_method: &str) -> jcode_sdk::ModelRouteInfo {
        jcode_sdk::ModelRouteInfo {
            model: model.into(),
            provider: "openai".into(),
            api_method: api_method.into(),
            available: true,
            detail: String::new(),
        }
    }

    #[test]
    fn auth_method_is_read_from_the_current_models_route() {
        let routes = vec![
            route("gpt-5.6-sol", "openai-oauth"),
            route("claude-fable-5", "anthropic-api-key"),
        ];
        assert_eq!(
            auth_method_for_model(Some("gpt-5.6-sol"), &routes).as_deref(),
            Some("oauth")
        );
        assert_eq!(
            auth_method_for_model(Some("claude-fable-5"), &routes).as_deref(),
            Some("api key")
        );
        // Unknown model or no model: nothing to claim.
        assert_eq!(auth_method_for_model(Some("other"), &routes), None);
        assert_eq!(auth_method_for_model(None, &routes), None);
    }

    #[test]
    fn model_picker_only_offers_available_routes() {
        let mut unavailable = route("luna", "openai-api-key");
        unavailable.available = false;
        let routes = vec![
            unavailable,
            route("gpt-5.6-sol", "openai-api-key"),
            route("gpt-5.6-sol", "openai-oauth"),
            route("claude-fable-5", "anthropic-api-key"),
        ];

        assert_eq!(
            available_model_names(&routes),
            vec!["claude-fable-5", "gpt-5.6-sol"]
        );
    }

    #[gpui::test]
    fn runtime_catalog_only_populates_picker_with_available_routes(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| crate::bind_workspace_keys(cx));
        let (workspace, vcx) = cx.add_window_view(|_, cx| {
            let mut workspace =
                crate::workspace::Workspace::for_test(crate::learning::Coach::new(), cx);
            workspace.push_test_panel("session-a", cx);
            workspace
        });

        workspace.update(vcx, |workspace, cx| {
            let panel = workspace.test_panel(0).expect("panel exists");
            panel.update(cx, |panel, cx| {
                let mut unavailable = route("luna", "openai-api-key");
                unavailable.available = false;
                panel.apply(
                    &ApiEvent::RuntimeInfo {
                        session_id: "session-a".into(),
                        provider: Some("openai".into()),
                        model: Some("gpt-5.6-sol".into()),
                        reasoning_effort: None,
                        routes: vec![unavailable, route("gpt-5.6-sol", "openai-api-key")],
                    },
                    cx,
                );
                assert_eq!(
                    panel.input.read(cx).command_models(),
                    &["gpt-5.6-sol"],
                    "an unavailable Luna route must not reach the visible picker"
                );
            });
        });
    }

    #[test]
    fn meta_line_shows_directory_model_auth_and_context() {
        let line = meta_line(
            Some("/srv/project"),
            Some("gpt-5.6-sol"),
            Some("openai"),
            Some("oauth"),
            Some("high"),
            Some((100_000, 8_000, 28_000)),
        )
        .unwrap();
        assert_eq!(
            line,
            "/srv/project  ·  gpt-5.6-sol (openai, oauth, high)  ·  136.0k / 272.0k (50%)"
        );
    }

    #[test]
    fn meta_line_omits_missing_parts_instead_of_showing_placeholders() {
        // No identity at all: no footer.
        assert_eq!(meta_line(None, None, None, None, None, None), None);
        // Model only: just the model, no empty parens.
        assert_eq!(
            meta_line(None, Some("gpt-5.6"), None, None, None, None).as_deref(),
            Some("gpt-5.6")
        );
        // Unknown model family: raw token count, no bogus percentage.
        assert_eq!(
            meta_line(
                None,
                Some("mystery-model"),
                None,
                None,
                None,
                Some((1_500, 500, 0))
            )
            .as_deref(),
            Some("mystery-model  ·  2.0k tokens")
        );
    }

    #[test]
    fn context_windows_match_the_families_jcode_uses() {
        assert_eq!(context_window_for_model("gpt-5.6-sol"), Some(272_000));
        assert_eq!(context_window_for_model("gpt-5.4-alto"), Some(1_000_000));
        assert_eq!(
            context_window_for_model("gpt-5.2-chat-latest"),
            Some(128_000)
        );
        assert_eq!(
            context_window_for_model("claude-sonnet-4-20250514"),
            Some(200_000)
        );
        assert_eq!(context_window_for_model("gemini-3-pro"), Some(1_000_000));
        assert_eq!(context_window_for_model("mystery"), None);
    }

    #[test]
    fn token_counts_format_compactly() {
        assert_eq!(format_tokens(950), "950");
        assert_eq!(format_tokens(12_500), "12.5k");
        assert_eq!(format_tokens(1_048_576), "1.0m");
    }

    /// The acceptance path for the scroll-lock fix: a real wheel event over a
    /// real painted transcript releases stick-to-bottom, the "↓ latest" chip
    /// paints, and clicking the chip re-engages following. Before the fix the
    /// panel yanked the view back down on every streamed event.
    #[gpui::test]
    fn scrolling_up_releases_follow_mode_and_the_chip_restores_it(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| crate::bind_workspace_keys(cx));
        let (workspace, vcx) = cx.add_window_view(|_, cx| {
            let mut workspace =
                crate::workspace::Workspace::for_test(crate::learning::Coach::new(), cx);
            workspace.push_test_panel("session-a", cx);
            workspace
        });
        vcx.run_until_parked();
        let panel = workspace
            .read_with(vcx, |workspace, _| workspace.test_panel(0))
            .expect("panel exists");

        // A long transcript, so the scroll region actually overflows.
        panel.update(vcx, |panel, cx| {
            for n in 0..80 {
                panel.items.push(Item::Assistant(format!("message {n}")));
            }
            cx.notify();
        });
        vcx.run_until_parked();
        // Scroll metrics are produced during layout. Repaint once with those
        // measured metrics, as the running app does on its next frame.
        panel.update(vcx, |_panel, cx| cx.notify());
        vcx.run_until_parked();
        assert!(
            panel.read_with(vcx, |panel, _| panel.stick_to_bottom),
            "a fresh panel follows the stream"
        );
        assert!(
            vcx.debug_bounds("jump-to-latest").is_none(),
            "no chip while following"
        );
        let scrollbar_before = vcx
            .debug_bounds("transcript-scrollbar")
            .expect("an overflowing transcript paints a scrollbar");
        assert_eq!(scrollbar_before.size.width, px(4.0));
        assert!(scrollbar_before.size.height >= px(28.0));

        // A real upward wheel event over the transcript.
        let transcript = vcx.debug_bounds("transcript").expect("transcript painted");
        vcx.simulate_event(gpui::ScrollWheelEvent {
            position: transcript.center(),
            delta: gpui::ScrollDelta::Pixels(gpui::point(px(0.), px(60.))),
            modifiers: gpui::Modifiers::default(),
            touch_phase: gpui::TouchPhase::Moved,
        });
        vcx.run_until_parked();
        panel.update(vcx, |_panel, cx| cx.notify());
        vcx.run_until_parked();
        assert!(
            !panel.read_with(vcx, |panel, _| panel.stick_to_bottom),
            "scrolling up must release follow mode"
        );
        let scrollbar_after = vcx
            .debug_bounds("transcript-scrollbar")
            .expect("the scrollbar remains visible after scrolling");
        assert_eq!(scrollbar_after.size.width, px(4.0));
        let chip = vcx
            .debug_bounds("jump-to-latest")
            .expect("the catch-up chip paints once detached");
        assert!(chip.size.width > px(0.) && chip.size.height > px(0.));

        // While detached, streamed events must not yank the view back down.
        let offset_before = panel.read_with(vcx, |panel, _| panel.scroll.offset().y);
        panel.update(vcx, |panel, cx| {
            panel.apply(
                &ApiEvent::TextDelta {
                    session_id: "session-a".into(),
                    text: "more streamed text".into(),
                },
                cx,
            );
        });
        vcx.run_until_parked();
        let offset_after = panel.read_with(vcx, |panel, _| panel.scroll.offset().y);
        assert_eq!(
            offset_before, offset_after,
            "streaming must not move a detached viewport"
        );

        // Clicking the chip re-engages following and removes the chip.
        vcx.simulate_click(chip.center(), gpui::Modifiers::default());
        vcx.run_until_parked();
        assert!(
            panel.read_with(vcx, |panel, _| panel.stick_to_bottom),
            "the chip must restore follow mode"
        );
        assert!(
            vcx.debug_bounds("jump-to-latest").is_none(),
            "the chip disappears once following again"
        );
    }

    /// The acceptance path for the code-copy affordance: the button paints in
    /// a real assistant message and a real click puts the code body (not the
    /// fence syntax) on the clipboard.
    #[gpui::test]
    fn clicking_copy_on_a_code_block_fills_the_clipboard(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| crate::bind_workspace_keys(cx));
        let (workspace, vcx) = cx.add_window_view(|_, cx| {
            let mut workspace =
                crate::workspace::Workspace::for_test(crate::learning::Coach::new(), cx);
            workspace.push_test_panel("session-a", cx);
            workspace
        });
        vcx.run_until_parked();
        let panel = workspace
            .read_with(vcx, |workspace, _| workspace.test_panel(0))
            .expect("panel exists");
        panel.update(vcx, |panel, cx| {
            panel.items.push(Item::Assistant(
                "```rust\nfn main() {\n    println!(\"hi\");\n}\n```".into(),
            ));
            cx.notify();
        });
        vcx.run_until_parked();

        let button = vcx
            .debug_bounds("code-copy")
            .expect("the copy button paints on a fenced block");
        vcx.simulate_click(button.center(), gpui::Modifiers::default());
        vcx.run_until_parked();

        let copied = vcx
            .update(|_, cx| cx.read_from_clipboard())
            .and_then(|item| item.text())
            .expect("clicking copy fills the clipboard");
        assert_eq!(copied, "fn main() {\n    println!(\"hi\");\n}");
    }

    /// The acceptance path for tool rows: the size hint paints on a collapsed
    /// finished call, a real click expands the detail (ANSI-clean, head and
    /// tail both present), and a second click collapses it again.
    #[gpui::test]
    fn background_progress_updates_one_native_card(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| crate::bind_workspace_keys(cx));
        let (workspace, vcx) = cx.add_window_view(|_, cx| {
            let mut workspace =
                crate::workspace::Workspace::for_test(crate::learning::Coach::new(), cx);
            workspace.push_test_panel("session-a", cx);
            workspace
        });
        vcx.run_until_parked();
        let panel = workspace
            .read_with(vcx, |workspace, _| workspace.test_panel(0))
            .expect("panel exists");

        for (percent, summary, done) in [
            (Some(35.0), "35% · Running tests", false),
            (None, "✓ completed · 8.2s · exit 0", true),
        ] {
            panel.update(vcx, |panel, cx| {
                panel.apply(
                    &ApiEvent::BackgroundProgress {
                        session_id: "session-a".into(),
                        task_id: "task-42".into(),
                        label: "Workspace tests".into(),
                        percent,
                        summary: summary.into(),
                        done,
                    },
                    cx,
                );
            });
            vcx.run_until_parked();
        }

        assert!(vcx.debug_bounds("background-task-card").is_some());
        panel.read_with(vcx, |panel, _| {
            let tasks: Vec<_> = panel
                .items
                .iter()
                .filter_map(|item| match item {
                    Item::BackgroundTask {
                        task_id,
                        summary,
                        percent,
                        done,
                        ..
                    } => Some((task_id, summary, percent, done)),
                    _ => None,
                })
                .collect();
            assert_eq!(tasks.len(), 1, "progress ticks update rather than append");
            assert_eq!(tasks[0].0, "task-42");
            assert_eq!(tasks[0].1, "✓ completed · 8.2s · exit 0");
            assert_eq!(*tasks[0].2, None);
            assert!(*tasks[0].3);
        });
    }

    #[gpui::test]
    fn clicking_a_tool_row_expands_clean_detail_and_collapses_again(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| crate::bind_workspace_keys(cx));
        let (workspace, vcx) = cx.add_window_view(|_, cx| {
            let mut workspace =
                crate::workspace::Workspace::for_test(crate::learning::Coach::new(), cx);
            workspace.push_test_panel("session-a", cx);
            workspace
        });
        vcx.run_until_parked();
        let panel = workspace
            .read_with(vcx, |workspace, _| workspace.test_panel(0))
            .expect("panel exists");

        // A finished call with long, ANSI-colored output: the shapes the
        // clipping and stripping paths exist for.
        let output: String = (0..60)
            .map(|n| format!("\u{1b}[32mline {n}\u{1b}[0m\n"))
            .collect();
        panel.update(vcx, |panel, cx| {
            panel.items.push(Item::Tool {
                call_id: "call-1".into(),
                name: "bash".into(),
                input: r#"{"command":"make build","intent":"compile"}"#.into(),
                output,
                done: true,
                error: None,
            });
            cx.notify();
        });
        vcx.run_until_parked();

        // Collapsed: the size hint paints, the detail does not.
        let hint = vcx
            .debug_bounds("tool-output-size")
            .expect("a collapsed finished call shows its output size");
        assert!(hint.size.width > px(0.));
        assert!(
            vcx.debug_bounds("tool-detail").is_none(),
            "detail stays hidden until expanded"
        );

        // A real click on the header expands it.
        let header = vcx
            .debug_bounds("tool-header")
            .expect("tool header painted");
        vcx.simulate_click(header.center(), gpui::Modifiers::default());
        vcx.run_until_parked();
        let detail = vcx
            .debug_bounds("tool-detail")
            .expect("clicking the header expands the detail");
        assert!(detail.size.height > px(0.));
        assert!(
            vcx.debug_bounds("tool-output-size").is_none(),
            "the size hint yields to the expanded detail"
        );
        // The rendered detail is the formatted string: ANSI-free, head and
        // tail kept around the fold marker.
        let rendered = panel.read_with(vcx, |panel, _| match &panel.items[0] {
            Item::Tool {
                name,
                input,
                output,
                ..
            } => tool_detail(name, input, output),
            other => panic!("expected the tool row, got {other:?}"),
        });
        assert!(!rendered.contains('\u{1b}'), "detail must be ANSI-free");
        assert!(rendered.contains("Tool call\nbash\n"));
        assert!(rendered.contains("\"command\": \"make build\""));
        assert!(rendered.contains("\n\nOutput\n"));
        assert!(rendered.contains("line 0") && rendered.contains("line 59"));
        assert!(rendered.contains("lines hidden"));

        // A second click collapses it again.
        vcx.simulate_click(header.center(), gpui::Modifiers::default());
        vcx.run_until_parked();
        assert!(
            vcx.debug_bounds("tool-detail").is_none(),
            "a second click collapses the detail"
        );
    }

    /// Legacy harness input deltas have no call id. Exercise the real event and
    /// paint path so an intent cannot silently disappear between transport and
    /// the visible tool header again.
    #[gpui::test]
    fn legacy_tool_input_renders_its_intent_in_the_tool_header(cx: &mut gpui::TestAppContext) {
        let (workspace, vcx) = cx.add_window_view(|_, cx| {
            let mut workspace =
                crate::workspace::Workspace::for_test(crate::learning::Coach::new(), cx);
            workspace.push_test_panel("session-a", cx);
            workspace
        });
        vcx.run_until_parked();
        let panel = workspace
            .read_with(vcx, |workspace, _| workspace.test_panel(0))
            .expect("panel exists");

        panel.update(vcx, |panel, cx| {
            panel.apply(
                &ApiEvent::ToolStart {
                    session_id: "session-a".into(),
                    call_id: "call-1".into(),
                    name: "bash".into(),
                },
                cx,
            );
            panel.apply(
                &ApiEvent::ToolInputDelta {
                    session_id: "session-a".into(),
                    call_id: String::new(),
                    delta: r#"{"intent":"check the build","command":"cargo test"}"#.into(),
                },
                cx,
            );
        });
        vcx.run_until_parked();

        let summary = panel.read_with(vcx, |panel, _| match &panel.items[0] {
            Item::Tool { input, .. } => tool_summary(input),
            other => panic!("expected the tool row, got {other:?}"),
        });
        assert_eq!(summary, "check the build");
        let rendered = vcx
            .debug_bounds("tool-summary")
            .expect("the intent summary paints in the tool header");
        assert!(rendered.size.width > px(0.) && rendered.size.height > px(0.));
    }

    /// Tool cards must retain their intrinsic row height when enough of them
    /// are appended to make the transcript scroll. Without `flex_none`, the
    /// transcript's flex layout distributes the height deficit across every
    /// card and visibly squashes their headers.
    #[gpui::test]
    fn tool_rows_do_not_shrink_when_the_transcript_overflows(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| crate::bind_workspace_keys(cx));
        let (workspace, vcx) = cx.add_window_view(|_, cx| {
            let mut workspace =
                crate::workspace::Workspace::for_test(crate::learning::Coach::new(), cx);
            workspace.push_test_panel("session-a", cx);
            workspace
        });
        vcx.run_until_parked();
        let panel = workspace
            .read_with(vcx, |workspace, _| workspace.test_panel(0))
            .expect("panel exists");

        panel.update(vcx, |panel, cx| {
            panel.items = vec![Item::Tool {
                call_id: "call-0".into(),
                name: "bash".into(),
                input: r#"{"command":"echo 0"}"#.into(),
                output: "done".into(),
                done: true,
                error: None,
            }];
            cx.notify();
        });
        vcx.run_until_parked();
        let baseline_height = vcx
            .debug_bounds("tool-card")
            .expect("a tool card paints before the transcript overflows")
            .size
            .height;

        panel.update(vcx, |panel, cx| {
            panel.items.extend((1..30).map(|index| Item::Tool {
                call_id: format!("call-{index}"),
                name: "bash".into(),
                input: format!(r#"{{"command":"echo {index}"}}"#),
                output: "done".into(),
                done: true,
                error: None,
            }));
            cx.notify();
        });
        vcx.run_until_parked();

        let overflowing_height = vcx
            .debug_bounds("tool-card")
            .expect("an overflowing transcript still paints a tool card")
            .size
            .height;
        assert_eq!(
            overflowing_height, baseline_height,
            "overflow changed tool-card height from {baseline_height:?} to {overflowing_height:?}"
        );
    }

    /// The review fixture itself must paint: every transcript shape the demo
    /// seeds (user, reasoning, running and finished tools with ANSI output,
    /// full markdown, error) renders together in one painted window without
    /// panicking, and the signature regions all occupy space.
    #[gpui::test]
    fn the_demo_transcript_paints_every_item_shape(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| crate::bind_workspace_keys(cx));
        let (workspace, vcx) = cx.add_window_view(|_, cx| {
            let mut workspace =
                crate::workspace::Workspace::for_test(crate::learning::Coach::new(), cx);
            workspace.push_test_panel("session-a", cx);
            workspace
        });
        vcx.run_until_parked();
        let panel = workspace
            .read_with(vcx, |workspace, _| workspace.test_panel(0))
            .expect("panel exists");

        // The same items JCODE_DESKTOP_DEMO_TRANSCRIPT=1 seeds, minus the
        // env-var gate so the test is hermetic.
        panel.update(vcx, |panel, cx| {
            panel.items = demo_item_fixtures();
            assert!(panel.items.len() >= 6, "demo covers every item shape");
            cx.notify();
        });
        vcx.run_until_parked();

        for selector in ["tool-header", "tool-output-size", "md-quote", "code-copy"] {
            let bounds = vcx
                .debug_bounds(selector)
                .unwrap_or_else(|| panic!("{selector} should paint in the demo"));
            assert!(
                bounds.size.width > px(0.) && bounds.size.height > px(0.),
                "{selector} must occupy space"
            );
        }
    }

    /// Blockquotes paint as a distinct region in a real assistant message.
    #[gpui::test]
    fn blockquotes_paint_in_a_real_transcript(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| crate::bind_workspace_keys(cx));
        let (workspace, vcx) = cx.add_window_view(|_, cx| {
            let mut workspace =
                crate::workspace::Workspace::for_test(crate::learning::Coach::new(), cx);
            workspace.push_test_panel("session-a", cx);
            workspace
        });
        vcx.run_until_parked();
        let panel = workspace
            .read_with(vcx, |workspace, _| workspace.test_panel(0))
            .expect("panel exists");
        panel.update(vcx, |panel, cx| {
            panel
                .items
                .push(Item::Assistant("> a quoted line\n> and another".into()));
            cx.notify();
        });
        vcx.run_until_parked();
        let quote = vcx
            .debug_bounds("md-quote")
            .expect("the quote region paints");
        assert!(quote.size.width > px(0.) && quote.size.height > px(0.));
    }

    /// The acceptance path: real events land in a real panel inside a painted
    /// window, and the footer element occupies space on screen. Without this,
    /// the tests above only prove the string is right, not that anyone sees it.
    #[gpui::test]
    fn the_identity_footer_paints_after_runtime_info_and_token_events(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| crate::bind_workspace_keys(cx));
        let (workspace, vcx) = cx.add_window_view(|window, cx| {
            let mut workspace =
                crate::workspace::Workspace::for_test(crate::learning::Coach::new(), cx);
            workspace.push_test_panel("session-a", cx);
            let _ = window;
            workspace
        });
        vcx.run_until_parked();

        // The footer owns stable layout space even before runtime identity
        // arrives, so the composer does not jump when attach completes.
        assert!(
            vcx.debug_bounds("panel-meta").is_some(),
            "identity footer reserves layout space before metadata arrives"
        );

        // The events the harness worker forwards after attach and during a turn.
        workspace.update(vcx, |workspace, cx| {
            let panel = workspace.test_panel(0).expect("panel exists");
            panel.update(cx, |panel, cx| {
                panel.apply(
                    &ApiEvent::RuntimeInfo {
                        session_id: "session-a".into(),
                        provider: Some("openai".into()),
                        model: Some("gpt-5.6-sol".into()),
                        routes: vec![jcode_sdk::ModelRouteInfo {
                            model: "gpt-5.6-sol".into(),
                            provider: "openai".into(),
                            api_method: "openai-oauth".into(),
                            available: true,
                            detail: String::new(),
                        }],
                        reasoning_effort: None,
                    },
                    cx,
                );
                panel.apply(
                    &ApiEvent::TokenUsage {
                        session_id: "session-a".into(),
                        input: 100_000,
                        output: 8_000,
                        cache_read_input: Some(28_000),
                    },
                    cx,
                );
                assert_eq!(panel.model.as_deref(), Some("gpt-5.6-sol"));
                assert_eq!(panel.provider.as_deref(), Some("openai"));
                assert_eq!(panel.auth_method.as_deref(), Some("oauth"));

                // Repeating the current model must not wipe the still-correct
                // auth label. A real switch to another model invalidates it.
                panel.apply(
                    &ApiEvent::ModelInfo {
                        session_id: "session-a".into(),
                        provider: Some("openai".into()),
                        model: Some("gpt-5.6-sol".into()),
                        reasoning_effort: None,
                    },
                    cx,
                );
                assert_eq!(
                    panel.auth_method.as_deref(),
                    Some("oauth"),
                    "same-model broadcast must not clear the auth label"
                );
                panel.apply(
                    &ApiEvent::ModelInfo {
                        session_id: "session-a".into(),
                        provider: Some("anthropic".into()),
                        model: Some("claude-fable-5".into()),
                        reasoning_effort: None,
                    },
                    cx,
                );
                assert_eq!(
                    panel.auth_method, None,
                    "a real model switch invalidates the auth label"
                );
            });
        });
        vcx.run_until_parked();

        let bounds = vcx
            .debug_bounds("panel-meta")
            .expect("the identity footer should have painted");
        assert!(
            bounds.size.width > gpui::px(0.) && bounds.size.height > gpui::px(0.),
            "the footer must occupy real space, got {bounds:?}"
        );
    }

    #[gpui::test]
    fn reconnect_history_recovers_a_response_missed_by_the_event_stream(
        cx: &mut gpui::TestAppContext,
    ) {
        let (workspace, vcx) = cx.add_window_view(|_, cx| {
            let mut workspace =
                crate::workspace::Workspace::for_test(crate::learning::Coach::new(), cx);
            workspace.push_test_panel("session-a", cx);
            workspace
        });
        let mut panel = None;
        workspace.update(vcx, |workspace, _| panel = workspace.test_panel(0));
        let panel = panel.expect("test panel exists");

        panel.update(vcx, |panel, cx| {
            panel.history_loaded = true;
            panel.items = vec![Item::User("hello".into())];
            panel.load_history(
                vec![
                    jcode_sdk::HistoryMessage {
                        role: "user".into(),
                        content: "hello".into(),
                    },
                    jcode_sdk::HistoryMessage {
                        role: "assistant".into(),
                        content: "recovered response".into(),
                    },
                ],
                Vec::new(),
                cx,
            );
            assert!(matches!(
                panel.items.last(),
                Some(Item::Assistant(text)) if text == "recovered response"
            ));

            panel.items = vec![Item::User("next".into())];
            panel.streaming_text = "partial".into();
            panel.load_history(
                vec![jcode_sdk::HistoryMessage {
                    role: "assistant".into(),
                    content: "partial response completed".into(),
                }],
                Vec::new(),
                cx,
            );
            assert_eq!(panel.streaming_text, "partial response completed");
        });
    }

    #[gpui::test]
    fn model_read_images_are_anchored_and_deduplicated_in_the_transcript(
        cx: &mut gpui::TestAppContext,
    ) {
        let (workspace, vcx) = cx.add_window_view(|_, cx| {
            let mut workspace =
                crate::workspace::Workspace::for_test(crate::learning::Coach::new(), cx);
            workspace.push_test_panel("session-a", cx);
            workspace
        });
        let mut panel = None;
        workspace.update(vcx, |workspace, _| panel = workspace.test_panel(0));
        let panel = panel.expect("test panel exists");

        panel.update(vcx, |panel, cx| {
            panel.items = vec![Item::Tool {
                call_id: "read-1".into(),
                name: "read".into(),
                input: r#"{"file_path":"chart.png"}"#.into(),
                output: "image loaded".into(),
                done: true,
                error: None,
            }];
            let event = ApiEvent::SidePaneImages {
                session_id: "session-a".into(),
                images: vec![jcode_sdk::RenderedImage {
                    media_type: "image/png".into(),
                    data: "iVBORw0KGgo=".into(),
                    label: Some("chart.png".into()),
                    source: jcode_sdk::RenderedImageSource::ToolResult {
                        tool_name: "read".into(),
                    },
                    anchor: Some(jcode_sdk::RenderedImageAnchor::ToolCall {
                        id: "read-1".into(),
                    }),
                }],
            };
            panel.apply(&event, cx);
            panel.apply(&event, cx);

            assert_eq!(panel.items.len(), 2, "replayed image events are deduplicated");
            assert!(matches!(&panel.items[1], Item::Image(image) if image.label.as_deref() == Some("chart.png")));
        });
    }

    #[gpui::test]
    fn history_restores_pasted_images_after_their_user_prompt(cx: &mut gpui::TestAppContext) {
        let (workspace, vcx) = cx.add_window_view(|_, cx| {
            let mut workspace =
                crate::workspace::Workspace::for_test(crate::learning::Coach::new(), cx);
            workspace.push_test_panel("session-a", cx);
            workspace
        });
        let mut panel = None;
        workspace.update(vcx, |workspace, _| panel = workspace.test_panel(0));
        let panel = panel.expect("test panel exists");

        panel.update(vcx, |panel, cx| {
            panel.items.clear();
            panel.load_history(
                vec![jcode_sdk::HistoryMessage {
                    role: "user".into(),
                    content: "what is in this?".into(),
                }],
                vec![jcode_sdk::RenderedImage {
                    media_type: "image/png".into(),
                    data: "iVBORw0KGgo=".into(),
                    label: None,
                    source: jcode_sdk::RenderedImageSource::UserInput,
                    anchor: Some(jcode_sdk::RenderedImageAnchor::UserPrompt { ordinal: 0 }),
                }],
                cx,
            );
            assert!(matches!(
                panel.items.as_slice(),
                [Item::User(_), Item::Image(_)]
            ));
        });
    }

    #[gpui::test]
    fn keyboard_submission_reaches_the_bridge_and_streamed_text_paints(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| crate::input::bind_keys(cx));
        let (bridge, commands) = crate::harness::spawn_recording();
        let (workspace, vcx) = cx.add_window_view(|_, cx| {
            let mut workspace =
                crate::workspace::Workspace::for_test(crate::learning::Coach::new(), cx);
            workspace.set_test_bridge(bridge);
            workspace.push_test_panel("session-a", cx);
            workspace
        });
        let mut panel = None;
        workspace.update(vcx, |workspace, _| panel = workspace.test_panel(0));
        let panel = panel.expect("test panel exists");
        vcx.update(|window, cx| {
            let handle = panel.read(cx).input.read(cx).focus_handle.clone();
            window.focus(&handle, cx);
        });
        vcx.run_until_parked();

        vcx.simulate_input("hello jcode");
        vcx.simulate_keystrokes("enter");
        assert!(matches!(
            commands.recv_timeout(std::time::Duration::from_millis(100)),
            Ok(Command::Send { session_id, content, images })
                if session_id == "session-a" && content == "hello jcode" && images.is_empty()
        ));

        panel.update(vcx, |panel, cx| {
            panel.apply(
                &ApiEvent::TextDelta {
                    session_id: "session-a".into(),
                    text: "hello back".into(),
                },
                cx,
            );
        });
        vcx.run_until_parked();

        let bounds = vcx
            .debug_bounds("assistant-response")
            .expect("streamed assistant response should paint");
        assert!(bounds.size.width > px(0.) && bounds.size.height > px(0.));
    }

    #[gpui::test]
    fn escape_cancels_an_active_transcript_from_the_focused_prompt(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| crate::input::bind_keys(cx));
        let (bridge, commands) = crate::harness::spawn_recording();
        let (workspace, vcx) = cx.add_window_view(|_, cx| {
            let mut workspace =
                crate::workspace::Workspace::for_test(crate::learning::Coach::new(), cx);
            workspace.set_test_bridge(bridge);
            workspace.push_test_panel("session-a", cx);
            workspace
        });
        let mut panel = None;
        workspace.update(vcx, |workspace, _| panel = workspace.test_panel(0));
        let panel = panel.expect("test panel exists");
        vcx.update(|window, cx| {
            let handle = panel.read(cx).input.read(cx).focus_handle.clone();
            window.focus(&handle, cx);
        });
        panel.update(vcx, |panel, _| panel.status = "busy".into());

        vcx.simulate_keystrokes("escape");

        assert!(matches!(
            commands.recv_timeout(std::time::Duration::from_millis(100)),
            Ok(Command::Cancel { session_id }) if session_id == "session-a"
        ));
    }

    /// Nested inline markdown used to produce overlapping highlight ranges,
    /// and GPUI aborts the whole process (`invalid text run`) when those reach
    /// `StyledText`. Streaming crash-shaped text through the public event path
    /// and painting it through the real window is the acceptance check that a
    /// live session can no longer take the desktop down.
    #[gpui::test]
    fn streamed_nested_markdown_paints_without_aborting(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| crate::input::bind_keys(cx));
        let (bridge, _commands) = crate::harness::spawn_recording();
        let (workspace, vcx) = cx.add_window_view(|_, cx| {
            let mut workspace =
                crate::workspace::Workspace::for_test(crate::learning::Coach::new(), cx);
            workspace.set_test_bridge(bridge);
            workspace.push_test_panel("session-a", cx);
            workspace
        });
        let mut panel = None;
        workspace.update(vcx, |workspace, _| panel = workspace.test_panel(0));
        let panel = panel.expect("test panel exists");

        // Every shape that nests one inline span in another, including the
        // exact input from the recorded crash ("n the" inside a 12-byte run).
        let crash_shaped = "prefix **`n the`** suffix, *italic with [a link](https://example.com) \
                            and `code`*, ~~strike **bold `code`** tail~~, and \
                            [**bold link**](https://example.com/x).";
        panel.update(vcx, |panel, cx| {
            panel.apply(
                &ApiEvent::TextDelta {
                    session_id: "session-a".into(),
                    text: crash_shaped.into(),
                },
                cx,
            );
        });
        vcx.run_until_parked();

        let bounds = vcx
            .debug_bounds("assistant-response")
            .expect("nested markdown response should paint");
        assert!(bounds.size.width > px(0.) && bounds.size.height > px(0.));
    }

    #[gpui::test]
    fn slash_commands_dispatch_native_session_operations(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| crate::input::bind_keys(cx));
        let (bridge, commands) = crate::harness::spawn_recording();
        let (workspace, vcx) = cx.add_window_view(|_, cx| {
            let mut workspace =
                crate::workspace::Workspace::for_test(crate::learning::Coach::new(), cx);
            workspace.set_test_bridge(bridge);
            workspace.push_test_panel("session-a", cx);
            workspace
        });
        let mut panel = None;
        workspace.update(vcx, |workspace, _| panel = workspace.test_panel(0));
        let panel = panel.expect("test panel exists");
        vcx.update(|window, cx| {
            let handle = panel.read(cx).input.read(cx).focus_handle.clone();
            window.focus(&handle, cx);
        });

        vcx.simulate_input("/effort x");
        vcx.simulate_keystrokes("enter");
        assert!(matches!(
            commands.recv_timeout(std::time::Duration::from_millis(100)),
            Ok(Command::SessionOperation {
                session_id,
                operation: SessionOperation::SetEffort(effort),
            }) if session_id == "session-a" && effort == "xhigh"
        ));

        vcx.simulate_input("/clear");
        vcx.simulate_keystrokes("enter");
        assert!(matches!(
            commands.recv_timeout(std::time::Duration::from_millis(100)),
            Ok(Command::SessionOperation {
                session_id,
                operation: SessionOperation::Clear,
            }) if session_id == "session-a"
        ));
    }

    #[gpui::test]
    fn model_command_opens_center_picker_and_enter_switches_selection(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| crate::input::bind_keys(cx));
        let (bridge, commands) = crate::harness::spawn_recording();
        let (workspace, vcx) = cx.add_window_view(|_, cx| {
            let mut workspace =
                crate::workspace::Workspace::for_test(crate::learning::Coach::new(), cx);
            workspace.set_test_bridge(bridge);
            workspace.push_test_panel("session-a", cx);
            workspace
        });
        let mut panel = None;
        workspace.update(vcx, |workspace, cx| {
            panel = workspace.test_panel(0);
            panel.as_ref().unwrap().update(cx, |panel, cx| {
                panel.apply(
                    &ApiEvent::RuntimeInfo {
                        session_id: "session-a".into(),
                        provider: Some("openai".into()),
                        model: Some("gpt-5.6-sol".into()),
                        reasoning_effort: None,
                        routes: vec![
                            route("claude-fable-5", "anthropic-api-key"),
                            route("gpt-5.6-sol", "openai-oauth"),
                        ],
                    },
                    cx,
                );
            });
        });
        let panel = panel.expect("test panel exists");
        vcx.update(|window, cx| {
            let handle = panel.read(cx).input.read(cx).focus_handle.clone();
            window.focus(&handle, cx);
        });

        vcx.simulate_input("/model");
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("model-picker-overlay").is_some());
        let overlay = vcx
            .debug_bounds("model-picker-overlay")
            .expect("picker overlay is rendered in the active panel");
        let dialog = vcx
            .debug_bounds("model-picker-dialog")
            .expect("picker dialog is rendered");
        assert!((dialog.center().x - overlay.center().x).abs() < px(1.0));
        assert!((dialog.center().y - overlay.center().y).abs() < px(1.0));
        assert!(vcx.debug_bounds("model-picker-logo-0").is_some());
        assert!(vcx.debug_bounds("model-picker-logo-1").is_some());
        vcx.update(|_, cx| {
            assert_eq!(panel.read(cx).input.read(cx).content.as_ref(), "/model ");
            assert_eq!(panel.read(cx).input.read(cx).model_picker_rows().len(), 2);
        });

        vcx.simulate_keystrokes("down");
        vcx.run_until_parked();
        vcx.update(|_, cx| {
            assert_eq!(
                panel.read(cx).input.read(cx).model_picker_rows(),
                vec![
                    ("claude-fable-5".into(), false),
                    ("gpt-5.6-sol".into(), true)
                ]
            );
        });
        vcx.simulate_keystrokes("enter");
        let received = commands.recv_timeout(std::time::Duration::from_millis(100));
        match received {
            Ok(Command::SetModel { session_id, model }) => {
                assert_eq!(session_id, "session-a");
                assert_eq!(model, "gpt-5.6-sol");
            }
            _ => panic!("picker did not emit SetModel"),
        }
        vcx.run_until_parked();
        assert!(vcx.debug_bounds("model-picker-overlay").is_none());
    }

    #[test]
    fn model_logos_follow_routes_then_fall_back_to_model_families() {
        assert_eq!(
            model_logo_provider("custom-model", "openai-oauth"),
            "openai"
        );
        assert_eq!(model_logo_provider("claude-fable-5", ""), "anthropic-api");
        assert_eq!(model_logo_provider("gemini-3-pro", ""), "gemini");
        assert_eq!(model_logo_provider("private-model", "private"), "private");
    }

    #[test]
    fn todo_tool_output_parses_items_and_plan() {
        let payload = parse_todo_tool_output(
            r#"[{"id":"build","content":"Build the card","status":"in_progress","priority":"high","group":"Desktop","confidence":"validated"}]
Plan: {"user_intention":"See progress at a glance","understands_user_intent":"clear"}
Goals: []"#,
        )
        .expect("todo payload parses");

        assert_eq!(payload.todos.len(), 1);
        assert_eq!(payload.todos[0].content, "Build the card");
        assert_eq!(payload.todos[0].group.as_deref(), Some("Desktop"));
        assert_eq!(
            payload.plan.user_intention.as_deref(),
            Some("See progress at a glance")
        );
    }

    #[gpui::test]
    fn completed_todo_tool_paints_as_a_native_card(cx: &mut gpui::TestAppContext) {
        let (workspace, vcx) = cx.add_window_view(|_, cx| {
            let mut workspace =
                crate::workspace::Workspace::for_test(crate::learning::Coach::new(), cx);
            workspace.push_test_panel("session-a", cx);
            workspace
        });
        let panel = workspace
            .read_with(vcx, |workspace, _| workspace.test_panel(0))
            .expect("panel exists");
        panel.update(vcx, |panel, cx| {
            panel.items = vec![
                Item::User("Keep the task list compact".into()),
                Item::Tool {
                    call_id: "todo-1".into(),
                    name: "todo".into(),
                    input: r#"{"intent":"Track implementation"}"#.into(),
                    output: r#"[{"id":"one","content":"Render rich rows","status":"completed","priority":"high","completion_confidence":"verified","blocked_by":[]},{"id":"two","content":"Test the card","status":"in_progress","priority":"medium","group":"Validation","confidence":"validated","blocked_by":[]}] Plan: {"user_intention":"See progress visually","understands_user_intent":"clear"} Goals: []"#.into(),
                    done: true,
                    error: None,
                },
            ];
            cx.notify();
        });
        vcx.run_until_parked();

        let bounds = vcx
            .debug_bounds("todo-card")
            .expect("todo tool output should paint as a native card");
        let pinned = vcx
            .debug_bounds("pinned-todo-card")
            .expect("todo card should be pinned outside the transcript");
        let prompt = vcx
            .debug_bounds("pinned-latest-prompt")
            .expect("latest prompt should stay visible above the todo card");
        let transcript = vcx.debug_bounds("transcript").expect("transcript paints");
        assert!(bounds.size.width > px(0.) && bounds.size.height > px(0.));
        assert!(prompt.bottom() <= bounds.top());
        assert!(pinned.bottom() <= transcript.top());
        assert!(vcx.debug_bounds("tool-card").is_none());
    }
}

/// `JCODE_DESKTOP_DEMO_TRANSCRIPT=1` seeds one panel with a sample of every
/// transcript shape, so rendering changes can be reviewed without driving a
/// real session through each case.
fn demo_items() -> Vec<Item> {
    if std::env::var("JCODE_DESKTOP_DEMO_TRANSCRIPT").as_deref() != Ok("1") {
        return Vec::new();
    }
    demo_item_fixtures()
}

/// The sample content behind `demo_items`, reachable from tests without
/// mutating process environment.
fn demo_item_fixtures() -> Vec<Item> {
    vec![
        Item::User("Explain **markdown** rendering and show `code`, a [link](https://example.com), ~~old~~ new.".into()),
        Item::Reasoning("The user wants a survey of the renderer. I should cover blocks, inline spans, and how streaming text is handled while a turn is still in flight, then mention the tables and code paths in order.".into()),
        Item::Tool {
            call_id: "1".into(),
            name: "bash".into(),
            input: r#"{"command":"cargo test --offline","intent":"run the suite"}"#.into(),
            output: "test result: ok. 61 passed".into(),
            done: true,
            error: None,
        },
        Item::Tool {
            call_id: "2".into(),
            name: "read".into(),
            input: r#"{"file_path":"src/markdown.rs"}"#.into(),
            output: String::new(),
            done: false,
            error: None,
        },
        // Long ANSI-colored output: exercises the size hint on the collapsed
        // row and the stripped, head-and-tail detail when expanded.
        Item::Tool {
            call_id: "3".into(),
            name: "bash".into(),
            input: r#"{"command":"cargo build 2>&1","intent":"noisy build"}"#.into(),
            output: (0..90)
                .map(|n| format!("\u{1b}[32m   Compiling\u{1b}[0m crate-{n} v0.1.{n}\n"))
                .collect(),
            done: true,
            error: None,
        },
        Item::Assistant(
            "# Heading one\n## Heading two\n\nA paragraph with *italic*, **bold**, `inline code`, and math $e^{i\\pi}+1=0$ plus \\(n \\to \\infty\\).\n\n- top level\n  - nested item\n- [x] finished task\n- [ ] pending task\n\n1. first\n2. second\n\n> A quote line\n> continued here\n\n| block | supported |\n| --- | --- |\n| tables | yes |\n| code | yes |\n\n```rust\nfn main() {\n    // a comment\n    let name = \"world\";\n    println!(\"hello {name}\");\n}\n```\n\n$$\n\\sum_{i=0}^{n} i^2\n$$\n\n\\[ E = mc^2 \\]\n\n---\n\nDone."
                .into(),
        ),
        Item::Error("provider returned 429: rate limited, retrying".into()),
    ]
}
