pub mod database {
    pub mod repositories {
        pub mod setting {
            pub const RK_SUMMARY_ENDPOINT: &str = "http://127.0.0.1:8040/v1";
            pub const RK_SUMMARY_MODEL: &str = "qwen3-4b";
        }
    }
}

#[path = "../../../frontend/src-tauri/src/audio/transcription/correction/mod.rs"]
pub mod correction;

#[path = "../../../frontend/src-tauri/src/audio/transcript_sink.rs"]
pub mod transcript_sink;

#[cfg(test)]
mod tests {
    use super::correction::{CorrectionConfig, Corrector, Segment};
    use std::{path::PathBuf, time::Duration};

    fn folder(name: &str) -> PathBuf {
        let folder = std::env::temp_dir().join(format!("rewrite-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        folder
    }

    #[tokio::test]
    async fn unavailable_model_keeps_original_and_disables_this_session() {
        let folder = folder("unavailable");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}/v1", listener.local_addr().unwrap());
        drop(listener);
        let config = CorrectionConfig::local_for_test(endpoint, Duration::from_millis(100));
        let mut corrector = Corrector::new("session-a".into(), Some(folder.clone()), config);
        let first = corrector.correct(Segment::new(0, "预算三万元，不能承诺。", 0.0, 2.0, "10:00:00"), 0).await;
        assert_eq!(first, "预算三万元，不能承诺。");
        assert!(!corrector.is_enabled());
        let second = corrector.correct(Segment::new(1, "下一段原文", 2.0, 4.0, "10:00:02"), 0).await;
        assert_eq!(second, "下一段原文");
        let journal = std::fs::read_to_string(folder.join("transcript_corrections.jsonl")).unwrap();
        let records: Vec<serde_json::Value> = journal.lines().map(|line| serde_json::from_str(line).unwrap()).collect();
        assert_eq!(records.len(), 4);
        assert_eq!(records[0]["original_text"], "预算三万元，不能承诺。");
        assert_eq!(records[0]["sequence_id"], 0);
        assert_eq!(records[3]["decision"], "disabled");
        std::fs::remove_dir_all(folder).unwrap();
    }

    async fn model_response(content: &str, delay: Duration) -> (String, tokio::task::JoinHandle<serde_json::Value>) {
        let body = serde_json::json!({"model": "qwen3-4b", "choices": [{"message": {"role": "assistant", "content": content}, "finish_reason": "stop"}], "usage": {"prompt_tokens": 0, "completion_tokens": 0, "total_tokens": 0}}).to_string();
        model_http_response(body, 200, delay).await
    }

    async fn model_http_response(body: String, status: u16, delay: Duration) -> (String, tokio::task::JoinHandle<serde_json::Value>) {
        model_http_response_after_request(body, status, delay, || {}).await
    }

    async fn model_http_response_after_request(
        body: String, status: u16, delay: Duration, after_request: impl FnOnce() + Send + 'static,
    ) -> (String, tokio::task::JoinHandle<serde_json::Value>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/v1", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut bytes = Vec::new();
            let (header_end, body_len) = loop {
                let mut buffer = [0; 2048];
                let n = stream.read(&mut buffer).await.unwrap();
                assert!(n > 0);
                bytes.extend_from_slice(&buffer[..n]);
                if let Some(end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&bytes[..end]);
                    let length = headers.lines().find_map(|line| {
                        line.to_ascii_lowercase().strip_prefix("content-length:")
                            .and_then(|value| value.trim().parse::<usize>().ok())
                    }).unwrap();
                    break (end + 4, length);
                }
            };
            while bytes.len() < header_end + body_len {
                let mut buffer = [0; 2048];
                let n = stream.read(&mut buffer).await.unwrap();
                assert!(n > 0);
                bytes.extend_from_slice(&buffer[..n]);
            }
            let request = serde_json::from_slice(&bytes[header_end..header_end + body_len]).unwrap();
            after_request();
            tokio::time::sleep(delay).await;
            let reply = format!("HTTP/1.1 {status} Reply\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body);
            let _ = stream.write_all(reply.as_bytes()).await;
            request
        });
        (url, task)
    }

    #[tokio::test]
    async fn accepted_correction_has_original_and_decision_journal() {
        let folder = folder("accepted");
        let (url, task) = model_response(r#"{"corrected_text":"今天的会议纪要已经发给大家了。"}"#, Duration::ZERO).await;
        let mut corrector = Corrector::new("accepted-session".into(), Some(folder.clone()),
            CorrectionConfig::local_for_test(url, Duration::from_millis(1500)));
        let text = corrector.correct(Segment::new(0, "今天的会议记要已经发给大家了。", 1.0, 3.0, "10:00:00"), 0).await;
        assert_eq!(text, "今天的会议纪要已经发给大家了。");
        let request = task.await.unwrap();
        assert_eq!(request["model"], "qwen3-4b");
        assert!(request.get("response_format").is_none());
        let records: Vec<serde_json::Value> = std::fs::read_to_string(folder.join("transcript_corrections.jsonl"))
            .unwrap().lines().map(|line| serde_json::from_str(line).unwrap()).collect();
        assert_eq!(records[0]["original_text"], "今天的会议记要已经发给大家了。");
        assert_eq!(records[0]["audio_start_time"], 1.0);
        assert_eq!(records[1]["decision"], "changed");
        assert_eq!(records[1]["corrected_text"], text);
        std::fs::remove_dir_all(folder).unwrap();
    }

    #[tokio::test]
    async fn unsafe_model_output_keeps_original() {
        let folder = folder("unsafe");
        let (url, task) = model_response(r#"{"corrected_text":"预算五万元，可以承诺。"}"#, Duration::ZERO).await;
        let mut corrector = Corrector::new("unsafe-session".into(), Some(folder.clone()),
            CorrectionConfig::local_for_test(url, Duration::from_millis(1500)));
        assert_eq!(corrector.correct(Segment::new(0, "预算三万元，不能承诺。", 0.0, 2.0, "10:00:00"), 0).await,
            "预算三万元，不能承诺。");
        task.await.unwrap();
        assert!(corrector.is_enabled());
        std::fs::remove_dir_all(folder).unwrap();
    }

    #[tokio::test]
    async fn timeout_disables_further_requests() {
        let folder = folder("timeout");
        let (url, task) = model_response(r#"{"corrected_text":"下一段。"}"#, Duration::from_millis(200)).await;
        let mut corrector = Corrector::new("timeout-session".into(), Some(folder.clone()),
            CorrectionConfig::local_for_test(url, Duration::from_millis(25)));
        assert_eq!(corrector.correct(Segment::new(0, "下一段", 0.0, 2.0, "10:00:00"), 0).await, "下一段");
        assert!(!corrector.is_enabled());
        assert_eq!(corrector.correct(Segment::new(1, "再下一段", 2.0, 4.0, "10:00:02"), 0).await, "再下一段");
        task.await.unwrap();
        std::fs::remove_dir_all(folder).unwrap();
    }

    #[tokio::test]
    async fn archive_failure_never_calls_model_or_replaces_original() {
        let folder = folder("archive-failure");
        std::fs::create_dir(folder.join("transcript_corrections.jsonl")).unwrap();
        let mut corrector = Corrector::new("broken-archive".into(), Some(folder.clone()),
            CorrectionConfig::local_for_test("http://127.0.0.1:9/v1".into(), Duration::from_millis(25)));
        assert_eq!(corrector.correct(Segment::new(0, "原文", 0.0, 2.0, "10:00:00"), 0).await, "原文");
        assert!(!corrector.is_enabled());
        std::fs::remove_dir_all(folder).unwrap();
    }

    #[tokio::test]
    async fn backlog_preserves_original_without_calling_model() {
        let folder = folder("backlog");
        let mut corrector = Corrector::new("backlog-session".into(), Some(folder.clone()),
            CorrectionConfig::local_for_test("http://127.0.0.1:9/v1".into(), Duration::from_millis(25)));
        assert_eq!(corrector.correct(Segment::new(0, "会议记要", 0.0, 2.0, "10:00:00"), 1).await, "会议记要");
        assert!(!corrector.is_enabled());
        let journal = std::fs::read_to_string(folder.join("transcript_corrections.jsonl")).unwrap();
        assert_eq!(serde_json::from_str::<serde_json::Value>(journal.lines().nth(1).unwrap()).unwrap()["reason"], "asr_backlog");
        std::fs::remove_dir_all(folder).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn decision_archive_failure_keeps_original_and_stops_appending() {
        // Limit file size only in a child test process; never affect parallel tests.
        const CHILD: &str = "MEETINGBOX_ARCHIVE_FAILURE_CHILD";
        if std::env::var_os(CHILD).is_none() {
            let status = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "tests::decision_archive_failure_keeps_original_and_stops_appending", "--nocapture"])
                .env(CHILD, "1").status().unwrap();
            assert!(status.success());
            return;
        }
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            let folder = folder("decision-failure");
            let journal_path = folder.join("transcript_corrections.jsonl");
            let mut previous = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
            assert_eq!(unsafe { libc::getrlimit(libc::RLIMIT_FSIZE, &mut previous) }, 0);
            let path = journal_path.clone();
            let hard_limit = previous.rlim_max;
            let response = serde_json::json!({"choices": [{"message": {"content": "{\"corrected_text\":\"今天的会议纪要已经发给大家了。\"}"}}]}).to_string();
            let (url, task) = model_http_response_after_request(response, 200, Duration::ZERO, move || {
                // The request proves revision 0 is durable. Allow a partial decision line,
                // then force the actual production file write to fail with EFBIG.
                let original_length = std::fs::metadata(&path).unwrap().len();
                let limit = libc::rlimit { rlim_cur: (original_length + 10) as libc::rlim_t, rlim_max: hard_limit };
                unsafe { libc::signal(libc::SIGXFSZ, libc::SIG_IGN); }
                assert_eq!(unsafe { libc::setrlimit(libc::RLIMIT_FSIZE, &limit) }, 0);
            }).await;
            let mut corrector = Corrector::new("decision-failure".into(), Some(folder.clone()),
                CorrectionConfig::local_for_test(url, Duration::from_millis(1500)));
            let original = "今天的会议记要已经发给大家了。";
            let text = corrector.correct(Segment::new(0, original, 0.0, 2.0, "10:00:00"), 0).await;
            assert_eq!(unsafe { libc::setrlimit(libc::RLIMIT_FSIZE, &previous) }, 0);
            task.await.unwrap();
            assert_eq!(text, original);
            assert!(!corrector.is_enabled());
            let bytes = std::fs::read(&journal_path).unwrap();
            let journal = String::from_utf8_lossy(&bytes);
            let record: serde_json::Value = serde_json::from_str(journal.lines().next().unwrap()).unwrap();
            assert_eq!(record["revision"], 0);
            assert_eq!(record["original_text"], original);
            assert_eq!(corrector.correct(Segment::new(1, "下一段", 2.0, 4.0, "10:00:02"), 0).await, "下一段");
            assert_eq!(std::fs::read(&journal_path).unwrap(), bytes);
            std::fs::remove_dir_all(folder).unwrap();
        });
    }

    #[tokio::test]
    async fn invalid_http_outputs_fail_closed_and_keep_session_available() {
        let cases = [
            ("malformed", "not json".into()),
            ("oversized", "x".repeat(65 * 1024)),
            ("truncated", serde_json::json!({"choices": [{"message": {"content": "{\"corrected_text\":\"原文。\"}"}, "finish_reason": "length"}]}).to_string()),
            ("ambiguous", serde_json::json!({"choices": [
                {"message": {"content": "{\"corrected_text\":\"原文。\"}"}},
                {"message": {"content": "{\"corrected_text\":\"原文！\"}"}}
            ]}).to_string()),
        ];
        for (name, body) in cases {
            let folder = folder(name);
            let (url, task) = model_http_response(body, 200, Duration::ZERO).await;
            let mut corrector = Corrector::new(name.into(), Some(folder.clone()),
                CorrectionConfig::local_for_test(url, Duration::from_millis(1500)));
            assert_eq!(corrector.correct(Segment::new(0, "原文", 0.0, 2.0, "10:00:00"), 0).await, "原文");
            task.await.unwrap();
            assert!(corrector.is_enabled());
            let records: Vec<serde_json::Value> = std::fs::read_to_string(folder.join("transcript_corrections.jsonl"))
                .unwrap().lines().map(|line| serde_json::from_str(line).unwrap()).collect();
            assert_eq!(records[1]["decision"], "rejected");
            assert_eq!(records[1]["corrected_text"], "原文");
            std::fs::remove_dir_all(folder).unwrap();
        }
    }

    #[tokio::test]
    async fn service_503_disables_only_the_current_session() {
        let folder = folder("503");
        let (url, task) = model_http_response("{}".into(), 503, Duration::ZERO).await;
        let mut first = Corrector::new("first".into(), Some(folder.clone()),
            CorrectionConfig::local_for_test(url, Duration::from_millis(1500)));
        assert_eq!(first.correct(Segment::new(0, "原文", 0.0, 2.0, "10:00:00"), 0).await, "原文");
        task.await.unwrap();
        assert!(!first.is_enabled());
        let (url, task) = model_response(r#"{"corrected_text":"原文。"}"#, Duration::ZERO).await;
        let mut next = Corrector::new("next".into(), Some(folder.clone()),
            CorrectionConfig::local_for_test(url, Duration::from_millis(1500)));
        assert_eq!(next.correct(Segment::new(0, "原文", 0.0, 2.0, "10:00:00"), 0).await, "原文。");
        let request = task.await.unwrap();
        let prompt: serde_json::Value = serde_json::from_str(request["messages"][1]["content"].as_str().unwrap()).unwrap();
        assert_eq!(prompt["context"], serde_json::json!([]));
        let records: Vec<serde_json::Value> = std::fs::read_to_string(folder.join("transcript_corrections.jsonl"))
            .unwrap().lines().map(|line| serde_json::from_str(line).unwrap()).collect();
        assert_eq!(records[0]["recording_session"], "first");
        assert_eq!(records[2]["recording_session"], "next");
        assert_ne!(records[0]["original_reference"], records[2]["original_reference"]);
        std::fs::remove_dir_all(folder).unwrap();
    }

    #[tokio::test]
    async fn request_context_keeps_two_original_segments_and_is_bounded() {
        let folder = folder("context");
        // Oversized segments are archived and remembered without an HTTP request.
        let (url, task) = model_response(r#"{"corrected_text":"原文。"}"#, Duration::ZERO).await;
        let mut corrector = Corrector::new("context-http".into(), Some(folder.clone()),
            CorrectionConfig::local_for_test(url, Duration::from_millis(1500)));
        for (seq, text) in [(0, "甲".repeat(600)), (1, "乙".repeat(600)), (2, "丙".repeat(600))] {
            corrector.correct(Segment::new(seq, &text, 0.0, 2.0, "10:00:00"), 0).await;
        }
        assert_eq!(corrector.correct(Segment::new(3, "原文", 0.0, 2.0, "10:00:00"), 0).await, "原文。");
        let request = task.await.unwrap();
        let prompt: serde_json::Value = serde_json::from_str(request["messages"][1]["content"].as_str().unwrap()).unwrap();
        assert_eq!(prompt["context"], serde_json::json!(["乙".repeat(200), "丙".repeat(200)]));
        assert_eq!(prompt["target"], "原文");
        std::fs::remove_dir_all(folder).unwrap();
    }

    #[tokio::test]
    #[ignore = "requires the box's existing local Qwen3-4B service"]
    async fn box_synthetic_smoke() {
        let folder = folder("box-smoke");
        let mut corrector = Corrector::new("synthetic-smoke".into(), Some(folder.clone()), CorrectionConfig::default());
        for (sequence, input) in [
            "今天的会议记要已经发给大家了。",
            "嗯那个我想先介绍一下项目然后我们再讨论具体的时间安排",
            "这个方按我们下午再讨论",
            "预算是12万元我们不能承诺五月二日交付",
        ].into_iter().enumerate() {
            let text = corrector.correct(Segment::new(sequence as u64, input, sequence as f64 * 2.0,
                sequence as f64 * 2.0 + 2.0, "10:00:00"), 0).await;
            let records: Vec<serde_json::Value> = std::fs::read_to_string(folder.join("transcript_corrections.jsonl"))
                .unwrap().lines().map(|line| serde_json::from_str(line).unwrap()).collect();
            let original = &records[sequence * 2];
            let decision = &records[sequence * 2 + 1];
            println!("synthetic-smoke decision={} reason={} elapsed_ms={} input={} output={}",
                decision["decision"], decision["reason"], decision["elapsed_ms"], input, text);
            assert!(corrector.is_enabled(), "local model unavailable or timeout");
            assert!(decision["decision"] == "changed" || decision["decision"] == "unchanged"
                || decision["decision"] == "rejected");
            assert_eq!(original["original_text"], input);
            assert_eq!(decision["corrected_text"], text);
            if decision["decision"] == "changed" {
                super::correction::validation::validate_correction(input, &text).unwrap();
            }
        }
        std::fs::remove_dir_all(folder).unwrap();
    }

    #[test]
    fn independent_session_sinks_do_not_leak_tail_between_meetings() {
        use super::transcript_sink::{TranscriptSegment, TranscriptSink};
        let a = folder("sink-a");
        let b = folder("sink-b");
        let first = TranscriptSink::new(Some(a.clone()));
        let second = TranscriptSink::new(Some(b.clone()));
        let segment = TranscriptSegment { id: "seg_0".into(), text: "甲会议".into(),
            audio_start_time: 0.0, audio_end_time: 2.0, duration: 2.0,
            display_time: "10:00:00".into(), confidence: 0.9, sequence_id: 0 };
        first.upsert(segment.clone()).unwrap();
        second.upsert(TranscriptSegment { text: "乙会议".into(), ..segment }).unwrap();
        first.flush().unwrap();
        second.flush().unwrap();
        assert_eq!(first.snapshot()[0].text, "甲会议");
        assert_eq!(second.snapshot()[0].text, "乙会议");
        std::fs::remove_dir_all(a).unwrap();
        std::fs::remove_dir_all(b).unwrap();
    }

    #[test]
    fn session_sink_survives_owner_drop_and_flushes_tail() {
        use super::transcript_sink::{TranscriptSegment, TranscriptSink};
        let folder = folder("sink");
        let owner = TranscriptSink::new(Some(folder.clone()));
        let listener_sink = owner.clone();
        drop(owner);
        for seq in 0..3 {
            listener_sink.upsert(TranscriptSegment { id: format!("seg_{seq}"), text: format!("段落{seq}"),
                audio_start_time: seq as f64 * 2.0, audio_end_time: seq as f64 * 2.0 + 2.0,
                duration: 2.0, display_time: "10:00:00".into(), confidence: 0.9, sequence_id: seq }).unwrap();
        }
        listener_sink.flush().unwrap();
        let body: serde_json::Value = serde_json::from_slice(&std::fs::read(folder.join("transcripts.json")).unwrap()).unwrap();
        assert_eq!(body["total_segments"], 3);
        assert_eq!(body["segments"][2]["text"], "段落2");
        std::fs::remove_dir_all(folder).unwrap();
    }
}
