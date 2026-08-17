//! A small embedded PTY terminal used by plain terminal panels.

use std::ops::Range;
use std::time::Duration;

use gpui::{
    Bounds, Context, ElementInputHandler, EntityInputHandler, FocusHandle, Focusable, KeyBinding,
    KeyDownEvent, Pixels, Render, UTF16Selection, Window, actions, canvas, div, prelude::*, px,
};
use jcode_desktop_api::HostHandle;

use crate::theme::Theme;

const ROWS: u16 = 40;
const COLS: u16 = 120;

actions!(
    terminal,
    [
        Enter, Backspace, Tab, Escape, Up, Down, Left, Right, Home, End, Delete
    ]
);

pub fn bind_keys(cx: &mut gpui::App) {
    cx.bind_keys([
        KeyBinding::new("enter", Enter, Some("Terminal")),
        KeyBinding::new("backspace", Backspace, Some("Terminal")),
        KeyBinding::new("tab", Tab, Some("Terminal")),
        KeyBinding::new("escape", Escape, Some("Terminal")),
        KeyBinding::new("up", Up, Some("Terminal")),
        KeyBinding::new("down", Down, Some("Terminal")),
        KeyBinding::new("left", Left, Some("Terminal")),
        KeyBinding::new("right", Right, Some("Terminal")),
        KeyBinding::new("home", Home, Some("Terminal")),
        KeyBinding::new("end", End, Some("Terminal")),
        KeyBinding::new("delete", Delete, Some("Terminal")),
    ]);
}

pub struct TerminalPanel {
    focus: FocusHandle,
    parser: vt100::Parser,
    host: HostHandle,
    resource_id: Option<u64>,
    status: String,
    _poll: gpui::Task<()>,
}

