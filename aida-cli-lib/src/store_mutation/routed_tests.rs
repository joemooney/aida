//! Synthetic launch identity; production client, broker, host and redo persistence.
// trace:TASK-3223 | ai:codex
use super::*;
use crate::store_mutation_client::Client;
use aida_core::db::{canonical_mutation::CanonicalOperation, redo};
use std::io::BufRead;
use std::process::{Child, Command, Stdio};

pub(crate) struct Route {
    tmp: tempfile::TempDir,
    child: Child,
    host: Option<std::thread::JoinHandle<()>>,
    epoch: uuid::Uuid,
    pub(crate) origin: String,
    root: PathBuf,
}
impl Route {
    fn new(drop_reply: bool) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("store");
        Self::at_root(tmp, root, drop_reply)
    }
    pub(crate) fn at(root: PathBuf) -> Self {
        Self::at_root(tempfile::tempdir().unwrap(), root, false)
    }
    fn at_root(tmp: tempfile::TempDir, root: PathBuf, drop_reply: bool) -> Self {
        aida_core::git_ops::init(&root).unwrap();
        aida_core::allocation_authority::write_mode(
            &root,
            aida_core::allocation_authority::AllocationRole::Root,
            None,
        )
        .unwrap();
        let origin = "TASK-b6b1278890774ef993d2ebc327862470-880".to_owned();
        let mut row = aida_core::Requirement::new("routed".into(), "description".into());
        row.spec_id = Some(origin.clone());
        row.agreed_id = Some("TASK-3223".into());
        aida_core::object_store::write_object(&root.join("objects"), &row).unwrap();
        let epoch = uuid::Uuid::new_v4();
        let (mut host, helper) = UnixStream::pair().unwrap();
        host.set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        host.set_write_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let scopes = vec![row.id.to_string()];
        let host_root = root.clone();
        let worker = std::thread::spawn(move || loop {
            let mut byte = [0];
            match host.read_exact(&mut byte) {
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::UnexpectedEof | std::io::ErrorKind::ConnectionReset
                    ) =>
                {
                    break
                }
                result => result.unwrap(),
            }
            if byte == [b'?'] {
                host.write_all(b"Y").unwrap();
                continue;
            }
            assert_eq!(byte, [b'M']);
            let request = read_operation(&mut host).unwrap();
            let result = host_operation(
                &HostContext {
                    profile: crate::contained_boundary::StoreWriteProfile::Mediated,
                    root: &host_root,
                    binding: "fixed",
                    epoch,
                    specs: &scopes,
                    author: "bound author",
                    role: "implementer",
                },
                &request,
                || Ok(()),
            );
            let mut reply =
                serde_json::json!({"launch_epoch":epoch,"request_id":request.request_id});
            match result {
                Ok(value) => reply["result"] = value,
                Err(error) => reply["error"] = serde_json::json!(error.to_string()),
            }
            let wire = serde_json::to_vec(&reply).unwrap();
            host.write_all(&(wire.len() as u32).to_be_bytes()).unwrap();
            host.write_all(&wire).unwrap();
        });
        let script = r#"
import json, os, select, socket, sys, time
from tests.test_store_mutation import ns, Service, Identity
channel = socket.socket(fileno=0)
authority = ns['MutationAuthority'](channel)
identity = Identity()
identity.allowed.add(os.getppid())
# Synthetic launch ancestry, not a production grant or namespace attestation.
# SO_CREDENTIALS supplies the host PID even for the bwrap PID namespace.
original_check = identity.check
def fixture_descendant(pid, uid, gid, sender_fd=None):
    if uid != os.getuid() or gid != os.getgid(): return False
    for _ in range(64):
        if original_check(pid, uid, gid, sender_fd=sender_fd): return True
        try:
            with open('/proc/%d/status' % pid) as status:
                pid = next(int(line.split()[1]) for line in status if line.startswith('PPid:'))
        except (OSError, StopIteration, ValueError): return False
        if pid <= 1: return False
    return False
identity.check = fixture_descendant
service = Service(dict(epoch=sys.argv[2], specs=[], role='implementer', store_profile='mediated'), identity, authority)
listener = socket.socket(socket.AF_UNIX, socket.SOCK_SEQPACKET)
listener.setsockopt(socket.SOL_SOCKET, socket.SO_PASSCRED, 1)
listener.setsockopt(socket.SOL_SOCKET, ns['SO_PASSPIDFD'], 1)
listener.bind(sys.argv[1])
listener.listen(8)
listener.setblocking(False)
drop_reply = sys.argv[3] == 'true'
class LostReply:
    def __init__(self, sock): self.sock = sock
    def recvmsg(self, *args): return self.sock.recvmsg(*args)
    def close(self): self.sock.close()
    def send(self, data, flags):
        global drop_reply
        if drop_reply and json.loads(data)['outcome'] == 'success':
            drop_reply = False
            raise OSError('injected lost reply after host persistence')
        return self.sock.send(data, flags)
