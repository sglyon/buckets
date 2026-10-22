//! Presentation state for a bounded, in-memory conversation. Provider and
//! durable transcript ownership remain in the plugin.
use buckets_plugin::{AgentActivity, AgentMode, AgentPhase, AgentResult, AgentUpdate};

pub const MAX_TURNS: usize = 24;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TurnState {
    Streaming,
    Complete,
    Stopped,
    Failed,
}
#[derive(Clone)]
pub struct Turn {
    pub id: u64,
    pub prompt: String,
    pub mode: AgentMode,
    pub request_prompt: String,
    legacy_status: Option<String>,
    pub text: String,
    pub state: TurnState,
    pub phase: AgentPhase,
    pub activities: Vec<AgentActivity>,
    pub activity_open: bool,
    pub activity_chosen: bool,
    pub error: Option<String>,
    pub result: Option<AgentResult>,
    sequence: u64,
}
impl Turn {
    pub fn legacy_status(&self) -> Option<&str> {
        (self.sequence == 0)
            .then_some(self.legacy_status.as_deref())
            .flatten()
    }
}
#[derive(Default)]
pub struct Transcript {
    pub turns: Vec<Turn>,
    next_id: u64,
    active: Option<u64>,
}
impl Transcript {
    pub fn begin(&mut self, mode: AgentMode, request_prompt: String) -> u64 {
        self.next_id = self.next_id.wrapping_add(1);
        let id = self.next_id;
        self.active = Some(id);
        self.turns.push(Turn {
            id,
            prompt: if mode == AgentMode::Index && request_prompt.is_empty() {
                "Reindex semantic search".into()
            } else {
                request_prompt.clone()
            },
            mode,
            request_prompt,
            legacy_status: None,
            text: String::new(),
            state: TurnState::Streaming,
            phase: AgentPhase::Waiting,
            activities: vec![],
            activity_open: false,
            activity_chosen: false,
            error: None,
            result: None,
            sequence: 0,
        });
        if self.turns.len() > MAX_TURNS {
            self.turns.remove(0);
        }
        id
    }
    pub fn get(&self, id: u64) -> Option<&Turn> {
        self.turns.iter().find(|turn| turn.id == id)
    }
    pub fn legacy_progress(&mut self, message: &str) {
        if let Some(turn) = self
            .active
            .and_then(|id| self.turns.iter_mut().find(|turn| turn.id == id))
            && turn.state == TurnState::Streaming
            && turn.sequence == 0
        {
            turn.legacy_status = Some(message.chars().take(512).collect());
        }
    }
    pub fn update(&mut self, update: AgentUpdate) -> bool {
        let Some(turn) = self
            .active
            .and_then(|id| self.turns.iter_mut().find(|turn| turn.id == id))
        else {
            return false;
        };
        if turn.state != TurnState::Streaming || update.sequence <= turn.sequence {
            return false;
        }
        turn.sequence = update.sequence;
        turn.text = update.text;
        turn.phase = update.phase;
        turn.activities = update.activities;
        if !turn.activity_chosen {
            turn.activity_open = turn.text.is_empty() && !turn.activities.is_empty();
        }
        true
    }
    pub fn finish(&mut self, result: Result<AgentResult, String>, stopped: bool) {
        let Some(id) = self.active.take() else {
            return;
        };
        let Some(turn) = self.turns.iter_mut().find(|turn| turn.id == id) else {
            return;
        };
        if stopped {
            turn.state = TurnState::Stopped;
            turn.error = None;
            return;
        }
        match result {
            Ok(result) => {
                turn.text = result.answer.clone();
                turn.result = Some(result);
                turn.state = TurnState::Complete;
            }
            Err(error) => {
                turn.state = TurnState::Failed;
                turn.error = Some(error);
            }
        }
        if !turn.activity_chosen {
            turn.activity_open = false;
        }
    }
    pub fn stop(&mut self) {
        self.finish(Err(String::new()), true);
    }
    pub fn toggle_activity(&mut self, id: u64) {
        if let Some(turn) = self.turns.iter_mut().find(|turn| turn.id == id) {
            turn.activity_open = !turn.activity_open;
            turn.activity_chosen = true;
        }
    }
    pub fn clear(&mut self) {
        self.turns.clear();
        self.active = None;
    }
}

/// Track user intent separately from changing content height. Measurements are
/// taken before applying a snapshot; a stream cannot re-enable following.
#[derive(Default)]
pub struct FollowTail {
    pub following: bool,
    pub unread: bool,
}
impl FollowTail {
    pub fn new() -> Self {
        Self {
            following: true,
            unread: false,
        }
    }
    pub fn observe_user_scroll(&mut self, distance_from_bottom: f32) {
        self.following = distance_from_bottom <= 28.;
        if self.following {
            self.unread = false;
        }
    }
    pub fn updated(&mut self) {
        if !self.following {
            self.unread = true;
        }
    }
    pub fn latest(&mut self) {
        self.following = true;
        self.unread = false;
    }
}

#[derive(Clone, Debug)]
pub struct MarkdownBlock {
    pub markdown: String,
    pub code: Option<String>,
}
fn escaped(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        if ch.is_ascii_punctuation() {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}
/// The pinned rich-text renderer opens links and loads images automatically.
/// Replace those AST nodes with inert labels, including autolinks, reference
/// definitions and HTML. Code stays literal. A second parse of the result has
/// no navigable or fetchable nodes; verified citations are separate controls.
pub fn markdown_blocks(source: &str) -> Vec<MarkdownBlock> {
    use markdown::{ParseOptions, mdast::Node};
    fn sanitized(node: &Node, source: &str) -> String {
        let Some(position) = node.position() else {
            return String::new();
        };
        match node {
            Node::Link(_) | Node::LinkReference(_) => escaped(&node.to_string()),
            Node::Image(image) => escaped(&image.alt),
            Node::ImageReference(image) => escaped(&image.alt),
            Node::Html(html) => escaped(&html.value),
            Node::Definition(_) => String::new(),
            _ => {
                let mut value = source[position.start.offset..position.end.offset].to_owned();
                if let Some(children) = node.children() {
                    for child in children.iter().rev() {
                        if let Some(child_position) = child.position() {
                            let start = child_position.start.offset - position.start.offset;
                            let end = child_position.end.offset - position.start.offset;
                            value.replace_range(start..end, &sanitized(child, source));
                        }
                    }
                }
                value
            }
        }
    }
    let Ok(root) = markdown::to_mdast(source, &ParseOptions::gfm()) else {
        return vec![MarkdownBlock {
            markdown: escaped(source),
            code: None,
        }];
    };
    root.children()
        .into_iter()
        .flatten()
        .filter_map(|node| {
            let value = sanitized(node, source);
            if value.is_empty() {
                return None;
            }
            Some(MarkdownBlock {
                markdown: value,
                code: match node {
                    Node::Code(code) => Some(code.value.clone()),
                    _ => None,
                },
            })
        })
        .collect()
}

/// A file preview stays literal even when the file itself contains Markdown fences.
pub fn literal_code(value: &str) -> String {
    let longest = value.split(|ch| ch != '`').map(str::len).max().unwrap_or(0);
    let fence = "`".repeat((longest + 1).max(3));
    format!("{fence}\n{value}\n{fence}")
}
