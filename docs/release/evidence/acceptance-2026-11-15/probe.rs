use std::{path::Path, sync::Arc, process::Command, ffi::OsString};
use hame_agent::TurnHook;
use hame_session::{Block, Event, Record};
use hame_session::contract::{Predicate, TaskContract, Authorization, Budget, CONTRACT_VERSION};
use hame_verify::{verify, OutcomeGate, Confinement, ConfinedCommand};
use hame_sandbox::{SandboxPolicy, wrap};

fn task(id: &str, predicate: Predicate) -> TaskContract {
    TaskContract { version: CONTRACT_VERSION, id: id.into(), outcome: "Check synthetic fixture".into(), constraints: vec![], acceptance: vec![predicate], authorization: Authorization::default(), budget: Budget::default() }
}

fn confined(policy: &SandboxPolicy, cwd: &Path, program: &str, args: &[&str]) -> std::process::Output {
    let args: Vec<OsString> = args.iter().map(OsString::from).collect();
    let wrapped = wrap(policy, Path::new("unused-on-macos"), Path::new(program), &args).unwrap();
    Command::new(wrapped.program).args(wrapped.args).current_dir(cwd).env_clear().output().unwrap()
}

fn main() {
    let root = std::env::temp_dir().join(format!("hame-acceptance-fixtures-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let root = root.canonicalize().unwrap();
    tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
        let marker = "SYNTHETIC_PRIVATE_MARKER_ONLY_FOR_ACCEPTANCE";
        std::fs::write(root.join(".env"), marker).unwrap();
        let policy = SandboxPolicy::workspace(&root);
        let direct = confined(&policy, &root, "/bin/cat", &[".env"]);
        let link = confined(&policy, &root, "/bin/ln", &["-s", ".env", "out.txt"]);
        assert!(!direct.status.success(), "fixture must be unreadable in the real sandbox");
        assert!(link.status.success(), "agent must be able to create the symlink");
        let confine: Confinement = Arc::new(move |program, args| {
            wrap(&policy, Path::new("unused-on-macos"), program, args).map(|w| ConfinedCommand { program: w.program, args: w.args, env: Some(vec![]) }).map_err(|e| e.to_string())
        });
        let contract = task("symlink-protected-file", Predicate::FileContains { path: "out.txt".into(), text: "expected-output".into() });
        let gate = OutcomeGate { cwd: root.clone(), probe: None, confine: Some(confine), max_rounds: 2 };
        let records = vec![Record { seq: 1, ts: 0, event: Event::TaskStart { contract: Box::new(contract) } }];
        let feedback = gate.before_stop(&records, true).await.unwrap();
        let leak = feedback.iter().any(|b| matches!(b, Block::Text { text } if text.contains(marker)));
        println!("protected_file: sandbox_read_denied={}, sandbox_symlink_created={}, marker_in_model_feedback={}", !direct.status.success(), link.status.success(), leak);
        assert!(leak, "reported vulnerability no longer reproduces");

        std::os::unix::fs::symlink("cycle", root.join("cycle")).unwrap();
        let stat_error = std::fs::metadata(root.join("cycle")).unwrap_err();
        let absence = verify(&task("absence-on-stat-error", Predicate::FileAbsent { path: "cycle".into() }), &root, None, None).await;
        println!("file_absent: metadata_error={:?}, actual_directory_entry_exists={}, verified={}", stat_error.kind(), std::fs::symlink_metadata(root.join("cycle")).is_ok(), absence.passed);
        assert!(absence.passed);

        std::fs::write(root.join("multiline.csv"), "name,note\nAlice,\"first line\nsecond line\"\n").unwrap();
        for expected in [1, 2] {
            let report = verify(&task("multiline-csv", Predicate::CsvShape { path: "multiline.csv".into(), rows: Some(expected), columns: vec!["name".into(), "note".into()] }), &root, None, None).await;
            println!("csv_multiline: actual_records=1, requested_records={}, verified={}, evidence={}", expected, report.passed, report.results[0].evidence);
            assert_eq!(report.passed, expected == 2);
        }
        std::fs::write(root.join("ragged.csv"), "name,amount\nAlice\n").unwrap();
        let report = verify(&task("ragged-csv", Predicate::CsvShape { path: "ragged.csv".into(), rows: Some(1), columns: vec!["name".into(), "amount".into()] }), &root, None, None).await;
        println!("csv_ragged: data_columns=1, header_columns=2, verified={}", report.passed);
    });
    std::fs::remove_dir_all(root).unwrap();
}
