#![allow(dead_code)]

use revenant_sdk::{ResourceRegistry, TaskContext, TaskManager};
use std::{
    future::Future,
    pin::Pin,
    sync::mpsc::{self, Receiver, Sender},
    task::{Context, Poll, Wake, Waker},
    thread,
};

pub struct Gate {
    pub started: Sender<()>,
    pub release: Receiver<()>,
}

pub fn gate() -> (Gate, Receiver<()>, Sender<()>) {
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    (
        Gate {
            started: started_tx,
            release: release_rx,
        },
        started_rx,
        release_tx,
    )
}

struct ThreadWake(thread::Thread);

impl Wake for ThreadWake {
    fn wake(self: std::sync::Arc<Self>) {
        self.0.unpark();
    }

    fn wake_by_ref(self: &std::sync::Arc<Self>) {
        self.0.unpark();
    }
}

pub fn block_on<F: Future>(future: F) -> F::Output {
    let waker = Waker::from(std::sync::Arc::new(ThreadWake(thread::current())));
    let mut context = Context::from_waker(&waker);
    let mut future = Box::pin(future);
    loop {
        match Pin::as_mut(&mut future).poll(&mut context) {
            Poll::Ready(output) => return output,
            Poll::Pending => thread::park(),
        }
    }
}

pub fn fresh_task(task_id: &str) -> (ResourceRegistry, TaskManager, TaskContext) {
    let resources = ResourceRegistry::new();
    resources.create_scope("test-scope", None).unwrap();
    let tasks = TaskManager::new(resources.clone());
    let context = tasks.create("test-scope", task_id).unwrap();
    (resources, tasks, context)
}