print('ready', flush=True)
# A watchdog bounds fixture failures independently of client timeouts.
end = time.monotonic() + 25
while time.monotonic() < end:
    if select.select([listener], [], [], .01)[0]:
        sock, _ = listener.accept()
        sock.setblocking(False)
        service.channels[sock.fileno()] = (LostReply(sock), time.monotonic() + 5)
    service.poll()
service.close()
channel.close()
"#;
        let helper = File::from(std::os::fd::OwnedFd::from(helper));
        let child = Command::new("python3")
            .args(["-c", script])
            .arg(tmp.path().join("service"))
            .arg(epoch.to_string())
            .arg(drop_reply.to_string())
            .current_dir(Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap())
            .stdin(Stdio::from(helper))
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let mut route = Self {
            tmp,
            child,
            host: Some(worker),
            epoch,
            origin,
            root,
        };
        let stdout = route.child.stdout.take().unwrap();
        let mut poll = libc::pollfd {
            fd: stdout.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        assert!(
            unsafe { libc::poll(&mut poll, 1, 5000) } > 0,
            "broker readiness timeout"
        );
        let mut ready = String::new();
        std::io::BufReader::new(stdout)
            .take(128)
            .read_line(&mut ready)
            .unwrap();
        assert_eq!(ready, "ready\n");
        route
    }
    fn client(&self) -> Client {
        Client::fixture(
            self.tmp.path().join("service").to_str().unwrap().into(),
            self.epoch,
        )
    }
    pub(crate) fn root(&self) -> PathBuf {
        self.root.clone()
    }
    pub(crate) fn endpoint(&self) -> PathBuf {
        self.tmp.path().join("service")
    }
    pub(crate) fn epoch(&self) -> uuid::Uuid {
        self.epoch
    }
    pub(crate) fn row(&self) -> aida_core::Requirement {
        serde_yaml::from_slice(&redo::object_bytes(&self.root(), &self.origin).unwrap()).unwrap()
    }
    fn operation(&self) -> CanonicalOperation {
        CanonicalOperation::CommentAdd {
            spec_id: self.origin.clone(),
            expected_revision: redo::hash(&redo::object_bytes(&self.root(), &self.origin).unwrap()),
            content: "routed once".into(),
        }
    }
}
impl Drop for Route {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(host) = self.host.take() {
            host.join().unwrap();
        }
    }
}

#[test]
fn client_broker_host_retry_after_lost_reply_and_stale_revision() {
    let route = Route::new(true);
    let op = route.operation();
    let id = uuid::Uuid::new_v4();
    let error = route.client().mutate(id, &op).unwrap_err();
    assert!(format!("{error:#}").contains("OutcomeUnknown"), "{error:#}");
    assert!(format!("{error:#}").contains("do not repeat with a new request ID"));
    assert_eq!(
        route.row().comments.len(),
        1,
        "host persisted before reply loss"
    );
    let first = route.client().mutate(id, &op).unwrap();
    assert_eq!(route.client().mutate(id, &op).unwrap(), first);
    assert_eq!(route.row().comments.len(), 1);
    let error = route
        .client()
        .mutate(uuid::Uuid::new_v4(), &op)
        .unwrap_err();
    assert!(format!("{error:#}").ends_with("Conflict"), "{error:#}");
    assert!(!format!("{error:#}").contains("do not repeat"));
    assert_eq!(route.row().comments.len(), 1);
    // Public agreed-ID translation also traverses the real broker and host.
    route
        .client()
        .comment(&route.root(), "TASK-3223", "agreed ID")
        .unwrap();
    assert_eq!(route.row().comments.len(), 2);
}

#[test]
fn client_broker_host_competing_mutations_compare_the_same_preimage() {
    let route = Route::new(false);
    let op = route.operation();
    let outcomes = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..2)
            .map(|_| {
                let client = route.client();
                let op = op.clone();
                scope.spawn(move || client.mutate(uuid::Uuid::new_v4(), &op))
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().unwrap())
            .collect::<Vec<_>>()
    });
    assert_eq!(outcomes.iter().filter(|r| r.is_ok()).count(), 1);
    let error = outcomes.into_iter().find_map(Result::err).unwrap();
    assert!(format!("{error:#}").ends_with("Conflict"), "{error:#}");
    let row = route.row();
    assert_eq!(row.comments.len(), 1);
    assert_eq!(row.comments[0].author, "bound author");
}

