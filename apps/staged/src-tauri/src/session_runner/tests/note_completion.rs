use super::*;
use crate::store::{Branch, Note, Project, ProjectNote, Session};
use acp_client::MessageWriter as _;

#[tokio::test]
async fn finished_notes_preserve_diagram_repair_and_completion_hooks() {
    for project_note in [false, true] {
        let store = Arc::new(Store::in_memory().unwrap());
        let session = Session::new_running("write a note", std::path::Path::new("."));
        store.create_session(&session).unwrap();
        let project = Project::new("test/notes");
        store.create_project(&project).unwrap();
        let id = if project_note {
            let note = ProjectNote::new(&project.id, "", "").with_session(&session.id);
            store.create_project_note(&note).unwrap();
            note.id
        } else {
            let branch = Branch::new(&project.id, "notes", "main");
            store.create_branch(&branch).unwrap();
            let note = Note::new(&branch.id, "", "").with_session(&session.id);
            store.create_note(&note).unwrap();
            note.id
        };
        let writer = MessageWriter::new(session.id.clone(), store.clone());
        writer
            .append_text("---\n# Design\n```pikchr\nbox \"unterminated\n```")
            .await;
        assert!(writer.should_finish_after_response().await);
        writer.finalize().await;
        let error = match validate_latest_session_pikchr(&store, &session.id).unwrap() {
            LatestAssistantPikchrValidation::Invalid(error) => error,
            other => panic!("expected diagram repair, got {other:?}"),
        };
        let correction = build_pikchr_correction_prompt(&error, true);
        store
            .add_session_message(&session.id, MessageRole::User, &correction)
            .unwrap();
        let repaired = "---\n# Design\nFull body.\n```pikchr\nbox \"fixed\"\n```\n```suggested-next-steps\n{\"suggestedNextCommitStep\":\"Implement\",\"suggestedNextNoteStep\":\"Explore\"}\n```";
        let writer = MessageWriter::new(session.id.clone(), store.clone());
        assert!(!writer.should_finish_after_response().await);
        writer.append_text(repaired).await;
        assert!(writer.should_finish_after_response().await);
        writer.finalize().await;
        assert_eq!(
            validate_latest_session_pikchr(&store, &session.id).unwrap(),
            LatestAssistantPikchrValidation::Valid
        );
        run_post_completion_hooks(
            &session.id,
            std::path::Path::new("."),
            None,
            None,
            None,
            &store,
        );
        let (title, content, completed, commit_step, note_step) = if project_note {
            let note = store.get_project_note(&id).unwrap().unwrap();
            (
                note.title,
                note.content,
                note.completed_at,
                note.suggested_next_commit_step,
                note.suggested_next_note_step,
            )
        } else {
            let note = store.get_note(&id).unwrap().unwrap();
            (
                note.title,
                note.content,
                note.completed_at,
                note.suggested_next_commit_step,
                note.suggested_next_note_step,
            )
        };
        assert_eq!(title, "Design");
        assert!(content.contains("Full body."));
        assert!(content.contains("box \"fixed\""));
        assert!(completed.is_some());
        assert_eq!(commit_step.as_deref(), Some("Implement"));
        assert_eq!(note_step.as_deref(), Some("Explore"));
    }
}

#[test]
fn note_finish_is_successful_and_keeps_queued_follow_ups_enabled() {
    let settle = Some(acp_client::SessionSettleReason::ResponseComplete);
    let reason = completed_turn_completion_reason(settle);
    assert_eq!(reason, CompletionReason::TurnComplete);
    assert!(completed_turn_survives_late_cancel(settle));
    let completed = terminal_state_completed_successfully("completed", &reason);
    assert!(queued_follow_up_should_start(completed, false));
    assert!(should_validate_pikchr_after_turn(
        &Ok(AgentRunOutcome::Completed),
        false
    ));
    // An actual Stop still suppresses further work, even after a note finished.
    assert!(!queued_follow_up_should_start(completed, true));
}

#[test]
fn completion_ingests_media_for_branch_and_project_notes() {
    use crate::note_media::{refs::media_ids, tests::Fixture};
    for project_note in [false, true] {
        let fixture = Fixture::new();
        let image_path = fixture.dir.path().join("shot.png");
        std::fs::write(&image_path, crate::note_media::tests::PNG).unwrap();
        let content = "---\n# Media note\n![Screenshot](shot.png)";
        let id = if project_note {
            let note =
                ProjectNote::new(&fixture.project.id, "", "").with_session(&fixture.session.id);
            fixture.store.create_project_note(&note).unwrap();
            note.id
        } else {
            let note = Note::new(&fixture.branch.id, "", "").with_session(&fixture.session.id);
            fixture.store.create_note(&note).unwrap();
            note.id
        };
        fixture
            .store
            .add_session_message(&fixture.session.id, MessageRole::Assistant, content)
            .unwrap();
        let store = fixture.store.clone();
        run_post_completion_hooks(
            &fixture.session.id,
            fixture.dir.path(),
            None,
            None,
            None,
            &store,
        );
        let note_content = if project_note {
            store.get_project_note(&id).unwrap().unwrap().content
        } else {
            store.get_note(&id).unwrap().unwrap().content
        };
        let ids = media_ids(&note_content);
        assert_eq!(ids.len(), 1);
        let image = store
            .get_image(ids.iter().next().unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(
            image.session_id.as_deref(),
            Some(fixture.session.id.as_str())
        );
        assert_eq!(
            image.branch_id.as_deref(),
            if project_note {
                None
            } else {
                Some(fixture.branch.id.as_str())
            }
        );
        assert_eq!(
            std::fs::read(
                crate::store::images::image_file_path(
                    &image.project_id,
                    &image.id,
                    &image.filename
                )
                .unwrap()
            )
            .unwrap(),
            crate::note_media::tests::PNG
        );
    }
}
