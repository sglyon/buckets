//! Generic agent answer pane. Provider/model/network work stays in plugins.
use crate::{
    agent_transcript::{FollowTail, MarkdownBlock, Transcript, TurnState, markdown_blocks},
    theme, ui,
};
use buckets_plugin::{
    AccountState, AgentActivityState, AgentEvent, AgentMode, AgentPhase, AgentResult, AgentService,
};
use gpui::{prelude::*, *};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::text::{TextView, TextViewStyle};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::{
    collections::BTreeMap,
    hash::{Hash, Hasher},
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
    Settings,
    ManageUsage(String),
    Close,
    CancelClose,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Control {
    Composer,
    Run,
    CopyTurn(u64),
    CopyCode(u64, usize),
    Retry(u64),
    Activity(u64),
    Latest,
    Results,
    More,
    Memory,
    Settings,
    ManageUsage,
    MemoryEditor,
    MemorySave,
    MemoryReload,
    MemoryRevert,
    Preview(u64, usize),
    Open(u64, usize),
    SaveClose,
    DiscardClose,
    KeepEditing,
}
actions!(agent_pane, [NextControl, PreviousControl]);
pub struct AgentView {
    plugin: String,
    title: String,
    account: Option<AccountState>,
    mode: AgentMode,
    focus: FocusHandle,
    composer: Entity<InputState>,
    memory: Entity<InputState>,
    _subscriptions: Vec<Subscription>,
    scroll: ScrollHandle,
    result: Option<AgentResult>,
    transcript: Transcript,
    follow: FollowTail,
    rendered: BTreeMap<u64, (String, Vec<MarkdownBlock>)>,
    selection_hold: bool,
    last_sent: Option<String>,
    preview: Option<(String, String)>,
    preview_generation: u64,
    message: String,
    error: bool,
    busy: bool,
    generation: u64,
    cancel: Option<Arc<AtomicBool>>,
    service: ServiceCache,
    selected: Control,
    memory_open: bool,
    compact_footer: bool,
    more_open: bool,
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
            KeyBinding::new(
                "shift-enter",
                gpui_component::input::Enter { secondary: true },
                Some("BucketsAgent"),
            ),
            KeyBinding::new("tab", NextControl, Some("BucketsAgent")),
            KeyBinding::new("shift-tab", PreviousControl, Some("BucketsAgent")),
        ]);
        let composer = cx.new(|cx| {
            let mut input = InputState::new(window, cx)
                .auto_grow(2, 6)
                .placeholder("Ask about these files…");
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
                if matches!(event, InputEvent::Focus) {
                    this.selection_hold = false;
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
            account: None,
            mode,
            focus: cx.focus_handle(),
            composer,
            memory,
            _subscriptions: subscriptions,
            scroll: ScrollHandle::new(),
            result: None,
            transcript: Transcript::default(),
            follow: FollowTail::new(),
            rendered: BTreeMap::new(),
            selection_hold: false,
            last_sent: None,
            preview: None,
            preview_generation: 0,
            message: String::new(),
            error: false,
            busy: false,
            generation: 0,
            cancel: None,
            service: Arc::new(Mutex::new(None)),
            selected: Control::Composer,
            memory_open: matches!(mode, AgentMode::Memory),
            compact_footer: false,
            more_open: false,
            memory_saved: None,
            memory_dirty: false,
            close_prompt: false,
            close_after_memory: false,
            pending_memory_update: None,
            exit_requested: false,
        }
    }
    pub fn account_state(&mut self, state: Option<AccountState>, cx: &mut Context<Self>) {
        let previous = self
            .account
            .as_ref()
            .and_then(|account| account.active_account_id.as_ref());
        let next = state
            .as_ref()
            .and_then(|account| account.active_account_id.as_ref());
        if previous.is_some() && previous != next {
            self.transcript.clear();
            self.rendered.clear();
            self.result = None;
            self.preview = None;
        }
        self.account = state;
        cx.notify();
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
        self.selected = Control::Composer;
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
        mode: AgentMode,
        prompt: &str,
        memory_update: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> (u64, Arc<AtomicBool>, ServiceCache) {
        self.generation += 1;
        self.pending_memory_update = memory_update;
        self.preview = None;
        self.preview_generation += 1;
        self.busy = true;
        self.error = false;
        self.message = "Working… · Escape cancels".into();
        if matches!(mode, AgentMode::Ask | AgentMode::Search | AgentMode::Index) {
            self.transcript.begin(mode, prompt.to_owned());
            self.last_sent = Some(prompt.to_owned());
            self.result = None;
            if self.composer.read(cx).value().as_str() == prompt {
                self.composer
                    .update(cx, |input, cx| input.set_value("", window, cx));
            }
            self.follow.latest();
            self.selection_hold = false;
            self.scroll.scroll_to_bottom();
        }
        let cancel = Arc::new(AtomicBool::new(false));
        self.cancel = Some(cancel.clone());
        cx.notify();
        (self.generation, cancel, self.service.clone())
    }
    pub fn accepts(&self, generation: u64) -> bool {
        self.generation == generation
    }
    pub fn progress(&mut self, generation: u64, event: AgentEvent, cx: &mut Context<Self>) {
        if !self.accepts(generation) || !self.busy {
            return;
        }
        if let Some(update) = event.update {
            if self.transcript.update(update) {
                self.follow.updated();
                if self.follow.following && !self.selection_hold {
                    self.scroll.scroll_to_bottom();
                }
            }
        } else if !event.message.is_empty() {
            self.transcript.legacy_progress(&event.message);
            self.message = event.message.chars().take(512).collect();
        }
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
        if !matches!(mode, AgentMode::Memory) {
            self.transcript.finish(result.clone(), cancelled);
            self.follow.updated();
        }
        if cancelled {
            self.pending_memory_update = None;
            self.close_after_memory = false;
            (self.message, self.error) = crate::agent_context::cancellation_message(
                result.as_ref().err().map(String::as_str),
            );
            self.restore_sent_if_empty(window, cx);
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
                    ""
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
                self.restore_sent_if_empty(window, cx);
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
        self.transcript.stop();
        self.message = "Stopping…".into();
        cx.notify();
    }
    pub fn invalidate(&mut self, message: &str, cx: &mut Context<Self>) {
        if let Some(cancel) = &self.cancel {
            cancel.store(true, Ordering::SeqCst);
        }
        self.generation += 1;
        self.transcript.stop();
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
            self.selected = Control::Composer;
            self.focus.focus(window);
            cx.notify();
        } else {
            cx.emit(AgentViewEvent::Close);
        }
    }
    fn submit(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let prompt = self.composer.read(cx).value().to_string();
        if prompt.trim().is_empty() && !matches!(self.mode, AgentMode::Memory | AgentMode::Index) {
            self.message = "Enter a question or search request.".into();
            self.error = true;
            cx.notify();
            return;
        }
        let mode = if matches!(self.mode, AgentMode::Memory | AgentMode::Index) {
            AgentMode::Ask
        } else {
            self.mode
        };
        self.mode = mode;
        cx.emit(AgentViewEvent::Run {
            mode,
            prompt,
            memory_update: None,
        });
    }
    fn restore_sent_if_empty(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.composer.read(cx).value().is_empty()
            && let Some(prompt) = self.last_sent.clone()
        {
            self.composer
                .update(cx, |input, cx| input.set_value(prompt, window, cx));
        }
    }
    pub fn clear_private(&mut self, message: &str, cx: &mut Context<Self>) {
        self.invalidate(message, cx);
        self.transcript.clear();
        self.rendered.clear();
        self.result = None;
        self.preview = None;
        self.selected = Control::Composer;
        self.last_sent = None;
    }
    fn controls(&self) -> Vec<Control> {
        if self.close_prompt {
            return vec![
                Control::SaveClose,
                Control::DiscardClose,
                Control::KeepEditing,
            ];
        }
        let mut controls = vec![Control::Composer, Control::Run];
        for turn in &self.transcript.turns {
            if !turn.activities.is_empty() {
                controls.push(Control::Activity(turn.id));
            }
            if let Some((text, blocks)) = self.rendered.get(&turn.id)
                && !text.is_empty()
            {
                for (index, block) in blocks.iter().enumerate() {
                    if block.code.is_some() {
                        controls.push(Control::CopyCode(turn.id, index));
                    }
                }
                controls.push(Control::CopyTurn(turn.id));
            }
            if !self.busy && matches!(turn.state, TurnState::Stopped | TurnState::Failed) {
                controls.push(Control::Retry(turn.id));
            }
            if let Some(result) = &turn.result {
                for index in 0..result.citations.len() {
                    controls.extend([
                        Control::Preview(turn.id, index),
                        Control::Open(turn.id, index),
                    ]);
                }
            }
        }
        if self.follow.unread || self.selection_hold {
            controls.push(Control::Latest);
        }
        if self
            .result
            .as_ref()
            .is_some_and(|result| !result.results.is_empty())
        {
            controls.push(Control::Results);
        }
        if self.compact_footer {
            controls.push(Control::More);
        }
        if !self.compact_footer || self.more_open {
            controls.extend([Control::Memory, Control::Settings]);
        }
        if (!self.compact_footer || self.more_open)
            && self
                .account
                .as_ref()
                .is_some_and(|account| account.using_plan && account.usage_url.is_some())
        {
            controls.push(Control::ManageUsage);
        }
        if self.memory_open {
            controls.extend([
                Control::MemoryEditor,
                Control::MemorySave,
                Control::MemoryReload,
                Control::MemoryRevert,
            ]);
        }
        controls
    }
    fn select(&mut self, control: Control, cx: &mut Context<Self>) {
        if self.controls().contains(&control) {
            self.selected = control;
        }
        cx.notify();
    }
    fn move_focus(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        let controls = self.controls();
        let selected = controls
            .iter()
            .position(|control| *control == self.selected)
            .unwrap_or(0);
        self.selected =
            controls[(selected as isize + delta).rem_euclid(controls.len() as isize) as usize];
        self.selection_hold = false;
        match self.selected {
            Control::Composer => self.composer.focus_handle(cx).focus(window),
            Control::MemoryEditor => self.memory.focus_handle(cx).focus(window),
            _ => self.focus.focus(window),
        }
        cx.notify();
    }
    fn activate(&mut self, control: Control, window: &mut Window, cx: &mut Context<Self>) {
        self.select(control, cx);
        self.selection_hold = false;
        match control {
            Control::Composer => self.composer.focus_handle(cx).focus(window),
            Control::Run => {
                if self.busy {
                    self.cancel(cx);
                } else {
                    self.submit(cx);
                }
            }
            Control::CopyTurn(id) => {
                if let Some((text, _)) = self.rendered.get(&id) {
                    cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
                    self.message = "Response copied.".into();
                }
            }
            Control::CopyCode(id, index) => {
                if let Some(code) = self
                    .rendered
                    .get(&id)
                    .and_then(|(_, blocks)| blocks.get(index))
                    .and_then(|block| block.code.clone())
                {
                    cx.write_to_clipboard(ClipboardItem::new_string(code));
                    self.message = "Code copied.".into();
                }
            }
            Control::Retry(id) => {
                if !self.busy
                    && let Some(turn) = self.transcript.get(id)
                {
                    cx.emit(AgentViewEvent::Run {
                        mode: turn.mode,
                        prompt: turn.request_prompt.clone(),
                        memory_update: None,
                    });
                }
            }
            Control::Activity(id) => self.transcript.toggle_activity(id),
            Control::Latest => {
                self.follow.latest();
                self.scroll.scroll_to_bottom();
            }
            Control::Results => cx.emit(AgentViewEvent::Results),
            Control::More => self.more_open = !self.more_open,
            Control::Settings => cx.emit(AgentViewEvent::Settings),
            Control::ManageUsage => {
                if let Some(url) = self
                    .account
                    .as_ref()
                    .filter(|account| account.using_plan)
                    .and_then(|account| account.usage_url.clone())
                {
                    cx.emit(AgentViewEvent::ManageUsage(url));
                }
            }
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
            Control::Preview(id, i) | Control::Open(id, i) => {
                if let Some(citation) = self
                    .transcript
                    .get(id)
                    .and_then(|turn| turn.result.as_ref())
                    .and_then(|result| result.citations.get(i))
                {
                    let event = if matches!(control, Control::Preview(_, _)) {
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
    fn escape(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let focused_input = if self.composer.focus_handle(cx).is_focused(window) {
            Some(self.composer.clone())
        } else if self.memory.focus_handle(cx).is_focused(window) {
            Some(self.memory.clone())
        } else {
            None
        };
        if focused_input.is_some_and(|input| {
            input.update(cx, |input, cx| {
                input.marked_text_range(window, cx).is_some()
            })
        }) {
            return false;
        }
        if self.close_prompt {
            self.activate(Control::KeepEditing, window, cx);
        } else if self.preview.is_some() {
            self.preview = None;
            cx.notify();
        } else if self.busy {
            self.cancel(cx);
        } else if self.memory_open {
            self.memory_open = false;
            self.composer.focus_handle(cx).focus(window);
            cx.notify();
        } else {
            self.selection_hold = false;
            self.focus.focus(window);
        }
        true
    }
    fn on_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let key = event.keystroke.key.as_str();
        let modifiers = event.keystroke.modifiers;
        if key == "escape" && !modifiers.platform && !modifiers.control && !modifiers.alt {
            if self.escape(window, cx) {
                cx.stop_propagation();
            }
            return;
        }
        if (self.focus.is_focused(window)
            || (!self.composer.focus_handle(cx).is_focused(window)
                && !self.memory.focus_handle(cx).is_focused(window)))
            && !modifiers.platform
            && !modifiers.control
            && !modifiers.alt
        {
            match key {
                "enter" | "space" => {
                    let controls = self.controls();
                    let control = if controls.contains(&self.selected) {
                        self.selected
                    } else {
                        Control::Composer
                    };
                    self.activate(control, window, cx);
                }
                "up" | "pageup" | "down" | "pagedown" => {
                    let mut offset = self.scroll.offset();
                    let amount = if key.starts_with("page") {
                        self.scroll.bounds().size.height * 0.8
                    } else {
                        ui(48.).to_pixels(window.rem_size())
                    };
                    offset.y += if matches!(key, "up" | "pageup") {
                        amount
                    } else {
                        -amount
                    };
                    self.scroll.set_offset(offset);
                    self.follow
                        .observe_user_scroll(f32::from(self.scroll.max_offset().height + offset.y));
                    cx.notify();
                }
                "end" => {
                    self.selection_hold = false;
                    self.follow.latest();
                    self.scroll.scroll_to_bottom();
                    cx.notify();
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
        let selected = self.selected == control;
        let scroll = self.scroll.clone();
        div()
            .relative()
            .id(SharedString::from(format!(
                "agent-control-{}",
                Self::control_key(control)
            )))
            .px_2()
            .py_1()
            .text_size(ui(if control == Control::Run { 11. } else { 10. }))
            .border_1()
            .border_color(rgb(if selected {
                colors.accent
            } else {
                colors.border
            }))
            .rounded_md()
            .bg(rgb(if control == Control::Run {
                colors.accent
            } else {
                colors.surface
            }))
            .text_color(rgb(if control == Control::Run {
                colors.bg
            } else {
                colors.text
            }))
            .cursor_pointer()
            .child(label.into())
            .when(
                selected
                    && matches!(
                        control,
                        Control::Preview(_, _)
                            | Control::Open(_, _)
                            | Control::CopyTurn(_)
                            | Control::CopyCode(_, _)
                            | Control::Activity(_)
                            | Control::Retry(_)
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
impl AgentView {
    fn reveal_editor(&self) -> impl IntoElement {
        let scroll = self.scroll.clone();
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
        .inset_0()
    }
    fn control_key(control: Control) -> String {
        match control {
            Control::CopyTurn(id) => format!("copy-{id}"),
            Control::CopyCode(id, block) => format!("code-{id}-{block}"),
            Control::Retry(id) => format!("retry-{id}"),
            Control::Activity(id) => format!("activity-{id}"),
            Control::Preview(id, index) => format!("preview-{id}-{index}"),
            Control::Open(id, index) => format!("open-{id}-{index}"),
            _ => format!("{control:?}"),
        }
    }
}
impl Render for AgentView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = theme::palette(cx);
        self.compact_footer =
            self.scroll.bounds().size.width < ui(520.).to_pixels(window.rem_size());
        // A mouse selection holds the visible text stable until focus leaves the
        // transcript or Jump to latest is chosen. Snapshots continue accumulating.
        let ids = self
            .transcript
            .turns
            .iter()
            .map(|turn| turn.id)
            .collect::<Vec<_>>();
        self.rendered.retain(|id, _| ids.contains(id));
        if !self.selection_hold {
            for turn in &self.transcript.turns {
                if self
                    .rendered
                    .get(&turn.id)
                    .is_none_or(|(text, _)| text != &turn.text)
                {
                    self.rendered
                        .insert(turn.id, (turn.text.clone(), markdown_blocks(&turn.text)));
                }
            }
        }
        if !self.controls().contains(&self.selected) {
            self.selected = Control::Composer;
        }
        let memory_height = (window.viewport_size().height * 0.24)
            .min(ui(160.).to_pixels(window.rem_size()))
            .max(ui(64.).to_pixels(window.rem_size()));
        let mut body = div()
            .id("agent-transcript")
            .track_scroll(&self.scroll)
            .overflow_y_scroll()
            .flex_1()
            .min_h_0()
            .p_4()
            .text_size(ui(13.))
            .line_height(ui(22.))
            .on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, window, cx| {
                let delta = event.delta.pixel_delta(window.line_height());
                this.follow.observe_user_scroll(f32::from(
                    this.scroll.max_offset().height + this.scroll.offset().y + delta.y,
                ));
                cx.notify();
            }))
            .capture_any_mouse_down(cx.listener(|this, _, _, cx| {
                this.selection_hold = true;
                this.follow.following = false;
                cx.notify();
            }));
        if self.close_prompt {
            body = body.child(
                div().child("Workspace memory has unsaved edits.").child(
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
                ),
            );
        } else {
            let mut column = div().w_full().max_w(ui(760.)).mx_auto().min_w_0();
            if self.transcript.turns.is_empty() && !self.memory_open {
                column = column.child(
                    div()
                        .pt_4()
                        .text_color(rgb(colors.muted))
                        .child("Ask about the files in your open panes."),
                );
            }
            for turn in &self.transcript.turns {
                let id = turn.id;
                let rendered = self
                    .rendered
                    .get(&id)
                    .cloned()
                    .unwrap_or_else(|| (String::new(), vec![]));
                let mut message = div().id(("agent-turn", id)).mb_6().min_w_0().child(
                    div()
                        .mb_3()
                        .px_3()
                        .py_2()
                        .bg(rgb(colors.surface))
                        .rounded_md()
                        .child(
                            div()
                                .mb_1()
                                .text_size(ui(10.))
                                .text_color(rgb(colors.muted))
                                .child("You"),
                        )
                        .child(div().whitespace_normal().child(turn.prompt.clone())),
                );
                if !turn.activities.is_empty() {
                    let running = turn.state == TurnState::Streaming
                        && turn
                            .activities
                            .iter()
                            .any(|activity| activity.state == AgentActivityState::Running);
                    message = message.child(div().mb_3().child(self.button(
                        format!(
                            "{} {} file action{}{}",
                            if turn.activity_open { "Hide" } else { "Show" },
                            turn.activities.len(),
                            if turn.activities.len() == 1 { "" } else { "s" },
                            if running { " · working" } else { "" }
                        ),
                        Control::Activity(id),
                        cx,
                    )));
                    if turn.activity_open {
                        for activity in &turn.activities {
                            message = message.child(
                                div()
                                    .mb_1()
                                    .text_size(ui(11.))
                                    .text_color(rgb(
                                        if activity.state == AgentActivityState::Error {
                                            colors.danger
                                        } else {
                                            colors.muted
                                        },
                                    ))
                                    .whitespace_normal()
                                    .child(format!(
                                        "{} · {}",
                                        match activity.state {
                                            AgentActivityState::Running =>
                                                if turn.state == TurnState::Streaming {
                                                    "Reading"
                                                } else {
                                                    "Stopped"
                                                },
                                            AgentActivityState::Complete => "Done",
                                            AgentActivityState::Error => "Failed",
                                        },
                                        activity.title
                                    )),
                            );
                        }
                    }
                }
                for (index, block) in rendered.1.iter().enumerate() {
                    let mut hasher = std::collections::hash_map::DefaultHasher::new();
                    block.markdown.hash(&mut hasher);
                    let block_id = format!("agent-markdown-{id}-{index}-{:x}", hasher.finish());
                    let rem_size = window.rem_size();
                    let mut code_style = div()
                        .bg(rgb(colors.surface))
                        .text_size(ui(12.))
                        .font_family(theme::terminal(cx).font_family);
                    let text_style = TextViewStyle::default()
                        .code_block(gpui::Styled::style(&mut code_style).clone())
                        .paragraph_gap(rems(0.75))
                        .heading_font_size(move |level, _| {
                            ui(match level {
                                1 => 20.,
                                2 => 17.,
                                _ => 14.,
                            })
                            .to_pixels(rem_size)
                        });
                    let mut block_view = div().mb_3().min_w_0().w_full();
                    if block.code.is_some() {
                        block_view =
                            block_view.child(div().mb_1().flex().justify_end().child(self.button(
                                "Copy code",
                                Control::CopyCode(id, index),
                                cx,
                            )));
                    }
                    block_view = block_view.child(
                        TextView::markdown(
                            SharedString::from(block_id),
                            block.markdown.clone(),
                            window,
                            cx,
                        )
                        .selectable(true)
                        .style(text_style),
                    );
                    message = message.child(block_view);
                }
                if turn.state == TurnState::Streaming {
                    let label = turn.legacy_status().unwrap_or(match turn.phase {
                        AgentPhase::Waiting => "Working…",
                        AgentPhase::Reading => "Reading files…",
                        AgentPhase::Responding => "Responding…",
                        AgentPhase::Verifying => "Checking references…",
                    });
                    message = message.child(
                        div()
                            .mt_2()
                            .text_size(ui(10.))
                            .text_color(rgb(colors.muted))
                            .child(label.to_owned()),
                    );
                }
                if matches!(turn.state, TurnState::Stopped | TurnState::Failed) {
                    message = message.child(
                        div()
                            .mt_2()
                            .text_size(ui(11.))
                            .text_color(rgb(if turn.state == TurnState::Failed {
                                colors.danger
                            } else {
                                colors.muted
                            }))
                            .whitespace_normal()
                            .child(if turn.state == TurnState::Stopped {
                                "Stopped · partial response".to_owned()
                            } else {
                                turn.error
                                    .clone()
                                    .unwrap_or_else(|| "Response interrupted".into())
                            }),
                    );
                }
                let mut actions = div().mt_2().flex().flex_wrap().gap_2();
                if !rendered.0.is_empty() {
                    actions =
                        actions.child(self.button("Copy response", Control::CopyTurn(id), cx));
                }
                if !self.busy && matches!(turn.state, TurnState::Stopped | TurnState::Failed) {
                    actions = actions.child(self.button("Retry", Control::Retry(id), cx));
                }
                message = message.child(actions);
                if let Some(result) = &turn.result
                    && !result.citations.is_empty()
                {
                    message = message.child(
                        div()
                            .mt_4()
                            .mb_2()
                            .text_size(ui(11.))
                            .text_color(rgb(colors.muted))
                            .child("References"),
                    );
                    for (index, citation) in result.citations.iter().enumerate() {
                        let label = citation.label.clone().unwrap_or_else(|| {
                            format!(
                                "{}{}",
                                citation.location,
                                citation
                                    .line
                                    .map(|line| format!(":{line}"))
                                    .unwrap_or_default()
                            )
                        });
                        message = message.child(
                            div()
                                .mb_3()
                                .min_w_0()
                                .child(div().text_size(ui(11.)).whitespace_normal().child(label))
                                .child(
                                    div()
                                        .mt_1()
                                        .flex()
                                        .flex_wrap()
                                        .gap_2()
                                        .child(self.button(
                                            "Preview",
                                            Control::Preview(id, index),
                                            cx,
                                        ))
                                        .child(self.button("Open", Control::Open(id, index), cx)),
                                ),
                        );
                    }
                }
                column = column.child(message);
            }
            if let Some((label, text)) = &self.preview {
                column = column.child(
                    div()
                        .mt_4()
                        .border_t_1()
                        .border_color(rgb(colors.border))
                        .pt_3()
                        .child(
                            div()
                                .mb_2()
                                .text_size(ui(11.))
                                .text_color(rgb(colors.muted))
                                .whitespace_normal()
                                .child(label.clone()),
                        )
                        .child(
                            TextView::markdown(
                                "agent-file-preview",
                                crate::agent_transcript::literal_code(text),
                                window,
                                cx,
                            )
                            .selectable(true),
                        ),
                );
            }
            if self.memory_open {
                column = column
                    .child(
                        div()
                            .mt_5()
                            .mb_2()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Workspace memory"),
                    )
                    .child(
                        div()
                            .relative()
                            .child(Input::new(&self.memory).h(memory_height))
                            .when(self.selected == Control::MemoryEditor, |editor| {
                                editor.child(self.reveal_editor())
                            }),
                    )
                    .child(
                        div()
                            .mt_2()
                            .flex()
                            .flex_wrap()
                            .gap_2()
                            .child(self.button("Save memory", Control::MemorySave, cx))
                            .child(self.button("Reload memory", Control::MemoryReload, cx))
                            .child(self.button("Revert edits", Control::MemoryRevert, cx)),
                    )
                    .child(
                        div()
                            .mt_2()
                            .text_size(ui(11.))
                            .text_color(rgb(if self.memory_dirty {
                                colors.warning
                            } else {
                                colors.muted
                            }))
                            .child(if self.memory_dirty {
                                "Unsaved memory edits"
                            } else {
                                "Memory unchanged"
                            }),
                    );
            }
            body = body.child(column);
        }
        let mut root = div()
            .size_full()
            .flex()
            .flex_col()
            .min_w_0()
            .bg(rgb(colors.bg))
            .text_color(rgb(colors.text))
            .font_family(theme::ui_font(cx))
            .track_focus(&self.focus)
            .key_context("BucketsAgent")
            .capture_key_down(cx.listener(Self::on_key))
            .capture_action(cx.listener(
                |this, action: &gpui_component::input::Enter, window, cx| {
                    // Input's multiline Enter action inserts a newline before emitting
                    // PressEnter. Intercept the action itself, ahead of the input.
                    if !this.composer.focus_handle(cx).is_focused(window)
                        || this.composer.update(cx, |input, cx| {
                            input.marked_text_range(window, cx).is_some()
                        })
                    {
                        cx.propagate();
                        return;
                    }
                    if action.secondary || window.modifiers().shift {
                        this.composer
                            .update(cx, |input, cx| input.replace("\n", window, cx));
                    } else {
                        this.submit(cx);
                    }
                    cx.stop_propagation();
                },
            ))
            .capture_action(
                cx.listener(|this, _: &gpui_component::input::Escape, window, cx| {
                    if this.escape(window, cx) {
                        cx.stop_propagation();
                    } else {
                        // Let the input dismiss its marked text before pane actions.
                        cx.propagate();
                    }
                }),
            )
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
            .child(body);
        if !self.close_prompt {
            let composer_height = (window.viewport_size().height * 0.3)
                .min(ui(156.).to_pixels(window.rem_size()))
                .max(ui(58.).to_pixels(window.rem_size()));
            let mut footer = div()
                .flex_shrink_0()
                .border_t_1()
                .border_color(rgb(colors.border))
                .bg(rgb(colors.chrome))
                .p_3();
            if self.follow.unread || self.selection_hold {
                footer = footer.child(div().mb_2().flex().justify_center().child(self.button(
                    "Jump to latest",
                    Control::Latest,
                    cx,
                )));
            }
            footer = footer
                .child(
                    div()
                        .max_h(composer_height)
                        .child(Input::new(&self.composer)),
                )
                .child(
                    div()
                        .mt_2()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap_2()
                        .child(self.button(
                            if self.busy { "Stop" } else { "Send" },
                            Control::Run,
                            cx,
                        ))
                        .when(
                            self.result
                                .as_ref()
                                .is_some_and(|result| !result.results.is_empty()),
                            |el| el.child(self.button("Open results", Control::Results, cx)),
                        )
                        .when(self.compact_footer, |el| {
                            el.child(self.button(
                                if self.more_open { "Less" } else { "More" },
                                Control::More,
                                cx,
                            ))
                        })
                        .when(!self.compact_footer, |el| {
                            el.child(self.button("Memory", Control::Memory, cx))
                                .child(self.button("Settings", Control::Settings, cx))
                        }),
                );
            if self.compact_footer && self.more_open {
                footer = footer.child(
                    div()
                        .mt_2()
                        .flex()
                        .flex_wrap()
                        .gap_2()
                        .child(self.button("Memory", Control::Memory, cx))
                        .child(self.button("Settings", Control::Settings, cx)),
                );
            }
            let mut account = div()
                .mt_2()
                .flex()
                .flex_wrap()
                .items_center()
                .gap_2()
                .text_size(ui(10.))
                .text_color(rgb(colors.muted));
            if let Some(state) = &self.account {
                if state.using_plan {
                    account = account.child(
                        state
                            .plan_label
                            .clone()
                            .unwrap_or_else(|| "Using account plan".into()),
                    );
                    if state.usage_url.is_some() && (!self.compact_footer || self.more_open) {
                        account =
                            account.child(self.button("Manage usage", Control::ManageUsage, cx));
                    }
                } else if state.sign_in_required {
                    account = account.child("Sign in from Settings to use your ChatGPT plan.");
                }
            }
            footer = footer.child(account);
            if !self.message.is_empty()
                && (!self.busy || self.transcript.turns.is_empty())
                && !self.transcript.turns.last().is_some_and(|turn| {
                    turn.state == TurnState::Failed
                        && turn.error.as_deref() == Some(self.message.as_str())
                })
            {
                footer = footer.child(
                    div()
                        .mt_2()
                        .text_size(ui(10.))
                        .text_color(rgb(if self.error {
                            colors.danger
                        } else {
                            colors.muted
                        }))
                        .whitespace_normal()
                        .child(self.message.clone()),
                );
            }
            root = root.child(footer);
        }
        root
    }
}
