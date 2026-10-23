//! File-operation application service. The UI reads cached snapshots only.
use crate::{Browser, human_size, theme, ui};
use buckets_core::{
    OperationAction, OperationJournal, OperationPlan, OperationProgress, OperationRecord,
    OperationState,
};
use gpui::{prelude::*, *};
use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

actions!(
    operations_panel,
    [NextControl, PreviousControl, ActivateControl]
);

pub(crate) fn bindings() -> Vec<KeyBinding> {
    vec![
        KeyBinding::new("tab", NextControl, Some("BucketsOperations")),
        KeyBinding::new("shift-tab", PreviousControl, Some("BucketsOperations")),
        KeyBinding::new("enter", ActivateControl, Some("BucketsOperations")),
        KeyBinding::new("space", ActivateControl, Some("BucketsOperations")),
    ]
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Action {
    Close,
    Cancel,
    Retry,
    Undo,
}
pub(crate) struct OperationUi {
    directory: PathBuf,
    journal: Arc<Mutex<Option<OperationJournal>>>,
    pub(crate) open: bool,
    records: Vec<OperationRecord>,
    progress: Option<OperationProgress>,
    cancel: Option<Arc<AtomicBool>>,
    selected: Option<String>,
    action: Action,
    scroll: UniformListScrollHandle,
    error: Option<String>,
    started: Option<Instant>,
    details_scroll: ScrollHandle,
    details_page: usize,
    history_generation: u64,
}
impl OperationUi {
    pub(crate) fn new(directory: PathBuf) -> Self {
        Self {
            directory,
            journal: Arc::new(Mutex::new(None)),
            open: false,
            records: vec![],
            progress: None,
            cancel: None,
            selected: None,
            action: Action::Close,
            scroll: UniformListScrollHandle::new(),
            error: None,
            started: None,
            details_scroll: ScrollHandle::new(),
            details_page: 0,
            history_generation: 0,
        }
    }
    fn chosen(&self) -> Option<&OperationRecord> {
        self.selected
            .as_ref()
            .and_then(|id| self.records.iter().find(|record| &record.id == id))
    }
    fn actions(&self) -> Vec<Action> {
        let mut actions = vec![Action::Close];
        if self.cancel.is_some() {
            actions.push(Action::Cancel);
        } else if let Some(record) = self.chosen() {
            if record.can_retry() {
                actions.push(Action::Retry);
            }
            if record.undo_available {
                actions.push(Action::Undo);
            }
        }
        actions
    }
}
enum Job {
    Execute(OperationPlan),
    Retry(String),
    Undo(String),
}
impl Browser {
    pub(crate) fn load_operation_history(&mut self, cx: &mut Context<Self>) {
        self.operation_ui.history_generation += 1;
        let path = self.operation_ui.directory.clone();
        let journal = self.operation_ui.journal.clone();
        let generation = self.operation_ui.history_generation;
        let task = cx.background_executor().spawn(async move {
            let mut slot = journal
                .lock()
                .map_err(|_| "Operation service failed".to_owned())?;
            let loaded = OperationJournal::open(path).map_err(|e| format!("{e:#}"))?;
            let records = loaded.records();
            *slot = Some(loaded);
            Ok::<_, String>(records)
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                if generation != this.operation_ui.history_generation {
                    return;
                }
                match result {
                    Ok(records) => {
                        this.operation_ui.selected = records.last().map(|record| record.id.clone());
                        this.operation_ui.records = records;
                        this.operation_ui.error = None;
                    }
                    Err(error) => this.operation_ui.error = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
    }
    pub(crate) fn execute_plan(&mut self, plan: OperationPlan, cx: &mut Context<Self>) {
        self.run_operation(Job::Execute(plan), cx);
    }
    fn run_operation(&mut self, job: Job, cx: &mut Context<Self>) {
        if self.prompt == Some(crate::Prompt::Search) {
            self.message(
                "Finish the name filter before changing files · Enter opens, Escape clears",
                false,
                cx,
            );
            return;
        }
        if self.operation_ui.cancel.is_some() {
            self.message(
                "A file operation is running. Open Operations to stop or inspect it.",
                true,
                cx,
            );
            return;
        }
        self.operation_ui.history_generation += 1;
        let cancel = Arc::new(AtomicBool::new(false));
        self.operation_ui.cancel = Some(cancel.clone());
        self.operation_ui.progress = None;
        self.operation_ui.error = None;
        self.operation_ui.started = Some(Instant::now());
        self.operations += 1;
        self.message(
            "File operation started · Open Operations for progress and recovery",
            false,
            cx,
        );
        let path = self.operation_ui.directory.clone();
        let journal = self.operation_ui.journal.clone();
        let snapshot = Arc::new(Mutex::new(None::<OperationProgress>));
        let writer = snapshot.clone();
        let (tx, rx) = mpsc::sync_channel(1);
        let task = cx.background_executor().spawn(async move {
            let result = (|| {
                let mut slot = journal
                    .lock()
                    .map_err(|_| anyhow::anyhow!("Operation service failed"))?;
                if slot.is_none() {
                    *slot = Some(OperationJournal::open(path)?);
                }
                let service = slot.as_mut().unwrap();
                let progress = |event| {
                    if let Ok(mut latest) = writer.lock() {
                        *latest = Some(event);
                    }
                };
                let record = match job {
                    Job::Execute(plan) => service.execute(plan, &cancel, progress),
                    Job::Retry(id) => service.retry(&id, &cancel, progress),
                    Job::Undo(id) => service.undo(&id, &cancel, progress),
                }?;
                Ok::<_, anyhow::Error>((record, service.records()))
            })();
            let _ = tx.send(result.map_err(|error| format!("{error:#}")));
        });
        task.detach();
        let executor = cx.background_executor().clone();
        cx.spawn(async move|this,cx|{loop{
            if let Ok(mut latest)=snapshot.lock() && let Some(progress)=latest.take(){let _=this.update(cx,|this,cx|{this.operation_ui.progress=Some(progress);cx.notify();});}
            match rx.try_recv(){
                Ok(result)=>{let _=this.update(cx,|this,cx|{this.operations=this.operations.saturating_sub(1);this.operation_ui.cancel=None;this.operation_ui.started=None;this.operation_ui.progress=None;
                    match result{Ok((record,records))=>{this.operation_ui.selected=Some(record.id.clone());this.operation_ui.details_page=0;this.operation_ui.details_scroll.set_offset(point(px(0.),px(0.)));this.operation_ui.records=records;this.operation_ui.error=None;let error=matches!(record.state,OperationState::Failed|OperationState::Cancelled|OperationState::Interrupted)||record.error.is_some();this.message(format!("{} · {} · Operations has details and recovery",record.label,state_label(record.state)),error,cx);}
                        Err(error)=>{this.operation_ui.error=Some(error.clone());this.message(format!("Operation was not completed: {error}"),true,cx);}}
                    for index in 0..this.panes.len(){if this.panes[index].is_files(){this.reload(index,cx);}}cx.notify();});break;}
                Err(mpsc::TryRecvError::Disconnected)=>{let _=this.update(cx,|this,cx|{this.operations=this.operations.saturating_sub(1);this.operation_ui.cancel=None;this.operation_ui.error=Some("Operation worker stopped; reopen Operations to inspect its journal".into());cx.notify();});break;}
                Err(mpsc::TryRecvError::Empty)=>{}
            }
            executor.timer(Duration::from_millis(80)).await;
        }}).detach();
    }
    fn operation_action(&mut self, action: Action, window: &mut Window, cx: &mut Context<Self>) {
        match action {
            Action::Close => {
                self.operation_ui.open = false;
                self.focus_active(window, cx);
            }
            Action::Cancel => {
                if let Some(cancel) = &self.operation_ui.cancel {
                    cancel.store(true, Ordering::Release);
                    self.operation_ui.error = Some(
                        "Stopping at the next safe boundary. Completed files are retained.".into(),
                    );
                }
            }
            Action::Retry | Action::Undo => {
                if let Some(record) = self.operation_ui.chosen() {
                    let id = record.id.clone();
                    self.run_operation(
                        if action == Action::Undo {
                            Job::Undo(id)
                        } else {
                            Job::Retry(id)
                        },
                        cx,
                    );
                }
            }
        }
        cx.notify();
    }
    pub(crate) fn move_operation_control(&mut self, forward: bool, cx: &mut Context<Self>) {
        let actions = self.operation_ui.actions();
        let index = actions
            .iter()
            .position(|action| *action == self.operation_ui.action)
            .unwrap_or(0);
        let delta = if forward { 1 } else { actions.len() - 1 };
        self.operation_ui.action = actions[(index + delta) % actions.len()];
        cx.notify();
    }
    pub(crate) fn activate_operation_control(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let action = self.operation_ui.action;
        if self.operation_ui.actions().contains(&action) {
            self.operation_action(action, window, cx);
        }
    }
    pub(crate) fn operations_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = event.keystroke.key.as_str();
        let modifiers = event.keystroke.modifiers;
        if modifiers.platform || modifiers.control || modifiers.alt {
            return;
        }
        match key {
            "escape" => self.operation_action(Action::Close, window, cx),
            "up" | "down" | "k" | "j" => {
                let records = &self.operation_ui.records;
                if !records.is_empty() {
                    let index = self
                        .operation_ui
                        .selected
                        .as_ref()
                        .and_then(|id| records.iter().rev().position(|record| &record.id == id))
                        .unwrap_or(0);
                    let target = if matches!(key, "up" | "k") {
                        index.saturating_sub(1)
                    } else {
                        (index + 1).min(records.len() - 1)
                    };
                    self.operation_ui.selected =
                        Some(records[records.len() - 1 - target].id.clone());
                    self.operation_ui
                        .scroll
                        .scroll_to_item(target, ScrollStrategy::Top);
                    self.operation_ui.action = Action::Close;
                    self.operation_ui.details_page = 0;
                    self.operation_ui
                        .details_scroll
                        .set_offset(point(px(0.), px(0.)));
                    cx.notify();
                }
            }
            "pageup" | "pagedown" => {
                let mut offset = self.operation_ui.details_scroll.offset();
                offset.y += if key == "pageup" {
                    window.viewport_size().height * 0.25
                } else {
                    -window.viewport_size().height * 0.25
                };
                self.operation_ui.details_scroll.set_offset(offset);
                cx.notify();
            }
            "[" | "]" => {
                let count = self
                    .operation_ui
                    .chosen()
                    .map(|record| record.items.len())
                    .unwrap_or(0);
                self.operation_ui.details_page = if key == "[" {
                    self.operation_ui.details_page.saturating_sub(1)
                } else {
                    (self.operation_ui.details_page + 1).min(count.saturating_sub(1) / 20)
                };
                self.operation_ui
                    .details_scroll
                    .set_offset(point(px(0.), px(0.)));
                cx.notify();
            }
            _ => return,
        }
        cx.stop_propagation();
    }
    fn operation_button(
        &self,
        title: &'static str,
        action: Action,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let colors = theme::palette(cx);
        div()
            .id(title)
            .px_3()
            .py_2()
            .rounded_md()
            .border_1()
            .border_color(rgb(if self.operation_ui.action == action {
                colors.accent
            } else {
                colors.border
            }))
            .bg(rgb(colors.surface))
            .cursor_pointer()
            .child(title)
            .on_click(
                cx.listener(move |this, _, window, cx| this.operation_action(action, window, cx)),
            )
    }
    pub(crate) fn render_operations(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let colors = theme::palette(cx);
        let chosen = self.operation_ui.chosen();
        let progress = self.operation_ui.progress.as_ref();
        let running = progress.map(|progress| {
            let elapsed = self
                .operation_ui
                .started
                .map(|start| start.elapsed().as_secs_f64())
                .unwrap_or(0.);
            let provider_stage = progress.stage.to_lowercase().contains("provider");
            let bytes = if provider_stage {
                String::new()
            } else {
                format!(
                    " · {} local copy bytes{}",
                    human_size(progress.bytes_copied),
                    if elapsed > 0.1 && progress.bytes_copied > 0 {
                        format!(
                            " · {}/s",
                            human_size((progress.bytes_copied as f64 / elapsed) as u64)
                        )
                    } else {
                        String::new()
                    }
                )
            };
            format!(
                "{} · {} / {} items{} · {}",
                progress.label,
                progress.completed_items,
                progress.total_items,
                bytes,
                progress.stage
            )
        });
        let actions = self.operation_ui.actions();
        let details =
            div()
                .id("operation-details")
                .h((window.viewport_size().height * 0.35)
                    .min(ui(210.).to_pixels(window.rem_size())))
                .min_h(px(70.))
                .overflow_y_scroll()
                .track_scroll(&self.operation_ui.details_scroll)
                .px_4()
                .py_2()
                .border_t_1()
                .border_color(rgb(colors.border))
                .text_size(ui(11.))
                .when_some(self.operation_ui.error.as_ref(), |el, error| {
                    el.child(
                        div()
                            .mb_2()
                            .text_color(rgb(colors.danger))
                            .child(error.clone()),
                    )
                })
                .when_some(chosen, |el, record| {
                    let start = self.operation_ui.details_page * 20;
                    el.when_some(record.error.as_ref(), |el, error| {
                        el.child(
                            div()
                                .mb_2()
                                .text_color(rgb(colors.danger))
                                .child(error.clone()),
                        )
                    })
                    .child(div().mb_2().text_color(rgb(colors.muted)).child(format!(
                        "Items {}–{} of {} · PgUp/PgDn scroll · [ / ] pages",
                        (start + 1).min(record.items.len()),
                        (start + 20).min(record.items.len()),
                        record.items.len()
                    )))
                    .when_some(
                        record.retry_reason().filter(|_| {
                            matches!(
                                record.state,
                                OperationState::Failed
                                    | OperationState::Cancelled
                                    | OperationState::Interrupted
                            )
                        }),
                        |el, reason| {
                            el.child(div().mb_2().text_color(rgb(colors.warning)).child(reason))
                        },
                    )
                    .when_some(record.undo_reason.as_ref(), |el, reason| {
                        el.child(
                            div()
                                .mb_2()
                                .text_color(rgb(colors.muted))
                                .child(reason.clone()),
                        )
                    })
                    .children(
                        record.items.iter().enumerate().skip(start).take(20).map(
                            |(index, item)| {
                                div()
                                    .mb_3()
                                    .child(format!(
                                        "{}. {:?} · {}",
                                        index + 1,
                                        item.state,
                                        describe(&item.action)
                                    ))
                                    .when_some(item.staging.as_ref(), |el, path| {
                                        el.child(
                                            div().mt_1().text_color(rgb(colors.warning)).child(
                                                format!("Recovery staging: {}", path.display()),
                                            ),
                                        )
                                    })
                                    .when_some(item.error.as_ref(), |el, error| {
                                        el.child(
                                            div()
                                                .mt_1()
                                                .text_color(rgb(colors.danger))
                                                .child(error.clone()),
                                        )
                                    })
                                    .when_some(item.undo_reason.as_ref(), |el, reason| {
                                        el.child(
                                            div()
                                                .mt_1()
                                                .text_color(rgb(colors.muted))
                                                .child(reason.clone()),
                                        )
                                    })
                            },
                        ),
                    )
                });
        div().absolute().inset_0().bg(rgb(colors.bg)).flex().flex_col().min_h_0()
            .child(div().flex_shrink_0().px_4().py_3().border_b_1().border_color(rgb(colors.border)).flex().flex_wrap().justify_between().gap_2().child("Operations").child(div().text_color(rgb(colors.muted)).text_size(ui(10.)).child("↑↓ select · Tab actions · Enter run · Esc return")))
            .when_some(running,|el,running|el.child(div().px_4().py_2().text_color(rgb(colors.accent)).text_size(ui(11.)).child(running)))
            .when_some(progress.and_then(|progress|progress.current.as_ref()),|el,current|el.child(div().px_4().pb_2().truncate().text_size(ui(10.)).text_color(rgb(colors.muted)).child(current.display())))
            .child(if self.operation_ui.records.is_empty(){div().flex_1().min_h_0().p_5().text_color(rgb(colors.muted)).child(if self.operation_ui.cancel.is_some(){"Preparing operation…"}else{"No file operations yet. Copy, move, rename, or trash files to see their progress and recovery here."}).into_any_element()}else{
                uniform_list("operation-history",self.operation_ui.records.len(),cx.processor(|this,range:std::ops::Range<usize>,_,cx|{
                    let colors=theme::palette(cx);
                    range.filter_map(|index|{
                        let record=this.operation_ui.records.iter().rev().nth(index)?;
                        let id=record.id.clone();
                        let selected=this.operation_ui.selected.as_deref()==Some(id.as_str());
                        Some(div().id(("operation",index)).h(ui(67.)).px_4().py_2().border_b_1().border_color(rgb(colors.border)).bg(rgb(if selected{colors.surface}else{colors.bg})).cursor_pointer()
                            .child(div().flex().justify_between().gap_3().child(record.label.clone()).child(div().text_size(ui(10.)).text_color(rgb(if record.state==OperationState::Failed{colors.danger}else{colors.muted})).child(state_label(record.state))))
                            .child(div().mt_1().text_size(ui(10.)).text_color(rgb(colors.muted)).truncate().child(record.items.first().map(|item|describe(&item.action)).unwrap_or_default()))
                            .on_click(cx.listener(move|this,_,_,cx|{this.operation_ui.selected=Some(id.clone());this.operation_ui.action=Action::Close;this.operation_ui.details_page=0;this.operation_ui.details_scroll.set_offset(point(px(0.),px(0.)));cx.notify();})))
                    }).collect::<Vec<_>>()
                })).track_scroll(self.operation_ui.scroll.clone()).flex_1().min_h(px(50.)).into_any_element()
            })
            .child(details)
            .child(div().flex_shrink_0().border_t_1().border_color(rgb(colors.border)).px_4().py_3().flex().flex_wrap().gap_2().children(actions.into_iter().map(|action|self.operation_button(match action{Action::Close=>"Return to files",Action::Cancel=>"Stop operation",Action::Retry=>"Retry incomplete",Action::Undo=>"Undo operation"},action,cx))))
    }
}
fn describe(action: &OperationAction) -> String {
    match action {
        OperationAction::Copy {
            source,
            destination,
        }
        | OperationAction::Move {
            source,
            destination,
        } => format!("{} → {}", source.display(), destination.display()),
        OperationAction::Duplicate { source } => format!("Duplicate {}", source.display()),
        OperationAction::Trash { source } => format!("Trash {}", source.display()),
        OperationAction::CreateFile { destination } => {
            format!("Create file {}", destination.display())
        }
        OperationAction::CreateDirectory { destination } => {
            format!("Create folder {}", destination.display())
        }
    }
}
fn state_label(state: OperationState) -> &'static str {
    match state {
        OperationState::Running => "Running",
        OperationState::Complete => "Complete",
        OperationState::Failed => "Failed",
        OperationState::Cancelled => "Stopped",
        OperationState::Interrupted => "Interrupted",
        OperationState::Undone => "Undone",
    }
}
impl OperationUi {
    pub(crate) fn running(&self) -> bool {
        self.cancel.is_some()
    }
}
impl OperationUi {
    pub(crate) fn has_records(&self) -> bool {
        !self.records.is_empty()
    }
}
