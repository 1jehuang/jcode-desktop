//! Embedded terminal: the host owns the PTY, Handterm owns emulation, and GPUI owns presentation.

use crate::{image_cache::ImageIds, theme::Theme};
use gpui::{
    Bounds, ClipboardItem, Context, ElementInputHandler, EntityInputHandler, FocusHandle,
    Focusable, KeyBinding, KeyDownEvent, KeyUpEvent, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, Pixels, Point, Render, RenderImage, ScrollWheelEvent, UTF16Selection, Window,
    actions, canvas, div, prelude::*, px,
};
use handterm_common::{
    grid::Selection,
    terminal::{MouseMode, Terminal},
};
use jcode_desktop_api::HostHandle;
use std::{collections::HashMap, ops::Range, sync::Arc, time::Duration};

#[path = "terminal_keys.rs"]
mod keys;
#[path = "terminal_paint.rs"]
mod paint;
#[cfg(test)]
#[path = "terminal_tests.rs"]
mod tests;

const ROWS: u16 = 40;
const COLS: u16 = 120;
const FONT_SIZE: f32 = 13.0;
const LINE_HEIGHT: f32 = 18.0;
const PADDING: f32 = 10.0;
// Keep one busy PTY from starving the UI thread. A later tick drains the rest.
const POLL_BUDGET: usize = 256 * 1024;

fn color_channels(color: gpui::Rgba) -> [u8; 3] {
    [color.r, color.g, color.b].map(|channel| (channel * 255.).round() as u8)
}

fn sync_terminal_colors(terminal: &mut Terminal, theme: &Theme, focused: bool) {
    // OSC 10/11 must describe the colors we actually paint, not Handterm's
    // standalone white-on-black defaults. Apps such as Jcode query these to
    // choose a readable palette.
    terminal.set_default_colors(
        color_channels(theme.TEXT),
        color_channels(theme.panel_background(focused)),
    );
}

actions!(terminal, [Copy, Paste]);

pub fn bind_keys(cx: &mut gpui::App) {
    cx.bind_keys([
        KeyBinding::new("ctrl-shift-c", Copy, Some("Terminal")),
        KeyBinding::new("ctrl-shift-v", Paste, Some("Terminal")),
        KeyBinding::new("shift-insert", Paste, Some("Terminal")),
    ]);
    #[cfg(target_os = "macos")]
    cx.bind_keys([
        KeyBinding::new("cmd-c", Copy, Some("Terminal")),
        KeyBinding::new("cmd-v", Paste, Some("Terminal")),
    ]);
}

struct CachedImage {
    fingerprint: u64,
    image: Arc<RenderImage>,
}

pub struct TerminalPanel {
    focus: FocusHandle,
    surface_focused: bool,
    terminal: Terminal,
    host: HostHandle,
    resource_id: Option<u64>,
    engine_replies: bool,
    output_cursor: u64,
    ime_key_text: Option<(String, std::time::Instant)>,
    status: String,
    exited: bool,
    bounds: Bounds<Pixels>,
    cell_width: Pixels,
    selecting: bool,
    wheel_remainder: f32,
    image_ids: ImageIds,
    images: HashMap<u32, CachedImage>,
    image_generation: Option<u64>,
    _poll: gpui::Task<()>,
    _focus_subscriptions: Vec<gpui::Subscription>,
}

