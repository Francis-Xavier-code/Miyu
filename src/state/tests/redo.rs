//! 重做与它的回滚。

use crate::state::*;
use super::shared::*;


#[test]
fn initial_prompt_redo_reuses_the_turn_with_a_new_revision() {
    let (_temp, store) = test_store();
    store
        .start_turn_with_display("t1", "original", "original", 999999, None)
        .unwrap();
    store.complete_turn("t1", "old answer", None).unwrap();

    let candidate = store.redo_candidate().unwrap().unwrap();
    assert_eq!(candidate.input_kind, RedoInputKind::Initial);
    let redo = store
        .begin_redo(
            "t1",
            "t1",
            RedoInputKind::Initial,
            candidate.revision,
            "edited internal",
            "edited",
            std::process::id(),
        )
        .unwrap();
    assert_eq!(redo.revision, 1);
    assert!(redo.checkpoint.is_none());

    let turn = store.load_turns().unwrap().remove(0);
    assert_eq!(turn.revision, 1);
    assert_eq!(turn.status, TurnStatus::Running);
    assert_eq!(turn.user_content, "edited internal");
    assert_eq!(turn.display_content, "edited");
    assert!(store
        .begin_redo(
            "t1",
            "t1",
            RedoInputKind::Initial,
            candidate.revision,
            "stale",
            "stale",
            std::process::id(),
        )
        .is_err());

    store
        .complete_turn_revision_with_usage_and_model(
            "t1",
            1,
            "new answer",
            None,
            None,
            None,
            TurnTokens::default(),
            false,
        )
        .unwrap();
    assert_eq!(
        store.load_turns().unwrap()[0].assistant_content,
        "new answer"
    );
}

#[test]
fn followup_redo_restores_the_last_batch_checkpoint() {
    let (_temp, store) = test_store();
    store
        .start_turn("t1", "initial", std::process::id())
        .unwrap();
    store
        .enqueue_prompt("q1", "followup", "followup", &[])
        .unwrap();
    let checkpoint = TurnRedoCheckpointPayload {
        replay_messages: vec![crate::llm::ChatMessage::plain("assistant", "prefix answer")],
        prefix_tool_reports: vec!["prefix report".to_string()],
        tool_rounds: 1,
        question_rounds: 0,
        loaded_items: Vec::new(),
        prefix_question_count: 0,
        prefix_image_asset_ids: Vec::new(),
        prefix_artifact_asset_ids: Vec::new(),
    };
    store
        .consume_queued_prompts_with_checkpoint(
            "t1",
            &[("q1".to_string(), "followup".to_string())],
            Some("prefix answer"),
            None,
            None,
            None,
            checkpoint,
        )
        .unwrap();
    store.complete_turn("t1", "old final", None).unwrap();

    let candidate = store.redo_candidate().unwrap().unwrap();
    assert_eq!(candidate.input_kind, RedoInputKind::Followup);
    assert_eq!(candidate.input_id, "q1");
    let redo = store
        .begin_redo(
            "t1",
            "q1",
            RedoInputKind::Followup,
            candidate.revision,
            "edited followup",
            "edited followup",
            std::process::id(),
        )
        .unwrap();
    let redo_revision = redo.revision;
    let checkpoint = redo.checkpoint.unwrap();
    assert_eq!(checkpoint.replay_messages.len(), 1);
    assert_eq!(checkpoint.prefix_tool_reports, vec!["prefix report"]);
    let turn = store.load_turns().unwrap().remove(0);
    assert_eq!(turn.followups[0].content, "edited followup");
    assert_eq!(turn.tool_reports, vec!["prefix report"]);
    store
        .enqueue_prompt("q2", "new during redo", "new during redo", &[])
        .unwrap();
    store
        .consume_queued_prompts_with_checkpoint(
            "t1",
            &[("q2".to_string(), "new during redo".to_string())],
            None,
            None,
            None,
            None,
            TurnRedoCheckpointPayload {
                replay_messages: Vec::new(),
                prefix_tool_reports: Vec::new(),
                tool_rounds: 0,
                question_rounds: 0,
                loaded_items: Vec::new(),
                prefix_question_count: 0,
                prefix_image_asset_ids: Vec::new(),
                prefix_artifact_asset_ids: Vec::new(),
            },
        )
        .unwrap();
    store.interrupt_turn_revision("t1", redo_revision).unwrap();
    let restored = store.load_turns().unwrap().remove(0);
    assert_eq!(restored.revision, 0);
    assert_eq!(restored.status, TurnStatus::Completed);
    assert_eq!(restored.assistant_content, "old final");
    assert_eq!(restored.followups[0].content, "followup");
    assert_eq!(restored.followups.len(), 1);
    assert_eq!(store.redo_candidate().unwrap().unwrap().input_id, "q1");
}

#[test]
fn cancelled_initial_redo_restores_the_previous_turn() {
    let (_temp, store) = test_store();
    store
        .start_turn_with_display("t1", "internal", "visible", 999999, None)
        .unwrap();
    store
        .complete_turn("t1", "old answer", Some("old reasoning"))
        .unwrap();
    let candidate = store.redo_candidate().unwrap().unwrap();
    let redo = store
        .begin_redo(
            "t1",
            "t1",
            RedoInputKind::Initial,
            candidate.revision,
            "edited internal",
            "edited visible",
            std::process::id(),
        )
        .unwrap();

    store.interrupt_turn_revision("t1", redo.revision).unwrap();
    let restored = store.load_turns().unwrap().remove(0);
    assert_eq!(restored.revision, 0);
    assert_eq!(restored.status, TurnStatus::Completed);
    assert_eq!(restored.user_content, "internal");
    assert_eq!(restored.display_content, "visible");
    assert_eq!(restored.assistant_content, "old answer");
    assert_eq!(
        restored.assistant_reasoning.as_deref(),
        Some("old reasoning")
    );
}

#[test]
fn cancelled_redo_restores_artifact_versions() {
    let (temp, store) = test_store();
    let artifact_dir = temp.path().join("data/artifacts/default");
    std::fs::create_dir_all(&artifact_dir).unwrap();
    let path = artifact_dir.join("report.md");
    std::fs::write(&path, "old artifact").unwrap();
    store
        .start_turn("t1", "create report", std::process::id())
        .unwrap();
    let old = store
        .save_artifact_asset("t1", Some("tool-old"), &path, "Report")
        .unwrap();
    store.complete_turn("t1", "old answer", None).unwrap();

    let candidate = store.redo_candidate().unwrap().unwrap();
    let redo = store
        .begin_redo(
            "t1",
            "t1",
            RedoInputKind::Initial,
            candidate.revision,
            "redo report",
            "redo report",
            std::process::id(),
        )
        .unwrap();
    assert!(store.load_artifact_assets().unwrap().is_empty());
    std::fs::write(&path, "new artifact").unwrap();
    store
        .save_artifact_asset("t1", Some("tool-new"), &path, "Report")
        .unwrap();
    store.interrupt_turn_revision("t1", redo.revision).unwrap();

    let restored = store.load_artifact_asset(&old.asset_id).unwrap().unwrap();
    assert_eq!(restored.asset.tool_id.as_deref(), Some("tool-old"));
    assert_eq!(restored.bytes, b"old artifact");
    assert_eq!(std::fs::read_to_string(path).unwrap(), "old artifact");
}
