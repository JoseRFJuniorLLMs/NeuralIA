//! O worker preguicoso (infra-egress, plano 2.3): uma thread com nome que so
//! nasce no primeiro trabalho, NUNCA no `App::new` -- a Home fica com as
//! threads e a RAM de sempre (measure-home).
//!
//! - Nasce no primeiro `submit`; `new` guarda so a funcao que fara o
//!   trabalho. Depois da primeira vez e sempre a mesma thread.
//! - Uma so vaga, a do mais recente: um pedido novo tira da vaga o que ainda
//!   esperava (os trabalhos sao "o estado mais recente", nao uma fila).
//! - Geracao: cada `submit` (e cada `cancel`) sobe a geracao; o trabalho em
//!   curso ve `JobContext::cancelled()` e para quando lhe convem, e o
//!   resultado de uma geracao velha pode ser deitado fora por quem o recebe.
//! - Largar o worker fecha-o: a thread ainda corre o que estava na vaga (a
//!   gravacao do consumo nao se perde; quem quer desistir chama `cancel`
//!   antes) e acaba. Ninguem espera por ela: a janela nunca fica presa num
//!   `join`.
//!
//! Portatil: testado tambem no runner Linux.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};

/// O que um trabalho sabe sobre si: a geracao e se ja foi ultrapassado.
pub(crate) struct JobContext {
    generation: u64,
    current: Arc<AtomicU64>,
}

impl JobContext {
    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }

    /// Veio um pedido mais novo ou um `cancel` depois deste trabalho.
    pub(crate) fn cancelled(&self) -> bool {
        self.current.load(Ordering::Acquire) != self.generation
    }
}

/// A thread nao arrancou (o sistema recusou-a). O trabalho perdeu-se e o
/// worker fica sem thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WorkerUnavailable;

struct State<Job> {
    slot: Option<(u64, Job)>,
    running: bool,
    closed: bool,
}

struct Shared<Job> {
    state: Mutex<State<Job>>,
    wake: Condvar,
    idle: Condvar,
}

impl<Job> Shared<Job> {
    fn lock(&self) -> MutexGuard<'_, State<Job>> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

type Handler<Job> = Box<dyn FnMut(Job, &JobContext) + Send>;

/// Uma thread com nome, criada no primeiro trabalho.
pub(crate) struct LazyWorker<Job: Send + 'static> {
    name: &'static str,
    shared: Arc<Shared<Job>>,
    generation: Arc<AtomicU64>,
    /// A funcao do trabalho, ate a thread nascer (entao vai com ela).
    handler: Option<Handler<Job>>,
    spawned: usize,
}

impl<Job: Send + 'static> LazyWorker<Job> {
    /// Guarda o nome da thread e a funcao do trabalho. Nao cria thread.
    pub(crate) fn new(
        name: &'static str,
        handler: impl FnMut(Job, &JobContext) + Send + 'static,
    ) -> Self {
        Self {
            name,
            shared: Arc::new(Shared {
                state: Mutex::new(State {
                    slot: None,
                    running: false,
                    closed: false,
                }),
                wake: Condvar::new(),
                idle: Condvar::new(),
            }),
            generation: Arc::new(AtomicU64::new(0)),
            handler: Some(Box::new(handler)),
            spawned: 0,
        }
    }

    /// Quantas threads este worker criou: 0 ate ao primeiro trabalho, 1
    /// depois, por muitos trabalhos que venham.
    pub(crate) fn threads_spawned(&self) -> usize {
        self.spawned
    }

    /// Poe o trabalho na vaga (tirando o que la esperava) e sobe a geracao;
    /// no primeiro, cria a thread. Devolve a geracao deste trabalho.
    pub(crate) fn submit(&mut self, job: Job) -> Result<u64, WorkerUnavailable> {
        if self.handler.is_none() && self.spawned == 0 {
            return Err(WorkerUnavailable);
        }
        let generation = self.generation.fetch_add(1, Ordering::AcqRel) + 1;
        self.shared.lock().slot = Some((generation, job));
        self.shared.wake.notify_one();
        if let Some(handler) = self.handler.take() {
            let shared = Arc::clone(&self.shared);
            let current = Arc::clone(&self.generation);
            let spawned = std::thread::Builder::new()
                .name(self.name.to_string())
                .spawn(move || run(shared, current, handler));
            if spawned.is_err() {
                self.shared.lock().slot = None;
                return Err(WorkerUnavailable);
            }
            self.spawned += 1;
        }
        Ok(generation)
    }

    /// Sobe a geracao e tira da vaga o que esperava: o trabalho em curso ve
    /// `cancelled()`, o que esperava ja nao corre.
    pub(crate) fn cancel(&self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
        self.shared.lock().slot = None;
        self.shared.idle.notify_all();
    }

    /// Espera (no maximo `timeout`) que a vaga fique vazia e nada corra.
    /// So nos testes: o produto nunca espera por um worker.
    #[cfg(test)]
    pub(crate) fn wait_idle(&self, timeout: std::time::Duration) -> bool {
        let state = self.shared.lock();
        let (state, _) = self
            .shared
            .idle
            .wait_timeout_while(state, timeout, |state| {
                state.slot.is_some() || state.running
            })
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.slot.is_none() && !state.running
    }
}

impl<Job: Send + 'static> Drop for LazyWorker<Job> {
    fn drop(&mut self) {
        let mut state = self.shared.lock();
        state.closed = true;
        drop(state);
        self.shared.wake.notify_all();
    }
}