// trace:TASK-3223 | ai:codex
#[test]
fn priority_routes_through_client_broker_host_with_bound_history() {
    let route = Route::new(false);
    let before = route.row();
    let result = route
        .client()
        .mcp(
            &route.root(),
            "update_requirement",
            &serde_json::json!({"id":"TASK-3223", "priority":"low"}),
        )
        .unwrap();
    let after = route.row();
    assert_eq!(after.priority, aida_core::RequirementPriority::Low);
    assert_eq!(after.title, before.title);
    assert_eq!(after.description, before.description);
    assert_eq!(
        result["revision"],
        redo::hash(&redo::object_bytes(&route.root(), &route.origin).unwrap())
    );
    let changes: Vec<_> = after
        .history
        .iter()
        .flat_map(|entry| {
            entry
                .changes
                .iter()
                .filter(|change| change.field_name == "priority")
                .map(move |change| (&entry.author, change))
        })
        .collect();
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].0, "bound author");
    assert_eq!(changes[0].1.old_value, before.priority.to_string());
    assert_eq!(changes[0].1.new_value, "Low");
}

// trace:TASK-3223 | ai:codex
#[test]
fn weight_set_and_clear_persist_before_reply_replay_and_refuse_stale_bases() {
    for weight in [Some(0.75_f32), None] {
        let route = Route::new(true);
        if weight.is_none() {
            let mut row = route.row();
            row.weight = Some(42.0);
            aida_core::object_store::write_object(&route.root().join("objects"), &row).unwrap();
        }
        let op = CanonicalOperation::SpecEdit {
            spec_id: route.origin.clone(),
            expected_revision: redo::hash(
                &redo::object_bytes(&route.root(), &route.origin).unwrap(),
            ),
            fields: aida_core::db::canonical_mutation::EditFields {
                title: None,
                description: None,
                priority: None,
                weight: Some(weight),
            },
        };
        let id = uuid::Uuid::new_v4();
        let error = route.client().mutate(id, &op).unwrap_err();
        assert!(format!("{error:#}").contains("OutcomeUnknown"));
        let after = route.row();
        assert_eq!(after.weight, weight, "persisted before lost reply");
        let result = route.client().mutate(id, &op).unwrap();
        assert_eq!(
            result["revision"],
            redo::hash(&redo::object_bytes(&route.root(), &route.origin).unwrap())
        );
        assert_eq!(route.client().mutate(id, &op).unwrap(), result);
        let error = route
            .client()
            .mutate(uuid::Uuid::new_v4(), &op)
            .unwrap_err();
        assert!(format!("{error:#}").ends_with("Conflict"));
        assert_eq!(route.row().history.len(), after.history.len());
        let changes: Vec<_> = after
            .history
            .iter()
            .flat_map(|entry| {
                entry
                    .changes
                    .iter()
                    .filter(|change| change.field_name == "weight")
                    .map(move |change| (&entry.author, change))
            })
            .collect();
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].0, "bound author");
    }
}

// trace:TASK-3223 | ai:codex
#[test]
fn weight_competing_mutations_compare_the_same_preimage() {
    let route = Route::new(false);
    let mut row = route.row();
    row.weight = Some(42.0);
    aida_core::object_store::write_object(&route.root().join("objects"), &row).unwrap();
    let revision = redo::hash(&redo::object_bytes(&route.root(), &route.origin).unwrap());
    let outcomes = std::thread::scope(|scope| {
        let handles: Vec<_> = [Some(0.75), None]
            .into_iter()
            .map(|weight| {
                let client = route.client();
                let op = CanonicalOperation::SpecEdit {
                    spec_id: route.origin.clone(),
                    expected_revision: revision.clone(),
                    fields: aida_core::db::canonical_mutation::EditFields {
                        title: None,
                        description: None,
                        priority: None,
                        weight: Some(weight),
                    },
                };
                scope.spawn(move || client.mutate(uuid::Uuid::new_v4(), &op))
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().unwrap())
            .collect::<Vec<_>>()
    });
    assert_eq!(outcomes.iter().filter(|r| r.is_ok()).count(), 1);
    let error = outcomes.into_iter().find_map(Result::err).unwrap();
    assert!(format!("{error:#}").ends_with("Conflict"));
}
