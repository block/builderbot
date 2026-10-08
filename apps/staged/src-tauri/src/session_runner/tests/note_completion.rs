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
        let turn = store
            .add_session_message(&session.id, MessageRole::User, "Write a note")
            .unwrap();
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
            turn,
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
            0,
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

#[test]
fn rewriting_turn_without_a_next_steps_block_keeps_stored_steps() {
    for project_note in [false, true] {
        let store = Arc::new(Store::in_memory().unwrap());
        let session = Session::new_running("write a note", std::path::Path::new("."));
        store.create_session(&session).unwrap();
        let project = Project::new("test/next-steps");
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
        let steps = |store: &Store| {
            if project_note {
                let note = store.get_project_note(&id).unwrap().unwrap();
                (
                    note.content,
                    note.suggested_next_commit_step,
                    note.suggested_next_note_step,
                )
            } else {
                let note = store.get_note(&id).unwrap().unwrap();
                (
                    note.content,
                    note.suggested_next_commit_step,
                    note.suggested_next_note_step,
                )
            }
        };
        let run_turn = |prompt: &str, response: &str| {
            let turn = store
                .add_session_message(&session.id, MessageRole::User, prompt)
                .unwrap();
            store
                .add_session_message(&session.id, MessageRole::Assistant, response)
                .unwrap();
            run_post_completion_hooks(
                &session.id,
                std::path::Path::new("."),
                None,
                None,
                None,
                &store,
                turn,
            );
            steps(&store)
        };

        let (_, commit, note) = run_turn(
            "Write a note",
            "---\n# Plan\n\nFirst.\n```suggested-next-steps\n{\"suggestedNextCommitStep\":\"Implement\",\"suggestedNextNoteStep\":\"Explore\"}\n```",
        );
        assert_eq!(commit.as_deref(), Some("Implement"));
        assert_eq!(note.as_deref(), Some("Explore"));

        // The agent rewrote the note but did not comply with the block request.
        let (content, commit, note) = run_turn("Update the note", "---\n# Plan\n\nSecond.");
        assert!(content.contains("Second."));
        assert_eq!(commit.as_deref(), Some("Implement"));
        assert_eq!(note.as_deref(), Some("Explore"));

        // A later block replaces both, including clearing one of them.
        let (_, commit, note) = run_turn(
            "Update again",
            "---\n# Plan\n\nThird.\n```suggested-next-steps\n{\"suggestedNextCommitStep\":\"Ship\"}\n```",
        );
        assert_eq!(commit.as_deref(), Some("Ship"));
        assert_eq!(note, None);
    }
}

#[test]
fn follow_up_turn_without_a_note_keeps_saved_media_even_when_sources_are_gone() {
    use crate::note_media::{
        refs::media_ids,
        tests::{Fixture, PNG},
    };
    let f = Fixture::new();
    let shot = f.dir.path().join("shot.png");
    std::fs::write(&shot, PNG).unwrap();
    let note = Note::new(&f.branch.id, "", "").with_session(&f.session.id);
    f.store.create_note(&note).unwrap();
    let run_turn = |prompt: &str, response: &str| {
        let turn = f
            .store
            .add_session_message(&f.session.id, MessageRole::User, prompt)
            .unwrap();
        f.store
            .add_session_message(&f.session.id, MessageRole::Assistant, response)
            .unwrap();
        run_post_completion_hooks(
            &f.session.id,
            f.dir.path(),
            None,
            None,
            None,
            &f.store,
            turn,
        );
        f.store.get_note(&note.id).unwrap().unwrap()
    };

    let saved = run_turn(
        "Write a note",
        &format!("Intro\n\n---\n# Shot\n\n![Shot]({})", shot.display()),
    );
    assert!(saved.content.contains("staged-media://"));
    let id = media_ids(&saved.content).into_iter().next().unwrap();
    let image = f.store.get_image(&id).unwrap().unwrap();
    let file = crate::store::images::image_file_path(&image.project_id, &image.id, &image.filename)
        .unwrap();
    assert!(file.exists());

    std::fs::remove_file(&shot).unwrap();
    let followed_up = run_turn("One more question", "Here's the answer.");
    assert_eq!(followed_up.content, saved.content);
    assert!(f.store.get_image(&id).unwrap().is_some());
    assert!(file.exists());
    assert!(followed_up.completed_at.is_some());

    // A rewrite built from chat history repeats the original path, not the
    // stored reference the agent never saw.
    let rewritten = run_turn(
        "Please update the note to reflect the latest chat",
        &format!("---\n# Shot\n\nRewritten.\n\n![Shot]({})", shot.display()),
    );
    assert!(rewritten.content.contains("Rewritten."));
    assert!(rewritten
        .content
        .contains(&format!("![Shot](staged-media://{id}.png)")));
    assert!(!rewritten.content.contains("unavailable"));
    assert!(f.store.get_image(&id).unwrap().is_some());
    assert!(file.exists());

    let amended = run_turn("Drop the screenshot", "---\n# Shot\n\nNo image now");
    assert!(amended.content.contains("No image now"));
    assert!(f.store.get_image(&id).unwrap().is_none());
    assert!(!file.exists());
}