fn run<Job>(shared: Arc<Shared<Job>>, current: Arc<AtomicU64>, mut handler: Handler<Job>) {
    loop {
        let (generation, job) = {
            let mut state = shared.lock();
            loop {
                if let Some(next) = state.slot.take() {
                    state.running = true;
                    break next;
                }
                if state.closed {
                    return;
                }
                state = shared
                    .wake
                    .wait(state)
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
            }
        };
        let context = JobContext {
            generation,
            current: Arc::clone(&current),
        };
        if !context.cancelled() {
            handler(job, &context);
        }
        shared.lock().running = false;
        shared.idle.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    const WAIT: Duration = Duration::from_secs(10);

    /// Gate (critico, infra-egress): o worker nao cria thread nenhuma ate ao
    /// primeiro trabalho, e depois cria UMA, por muitos trabalhos que
    /// venham -- a relacao, nao um numero de relogio. A thread tem o nome
    /// dado (e o que o measure-home e o gestor de tarefas veem).
    #[test]
    fn lazy_worker_spawns_nothing_until_first_job() {
        let (seen_tx, seen_rx) = mpsc::channel();
        let mut worker = LazyWorker::new("neural-lazy-test", move |job: u32, _: &JobContext| {
            let name = std::thread::current().name().map(str::to_string);
            let _ = seen_tx.send((job, name));
        });
        assert_eq!(worker.threads_spawned(), 0, "new() criou uma thread");
        worker.cancel();
        assert_eq!(worker.threads_spawned(), 0, "cancel() criou uma thread");
        assert!(seen_rx.try_recv().is_err());

        worker.submit(1).expect("primeiro");
        let (job, name) = seen_rx.recv_timeout(WAIT).expect("o primeiro corre");
        assert_eq!((job, name.as_deref()), (1, Some("neural-lazy-test")));
        assert_eq!(worker.threads_spawned(), 1);

        // Mais 50 trabalhos: a mesma thread, nenhuma nova.
        for job in 2..=51 {
            assert!(worker.wait_idle(WAIT));
            worker.submit(job).expect("seguinte");
        }
        assert!(worker.wait_idle(WAIT));
        assert_eq!(worker.threads_spawned(), 1, "um trabalho, uma thread");
        let names: Vec<_> = seen_rx.try_iter().map(|(_, name)| name).collect();
        assert_eq!(names.len(), 50);
        assert!(
            names
                .iter()
                .all(|name| name.as_deref() == Some("neural-lazy-test"))
        );
    }

    /// Uma vaga, a do mais recente; o trabalho em curso ve o cancelamento.
    #[test]
    fn latest_wins_and_generation_cancel() {
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel::<()>();
        let (done_tx, done_rx) = mpsc::channel();
        let mut worker = LazyWorker::new("neural-lazy-slot", move |job: u32, ctx: &JobContext| {
            if job == 1 {
                started_tx.send(()).expect("avisar");
                release_rx.recv().expect("esperar");
            }
            done_tx
                .send((job, ctx.generation(), ctx.cancelled()))
                .expect("fim");
        });
        let first = worker.submit(1).expect("1");
        started_rx.recv_timeout(WAIT).expect("o 1 comecou");
        // Com o 1 a correr, chegam 2, 3 e 4: so o 4 fica na vaga.
        worker.submit(2).expect("2");
        worker.submit(3).expect("3");
        let last = worker.submit(4).expect("4");
        release_tx.send(()).expect("soltar");
        assert!(worker.wait_idle(WAIT));
        let done: Vec<_> = done_rx.try_iter().collect();
        assert_eq!(
            done,
            vec![(1, first, true), (4, last, false)],
            "o 1 ve que foi ultrapassado; 2 e 3 nunca correm"
        );

        // `cancel` tira da vaga o que esperava e marca o que corre.
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel::<()>();
        let (done_tx, done_rx) = mpsc::channel();
        let mut worker =
            LazyWorker::new("neural-lazy-cancel", move |job: u32, ctx: &JobContext| {
                if job == 1 {
                    started_tx.send(()).expect("avisar");
                    release_rx.recv().expect("esperar");
                }
                done_tx.send((job, ctx.cancelled())).expect("fim");
            });
        worker.submit(1).expect("1");
        started_rx.recv_timeout(WAIT).expect("o 1 comecou");
        worker.submit(2).expect("2");
        worker.cancel();
        release_tx.send(()).expect("soltar");
        assert!(worker.wait_idle(WAIT));
        assert_eq!(done_rx.try_iter().collect::<Vec<_>>(), vec![(1, true)]);

        // Largar o worker nao deita fora o que esta na vaga: a thread
        // corre-o e so depois acaba (a gravacao do consumo nao se perde).
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel::<()>();
        let (done_tx, done_rx) = mpsc::channel();
        let mut worker = LazyWorker::new("neural-lazy-drop", move |job: u32, _: &JobContext| {
            if job == 1 {
                started_tx.send(()).expect("avisar");
                release_rx.recv().expect("esperar");
            }
            done_tx.send(job).expect("fim");
        });
        worker.submit(1).expect("1");
        started_rx.recv_timeout(WAIT).expect("o 1 comecou");
        worker.submit(2).expect("2");
        drop(worker);
        release_tx.send(()).expect("soltar");
        assert_eq!(done_rx.recv_timeout(WAIT), Ok(1));
        assert_eq!(done_rx.recv_timeout(WAIT), Ok(2));
        // E depois a thread acaba: o canal fecha.
        assert_eq!(
            done_rx.recv_timeout(WAIT),
            Err(mpsc::RecvTimeoutError::Disconnected)
        );
    }
}