impl TerminalPanel {
    pub fn new(
        working_dir: Option<String>,
        requested_id: Option<u64>,
        host: HostHandle,
        cx: &mut Context<Self>,
    ) -> Self {
        let resource_id = host.terminal_create(requested_id, working_dir.as_deref());
        let status = if resource_id.is_some() {
            String::new()
        } else if requested_id.is_some() {
            "terminal session is no longer available".into()
        } else {
            "terminal unavailable".into()
        };
        let poll_id = resource_id;
        let poll = cx.spawn(async move |this, cx| {
            let Some(resource_id) = poll_id else { return };
            let mut cursor = 0;
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(16))
                    .await;
                let mut chunks = Vec::new();
                loop {
                    let mut buffer = vec![0; 32 * 1024];
                    let read = host.terminal_read(resource_id, cursor, &mut buffer);
                    cursor = read.next_cursor;
                    buffer.truncate(read.copied);
                    if !buffer.is_empty() {
                        chunks.push(buffer);
                    }
                    if read.copied < 32 * 1024 {
                        break;
                    }
                }
                if chunks.is_empty() {
                    continue;
                }
                if this
                    .update(cx, |this, cx| {
                        for chunk in chunks {
                            this.parser.process(&chunk);
                        }
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        });

        Self {
            focus: cx.focus_handle(),
            parser: vt100::Parser::new(ROWS, COLS, 10_000),
            host,
            resource_id,
            status,
            _poll: poll,
        }
    }

    pub fn resource_id(&self) -> Option<u64> {
        self.resource_id
    }

    fn send(&self, bytes: &[u8]) {
        if let Some(resource_id) = self.resource_id {
            let _ = self.host.terminal_write(resource_id, bytes);
        }
    }

    fn send_text(&self, text: &str) {
        if text.contains('\n') || text.contains('\r') {
            let normalized = text.replace("\r\n", "\r").replace('\n', "\r");
            self.send(normalized.as_bytes());
        } else {
            self.send(text.as_bytes());
        }
    }

    fn key_down(&mut self, event: &KeyDownEvent, _: &mut Window, _: &mut Context<Self>) {
        let modifiers = event.keystroke.modifiers;
        // Super combinations belong to the workspace. Do not turn Super+O/N/Q
        // into shell input or stop their action from bubbling.
        if modifiers.platform || modifiers.function {
            return;
        }
        let key = event.keystroke.key.as_str();
        if modifiers.control && !modifiers.alt {
            if let Some(ch) = key.chars().next().filter(|ch| ch.is_ascii_alphabetic()) {
                self.send(&[(ch.to_ascii_uppercase() as u8) - b'@']);
            }
        } else if !modifiers.alt && event.keystroke.key_char.is_none() && key.chars().count() == 1 {
            // Synthetic/raw keyboard sources may not generate an IME commit.
            // Normal text still comes through EntityInputHandler, avoiding
            // duplicate characters on Wayland.
            self.send(key.as_bytes());
        }
    }

    fn enter(&mut self, _: &Enter, _: &mut Window, _: &mut Context<Self>) {
        self.send(b"\r");
    }
    fn backspace(&mut self, _: &Backspace, _: &mut Window, _: &mut Context<Self>) {
        self.send(&[0x7f]);
    }
    fn tab(&mut self, _: &Tab, _: &mut Window, _: &mut Context<Self>) {
        self.send(b"\t");
    }
    fn escape(&mut self, _: &Escape, _: &mut Window, _: &mut Context<Self>) {
        self.send(&[0x1b]);
    }
    fn up(&mut self, _: &Up, _: &mut Window, _: &mut Context<Self>) {
        self.send(b"\x1b[A");
    }
    fn down(&mut self, _: &Down, _: &mut Window, _: &mut Context<Self>) {
        self.send(b"\x1b[B");
    }
    fn left(&mut self, _: &Left, _: &mut Window, _: &mut Context<Self>) {
        self.send(b"\x1b[D");
    }
    fn right(&mut self, _: &Right, _: &mut Window, _: &mut Context<Self>) {
        self.send(b"\x1b[C");
    }
    fn home(&mut self, _: &Home, _: &mut Window, _: &mut Context<Self>) {
        self.send(b"\x1b[H");
    }
    fn end(&mut self, _: &End, _: &mut Window, _: &mut Context<Self>) {
        self.send(b"\x1b[F");
    }
    fn delete(&mut self, _: &Delete, _: &mut Window, _: &mut Context<Self>) {
        self.send(b"\x1b[3~");
    }

    pub fn focus(&self, window: &mut Window, cx: &mut gpui::App) {
        window.focus(&self.focus, cx);
    }

    #[cfg(test)]
    pub fn screen_contents(&self) -> String {
        self.parser.screen().contents()
    }
}

// Printable text on Wayland/macOS is delivered through GPUI's text-input/IME
// path, not reliably through KeyDownEvent. Registering this handler is what
// makes normal typing reach the PTY. Workspace shortcuts remain actions and
// continue bubbling through the terminal to Workspace.
impl EntityInputHandler for TerminalPanel {
    fn text_for_range(
        &mut self,
        _range: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        actual_range.replace(0..0);
        Some(String::new())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: 0..0,
            reversed: false,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        None
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {}

    fn replace_text_in_range(
        &mut self,
        _range: Option<Range<usize>>,
        new_text: &str,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        self.send_text(new_text);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _range: Option<Range<usize>>,
        new_text: &str,
        _new_selected_range: Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        self.send_text(new_text);
    }

    fn bounds_for_range(
        &mut self,
        _range: Range<usize>,
        bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        Some(Bounds::new(bounds.origin, gpui::size(px(1.0), px(18.0))))
    }

    fn character_index_for_point(
        &mut self,
        _point: gpui::Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        Some(0)
    }
}

impl Drop for TerminalPanel {
    fn drop(&mut self) {
        if let Some(resource_id) = self.resource_id {
            self.host.terminal_release(resource_id);
        }
    }
}

impl Render for TerminalPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let content = if self.status.is_empty() {
            self.parser.screen().contents()
        } else {
            self.status.clone()
        };
        let input = cx.entity();
        div()
            .id("plain-terminal")
            .debug_selector(|| "plain-terminal".into())
            .key_context("Terminal")
            .track_focus(&self.focus)
            .relative()
            .size_full()
            .overflow_hidden()
            .p_3()
            .bg(Theme::BG)
            .font_family(Theme::FONT_MONO)
            .text_size(px(13.0))
            .text_color(Theme::TEXT)
            .whitespace_nowrap()
            // One element per terminal row preserves line boundaries. A single
            // text child is laid out like prose and can collapse terminal output.
            .children(
                content
                    .split('\n')
                    .map(|line| div().h(px(18.0)).child(line.to_owned())),
            )
            .child(
                canvas(
                    |_, _, _| (),
                    move |bounds, _, window, cx| {
                        let focus = input.read(cx).focus.clone();
                        window.handle_input(
                            &focus,
                            ElementInputHandler::new(bounds, input.clone()),
                            cx,
                        );
                    },
                )
                .absolute()
                .size_full(),
            )
            .on_action(cx.listener(Self::enter))
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::tab))
            .on_action(cx.listener(Self::escape))
            .on_action(cx.listener(Self::up))
            .on_action(cx.listener(Self::down))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::delete))
            .on_key_down(cx.listener(Self::key_down))
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|this, _, window, cx| this.focus(window, cx)),
            )
    }
}

impl Focusable for TerminalPanel {
    fn focus_handle(&self, _: &gpui::App) -> FocusHandle {
        self.focus.clone()
    }
}
