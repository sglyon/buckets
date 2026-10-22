use crate::{
    TerminalAppearance, TerminalSession,
    input::{encode_key, is_text_input},
};
use alacritty_terminal::{
    event::Event,
    grid::{Dimensions, Scroll},
    index::{Column, Line, Point as TermPoint, Side},
    selection::{Selection, SelectionType},
    term::{TermMode, cell::Flags},
    vte::ansi::{Color, CursorShape, NamedColor},
};
use gpui::{prelude::*, *};
use std::{ops::Range, path::Path, sync::atomic::Ordering, time::Duration};

const BASE_FONT_SIZE: f32 = 13.;
const BASE_CELL_HEIGHT: f32 = 20.;
const PAD: f32 = 8.;

actions!(buckets_terminal, [TerminalTab, TerminalBacktab]);
struct TerminalBindings;
impl Global for TerminalBindings {}

pub struct TerminalView {
    appearance: TerminalAppearance,
    session: TerminalSession,
    focus: FocusHandle,
    title: String,
    bounds: Bounds<Pixels>,
    cell_width: f32,
    font_size: f32,
    cell_height: f32,
    selecting: bool,
    marked: String,
    scroll_remainder: f32,
    error: Option<String>,
}
impl TerminalView {
    pub fn new(session: TerminalSession, window: &mut Window, cx: &mut Context<Self>) -> Self {
        // Component libraries may bind Tab globally for focus navigation. Give
        // the focused terminal a more specific binding before key dispatch so
        // those inherited actions cannot swallow TUI input.
        if !cx.has_global::<TerminalBindings>() {
            cx.bind_keys([
                KeyBinding::new("tab", TerminalTab, Some("BucketsTerminal")),
                KeyBinding::new("shift-tab", TerminalBacktab, Some("BucketsTerminal")),
            ]);
            cx.set_global(TerminalBindings);
        }
        let title = session.title().to_owned();
        let shared = session.shared.clone();
        let executor = cx.background_executor().clone();
        // Coalesce arbitrary PTY throughput into at most one UI invalidation per 8ms.
        // Idle terminals never invalidate or build grid snapshots.
        cx.spawn(async move |this, cx| {
            let mut revision = 0;
            loop {
                executor.timer(Duration::from_millis(8)).await;
                let next = shared.revision.load(Ordering::Acquire);
                if next == revision {
                    if this.upgrade().is_none() {
                        break;
                    }
                    continue;
                }
                revision = next;
                if this
                    .update(cx, |this, cx| {
                        let events = std::mem::take(&mut *shared.proxy.events.lock());
                        for event in events {
                            match event {
                                Event::Title(title) => this.title = title,
                                Event::ResetTitle => this.title = this.session.title().to_owned(),
                                Event::ClipboardStore(_, text) => {
                                    cx.write_to_clipboard(ClipboardItem::new_string(text))
                                }
                                _ => {}
                            }
                        }
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        let focus = cx.focus_handle();
        cx.on_focus(&focus, window, |this, _, cx| {
            if this
                .session
                .shared
                .term
                .lock()
                .mode()
                .contains(TermMode::FOCUS_IN_OUT)
            {
                let _ = this.session.write(b"\x1b[I");
            }
            cx.notify();
        })
        .detach();
        cx.on_blur(&focus, window, |this, _, cx| {
            if this
                .session
                .shared
                .term
                .lock()
                .mode()
                .contains(TermMode::FOCUS_IN_OUT)
            {
                let _ = this.session.write(b"\x1b[O");
            }
            cx.notify();
        })
        .detach();
        Self {
            appearance: TerminalAppearance::default(),
            session,
            focus,
            title,
            bounds: Bounds::default(),
            cell_width: 7.8 * f32::from(window.rem_size()) / BASE_FONT_SIZE,
            font_size: f32::from(window.rem_size()),
            cell_height: BASE_CELL_HEIGHT * f32::from(window.rem_size()) / BASE_FONT_SIZE,
            selecting: false,
            marked: String::new(),
            scroll_remainder: 0.,
            error: None,
        }
    }
    /// Apply host preferences without replacing the PTY or its running program.
    /// The next layout pass measures the new font and resizes the existing grid.
    pub fn set_appearance(&mut self, appearance: TerminalAppearance, cx: &mut Context<Self>) {
        self.session.set_appearance(appearance.clone());
        self.appearance = appearance;
        cx.notify();
    }
    pub fn title(&self) -> &str {
        &self.title
    }
    pub fn cwd(&self) -> &Path {
        self.session.cwd()
    }
    pub fn exited(&self) -> bool {
        self.session.exited()
    }
    pub fn exit_status(&self) -> Option<String> {
        self.session.exit_status()
    }
    pub fn close(&mut self) {
        self.session.close();
    }
    pub fn last_error(&self) -> Option<&str> {
        self.error.as_deref()
    }
    /// Copy terminal selection through the same path used by Command-C.
    pub fn copy(&mut self, cx: &mut Context<Self>) {
        if let Some(text) = self.session.shared.term.lock().selection_to_string() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }
    /// Paste into the PTY, respecting the application's bracketed-paste mode.
    pub fn paste(&mut self, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.error = self
                .session
                .paste(&text)
                .err()
                .map(|error| format!("Paste: {error:#}"));
        }
        cx.notify();
    }
    /// Select the complete terminal buffer, including bounded scrollback.
    pub fn select_all(&mut self, cx: &mut Context<Self>) {
        let mut term = self.session.shared.term.lock();
        let mut selection = Selection::new(
            SelectionType::Simple,
            TermPoint::new(term.topmost_line(), Column(0)),
            Side::Left,
        );
        selection.update(
            TermPoint::new(term.bottommost_line(), term.last_column()),
            Side::Right,
        );
        term.selection = Some(selection);
        drop(term);
        cx.notify();
    }
    fn send(&mut self, bytes: impl AsRef<[u8]>) {
        self.error = self
            .session
            .write(bytes)
            .err()
            .map(|error| format!("Terminal input: {error:#}"));
    }
    fn input(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let key = &event.keystroke;
        let mods = key.modifiers;
        if mods.platform && key.key == "a" {
            self.select_all(cx);
            cx.stop_propagation();
            return;
        }
        if mods.platform && key.key == "c" || mods.control && mods.shift && key.key == "c" {
            self.copy(cx);
            cx.stop_propagation();
            return;
        }
        if mods.platform && key.key == "v" || mods.control && mods.shift && key.key == "v" {
            self.paste(cx);
            cx.stop_propagation();
            return;
        }
        let mode = *self.session.shared.term.lock().mode();
        if mods.shift
            && !mods.control
            && !mods.alt
            && !mods.platform
            && !mode.contains(TermMode::ALT_SCREEN)
        {
            let scroll = match key.key.as_str() {
                "pageup" => Some(Scroll::PageUp),
                "pagedown" => Some(Scroll::PageDown),
                "home" => Some(Scroll::Top),
                "end" => Some(Scroll::Bottom),
                _ => None,
            };
            if let Some(scroll) = scroll {
                self.session.shared.term.lock().scroll_display(scroll);
                cx.notify();
                cx.stop_propagation();
                return;
            }
        }
        // Text (including composed Unicode) goes through the platform input handler.
        // Control, Option and special keys are terminal escape sequences.
        if is_text_input(key) {
            return;
        }
        if let Some(bytes) = encode_key(key, mode) {
            {
                let mut term = self.session.shared.term.lock();
                term.selection = None;
                term.scroll_display(Scroll::Bottom);
            }
            self.send(bytes);
            cx.notify();
            cx.stop_propagation();
        }
    }
    fn grid_point(&self, pos: Point<Pixels>) -> TermPoint {
        let term = self.session.shared.term.lock();
        let col = ((f32::from(pos.x - self.bounds.origin.x) - PAD) / self.cell_width)
            .floor()
            .max(0.) as usize;
        let row = ((f32::from(pos.y - self.bounds.origin.y) - PAD) / self.cell_height)
            .floor()
            .max(0.) as i32;
        TermPoint::new(
            Line(row.min(term.screen_lines() as i32 - 1) - term.grid().display_offset() as i32),
            Column(col.min(term.columns() - 1)),
        )
    }
    fn mouse(&mut self, pos: Point<Pixels>, button: u8, release: bool, mods: Modifiers) -> bool {
        let term = self.session.shared.term.lock();
        if mods.shift || !term.mode().intersects(TermMode::MOUSE_MODE) {
            return false;
        }
        if button & 32 != 0
            && !term
                .mode()
                .intersects(TermMode::MOUSE_DRAG | TermMode::MOUSE_MOTION)
        {
            return true;
        }
        let col = (((f32::from(pos.x - self.bounds.origin.x) - PAD) / self.cell_width)
            .floor()
            .max(0.) as usize
            + 1)
        .min(term.columns());
        let row = (((f32::from(pos.y - self.bounds.origin.y) - PAD) / self.cell_height)
            .floor()
            .max(0.) as usize
            + 1)
        .min(term.screen_lines());
        let code = button + if mods.alt { 8 } else { 0 } + if mods.control { 16 } else { 0 };
        let sgr = term.mode().contains(TermMode::SGR_MOUSE);
        drop(term);
        let bytes = if sgr {
            format!(
                "\x1b[<{code};{col};{row}{}",
                if release { 'm' } else { 'M' }
            )
            .into_bytes()
        } else if col < 224 && row < 224 {
            vec![
                27,
                b'[',
                b'M',
                32 + if release { 3 } else { code },
                32 + col as u8,
                32 + row as u8,
            ]
        } else {
            return true;
        };
        self.send(bytes);
        true
    }
}
impl Focusable for TerminalView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
impl Render for TerminalView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let entity = cx.entity();
        let paint_entity = entity.clone();
        let background = {
            let term = self.session.shared.term.lock();
            self.appearance.color(
                Color::Named(NamedColor::Background),
                term.renderable_content().colors,
            )
        };
        div()
            .id("terminal-surface")
            .size_full()
            .flex()
            .flex_col()
            .min_w_0()
            .min_h_0()
            .overflow_hidden()
            .bg(rgb(background))
            .track_focus(&self.focus)
            .key_context("BucketsTerminal")
            .on_action(cx.listener(|this, _: &TerminalTab, _, cx| {
                this.send(b"\t");
                cx.notify();
                cx.stop_propagation();
            }))
            .on_action(cx.listener(|this, _: &TerminalBacktab, _, cx| {
                this.send(b"\x1b[Z");
                cx.notify();
                cx.stop_propagation();
            }))
            .on_key_down(cx.listener(Self::input))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                    window.focus(&this.focus);
                    if this.mouse(event.position, 0, false, event.modifiers) {
                        return;
                    }
                    let point = this.grid_point(event.position);
                    let ty = match event.click_count {
                        2 => SelectionType::Semantic,
                        3.. => SelectionType::Lines,
                        _ => SelectionType::Simple,
                    };
                    this.session.shared.term.lock().selection =
                        Some(Selection::new(ty, point, Side::Left));
                    this.selecting = true;
                    cx.notify();
                    cx.stop_propagation();
                }),
            )
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                if event.pressed_button == Some(MouseButton::Left) {
                    if this.mouse(event.position, 32, false, event.modifiers) {
                        return;
                    }
                    if this.selecting {
                        let point = this.grid_point(event.position);
                        if let Some(s) = this.session.shared.term.lock().selection.as_mut() {
                            s.update(point, Side::Right);
                        }
                        cx.notify();
                    }
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, event: &MouseUpEvent, _, cx| {
                    this.mouse(event.position, 0, true, event.modifiers);
                    this.selecting = false;
                    cx.stop_propagation();
                }),
            )
            .on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, _, cx| {
                let delta =
                    f32::from(event.delta.pixel_delta(px(this.cell_height)).y) / this.cell_height;
                this.scroll_remainder += delta;
                let lines = this.scroll_remainder.trunc() as i32;
                if lines != 0 {
                    this.scroll_remainder -= lines as f32;
                    if this.mouse(
                        event.position,
                        if lines > 0 { 64 } else { 65 },
                        false,
                        event.modifiers,
                    ) {
                        for _ in 1..lines.abs().min(50) {
                            this.mouse(
                                event.position,
                                if lines > 0 { 64 } else { 65 },
                                false,
                                event.modifiers,
                            );
                        }
                    } else {
                        let mode = *this.session.shared.term.lock().mode();
                        if mode.contains(TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL)
                            && !event.modifiers.shift
                        {
                            for _ in 0..lines.abs().min(50) {
                                this.send(if lines > 0 { b"\x1bOA" } else { b"\x1bOB" });
                            }
                        } else {
                            this.session.scroll(lines);
                        }
                    }
                    cx.notify();
                }
                cx.stop_propagation();
            }))
            .child(
                canvas(
                    move |bounds, window, cx| {
                        entity.update(cx, |this, _| {
                            this.bounds = bounds;
                            this.font_size = f32::from(window.rem_size());
                            this.cell_height = BASE_CELL_HEIGHT * this.font_size / BASE_FONT_SIZE;
                            let run = TextRun {
                                len: 1,
                                font: font(this.appearance.font_family.clone()),
                                color: rgb(this.appearance.foreground).into(),
                                background_color: None,
                                underline: None,
                                strikethrough: None,
                            };
                            this.cell_width = f32::from(
                                window
                                    .text_system()
                                    .shape_line("M".into(), px(this.font_size), &[run], None)
                                    .width,
                            )
                            .max(1.);
                            let cols = ((f32::from(bounds.size.width) - 2. * PAD) / this.cell_width)
                                .floor()
                                .max(2.) as usize;
                            let rows = ((f32::from(bounds.size.height) - 2. * PAD)
                                / this.cell_height)
                                .floor()
                                .max(1.) as usize;
                            if let Err(error) = this.session.resize(cols, rows) {
                                this.error = Some(format!("Resize: {error:#}"));
                            }
                            let mut snapshot = snapshot(this);
                            snapshot.focused = this.focus.is_focused(window);
                            snapshot
                        })
                    },
                    move |bounds, snapshot, window, cx| {
                        let focus = paint_entity.read(cx).focus.clone();
                        window.handle_input(
                            &focus,
                            ElementInputHandler::new(bounds, paint_entity.clone()),
                            cx,
                        );
                        window.with_content_mask(Some(ContentMask { bounds }), |window| {
                            paint(snapshot, bounds, window, cx)
                        });
                    },
                )
                .flex_1()
                .min_h_0()
                .w_full(),
            )
            .when_some(self.error.clone(), |this, error| {
                this.child(
                    div()
                        .flex_shrink_0()
                        .px_2()
                        .py_1()
                        .bg(rgb(self.appearance.surface))
                        .text_size(rems(11. / BASE_FONT_SIZE))
                        .text_color(rgb(self.appearance.danger))
                        .child(error),
                )
            })
    }
}

