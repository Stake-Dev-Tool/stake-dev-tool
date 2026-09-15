//! CLI wire-contract tests against a local HTTP fixture (no cloud writes).
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::process::Command;
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct Faults {
    get_status: Option<u16>,
    final_get_status: Option<u16>,
    fail_event: Option<u32>,
    hide_created: bool,
}

struct Fixture {
    faults: Arc<Mutex<Faults>>,
    url: String,
    requests: Arc<Mutex<Vec<(String, Value)>>>,
    rounds: Arc<Mutex<Vec<Value>>>,
    stop: Arc<std::sync::atomic::AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl Fixture {
    fn new(rounds: Vec<Value>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let stored = Arc::new(Mutex::new(rounds));
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let faults = Arc::new(Mutex::new(Faults::default()));
        let behavior = faults.clone();
        let original = stored.lock().unwrap().clone();
        let (log, data, done) = (requests.clone(), stored.clone(), stop.clone());
        let thread = std::thread::spawn(move || {
            while !done.load(std::sync::atomic::Ordering::Relaxed) {
                let (mut stream, _) = match listener.accept() {
                    Ok(v) => v,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(std::time::Duration::from_millis(5));
                        continue;
                    }
                    Err(e) => panic!("{e}"),
                };
                stream
                    .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                    .unwrap();
                let mut bytes = Vec::new();
                let end = loop {
                    let mut buf = [0; 4096];
                    let n = stream.read(&mut buf).unwrap();
                    assert!(n > 0);
                    bytes.extend_from_slice(&buf[..n]);
                    if let Some(i) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                        break i + 4;
                    }
                };
                let headers = String::from_utf8(bytes[..end].to_vec())
                    .unwrap()
                    .to_lowercase();
                assert!(headers.contains("authorization: bearer fixture-token\r\n"));
                let length: usize = headers
                    .lines()
                    .find_map(|l| l.strip_prefix("content-length: "))
                    .unwrap_or("0")
                    .parse()
                    .unwrap();
                while bytes.len() < end + length {
                    let mut buf = [0; 4096];
                    let n = stream.read(&mut buf).unwrap();
                    assert!(n > 0);
                    bytes.extend_from_slice(&buf[..n]);
                }
                let line = String::from_utf8(bytes[..end].to_vec())
                    .unwrap()
                    .lines()
                    .next()
                    .unwrap()
                    .to_string();
                let body: Value = if length == 0 {
                    Value::Null
                } else {
                    serde_json::from_slice(&bytes[end..end + length]).unwrap()
                };
                log.lock().unwrap().push((line.clone(), body.clone()));
                let faults = behavior.lock().unwrap();
                let status = if line.starts_with("GET ") {
                    if log
                        .lock()
                        .unwrap()
                        .iter()
                        .any(|(line, _)| line.starts_with("POST "))
                    {
                        faults.final_get_status.or(faults.get_status).unwrap_or(200)
                    } else {
                        faults.get_status.unwrap_or(200)
                    }
                } else if body["eventId"].as_u64() == faults.fail_event.map(u64::from) {
                    500
                } else {
                    200
                };
                let response = if status != 200 {
                    json!({"error":{"code":"fixture_failure","message":"fixture error"}})
                } else if line == "GET /api/workspaces/team/games/demo/revisions?limit=1 HTTP/1.1" {
                    json!({"revisions":[{"number":7}]})
                } else if line.starts_with("POST ") {
                    let mut r = body;
                    let mut rounds = data.lock().unwrap();
                    r["id"] = json!(format!("cloud-{}", rounds.len()));
                    r["createdAt"] = json!(100);
                    r["updatedAt"] = json!(100);
                    rounds.push(r.clone());
                    r
                } else {
                    if faults.hide_created {
                        json!({"rounds": original})
                    } else {
                        json!({"rounds": *data.lock().unwrap()})
                    }
                };
                let body = response.to_string();
                write!(stream, "HTTP/1.1 {status} Fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            }
        });
        Self {
            faults,
            url,
            requests,
            rounds: stored,
            stop,
            thread: Some(thread),
        }
    }
    fn run(&self, input: Value) -> std::process::Output {
        self.run_with_rev(input, Some("7"))
    }
    fn run_with_rev(&self, input: Value, rev: Option<&str>) -> std::process::Output {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rounds.json");
        std::fs::write(&path, input.to_string()).unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_sdt"));
        command
            .args([
                "--server",
                &self.url,
                "--token",
                "fixture-token",
                "push-rounds",
            ])
            .arg(path)
            .args(["--workspace", "team", "--game", "demo", "--json"]);
        if let Some(rev) = rev {
            command.args(["--rev", rev]);
        }
        command.output().unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
        self.thread.take().unwrap().join().unwrap();
    }
}
fn round(event: u32, description: &str) -> Value {
    json!({"gameSlug":"demo", "mode":"base", "eventId":event, "description":description})
}

#[test]
fn auth_and_missing_target_errors_never_write() {
    for (status, exit) in [(401, 2), (403, 2), (404, 3)] {
        let fixture = Fixture::new(vec![]);
        fixture.faults.lock().unwrap().get_status = Some(status);
        let output = fixture.run(json!([round(1, "")]));
        assert_eq!(output.status.code(), Some(exit));
        assert!(fixture.rounds.lock().unwrap().is_empty());
        assert_eq!(fixture.requests.lock().unwrap().len(), 1);
        assert!(output.stdout.is_empty());
    }
}

#[test]
fn readback_transport_error_reports_already_confirmed_writes() {
    let fixture = Fixture::new(vec![]);
    fixture.faults.lock().unwrap().final_get_status = Some(503);
    let output = fixture.run(json!([round(1, "")]));
    assert_eq!(output.status.code(), Some(3));
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(
        error.contains("1 confirmed") && error.contains("--rev 7"),
        "{error}"
    );
    assert!(output.stdout.is_empty());
}

#[test]
fn readback_failure_does_not_claim_success() {
    let fixture = Fixture::new(vec![]);
    fixture.faults.lock().unwrap().hide_created = true;
    let output = fixture.run(json!([round(1, "")]));
    assert_eq!(output.status.code(), Some(3));
    assert!(output.stdout.is_empty());
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(error.contains("verification failed"), "{error}");
    assert!(error.contains("--rev 7"), "{error}");
}

#[test]
fn reports_partial_push_without_retrying_non_idempotent_posts() {
    let fixture = Fixture::new(vec![]);
    fixture.faults.lock().unwrap().fail_event = Some(2);
    let output = fixture.run(json!([
        round(1, "first"),
        round(2, "second"),
        round(3, "third")
    ]));
    assert_eq!(output.status.code(), Some(3));
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(
        error.contains("1 confirmed") && error.contains("--rev 7"),
        "{error}"
    );
    assert!(output.stdout.is_empty());
    assert_eq!(fixture.rounds.lock().unwrap().len(), 1);
    assert_eq!(
        fixture
            .requests
            .lock()
            .unwrap()
            .iter()
            .filter(|(line, _)| line.starts_with("POST "))
            .count(),
        2
    );
}

#[test]
fn omitted_revision_resolves_head_once_and_pins_all_round_requests() {
    let fixture = Fixture::new(vec![]);
    let output = fixture.run_with_rev(json!([{"mode":"base","eventId":1}]), None);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let requests = fixture.requests.lock().unwrap();
    assert_eq!(
        requests
            .iter()
            .filter(|(line, _)| line.contains("/revisions"))
            .count(),
        1
    );
    assert!(
        requests
            .iter()
            .skip(1)
            .all(|(line, _)| line.contains("/r/7/api/devtool/saved-rounds"))
    );
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["revision"], 7);
}

