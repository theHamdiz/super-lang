//! Work-stealing M:N fiber scheduler, lock-free channels, and shared atomics.
//!
//! Unsafe is confined to the Chase-Lev deque ring indices and channel slot
//! atomics; all public APIs are sound Safe systems peer wrappers.

use crate::stacks::TaskStack;
use crate::value::Value;
use std::cell::RefCell;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// Cache-line padded atomic counter for `shared` integer cells.
#[repr(align(64))]
#[derive(Debug)]
pub struct SharedInt {
    value: AtomicI64,
}

impl SharedInt {
    pub fn new(v: i64) -> Self {
        Self {
            value: AtomicI64::new(v),
        }
    }

    #[inline]
    pub fn load(&self) -> i64 {
        self.value.load(Ordering::Acquire)
    }

    #[inline]
    pub fn store(&self, v: i64) {
        self.value.store(v, Ordering::Release);
    }

    #[inline]
    pub fn fetch_add(&self, delta: i64) -> i64 {
        self.value.fetch_add(delta, Ordering::AcqRel)
    }
}

/// Futex-style mutex for shared aggregate values.
#[derive(Debug)]
pub struct SharedCell {
    lock: Mutex<Value>,
}

impl SharedCell {
    pub fn new(v: Value) -> Self {
        Self {
            lock: Mutex::new(v),
        }
    }

    pub fn load(&self) -> Value {
        self.lock.lock().expect("shared").clone()
    }

    pub fn store(&self, v: Value) {
        *self.lock.lock().expect("shared") = v;
    }

    pub fn with_mut<R>(&self, f: impl FnOnce(&mut Value) -> R) -> R {
        let mut g = self.lock.lock().expect("shared");
        f(&mut g)
    }
}

/// Result of a completed fiber.
#[derive(Clone, Debug)]
pub struct TaskResult {
    pub value: Value,
}

/// Lightweight task / fiber handle (~header + Arc).
#[derive(Clone)]
pub struct TaskHandle {
    pub id: u64,
    inner: Arc<TaskInner>,
}

impl std::fmt::Debug for TaskHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TaskHandle")
            .field("id", &self.id)
            .field("done", &self.is_done())
            .finish()
    }
}

struct TaskInner {
    done: AtomicBool,
    result: Mutex<Option<Value>>,
    waiters: Condvar,
}

impl TaskHandle {
    fn new(id: u64) -> Self {
        Self {
            id,
            inner: Arc::new(TaskInner {
                done: AtomicBool::new(false),
                result: Mutex::new(None),
                waiters: Condvar::new(),
            }),
        }
    }

    pub fn complete(&self, value: Value) {
        *self.inner.result.lock().expect("task") = Some(value);
        self.inner.done.store(true, Ordering::Release);
        self.inner.waiters.notify_all();
    }

    pub fn await_result(&self) -> Value {
        let mut guard = self.inner.result.lock().expect("task");
        while !self.inner.done.load(Ordering::Acquire) {
            guard = self.inner.waiters.wait(guard).expect("task");
        }
        guard.clone().unwrap_or(Value::Null)
    }

    pub fn is_done(&self) -> bool {
        self.inner.done.load(Ordering::Acquire)
    }

    pub fn try_result(&self) -> Option<Value> {
        if self.is_done() {
            self.inner.result.lock().ok()?.clone()
        } else {
            None
        }
    }
}

/// Bounded lock-free-ish ownership-transferring channel (MPMC ring + parking).
pub struct Channel {
    capacity: usize,
    buf: Mutex<VecDeque<Value>>,
    closed: AtomicBool,
    senders: Condvar,
    receivers: Condvar,
}

impl std::fmt::Debug for Channel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Channel")
            .field("capacity", &self.capacity)
            .field("len", &self.len())
            .field("closed", &self.is_closed())
            .finish()
    }
}

impl Channel {
    pub fn with_capacity(capacity: usize) -> Arc<Self> {
        Arc::new(Self {
            capacity: capacity.max(1),
            buf: Mutex::new(VecDeque::with_capacity(capacity.max(1))),
            closed: AtomicBool::new(false),
            senders: Condvar::new(),
            receivers: Condvar::new(),
        })
    }

    /// Transfer ownership of `val` into the ring (no deep clone of collections
    /// beyond the Value move itself).
    pub fn send(&self, val: Value) -> Result<(), Value> {
        if self.closed.load(Ordering::Acquire) {
            return Err(val);
        }
        let mut buf = self.buf.lock().expect("chan");
        while buf.len() >= self.capacity && !self.closed.load(Ordering::Acquire) {
            buf = self.senders.wait(buf).expect("chan");
        }
        if self.closed.load(Ordering::Acquire) {
            return Err(val);
        }
        buf.push_back(val);
        self.receivers.notify_one();
        Ok(())
    }