#[derive(Clone, Copy, PartialEq)]
struct Style {
    fg: u32,
    bg: u32,
    flags: Flags,
}
struct Segment {
    row: usize,
    col: usize,
    text: String,
    style: Style,
}
struct Snapshot {
    appearance: TerminalAppearance,
    cursor_color: u32,
    segments: Vec<Segment>,
    backgrounds: Vec<(usize, usize, usize, u32)>,
    cursor: Option<(usize, usize, CursorShape)>,
    cell_width: f32,
    font_size: f32,
    cell_height: f32,
    focused: bool,
    marked: String,
}
fn snapshot(view: &TerminalView) -> Snapshot {
    let term = view.session.shared.term.lock();
    let content = term.renderable_content();
    let mut segments: Vec<Segment> = vec![];
    let mut backgrounds: Vec<(usize, usize, usize, u32)> = vec![];
    for indexed in content.display_iter {
        let cell = indexed.cell;
        let row = (indexed.point.line.0 + content.display_offset as i32) as usize;
        let col = indexed.point.column.0;
        let mut fg = view.appearance.color(cell.fg, content.colors);
        let mut bg = view.appearance.color(cell.bg, content.colors);
        if cell.flags.contains(Flags::INVERSE) {
            std::mem::swap(&mut fg, &mut bg);
        }
        if content.selection.is_some_and(|s| s.contains(indexed.point)) {
            bg = view.appearance.selection_background;
        }
        if cell.flags.contains(Flags::DIM) {
            fg = (((fg >> 16 & 255) * 2 / 3) << 16)
                | (((fg >> 8 & 255) * 2 / 3) << 8)
                | ((fg & 255) * 2 / 3);
        }
        if bg != view.appearance.background {
            if let Some(last) = backgrounds
                .last_mut()
                .filter(|last| last.0 == row && last.1 + last.2 == col && last.3 == bg)
            {
                last.2 += 1;
            } else {
                backgrounds.push((row, col, 1, bg));
            }
        }
        if cell
            .flags
            .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
        {
            continue;
        }
        let style = Style {
            fg,
            bg,
            flags: cell.flags
                & (Flags::BOLD | Flags::ITALIC | Flags::ALL_UNDERLINES | Flags::STRIKEOUT),
        };
        let mut text = if cell.flags.contains(Flags::HIDDEN) {
            " ".to_owned()
        } else {
            cell.c.to_string()
        };
        if !cell.flags.contains(Flags::HIDDEN)
            && let Some(chars) = cell.zerowidth()
        {
            text.extend(chars);
        }
        // Keep wide/combining cells isolated; normal adjacent cells share one shaping call.
        let simple = text.len() == 1 && !cell.flags.contains(Flags::WIDE_CHAR);
        if simple
            && let Some(last) = segments.last_mut().filter(|last| {
                last.row == row
                    && last.col + last.text.len() == col
                    && last.style == style
                    && last.text.is_ascii()
            })
        {
            last.text.push_str(&text);
        } else {
            segments.push(Segment {
                row,
                col,
                text,
                style,
            });
        }
    }
    let row = content.cursor.point.line.0 + content.display_offset as i32;
    let cursor = if row >= 0
        && row < term.screen_lines() as i32
        && content.cursor.shape != CursorShape::Hidden
    {
        Some((
            row as usize,
            content.cursor.point.column.0,
            content.cursor.shape,
        ))
    } else {
        None
    };
    Snapshot {
        appearance: view.appearance.clone(),
        cursor_color: view
            .appearance
            .color(Color::Named(NamedColor::Cursor), content.colors),
        segments,
        backgrounds,
        cursor,
        cell_width: view.cell_width,
        font_size: view.font_size,
        cell_height: view.cell_height,
        focused: false,
        marked: view.marked.clone(),
    }
}
fn paint(snapshot: Snapshot, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
    let font_size = snapshot.font_size;
    let cell_height = snapshot.cell_height;
    let origin = bounds.origin + point(px(PAD), px(PAD));
    let cw = px(snapshot.cell_width);
    for (row, col, width, bg) in snapshot.backgrounds {
        window.paint_quad(fill(
            Bounds::new(
                origin + point(cw * col as f32, px(cell_height * row as f32)),
                size(cw * width as f32, px(cell_height)),
            ),
            rgb(bg),
        ));
    }
    for segment in snapshot.segments {
        let mut face = font(snapshot.appearance.font_family.clone());
        face.features = FontFeatures::disable_ligatures();
        if segment.style.flags.contains(Flags::BOLD) {
            face = face.bold();
        }
        if segment.style.flags.contains(Flags::ITALIC) {
            face = face.italic();
        }
        let underline = segment
            .style
            .flags
            .intersects(Flags::ALL_UNDERLINES)
            .then_some(UnderlineStyle {
                thickness: px(1.),
                color: Some(rgb(segment.style.fg).into()),
                wavy: segment.style.flags.contains(Flags::UNDERCURL),
            });
        let run = TextRun {
            len: segment.text.len(),
            font: face,
            color: rgb(segment.style.fg).into(),
            background_color: None,
            underline,
            strikethrough: segment.style.flags.contains(Flags::STRIKEOUT).then_some(
                StrikethroughStyle {
                    thickness: px(1.),
                    color: Some(rgb(segment.style.fg).into()),
                },
            ),
        };
        let force_width = segment.text.is_ascii().then_some(cw);
        let line = window.text_system().shape_line(
            segment.text.into(),
            px(font_size),
            &[run],
            force_width,
        );
        let _ = line.paint(
            origin
                + point(
                    cw * segment.col as f32,
                    px(cell_height * segment.row as f32),
                ),
            px(cell_height),
            window,
            cx,
        );
    }
    if let Some((row, col, shape)) = snapshot.cursor {
        let pos = origin + point(cw * col as f32, px(cell_height * row as f32));
        let cursor = match shape {
            CursorShape::Beam => Bounds::new(pos, size(px(2.), px(cell_height))),
            CursorShape::Underline => {
                Bounds::new(pos + point(px(0.), px(cell_height - 2.)), size(cw, px(2.)))
            }
            _ => Bounds::new(pos, size(cw, px(cell_height))),
        };
        // A translucent block preserves underlying text and matches the workspace accent.
        window.paint_quad(fill(
            cursor,
            rgba((snapshot.cursor_color << 8) | if snapshot.focused { 0x80 } else { 0x50 }),
        ));
        if !snapshot.marked.is_empty() {
            let run = TextRun {
                len: snapshot.marked.len(),
                font: font(snapshot.appearance.font_family.clone()),
                color: rgb(snapshot.appearance.foreground).into(),
                background_color: Some(rgb(snapshot.appearance.surface).into()),
                underline: Some(UnderlineStyle {
                    thickness: px(1.),
                    color: None,
                    wavy: false,
                }),
                strikethrough: None,
            };
            let line = window.text_system().shape_line(
                snapshot.marked.into(),
                px(font_size),
                &[run],
                Some(cw),
            );
            let _ = line.paint(pos, px(cell_height), window, cx);
        }
    }
}