impl TerminalPanel {
    pub fn new(
        working_dir: Option<String>,
        requested_id: Option<u64>,
        replay_until: Option<u64>,
        host: HostHandle,
        cx: &mut Context<Self>,
    ) -> Self {
        // Probe before spawning: protocol replies belong to exactly one layer.
        let engine_replies = host.terminal_engine_replies();
        let resource_id = host.terminal_create(requested_id, working_dir.as_deref());
        let status = match (resource_id, requested_id) {
            (Some(_), _) => String::new(),
            (None, Some(_)) => "terminal session is no longer available".into(),
            _ => "terminal unavailable".into(),
        };
        let poll = cx.spawn(async move |this, cx| {
            let Some(id) = resource_id else { return };
            let mut cursor = 0;
            let mut replaying = requested_id.is_some();
            let mut buffer = vec![0; 32 * 1024];
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(16))
                    .await;
                let mut output = Vec::new();
                let mut closed = false;
                let mut gap = false;
                let mut output_start = cursor;
                let mut drained = false;
                while output.len() < POLL_BUDGET {
                    let read = host.terminal_read(id, cursor, &mut buffer);
                    if output.is_empty() {
                        output_start = cursor.max(read.available_from);
                    }
                    gap |= cursor < read.available_from;
                    cursor = read.next_cursor;
                    closed |= read.closed != 0;
                    output.extend_from_slice(&buffer[..read.copied]);
                    if read.copied < buffer.len() {
                        drained = true;
                        break;
                    }
                }
                if output.is_empty() && !closed {
                    replaying = false;
                    continue;
                }
                if this
                    .update(cx, |this, cx| {
                        if gap {
                            this.terminal = Terminal::new_with_scrollback(
                                this.terminal.cols,
                                this.terminal.rows,
                                crate::config::get().terminal.scrollback_lines,
                            );
                            this.image_generation = None;
                        }
                        if let Some(replay_until) = replay_until {
                            let historical = replay_until
                                .saturating_sub(output_start)
                                .min(output.len() as u64)
                                as usize;
                            this.process_output(&output[..historical], false);
                            this.process_output(&output[historical..], true);
                        } else {
                            // Older snapshots have no consumed cursor. Conservatively
                            // suppress the initial replay, then reply to new output.
                            this.process_output(&output, !replaying);
                        }
                        this.output_cursor = cursor;
                        this.exited |= closed && drained;
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
                if drained {
                    replaying = false;
                }
                if closed && drained {
                    break;
                }
            }
        });
        cx.on_release(|this, cx| {
            let images = std::mem::take(&mut this.images);
            cx.defer(move |cx| {
                for cached in images.into_values() {
                    cx.drop_image(cached.image, None);
                }
            });
        })
        .detach();
        Self {
            focus: cx.focus_handle(),
            surface_focused: false,
            terminal: Terminal::new_with_scrollback(
                COLS,
                ROWS,
                crate::config::get().terminal.scrollback_lines,
            ),
            host,
            resource_id,
            engine_replies,
            output_cursor: 0,
            ime_key_text: None,
            status,
            exited: false,
            bounds: Bounds::default(),
            cell_width: px(8.0),
            selecting: false,
            wheel_remainder: 0.0,
            image_ids: ImageIds::get(cx),
            images: HashMap::new(),
            image_generation: None,
            _poll: poll,
            _focus_subscriptions: Vec::new(),
        }
    }

    fn process_output(&mut self, output: &[u8], reply: bool) {
        // Sync before parsing queries, including the first output after creation,
        // replay/reset, and a Desktop theme change.
        sync_terminal_colors(&mut self.terminal, Theme::global(), self.surface_focused);
        self.terminal.process(output);
        if let Some(responses) = self.terminal.drain_responses() {
            if reply {
                if self.engine_replies {
                    self.send(&responses);
                } else {
                    // ABI-compatible live upgrade from the old host, which still
                    // answers a small set of Fish probes. Keep DSR and image ACKs.
                    self.send(&keys::without_legacy_host_replies(&responses));
                }
            }
        }
        // These are notifications, not another history buffer. Images are already
        // decoded by the engine. Do not retain raw APC uploads or honor remote
        // clipboard writes without an explicit user gesture.
        self.terminal.drain_control_strings();
        self.terminal.drain_osc();
        self.terminal.drain_dcs();
        self.terminal.drain_sixel();
        self.terminal.drain_apc();
        self.terminal.take_osc52_clipboard();
        self.terminal.take_title();
        self.terminal.take_bell();
    }

    pub fn resource_id(&self) -> Option<u64> {
        self.resource_id
    }

    pub(crate) fn output_cursor(&self) -> u64 {
        self.output_cursor
    }

    fn send(&self, bytes: &[u8]) {
        if !bytes.is_empty() && !self.exited {
            if let Some(id) = self.resource_id {
                let _ = self.host.terminal_write(id, bytes);
            }
        }
    }

    fn send_text(&mut self, text: &str) {
        if let Some((sent, at)) = self.ime_key_text.take() {
            if sent == text && at.elapsed() < Duration::from_millis(100) {
                return;
            }
        }
        self.terminal.grid.scroll_offset = 0;
        self.terminal.grid.selection = None;
        self.send(text.replace("\r\n", "\r").replace('\n', "\r").as_bytes());
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        let text = self.terminal.grid.get_selection_text();
        if !text.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    fn paste(&mut self, _: &Paste, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.terminal.grid.scroll_offset = 0;
            self.terminal.grid.selection = None;
            self.send(&keys::paste_bytes(
                &text,
                self.terminal.bracketed_paste_mode(),
            ));
            cx.notify();
        }
    }

    fn resize(&mut self, bounds: Bounds<Pixels>, cell_width: Pixels, cx: &mut Context<Self>) {
        self.bounds = bounds;
        self.cell_width = cell_width.max(px(1.));
        let cols = ((bounds.size.width / self.cell_width).floor() as u16).clamp(1, 1000);
        let rows = ((bounds.size.height / px(LINE_HEIGHT)).floor() as u16).clamp(1, 1000);
        if (cols, rows) != (self.terminal.cols, self.terminal.rows) {
            self.terminal.resize(cols, rows);
            self.terminal.grid.selection = None;
            if let Some(id) = self.resource_id {
                let _ = self.host.terminal_resize(id, rows, cols);
            }
            cx.notify();
        }
    }

