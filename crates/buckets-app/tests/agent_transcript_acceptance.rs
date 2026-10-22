#[path = "../src/agent_transcript.rs"]
mod agent_transcript;
use agent_transcript::{
    FollowTail, MAX_TURNS, Transcript, TurnState, literal_code, markdown_blocks,
};
use buckets_plugin::{AgentActivity, AgentActivityState, AgentPhase, AgentResult, AgentUpdate};

fn snapshot(sequence: u64, text: &str) -> AgentUpdate {
    AgentUpdate {
        sequence,
        text: text.into(),
        phase: AgentPhase::Responding,
        activities: vec![],
    }
}
#[test]
fn snapshots_replace_and_stale_updates_cannot_rewrite_a_stopped_or_completed_turn() {
    let mut transcript = Transcript::default();
    let first = transcript.begin("Read these files".into());
    assert!(transcript.update(snapshot(3, "Hello, 世界")));
    assert!(!transcript.update(snapshot(2, "older")));
    assert!(transcript.update(snapshot(4, "Replacement after a tool")));
    transcript.stop();
    assert!(!transcript.update(snapshot(5, "late content")));
    assert_eq!(
        transcript.get(first).unwrap().text,
        "Replacement after a tool"
    );
    assert_eq!(transcript.get(first).unwrap().state, TurnState::Stopped);
    assert!(transcript.get(first).unwrap().result.is_none());
    let second = transcript.begin("Follow up".into());
    transcript.update(snapshot(1, "partial"));
    transcript.finish(
        Ok(AgentResult {
            answer: "Verified answer".into(),
            ..Default::default()
        }),
        false,
    );
    assert!(!transcript.update(snapshot(9, "late")));
    assert_eq!(transcript.get(second).unwrap().text, "Verified answer");
    assert!(transcript.get(second).unwrap().result.is_some());
    assert_eq!(transcript.get(first).unwrap().state, TurnState::Stopped);
}
#[test]
fn activity_collapses_on_text_without_overriding_the_users_explicit_choice() {
    let mut transcript = Transcript::default();
    let id = transcript.begin("Question".into());
    let activity = AgentActivity {
        id: "read-1".into(),
        title: "Read README.md".into(),
        state: AgentActivityState::Running,
    };
    transcript.update(AgentUpdate {
        activities: vec![activity.clone()],
        ..snapshot(1, "")
    });
    assert!(transcript.get(id).unwrap().activity_open);
    transcript.update(AgentUpdate {
        activities: vec![activity.clone()],
        ..snapshot(2, "Answer")
    });
    assert!(!transcript.get(id).unwrap().activity_open);
    transcript.toggle_activity(id);
    transcript.update(AgentUpdate {
        activities: vec![activity],
        ..snapshot(3, "Answer grows")
    });
    assert!(transcript.get(id).unwrap().activity_open);
    transcript.finish(Err("Provider interrupted".into()), false);
    assert_eq!(transcript.get(id).unwrap().state, TurnState::Failed);
    assert_eq!(transcript.get(id).unwrap().text, "Answer grows");
    assert!(transcript.get(id).unwrap().result.is_none());
}
#[test]
fn transcript_is_bounded_and_private_context_reset_discards_displayed_history() {
    let mut transcript = Transcript::default();
    for index in 0..MAX_TURNS + 3 {
        transcript.begin(format!("Question {index}"));
        transcript.finish(Ok(AgentResult::default()), false);
    }
    assert_eq!(transcript.turns.len(), MAX_TURNS);
    assert_eq!(transcript.turns[0].prompt, "Question 3");
    let old = transcript.turns.last().unwrap().id;
    transcript.clear();
    assert!(transcript.turns.is_empty());
    assert!(!transcript.update(snapshot(900, "previous account")));
    let next = transcript.begin("New account".into());
    assert_ne!(old, next);
}
#[test]
fn following_never_resumes_from_new_content_and_latest_is_explicit() {
    let mut follow = FollowTail::new();
    follow.updated();
    assert!(follow.following);
    assert!(!follow.unread);
    follow.observe_user_scroll(200.);
    for _ in 0..100 {
        follow.updated();
    }
    assert!(!follow.following);
    assert!(follow.unread);
    follow.latest();
    assert!(follow.following);
    assert!(!follow.unread);
    follow.observe_user_scroll(20.);
    assert!(follow.following);
}

fn assert_inert(node: &markdown::mdast::Node) {
    use markdown::mdast::Node;
    assert!(
        !matches!(
            node,
            Node::Link(_)
                | Node::LinkReference(_)
                | Node::Image(_)
                | Node::ImageReference(_)
                | Node::Html(_)
                | Node::Definition(_)
        ),
        "active Markdown survived: {node:?}"
    );
    if let Some(children) = node.children() {
        for child in children {
            assert_inert(child);
        }
    }
}
#[test]
fn hostile_markdown_stays_inert_after_the_renderers_second_gfm_parse() {
    let examples = [
        "[open](file:///etc/passwd) and ![pixel](https://example.test/tracker)",
        "<https://example.test> www.example.test user@example.test",
        "[nested **bold** label](https://example.test/a_(b))",
        "![ref][img]\n\n[img]: https://example.test/pixel.png",
        "[file][r]\n\n[r]: ssh://host/private/file",
        "<img src=\"https://example.test/x\"><a href=\"file:///tmp/a\">file</a>",
        "[encoded](javascript&#58;alert(1)) and <https://example.test/?x=1&y=2>",
        "Escaped \\[label](https://example.test) and **https://example.test**",
        "[outer ![inner](https://example.test/pixel)](https://example.test/page)",
    ];
    for source in examples {
        for block in markdown_blocks(source) {
            let reparsed =
                markdown::to_mdast(&block.markdown, &markdown::ParseOptions::gfm()).unwrap();
            assert_inert(&reparsed);
        }
        for boundary in source
            .char_indices()
            .map(|(offset, _)| offset)
            .chain(std::iter::once(source.len()))
        {
            for block in markdown_blocks(&source[..boundary]) {
                let reparsed =
                    markdown::to_mdast(&block.markdown, &markdown::ParseOptions::gfm()).unwrap();
                assert_inert(&reparsed);
            }
        }
    }
}
#[test]
fn fenced_code_keeps_exact_copy_bytes_and_file_previews_cannot_break_out() {
    let code = "let x = \"世界\";\n// https://example.test is literal";
    let blocks = markdown_blocks(&format!(
        "## Example\n\n```rust\n{code}\n```\n\n- **one**\n- `two`"
    ));
    assert_eq!(blocks[1].code.as_deref(), Some(code));
    let hostile =
        "```\n![remote](https://example.test/pixel)\n````\n<img src=\"https://example.test\">";
    let preview =
        markdown::to_mdast(&literal_code(hostile), &markdown::ParseOptions::gfm()).unwrap();
    assert_inert(&preview);
    let children = preview.children().unwrap();
    assert_eq!(children.len(), 1);
    assert!(matches!(&children[0],markdown::mdast::Node::Code(code) if code.value==hostile));
}