#[test]
fn rejects_unsafe_target_without_contacting_server() {
    for (flag, value) in [
        ("--workspace", "../team"),
        ("--workspace", "team?x"),
        ("--game", "demo/other"),
        ("--game", "%2e%2e"),
        ("--rev", "0"),
    ] {
        let fixture = Fixture::new(vec![]);
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("rounds.json");
        std::fs::write(&file, json!({"rounds":[round(1,"")]}).to_string()).unwrap();
        let mut args = vec!["--workspace", "team", "--game", "demo", "--rev", "7"];
        let pos = args.iter().position(|v| *v == flag).unwrap();
        args[pos + 1] = value;
        let output = Command::new(env!("CARGO_BIN_EXE_sdt"))
            .args([
                "--server",
                &fixture.url,
                "--token",
                "fixture-token",
                "push-rounds",
            ])
            .arg(file)
            .args(args)
            .output()
            .unwrap();
        assert!(!output.status.success(), "accepted {flag} {value}");
        assert!(fixture.requests.lock().unwrap().is_empty());
    }
}

#[test]
fn validates_entire_input_before_any_network_request() {
    for bad in [
        json!({"mode":"base","eventId":0}),
        json!({"mode":"  ","eventId":2}),
        json!({"gameSlug":"other-game","mode":"base","eventId":2}),
        json!({"gameSlug":"","mode":"base","eventId":2}),
        json!({"mode":"base","eventId":-1}),
        json!({"mode":"base","eventId":4294967296u64}),
        json!({"mode":"base","eventId":1.5}),
    ] {
        let fixture = Fixture::new(vec![]);
        let output = fixture.run(json!({"rounds":[round(1,"valid"),bad]}));
        assert_eq!(
            output.status.code(),
            Some(1),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(fixture.requests.lock().unwrap().is_empty());
    }
}

#[test]
fn accepts_compact_array_with_target_game_and_default_description() {
    let fixture = Fixture::new(vec![]);
    let output = fixture.run(json!([{"mode":"base","eventId":2}]));
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let rounds = fixture.rounds.lock().unwrap();
    assert_eq!(rounds[0]["gameSlug"], "demo");
    assert_eq!(rounds[0]["description"], "");
}

#[test]
fn repeat_push_skips_exact_matches_including_duplicates_in_file() {
    let fixture = Fixture::new(vec![round(1, "existing")]);
    let input = json!({"rounds":[round(1,"existing"),round(2,"new"),round(2,"new")]});
    for expected in [(1, 2), (0, 3)] {
        let output = fixture.run(input.clone());
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let result: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(result["created"], expected.0);
        assert_eq!(result["skipped"], expected.1);
    }
    assert_eq!(fixture.rounds.lock().unwrap().len(), 2);
}

#[test]
fn pushes_to_pinned_workbench_store_and_preserves_existing_rounds() {
    let existing = round(1, "keep me");
    let fixture = Fixture::new(vec![existing.clone()]);
    let mut source = round(2, "new round");
    source["id"] = json!("desktop-id");
    source["createdAt"] = json!(1);
    source["updatedAt"] = json!(2);
    let output = fixture.run(json!({"rounds":[source]}));
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["created"], 1);
    assert_eq!(result["revision"], 7);
    let rounds = fixture.rounds.lock().unwrap();
    assert_eq!(rounds.len(), 2);
    assert_eq!(rounds[0], existing);
    let requests = fixture.requests.lock().unwrap();
    let posts: Vec<_> = requests
        .iter()
        .filter(|(line, _)| line.starts_with("POST "))
        .collect();
    assert_eq!(posts.len(), 1);
    assert_eq!(
        posts[0].0,
        "POST /api/ws/team/g/demo/r/7/api/devtool/saved-rounds HTTP/1.1"
    );
    assert_eq!(posts[0].1, round(2, "new round"));
    assert!(
        requests
            .last()
            .unwrap()
            .0
            .starts_with("GET /api/ws/team/g/demo/r/7/api/devtool/saved-rounds ")
    );
}