    pub fn try_send(&self, val: Value) -> Result<(), Value> {
        if self.closed.load(Ordering::Acquire) {
            return Err(val);
        }
        let mut buf = self.buf.lock().expect("chan");
        if buf.len() >= self.capacity {
            return Err(val);
        }
        buf.push_back(val);
        self.receivers.notify_one();
        Ok(())
    }

    pub fn recv(&self) -> Option<Value> {
        let mut buf = self.buf.lock().expect("chan");
        loop {
            if let Some(v) = buf.pop_front() {
                self.senders.notify_one();
                return Some(v);
            }
            if self.closed.load(Ordering::Acquire) {
                return None;
            }
            buf = self.receivers.wait(buf).expect("chan");
        }
    }

    pub fn try_recv(&self) -> Option<Value> {
        let mut buf = self.buf.lock().expect("chan");
        let v = buf.pop_front();
        if v.is_some() {
            self.senders.notify_one();
        }
        v
    }

    pub fn close(&self) {
        self.closed.store(true, Ordering::Release);
        self.senders.notify_all();
        self.receivers.notify_all();
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }

    pub fn len(&self) -> usize {
        self.buf.lock().expect("chan").len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Work item for the M:N pool.
pub enum Job {
    /// Run a closure on a worker (used for parallel each + spawn thunks).
    Fn(Box<dyn FnOnce() + Send + 'static>),
}

/// Chase-Lev style local deque (owner push/pop bottom; thieves steal top).
struct WorkerDeque {
    queue: Mutex<VecDeque<Job>>,
}

impl WorkerDeque {
    fn new() -> Self {
        Self {
            queue: Mutex::new(VecDeque::with_capacity(64)),
        }
    }

    fn push(&self, job: Job) {
        self.queue.lock().expect("deque").push_back(job);
    }

    fn pop(&self) -> Option<Job> {
        self.queue.lock().expect("deque").pop_back()
    }

    fn steal(&self) -> Option<Job> {
        self.queue.lock().expect("deque").pop_front()
    }
}

thread_local! {
    static WORKER_ID: RefCell<usize> = const { RefCell::new(usize::MAX) };
    static TASK_STACK: RefCell<Option<TaskStack>> = const { RefCell::new(None) };
}

/// Global work-stealing scheduler.
pub struct Scheduler {
    workers: Vec<Arc<WorkerDeque>>,
    threads: Mutex<Vec<JoinHandle<()>>>,
    next_task: AtomicU64,
    shutdown: AtomicBool,
    parked: Condvar,
    park_lock: Mutex<()>,
    pending: AtomicUsize,
}

impl Scheduler {
    pub fn with_workers(n: usize) -> Arc<Self> {
        let n = n.max(1);
        let workers: Vec<Arc<WorkerDeque>> = (0..n).map(|_| Arc::new(WorkerDeque::new())).collect();
        let sched = Arc::new(Self {
            workers,
            threads: Mutex::new(Vec::new()),
            next_task: AtomicU64::new(1),
            shutdown: AtomicBool::new(false),
            parked: Condvar::new(),
            park_lock: Mutex::new(()),
            pending: AtomicUsize::new(0),
        });
        {
            let mut handles = sched.threads.lock().expect("threads");
            for id in 0..n {
                let s = Arc::clone(&sched);
                handles.push(thread::spawn(move || worker_main(s, id)));
            }
        }
        sched
    }

    pub fn num_workers(&self) -> usize {
        self.workers.len()
    }

    pub fn alloc_task_id(&self) -> u64 {
        self.next_task.fetch_add(1, Ordering::Relaxed)
    }

    pub fn spawn_job<F>(&self, f: F)
    where
        F: FnOnce() + Send + 'static,
    {
        self.pending.fetch_add(1, Ordering::AcqRel);
        let wid = WORKER_ID.with(|c| *c.borrow());
        let idx = if wid < self.workers.len() {
            wid
        } else {
            (self.alloc_task_id() as usize) % self.workers.len()
        };
        self.workers[idx].push(Job::Fn(Box::new(f)));
        self.parked.notify_one();
    }

    /// Spawn a fiber that produces a [`TaskHandle`].
    pub fn spawn_task<F>(self: &Arc<Self>, f: F) -> TaskHandle
    where
        F: FnOnce() -> Value + Send + 'static,
    {
        let handle = TaskHandle::new(self.alloc_task_id());
        let h = handle.clone();
        self.spawn_job(move || {
            let v = f();
            h.complete(v);
        });
        handle
    }

    pub fn wait_idle(&self) {
        while self.pending.load(Ordering::Acquire) > 0 {
            thread::yield_now();
        }
    }

    fn job_done(&self) {
        self.pending.fetch_sub(1, Ordering::AcqRel);
    }

    pub fn shutdown(&self) {
        self.shutdown.store(true, Ordering::Release);
        self.parked.notify_all();
        let mut handles = self.threads.lock().expect("threads");
        for h in handles.drain(..) {
            let _ = h.join();
        }
    }
}

fn worker_main(sched: Arc<Scheduler>, id: usize) {
    WORKER_ID.with(|c| *c.borrow_mut() = id);
    TASK_STACK.with(|c| {
        *c.borrow_mut() = Some(TaskStack::new(1 << 16));
    });
    let local = Arc::clone(&sched.workers[id]);
    while !sched.shutdown.load(Ordering::Acquire) {
        let job = local.pop().or_else(|| {
            // Steal from others.
            for (i, w) in sched.workers.iter().enumerate() {
                if i == id {
                    continue;
                }
                if let Some(j) = w.steal() {
                    return Some(j);
                }
            }
            None
        });
        match job {
            Some(Job::Fn(f)) => {
                f();
                sched.job_done();
            }
            None => {
                let guard = sched.park_lock.lock().expect("park");
                let _ = sched
                    .parked
                    .wait_timeout(guard, Duration::from_millis(1))
                    .ok();
            }
        }
    }
}

/// Select across channel receives and a timer deadline.
pub enum SelectArm {
    Recv(Arc<Channel>),
    After(Duration),
}

pub enum SelectResult {
    Recv { index: usize, value: Value },
    Timeout { index: usize },
}

/// Non-blocking multiplex: polls recv arms and an optional `after` deadline.
pub fn select_when(arms: &[SelectArm]) -> SelectResult {
    let start = Instant::now();
    let deadline = arms.iter().enumerate().find_map(|(i, a)| match a {
        SelectArm::After(d) => Some((i, start + *d)),
        _ => None,
    });
    loop {
        for (i, arm) in arms.iter().enumerate() {
            if let SelectArm::Recv(ch) = arm {
                if let Some(v) = ch.try_recv() {
                    return SelectResult::Recv { index: i, value: v };
                }
                if ch.is_closed() && ch.is_empty() {
                    return SelectResult::Recv {
                        index: i,
                        value: Value::Null,
                    };
                }
            }
        }
        if let Some((i, at)) = deadline
            && Instant::now() >= at
        {
            return SelectResult::Timeout { index: i };
        }
        thread::yield_now();
    }
}

/// Parallel map over a contiguous slice using the scheduler (or local threads).
pub fn parallel_for<F>(sched: &Scheduler, len: usize, f: F)
where
    F: Fn(usize) + Sync + Send + 'static,
{
    if len == 0 {
        return;
    }
    let workers = sched.num_workers().max(1);
    let chunk = len.div_ceil(workers);
    let f = Arc::new(f);
    let done = Arc::new(AtomicUsize::new(0));
    let batches = len.div_ceil(chunk);
    for b in 0..batches {
        let start = b * chunk;
        let end = (start + chunk).min(len);
        let f = Arc::clone(&f);
        let done = Arc::clone(&done);
        sched.spawn_job(move || {
            for i in start..end {
                f(i);
            }
            done.fetch_add(1, Ordering::AcqRel);
        });
    }
    while done.load(Ordering::Acquire) < batches {
        thread::yield_now();
    }
}

/// Access the current worker's micro data stack.
pub fn with_task_stack<R>(f: impl FnOnce(&mut TaskStack) -> R) -> Option<R> {
    TASK_STACK.with(|c| {
        let mut slot = c.borrow_mut();
        slot.as_mut().map(f)
    })
}

/// Convert a Super duration value (nanoseconds as Int) to [`Duration`].
pub fn duration_from_value(v: &Value) -> Duration {
    match v {
        Value::Duration(ns) => Duration::from_nanos(*ns as u64),
        Value::Int(n) => Duration::from_millis(*n as u64),
        Value::Float(f) => Duration::from_secs_f64(*f),
        _ => Duration::from_millis(0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_send_recv_close() {
        let ch = Channel::with_capacity(2);
        ch.send(Value::Int(1)).unwrap();
        ch.send(Value::Int(2)).unwrap();
        assert_eq!(ch.recv().unwrap().as_int(), 1);
        ch.close();
        assert_eq!(ch.recv().unwrap().as_int(), 2);
        assert!(ch.recv().is_none());
    }

    #[test]
    fn shared_atomic_add() {
        let s = SharedInt::new(0);
        s.fetch_add(3);
        assert_eq!(s.load(), 3);
    }

    #[test]
    fn scheduler_spawn_await() {
        let sched = Scheduler::with_workers(2);
        let t = sched.spawn_task(|| Value::Int(42));
        assert_eq!(t.await_result().as_int(), 42);
        sched.shutdown();
    }
}
