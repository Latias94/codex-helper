use super::*;

use pretty_assertions::assert_eq;

#[test]
fn session_cwd_parent_of_current_dir_matches() {
    let base = std::env::current_dir().expect("cwd");
    let project = base.join("codex_project_parent");
    let child = project.join("subdir");
    let session_cwd = project.to_str().expect("project path utf8").to_string();

    assert!(
        path_matches_current_dir(&session_cwd, &child),
        "session cwd should match when it is a parent of current dir"
    );
}

#[test]
fn session_cwd_child_of_current_dir_matches() {
    let base = std::env::current_dir().expect("cwd");
    let project = base.join("codex_project_child");
    let child = project.join("subdir");
    let session_cwd = child.to_str().expect("child path utf8").to_string();

    assert!(
        path_matches_current_dir(&session_cwd, &project),
        "session cwd should match when it is a child of current dir"
    );
}

#[test]
fn unrelated_paths_do_not_match() {
    let base = std::env::current_dir().expect("cwd");
    let project = base.join("codex_project_main");
    let other = base.join("other_project_main");
    let session_cwd = other.to_str().expect("other path utf8").to_string();

    assert!(
        !path_matches_current_dir(&session_cwd, &project),
        "unrelated paths should not match"
    );
}

#[test]
fn parse_rollout_filename_splits_uuid_correctly() {
    let name = "rollout-2025-12-20T16-01-02-550e8400-e29b-41d4-a716-446655440000.jsonl";
    let (ts, uuid) = parse_timestamp_and_uuid(name).expect("should parse");
    assert_eq!(ts, "2025-12-20T16-01-02");
    assert_eq!(uuid, "550e8400-e29b-41d4-a716-446655440000");
}

