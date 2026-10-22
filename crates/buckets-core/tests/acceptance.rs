//! Independent acceptance tests: real fixtures, never the user's directories.
use buckets_core::{Backend, Location, SearchEngine, SearchQuery};
use std::{fs, process::Command};

#[test]
fn remote_location_roundtrip_preserves_literal_percent_and_reserved_characters() {
    for base in [
        "ssh://host/root",
        "s3://bucket/root",
        "plugin://fixture/root",
    ] {
        for name in [
            "literal%2Fname",
            "100% complete",
            "hash#query?.txt",
            "café.txt",
            "line\nbreak",
        ] {
            let location = Location::parse(base).unwrap().join(name).unwrap();
            let serialized = location.display();
            assert_eq!(
                Location::parse(&serialized).unwrap(),
                location,
                "failed {serialized:?}"
            );
        }
    }
}

#[test]
fn plugin_ids_accepted_by_manifest_can_be_opened_as_locations() {
    assert!(Location::parse("plugin://com.example.search/").is_ok());
}

#[test]
fn rename_preserves_unicode_spaces_and_never_replaces_a_dangling_symlink() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("café source.txt");
    fs::write(&source, "keep source").unwrap();
    #[cfg(unix)]
    {
        let target = temp.path().join("dangling");
        std::os::unix::fs::symlink("missing-target", &target).unwrap();
        assert!(
            Backend
                .rename(&Location::local(&source), "dangling")
                .is_err()
        );
        assert_eq!(
            fs::read_link(target).unwrap(),
            std::path::PathBuf::from("missing-target")
        );
        assert_eq!(fs::read_to_string(&source).unwrap(), "keep source");
    }
    let renamed = Backend
        .rename(&Location::local(&source), "résumé renamed.txt")
        .unwrap();
    assert_eq!(
        fs::read_to_string(renamed.local_path().unwrap()).unwrap(),
        "keep source"
    );
}

#[cfg(unix)]
#[test]
fn parsing_local_paths_preserves_symlink_parent_semantics() {
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir_all(temp.path().join("real/child")).unwrap();
    fs::write(temp.path().join("real/marker"), "correct target").unwrap();
    fs::write(temp.path().join("marker"), "wrong target").unwrap();
    std::os::unix::fs::symlink(temp.path().join("real/child"), temp.path().join("link")).unwrap();
    let input = temp.path().join("link/../marker");
    let location = Location::parse(input.to_str().unwrap()).unwrap();
    assert_eq!(
        Backend.preview(&location, 100).unwrap().text,
        "correct target"
    );
}

fn has_program(program: &str) -> bool {
    Command::new(program)
        .arg("--version")
        .output()
        .is_ok_and(|out| out.status.success())
}
fn query(engine: SearchEngine, text: &str) -> SearchQuery {
    SearchQuery {
        text: text.into(),
        engine,
        max_results: 20,
        ..Default::default()
    }
}
#[test]
fn fd_and_rg_find_real_content_and_apply_gitignore() {
    let temp = tempfile::tempdir().unwrap();
    assert!(
        Command::new("git")
            .args(["init", "-q"])
            .current_dir(temp.path())
            .status()
            .unwrap()
            .success()
    );
    fs::write(temp.path().join("keep filename.txt"), "nebula-token\n").unwrap();
    fs::write(temp.path().join("ignored filename.txt"), "nebula-token\n").unwrap();
    fs::write(temp.path().join(".gitignore"), "ignored*\n").unwrap();
    let root = Location::local(temp.path());
    for (program, engine, needle) in [
        ("fd", SearchEngine::Fd, "filename"),
        ("rg", SearchEngine::Rg, "nebula-token"),
    ] {
        if !has_program(program) {
            continue;
        }
        let hits = Backend.search(&root, &query(engine, needle)).unwrap();
        assert_eq!(hits.len(), 1, "{program} should apply .gitignore: {hits:?}");
        assert!(hits[0].name.starts_with("keep"));
        let mut all = query(engine, needle);
        all.respect_gitignore = false;
        assert_eq!(Backend.search(&root, &all).unwrap().len(), 2);
    }
}

