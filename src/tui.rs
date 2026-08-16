use std::io::{self, Stdout};
use std::panic::{self, PanicHookInfo};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

use crossterm::{
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};

pub type AppTerminal = Terminal<CrosstermBackend<Stdout>>;
type Hook = Box<dyn Fn(&PanicHookInfo<'_>) + Send + Sync + 'static>;

struct HookState {
    cleanup: Mutex<CleanupState>,
    hook_restored: AtomicBool,
    previous: Mutex<Option<Hook>>,
}

struct CleanupState {
    steps: [bool; 3],
    running: bool,
}

static SESSION_ACTIVE: AtomicBool = AtomicBool::new(false);
const PANIC_EXIT_CODE: i32 = 101;

pub struct TerminalSession {
    terminal: AppTerminal,
    stdout: Stdout,
    hook: Arc<HookState>,
}

impl TerminalSession {
    pub fn terminal_mut(&mut self) -> &mut AppTerminal {
        &mut self.terminal
    }

    pub fn restore(&mut self) -> io::Result<()> {
        let result = restore_once(&mut self.stdout, &self.hook.cleanup);
        if !std::thread::panicking() {
            self.restore_previous_hook();
            SESSION_ACTIVE.store(false, Ordering::Release);
        }
        result
    }

    fn restore_previous_hook(&self) {
        if std::thread::panicking() {
            return;
        }
        if self.hook.hook_restored.swap(true, Ordering::AcqRel) {
            return;
        }
        let _installed = panic::take_hook();
        if let Some(previous) = self.hook.previous.lock().unwrap().take() {
            panic::set_hook(previous);
        }
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        let _ = restore_once(&mut self.stdout, &self.hook.cleanup);
        if !std::thread::panicking() {
            self.restore_previous_hook();
            SESSION_ACTIVE.store(false, Ordering::Release);
        }
    }
}

pub fn enter() -> io::Result<TerminalSession> {
    if !claim_session() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "a terminal session is already active",
        ));
    }
    let mut stdout = io::stdout();
    if let Err(error) = enable_raw_mode() {
        SESSION_ACTIVE.store(false, Ordering::Release);
        return Err(error);
    }
    let hook = install_panic_hook();

    if let Err(error) = execute!(stdout, EnterAlternateScreen, crossterm::cursor::Hide) {
        let _ = restore_once(&mut stdout, &hook.cleanup);
        restore_hook(hook);
        SESSION_ACTIVE.store(false, Ordering::Release);
        return Err(error);
    }

    let terminal = match setup_with_restore(
        || Terminal::new(CrosstermBackend::new(stdout)),
        || {
            let mut stdout = io::stdout();
            restore_once(&mut stdout, &hook.cleanup)
        },
    ) {
        Ok(terminal) => terminal,
        Err(error) => {
            restore_hook(hook);
            SESSION_ACTIVE.store(false, Ordering::Release);
            return Err(error);
        }
    };

    Ok(TerminalSession {
        terminal,
        stdout: io::stdout(),
        hook,
    })
}

fn claim_session() -> bool {
    !SESSION_ACTIVE.swap(true, Ordering::AcqRel)
}

fn install_panic_hook() -> Arc<HookState> {
    let state = Arc::new(HookState {
        cleanup: Mutex::new(CleanupState {
            steps: [false; 3],
            running: false,
        }),
        hook_restored: AtomicBool::new(false),
        previous: Mutex::new(Some(panic::take_hook())),
    });
    let hook_state = Arc::clone(&state);
    panic::set_hook(Box::new(move |info| {
        let mut stdout = io::stdout();
        let _ = restore_once(&mut stdout, &hook_state.cleanup);
        if let Some(previous) = hook_state.previous.lock().unwrap().as_ref() {
            previous(info);
        }
        std::process::exit(PANIC_EXIT_CODE);
    }));
    state
}

fn restore_hook(state: Arc<HookState>) {
    if std::thread::panicking() {
        return;
    }
    let _installed = panic::take_hook();
    if let Some(previous) = state.previous.lock().unwrap().take() {
        panic::set_hook(previous);
    }
}

fn restore_once(stdout: &mut Stdout, state: &Mutex<CleanupState>) -> io::Result<()> {
    restore_with_steps(
        stdout,
        state,
        disable_raw_mode,
        |stdout| execute!(stdout, LeaveAlternateScreen),
        |stdout| execute!(stdout, crossterm::cursor::Show),
    )
}

fn restore_with_steps<Disable, Leave, Show>(
    stdout: &mut Stdout,
    state: &Mutex<CleanupState>,
    mut disable: Disable,
    mut leave: Leave,
    mut show: Show,
) -> io::Result<()>
where
    Disable: FnMut() -> io::Result<()>,
    Leave: FnMut(&mut Stdout) -> io::Result<()>,
    Show: FnMut(&mut Stdout) -> io::Result<()>,
{
    let mut cleanup = state.lock().unwrap();
    if cleanup.running || cleanup.steps == [true; 3] {
        return Ok(());
    }
    cleanup.running = true;
    let mut first_error = None;
    let mut run = |index, operation: &mut dyn FnMut() -> io::Result<()>| {
        if cleanup.steps[index] {
            return;
        }
        match operation() {
            Ok(()) => cleanup.steps[index] = true,
            Err(error) if first_error.is_none() => first_error = Some(error),
            Err(_) => {}
        }
    };
    run(0, &mut disable);
    run(1, &mut || leave(stdout));
    run(2, &mut || show(stdout));
    cleanup.running = false;
    first_error.map_or(Ok(()), Err)
}

