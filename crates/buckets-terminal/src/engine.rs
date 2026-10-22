use crate::TerminalAppearance;
use alacritty_terminal::{
    Term,
    event::{Event, EventListener, WindowSize},
    grid::{Dimensions, Scroll},
    term::{Config, TermMode},
    vte::ansi::{Processor, Rgb},
};
use anyhow::{Context, Result, bail};
use parking_lot::Mutex;
use portable_pty::{ChildKiller, CommandBuilder, MasterPty, PtySize};
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    thread,
};

#[derive(Clone, Debug)]
pub struct TerminalSpec {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub title: String,
    pub env: BTreeMap<String, String>,
}

#[derive(Clone, Copy)]
pub(crate) struct Size {
    pub cols: usize,
    pub rows: usize,
}
impl Dimensions for Size {
    fn total_lines(&self) -> usize {
        self.rows
    }
    fn screen_lines(&self) -> usize {
        self.rows
    }
    fn columns(&self) -> usize {
        self.cols
    }
}

enum Input {
    Bytes(Vec<u8>),
    Close,
}
#[derive(Clone)]
pub(crate) struct Proxy {
    input: mpsc::SyncSender<Input>,
    pub events: Arc<Mutex<Vec<Event>>>,
    size: Arc<Mutex<Size>>,
    appearance: Arc<Mutex<TerminalAppearance>>,
    color_requests: Arc<Mutex<Vec<Event>>>,
}
impl Proxy {
    fn answer_color_requests(&self, term: &Term<Self>) {
        let requests = std::mem::take(&mut *self.color_requests.lock());
        if requests.is_empty() {
            return;
        }
        let appearance = self.appearance.lock();
        let content = term.renderable_content();
        for request in requests {
            if let Event::ColorRequest(index, format) = request {
                let c = appearance.indexed_color(index, content.colors);
                let response = format(Rgb {
                    r: (c >> 16) as u8,
                    g: (c >> 8) as u8,
                    b: c as u8,
                });
                let _ = self.input.try_send(Input::Bytes(response.into_bytes()));
            }
        }
    }
}
impl EventListener for Proxy {
    fn send_event(&self, event: Event) {
        let response = match &event {
            Event::PtyWrite(text) => Some(text.clone()),
            Event::ColorRequest(..) => {
                // The parser holds Term's lock. Resolve after parsing so OSC
                // overrides are honored without recursively acquiring that lock.
                let mut requests = self.color_requests.lock();
                if requests.len() < 128 {
                    requests.push(event.clone());
                }
                None
            }
            Event::TextAreaSizeRequest(format) => {
                let size = *self.size.lock();
                Some(format(WindowSize {
                    num_lines: size.rows as u16,
                    num_cols: size.cols as u16,
                    cell_width: 8,
                    cell_height: 20,
                }))
            }
            _ => None,
        };
        if let Some(text) = response {
            let _ = self.input.try_send(Input::Bytes(text.into_bytes()));
        }
        // Only these events require the UI. Keep the queue bounded for noisy applications.
        if matches!(
            event,
            Event::Title(_) | Event::ResetTitle | Event::ClipboardStore(..)
        ) {
            let mut events = self.events.lock();
            if events.len() < 128 {
                events.push(event);
            }
        }
    }
}

pub(crate) struct Shared {
    pub term: Mutex<Term<Proxy>>,
    pub revision: AtomicU64,
    pub exited: AtomicBool,
    closing: AtomicBool,
    cleanup: Mutex<()>,
    pub exit_status: Mutex<Option<String>>,
    pub proxy: Proxy,
}