impl EntityInputHandler for TerminalView {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        actual: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let units: Vec<u16> = self.marked.encode_utf16().collect();
        let mut start = range.start.min(units.len());
        let mut end = range.end.min(units.len()).max(start);
        if start > 0 && start < units.len() && (0xdc00..=0xdfff).contains(&units[start]) {
            start -= 1;
        }
        if end < units.len() && (0xdc00..=0xdfff).contains(&units[end]) {
            end += 1;
        }
        *actual = Some(start..end);
        Some(String::from_utf16_lossy(&units[start..end]))
    }
    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let len = self.marked.encode_utf16().count();
        Some(UTF16Selection {
            range: len..len,
            reversed: false,
        })
    }
    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        (!self.marked.is_empty()).then(|| 0..self.marked.encode_utf16().count())
    }
    fn unmark_text(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.marked.clear();
        cx.notify();
    }
    fn replace_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.marked.clear();
        {
            let mut term = self.session.shared.term.lock();
            term.selection = None;
            term.scroll_display(Scroll::Bottom);
        }
        self.send(text);
        cx.notify();
    }
    fn replace_and_mark_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        text: &str,
        _: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.marked = text.into();
        cx.notify();
    }
    fn bounds_for_range(
        &mut self,
        _: Range<usize>,
        _: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let term = self.session.shared.term.lock();
        let p = term.grid().cursor.point;
        Some(Bounds::new(
            self.bounds.origin
                + point(
                    px(PAD + self.cell_width * p.column.0 as f32),
                    px(PAD + self.cell_height * p.line.0 as f32),
                ),
            size(px(self.cell_width), px(self.cell_height)),
        ))
    }
    fn character_index_for_point(
        &mut self,
        _: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        Some(0)
    }
}
