use buckets_plugin::{
    CommandContext, CommandDescriptor, CommandResult, PluginOverrides, PluginRegistry,
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

fn fixture(descriptors: Value, invoke: &str) -> (tempfile::TempDir, PluginRegistry) {
    let root = tempfile::tempdir().unwrap();
    let listing = serde_json::to_string(&descriptors.to_string()).unwrap();
    fs::write(root.path().join("plugin.py"), format!("import json,sys,os,time\nr=json.load(sys.stdin)\nif r['method']=='commands.list':\n result=json.loads({listing})\nelse:\n{invoke}\nprint(json.dumps({{'jsonrpc':'2.0','id':r['id'],'result':result}}))\n")).unwrap();
    fs::write(root.path().join("plugin.json"), serde_json::to_vec(&json!({"id":"fixture","name":"Fixture","protocol_version":1,"executable":"python3","args":["plugin.py"],"capabilities":["commands"],"timeout_ms":3000})).unwrap()).unwrap();
    let registry = PluginRegistry::discover(root.path()).unwrap();
    (root, registry)
}
fn descriptor() -> Value {
    json!({"id":"open","title":"Open","command_line":{"name":"open","aliases":["o"],"arguments":[{"name":"paths","kind":"path","variadic":true}],"locations":["local"],"unavailable_reason":"Local files only"}})
}
fn context() -> CommandContext {
    CommandContext {
        location: "/tmp/frozen location".into(),
        selected: vec!["/tmp/selected 'quoted'".into()],
    }
}

#[test]
fn legacy_descriptors_and_invocation_remain_unchanged() {
    let (_root, registry) = fixture(
        json!([{"id":"hello","title":"Hello","shortcut":"ctrl-e"}]),
        " result=r['params']['request']",
    );
    let descriptors = registry.commands("fixture").unwrap();
    assert!(descriptors[0].command_line.is_none());
    assert!(
        serde_json::to_value(&descriptors[0])
            .unwrap()
            .get("command_line")
            .is_none()
    );
    let result = registry
        .invoke_command("fixture", "hello", json!({"custom":"legacy"}))
        .unwrap();
    assert_eq!(
        result,
        json!({"command":"hello","arguments":{"custom":"legacy"}})
    );
}

#[test]
fn typed_invocation_preserves_frozen_context_and_literal_argv() {
    let (_root, registry) = fixture(json!([descriptor()]), " result=r['params']['request']");
    let command = registry.commands("fixture").unwrap().remove(0);
    let args = vec![
        "a 'quote' $(touch INJECTED)".into(),
        String::new(),
        "line\nbreak".into(),
    ];
    let CommandResult::Value(result) = registry
        .invoke_command_line_result(
            "fixture",
            &command,
            context(),
            args.clone(),
            &AtomicBool::new(false),
        )
        .unwrap()
    else {
        panic!("value expected")
    };
    assert_eq!(
        result,
        json!({"command":"open","arguments":{"location":context().location,"selected":context().selected,"user_args":args}})
    );
}

#[test]
fn descriptors_reject_conflicting_names_and_malformed_metadata() {
    let cases = [
        json!({"name":"bad name"}),
        json!({"name":"ok","aliases":["ok"]}),
        json!({"name":"ok","aliases":["duplicate","duplicate"]}),
        json!({"name":"ok","arguments":[{"name":"first","variadic":true},{"name":"last"}]}),
        json!({"name":"ok","arguments":[{"name":"first"},{"name":"last","required":true}]}),
        json!({"name":"ok","arguments":[{"name":"mode","kind":"choice","choices":["bad\nchoice"]}]}),
        json!({"name":"ok","arguments":[{"name":"mode","kind":"choice","choices":[]}]}),
        json!({"name":"ok","arguments":[{"name":"mode","choices":["unused"]}]}),
        json!({"name":"ok","arguments":[{"name":"arg","unexpected":true}]}),
        json!({"name":"ok","locations":["local","local"]}),
        json!({"name":"ok","locations":["unknown"]}),
    ];
    for spec in cases {
        let (root, registry) = fixture(
            json!([{"id":"open","title":"Open","command_line":spec}]),
            " result=None",
        );
        assert!(
            registry.commands("fixture").is_err(),
            "accepted invalid metadata {spec} in {}",
            root.path().display()
        );
    }
    for descriptors in [
        json!([descriptor(), descriptor()]),
        json!([descriptor(),{"id":"other","title":"Other","command_line":{"name":"o"}}]),
    ] {
        let (_root, registry) = fixture(descriptors, " result=None");
        assert!(registry.commands("fixture").is_err());
    }
}

#[test]
fn invalid_context_args_provider_and_disabled_plugin_do_not_spawn() {
    let (root, registry) = fixture(
        json!([descriptor()]),
        " open('CALLED','w').write('called')\n result=None",
    );
    let mut command: CommandDescriptor = serde_json::from_value(descriptor()).unwrap();
    command.command_line.as_mut().unwrap().arguments = serde_json::from_value(
        json!([{"name":"mode","kind":"choice","required":true,"choices":["fast","full"]}]),
    )
    .unwrap();
    let cancel = AtomicBool::new(false);
    for args in [
        vec![],
        vec!["wrong".into()],
        vec!["fast".into(), "extra".into()],
        vec!["bad\0value".into()],
        vec!["x".repeat(32769)],
        vec!["fast".into(); 257],
    ] {
        assert!(
            registry
                .invoke_command_line_result("fixture", &command, context(), args, &cancel)
                .is_err()
        );
    }
    let mut remote = context();
    remote.location = "ssh://host/frozen".into();
    assert!(
        format!(
            "{:#}",
            registry
                .invoke_command_line_result(
                    "fixture",
                    &command,
                    remote,
                    vec!["fast".into()],
                    &cancel
                )
                .unwrap_err()
        )
        .contains("Local files only")
    );
    let mut invalid = context();
    invalid.selected = vec!["bad\0path".into()];
    assert!(
        registry
            .invoke_command_line_result("fixture", &command, invalid, vec!["fast".into()], &cancel)
            .is_err()
    );
    let disabled = registry
        .with_settings(&BTreeMap::from([(
            "fixture".into(),
            PluginOverrides {
                enabled: Some(false),
                ..Default::default()
            },
        )]))
        .unwrap();
    assert!(
        format!(
            "{:#}",
            disabled
                .invoke_command_line_result(
                    "fixture",
                    &command,
                    context(),
                    vec!["fast".into()],
                    &cancel
                )
                .unwrap_err()
        )
        .contains("disabled")
    );
    assert!(!root.path().join("CALLED").exists());
}

#[test]
fn command_cancellation_refuses_start_and_reaps_a_running_invocation() {
    let (root, registry) = fixture(
        json!([descriptor()]),
        " open('pid','w').write(str(os.getpid()))\n time.sleep(30)\n result={'message':'too late'}",
    );
    let command: CommandDescriptor = serde_json::from_value(descriptor()).unwrap();
    assert!(
        registry
            .invoke_command_line_result(
                "fixture",
                &command,
                context(),
                vec![],
                &AtomicBool::new(true)
            )
            .is_err()
    );
    assert!(!root.path().join("pid").exists());
    let cancelled = Arc::new(AtomicBool::new(false));
    let trigger = cancelled.clone();
    let pid_file = root.path().join("pid");
    let wait_file = pid_file.clone();
    let worker = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(2);
        while !wait_file.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        trigger.store(true, Ordering::Release);
    });
    let start = Instant::now();
    let error = registry
        .invoke_command_line_result("fixture", &command, context(), vec![], &cancelled)
        .unwrap_err();
    worker.join().unwrap();
    assert!(format!("{error:#}").contains("cancelled"));
    assert!(start.elapsed() < Duration::from_secs(3));
    #[cfg(unix)]
    {
        let pid: i32 = fs::read_to_string(pid_file).unwrap().parse().unwrap();
        assert_eq!(
            unsafe { libc::kill(pid, 0) },
            -1,
            "cancelled plugin remains alive"
        );
    }
}