    fn key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.ime_key_text = None;
        if let Some(bytes) = keys::key_bytes(&event.keystroke, &self.terminal, false, event.is_held)
        {
            self.terminal.grid.scroll_offset = 0;
            self.terminal.grid.selection = None;
            self.ime_key_text = event
                .keystroke
                .key_char
                .clone()
                .map(|text| (text, std::time::Instant::now()));
            self.send(&bytes);
            cx.stop_propagation();
            cx.notify();
        }
    }

    fn key_up(&mut self, event: &KeyUpEvent, _: &mut Window, _: &mut Context<Self>) {
        if let Some(bytes) = keys::key_bytes(&event.keystroke, &self.terminal, true, false) {
            self.send(&bytes);
        }
    }

    fn cell_at_point(&self, position: Point<Pixels>) -> (usize, usize) {
        let col = ((position.x - self.bounds.origin.x) / self.cell_width)
            .floor()
            .max(0.) as usize;
        let row = ((position.y - self.bounds.origin.y) / px(LINE_HEIGHT))
            .floor()
            .max(0.) as usize;
        (
            col.min(self.terminal.cols as usize - 1),
            row.min(self.terminal.rows as usize - 1),
        )
    }

    fn mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.focus(window, cx);
        let (col, row) = self.cell_at_point(event.position);
        if !event.modifiers.shift && self.terminal.mouse_mode != MouseMode::Off {
            if let Some(bytes) = self.terminal.encode_mouse(
                keys::mouse_button(event.button, event.modifiers),
                col,
                row,
                true,
            ) {
                self.send(&bytes);
            }
        } else if event.button == MouseButton::Left {
            self.selecting = true;
            self.terminal.grid.selection = Some(Selection {
                start_col: col,
                start_row: row,
                end_col: col,
                end_row: row,
            });
            cx.notify();
        }
        // The workspace must also select this pane, not just move keyboard
        // focus into its terminal. Keep local selection/reporting above intact.
        if event.button != MouseButton::Left {
            cx.stop_propagation();
        }
    }

    fn mouse_up(&mut self, event: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selecting && !event.modifiers.shift && self.terminal.mouse_mode != MouseMode::Off {
            let (col, row) = self.cell_at_point(event.position);
            if let Some(bytes) = self.terminal.encode_mouse(
                keys::mouse_button(event.button, event.modifiers),
                col,
                row,
                false,
            ) {
                self.send(&bytes);
            }
        }
        self.selecting = false;
        cx.stop_propagation();
    }

    fn mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        let (col, row) = self.cell_at_point(event.position);
        if self.selecting {
            if !event.dragging() {
                self.selecting = false;
                return;
            }
            if let Some(sel) = &mut self.terminal.grid.selection {
                sel.end_col = col;
                sel.end_row = row;
            }
            cx.notify();
        } else if !event.modifiers.shift
            && (self.terminal.mouse_mode == MouseMode::AnyEvent
                || (self.terminal.mouse_mode == MouseMode::ButtonEvent
                    && event.pressed_button.is_some()))
        {
            let button = event
                .pressed_button
                .map(|b| keys::mouse_button(b, event.modifiers))
                .unwrap_or(3 | keys::mouse_modifiers(event.modifiers))
                | 32;
            if let Some(bytes) = self.terminal.encode_mouse(button, col, row, true) {
                self.send(&bytes);
            }
        }
    }

    fn scroll(&mut self, event: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
        let delta = event.delta.pixel_delta(px(LINE_HEIGHT)).y / px(LINE_HEIGHT);
        self.wheel_remainder += delta;
        let lines = self.wheel_remainder.trunc() as i32;
        self.wheel_remainder -= lines as f32;
        let (col, row) = self.cell_at_point(event.position);
        if !event.modifiers.shift && self.terminal.mouse_mode != MouseMode::Off {
            for _ in 0..lines.unsigned_abs().min(100) {
                if let Some(bytes) = self.terminal.encode_mouse(
                    (if lines > 0 { 64 } else { 65 }) | keys::mouse_modifiers(event.modifiers),
                    col,
                    row,
                    true,
                ) {
                    self.send(&bytes);
                }
            }
        } else if self.terminal.in_alt_screen() {
            if self.terminal.alternate_scroll_mode() {
                for _ in 0..lines.unsigned_abs().min(100) {
                    let prefix = if self.terminal.application_cursor_keys {
                        "\x1bO"
                    } else {
                        "\x1b["
                    };
                    self.send(format!("{prefix}{}", if lines > 0 { 'A' } else { 'B' }).as_bytes());
                }
            }
        } else {
            let grid = &mut self.terminal.grid;
            grid.scroll_offset = (grid.scroll_offset as i64 + i64::from(lines))
                .clamp(0, grid.scrollback_len() as i64) as usize;
            grid.selection = None;
            cx.notify();
        }
        cx.stop_propagation();
    }

    pub fn focus(&self, window: &mut Window, cx: &mut gpui::App) {
        window.focus(&self.focus, cx);
    }

    pub(crate) fn set_surface_focused(&mut self, focused: bool, cx: &mut Context<Self>) {
        if self.surface_focused != focused {
            self.surface_focused = focused;
            sync_terminal_colors(&mut self.terminal, Theme::global(), focused);
            cx.notify();
        }
    }

    pub(crate) fn debug_snapshot(&self) -> serde_json::Value {
        serde_json::json!({
            "resource_id": self.resource_id, "rows": self.terminal.rows, "cols": self.terminal.cols,
            "contents": self.terminal.grid.get_text(0, self.terminal.rows as usize),
            "cursor": self.terminal.grid.cursor_pos(), "image_count": self.terminal.kitty_images().len(),
            "placements": self.terminal.kitty_placements().len(),
            "scrollback_offset": self.terminal.grid.scroll_offset,
            "scrollback_lines": self.terminal.grid.scrollback_len(),
            "selection": self.terminal.grid.get_selection_text(), "exited": self.exited,
        })
    }

    #[cfg(test)]
    pub fn screen_contents(&self) -> String {
        self.terminal.grid.get_text(0, self.terminal.rows as usize)
    }
}

