use buckets_plugin::{AgentMode, AgentPhase, AgentRequest, PluginRegistry, WorkspaceContext};
use serde_json::json;
use std::{fs, sync::atomic::AtomicBool};

fn fixture(mode: &str) -> (tempfile::TempDir, PluginRegistry, AgentRequest) {
    let directory = tempfile::tempdir().unwrap();
    fs::write(
        directory.path().join("plugin.json"),
        json!({
            "id":"stream-fixture","name":"Stream fixture","protocol_version":1,
            "executable":"python3","args":["stream.py"],"capabilities":["agent"],
            "timeout_ms":if mode == "callback_budget" { 30000 } else { 5000 }
        })
        .to_string(),
    )
    .unwrap();
    fs::write(directory.path().join("stream.py"), r#"import json,sys
for line in sys.stdin:
 f=json.loads(line);r=f['params']['request'];mode=r['prompt']
 if mode=='callback_budget':
  for n in range(300):
   callback={'jsonrpc':'2.0','id':'callback-'+str(n),'method':'workspace.snapshot','params':{'padding':'x'*524288}}
   print(json.dumps(callback)+' '*(524288-1024),flush=True)
   if not sys.stdin.readline(): sys.exit(0)
  print(json.dumps({'jsonrpc':'2.0','id':f['id'],'result':{'answer':'Budget was not enforced','citations':[],'results':[]}}),flush=True)
  continue
 count=1100 if mode=='many' else 2
 for n in range(count):
  update={'sequence':n+1,'text':'Hello 😀\n**Markdown**','phase':'responding','activities':[{'id':'read-1','title':'Read file · /fixture/main.rs','state':'complete'}]}
  request_id=r['request_id']
  if mode=='sequence' and n==1: update['sequence']=1
  if mode=='zero': update['sequence']=0
  if mode=='utf8': update['text']='😀'*16385
  if mode=='activity': update['activities'][0]['title']='x'*513
  if mode=='duplicates': update['activities']*=2
  if mode=='controls': update['activities'][0]['title']='read\nspoof'
  if mode=='request': request_id='another-request'
  print(json.dumps({'jsonrpc':'2.0','method':'agent.progress','params':{'request_id':request_id,'message':'Responding','update':update}}),flush=True)
 print(json.dumps({'jsonrpc':'2.0','id':f['id'],'result':{'answer':'Verified final answer','citations':[],'results':[]}}),flush=True)
"#).unwrap();
    let registry = PluginRegistry::discover(directory.path()).unwrap();
    let request = AgentRequest {
        request_id: "current-request".into(),
        mode: AgentMode::Ask,
        prompt: mode.into(),
        context: WorkspaceContext::default(),
        storage_dir: directory.path().join("state"),
        memory_update: None,
    };
    (directory, registry, request)
}

#[test]
fn streaming_snapshots_survive_long_runs_and_finish_separately_from_verified_results() {
    let (_directory, registry, request) = fixture("many");
    let mut service = registry.start_agent("stream-fixture").unwrap();
    let mut events = vec![];
    let result = service
        .invoke(
            request,
            |_, _| panic!("No callbacks"),
            |event| events.push(event),
            &AtomicBool::new(false),
        )
        .unwrap();
    assert_eq!(events.len(), 1100);
    for (index, event) in events.iter().enumerate() {
        let update = event.update.as_ref().unwrap();
        assert_eq!(update.sequence, index as u64 + 1);
        assert_eq!(update.text, "Hello 😀\n**Markdown**");
        assert_eq!(update.phase, AgentPhase::Responding);
    }
    assert_eq!(result.answer, "Verified final answer");
    assert!(service.is_running());
}

#[test]
fn malformed_streams_are_bounded_before_reaching_presentation_and_poison_the_service() {
    for (mode, message) in [
        ("sequence", "sequence"),
        ("zero", "sequence"),
        ("utf8", "64 KiB"),
        ("activity", "title"),
        ("duplicates", "Duplicate"),
        ("controls", "title"),
        ("request", "progress context"),
    ] {
        let (_directory, registry, request) = fixture(mode);
        let mut service = registry.start_agent("stream-fixture").unwrap();
        let error = service
            .invoke(
                request,
                |_, _| panic!("No callbacks"),
                |_| {},
                &AtomicBool::new(false),
            )
            .unwrap_err();
        assert!(error.to_string().contains(message), "{mode}: {error:#}");
        assert!(!service.is_running());
    }
}

#[test]
fn callback_only_traffic_exhausts_the_invocation_wire_budget_before_dispatch() {
    let (_directory, registry, request) = fixture("callback_budget");
    let mut service = registry.start_agent("stream-fixture").unwrap();
    let mut callbacks = 0;
    let error = service
        .invoke(
            request,
            |method, _| {
                assert_eq!(method, "workspace.snapshot");
                callbacks += 1;
                Ok(json!({}))
            },
            |_| panic!("The fixture sends callbacks only, no progress frames"),
            &AtomicBool::new(false),
        )
        .unwrap_err();
    assert!(
        error.to_string().contains("invocation byte budget"),
        "{error:#}"
    );
    // Each physical line is just below 1 MiB; half of it is whitespace.
    // Charging normalized JSON would fail to exhaust the budget in 300 calls.
    assert_eq!(callbacks, 256);
    assert!(!service.is_running());
}