/// A host-owned PTY process. Dropping it terminates its foreground job and session.
/// PTY reads, ANSI parsing and writes run on background threads, never GPUI's render thread.
pub struct TerminalSession {
    pub(crate) shared: Arc<Shared>,
    master: Mutex<Box<dyn MasterPty + Send>>,
    killer: Mutex<Box<dyn ChildKiller + Send + Sync>>,
    pid: Option<u32>,
    spec: TerminalSpec,
    closed: AtomicBool,
}
impl TerminalSession {
    pub(crate) fn set_appearance(&self, appearance: TerminalAppearance) {
        *self.shared.proxy.appearance.lock() = appearance;
    }
    pub fn spawn(spec: TerminalSpec) -> Result<Self> {
        if spec.program.is_empty() || !spec.cwd.is_absolute() || !spec.cwd.is_dir() {
            bail!("Terminal requires an executable and an existing absolute working directory");
        }
        let size = Size { cols: 80, rows: 24 };
        let pair = portable_pty::native_pty_system().openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })?;
        let mut command = CommandBuilder::new(&spec.program);
        command.args(&spec.args);
        command.cwd(&spec.cwd);
        command.env("TERM", "xterm-256color");
        command.env("COLORTERM", "truecolor");
        command.env("TERM_PROGRAM", "Buckets");
        // Finder-launched applications need the usual developer executable paths.
        let path = std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin:/usr/sbin:/sbin".into());
        command.env("PATH", format!("/opt/homebrew/bin:/usr/local/bin:{path}"));
        for (name, value) in &spec.env {
            command.env(name, value);
        }
        let child = pair
            .slave
            .spawn_command(command)
            .with_context(|| format!("Launch {}", spec.program))?;
        let mut child = SpawnGuard(Some(child));
        let killer = child.0.as_ref().unwrap().clone_killer();
        let pid = child.0.as_ref().unwrap().process_id();
        drop(pair.slave);
        #[cfg(unix)]
        let mut reader = {
            use std::os::fd::FromRawFd;
            let fd = pair
                .master
                .as_raw_fd()
                .context("PTY has no readable descriptor")?;
            let duplicate = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 0) };
            if duplicate < 0 {
                return Err(std::io::Error::last_os_error().into());
            }
            // This duplicate is owned exclusively by the output thread.
            unsafe { std::fs::File::from_raw_fd(duplicate) }
        };
        #[cfg(not(unix))]
        let mut reader = pair.master.try_clone_reader()?;
        let mut writer = pair.master.take_writer()?;
        let (tx, rx) = mpsc::sync_channel(128);
        let proxy = Proxy {
            input: tx,
            events: Arc::new(Mutex::new(Vec::new())),
            size: Arc::new(Mutex::new(size)),
            appearance: Arc::new(Mutex::new(TerminalAppearance::default())),
            color_requests: Arc::new(Mutex::new(Vec::new())),
        };
        let config = Config {
            scrolling_history: 10_000,
            kitty_keyboard: false,
            ..Config::default()
        };
        let shared = Arc::new(Shared {
            term: Mutex::new(Term::new(config, &size, proxy.clone())),
            revision: AtomicU64::new(1),
            exited: AtomicBool::new(false),
            closing: AtomicBool::new(false),
            cleanup: Mutex::new(()),
            exit_status: Mutex::new(None),
            proxy,
        });
        thread::Builder::new()
            .name("terminal-input".into())
            .spawn(move || {
                while let Ok(Input::Bytes(bytes)) = rx.recv() {
                    if writer
                        .write_all(&bytes)
                        .and_then(|_| writer.flush())
                        .is_err()
                    {
                        break;
                    }
                }
            })?;
        let read_shared = shared.clone();
        thread::Builder::new()
            .name("terminal-output".into())
            .spawn(move || {
                let mut parser: Processor = Processor::new();
                let mut bytes = [0; 65536];
                loop {
                    if read_shared.closing.load(Ordering::Acquire) {
                        break;
                    }
                    #[cfg(unix)]
                    {
                        use std::os::fd::AsRawFd;
                        let mut descriptor = libc::pollfd {
                            fd: reader.as_raw_fd(),
                            events: libc::POLLIN,
                            revents: 0,
                        };
                        let result = unsafe { libc::poll(&mut descriptor, 1, 50) };
                        if result < 0 {
                            if std::io::Error::last_os_error().kind()
                                == std::io::ErrorKind::Interrupted
                            {
                                continue;
                            }
                            break;
                        }
                        if result == 0 {
                            if parser
                                .sync_timeout()
                                .sync_timeout()
                                .is_some_and(|deadline| deadline <= std::time::Instant::now())
                            {
                                parser.stop_sync(&mut *read_shared.term.lock());
                                read_shared.revision.fetch_add(1, Ordering::Release);
                            }
                            continue;
                        }
                    }
                    match reader.read(&mut bytes) {
                        Ok(0) => break,
                        Ok(n) => {
                            let mut term = read_shared.term.lock();
                            parser.advance(&mut *term, &bytes[..n]);
                            read_shared.proxy.answer_color_requests(&term);
                            drop(term);
                            read_shared.revision.fetch_add(1, Ordering::Release);
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                        Err(_) => break,
                    }
                }
                read_shared.revision.fetch_add(1, Ordering::Release);
            })?;
        let wait_shared = shared.clone();
        thread::Builder::new()
            .name("terminal-wait".into())
            .spawn(move || {
                *wait_shared.exit_status.lock() = Some(match child.0.as_mut().unwrap().wait() {
                    Ok(status) => status.to_string(),
                    Err(e) => e.to_string(),
                });
                child.0.take();
                let _cleanup = wait_shared.cleanup.lock();
                // Clean jobs immediately when their leader exits. Never retain a stale
                // numeric session ID for cleanup after an exited pane has sat open.
                #[cfg(unix)]
                if !wait_shared.closing.load(Ordering::Acquire)
                    && let Some(pid) = pid
                {
                    terminate_session_jobs(pid as i32);
                }
                wait_shared.exited.store(true, Ordering::Release);
                wait_shared.revision.fetch_add(1, Ordering::Release);
            })?;
        Ok(Self {
            shared,
            master: Mutex::new(pair.master),
            killer: Mutex::new(killer),
            pid,
            spec,
            closed: AtomicBool::new(false),
        })
    }
    pub fn title(&self) -> &str {
        &self.spec.title
    }
    pub fn cwd(&self) -> &Path {
        &self.spec.cwd
    }
    pub fn process_id(&self) -> Option<u32> {
        self.pid
    }
    pub fn exited(&self) -> bool {
        self.shared.exited.load(Ordering::Acquire)
    }
    pub fn exit_status(&self) -> Option<String> {
        self.shared.exit_status.lock().clone()
    }
    pub fn write(&self, bytes: impl AsRef<[u8]>) -> Result<()> {
        if self.closed.load(Ordering::Acquire) || self.exited() {
            bail!("Terminal process has exited");
        }
        if bytes.as_ref().len() > 4 * 1024 * 1024 {
            bail!("Terminal input exceeds 4 MiB; paste smaller chunks");
        }
        self.shared
            .proxy
            .input
            .try_send(Input::Bytes(bytes.as_ref().to_vec()))
            .context("Terminal input queue full or closed")
    }
    pub fn paste(&self, text: &str) -> Result<()> {
        let bracketed = self
            .shared
            .term
            .lock()
            .mode()
            .contains(TermMode::BRACKETED_PASTE);
        // Embedded ESC in paste must not terminate bracketed paste and inject commands.
        let safe = text.replace('\x1b', "");
        if bracketed {
            self.write(format!("\x1b[200~{safe}\x1b[201~"))
        } else {
            self.write(safe.replace("\r\n", "\n").replace('\n', "\r"))
        }
    }
    pub fn resize(&self, cols: usize, rows: usize) -> Result<()> {
        let size = Size {
            cols: cols.clamp(2, 1000),
            rows: rows.clamp(1, 500),
        };
        let old = *self.shared.proxy.size.lock();
        if old.cols == size.cols && old.rows == size.rows {
            return Ok(());
        }
        // Resize the model before SIGWINCH can trigger a full-screen redraw.
        self.shared.term.lock().resize(size);
        self.master.lock().resize(PtySize {
            rows: size.rows as u16,
            cols: size.cols as u16,
            pixel_width: 0,
            pixel_height: 0,
        })?;
        *self.shared.proxy.size.lock() = size;
        self.shared.revision.fetch_add(1, Ordering::Release);
        Ok(())
    }
    pub fn screen_text(&self) -> String {
        let term = self.shared.term.lock();
        let mut text = String::new();
        for line in 0..term.screen_lines() {
            for col in 0..term.columns() {
                text.push(
                    term.grid()[alacritty_terminal::index::Point::new(
                        alacritty_terminal::index::Line(line as i32),
                        alacritty_terminal::index::Column(col),
                    )]
                    .c,
                );
            }
            text.push('\n');
        }
        text
    }
    pub fn scroll(&self, lines: i32) {
        self.shared.term.lock().scroll_display(Scroll::Delta(lines));
        self.shared.revision.fetch_add(1, Ordering::Release);
    }
    pub fn close(&self) {
        if self.closed.swap(true, Ordering::AcqRel) {
            return;
        }
        self.shared.closing.store(true, Ordering::Release);
        {
            let _cleanup = self.shared.cleanup.lock();
            #[cfg(unix)]
            {
                if !self.exited()
                    && let Some(pid) = self.pid
                {
                    terminate_session_jobs(pid as i32);
                }
                let master = self.master.lock();
                if let Some(fd) = master.as_raw_fd() {
                    // A shell may put a TUI in its own foreground process group.
                    let foreground = unsafe { libc::tcgetpgrp(fd) };
                    if foreground > 0 && foreground != unsafe { libc::getpgrp() } {
                        unsafe {
                            libc::kill(-foreground, libc::SIGKILL);
                        }
                    }
                }
                if !self.exited()
                    && let Some(pid) = self.pid
                {
                    unsafe {
                        libc::kill(-(pid as i32), libc::SIGKILL);
                    }
                }
            }
            if !self.exited() {
                let _ = self.killer.lock().kill();
            }
        }
        let _ = self.shared.proxy.input.try_send(Input::Close);
    }
}
impl Drop for TerminalSession {
    fn drop(&mut self) {
        self.close();
    }
}