impl Render for TerminalPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self._focus_subscriptions.is_empty() {
            self._focus_subscriptions
                .push(cx.on_focus(&self.focus, window, |this, _, cx| {
                    if this.terminal.focus_events_mode() {
                        this.send(b"\x1b[I");
                    }
                    cx.notify();
                }));
            self._focus_subscriptions
                .push(cx.on_blur(&self.focus, window, |this, _, cx| {
                    if this.terminal.focus_events_mode() {
                        this.send(b"\x1b[O");
                    }
                    cx.notify();
                }));
        }
        let prepare = cx.entity();
        let input = cx.entity();
        div()
            .id("plain-terminal")
            .debug_selector(|| "plain-terminal".into())
            .key_context("Terminal")
            .track_focus(&self.focus)
            .relative()
            .size_full()
            .overflow_hidden()
            .p(px(PADDING))
            .bg(Theme::global().panel_background(self.surface_focused))
            .child(
                canvas(
                    move |bounds, window, cx| {
                        prepare.update(cx, |terminal, cx| terminal.prepare(bounds, window, cx))
                    },
                    move |bounds, scene, window, cx| {
                        let focus = input.read(cx).focus.clone();
                        window.handle_input(
                            &focus,
                            ElementInputHandler::new(bounds, input.clone()),
                            cx,
                        );
                        scene.paint(bounds, window, cx);
                    },
                )
                .size_full(),
            )
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::paste))
            .on_key_down(cx.listener(Self::key_down))
            .on_key_up(cx.listener(Self::key_up))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::mouse_down))
            .on_mouse_down(MouseButton::Right, cx.listener(Self::mouse_down))
            .on_mouse_down(MouseButton::Middle, cx.listener(Self::mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::mouse_up))
            .on_mouse_up(MouseButton::Right, cx.listener(Self::mouse_up))
            .on_mouse_up(MouseButton::Middle, cx.listener(Self::mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::mouse_up))
            .on_mouse_up_out(MouseButton::Right, cx.listener(Self::mouse_up))
            .on_mouse_up_out(MouseButton::Middle, cx.listener(Self::mouse_up))
            .on_mouse_move(cx.listener(Self::mouse_move))
            .on_scroll_wheel(cx.listener(Self::scroll))
    }
}

impl Focusable for TerminalPanel {
    fn focus_handle(&self, _: &gpui::App) -> FocusHandle {
        self.focus.clone()
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
        cx: &mut Context<Self>,
    ) {
        self.send_text(new_text);
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _range: Option<Range<usize>>,
        _new_text: &str,
        _new_selected_range: Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        // Preedit text is mutable IME composition. It must not reach the PTY
        // until GPUI commits it through `replace_text_in_range`.
    }

    fn bounds_for_range(
        &mut self,
        _range: Range<usize>,
        bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let (col, row) = self.terminal.grid.cursor_pos();
        Some(Bounds::new(
            bounds.origin + gpui::point(self.cell_width * col, px(LINE_HEIGHT) * row),
            gpui::size(self.cell_width, px(LINE_HEIGHT)),
        ))
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
