use std::cell::{Cell, RefCell, RefMut};

use async_task::Runnable;

thread_local!(
    static CONTEXT: Context = Context {
        state: Cell::new(State::Stopped),
        runnables: RefCell::new(Vec::new()),
    }
);

#[derive(Clone, Copy)]
enum State {
    Running,
    Stopping,
    Stopped,
}

pub struct Context {
    state: Cell<State>,
    runnables: RefCell<Vec<Runnable>>,
}

impl Context {
    pub fn schedule(runnable: Runnable) {
        CONTEXT.with(|context| match context.state.get() {
            State::Running => context.runnables.borrow_mut().push(runnable),
            State::Stopping => (),
            State::Stopped => panic!(
                "not within a schedwalk context, must be called from within a schedwalk context"
            ),
        })
    }

    pub fn init<R>(f: impl FnOnce(&Context) -> R) -> R {
        CONTEXT.with(|context| {
            assert!(
                matches!(context.state.get(), State::Stopped),
                "already within a schedwalk context, cannot start new context here"
            );

            context.state.set(State::Running);

            struct DropGuard<'a>(&'a Context);

            impl Drop for DropGuard<'_> {
                fn drop(&mut self) {
                    self.0.state.set(State::Stopping);
                    self.0.runnables.borrow_mut().clear();
                    self.0.state.set(State::Stopped);
                }
            }

            let _drop_guard = DropGuard(context);

            f(context)
        })
    }

    pub fn runnables(&self) -> RefMut<Vec<Runnable>> {
        self.runnables.borrow_mut()
    }
}
