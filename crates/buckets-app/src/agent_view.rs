//! Generic agent answer pane. Provider/model/network work stays in plugins.
use crate::{theme, ui};
use buckets_plugin::{AgentMode, AgentResult, AgentService};
use gpui::{prelude::*, *};
use gpui_component::input::{Input, InputEvent, InputState};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

pub type ServiceCache = Arc<Mutex<Option<AgentService>>>;
#[derive(Clone)]
pub enum AgentViewEvent {
    Run {
        mode: AgentMode,
        prompt: String,
        memory_update: Option<String>,
    },
    Preview {
        location: String,
        line: Option<u64>,
    },
    Open {
        location: String,
        line: Option<u64>,
    },
    Results,
    Close,
    CancelClose,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Control {
    Composer,
    Run,
    Copy,
    Results,
    Memory,
    MemoryEditor,
    MemorySave,
    MemoryReload,
    MemoryRevert,
    Preview(usize),
    Open(usize),
    SaveClose,
    DiscardClose,
    KeepEditing,
}
actions!(agent_pane, [NextControl, PreviousControl]);
pub struct AgentView {
    plugin: String,
    title: String,
    mode: AgentMode,
    focus: FocusHandle,
    composer: Entity<InputState>,
    memory: Entity<InputState>,
    _subscriptions: Vec<Subscription>,
    scroll: ScrollHandle,
    result: Option<AgentResult>,
    progress: Vec<String>,
    preview: Option<(String, String)>,
    preview_generation: u64,
    message: String,
    error: bool,
    busy: bool,
    generation: u64,
    cancel: Option<Arc<AtomicBool>>,
    service: ServiceCache,
    selected: usize,
    memory_open: bool,
    memory_saved: Option<String>,
    memory_dirty: bool,
    close_prompt: bool,
    close_after_memory: bool,
    pending_memory_update: Option<String>,
    exit_requested: bool,
}
impl EventEmitter<AgentViewEvent> for AgentView {}
impl Focusable for AgentView {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        if self.close_prompt {
            self.focus.clone()
        } else if self.memory_open {
            self.memory.focus_handle(cx)
        } else {
            self.composer.focus_handle(cx)
        }
    }
}
impl Drop for AgentView {
    fn drop(&mut self) {
        if let Some(cancel) = &self.cancel {
            cancel.store(true, Ordering::SeqCst);
        }
        let service = self.service.clone();
        std::thread::spawn(move || {
            if let Ok(mut slot) = service.lock() {
                slot.take();
            }
        });
    }
}
impl AgentView {
    pub fn new(
        plugin: String,
        title: String,
        mode: AgentMode,
        prompt: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.bind_keys([
            KeyBinding::new("tab", NextControl, Some("BucketsAgent")),
            KeyBinding::new("shift-tab", PreviousControl, Some("BucketsAgent")),
        ]);
        let composer = cx.new(|cx| {
            let mut input = InputState::new(window, cx).placeholder("Ask about the file panes…");
            input.set_value(prompt, window, cx);
            input
        });
        let memory = cx.new(|cx| {
            InputState::new(window, cx)
                .multi_line(true)
                .rows(8)
                .placeholder("Workspace notes saved by this plugin…")
        });
        let subscriptions = vec![
            cx.subscribe_in(&composer, window, |this, _, event, _, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    this.submit(cx);
                } else if matches!(event, InputEvent::Focus) {
                    this.select(Control::Composer, cx);
                }
            }),
            cx.subscribe_in(&memory, window, |this, _, event, _, cx| {
                if matches!(event, InputEvent::Change) {
                    this.memory_dirty =
                        Some(this.memory.read(cx).value().as_str()) != this.memory_saved.as_deref();
                    cx.notify();
                } else if matches!(event, InputEvent::Focus) {
                    this.select(Control::MemoryEditor, cx);
                }
            }),
        ];
        Self {
            plugin,
            title,
            mode,
            focus: cx.focus_handle(),
            composer,
            memory,
            _subscriptions: subscriptions,
            scroll: ScrollHandle::new(),
            result: None,
            progress: vec![],
            preview: None,
            preview_generation: 0,
            message: "Choose file panes, then ask a question. Host tools are read-only.".into(),
            error: false,
            busy: false,
            generation: 0,
            cancel: None,
            service: Arc::new(Mutex::new(None)),
            selected: 0,
            memory_open: matches!(mode, AgentMode::Memory),
            memory_saved: None,
            memory_dirty: false,
            close_prompt: false,
            close_after_memory: false,
            pending_memory_update: None,
            exit_requested: false,
        }
    }
    pub fn plugin(&self) -> &str {
        &self.plugin
    }
    pub fn title(&self) -> &str {
        &self.title
    }
    pub fn mode(&self) -> AgentMode {
        self.mode
    }
    pub fn result(&self) -> Option<&AgentResult> {
        self.result.as_ref()
    }
    pub fn busy(&self) -> bool {
        self.busy
    }
    pub fn has_unsaved_memory(&self) -> bool {
        self.memory_dirty
    }
    pub fn focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.selected = 0;
        if self.close_prompt {
            self.focus.focus(window);
        } else if self.memory_open {
            self.select(Control::MemoryEditor, cx);
            self.memory.focus_handle(cx).focus(window);
        } else {
            self.composer.focus_handle(cx).focus(window);
        }
        cx.notify();
    }
    pub fn configure(
        &mut self,
        title: String,
        mode: AgentMode,
        prompt: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.title = title;
        self.mode = mode;
        if !prompt.is_empty() {
            self.composer
                .update(cx, |input, cx| input.set_value(prompt, window, cx));
        }
        self.memory_open = matches!(mode, AgentMode::Memory);
        self.focus(window, cx);
    }
    pub fn begin(
        &mut self,
        memory_update: Option<String>,
        cx: &mut Context<Self>,
    ) -> (u64, Arc<AtomicBool>, ServiceCache) {
        self.generation += 1;
        self.pending_memory_update = memory_update;
        self.preview = None;
        self.preview_generation += 1;
        self.busy = true;
        self.error = false;
        self.message = "Working… · Escape cancels".into();
        self.progress.clear();
        let cancel = Arc::new(AtomicBool::new(false));
        self.cancel = Some(cancel.clone());
        cx.notify();
        (self.generation, cancel, self.service.clone())
    }
    pub fn accepts(&self, generation: u64) -> bool {
        self.generation == generation
    }
    pub fn progress(&mut self, generation: u64, message: String, cx: &mut Context<Self>) {
        if !self.accepts(generation) || !self.busy {
            return;
        }
        if self.progress.len() == 128 {
            self.progress.remove(0);
        }
        self.progress.push(message.chars().take(2048).collect());
        cx.notify();
    }
    pub fn complete(
        &mut self,
        generation: u64,
        mode: AgentMode,
        result: Result<AgentResult, String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.accepts(generation) {
            return;
        }
        let cancelled = self
            .cancel
            .take()
            .is_some_and(|cancel| cancel.load(Ordering::SeqCst));
        self.busy = false;
        if cancelled {
            self.pending_memory_update = None;
            self.close_after_memory = false;
            (self.message, self.error) = crate::agent_context::cancellation_message(
                result.as_ref().err().map(String::as_str),
            );
            cx.notify();
            return;
        }
        match result {
            Ok(result) => {
                if let Some(memory) = &result.memory {
                    let submitted = self.pending_memory_update.take();
                    let can_replace = if matches!(mode, AgentMode::Memory) {
                        submitted.as_ref().map_or(!self.memory_dirty, |submitted| {
                            self.memory.read(cx).value().as_str() == submitted
                        })
                    } else {
                        !self.memory_dirty
                    };
                    if can_replace {
                        self.memory
                            .update(cx, |input, cx| input.set_value(memory.clone(), window, cx));
                        self.memory_dirty = false;
                    }
                    self.memory_saved = Some(memory.clone());
                    self.memory_dirty = self.memory.read(cx).value().as_str() != memory;
                }
                if !matches!(mode, AgentMode::Memory) {
                    self.result = Some(result);
                }
                self.message = if matches!(mode, AgentMode::Memory) {
                    "Workspace memory loaded or saved."
                } else {
                    "Complete · Use Tab to reach citations, results and memory."
                }
                .into();
                self.error = false;
                if self.close_after_memory
                    && matches!(mode, AgentMode::Memory)
                    && !self.memory_dirty
                {
                    self.close_after_memory = false;
                    self.memory_dirty = false;
                    cx.emit(AgentViewEvent::Close);
                } else if matches!(mode, AgentMode::Memory) {
                    self.close_after_memory = false;
                }
            }
            Err(error) => {
                self.pending_memory_update = None;
                self.error = true;
                self.message = error;
                self.close_after_memory = false;
            }
        }
        cx.notify();
    }
    pub fn begin_preview(&mut self, cx: &mut Context<Self>) -> (u64, u64) {
        self.preview_generation += 1;
        self.preview = Some(("Preview".into(), "Loading…".into()));
        cx.notify();
        (self.generation, self.preview_generation)
    }
    pub fn set_preview(
        &mut self,
        token: (u64, u64),
        label: String,
        text: String,
        cx: &mut Context<Self>,
    ) {
        if token != (self.generation, self.preview_generation) {
            return;
        }
        self.preview = Some((label, text));
        cx.notify();
    }
    pub fn cancel(&mut self, cx: &mut Context<Self>) {
        if let Some(cancel) = &self.cancel {
            cancel.store(true, Ordering::SeqCst);
        }
        self.message = "Cancelling…".into();
        cx.notify();
    }
    pub fn invalidate(&mut self, message: &str, cx: &mut Context<Self>) {
        if let Some(cancel) = &self.cancel {
            cancel.store(true, Ordering::SeqCst);
        }
        self.generation += 1;
        self.busy = false;
        self.error = true;
        self.message = message.into();
        let service = std::mem::replace(&mut self.service, Arc::new(Mutex::new(None)));
        std::thread::spawn(move || {
            if let Ok(mut slot) = service.lock() {
                slot.take();
            }
        });
        cx.notify();
    }
    pub fn request_close(&mut self, exit: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            self.cancel(cx);
        }
        self.exit_requested = exit;
        if self.memory_dirty {
            self.close_prompt = true;
            self.selected = 0;
            self.focus.focus(window);
            cx.notify();
        } else {
            cx.emit(AgentViewEvent::Close);
        }
    }
    fn submit(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            self.cancel(cx);
            return;
        }
        let prompt = self.composer.read(cx).value().to_string();
        if prompt.trim().is_empty() && !matches!(self.mode, AgentMode::Memory | AgentMode::Index) {
            self.message = "Enter a question or search request.".into();
            self.error = true;
            cx.notify();
            return;
        }
        cx.emit(AgentViewEvent::Run {
            mode: self.mode,
            prompt,
            memory_update: None,
        });
    }
    fn controls(&self) -> Vec<Control> {
        if self.close_prompt {
            return vec![
                Control::SaveClose,
                Control::DiscardClose,
                Control::KeepEditing,
            ];
        }
        let mut controls = vec![
            Control::Composer,
            Control::Run,
            Control::Copy,
            Control::Results,
            Control::Memory,
        ];
        if self.memory_open {
            controls.extend([
                Control::MemoryEditor,
                Control::MemorySave,
                Control::MemoryReload,
                Control::MemoryRevert,
            ]);
        }
        if let Some(result) = &self.result {
            for i in 0..result.citations.len() {
                controls.extend([Control::Preview(i), Control::Open(i)]);
            }
        }
        controls
    }
    fn select(&mut self, control: Control, cx: &mut Context<Self>) {
        if let Some(index) = self
            .controls()
            .iter()
            .position(|candidate| *candidate == control)
        {
            self.selected = index;
        }
        cx.notify();
    }
    fn move_focus(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        let controls = self.controls();
        self.selected =
            (self.selected as isize + delta).rem_euclid(controls.len() as isize) as usize;
        match controls[self.selected] {
            Control::Composer => self.composer.focus_handle(cx).focus(window),
            Control::MemoryEditor => self.memory.focus_handle(cx).focus(window),
            _ => self.focus.focus(window),
        }
        cx.notify();
    }
    fn activate(&mut self, control: Control, window: &mut Window, cx: &mut Context<Self>) {
        self.select(control, cx);
        match control {
            Control::Composer => self.composer.focus_handle(cx).focus(window),
            Control::Run => self.submit(cx),
            Control::Copy => {
                if let Some(result) = &self.result {
                    cx.write_to_clipboard(ClipboardItem::new_string(result.answer.clone()));
                    self.message = "Answer copied.".into();
                }
            }
            Control::Results => cx.emit(AgentViewEvent::Results),
            Control::Memory => {
                self.memory_open = !self.memory_open;
                if self.memory_open && self.memory_saved.is_none() {
                    cx.emit(AgentViewEvent::Run {
                        mode: AgentMode::Memory,
                        prompt: String::new(),
                        memory_update: None,
                    });
                }
            }
            Control::MemoryEditor => self.memory.focus_handle(cx).focus(window),
            Control::MemorySave | Control::SaveClose => {
                if self.busy {
                    self.message = "Wait for cancellation to finish before saving memory.".into();
                    self.error = true;
                } else {
                    self.close_after_memory = control == Control::SaveClose;
                    cx.emit(AgentViewEvent::Run {
                        mode: AgentMode::Memory,
                        prompt: String::new(),
                        memory_update: Some(self.memory.read(cx).value().to_string()),
                    });
                }
            }
            Control::MemoryRevert => {
                if let Some(saved) = self.memory_saved.clone() {
                    self.memory
                        .update(cx, |input, cx| input.set_value(saved, window, cx));
                    self.memory_dirty = false;
                    self.message = "Unsaved memory edits reverted.".into();
                    self.error = false;
                }
            }
            Control::MemoryReload => {
                if self.memory_dirty {
                    self.message="Memory has unsaved edits. Save it, or clear the editor back to its saved contents before reloading.".into();
                    self.error = true;
                } else {
                    cx.emit(AgentViewEvent::Run {
                        mode: AgentMode::Memory,
                        prompt: String::new(),
                        memory_update: None,
                    });
                }
            }
            Control::Preview(i) | Control::Open(i) => {
                if let Some(citation) = self
                    .result
                    .as_ref()
                    .and_then(|result| result.citations.get(i))
                {
                    let event = if matches!(control, Control::Preview(_)) {
                        AgentViewEvent::Preview {
                            location: citation.location.clone(),
                            line: citation.line,
                        }
                    } else {
                        AgentViewEvent::Open {
                            location: citation.location.clone(),
                            line: citation.line,
                        }
                    };
                    cx.emit(event);
                }
            }
            Control::DiscardClose => {
                self.memory_dirty = false;
                cx.emit(AgentViewEvent::Close);
            }
            Control::KeepEditing => {
                self.close_prompt = false;
                self.memory_open = true;
                self.exit_requested = false;
                cx.emit(AgentViewEvent::CancelClose);
                self.memory.focus_handle(cx).focus(window);
            }
        }
        cx.notify();
    }
    fn on_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let key = event.keystroke.key.as_str();
        let modifiers = event.keystroke.modifiers;
        if key == "escape" && !modifiers.platform && !modifiers.control && !modifiers.alt {
            if self.close_prompt {
                self.activate(Control::KeepEditing, window, cx);
            } else if self.busy {
                self.cancel(cx);
            } else {
                self.focus.focus(window);
            }
            cx.stop_propagation();
            return;
        }
        if self.focus.is_focused(window)
            && !modifiers.platform
            && !modifiers.control
            && !modifiers.alt
        {
            match key {
                "enter" | "space" => {
                    let controls = self.controls();
                    self.activate(controls[self.selected.min(controls.len() - 1)], window, cx);
                }
                "up" => {
                    let mut offset = self.scroll.offset();
                    offset.y += px(60.);
                    self.scroll.set_offset(offset);
                }
                "down" => {
                    let mut offset = self.scroll.offset();
                    offset.y -= px(60.);
                    self.scroll.set_offset(offset);
                }
                _ => return,
            }
            cx.stop_propagation();
        }
    }
    fn button(
        &self,
        label: impl Into<SharedString>,
        control: Control,
        cx: &Context<Self>,
    ) -> Stateful<Div> {
        let colors = theme::palette(cx);
        let index = self
            .controls()
            .iter()
            .position(|candidate| *candidate == control)
            .unwrap_or(0);
        let selected = self.selected == index;
        let scroll = self.scroll.clone();
        div()
            .relative()
            .id(("agent-control", index))
            .px_3()
            .py_2()
            .border_1()
            .border_color(rgb(if selected {
                colors.accent
            } else {
                colors.border
            }))
            .rounded_md()
            .bg(rgb(colors.surface))
            .cursor_pointer()
            .child(label.into())
            .when(
                selected
                    && matches!(
                        control,
                        Control::Preview(_)
                            | Control::Open(_)
                            | Control::MemorySave
                            | Control::MemoryReload
                            | Control::MemoryRevert
                    ),
                |el| {
                    el.child(
                        canvas(
                            move |bounds, window, cx| {
                                let viewport = scroll.bounds();
                                if viewport.size.height <= px(0.) {
                                    return;
                                }
                                let shift = if bounds.top() < viewport.top() {
                                    viewport.top() - bounds.top()
                                } else if bounds.bottom() > viewport.bottom() {
                                    viewport.bottom() - bounds.bottom()
                                } else {
                                    px(0.)
                                };
                                if shift.abs() > px(0.5) {
                                    let mut offset = scroll.offset();
                                    offset.y += shift;
                                    let scroll = scroll.clone();
                                    window.defer(cx, move |_, cx| {
                                        scroll.set_offset(offset);
                                        cx.refresh_windows();
                                    });
                                }
                            },
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .inset_0(),
                    )
                },
            )
            .on_click(cx.listener(move |this, _, window, cx| {
                this.focus.focus(window);
                this.activate(control, window, cx);
            }))
    }
}
impl Render for AgentView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = theme::palette(cx);
        let mut memory_height =
            px((f32::from(window.viewport_size().height) * 0.24).clamp(72., 160.));
        // Keep the complete editor viewport reachable in a compact pane. Its
        // own text scroll handles longer notes; the outer body handles controls.
        let body_height = self.scroll.bounds().size.height;
        if body_height > px(0.) {
            memory_height = memory_height.min(body_height);
        }
        let memory_selected = self.controls().get(self.selected) == Some(&Control::MemoryEditor);
        let memory_scroll = self.scroll.clone();
        let body = if self.close_prompt {
            div()
                .p_4()
                .child("Workspace memory has unsaved edits.")
                .child(
                    div()
                        .mt_3()
                        .flex()
                        .flex_wrap()
                        .gap_2()
                        .child(self.button(
                            if self.exit_requested {
                                "Save and quit"
                            } else {
                                "Save and close"
                            },
                            Control::SaveClose,
                            cx,
                        ))
                        .child(self.button("Discard edits", Control::DiscardClose, cx))
                        .child(self.button("Keep editing", Control::KeepEditing, cx)),
                )
                .into_any_element()
        } else {
            let mut body = div()
                .id("agent-answer")
                .track_scroll(&self.scroll)
                .overflow_y_scroll()
                .flex_1()
                .min_h_0()
                .p_4()
                .text_size(ui(12.))
                .line_height(ui(20.));
            if let Some(result) = &self.result {
                body = body.child(div().whitespace_normal().child(result.answer.clone()));
                if !result.citations.is_empty() {
                    body = body.child(
                        div()
                            .mt_5()
                            .mb_2()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Cited files"),
                    );
                    for (i, citation) in result.citations.iter().enumerate() {
                        body = body.child(
                            div()
                                .mb_3()
                                .child(
                                    div()
                                        .text_color(rgb(colors.muted))
                                        .whitespace_normal()
                                        .child(citation.label.clone().unwrap_or_else(|| {
                                            format!(
                                                "{}{}",
                                                citation.location,
                                                citation
                                                    .line
                                                    .map(|line| format!(":{line}"))
                                                    .unwrap_or_default()
                                            )
                                        })),
                                )
                                .child(
                                    div()
                                        .mt_1()
                                        .flex()
                                        .flex_wrap()
                                        .gap_2()
                                        .child(self.button("Preview", Control::Preview(i), cx))
                                        .child(self.button(
                                            "Open in file pane",
                                            Control::Open(i),
                                            cx,
                                        )),
                                ),
                        );
                    }
                }
                if !result.results.is_empty() {
                    body = body.child(div().mt_3().child(format!(
                        "{} matched files · Open results to browse and operate on them.",
                        result.results.len()
                    )));
                }
            }
            if self.busy {
                body = body.child(
                    div().mt_3().text_color(rgb(colors.muted)).children(
                        self.progress
                            .iter()
                            .map(|message| div().whitespace_normal().child(message.clone())),
                    ),
                );
            }
            if self.result.is_none() && !self.busy && !self.memory_open {
                body=body.child(div().text_color(rgb(colors.muted)).whitespace_normal().child("Ask a question about the open file panes, or choose a search command. Only admitted pane and configured roots are available to host tools."));
            }
            if let Some((label, text)) = &self.preview {
                body = body.child(
                    div()
                        .mt_4()
                        .border_t_1()
                        .border_color(rgb(colors.border))
                        .pt_3()
                        .child(label.clone())
                        .child(
                            div()
                                .mt_2()
                                .font_family(theme::terminal(cx).font_family)
                                .whitespace_normal()
                                .child(text.clone()),
                        ),
                );
            }
            if self.memory_open {
                body=body.child(div().mt_5().mb_2().font_weight(FontWeight::SEMIBOLD).child("Workspace memory")).child(div().text_color(rgb(colors.muted)).child("Persistent notes for this workspace and plugin. Changes save only when chosen.")).child(div().relative().mt_2().child(Input::new(&self.memory).h(memory_height)).when(memory_selected,|el|el.child(canvas(move|bounds,window,cx|{let viewport=memory_scroll.bounds();if viewport.size.height<=px(0.){return;}let shift=if bounds.size.height>viewport.size.height||bounds.top()<viewport.top(){viewport.top()-bounds.top()}else if bounds.bottom()>viewport.bottom(){viewport.bottom()-bounds.bottom()}else{px(0.)};if shift.abs()>px(0.5){let mut offset=memory_scroll.offset();offset.y+=shift;let scroll=memory_scroll.clone();window.defer(cx,move|_,cx|{scroll.set_offset(offset);cx.refresh_windows();});}},|_,_,_,_|{}).absolute().inset_0()))).child(div().mt_2().flex().flex_wrap().gap_2().child(self.button("Save memory",Control::MemorySave,cx)).child(self.button("Reload memory",Control::MemoryReload,cx)).child(self.button("Revert edits",Control::MemoryRevert,cx))).child(div().mt_2().text_color(rgb(if self.memory_dirty{colors.warning}else{colors.muted})).child(if self.memory_dirty{"Unsaved memory edits"}else{"Memory unchanged"}));
            }
            body.into_any_element()
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(colors.bg))
            .text_color(rgb(colors.text))
            .font_family(theme::ui_font(cx))
            .track_focus(&self.focus)
            .key_context("BucketsAgent")
            .capture_key_down(cx.listener(Self::on_key))
            .capture_action(cx.listener(|this, _: &NextControl, window, cx| {
                this.move_focus(1, window, cx);
                cx.stop_propagation();
            }))
            .capture_action(cx.listener(|this, _: &PreviousControl, window, cx| {
                this.move_focus(-1, window, cx);
                cx.stop_propagation();
            }))
            .capture_action(cx.listener(
                |this, _: &gpui_component::input::IndentInline, window, cx| {
                    this.move_focus(1, window, cx);
                    cx.stop_propagation();
                },
            ))
            .capture_action(cx.listener(
                |this, _: &gpui_component::input::OutdentInline, window, cx| {
                    this.move_focus(-1, window, cx);
                    cx.stop_propagation();
                },
            ))
            .child(body)
            .when(!self.close_prompt, |el| {
                el.child(
                    div()
                        .flex_shrink_0()
                        .p_3()
                        .border_t_1()
                        .border_color(rgb(colors.border))
                        .bg(rgb(colors.chrome))
                        .child(Input::new(&self.composer))
                        .child(
                            div()
                                .mt_2()
                                .flex()
                                .flex_wrap()
                                .gap_2()
                                .child(self.button(
                                    if self.busy { "Cancel" } else { "Run" },
                                    Control::Run,
                                    cx,
                                ))
                                .child(self.button("Copy", Control::Copy, cx))
                                .child(self.button("Results", Control::Results, cx))
                                .child(self.button("Memory", Control::Memory, cx)),
                        ),
                )
            })
            .child(
                div()
                    .flex_shrink_0()
                    .px_3()
                    .py_2()
                    .text_size(ui(10.))
                    .text_color(rgb(if self.error {
                        colors.danger
                    } else {
                        colors.muted
                    }))
                    .whitespace_normal()
                    .child(self.message.clone())
                    .child(
                        div().child("Enter runs · Tab / Shift-Tab controls · Escape cancels work"),
                    ),
            )
    }
}