fn setup_with_restore<Setup, Restore>(setup: Setup, restore: Restore) -> io::Result<AppTerminal>
where
    Setup: FnOnce() -> io::Result<AppTerminal>,
    Restore: FnOnce() -> io::Result<()>,
{
    match setup() {
        Ok(terminal) => Ok(terminal),
        Err(error) => {
            let _ = restore();
            Err(error)
        }
    }
}

#[cfg(test)]
fn restore_steps<Disable, Leave, Show>(
    stdout: &mut Stdout,
    mut disable: Disable,
    mut leave: Leave,
    mut show: Show,
) -> io::Result<()>
where
    Disable: FnMut() -> io::Result<()>,
    Leave: FnMut(&mut Stdout) -> io::Result<()>,
    Show: FnMut(&mut Stdout) -> io::Result<()>,
{
    let mut first_error = None;
    if let Err(error) = disable() {
        first_error = Some(error);
    }
    if let Err(error) = leave(stdout) {
        if first_error.is_none() {
            first_error = Some(error);
        }
    }
    if let Err(error) = show(stdout) {
        if first_error.is_none() {
            first_error = Some(error);
        }
    }
    first_error.map_or(Ok(()), Err)
}

#[cfg(test)]
mod tests {
    use super::{claim_session, restore_steps, setup_with_restore, CleanupState};
    use std::cell::RefCell;
    use std::io;
    use std::panic;
    use std::rc::Rc;
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    };

    static HOOK_TEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn restoration_attempts_every_step_and_returns_first_error() {
        let steps = Rc::new(RefCell::new(Vec::new()));
        let mut stdout = io::stdout();
        let result = restore_steps(
            &mut stdout,
            || {
                steps.borrow_mut().push("disable");
                Err(io::Error::other("disable"))
            },
            |_| {
                steps.borrow_mut().push("leave");
                Err(io::Error::other("leave"))
            },
            |_| {
                steps.borrow_mut().push("show");
                Err(io::Error::other("show"))
            },
        );
        assert_eq!(result.unwrap_err().to_string(), "disable");
        assert_eq!(*steps.borrow(), ["disable", "leave", "show"]);
    }

    #[test]
    fn only_one_session_can_claim_process_ownership() {
        super::SESSION_ACTIVE.store(false, Ordering::Release);
        assert!(claim_session());
        assert!(!claim_session());
        super::SESSION_ACTIVE.store(false, Ordering::Release);
    }

    #[test]
    fn failed_cleanup_step_is_retried() {
        let state = Mutex::new(CleanupState {
            steps: [false; 3],
            running: false,
        });
        let mut calls = 0;
        let mut stdout = io::stdout();
        let first = super::restore_with_steps(
            &mut stdout,
            &state,
            || Ok(()),
            |_| {
                calls += 1;
                Err(io::Error::other("temporary"))
            },
            |_| Ok(()),
        );
        assert!(first.is_err());
        let second = super::restore_with_steps(
            &mut stdout,
            &state,
            || Ok(()),
            |_| {
                calls += 1;
                Ok(())
            },
            |_| Ok(()),
        );
        assert!(second.is_ok());
        assert_eq!(calls, 2);
    }

    #[test]
    fn restoring_the_hook_returns_the_prior_hook() {
        let _guard = HOOK_TEST_LOCK.lock().unwrap();
        let original = panic::take_hook();
        let called = std::sync::Arc::new(AtomicBool::new(false));
        let called_by_hook = std::sync::Arc::clone(&called);
        panic::set_hook(Box::new(move |_| {
            called_by_hook.store(true, Ordering::Release);
        }));
        let state = super::install_panic_hook();
        super::restore_hook(state);
        let result = panic::catch_unwind(|| panic!("hook test"));
        assert!(result.is_err());
        assert!(called.load(Ordering::Acquire));
        let _ = panic::take_hook();
        panic::set_hook(original);
    }

    #[test]
    fn spawned_tokio_panic_terminates_process() {
        if std::env::var("COIN_TUI_BACKGROUND_PANIC_CHILD").as_deref() == Ok("1") {
            let _ = panic::take_hook();
            panic::set_hook(Box::new(|_| eprintln!("previous-hook-called")));
            super::SESSION_ACTIVE.store(true, Ordering::Release);
            let _state = super::install_panic_hook();
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            runtime.block_on(async {
                tokio::spawn(async { panic!("background panic") })
                    .await
                    .unwrap();
            });
        }

        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "tui::tests::spawned_tokio_panic_terminates_process",
                "--nocapture",
            ])
            .env("COIN_TUI_BACKGROUND_PANIC_CHILD", "1")
            .output()
            .unwrap();
        assert_eq!(status.status.code(), Some(super::PANIC_EXIT_CODE));
        assert!(String::from_utf8_lossy(&status.stderr).contains("previous-hook-called"));
    }

    #[test]
    fn setup_failure_runs_best_effort_cleanup() {
        let restored = Rc::new(RefCell::new(false));
        let restored_by_setup = Rc::clone(&restored);
        let result = setup_with_restore(
            || Err(io::Error::other("injected setup failure")),
            || {
                *restored_by_setup.borrow_mut() = true;
                Ok(())
            },
        );
        assert!(result.is_err());
        assert!(*restored.borrow());
    }
}