#[tokio::test]
async fn summarize_session_tracks_rounds_and_last_response() {
    let dir = std::env::temp_dir().join(format!("codex-helper-test-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).expect("create tmp dir");
    let path = dir.join("rollout-2025-12-22T00-00-00-00000000-0000-0000-0000-000000000000.jsonl");
    let cwd = dir.join("project");
    std::fs::create_dir_all(&cwd).expect("create cwd dir");
    let cwd_str = cwd.to_str().expect("cwd utf8");

    let meta_line = serde_json::json!({
        "timestamp": "2025-12-22T00:00:00.000Z",
        "type": "session_meta",
        "payload": {
            "id": "sid-1",
            "cwd": cwd_str,
            "timestamp": "2025-12-22T00:00:00.000Z"
        }
    })
    .to_string();
    let lines = [
            meta_line,
            r#"{"timestamp":"2025-12-22T00:00:01.000Z","type":"event_msg","payload":{"type":"user_message","message":"hi"}}"#.to_string(),
            r#"{"timestamp":"2025-12-22T00:00:02.000Z","type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"hello"}]}}"#.to_string(),
            r#"{"timestamp":"2025-12-22T00:00:03.000Z","type":"event_msg","payload":{"type":"user_message","message":"next"}}"#.to_string(),
            r#"{"timestamp":"2025-12-22T00:00:04.000Z","type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"ok"}]}}"#.to_string(),
        ]
        .join("\n");
    std::fs::write(&path, lines).expect("write session file");

    let summary = summarize_session_for_current_dir(&path, &cwd)
        .await
        .expect("summarize ok")
        .expect("some summary");

    assert_eq!(
        summary.user_turns, 2,
        "should count user_message events as user turns"
    );
    assert_eq!(
        summary.assistant_turns, 2,
        "should count assistant response_item messages"
    );
    assert_eq!(summary.rounds, 2, "rounds should match assistant turns");
    assert_eq!(
        summary.last_response_at.as_deref(),
        Some("2025-12-22T00:00:04.000Z")
    );
    assert_eq!(
        summary.updated_at.as_deref(),
        Some("2025-12-22T00:00:04.000Z"),
        "updated_at should prefer last_response_at"
    );
}

#[tokio::test]
async fn read_codex_session_transcript_extracts_messages_and_tail() {
    let dir = std::env::temp_dir().join(format!("codex-helper-test-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).expect("create tmp dir");
    let path = dir.join("rollout-2025-12-22T00-00-00-00000000-0000-0000-0000-000000000000.jsonl");

    let meta_line = serde_json::json!({
        "timestamp": "2025-12-22T00:00:00.000Z",
        "type": "session_meta",
        "payload": {
            "id": "00000000-0000-0000-0000-000000000000",
            "cwd": "G:/code/project",
            "timestamp": "2025-12-22T00:00:00.000Z"
        }
    })
    .to_string();

    let lines = [
            meta_line,
            r#"{"timestamp":"2025-12-22T00:00:01.000Z","type":"event_msg","payload":{"type":"user_message","message":"hi"}}"#.to_string(),
            r#"{"timestamp":"2025-12-22T00:00:02.000Z","type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"hello"}]}}"#.to_string(),
            r#"{"timestamp":"2025-12-22T00:00:03.000Z","type":"event_msg","payload":{"type":"user_message","message":"next"}}"#.to_string(),
            r#"{"timestamp":"2025-12-22T00:00:04.000Z","type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"ok"}]}}"#.to_string(),
        ]
        .join("\n");
    std::fs::write(&path, lines).expect("write session file");

    let all = read_codex_session_transcript(&path, None)
        .await
        .expect("read transcript ok");
    assert_eq!(all.len(), 4);
    assert_eq!(all[0].role, "User");
    assert_eq!(all[0].text, "hi");
    assert_eq!(all[1].role, "Assistant");
    assert_eq!(all[1].text, "hello");

    let tail = read_codex_session_transcript(&path, Some(2))
        .await
        .expect("read tail ok");
    assert_eq!(tail.len(), 2);
    assert_eq!(tail[0].text, "next");
    assert_eq!(tail[1].text, "ok");

    assert!(
        codex_session_transcript_tail_contains_query(&path, "HELLO", 3)
            .await
            .expect("search ok"),
        "should match case-insensitively within tail"
    );
    assert!(
        !codex_session_transcript_tail_contains_query(&path, "missing", 10)
            .await
            .expect("search ok"),
        "should return false when not found"
    );
}

#[tokio::test]
async fn recent_sessions_filters_by_mtime_and_prefers_meta_id() {
    let tmp = std::env::temp_dir().join(format!("codex-helper-test-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&tmp).expect("create tmp dir");

    let sessions = tmp.join("sessions").join("2026").join("02").join("01");
    std::fs::create_dir_all(&sessions).expect("create sessions dir");

    let file1 =
        sessions.join("rollout-2026-02-01T00-00-00-11111111-1111-1111-1111-111111111111.jsonl");
    let file2 =
        sessions.join("rollout-2026-02-01T00-00-01-22222222-2222-2222-2222-222222222222.jsonl");

    let meta1 = serde_json::json!({
        "timestamp": "2026-02-01T00:00:00.000Z",
        "type": "session_meta",
        "payload": {
            "id": "sid-old",
            "cwd": "G:/code/old",
            "timestamp": "2026-02-01T00:00:00.000Z"
        }
    })
    .to_string();
    std::fs::write(&file1, meta1).expect("write file1");

    std::thread::sleep(std::time::Duration::from_millis(50));

    let meta2 = serde_json::json!({
        "timestamp": "2026-02-01T00:00:01.000Z",
        "type": "session_meta",
        "payload": {
            "id": "sid-new",
            "cwd": "G:/code/new",
            "timestamp": "2026-02-01T00:00:01.000Z"
        }
    })
    .to_string();
    std::fs::write(&file2, meta2).expect("write file2");

    let recent = find_recent_codex_sessions_in_dir(
        &tmp.join("sessions"),
        Duration::from_secs(24 * 3600),
        10,
    )
    .await
    .expect("recent ok");
    assert_eq!(recent.len(), 2);
    assert_eq!(recent[0].id, "sid-new");
    assert_eq!(recent[1].id, "sid-old");

    let none = find_recent_codex_sessions_in_dir(&tmp.join("sessions"), Duration::from_secs(0), 10)
        .await
        .expect("recent ok");
    assert_eq!(none.len(), 0, "since=0 should filter everything out");
}

#[tokio::test]
async fn batch_find_session_files_resolves_uuid_and_meta_ids() {
    let tmp = std::env::temp_dir().join(format!("codex-helper-test-{}", uuid::Uuid::new_v4()));
    let sessions = tmp.join("sessions").join("2026").join("03").join("12");
    std::fs::create_dir_all(&sessions).expect("create sessions dir");

    let file_by_uuid =
        sessions.join("rollout-2026-03-12T00-00-00-11111111-1111-1111-1111-111111111111.jsonl");
    let file_by_meta =
        sessions.join("rollout-2026-03-12T00-00-01-22222222-2222-2222-2222-222222222222.jsonl");

    std::fs::write(
        &file_by_uuid,
        serde_json::json!({
            "timestamp": "2026-03-12T00:00:00.000Z",
            "type": "session_meta",
            "payload": {
                "id": "11111111-1111-1111-1111-111111111111",
                "cwd": "G:/code/by-uuid",
                "timestamp": "2026-03-12T00:00:00.000Z"
            }
        })
        .to_string(),
    )
    .expect("write uuid file");

    std::fs::write(
        &file_by_meta,
        serde_json::json!({
            "timestamp": "2026-03-12T00:00:01.000Z",
            "type": "session_meta",
            "payload": {
                "id": "sid-meta",
                "cwd": "G:/code/by-meta",
                "timestamp": "2026-03-12T00:00:01.000Z"
            }
        })
        .to_string(),
    )
    .expect("write meta file");

    let found = find_codex_session_files_by_ids_in_dir(
        &tmp.join("sessions"),
        &[
            "11111111-1111-1111-1111-111111111111".to_string(),
            "sid-meta".to_string(),
            "missing".to_string(),
        ],
    )
    .await
    .expect("batch find ok");

    assert_eq!(
        found
            .get("11111111-1111-1111-1111-111111111111")
            .expect("uuid match"),
        &file_by_uuid
    );
    assert_eq!(found.get("sid-meta").expect("meta match"), &file_by_meta);
    assert!(
        !found.contains_key("missing"),
        "missing session ids should not be included"
    );
}