#[test]
fn fd_result_limit_never_returns_a_partial_nonexistent_filename() {
    if !has_program("fd") {
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    for index in 0..1200 {
        fs::write(
            temp.path()
                .join(format!("needle-{index:04}-{}.txt", "x".repeat(160))),
            "",
        )
        .unwrap();
    }
    let mut q = query(SearchEngine::Fd, "needle");
    q.max_results = 1000;
    let hits = Backend.search(&Location::local(temp.path()), &q).unwrap();
    assert_eq!(hits.len(), 1000);
    assert!(
        hits.iter()
            .all(|hit| hit.location.local_path().unwrap().is_file()),
        "fd truncated a NUL-delimited filename"
    );
}

#[test]
fn rg_returns_requested_results_from_many_matching_files() {
    if !has_program("rg") {
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    for index in 0..300 {
        fs::write(temp.path().join(format!("file-{index:04}.txt")), "marker\n").unwrap();
    }
    let mut q = query(SearchEngine::Rg, "marker");
    q.max_results = 200;
    assert_eq!(
        Backend
            .search(&Location::local(temp.path()), &q)
            .unwrap()
            .len(),
        200
    );
}

#[test]
fn truncated_plugin_reads_are_not_silently_copied_as_complete_files() {
    if std::env::var_os("BUCKETS_ACCEPTANCE_PLUGIN_CHILD").is_some() {
        let destination =
            std::path::PathBuf::from(std::env::var_os("BUCKETS_ACCEPTANCE_DEST").unwrap());
        let source = Location::parse("plugin://fixture/document.txt").unwrap();
        let outcome = Backend.copy(&source, &Location::local(&destination));
        assert!(
            outcome.is_err(),
            "Copy accepted a plugin response with truncated=true and silently wrote {} bytes",
            fs::metadata(&destination)
                .map(|m| m.len())
                .unwrap_or_default()
        );
        assert!(
            !destination.exists(),
            "A refused partial copy must leave no output"
        );
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let plugin = temp.path().join("plugins/fixture");
    fs::create_dir_all(&plugin).unwrap();
    fs::write(plugin.join("plugin.json"),serde_json::to_vec(&serde_json::json!({"id":"fixture","name":"Partial response fixture","protocol_version":1,"executable":"python3","args":["provider.py"],"capabilities":["provider"],"permissions":{"read_roots":[temp.path()]},"timeout_ms":2000})).unwrap()).unwrap();
    fs::write(
        plugin.join("provider.py"),
        r#"import sys,json
q=json.loads(sys.stdin.readline())
if q['method']=='provider.list': result=[dict(name='document.txt',is_dir=False,size=20)]
elif q['method']=='provider.read': result=dict(text='part',truncated=True)
else: raise RuntimeError('unexpected method')
print(json.dumps(dict(jsonrpc='2.0',id=q['id'],result=result)))
"#,
    )
    .unwrap();
    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "truncated_plugin_reads_are_not_silently_copied_as_complete_files",
            "--nocapture",
        ])
        .env("BUCKETS_ACCEPTANCE_PLUGIN_CHILD", "1")
        .env(
            "BUCKETS_ACCEPTANCE_DEST",
            temp.path().join("destination.txt"),
        )
        .env("BUCKETS_PLUGIN_DIR", temp.path().join("plugins"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn text_only_plugin_provider_refuses_binary_and_invalid_utf8_copy() {
    if let Some(root) = std::env::var_os("BUCKETS_ACCEPTANCE_BINARY_CHILD") {
        let root = std::path::PathBuf::from(root);
        for name in ["binary.bin", "invalid-utf8.bin"] {
            let source = Location::parse(&format!("plugin://fixture/{name}")).unwrap();
            let destination = root.join(format!("copied-{name}"));
            assert!(
                Backend
                    .copy(&source, &Location::local(&destination))
                    .is_err(),
                "Text-only provider silently rewrote binary data from {name}"
            );
            assert!(!destination.exists());
        }
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    fs::write(temp.path().join("binary.bin"), [0, 1, 2, 3]).unwrap();
    fs::write(temp.path().join("invalid-utf8.bin"), [0xff, 0xfe, 0x41]).unwrap();
    let plugin = temp.path().join("plugins/fixture");
    fs::create_dir_all(&plugin).unwrap();
    let example = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/plugins/local-provider/provider.py");
    fs::copy(example, plugin.join("provider.py")).unwrap();
    fs::write(plugin.join("plugin.json"),serde_json::to_vec(&serde_json::json!({"id":"fixture","name":"Text-only provider fixture","protocol_version":1,"executable":"python3","args":["provider.py"],"capabilities":["provider"],"permissions":{"read_roots":[temp.path()]},"config":{"root":temp.path()},"timeout_ms":2000})).unwrap()).unwrap();
    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "text_only_plugin_provider_refuses_binary_and_invalid_utf8_copy",
            "--nocapture",
        ])
        .env("BUCKETS_ACCEPTANCE_BINARY_CHILD", temp.path())
        .env("BUCKETS_PLUGIN_DIR", temp.path().join("plugins"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[cfg(unix)]
#[test]
fn ssh_adapter_preserves_names_and_no_replace_through_real_remote_scripts() {
    if let Some(root) = std::env::var_os("BUCKETS_ACCEPTANCE_SSH_CHILD") {
        let root = std::path::PathBuf::from(root);
        let remote = Location::Ssh {
            host: "fixture".into(),
            path: root.join("remote").to_string_lossy().into_owned(),
        };
        let original = "quotes' dollar$(no-shell) ; café%2F.txt";
        let entries = Backend.list(&remote).unwrap();
        assert!(entries.iter().any(|entry| entry.name == original));
        let source = remote.join(original).unwrap();
        assert_eq!(
            Backend.preview(&source, 100).unwrap().text,
            "remote content"
        );
        let copied = remote.join("copied.txt").unwrap();
        Backend.copy(&source, &copied).unwrap();
        assert_eq!(
            fs::read_to_string(root.join("remote/copied.txt")).unwrap(),
            "remote content"
        );
        assert!(Backend.copy(&source, &copied).is_err());
        assert!(Backend.rename(&source, "copied.txt").is_err());
        let renamed = Backend.rename(&source, "renamed ' safe.txt").unwrap();
        assert_eq!(
            Backend.preview(&renamed, 100).unwrap().text,
            "remote content"
        );
        assert!(!root.join("remote").join(original).exists());
        let local = root.join("download.txt");
        Backend.copy(&renamed, &Location::local(&local)).unwrap();
        assert_eq!(fs::read_to_string(local).unwrap(), "remote content");
        fs::write(root.join("remote/.gitignore"), "ignored.txt\n").unwrap();
        fs::write(root.join("remote/ignored.txt"), "remote content").unwrap();
        if has_program("fd") {
            let hits = Backend
                .search(&remote, &query(SearchEngine::Fd, "copied"))
                .unwrap();
            assert_eq!(hits.len(), 1);
            assert_eq!(hits[0].name, "copied.txt");
        }
        if has_program("rg") {
            let hits = Backend
                .search(&remote, &query(SearchEngine::Rg, "remote content"))
                .unwrap();
            assert_eq!(hits.len(), 2);
            assert!(hits.iter().all(|hit| hit.line == Some(1)));
            assert!(!hits.iter().any(|hit| hit.name == "ignored.txt"));
        }
        let tree = root.join("local-tree");
        fs::create_dir_all(tree.join("nested")).unwrap();
        fs::write(tree.join("nested/data.txt"), "tree data").unwrap();
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        fs::set_permissions(
            tree.join("nested/data.txt"),
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        std::os::unix::fs::symlink("missing", tree.join("dangling")).unwrap();
        let remote_tree = remote.join("tree").unwrap();
        Backend.copy(&Location::local(&tree), &remote_tree).unwrap();
        assert_eq!(
            fs::read_link(root.join("remote/tree/dangling")).unwrap(),
            std::path::PathBuf::from("missing")
        );
        assert_eq!(
            fs::metadata(root.join("remote/tree/nested/data.txt"))
                .unwrap()
                .mode()
                & 0o777,
            0o755
        );
        let alias = root.join("alias");
        std::os::unix::fs::symlink(root.join("remote/tree/nested"), &alias).unwrap();
        let unsafe_destination = Location::Ssh {
            host: "fixture".into(),
            path: alias.join("copy").to_string_lossy().into_owned(),
        };
        assert!(Backend.copy(&remote_tree, &unsafe_destination).is_err());
        assert!(!alias.join("copy").exists());
        let moved_tree = root.join("moved-tree");
        Backend
            .move_entry(&remote_tree, &Location::local(&moved_tree))
            .unwrap();
        assert!(!root.join("remote/tree").exists());
        assert_eq!(
            fs::read_to_string(moved_tree.join("nested/data.txt")).unwrap(),
            "tree data"
        );
        assert_eq!(
            fs::metadata(moved_tree.join("nested/data.txt"))
                .unwrap()
                .mode()
                & 0o777,
            0o755
        );
        assert_eq!(
            fs::read_link(moved_tree.join("dangling")).unwrap(),
            std::path::PathBuf::from("missing")
        );
        // Larger than both the old 128 MiB transfer cap and old 32 MiB process
        // output cap. Sparse input keeps fixture creation cheap; markers and a
        // digest catch missing ranges and incomplete streaming.
        use std::io::{Read, Seek, SeekFrom, Write};
        let large = root.join("large-source.bin");
        let mut large_file = fs::File::create(&large).unwrap();
        let byte_count = 129 * 1024 * 1024 + 17;
        large_file.set_len(byte_count).unwrap();
        large_file.write_all(b"stream head").unwrap();
        large_file.seek(SeekFrom::End(-11)).unwrap();
        large_file.write_all(b"stream tail").unwrap();
        drop(large_file);
        let remote_large = remote.join("large-copy.bin").unwrap();
        Backend
            .copy(&Location::local(&large), &remote_large)
            .unwrap();
        let downloaded = root.join("large-downloaded.bin");
        Backend
            .copy(&remote_large, &Location::local(&downloaded))
            .unwrap();
        assert_eq!(fs::metadata(&downloaded).unwrap().len(), byte_count);
        let digest = |path: &std::path::Path| {
            use sha2::{Digest, Sha256};
            let mut file = fs::File::open(path).unwrap();
            let mut hash = Sha256::new();
            let mut buffer = [0; 65536];
            loop {
                let read = file.read(&mut buffer).unwrap();
                if read == 0 {
                    break;
                }
                hash.update(&buffer[..read]);
            }
            hash.finalize().to_vec()
        };
        assert_eq!(digest(&large), digest(&downloaded));
        return;
    }
    use std::os::unix::fs::PermissionsExt;
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir(temp.path().join("remote")).unwrap();
    fs::write(
        temp.path()
            .join("remote/quotes' dollar$(no-shell) ; café%2F.txt"),
        "remote content",
    )
    .unwrap();
    let bin = temp.path().join("bin");
    fs::create_dir(&bin).unwrap();
    let shim = bin.join("ssh");
    // Replace only the SSH transport. The production provider's Python command,
    // quoting, filesystem mutations and errors execute unchanged in our fixture.
    fs::write(&shim,"#!/usr/bin/env python3\nimport os,sys,shlex\ncommand=shlex.split(sys.argv[-1])\nos.execvpe(command[0],command,os.environ)\n").unwrap();
    fs::set_permissions(&shim, fs::Permissions::from_mode(0o755)).unwrap();
    let mut search_path = vec![bin];
    search_path.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));
    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "ssh_adapter_preserves_names_and_no_replace_through_real_remote_scripts",
            "--nocapture",
        ])
        .env("BUCKETS_ACCEPTANCE_SSH_CHILD", temp.path())
        .env("HOME", temp.path())
        .env_remove("XDG_DATA_HOME")
        .env("PATH", std::env::join_paths(search_path).unwrap())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[cfg(unix)]
#[test]
fn s3_adapter_handles_empty_objects_prefix_collisions_and_conditional_moves() {
    if let Some(root) = std::env::var_os("BUCKETS_ACCEPTANCE_S3_CHILD") {
        let root = std::path::PathBuf::from(root);
        let bucket = Location::parse("s3://fixture/").unwrap();
        let file = bucket.join("foo").unwrap();
        let folder = Location::parse("s3://fixture/foo/").unwrap();
        let listing = Backend.list(&bucket).unwrap();
        assert!(listing.iter().any(|e| e.name == "foo" && e.is_dir));
        assert!(listing.iter().any(|e| e.name == "foo" && !e.is_dir));
        assert!(
            !Backend.stat(&file).unwrap().is_dir,
            "Exact object foo must win over directory fallback foo/"
        );
        assert!(Backend.stat(&folder).unwrap().is_dir);
        Backend
            .copy(&file, &Location::local(root.join("flat.txt")))
            .unwrap();
        assert_eq!(
            fs::read_to_string(root.join("flat.txt")).unwrap(),
            "flat object"
        );
        Backend
            .copy(&folder, &Location::local(root.join("directory")))
            .unwrap();
        assert_eq!(
            fs::read_to_string(root.join("directory/child.txt")).unwrap(),
            "folder child"
        );
        let empty = bucket.join("empty").unwrap();
        assert_eq!(Backend.preview(&empty, 100).unwrap().text, "");
        Backend
            .copy(&empty, &Location::local(root.join("empty-copy")))
            .unwrap();
        assert_eq!(fs::metadata(root.join("empty-copy")).unwrap().len(), 0);
        let source = bucket.join("source").unwrap();
        assert!(
            Backend
                .move_entry(&source, &bucket.join("occupied").unwrap())
                .is_err()
        );
        assert_eq!(Backend.preview(&source, 100).unwrap().text, "source bytes");
        assert_eq!(
            Backend
                .preview(&bucket.join("occupied").unwrap(), 100)
                .unwrap()
                .text,
            "existing bytes"
        );
        Backend
            .move_entry(&source, &bucket.join("moved").unwrap())
            .unwrap();
        assert!(Backend.stat(&source).is_err());
        assert_eq!(
            Backend
                .preview(&bucket.join("moved").unwrap(), 100)
                .unwrap()
                .text,
            "source bytes"
        );
        let mutable = bucket.join("mutable").unwrap();
        assert!(
            Backend
                .move_entry(&mutable, &bucket.join("mutation-destination").unwrap())
                .is_err()
        );
        assert_eq!(
            Backend.preview(&mutable, 100).unwrap().text,
            "concurrent changed"
        );
        assert_eq!(
            Backend
                .preview(&bucket.join("mutation-destination").unwrap(), 100)
                .unwrap()
                .text,
            "original bytes"
        );
        let race = bucket.join("delete-race").unwrap();
        assert!(
            Backend
                .move_entry(&race, &bucket.join("delete-race-destination").unwrap())
                .is_err()
        );
        assert_eq!(
            Backend.preview(&race, 100).unwrap().text,
            "changed before conditional delete"
        );
        assert_eq!(
            Backend
                .preview(&bucket.join("delete-race-destination").unwrap(), 100)
                .unwrap()
                .text,
            "race bytes"
        );
        Backend
            .move_entry(&folder, &bucket.join("moved-folder").unwrap())
            .unwrap();
        assert_eq!(
            Backend
                .preview(
                    &Location::parse("s3://fixture/moved-folder/child.txt").unwrap(),
                    100
                )
                .unwrap()
                .text,
            "folder child"
        );
        assert_eq!(Backend.preview(&file, 100).unwrap().text, "flat object");
        let state: serde_json::Value =
            serde_json::from_slice(&fs::read(root.join("state.json")).unwrap()).unwrap();
        let calls = state["calls"].as_array().unwrap();
        assert!(
            calls
                .iter()
                .filter(|call| call["action"] == "list-objects-v2")
                .count()
                > 1,
            "Fixture must exercise pagination"
        );
        assert!(calls.iter().any(|call| call["action"] == "delete-object"
            && call["key"] == "source"
            && call["conditional"] == true));
        return;
    }
    use std::os::unix::fs::PermissionsExt;
    let temp = tempfile::tempdir().unwrap();
    let state = serde_json::json!({"objects":{
        "empty":{"text":"","version":1},"foo":{"text":"flat object","version":1},"foo/child.txt":{"text":"folder child","version":1},
        "source":{"text":"source bytes","version":1},"occupied":{"text":"existing bytes","version":1},
        "mutable":{"text":"original bytes","version":1},"delete-race":{"text":"race bytes","version":1}
    },"calls":[]});
    fs::write(
        temp.path().join("state.json"),
        serde_json::to_vec(&state).unwrap(),
    )
    .unwrap();
    let bin = temp.path().join("bin");
    fs::create_dir(&bin).unwrap();
    let shim = bin.join("aws");
    fs::write(&shim, S3_CLI_FIXTURE).unwrap();
    fs::set_permissions(&shim, fs::Permissions::from_mode(0o755)).unwrap();
    let mut search_path = vec![bin];
    search_path.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));
    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "s3_adapter_handles_empty_objects_prefix_collisions_and_conditional_moves",
            "--nocapture",
        ])
        .env("BUCKETS_ACCEPTANCE_S3_CHILD", temp.path())
        .env(
            "BUCKETS_ACCEPTANCE_S3_STATE",
            temp.path().join("state.json"),
        )
        .env("PATH", std::env::join_paths(search_path).unwrap())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

// A stateful AWS CLI protocol fixture. Production Rust dispatch and transfer
// code run unchanged; this does not claim live S3 endpoint certification.
#[cfg(unix)]
const S3_CLI_FIXTURE: &str = r#"#!/usr/bin/env python3
import sys,os,json,hashlib
from pathlib import Path
args=sys.argv[1:]
action=args[args.index('s3api')+1]
def option(name,default=None):
 return args[args.index(name)+1] if name in args else default
state=Path(os.environ['BUCKETS_ACCEPTANCE_S3_STATE'])
db=json.loads(state.read_text())
objects=db['objects']
key=option('--key')
def save():state.write_text(json.dumps(db))
def fail(message):
 save();print(message,file=sys.stderr);sys.exit(4)
def metadata(obj):
 data=obj['text'].encode()
 return dict(ContentLength=len(data),ETag='"'+hashlib.md5(data).hexdigest()+'"',VersionId=str(obj['version']))
db['calls'].append(dict(action=action,key=key,conditional=('--if-match' in args)))
if action=='list-objects-v2':
 prefix=option('--prefix','');delimiter=option('--delimiter')
 entries={}
 for name,obj in objects.items():
  if not name.startswith(prefix):continue
  rest=name[len(prefix):]
  if delimiter and delimiter in rest:
   common=prefix+rest.split(delimiter,1)[0]+delimiter
   entries[common]=('directory',dict(Prefix=common))
  else:entries[name]=('file',dict(Key=name,Size=len(obj['text'].encode())))
 ordered=sorted(entries.items());start=int(option('--continuation-token','0'));page=ordered[start:start+2]
 result=dict(Contents=[item for _,(kind,item) in page if kind=='file'],CommonPrefixes=[item for _,(kind,item) in page if kind=='directory'],IsTruncated=start+2<len(ordered))
 if result['IsTruncated']:result['NextContinuationToken']=str(start+2)
elif action=='head-object':
 if key not in objects:fail('NotFound')
 result=metadata(objects[key])
elif action=='get-object':
 if key not in objects:fail('NoSuchKey')
 data=objects[key]['text'].encode();byte_range=option('--range')
 if byte_range:
  if not data:fail('InvalidRange')
  start,end=map(int,byte_range.removeprefix('bytes=').split('-'));data=data[start:end+1]
 Path(args[-1]).write_bytes(data);result=metadata(objects[key])
elif action=='put-object':
 if option('--if-none-match')!='*':fail('Missing safe create-new condition')
 if key in objects:fail('PreconditionFailed')
 text=Path(option('--body')).read_bytes().decode()
 objects[key]=dict(text=text,version=1)
 if key=='mutation-destination':objects['mutable']=dict(text='concurrent changed',version=2)
 result=metadata(objects[key])
elif action=='delete-object':
 if key=='delete-race':objects[key]=dict(text='changed before conditional delete',version=2)
 if key not in objects:fail('NoSuchKey')
 if option('--if-match')!=metadata(objects[key])['ETag']:fail('PreconditionFailed')
 del objects[key];result={}
else:fail('Unsupported action '+action)
save();print(json.dumps(result))
"#;