// Job-control shells create several process groups inside one PTY session.
// Restrict cleanup to that session so a background job cannot outlive a closed pane.
#[cfg(unix)]
fn terminate_session_jobs(session: i32) {
    #[cfg(target_os = "macos")]
    let pids = {
        let count = unsafe { libc::proc_listallpids(std::ptr::null_mut(), 0) };
        let mut pids = vec![0i32; count.max(0) as usize + 256];
        let count = unsafe {
            libc::proc_listallpids(
                pids.as_mut_ptr().cast(),
                (pids.len() * std::mem::size_of::<i32>()) as i32,
            )
        };
        pids.truncate(count.max(0) as usize);
        pids
    };
    #[cfg(target_os = "linux")]
    let pids: Vec<i32> = std::fs::read_dir("/proc")
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| entry.file_name().to_str()?.parse().ok())
        .collect();
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    let pids: Vec<i32> = vec![];
    for pid in pids {
        if pid > 0 && pid != unsafe { libc::getpid() } && unsafe { libc::getsid(pid) } == session {
            unsafe {
                libc::kill(pid, libc::SIGKILL);
            }
        }
    }
}

// Any failure between spawning the child and handing it to the wait thread must reap it.
struct SpawnGuard(Option<Box<dyn portable_pty::Child + Send + Sync>>);
impl Drop for SpawnGuard {
    fn drop(&mut self) {
        if let Some(child) = self.0.as_mut() {
            #[cfg(unix)]
            if let Some(pid) = child.process_id() {
                terminate_session_jobs(pid as i32);
            }
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

pub(crate) fn palette(index: usize) -> u32 {
    const BASE: [u32; 16] = [
        0x15161e, 0xf7768e, 0x9ece6a, 0xe0af68, 0x7aa2f7, 0xbb9af7, 0x7dcfff, 0xa9b1d6, 0x414868,
        0xf7768e, 0x9ece6a, 0xe0af68, 0x7aa2f7, 0xbb9af7, 0x7dcfff, 0xc0caf5,
    ];
    match index {
        0..=15 => BASE[index],
        16..=231 => {
            let n = index - 16;
            let component = |n: usize| if n == 0 { 0 } else { 55 + 40 * n as u32 };
            (component(n / 36) << 16) | (component((n / 6) % 6) << 8) | component(n % 6)
        }
        232..=255 => {
            let v = 8 + 10 * (index as u32 - 232);
            (v << 16) | (v << 8) | v
        }
        257 => 0x1a1b26,
        _ => 0xc0caf5,
    }
}

#[cfg(test)]
mod appearance_tests {
    use super::*;

    #[test]
    fn osc_queries_report_configured_and_program_overridden_colors() {
        let (tx, rx) = mpsc::sync_channel(128);
        let size = Size { cols: 80, rows: 24 };
        let proxy = Proxy {
            input: tx,
            events: Arc::new(Mutex::new(Vec::new())),
            size: Arc::new(Mutex::new(size)),
            appearance: Arc::new(Mutex::new(TerminalAppearance {
                foreground: 0x123456,
                ..Default::default()
            })),
            color_requests: Arc::new(Mutex::new(Vec::new())),
        };
        let mut term = Term::new(Config::default(), &size, proxy.clone());
        let mut parser: Processor = Processor::new();
        parser.advance(&mut term, b"\x1b]10;?\x07");
        proxy.answer_color_requests(&term);
        let Input::Bytes(bytes) = rx.try_recv().unwrap() else {
            panic!("missing query response")
        };
        assert_eq!(bytes, b"\x1b]10;rgb:1212/3434/5656\x07");
        parser.advance(&mut term, b"\x1b]11;#abcdef\x07\x1b]11;?\x07");
        proxy.answer_color_requests(&term);
        let Input::Bytes(bytes) = rx.try_recv().unwrap() else {
            panic!("missing query response")
        };
        assert_eq!(bytes, b"\x1b]11;rgb:abab/cdcd/efef\x07");
    }
}
